#![doc = include_str!("../README.md")]

use async_std::io;
use clap::Parser;
use futures::{prelude::*, select, StreamExt};
use libp2p::{
    core::Multiaddr,
    identity,
    identity::{ed25519, Keypair},
    kad,
    kad::store::MemoryStore,
    kad::Mode,
    mdns,
    multiaddr::Protocol,
    noise,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, PeerId,
};
use std::collections::HashSet;
use std::error::Error;
use std::io::Write;
use std::path::PathBuf;
use tokio::task::spawn;
use tokio::time::Duration;
use tracing_subscriber::EnvFilter;

pub(crate) fn handle_input_line(kademlia: &mut kad::Behaviour<MemoryStore>, line: String) {
    //let mut args = line.replace("[","").replace("]","").replace(",","").split(' ');
    let binding = line
        .replace("[", "")
        .replace("]", "")
        .replace(",", "")
        .replace("\"", "");
    println!("binding={}", binding);
    let mut args = binding.split(' ');

    match args.next() {
        Some("GET") => {
            let key = {
                match args.next() {
                    Some(key) => kad::RecordKey::new(&key),
                    None => {
                        eprintln!("Expected key");
                        return;
                    }
                }
            };
            kademlia.get_record(key);
        }
        Some("GET_PROVIDERS") => {
            let key = {
                match args.next() {
                    Some(key) => kad::RecordKey::new(&key),
                    None => {
                        eprintln!("Expected key");
                        return;
                    }
                }
            };
            kademlia.get_providers(key);
        }
        Some("PUT") => {
            let key = {
                match args.next() {
                    Some(key) => kad::RecordKey::new(&key),
                    None => {
                        eprintln!("Expected key");
                        return;
                    }
                }
            };
            let value = {
                match args.next() {
                    Some(value) => value.as_bytes().to_vec(),
                    None => {
                        eprintln!("Expected value");
                        return;
                    }
                }
            };
            let record = kad::Record {
                key: key.clone(),
                value,
                publisher: None,
                expires: None,
            };
            kademlia
                .put_record(record, kad::Quorum::One)
                .expect("Failed to store record locally.");
            //automatically start providing
            kademlia
                .start_providing(key)
                .expect("Failed to start providing key");
        }
        Some("PUT_PROVIDER") => {
            let key = {
                match args.next() {
                    Some(key) => kad::RecordKey::new(&key),
                    None => {
                        eprintln!("Expected key");
                        return;
                    }
                }
            };
            kademlia
                .start_providing(key)
                .expect("Failed to start providing key");
        }
        _ => {
            //TODO provide git HEAD
            eprintln!("expected GET, GET_PROVIDERS, PUT or PUT_PROVIDER");
        }
    }
}

#[derive(Parser, Debug)]
#[clap(name = "libp2p file sharing example")]
struct Opt {
    /// Fixed value to generate deterministic peer ID.
    #[clap(long, default_value = "0")]
    secret_key_seed: Option<u8>,

    #[clap(long)]
    get: Option<String>,

    #[clap(long)]
    get_providers: Option<String>,

    #[clap(long, num_args = 2)]
    put: Option<Vec<String>>,

    #[clap(long)]
    put_provider: Option<String>,

    #[clap(long)]
    peer: Option<Multiaddr>,

    #[clap(long, default_value = "/ip4/0.0.0.0/tcp/0")]
    listen_address: Option<Multiaddr>,

    #[clap(subcommand)]
    argument: Option<CliArgument>,
}

#[derive(Debug, Parser)]
enum CliArgument {
    Provide {
        #[clap(long)]
        path: PathBuf,
        #[clap(long)]
        name: String,
        #[clap(long)]
        peer: Option<Multiaddr>,

        #[clap(long, default_value = "/ip4/0.0.0.0/tcp/0")]
        listen_address: Option<Multiaddr>,
    },
    Get {
        #[clap(long)]
        name: String,
        #[clap(long)]
        peer: Option<Multiaddr>,

        #[clap(long, default_value = "/ip4/0.0.0.0/tcp/0")]
        listen_address: Option<Multiaddr>,
    },
}
