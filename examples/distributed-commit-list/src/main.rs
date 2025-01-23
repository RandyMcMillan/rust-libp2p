#![doc = include_str!("../README.md")]

use clap::Parser;
use git2::{Commit, DiffOptions, ObjectType, Repository, Signature, Time};
use git2::{DiffFormat, Error as GitError, Pathspec};
use std::str;
use std::{error::Error, time::Duration};

use futures::stream::StreamExt;
use libp2p::StreamProtocol;
use libp2p::{
    identify, kad,
    kad::{store::MemoryStore, Mode},
    mdns, noise, ping, rendezvous,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux,
};
use tokio::{
    io::{self, AsyncBufReadExt},
    select,
};
use tracing::Level;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, EnvFilter};
//use tracing_log::LogTracer;
use tracing_log::log;
fn init_subscriber(_level: Level) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    let fmt_layer = fmt::layer().with_target(false);
    let filter_layer = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info")) //default
        .unwrap();

    tracing_subscriber::registry()
        .with(filter_layer)
        .with(fmt_layer)
        .init();

    tracing_subscriber::fmt()
        // Setting a filter based on the value of the RUST_LOG environment variable
        // Examples:
        //
        // RUST_LOG="off,libp2p_mdns::behaviour=off"
        // RUST_LOG="warn,libp2p_mdns::behaviour=off"
        // RUST_LOG="debug,libp2p_mdns::behaviour=off"
        //
        .with_env_filter(EnvFilter::from_default_env())
        //.with_max_level(level)
        // Configure the subscriber to emit logs in JSON format.
        .json()
        // Configure the subscriber to flatten event fields in the output JSON objects.
        //.flatten_event(true)
        // Set the subscriber as the default, returning an error if this fails.
        .try_init()?;

    Ok(())
}

async fn get_blockheight() -> Result<String, Box<dyn Error>> {
    let blockheight = reqwest::get("https://mempool.space/api/blocks/tip/height")
        .await?
        .text()
        .await?;
    Ok(blockheight)
}

const BOOTNODES: [&str; 4] = [
    "QmNnooDu7bfjPFoTZYxMNLWUQJyrVwtbZg5gBMjTezGAJN",
    "QmQCU2EcMqAqQPR2i9bChDtGNJchTbq5TbXJJ16u19uLTa",
    "QmbLHAnMoJPWSCR5Zhtx6BHJX9KiKNN6tpvbUcqanj75Nb",
    "QmcZf59bWwK5XFi76CZX8cbJ4BhTzzA3gU1ZjYZcYW3dwt",
];
const IPFS_PROTO_NAME: StreamProtocol = StreamProtocol::new("/ipfs/kad/1.0.0");

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _ = init_subscriber(Level::INFO);

    let blockheight = get_blockheight().await.unwrap();
    log::info!("blockheight = {blockheight:?}");

    //TODO create key from arg
    let args = Args::parse();

    // Results in PeerID 12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN which is
    // used as the rendezvous point by the other peer examples.
    // TODO --key arg
    let keypair = libp2p::identity::Keypair::ed25519_from_bytes([0; 32]).unwrap();

    // We create a custom network behaviour that combines
    // Kademlia and mDNS identify rendezvous ping
    #[derive(NetworkBehaviour)]
    struct Behaviour {
        ipfs: kad::Behaviour<MemoryStore>,
        kademlia: kad::Behaviour<MemoryStore>,
        mdns: mdns::tokio::Behaviour,
        identify: identify::Behaviour,
        rendezvous: rendezvous::server::Behaviour,
        ping: ping::Behaviour,
    }

    // let mut swarm = libp2p::SwarmBuilder::with_new_identity()
    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_dns()?
        .with_behaviour(|key| {
            let mut ipfs_cfg = kad::Config::new(IPFS_PROTO_NAME);
            ipfs_cfg.set_query_timeout(Duration::from_secs(5 * 60));
            let ipfs_store = kad::store::MemoryStore::new(key.public().to_peer_id());
            Ok(Behaviour {
                ipfs: kad::Behaviour::with_config(key.public().to_peer_id(), ipfs_store, ipfs_cfg),
                identify: identify::Behaviour::new(identify::Config::new(
                    "gnostr/1.0.0".to_string(),
                    key.public(),
                )),
                rendezvous: rendezvous::server::Behaviour::new(
                    rendezvous::server::Config::default(),
                ),
                ping: ping::Behaviour::new(
                    ping::Config::new().with_interval(Duration::from_secs(1)),
                ),
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

    // Add the bootnodes to the local routing table. `libp2p-dns` built
    // into the `transport` resolves the `dnsaddr` when Kademlia tries
    // to dial these nodes.
    for peer in &BOOTNODES {
        //swarm.behaviour_mut().kademlia.set_mode(Some(Mode::Server));
        swarm
            .behaviour_mut()
            .ipfs
            .add_address(&peer.parse()?, "/dnsaddr/bootstrap.libp2p.io".parse()?);
        swarm
            .behaviour_mut()
            .kademlia
            .add_address(&peer.parse()?, "/dnsaddr/bootstrap.libp2p.io".parse()?);
    }

    // TODO get weeble/blockheight/wobble
    let listen_on = swarm.listen_on("/ip4/0.0.0.0/tcp/62649".parse().unwrap());
    log::debug!("listen_on={}", listen_on.unwrap());
    swarm.behaviour_mut().kademlia.set_mode(Some(Mode::Server));
    log::info!("swarm.local_peer_id()={:?}", swarm.local_peer_id());
    //net work is primed

    //run
    let result = run(&args, &mut swarm.behaviour_mut().kademlia)?;
    log::trace!("result={:?}", result);

    //push commit hashes and commit diffs

    // Read full lines from stdin
    let mut stdin = io::BufReader::new(io::stdin()).lines();

    // Listen on all interfaces and whatever port the OS assigns.
    // TODO get weeble/blockheight/wobble
    let listen_on = swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;
    log::debug!("listen_on={}", listen_on);

    // Kick it off.
    loop {
        select! {
                Ok(Some(line)) = stdin.next_line() => {
                    log::trace!("line.len()={}", line.len());
                    if line.len() <= 3 {
                    println!("{:?}", swarm.local_peer_id());
                    for address in swarm.external_addresses() {
                        println!("{:?}", address);
                    }
                    for peer in swarm.connected_peers() {
                        println!("{:?}", peer);
                    }
                    }
                    handle_input_line(&mut swarm.behaviour_mut().kademlia, line).await;
                }

                event = swarm.select_next_some() => match event {


                //match event

                    SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                        tracing::info!("Connected to {}", peer_id);
                    }
                    SwarmEvent::ConnectionClosed { peer_id, .. } => {
                        tracing::info!("Disconnected from {}", peer_id);
                    }
                    SwarmEvent::Behaviour(BehaviourEvent::Rendezvous(
                        rendezvous::server::Event::PeerRegistered { peer, registration },
                    )) => {
                        tracing::info!(
                            "Peer {} registered for namespace '{}'",
                            peer,
                            registration.namespace
                        );
                    }
                    SwarmEvent::Behaviour(BehaviourEvent::Rendezvous(
                        rendezvous::server::Event::DiscoverServed {
                            enquirer,
                            registrations,
                        },
                    )) => {
                        tracing::info!(
                            "Served peer {} with {} registrations",
                            enquirer,
                            registrations.len()
                        );
                    }
                //    other => {
                //        tracing::debug!("Unhandled {:?}", other);
                //    }

                SwarmEvent::NewListenAddr { address, .. } => {
                    log::debug!("Listening in {address:?}");
                }


                SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                    for (peer_id, multiaddr) in list {
                        swarm.behaviour_mut().kademlia.add_address(&peer_id, multiaddr);
                    }
                }



                SwarmEvent::Behaviour(BehaviourEvent::Kademlia(kad::Event::OutboundQueryProgressed { result, ..})) => {
                match result {
                    kad::QueryResult::GetProviders(Ok(kad::GetProvidersOk::FoundProviders { key, providers, .. })) => {
                        for peer in providers {
                            log::info!(
                                "Peer {peer:?} provides key {:?}",
                                std::str::from_utf8(key.as_ref()).unwrap()
                            );
                        }
                    }
                    kad::QueryResult::GetProviders(Err(err)) => {
                        //eprintln!("Failed to get providers: {err:?}");
                        log::trace!("Failed to get providers: {err:?}");
                    }
                    kad::QueryResult::GetRecord(Ok(
                        kad::GetRecordOk::FoundRecord(kad::PeerRecord {
                            record: kad::Record { key, value, .. },
                            ..
                        })
                    )) => {
                        println!(
                            "{{\"commit\":{:?},\"diff\":{:?}}}",
                            std::str::from_utf8(key.as_ref()).unwrap(),
                            std::str::from_utf8(&value).unwrap(),
                        );
                    }
                    kad::QueryResult::GetRecord(Ok(_)) => {}
                    kad::QueryResult::GetRecord(Err(err)) => {
                        //eprintln!("Failed to get record: {err:?}");
                        log::info!("Failed to get record: {err:?}");
                    }
                    kad::QueryResult::PutRecord(Ok(kad::PutRecordOk { key })) => {
                        log::info!(
                            "PUT {:?}",
                            std::str::from_utf8(key.as_ref()).unwrap()
                        );
                    }
                    kad::QueryResult::PutRecord(Err(err)) => {
                        //eprintln!("Failed to put record: {err:?}");
                        log::info!("Failed to put record: {err:?}");
                    }
                    kad::QueryResult::StartProviding(Ok(kad::AddProviderOk { key })) => {
                        log::info!(
                            "PUT_PROVIDER {:?}",
                            std::str::from_utf8(key.as_ref()).unwrap()
                        );
                    }
                    kad::QueryResult::StartProviding(Err(err)) => {
                        //eprintln!("Failed to put provider record: {err:?}");
                        log::trace!("Failed to put provider record: {err:?}");
                    }
                    _ => {}
                }
            }
            other => {
                tracing::debug!("Unhandled {:?}", other);
            }
                }
        }
    }
}

//fn commit_list(kademlia: &mut kad::Behaviour<MemoryStore>) {
//    let key = {
//            let Some(key) => kad::RecordKey::new(&key)
//    };
//
//    kademlia
//        .start_providing(key)
//        .expect("Failed to start providing key");
//}
async fn handle_input_line(kademlia: &mut kad::Behaviour<MemoryStore>, line: String) {
    let mut args = line.split(' ');

    match args.next() {
        Some("GET") => {
            let key = {
                match args.next() {
                    Some(key) => kad::RecordKey::new(&key),
                    None => {
                        eprintln!("Expected key");
                        eprint!("194> ");
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
                        eprint!("307> ");
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
                        eprint!("320> ");
                        return;
                    }
                }
            };
            let value = {
                match args.next() {
                    Some(value) => value.as_bytes().to_vec(),
                    None => {
                        eprintln!("Expected value");
                        eprint!("330> ");
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
                        eprint!("351> ");
                        return;
                    }
                }
            };

            kademlia
                .start_providing(key)
                .expect("Failed to start providing key");
        }
        Some("FETCH") => {
            let key = {
                match args.next() {
                    Some(key) => kad::RecordKey::new(&key),
                    None => {
                        eprintln!("Expected key");
                        eprint!("367> ");
                        return;
                    }
                }
            };
            kademlia.get_providers(key);

            //std::process::exit(0);
        }
        Some("QUIT") => {
            std::process::exit(0);
        }
        Some("Q") => {
            std::process::exit(0);
        }
        Some("EXIT") => {
            std::process::exit(0);
        }
        _ => {
            eprintln!("expected GET, GET_PROVIDERS, PUT or PUT_PROVIDER");
            eprint!("{}/gnostr> ", get_blockheight().await.expect("REASON"));
        }
    }
}

fn sig_matches(sig: &Signature, arg: &Option<String>) -> bool {
    match *arg {
        Some(ref s) => {
            sig.name().map(|n| n.contains(s)).unwrap_or(false)
                || sig.email().map(|n| n.contains(s)).unwrap_or(false)
        }
        None => true,
    }
}

fn log_message_matches(msg: Option<&str>, grep: &Option<String>) -> bool {
    match (grep, msg) {
        (&None, _) => true,
        (&Some(_), None) => false,
        (&Some(ref s), Some(msg)) => msg.contains(s),
    }
}

//this formats and prints the commit header/message
fn _print_commit_header(commit: &Commit) {
    println!("commit {}", commit.id());

    if commit.parents().len() > 1 {
        print!("Merge:");
        for id in commit.parent_ids() {
            print!(" {:.8}", id);
        }
        println!();
    }

    let author = commit.author();
    println!("Author: {}", author);
    _print_time(&author.when(), "Date:   ");
    println!();

    for line in String::from_utf8_lossy(commit.message_bytes()).lines() {
        println!("    {}", line);
    }
    println!();
}

//called from above
//part of formatting the output
fn _print_time(time: &Time, prefix: &str) {
    let (offset, sign) = match time.offset_minutes() {
        n if n < 0 => (-n, '-'),
        n => (n, '+'),
    };
    let (hours, minutes) = (offset / 60, offset % 60);
    let ts = time::Timespec::new(time.seconds() + (time.offset_minutes() as i64) * 60, 0);
    let time = time::at(ts);

    println!(
        "{}{} {}{:02}{:02}",
        prefix,
        time.strftime("%a %b %e %T %Y").unwrap(),
        sign,
        hours,
        minutes
    );
}

fn match_with_parent(
    repo: &Repository,
    commit: &Commit,
    parent: &Commit,
    opts: &mut DiffOptions,
) -> Result<bool, GitError> {
    let a = parent.tree()?;
    let b = commit.tree()?;
    let diff = repo.diff_tree_to_tree(Some(&a), Some(&b), Some(opts))?;
    Ok(diff.deltas().len() > 0)
}

fn run(args: &Args, kademlia: &mut kad::Behaviour<MemoryStore>) -> Result<(), GitError> {
    let path = args.flag_git_dir.as_ref().map(|s| &s[..]).unwrap_or(".");
    let repo = Repository::discover(path)?;

    let tag_names = &repo.tag_names(Some("")).expect("REASON");
    for tag in tag_names {
        //println!("println!={}", tag.unwrap());
        log::debug!("tag.unwrap()={}", tag.unwrap());
    }

    let mut revwalk = repo.revwalk()?;

    // Prepare the revwalk based on CLI parameters
    let base = if args.flag_reverse {
        git2::Sort::REVERSE
    } else {
        git2::Sort::NONE
    };
    revwalk.set_sorting(
        base | if args.flag_topo_order {
            git2::Sort::TOPOLOGICAL
        } else if args.flag_date_order {
            git2::Sort::TIME
        } else {
            git2::Sort::NONE
        },
    )?;
    for commit in &args.arg_commit {
        if commit.starts_with('^') {
            let obj = repo.revparse_single(&commit[1..])?;
            revwalk.hide(obj.id())?;
            continue;
        }
        let revspec = repo.revparse(commit)?;
        if revspec.mode().contains(git2::RevparseMode::SINGLE) {
            revwalk.push(revspec.from().unwrap().id())?;
        } else {
            let from = revspec.from().unwrap().id();
            let to = revspec.to().unwrap().id();
            revwalk.push(to)?;
            if revspec.mode().contains(git2::RevparseMode::MERGE_BASE) {
                let base = repo.merge_base(from, to)?;
                let o = repo.find_object(base, Some(ObjectType::Commit))?;
                revwalk.push(o.id())?;
            }
            revwalk.hide(from)?;
        }
    }
    if args.arg_commit.is_empty() {
        revwalk.push_head()?;
    }

    // Prepare our diff options and pathspec matcher
    let (mut diffopts, mut diffopts2) = (DiffOptions::new(), DiffOptions::new());
    for spec in &args.arg_spec {
        diffopts.pathspec(spec);
        diffopts2.pathspec(spec);
    }
    let ps = Pathspec::new(args.arg_spec.iter())?;

    // Filter our revwalk based on the CLI parameters
    macro_rules! filter_try {
        ($e:expr) => {
            match $e {
                Ok(t) => t,
                Err(e) => return Some(Err(e)),
            }
        };
    }
    let revwalk = revwalk
        .filter_map(|id| {
            let id = filter_try!(id);
            let commit = filter_try!(repo.find_commit(id));
            let parents = commit.parents().len();
            if parents < args.min_parents() {
                return None;
            }
            if let Some(n) = args.max_parents() {
                if parents >= n {
                    return None;
                }
            }
            if !args.arg_spec.is_empty() {
                match commit.parents().len() {
                    0 => {
                        let tree = filter_try!(commit.tree());
                        let flags = git2::PathspecFlags::NO_MATCH_ERROR;
                        if ps.match_tree(&tree, flags).is_err() {
                            return None;
                        }
                    }
                    _ => {
                        let m = commit.parents().all(|parent| {
                            match_with_parent(&repo, &commit, &parent, &mut diffopts)
                                .unwrap_or(false)
                        });
                        if !m {
                            return None;
                        }
                    }
                }
            }
            if !sig_matches(&commit.author(), &args.flag_author) {
                return None;
            }
            if !sig_matches(&commit.committer(), &args.flag_committer) {
                return None;
            }
            if !log_message_matches(commit.message(), &args.flag_grep) {
                return None;
            }
            Some(Ok(commit))
        })
        .skip(args.flag_skip.unwrap_or(0))
        .take(args.flag_max_count.unwrap_or(!0));

    let tag_names = &repo.tag_names(Some("")).expect("REASON");
    log::debug!("tag_names.len()={}", tag_names.len());
    for tag in tag_names {
        log::trace!("{}", tag.unwrap());
        let key = kad::RecordKey::new(&format!("{}", &tag.unwrap()));

        ////push commit key and commit content as value
        ////let value = Vec::from(commit.message_bytes().clone());
        //let value = Vec::from(commit.message_bytes());
        //let record = kad::Record {
        //    key,
        //    value,
        //    publisher: None,
        //    expires: None,
        //};
        //kademlia
        //    .put_record(record, kad::Quorum::One)
        //    .expect("Failed to store record locally.");
        //let key = kad::RecordKey::new(&format!("{}", &commit.id()));
        //kademlia
        //    .start_providing(key)
        //    .expect("Failed to start providing key");
    }

    // print!
    for commit in revwalk {
        let commit = commit?;

        //TODO construct nostr event
        //commit_privkey
        let commit_privkey: String = String::from(format!("{:0>64}", &commit.id().clone()));
        log::trace!("commit_privkey={}", commit_privkey);

        //commit.id
        //we want to broadcast as provider for the actual commit.id()
        log::debug!("&commit.id={}", &commit.id());

        let key = kad::RecordKey::new(&format!("{}", &commit.id()));

        //push commit key and commit content as value
        //let value = Vec::from(commit.message_bytes().clone());
        let value = Vec::from(commit.message_bytes());
        let record = kad::Record {
            key,
            value,
            publisher: None,
            expires: None,
        };
        kademlia
            .put_record(record, kad::Quorum::One)
            .expect("Failed to store record locally.");
        let key = kad::RecordKey::new(&format!("{}", &commit.id()));
        kademlia
            .start_providing(key)
            .expect("Failed to start providing key");

        //println!("commit.tree_id={}", &commit.tree_id());
        //println!("commit.tree={:?}", &commit.tree());
        //println!("commit.raw={:?}", &commit.raw()); //pointer?

        //println!("commit.message={:?}", &commit.message()); //commit diff body
        let mut part_index = 0;
        let commit_parts = commit.message().clone().unwrap().split("\n");
        //let parts = commit.message().clone().unwrap().split("gpgsig");
        for part in commit_parts {
            log::debug!(
                "commit.message part={}:{}",
                part_index,
                part.replace("", "")
            );
            part_index += 1;
        }
        part_index = 0;

        ////println!("commit.message_bytes{:?}", &commit.message_bytes());
        //println!("commit.message_encoding={:?}", &commit.message_encoding());
        //println!("commit.message_raw={:?}", &commit.message_raw());
        ////println!("commit.message_raw_bytes={:?}", &commit.message_raw_bytes());

        //raw_header
        //println!("commit.raw_header={:?}", commit.raw_header());
        let raw_header_parts = commit.raw_header().clone().unwrap().split("\n");
        for part in raw_header_parts {
            log::debug!("raw_header part={}:{}", part_index, part.replace("", ""));
            part_index += 1;
        }
        //parts = commit.raw_header().clone().unwrap().split("gpgsig");
        //for part in parts {
        //    println!("raw_header gpgsig part={}", part.replace("", ""))
        //};
        ////println!("commit.header_field_bytes={:?}", &commit.header_field_bytes());
        ////println!("commit.raw_header_bytes={:?}", &commit.raw_header_bytes());
        //println!("commit.summary={:?}", &commit.summary());
        ////println!("commit.summary_bytes={:?}", &commit.summary_bytes());
        //println!("commit.body={:?}", &commit.body());
        ////println!("commit.body_bytes={:?}", &commit.body_bytes());
        //println!("commit.time={:?}", &commit.time());
        //println!("commit.author={:?}", &commit.author().name());
        //print_commit_header(&commit);

        if !args.flag_patch || commit.parents().len() > 1 {
            continue;
        }

        let a = if commit.parents().len() == 1 {
            //we have arrived at the initial commit
            let parent = commit.parent(0)?;
            Some(parent.tree()?)
        } else {
            None
        };

        //print the diff content
        //push diff to commit_key
        let b = commit.tree()?;
        let diff = repo.diff_tree_to_tree(a.as_ref(), Some(&b), Some(&mut diffopts2))?;
        diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
            match line.origin() {
                ' ' | '+' | '-' => print!("{}", line.origin()),
                _ => {}
            }
            print!(
                "509:==================>{}",
                str::from_utf8(line.content()).unwrap()
            );
            true
        })?;
    }

    Ok(())
}

//TODO Server Mode or ??
#[derive(Parser)]
struct Args {
    #[structopt(name = "topo-order", long)]
    /// sort commits in topological order
    flag_topo_order: bool,
    #[structopt(name = "date-order", long)]
    /// sort commits in date order
    flag_date_order: bool,
    #[structopt(name = "reverse", long)]
    /// sort commits in reverse
    flag_reverse: bool,
    #[structopt(name = "author", long)]
    /// author to sort by
    flag_author: Option<String>,
    #[structopt(name = "committer", long)]
    /// committer to sort by
    flag_committer: Option<String>,
    #[structopt(name = "pat", long = "grep")]
    /// pattern to filter commit messages by
    flag_grep: Option<String>,
    #[structopt(name = "dir", long = "git-dir")]
    /// alternative git directory to use
    flag_git_dir: Option<String>,
    #[structopt(name = "skip", long)]
    /// number of commits to skip
    flag_skip: Option<usize>,
    #[structopt(name = "max-count", short = 'n', long)]
    /// maximum number of commits to show
    flag_max_count: Option<usize>,
    #[structopt(name = "merges", long)]
    /// only show merge commits
    flag_merges: bool,
    #[structopt(name = "no-merges", long)]
    /// don't show merge commits
    flag_no_merges: bool,
    #[structopt(name = "no-min-parents", long)]
    /// don't require a minimum number of parents
    flag_no_min_parents: bool,
    #[structopt(name = "no-max-parents", long)]
    /// don't require a maximum number of parents
    flag_no_max_parents: bool,
    #[structopt(name = "max-parents")]
    /// specify a maximum number of parents for a commit
    flag_max_parents: Option<usize>,
    #[structopt(name = "min-parents")]
    /// specify a minimum number of parents for a commit
    flag_min_parents: Option<usize>,
    #[structopt(name = "patch", long, short)]
    /// show commit diff
    flag_patch: bool,
    #[structopt(name = "commit")]
    arg_commit: Vec<String>,
    #[structopt(name = "spec", last = true)]
    arg_spec: Vec<String>,
}

impl Args {
    fn min_parents(&self) -> usize {
        if self.flag_no_min_parents {
            return 0;
        }
        self.flag_min_parents
            .unwrap_or(if self.flag_merges { 2 } else { 0 })
    }

    fn max_parents(&self) -> Option<usize> {
        if self.flag_no_max_parents {
            return None;
        }
        self.flag_max_parents
            .or(if self.flag_no_merges { Some(1) } else { None })
    }
}
