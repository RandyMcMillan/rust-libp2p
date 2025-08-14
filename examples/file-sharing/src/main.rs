#![doc = include_str!("../README.md")]

mod handle_input;
mod network;

use crate::handle_input::handle_input_line;
use crate::network::FileRequest;
use crate::network::FileResponse;
use async_std::io;
use clap::Parser;
use futures::{
    channel::{mpsc, oneshot},
    prelude::*,
    select, StreamExt,
};
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
    request_response::{self, OutboundRequestId, ProtocolSupport, ResponseChannel},
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, PeerId, StreamProtocol,
};
use std::collections::HashSet;
use std::error::Error;
use std::io::Write;
use std::path::PathBuf;
use tokio::task::spawn;
use tokio::time::Duration;
use tracing_subscriber::EnvFilter;

fn hex_to_bytes(s: &str) -> Result<[u8; 32], String> {
    if s.len() != 64 {
        return Err("Hex string must be 64 characters long for a 32-byte seed".to_string());
    }
    let mut bytes = [0u8; 32];
    for i in 0..32 {
        bytes[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)
            .map_err(|e| format!("Invalid hex character: {}", e))?;
    }
    Ok(bytes)
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

    let opt = Opt::parse();
    let mut enter_loop: bool = false;

    // We create a custom network behaviour that combines Kademlia and mDNS.
    #[derive(NetworkBehaviour)]
    struct Behaviour {
        kademlia: kad::Behaviour<MemoryStore>,
        mdns: mdns::async_io::Behaviour,
        request_response: request_response::cbor::Behaviour<FileRequest, FileResponse>,
    }

    // Create a static known PeerId based on given secret
    let local_key: identity::Keypair = generate_ed25519(opt.secret_key_seed.ok_or(..).expect(""));

    //intercept prior to file_share
    let (mut network_client, mut network_events, network_event_loop) =
        network::new(opt.secret_key_seed).await?;

    println!("{:?}", network_client);
    // Spawn the network task for it to run in the background.
    spawn(network_event_loop.run());

    if let Some(ref get) = opt.get {
        let line = format!("GET {get}");
        println!("98:line={line}");
        //handle_input::handle_input_line(&mut kv_swarm.behaviour_mut().kademlia, line);
        if !opt.argument.is_some() {
            enter_loop = true;
        }
    }
    if let Some(ref get_providers) = opt.get_providers {
        let line = format!("GET_PROVIDERS {get_providers}");
        println!("104:line={line}");
        //handle_input_line(&mut kv_swarm.behaviour_mut().kademlia, line);
        if !opt.argument.is_some() {
            enter_loop = true;
        }
    }
    if let Some(ref put) = opt.put {
        let line = format!("PUT {put:?}");
        println!("110:line={line}");
        //handle_input_line(&mut kv_swarm.behaviour_mut().kademlia, line);
        if !opt.argument.is_some() {
            enter_loop = true;
        }
    }
    if let Some(ref put_provider) = opt.put_provider {
        let line = format!("PUT_PROVIDER {put_provider}");
        println!("116:line={line}");
        //handle_input_line(&mut kv_swarm.behaviour_mut().kademlia, line);
        if !opt.argument.is_some() {
            enter_loop = true;
        }
    }

    //if enter_loop {
    //    // Read full lines from stdin
    //    let mut stdin = io::BufReader::new(io::stdin()).lines().fuse();

    //    // Listen on all interfaces and whatever port the OS assigns.
    //    //kv_swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    //    // Kick it off.
    //    loop {
    //        select! {
    //        line = stdin.select_next_some() => handle_input_line(&mut kv_swarm.behaviour_mut().kademlia, line.expect("Stdin not to close")),
    //        event = kv_swarm.select_next_some() => match event {
    //            //
    //            SwarmEvent::NewListenAddr { address, .. } => {
    //                println!("Listening in {address:?}");
    //            },
    //            //
    //            SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
    //                for (peer_id, multiaddr) in list {
    //                    kv_swarm.behaviour_mut().kademlia.add_address(&peer_id, multiaddr);
    //                }
    //            }
    //            //
    //            SwarmEvent::Behaviour(BehaviourEvent::Kademlia(kad::Event::OutboundQueryProgressed { result, ..})) => {
    //                match result {
    //            //
    //                    kad::QueryResult::GetProviders(Ok(kad::GetProvidersOk::FoundProviders { key, providers, .. })) => {
    //                        for peer in providers {
    //                            println!(
    //                                "Peer {peer:?} provides key {:?}",
    //                                std::str::from_utf8(key.as_ref()).unwrap()
    //                            );
    //                        }
    //                    }
    //            //
    //                    kad::QueryResult::GetProviders(Err(err)) => {
    //                        eprintln!("Failed to get providers: {err:?}");
    //                    }
    //            //
    //                    kad::QueryResult::GetRecord(Ok(
    //                        kad::GetRecordOk::FoundRecord(kad::PeerRecord {
    //                            record: kad::Record { key, value, .. },
    //                            ..
    //                        })
    //                    )) => {
    //                        println!(
    //                            "Got record {:?} {:?}",
    //                            std::str::from_utf8(key.as_ref()).unwrap(),
    //                            std::str::from_utf8(&value).unwrap(),
    //                        );
    //                    }
    //            //
    //                    kad::QueryResult::GetRecord(Ok(_)) => {}
    //            //
    //                    kad::QueryResult::GetRecord(Err(err)) => {
    //                        eprintln!("Failed to get record: {err:?}");
    //                    }
    //            //
    //                    kad::QueryResult::PutRecord(Ok(kad::PutRecordOk { key })) => {
    //                        println!(
    //                            "Successfully put record {:?}",
    //                            std::str::from_utf8(key.as_ref()).unwrap()
    //                        );
    //                    }
    //            //
    //                    kad::QueryResult::PutRecord(Err(err)) => {
    //                        eprintln!("Failed to put record: {err:?}");
    //                    }
    //            //
    //                    kad::QueryResult::StartProviding(Ok(kad::AddProviderOk { key })) => {
    //                        println!(
    //                            "Successfully put provider record {:?}",
    //                            std::str::from_utf8(key.as_ref()).unwrap()
    //                        );
    //                    }
    //            //
    //                    kad::QueryResult::StartProviding(Err(err)) => {
    //                        eprintln!("Failed to put provider record: {err:?}");
    //                    }
    //                    _ => {}
    //                }
    //            }
    //            _ => {}
    //        }
    //        }
    //    } //end loop
    //} //end if enter_loop

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
    if let Some(addr) = opt.peer.clone() {
        let Some(Protocol::P2p(peer_id)) = addr.iter().last() else {
            return Err("Expect peer multiaddr to contain peer ID.".into());
        };
        network_client
            .dial(peer_id, addr.clone())
            .await
            .expect("245:Dial to succeed");
    }

    match opt.argument {
        // Providing a file.
        Some(CliArgument::Provide {
            path,
            name,
            listen_address,
            ..
        }) => {
            if opt.put.is_some() {
                println!("Get: opt.put={:?}", opt.put);
                network_client
                    .start_providing(opt.put.clone().expect(""))
                    .await;
            } else {
                let mut put_vec: Vec<String> = vec![];
                put_vec.push(name.clone());
                network_client.start_providing(put_vec).await;
            };

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
                } //end match
            } //end loop
        }
        // Locating and getting a file.
        Some(CliArgument::Get { name, peer, .. }) => {
            let mut providers = network_client.get_providers(name.clone()).await;
            if opt.get.is_some() {
                println!("Get: opt.get={:?}", opt.get);
                providers = network_client
                    .get_providers(opt.get.clone().expect("REASON"))
                    .await;
            } else {
                println!("Get: opt.get={:?}", opt.get);
            }
            // Locate all nodes providing the file.
            if providers.is_empty() {
                let mut peer_ids: HashSet<PeerId> = HashSet::new();

                //let peer_id_string = peer;

                // In case the user provided an address of a peer on the CLI, dial it.
                if let Some(addr) = opt.peer.clone() {
                    let Some(Protocol::P2p(peer_id)) = addr.iter().last() else {
                        println!("");
                        return Ok(());
                    }; //{
                       //peer_ids.insert(peer_id);
                       //providers = peer_ids;
                       //for provider in providers.clone() {
                       //    println!("{}", provider)
                       //}
                } else {
                    //peer_ids.insert(peer_id);
                    //providers = peer_ids;
                    //for provider in providers.clone(){println!("{}", provider)};

                    return Err("Expect peer multiaddr to contain peer ID.".into());
                }; //end if let Some
                   //network_client
                   //    .dial(peer_id, addr)
                   //    .await
                   //    .expect("Dial to succeed");
                   //peer_ids.insert(peer_id);
                   //providers = peer_ids;
                   //for provider in providers.clone(){println!("{}", provider)};
            };

            //// Use from_str to parse the string into a PeerId, handling potential errors.
            //let new_peer_id = match PeerId::from_str(peer_id_string) {
            //    Ok(id) => id,
            //    Err(e) => {
            //        eprintln!("Failed to parse PeerId from string: {}", e);
            //        return; // Exit the program if the PeerId string is invalid
            //    }
            //};

            //providers = peer;
            //return Err(format!("Could not find provider for file {name}.").into());
            //};

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

fn _handle_input_line(kademlia: &mut kad::Behaviour<MemoryStore>, line: String) {
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
