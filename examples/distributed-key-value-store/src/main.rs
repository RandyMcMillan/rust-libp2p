#![doc = include_str!("../README.md")]
mod file_transfer;

use base64::{engine::general_purpose, Engine as _};
use futures::stream::StreamExt;
use git2::Repository;
//use libp2p::identity::Keypair;
//use libp2p::request_response::Behaviour;
use libp2p::{
    kad,
    kad::{store::MemoryStore, Mode},
    mdns, noise,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux,
};
use sha2::{Digest, Sha256};
use std::error::Error;
use std::num::NonZeroUsize;
use std::path::Path;
use tokio::{
    io::{self, AsyncBufReadExt},
    select,
};
use tracing_subscriber::EnvFilter;

fn hash_folder_name(path: &Path) -> Option<String> {
    if let Some(folder_name) = path.file_name().and_then(|name| name.to_str()) {
        let mut hasher = Sha256::new();
        hasher.update(folder_name.as_bytes());
        let result = hasher.finalize();
        Some(format!("{result:x}"))
    } else {
        None
    }
}

fn create_keypair_from_hex_string(
    secret_key_hex: &str,
) -> Result<libp2p::identity::Keypair, hex::FromHexError> {
    // The secret key for Ed25519 is 32 bytes.
    let mut secret_key_bytes = [0u8; 32];
    hex::decode_to_slice(secret_key_hex, &mut secret_key_bytes)?;
    Ok(libp2p::identity::Keypair::ed25519_from_bytes(secret_key_bytes).unwrap())
}

fn get_repo_name<P: AsRef<Path>>(repo_path: P) -> Result<Option<String>, git2::Error> {
    //discover repo root
    let repo = Repository::discover(repo_path)?;

    // Get the path of the repository
    let path = repo.path();

    //println!("repo_name:{}", path.display());
    // The repo path typically ends in `.git`.
    // We want the parent directory's name.
    let parent_path = path.parent().unwrap_or(path);

    // Use `file_name()` to get the last component of the path.
    // `to_str()` is used to convert OsStr to a string slice.
    let repo_name = parent_path.file_name().and_then(|name| name.to_str());

    Ok(repo_name.map(String::from))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    //let file_transfer = file_transfer().await;
    let my_path = Path::new(".");

    let repo_name = get_repo_name(my_path);

    let repo_name_clone = get_repo_name(my_path);
    let repo_name_clone2 = get_repo_name(my_path);

    println!("repo_name={}", repo_name_clone.unwrap().ok_or("")?);

    let keypair: libp2p::identity::Keypair;

    // We create a custom network behaviour that combines Kademlia and mDNS.
    #[derive(NetworkBehaviour)]
    struct Behaviour {
        kademlia: kad::Behaviour<MemoryStore>,
        mdns: mdns::tokio::Behaviour,
    }

    let mut swarm = libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
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
                mdns: mdns::tokio::Behaviour::new(
                    mdns::Config::default(),
                    key.public().to_peer_id(),
                )?,
            })
        })?
        .build();

    if let Some(hash) = hash_folder_name(Path::new(&repo_name?.expect("").to_string())) {
        println!("hash_folder_name={hash}");

        keypair = create_keypair_from_hex_string(&hash).expect("");

        println!("{:?}", keypair.public());

        let protobuf_bytes = keypair
            .to_protobuf_encoding()
            .expect("should be able to encode keypair");

        let base64_string = general_purpose::STANDARD.encode(&protobuf_bytes);

        print!(
            "Keypair as Base64 string (Protobuf encoded):\n{base64_string}"
        );

        let decoded_bytes = general_purpose::STANDARD
            .decode(&base64_string)
            .expect("should be able to decode base64");

        let rehydrated_keypair = libp2p::identity::Keypair::from_protobuf_encoding(&decoded_bytes)
            .expect("should be able to decode protobuf bytes");

        let rehydrated_public_key = rehydrated_keypair.public();
        println!("\nRehydrated Public Key:\n{rehydrated_public_key:?}");

        assert_eq!(keypair.public(), rehydrated_public_key);
        println!("Successfully rehydrated the keypair! They are identical.");

        let line = format!("PUT {:?} {:?}", repo_name_clone2.unwrap().unwrap(), hash);
        println!("{line}");
        put_repo_key_value(&mut swarm.behaviour_mut().kademlia, line.replace("\"", ""));
    } else {
        println!("Could not get folder name.");
    }

    swarm.behaviour_mut().kademlia.set_mode(Some(Mode::Server));

    // Read full lines from stdin
    let mut stdin = io::BufReader::new(io::stdin()).lines();

    // Listen on all interfaces and whatever port the OS assigns.
    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    // Kick it off.
    loop {
        select! {
        Ok(Some(line)) = stdin.next_line() => {
            handle_input_line(&mut swarm.behaviour_mut().kademlia, line);
        }
        event = swarm.select_next_some() => match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                println!("Listening in {address:?}");
            },
            SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                for (peer_id, multiaddr) in list {
                    swarm.behaviour_mut().kademlia.add_address(&peer_id, multiaddr);
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
                        //eprintln!("Quorum may have failed to put record: {err:?}");
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
    }
}

fn put_repo_key_value(kademlia: &mut kad::Behaviour<MemoryStore>, line: String) {
    println!("line={}", line.replace("\"", ""));
    let line = line.to_string();
    println!("line={}", line.replace("\"", ""));
    let mut args = line.split(' ');
    match args.next() {
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
            let key_clone = key.clone();
            let record = kad::Record {
                key,
                value,
                publisher: None,
                expires: None,
            };
            kademlia
                .put_record(
                    record,
                    kad::Quorum::N(NonZeroUsize::new(1).expect("REASON")),
                )
                .expect("Failed to store record locally.");
            kademlia
                .start_providing(key_clone)
                .expect("Failed to start providing key");
        }
        _ => {
            eprintln!("put_repo_key_value failed!");
        }
    }
}

fn handle_input_line(kademlia: &mut kad::Behaviour<MemoryStore>, line: String) {
    let mut args = line.split(' ');

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
                key,
                value,
                publisher: None,
                expires: None,
            };
            kademlia
                .put_record(record, kad::Quorum::One)
                .expect("Failed to store record locally.");
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
