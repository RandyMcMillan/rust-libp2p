use std::{error::Error, path::PathBuf, str::FromStr};

use base64::Engine;
use clap::Parser;
use futures::stream::StreamExt;
use std::net::Ipv4Addr;

use libp2p::{
    core::{multiaddr::Protocol, Multiaddr},
    identify, identity,
    identity::PeerId,
    kad,
    metrics::{Metrics, Recorder},
    noise, ping, relay,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux,
};
use prometheus_client::{metrics::info::Info, registry::Registry};
use tracing_subscriber::EnvFilter;
use zeroize::Zeroizing;

mod behaviour;
mod config;
mod http_service;

#[derive(Debug, Parser)]
#[clap(name = "libp2p server", about = "A rust-libp2p server binary.")]
struct Opts {
    /// Path to IPFS config file.
    #[clap(long, short, default_value = "./config.json")]
    config: PathBuf,

    /// Metric endpoint path.
    #[clap(long, default_value = "/metrics")]
    metrics_path: String,

    /// Whether to run the libp2p Kademlia protocol and join the IPFS DHT.
    #[clap(long, short = 'k', default_value = "false")]
    enable_kademlia: bool,

    /// Whether to run the libp2p Autonat protocol.
    #[clap(long, short = 'a', default_value = "false")]
    enable_autonat: bool,

    /// Fixed value to generate deterministic peer id
    #[clap(long, short, default_value = "0")]
    secret_key_seed: Option<u8>,

    /// Fixed value to generate deterministic peer id
    #[clap(long, short, default_value = "0")]
    port: Option<u16>,
}

fn generate_ed25519(secret_key_seed: u8) -> identity::Keypair {
    let mut bytes = [0u8; 32];
    bytes[0] = secret_key_seed;

    identity::Keypair::ed25519_from_bytes(bytes).expect("only errors on wrong length")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let opt = Opts::parse();

    let mut config = Zeroizing::new(config::Config::from_file(opt.config.as_path())?);

    let mut metric_registry = Registry::default();

    let local_keypair = {
        let keypair: identity::Keypair = generate_ed25519(opt.secret_key_seed.unwrap_or(0));
        let keypai_r: identity::Keypair =
            identity::Keypair::from_protobuf_encoding(&Zeroizing::new(
                base64::engine::general_purpose::STANDARD
                    .decode(config.identity.priv_key.as_bytes())?,
            ))?;

        let peer_id: PeerId = keypair.public().into();
        //assert_eq!(
        //    PeerId::from_str(&config.identity.peer_id)?,
        //    peer_id,
        //    "Expect peer id derived from private key and peer id retrieved from config to match."
        //);

        keypair
    };

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(local_keypair)
        .with_tokio()
        .with_tcp(
            tcp::Config::default().nodelay(true),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_quic()
        .with_dns()?
        .with_websocket(noise::Config::new, yamux::Config::default)
        .await?
        .with_bandwidth_metrics(&mut metric_registry)
        .with_behaviour(|key| {
            behaviour::Behaviour::new(key.public(), opt.enable_kademlia, opt.enable_autonat)
        })?
        .build();

    if config.addresses.swarm.is_empty() {
        tracing::warn!("No listen addresses configured");
    }

    let sctp_address: Multiaddr = "/ip4/127.0.0.1/udt/sctp/0".parse().unwrap();
    let components = sctp_address.iter().collect::<Vec<_>>();
    assert_eq!(components[0], Protocol::Ip4(Ipv4Addr::new(127, 0, 0, 1)));
    assert_eq!(components[1], Protocol::Udt);
    assert_eq!(components[2], Protocol::Sctp(0));
    config.addresses.append_announce.push(sctp_address);

    for address in &config.addresses.swarm {
        tracing::info!("{}", address.clone());
        match swarm.listen_on(address.clone()) {
            Ok(_) => {}
            Err(e @ libp2p::TransportError::MultiaddrNotSupported(_)) => {
                tracing::warn!(%address, "Failed to listen on address, continuing anyways, {e}")
            }
            Err(e) => return Err(e.into()),
        }
    }

    if config.addresses.append_announce.is_empty() {
        tracing::warn!("No external addresses configured");
    }
    for address in &config.addresses.append_announce {
        swarm.add_external_address(address.clone())
    }
    tracing::info!(
        "External addresses: {:?}",
        swarm.external_addresses().collect::<Vec<_>>()
    );

    let metrics = Metrics::new(&mut metric_registry);
    let build_info = Info::new(vec![("version".to_string(), env!("CARGO_PKG_VERSION"))]);
    metric_registry.register(
        "build",
        "A metric with a constant '1' value labeled by version",
        build_info,
    );
    tokio::spawn(async move {
        if let Err(e) = http_service::metrics_server(metric_registry, opt.metrics_path).await {
            tracing::error!("Metrics server failed: {e}");
        }
    });

    loop {
        let event = swarm.next().await.expect("Swarm not to terminate.");
        metrics.record(&event);
        match event {
            SwarmEvent::Behaviour(behaviour::BehaviourEvent::Identify(e)) => {
                tracing::info!("{:?}", e);
                metrics.record(&e);

                if let identify::Event::Received {
                    peer_id,
                    info:
                        identify::Info {
                            listen_addrs,
                            protocols,
                            ..
                        },
                    ..
                } = e
                {
                    if protocols.iter().any(|p| *p == kad::PROTOCOL_NAME) {
                        for addr in listen_addrs {
                            swarm
                                .behaviour_mut()
                                .kademlia
                                .as_mut()
                                .map(|k| k.add_address(&peer_id, addr));
                        }
                    }
                }
            }
            SwarmEvent::Behaviour(behaviour::BehaviourEvent::Ping(e)) => {
                tracing::debug!("{:?}", e);
                metrics.record(&e);
            }
            SwarmEvent::Behaviour(behaviour::BehaviourEvent::Kademlia(e)) => {
                tracing::debug!("{:?}", e);
                metrics.record(&e);
            }
            SwarmEvent::Behaviour(behaviour::BehaviourEvent::Relay(e)) => {
                tracing::info!("{:?}", e);
                metrics.record(&e)
            }
            SwarmEvent::Behaviour(behaviour::BehaviourEvent::Autonat(e)) => {
                tracing::info!("{:?}", e);
                // TODO: Add metric recording for `NatStatus`.
                // metrics.record(&e)
            }
            SwarmEvent::NewListenAddr { address, .. } => {
                tracing::info!(%address, "Listening on address");
            }
            _ => {}
        }
    }
}
