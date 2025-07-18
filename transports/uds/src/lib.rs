// Copyright 2018 Parity Technologies (UK) Ltd.
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

//! Implementation of the libp2p `Transport` trait for Unix domain sockets.
//!
//! # Platform support
//!
//! This transport only works on Unix platforms.
//!
//! # Usage
//!
//! The `UdsConfig` transport supports multiaddresses of the form `/unix//tmp/foo`.
//!
//! The `UdsConfig` structs implements the `Transport` trait of the `core` library. See the
//! documentation of `core` and of libp2p in general to learn how to use the `Transport` trait.

#![cfg(all(unix, not(target_os = "emscripten"), feature = "tokio"))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use std::{
    collections::VecDeque,
    io,
    path::PathBuf,
    pin::Pin,
    task::{Context, Poll},
};

use futures::{
    future::{BoxFuture, Ready},
    prelude::*,
    stream::BoxStream,
};
use libp2p_core::{
    multiaddr::{Multiaddr, Protocol},
    transport::{DialOpts, ListenerId, TransportError, TransportEvent},
    Transport,
};

pub type Listener<T> = BoxStream<
    'static,
    Result<
        TransportEvent<<T as Transport>::ListenerUpgrade, <T as Transport>::Error>,
        Result<(), <T as Transport>::Error>,
    >,
>;

macro_rules! codegen {
    ($feature_name:expr, $uds_config:ident, $build_listener:expr, $unix_stream:ty, $($mut_or_not:tt)*) => {
        /// Represents the configuration for a Unix domain sockets transport capability for libp2p.
        pub struct $uds_config {
            listeners: VecDeque<(ListenerId, Listener<Self>)>,
        }

        impl $uds_config {
            /// Creates a new configuration object for Unix domain sockets.
            pub fn new() -> $uds_config {
                $uds_config {
                    listeners: VecDeque::new(),
                }
            }
        }

        impl Default for $uds_config {
            fn default() -> Self {
                Self::new()
            }
        }

        impl Transport for $uds_config {
            type Output = $unix_stream;
            type Error = io::Error;
            type ListenerUpgrade = Ready<Result<Self::Output, Self::Error>>;
            type Dial = BoxFuture<'static, Result<Self::Output, Self::Error>>;

            fn listen_on(
                &mut self,
                id: ListenerId,
                addr: Multiaddr,
            ) -> Result<(), TransportError<Self::Error>> {
                if let Ok(path) = multiaddr_to_path(&addr) {
                    #[allow(clippy::redundant_closure_call)]
                    let listener = $build_listener(path)
                        .map_err(Err)
                        .map_ok(move |listener| {
                            stream::once({
                                let addr = addr.clone();
                                async move {
                                    tracing::debug!(address=%addr, "Now listening on address");
                                    Ok(TransportEvent::NewAddress {
                                        listener_id: id,
                                        listen_addr: addr,
                                    })
                                }
                            })
                            .chain(stream::unfold(
                                listener,
                                move |listener| {
                                    let addr = addr.clone();
                                    async move {
                                        let event = match listener.accept().await {
                                            Ok((stream, _)) => {
                                                tracing::debug!(address=%addr, "incoming connection on address");
                                                TransportEvent::Incoming {
                                                    upgrade: future::ok(stream),
                                                    local_addr: addr.clone(),
                                                    send_back_addr: addr.clone(),
                                                    listener_id: id,
                                                }
                                            }
                                            Err(error) => TransportEvent::ListenerError {
                                                listener_id: id,
                                                error,
                                            },
                                        };
                                        Some((Ok(event), listener))
                                    }
                                },
                            ))
                        })
                        .try_flatten_stream()
                        .boxed();
                    self.listeners.push_back((id, listener));
                    Ok(())
                } else {
                    Err(TransportError::MultiaddrNotSupported(addr))
                }
            }

            fn remove_listener(&mut self, id: ListenerId) -> bool {
                if let Some(index) = self
                    .listeners
                    .iter()
                    .position(|(listener_id, _)| listener_id == &id)
                {
                    let listener_stream = self.listeners.get_mut(index).unwrap();
                    let report_closed_stream = stream::once(async { Err(Ok(())) }).boxed();
                    *listener_stream = (id, report_closed_stream);
                    true
                } else {
                    false
                }
            }

            fn dial(&mut self, addr: Multiaddr, _dial_opts: DialOpts) -> Result<Self::Dial, TransportError<Self::Error>> {
                // TODO: Should we dial at all?
                if let Ok(path) = multiaddr_to_path(&addr) {
                    tracing::debug!(address=%addr, "Dialing address");
                    Ok(async move { <$unix_stream>::connect(&path).await }.boxed())
                } else {
                    Err(TransportError::MultiaddrNotSupported(addr))
                }
            }

            fn poll(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<TransportEvent<Self::ListenerUpgrade, Self::Error>> {
                let mut remaining = self.listeners.len();
                while let Some((id, mut listener)) = self.listeners.pop_back() {
                    let event = match Stream::poll_next(Pin::new(&mut listener), cx) {
                        Poll::Pending => None,
                        Poll::Ready(None) => panic!("Alive listeners always have a sender."),
                        Poll::Ready(Some(Ok(event))) => Some(event),
                        Poll::Ready(Some(Err(reason))) => {
                            return Poll::Ready(TransportEvent::ListenerClosed {
                                listener_id: id,
                                reason,
                            })
                        }
                    };
                    self.listeners.push_front((id, listener));
                    if let Some(event) = event {
                        return Poll::Ready(event);
                    } else {
                        remaining -= 1;
                        if remaining == 0 {
                            break;
                        }
                    }
                }
                Poll::Pending
            }
        }
    };
}

#[cfg(feature = "tokio")]
codegen!(
    "tokio",
    TokioUdsConfig,
    |addr| async move { tokio::net::UnixListener::bind(&addr) },
    tokio::net::UnixStream,
);

/// Turns a `Multiaddr` containing a single `Unix` component into a path.
///
/// Also returns an error if the path is not absolute, as we don't want to dial/listen on relative
/// paths.
// This type of logic should probably be moved into the multiaddr package
fn multiaddr_to_path(addr: &Multiaddr) -> Result<PathBuf, ()> {
    let mut protocols = addr.iter();
    match protocols.next() {
        Some(Protocol::Unix(ref path)) => {
            let path = PathBuf::from(path.as_ref());
            if !path.is_absolute() {
                return Err(());
            }
            match protocols.next() {
                None | Some(Protocol::P2p(_)) => Ok(path),
                Some(_) => Err(()),
            }
        }
        _ => Err(()),
    }
}

#[cfg(all(test, feature = "tokio"))]
mod tests {
    use std::{borrow::Cow, path::Path};

    use futures::{channel::oneshot, prelude::*};
    use libp2p_core::{
        multiaddr::{Multiaddr, Protocol},
        transport::{DialOpts, ListenerId, PortUse},
        Endpoint, Transport,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::{multiaddr_to_path, TokioUdsConfig};

    #[test]
    fn multiaddr_to_path_conversion() {
        assert!(
            multiaddr_to_path(&"/ip4/127.0.0.1/udp/1234".parse::<Multiaddr>().unwrap()).is_err()
        );

        assert_eq!(
            multiaddr_to_path(&Multiaddr::from(Protocol::Unix("/tmp/foo".into()))),
            Ok(Path::new("/tmp/foo").to_owned())
        );
        assert_eq!(
            multiaddr_to_path(&Multiaddr::from(Protocol::Unix("/home/bar/baz".into()))),
            Ok(Path::new("/home/bar/baz").to_owned())
        );
    }

    #[tokio::test]
    async fn communicating_between_dialer_and_listener() {
        let temp_dir = tempfile::tempdir().unwrap();
        let socket = temp_dir.path().join("socket");
        let addr = Multiaddr::from(Protocol::Unix(Cow::Owned(
            socket.to_string_lossy().into_owned(),
        )));

        let (tx, rx) = oneshot::channel();

        let listener = async move {
            let mut transport = TokioUdsConfig::new().boxed();
            transport.listen_on(ListenerId::next(), addr).unwrap();

            let listen_addr = transport
                .select_next_some()
                .await
                .into_new_address()
                .expect("listen address");

            tx.send(listen_addr).unwrap();

            let (sock, _addr) = transport
                .select_next_some()
                .await
                .into_incoming()
                .expect("incoming stream");

            let mut sock = sock.await.unwrap();
            let mut buf = [0u8; 3];
            sock.read_exact(&mut buf).await.unwrap();
            assert_eq!(buf, [1, 2, 3]);
        };

        let dialer = async move {
            let mut uds = TokioUdsConfig::new();
            let addr = rx.await.unwrap();
            let mut socket = uds
                .dial(
                    addr,
                    DialOpts {
                        role: Endpoint::Dialer,
                        port_use: PortUse::Reuse,
                    },
                )
                .unwrap()
                .await
                .unwrap();
            let _ = socket.write(&[1, 2, 3]).await.unwrap();
        };

        tokio::join!(listener, dialer);
    }

    #[test]
    #[ignore] // TODO: for the moment unix addresses fail to parse
    fn larger_addr_denied() {
        let mut uds = TokioUdsConfig::new();

        let addr = "/unix//foo/bar".parse::<Multiaddr>().unwrap();
        assert!(uds.listen_on(ListenerId::next(), addr).is_err());
    }

    #[test]
    #[ignore] // TODO: for the moment unix addresses fail to parse
    fn relative_addr_denied() {
        assert!("/unix/./foo/bar".parse::<Multiaddr>().is_err());
    }
}
