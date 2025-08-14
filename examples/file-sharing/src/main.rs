// Copyright 2021 Protocol Labs.
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

#![doc = include_str!("../README.md")]

mod network;

use clap::Parser;
use futures::prelude::*;
use futures::StreamExt;
use libp2p::{core::Multiaddr, kad, kad::store::MemoryStore, kad::Mode, multiaddr::Protocol};
use std::error::Error;
use std::io::Write;
use std::path::PathBuf;
use tokio::task::spawn;
use tokio::time::Duration;
use tracing_subscriber::EnvFilter;

use async_std::io;
use futures::{prelude::*, select};
use libp2p::{
    mdns, noise,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let mut enter_loop: bool = false;

    // We create a custom network behaviour that combines Kademlia and mDNS.
    #[derive(NetworkBehaviour)]
    struct Behaviour {
        kademlia: kad::Behaviour<MemoryStore>,
        mdns: mdns::async_io::Behaviour,
    }

    let mut kv_swarm = libp2p::SwarmBuilder::with_new_identity()
        .with_async_std()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|key| {
            Ok(Behaviour {
                kademlia: kad::Behaviour::new(
                    key.public().to_peer_id(),
                    MemoryStore::new(key.public().to_peer_id()),
                ),
                mdns: mdns::async_io::Behaviour::new(
                    mdns::Config::default(),
                    key.public().to_peer_id(),
                )?,
            })
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();
    kv_swarm
        .behaviour_mut()
        .kademlia
        .set_mode(Some(Mode::Server));

    let opt = Opt::parse();
    if let Some(address) = opt.listen_address.clone() {
        kv_swarm
            .listen_on(address.clone())
            .expect("Listening not to fail.");
    } else {
        kv_swarm
            .listen_on("/ip4/0.0.0.0/tcp/0".parse()?)
            .expect("Listening not to fail.");
    };

    if let Some(get) = opt.get {
        let line = format!("GET {}", get);
        println!("98:line={}", line);
        handle_input_line(&mut kv_swarm.behaviour_mut().kademlia, line);
        enter_loop = true;
    }
    if let Some(get_providers) = opt.get_providers {
        let line = format!("GET_PROVIDERS {}", get_providers);
        println!("104:line={}", line);
        handle_input_line(&mut kv_swarm.behaviour_mut().kademlia, line);
        enter_loop = true;
    }
    if let Some(put) = opt.put {
        let line = format!("PUT {:?}", put);
        println!("110:line={}", line);
        handle_input_line(&mut kv_swarm.behaviour_mut().kademlia, line);
        enter_loop = true;
    }
    if let Some(put_provider) = opt.put_provider {
        let line = format!("PUT_PROVIDER {}", put_provider);
        println!("116:line={}", line);
        handle_input_line(&mut kv_swarm.behaviour_mut().kademlia, line);
        enter_loop = true;
    }

    if enter_loop {
        // Read full lines from stdin
        let mut stdin = io::BufReader::new(io::stdin()).lines().fuse();

        // Listen on all interfaces and whatever port the OS assigns.
        //kv_swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

        // Kick it off.
        loop {
            select! {
            line = stdin.select_next_some() => handle_input_line(&mut kv_swarm.behaviour_mut().kademlia, line.expect("Stdin not to close")),
            event = kv_swarm.select_next_some() => match event {
                SwarmEvent::NewListenAddr { address, .. } => {
                    println!("Listening in {address:?}");
                },
                SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                    for (peer_id, multiaddr) in list {
                        kv_swarm.behaviour_mut().kademlia.add_address(&peer_id, multiaddr);
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Kademlia(kad::Event::OutboundQueryProgressed { result, ..})) => {
                    match result {
                        kad::QueryResult::GetProviders(Ok(kad::GetProvidersOk::FoundProviders { key, providers, .. })) => {
                            for peer in providers {
                                println!(
                                    "Peer {peer:?} provides key {:?}",
                                    std::str::from_utf8(key.as_ref()).unwrap()
                                );
                            }
                        }
                        kad::QueryResult::GetProviders(Err(err)) => {
                            eprintln!("Failed to get providers: {err:?}");
                        }
                        kad::QueryResult::GetRecord(Ok(
                            kad::GetRecordOk::FoundRecord(kad::PeerRecord {
                                record: kad::Record { key, value, .. },
                                ..
                            })
                        )) => {
                            println!(
                                "Got record {:?} {:?}",
                                std::str::from_utf8(key.as_ref()).unwrap(),
                                std::str::from_utf8(&value).unwrap(),
                            );
                        }
                        kad::QueryResult::GetRecord(Ok(_)) => {}
                        kad::QueryResult::GetRecord(Err(err)) => {
                            eprintln!("Failed to get record: {err:?}");
                        }
                        kad::QueryResult::PutRecord(Ok(kad::PutRecordOk { key })) => {
                            println!(
                                "Successfully put record {:?}",
                                std::str::from_utf8(key.as_ref()).unwrap()
                            );
                        }
                        kad::QueryResult::PutRecord(Err(err)) => {
                            eprintln!("Failed to put record: {err:?}");
                        }
                        kad::QueryResult::StartProviding(Ok(kad::AddProviderOk { key })) => {
                            println!(
                                "Successfully put provider record {:?}",
                                std::str::from_utf8(key.as_ref()).unwrap()
                            );
                        }
                        kad::QueryResult::StartProviding(Err(err)) => {
                            eprintln!("Failed to put provider record: {err:?}");
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            }
        } //end loop
    } //end if enter_loop

    //intercept prior to file_share
    let (mut network_client, mut network_events, network_event_loop) =
        network::new(opt.secret_key_seed).await?;

    // Spawn the network task for it to run in the background.
    spawn(network_event_loop.run());

    // In case a listen address was provided use it, otherwise listen on any
    // address.
    match opt.listen_address {
        Some(addr) => network_client
            .start_listening(addr)
            .await
            .expect("Listening not to fail."),
        None => network_client
            .start_listening("/ip4/0.0.0.0/tcp/0".parse()?)
            .await
            .expect("Listening not to fail."),
    };

    // In case the user provided an address of a peer on the CLI, dial it.
    if let Some(addr) = opt.peer {
        let Some(Protocol::P2p(peer_id)) = addr.iter().last() else {
            return Err("Expect peer multiaddr to contain peer ID.".into());
        };
        network_client
            .dial(peer_id, addr)
            .await
            .expect("Dial to succeed");
    }

    match opt.argument {
        // Providing a file.
        Some(CliArgument::Provide { path, name }) => {
            // Advertise oneself as a provider of the file on the DHT.
            network_client.start_providing(name.clone()).await;

            loop {
                match network_events.next().await {
                    // Reply with the content of the file on incoming requests.
                    Some(network::Event::InboundRequest { request, channel }) => {
                        if request == name {
                            network_client
                                .respond_file(std::fs::read(&path)?, channel)
                                .await;
                        }
                    }
                    e => todo!("{:?}", e),
                }
            }
        }
        // Locating and getting a file.
        Some(CliArgument::Get { name }) => {
            // Locate all nodes providing the file.
            let providers = network_client.get_providers(name.clone()).await;
            if providers.is_empty() {
                return Err(format!("Could not find provider for file {name}.").into());
            }

            // Request the content of the file from each node.
            let requests = providers.into_iter().map(|p| {
                let mut network_client = network_client.clone();
                let name = name.clone();
                async move { network_client.request_file(p, name).await }.boxed()
            });

            // Await the requests, ignore the remaining once a single one succeeds.
            let file_content = futures::future::select_ok(requests)
                .await
                .map_err(|_| "None of the providers returned file.")?
                .0;

            std::io::stdout().write_all(&file_content)?;
        }
        None => {}
    }

    Ok(())
}

fn handle_input_line(kademlia: &mut kad::Behaviour<MemoryStore>, line: String) {
    //let mut args = line.replace("[","").replace("]","").replace(",","").split(' ');
    let binding = line
        .replace("[", "")
        .replace("]", "")
        .replace(",", "")
        .replace("\"", "");
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
            eprintln!("expected GET, GET_PROVIDERS, PUT or PUT_PROVIDER");
        }
    }
}

#[derive(Parser, Debug)]
#[clap(name = "libp2p file sharing example")]
struct Opt {
    /// Fixed value to generate deterministic peer ID.
    #[clap(long)]
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
    },
    Get {
        #[clap(long)]
        name: String,
    },
}
