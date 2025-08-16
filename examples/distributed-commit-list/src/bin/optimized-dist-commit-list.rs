#![doc = include_str!("../../README.md")]
use clap::Parser;
use futures::stream::StreamExt;
use git2::{Commit, DiffOptions, ObjectType, Oid, Repository, Signature, Time};
use git2::{DiffFormat, Error as GitError, Pathspec};
use libp2p::StreamProtocol;
use libp2p::{
    identify, identity, kad,
    kad::store::MemoryStore,
    kad::store::MemoryStoreConfig,
    kad::Config as KadConfig,
    mdns, noise, ping, rendezvous,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, Swarm,
};
use std::error::Error;
use std::str;
use std::time::Duration;
use tokio::{
    io::{self, AsyncBufReadExt},
    select,
};
use tracing::{debug, info, trace, Level};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

// --- Top-level NetworkBehaviour Definition ---
#[derive(NetworkBehaviour)]
struct Behaviour {
    ipfs: kad::Behaviour<kad::store::MemoryStore>,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
    mdns: mdns::tokio::Behaviour,
    identify: identify::Behaviour,
    rendezvous: rendezvous::server::Behaviour,
    ping: ping::Behaviour,
}

fn init_subscriber() -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    let filter_layer = EnvFilter::try_from_default_env().or_else(|_| EnvFilter::try_new("info"))?;

    let fmt_layer = fmt::layer().with_target(false).with_ansi(true);

    tracing_subscriber::registry()
        .with(filter_layer)
        .with(fmt_layer)
        .init();

    Ok(())
}

const IPFS_BOOTNODES: [&str; 4] = [
    "QmNnooDu7bfjPFoTZYxMNLWUQJyrVwtbZg5gBMjTezGAJN",
    "QmQCU2EcMqAqQPR2i9bChDtGNJchTbq5TbXJJ16u19uLTa",
    "QmbLHAnMoJPWSCR5Zhtx6BHJX9KiKNN6tpvbUcqanj75Nb",
    "QmcZf59bWwK5XFi76CZX8cbJ4BhTzzA3gU1ZjYZcYW3dwt",
];
const IPFS_PROTO_NAME: StreamProtocol = StreamProtocol::new("/ipfs/kad/1.0.0");

fn get_commit_diff_as_bytes(repo: &Repository, commit: &Commit) -> Result<Vec<u8>, git2::Error> {
    let tree = commit.tree()?;
    let parent_tree = if commit.parent_count() > 0 {
        Some(commit.parent(0)?.tree()?)
    } else {
        None
    };

    let diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)?;
    let mut buf = Vec::new();

    diff.print(DiffFormat::Patch, |_, _, line| {
        buf.extend_from_slice(line.content());
        true
    })?;

    Ok(buf)
}

fn get_commit_id_of_tag(repo: &Repository, tag_name: &str) -> Result<String, git2::Error> {
    let reference_name = format!("refs/tags/{}", tag_name);
    let reference = repo.find_reference(&reference_name)?;
    let object = reference.peel(ObjectType::Commit)?;
    Ok(object.id().to_string())
}

fn generate_ed25519(secret_key_seed: u8) -> identity::Keypair {
    let mut bytes = [0u8; 32];
    bytes[0] = secret_key_seed;
    identity::Keypair::ed25519_from_bytes(bytes).expect("only errors on wrong length")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    init_subscriber();

    let args = Args::parse();
    debug!("args={:?}", args);

    let keypair = generate_ed25519(args.secret.unwrap_or(0));
    let peer_id = PeerId::from(keypair.public());
    info!("Local PeerId: {}", peer_id);

    tracing::debug!("args={:?}", args);
    // Create a static known PeerId based on given secret
    let keypair: identity::Keypair = generate_ed25519(args.secret.clone().unwrap_or(0));
    let keypair_clone: identity::Keypair = generate_ed25519(args.secret.unwrap_or(0));

    let public_key = keypair.public();
    let peer_id = PeerId::from_public_key(&public_key);
    let kad_store_config = MemoryStoreConfig {
        max_provided_keys: usize::MAX,
        max_providers_per_key: usize::MAX,
        max_records: usize::MAX,
        max_value_bytes: usize::MAX,
    };
    let kad_memstore = MemoryStore::with_config(peer_id.clone(), kad_store_config.clone());
    let mut kad_config = KadConfig::default();

    // --- Create the Swarm ---
    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_dns()?
        .with_behaviour(|key| {
            let kad_store_config = MemoryStoreConfig {
                max_provided_keys: usize::MAX,
                max_providers_per_key: usize::MAX,
                max_records: usize::MAX,
                max_value_bytes: usize::MAX,
            };
            //    let kad_memstore = MemoryStore::with_config(peer_id.clone(), kad_store_config.clone());

            // Configure the Kademlia behaviour for application-specific data
            let mut kad_config = kad::Config::default();
            kad_config.set_query_timeout(Duration::from_secs(30));
            kad_config.set_replication_factor(std::num::NonZeroUsize::new(10).unwrap());
            kad_config.set_publication_interval(Some(Duration::from_secs(3600)));
            kad_config.disjoint_query_paths(true);
            let kad_store = MemoryStore::with_config(peer_id.clone(), kad_store_config);
            //            let kad_store = kad::store::MemoryStore::new(key.public().to_peer_id());

            // Configure a separate Kademlia behaviour for IPFS bootstrapping
            let mut ipfs_cfg = kad::Config::new(IPFS_PROTO_NAME);
            ipfs_cfg.set_query_timeout(Duration::from_secs(5 * 60));
            let ipfs_store = kad::store::MemoryStore::new(key.public().to_peer_id());

            Ok(Behaviour {
                ipfs: kad::Behaviour::with_config(key.public().to_peer_id(), ipfs_store, ipfs_cfg),
                kademlia: kad::Behaviour::with_config(
                    key.public().to_peer_id(),
                    kad_store,
                    kad_config,
                ),
                identify: identify::Behaviour::new(identify::Config::new(
                    "/yamux/1.0.0".to_string(),
                    key.public(),
                )),
                rendezvous: rendezvous::server::Behaviour::new(
                    rendezvous::server::Config::default(),
                ),
                ping: ping::Behaviour::new(
                    ping::Config::new().with_interval(Duration::from_secs(60)),
                ),
                mdns: mdns::tokio::Behaviour::new(
                    mdns::Config::default(),
                    key.public().to_peer_id(),
                )?,
            })
        })?
        .build();

    for peer in &IPFS_BOOTNODES {
        swarm
            .behaviour_mut()
            .ipfs
            .add_address(&peer.parse()?, "/dnsaddr/bootstrap.libp2p.io".parse()?);
        swarm
            .behaviour_mut()
            .kademlia
            .add_address(&peer.parse()?, "/dnsaddr/bootstrap.libp2p.io".parse()?);
    }

    let bootstrap_node: Multiaddr = "/dnsaddr/bootstrap.libp2p.io"
        .parse()
        .expect("Hardcoded bootstrap address should be valid");

    // --- Bootstrap the Swarm ---
    for peer in &IPFS_BOOTNODES {
        let peer_id: PeerId = peer.parse()?;
        let addr: Multiaddr = "/dnsaddr/bootstrap.libp2p.io".parse()?;
        swarm
            .behaviour_mut()
            .ipfs
            .add_address(&peer_id, addr.clone());
        swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
    }

    swarm
        .behaviour_mut()
        .kademlia
        .set_mode(Some(kad::Mode::Server));

    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    // --- Initial Data Publishing from Git Repo ---
    info!("Starting initial git repository scan and data publishing...");
    if let Err(e) = run(&args, &mut swarm).await {
        eprintln!("Error during initial git processing: {}", e);
    }
    info!("Initial data publishing complete.");

    // --- Main Event Loop ---
    let mut stdin = io::BufReader::new(io::stdin()).lines();
    loop {
        select! {
            line = stdin.next_line() => {
                let line = line?.ok_or("stdin closed")?;
                handle_input_line(&mut swarm.behaviour_mut().kademlia, line).await;
            }
            event = swarm.select_next_some() => {
                handle_swarm_event(&mut swarm, event).await;
            }
        }
    }
}

async fn handle_swarm_event(swarm: &mut Swarm<Behaviour>, event: SwarmEvent<BehaviourEvent>) {
    match event {
        SwarmEvent::NewListenAddr { address, .. } => {
            info!("Listening on {address}");
        }
        SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
            for (peer_id, multiaddr) in list {
                info!("mDNS discovered a new peer: {peer_id}");
                swarm
                    .behaviour_mut()
                    .kademlia
                    .add_address(&peer_id, multiaddr);
            }
        }
        SwarmEvent::Behaviour(BehaviourEvent::Kademlia(kad::Event::OutboundQueryProgressed {
            result,
            ..
        })) => match result {
            kad::QueryResult::GetRecord(Ok(kad::GetRecordOk::FoundRecord(kad::PeerRecord {
                record,
                ..
            }))) => {
                println!(
                    "{{\"commit\":{:?},\"message\":{:?}}}",
                    std::str::from_utf8(record.key.as_ref()).unwrap_or("invalid utf8"),
                    std::str::from_utf8(&record.value).unwrap_or("invalid utf8"),
                );
            }
            kad::QueryResult::GetRecord(Err(err)) => {
                eprintln!("Failed to get record: {err:?}");
            }
            kad::QueryResult::PutRecord(Ok(kad::PutRecordOk { key })) => {
                info!(
                    "Successfully PUT record for key: {:?}",
                    std::str::from_utf8(key.as_ref())
                );
            }
            kad::QueryResult::PutRecord(Err(err)) => {
                eprintln!("Failed to PUT record: {err:?}");
            }
            kad::QueryResult::StartProviding(Ok(kad::AddProviderOk { key, .. })) => {
                info!(
                    "Successfully started PROVIDING key: {:?}",
                    std::str::from_utf8(key.as_ref())
                );
            }
            kad::QueryResult::StartProviding(Err(err)) => {
                eprintln!("Failed to start PROVIDING: {err:?}");
            }
            _ => {}
        },
        _ => {}
    }
}

async fn handle_input_line(kademlia: &mut kad::Behaviour<kad::store::MemoryStore>, line: String) {
    let mut args = line.split_whitespace();
    match args.next() {
        Some("GET") => {
            if let Some(key_str) = args.next() {
                let key = kad::RecordKey::new(&key_str);
                kademlia.get_record(key);
            } else {
                eprintln!("Usage: GET <key>");
            }
        }
        Some("GET_PROVIDERS") => {
            if let Some(key_str) = args.next() {
                let key = kad::RecordKey::new(&key_str);
                kademlia.get_providers(key);
            } else {
                eprintln!("Usage: GET_PROVIDERS <key>");
            }
        }
        Some("PUT") => {
            if let (Some(key_str), Some(value_str)) = (args.next(), args.next()) {
                let key = kad::RecordKey::new(&key_str);
                let value = value_str.as_bytes().to_vec();
                let record = kad::Record {
                    key,
                    value,
                    publisher: None,
                    expires: None,
                };
                if let Err(e) = kademlia.put_record(record, kad::Quorum::One) {
                    eprintln!("Failed to store record locally: {:?}", e);
                }
            } else {
                eprintln!("Usage: PUT <key> <value>");
            }
        }
        Some("QUIT") | Some("Q") | Some("EXIT") => {
            std::process::exit(0);
        }
        _ => {
            eprintln!("Commands: GET, GET_PROVIDERS, PUT, QUIT");
        }
    }
}

async fn run(args: &Args, swarm: &mut Swarm<Behaviour>) -> Result<(), Box<dyn Error>> {
    let path = args.flag_git_dir.as_ref().map_or(".", |s| &s[..]);
    let repo = Repository::discover(path)?;

    // --- Publish Tags ---
    if let Ok(tag_names) = repo.tag_names(None) {
        for tag_name_opt in tag_names.iter() {
            if let Some(tag_name) = tag_name_opt {
                if let Ok(commit_id) = get_commit_id_of_tag(&repo, tag_name) {
                    let key = kad::RecordKey::new(&tag_name);
                    let record = kad::Record {
                        key: key.clone(),
                        value: commit_id.into_bytes(),
                        publisher: Some(swarm.local_peer_id().clone()),
                        expires: None,
                    };
                    swarm
                        .behaviour_mut()
                        .kademlia
                        .put_record(record, kad::Quorum::One)?;
                    swarm.behaviour_mut().kademlia.start_providing(key)?;
                }
            }
        }
    }

    // --- Prepare Commit Traversal (Revwalk) ---
    let mut revwalk = repo.revwalk()?;
    let base = if args.flag_reverse {
        git2::Sort::REVERSE
    } else {
        git2::Sort::NONE
    };
    let sorting = base
        | if args.flag_topo_order {
            git2::Sort::TOPOLOGICAL
        } else if args.flag_date_order {
            git2::Sort::TIME
        } else {
            git2::Sort::NONE
        };
    revwalk.set_sorting(sorting)?;

    if args.arg_commit.is_empty() {
        revwalk.push_head()?;
    } else {
        // (Simplified revspec logic for brevity, original logic can be retained if complex specs are needed)
        for commit_spec in &args.arg_commit {
            let obj = repo.revparse_single(commit_spec)?;
            revwalk.push(obj.id())?;
        }
    }

    // --- Iterate and Publish Commits ---
    let revwalk_iterator = revwalk
        .filter_map(Result::ok)
        .filter_map(|id| repo.find_commit(id).ok());

    for commit in revwalk_iterator.take(args.flag_max_count.unwrap_or(usize::MAX)) {
        let commit_id_str = commit.id().to_string();

        // Publish commit message
        let msg_key = kad::RecordKey::new(&commit_id_str);
        let msg_record = kad::Record {
            key: msg_key.clone(),
            value: commit.message_bytes().to_vec(),
            publisher: Some(*swarm.local_peer_id()),
            expires: None,
        };
        swarm
            .behaviour_mut()
            .kademlia
            .put_record(msg_record, kad::Quorum::One)?;
        swarm.behaviour_mut().kademlia.start_providing(msg_key)?;

        // Publish commit diff
        if let Ok(diff_bytes) = get_commit_diff_as_bytes(&repo, &commit) {
            let diff_key_str = format!("{}/diff", commit_id_str);
            let diff_key = kad::RecordKey::new(&diff_key_str);
            let diff_record = kad::Record {
                key: diff_key.clone(),
                value: diff_bytes,
                publisher: Some(*swarm.local_peer_id()),
                expires: None,
            };
            swarm
                .behaviour_mut()
                .kademlia
                .put_record(diff_record, kad::Quorum::One)?;
            swarm.behaviour_mut().kademlia.start_providing(diff_key)?;
        }
    }

    Ok(())
}

// --- CLI Arguments Struct (unchanged from original) ---
#[derive(Debug, Parser)]
struct Args {
    #[clap(long)]
    secret: Option<u8>,
    #[clap(long)]
    flag_topo_order: bool,
    #[clap(long)]
    flag_date_order: bool,
    #[clap(long)]
    flag_reverse: bool,
    #[clap(long)]
    flag_author: Option<String>,
    #[clap(long)]
    flag_committer: Option<String>,
    #[clap(long = "grep")]
    flag_grep: Option<String>,
    #[clap(long = "git-dir")]
    flag_git_dir: Option<String>,
    #[clap(long)]
    flag_skip: Option<usize>,
    #[clap(short = 'n', long)]
    flag_max_count: Option<usize>,
    #[clap(long)]
    flag_merges: bool,
    #[clap(long)]
    flag_no_merges: bool,
    #[clap(long)]
    flag_no_min_parents: bool,
    #[clap(long)]
    flag_no_max_parents: bool,
    #[clap(long)]
    flag_max_parents: Option<usize>,
    #[clap(long)]
    flag_min_parents: Option<usize>,
    #[clap(long, short)]
    flag_patch: bool,
    arg_commit: Vec<String>,
    #[clap(last = true)]
    arg_spec: Vec<String>,
}

// (impl Args methods for min/max parents can be kept if needed, but are omitted here as they weren't used in the simplified `run` function)
