use std::cmp::min;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use bytes::BytesMut;
use futures::task::AtomicWaker;
use futures::{AsyncRead, AsyncWrite};
use wasm_bindgen::{prelude::*, JsCast};
use web_sys::{MessageEvent, RtcDataChannel, RtcDataChannelState};

/// WebRTC data channels only support backpressure for reading in a limited way.
/// We have to check the `bufferedAmount` property and compare it to chosen constant.
/// Once we exceed the constant, we pause sending of data until we receive the `bufferedAmountLow` event.
///
/// As per spec, we limit the maximum amount to 16KB, see <https://www.rfc-editor.org/rfc/rfc8831.html#name-transferring-user-data-on-a>.
const MAX_BUFFER: usize = 16 * 1024;

/// [`DataChannel`] is a wrapper around around [`RtcDataChannel`] which implements [`AsyncRead`] and [`AsyncWrite`].

#[derive(Debug, Clone)]
pub(crate) struct DataChannel {
    /// The [RtcDataChannel] being wrapped.
    inner: RtcDataChannel,

    new_data_waker: Arc<AtomicWaker>,
    read_buffer: Arc<Mutex<BytesMut>>,

    /// Waker for when we are waiting for the DC to be opened.
    open_waker: Arc<AtomicWaker>,

    /// Waker for when we are waiting to write (again) to the DC because we previously exceeded the `MAX_BUFFERED_AMOUNT` threshold.
    write_waker: Arc<AtomicWaker>,

    /// Waker for when we are waiting for the DC to be closed.
    close_waker: Arc<AtomicWaker>,
}

impl DataChannel {
    pub(crate) fn new(inner: RtcDataChannel) -> Self {
        let open_waker = Arc::new(AtomicWaker::new());
        inner.set_onopen(Some(
            Closure::once_into_js({
                let open_waker = open_waker.clone();

                move || {
                    log::trace!("DataChannel opened");
                    open_waker.wake();
                }
            })
            .unchecked_ref(),
        ));

        let write_waker = Arc::new(AtomicWaker::new());
        inner.set_buffered_amount_low_threshold(0);
        inner.set_onbufferedamountlow(Some(
            Closure::once_into_js({
                let write_waker = write_waker.clone();

                move || {
                    log::trace!("DataChannel available for writing (again)");
                    write_waker.wake();
                }
            })
            .unchecked_ref(),
        ));

        let close_waker = Arc::new(AtomicWaker::new());
        inner.set_onclose(Some(
            Closure::once_into_js({
                let close_waker = close_waker.clone();

                move || {
                    log::trace!("DataChannel closed");
                    close_waker.wake();
                }
            })
            .unchecked_ref(),
        ));

        let new_data_waker = Arc::new(AtomicWaker::new());
        let read_buffer = Arc::new(Mutex::new(BytesMut::new())); // We purposely don't use `with_capacity` so we don't eagerly allocate `MAX_READ_BUFFER` per stream.

        let onmessage_callback = Closure::<dyn FnMut(_)>::new({
            let new_data_waker = new_data_waker.clone();
            let read_buffer = read_buffer.clone();

            move |ev: MessageEvent| {
                let data = js_sys::Uint8Array::new(&ev.data());

                let mut read_buffer = read_buffer.lock().unwrap();

                if read_buffer.len() + data.length() as usize >= MAX_BUFFER {
                    log::warn!(
                        "Remote is overloading us with messages, dropping {} bytes of data",
                        data.length()
                    );
                    return;
                }

                read_buffer.copy_from_slice(&data.to_vec());
                new_data_waker.wake();
            }
        })
        .into_js_value();
        inner.set_onmessage(Some(onmessage_callback.unchecked_ref()));

        Self {
            inner,
            new_data_waker,
            read_buffer,
            open_waker,
            write_waker,
            close_waker,
        }
    }

    /// Returns the [RtcDataChannelState] of the [RtcDataChannel]
    fn ready_state(&self) -> RtcDataChannelState {
        self.inner.ready_state()
    }

    /// Returns the current [RtcDataChannel] BufferedAmount
    fn buffered_amount(&self) -> usize {
        self.inner.buffered_amount() as usize
    }
}

impl AsyncRead for DataChannel {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        match self.ready_state() {
            RtcDataChannelState::Connecting => {
                self.open_waker.register(cx.waker());
                return Poll::Pending;
            }
            RtcDataChannelState::Closing | RtcDataChannelState::Closed => {
                return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()))
            }
            RtcDataChannelState::Open | RtcDataChannelState::__Nonexhaustive => {}
        }

        let mut read_buffer = self.read_buffer.lock().unwrap();

        if read_buffer.is_empty() {
            self.new_data_waker.register(cx.waker());
            return Poll::Pending;
        }

        // Ensure that we:
        // - at most return what the caller can read (`buf.len()`)
        // - at most what we have (`read_buffer.len()`)
        let split_index = min(buf.len(), read_buffer.len());

        let bytes_to_return = read_buffer.split_off(split_index);
        buf.copy_from_slice(&bytes_to_return);

        Poll::Ready(Ok(bytes_to_return.len()))
    }
}

impl AsyncWrite for DataChannel {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        debug_assert!(self.buffered_amount() <= MAX_BUFFER);
        let remaining_space = MAX_BUFFER - self.buffered_amount();

        if remaining_space == 0 {
            self.write_waker.register(cx.waker());
            return Poll::Pending;
        }

        let bytes_to_send = min(buf.len(), remaining_space);

        if self
            .inner
            .send_with_u8_array(&buf[..bytes_to_send])
            .is_err()
        {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }

        Poll::Ready(Ok(bytes_to_send))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.buffered_amount() == 0 {
            return Poll::Ready(Ok(()));
        }

        self.write_waker.register(cx.waker());
        Poll::Pending
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.ready_state() == RtcDataChannelState::Closed {
            return Poll::Ready(Ok(()));
        }

        if self.ready_state() != RtcDataChannelState::Closing {
            self.inner.close();
        }

        self.close_waker.register(cx.waker());
        Poll::Pending
    }
}
