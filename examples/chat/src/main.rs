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

#![doc = include_str!("../README.md")]

use std::{
    collections::hash_map::DefaultHasher,
    error::Error as StdError,
    hash::{Hash, Hasher},
    //thread,
    time::Duration,
};

use futures::stream::StreamExt;
//use git2::Config;
//use git2::ErrorCode;
use git2::Repository;

use ureq::{Agent, /*AgentBuilder, */ Error};

use libp2p::{
    gossipsub, mdns, noise,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux,
};
use std::env;
use tokio::{io, io::AsyncBufReadExt, select, task};

use env_logger::{Builder, Env};
use log::{debug, error, info, trace};
use std::env::args;

// We create a custom network behaviour that combines Gossipsub and Mdns.
#[derive(NetworkBehaviour)]
struct MyBehaviour {
    gossipsub: gossipsub::Behaviour,
    mdns: mdns::tokio::Behaviour,
}

fn get_blockheight() -> String {
    let blockheight = String::from("000000");
    blockheight
}

async fn fetch_data_async(url: String) -> Result<ureq::Response, ureq::Error> {
    task::spawn_blocking(move || {
        let response = ureq::get(&url).call();
        response
    })
    .await
    .unwrap() // Handle potential join errors
}

async fn async_prompt(mempool_url: String) -> String {
    let s = tokio::spawn(async move {
        let agent: Agent = ureq::AgentBuilder::new()
            .timeout_read(Duration::from_secs(10))
            .timeout_write(Duration::from_secs(10))
            .build();
        let body: String = agent
            .get(&mempool_url)
            .call()
            .expect("")
            .into_string()
            .expect("mempool_url:body:into_string:fail!");

        body
    });

    s.await.unwrap()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn StdError>> {
    //let _ = tracing_subscriber::fmt()
    //    .with_env_filter(EnvFilter::from_default_env())
    //    .try_init();

    let args_vec: Vec<String> = env::args().collect();

    if args_vec.len() < 2 {
        info!("Please provide at least one argument.");
        ()
    }

    if let Some(log_level) = args().nth(1) {
        Builder::from_env(
            Env::default().default_filter_or(log_level + ",libp2p_gossipsub::behaviour=error"),
        )
        .init();
    } else {
        Builder::from_env(
            Env::default().default_filter_or("none,libp2p_gossipsub::behaviour=error"),
        )
        .init();
    }

    trace!("Arguments:");
    for (index, arg) in args_vec.iter().enumerate() {
        if Some(index) == Some(0) {
            trace!("Some(index) = Some(0):  {}: {}", index, arg);
        } else {
            trace!("  {}: {}", index, arg);
        }
    }

    let mut swarm = libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_quic()
        //a behaviour
        .with_behaviour(|key| {
            // To content-address message, we can take the hash of message and use it as an ID.
            let message_id_fn = |message: &gossipsub::Message| {
                let mut s = DefaultHasher::new();
                message.data.hash(&mut s);
                debug!("message:\n{0:?}", message);
                debug!("message.data:\n{0:?}", message.data);
                debug!("message.source:\n{0:?}", message.source);
                debug!("message.source:\n{0:1?}", message.source);
                debug!("message.source.peer_id:\n{0:2?}", message.source.unwrap());
                //TODO https://docs.rs/gossipsub/latest/gossipsub/trait.DataTransform.html
                //send Recieved message back
                debug!(
                    "message.source.peer_id:\n{0:3}",
                    message.source.unwrap().to_string()
                );
                debug!("message.sequence_number:\n{0:?}", message.sequence_number);
                debug!("message.topic:\n{0:?}", message.topic);
                debug!("message.topic.hash:\n{0:0}", message.topic.clone());
                //println!("{:?}", s);
                gossipsub::MessageId::from(s.finish().to_string())
            };

            // Set a custom gossipsub configuration
            let gossipsub_config = gossipsub::ConfigBuilder::default()
                // This is set to aid debugging by not cluttering the log space
                .heartbeat_interval(Duration::from_secs(1))
                // REF https://docs.rs/gossipsub/latest/gossipsub/enum.ValidationMode.html
                .validation_mode(gossipsub::ValidationMode::Permissive)
                // content-address messages. No two messages of the same content will be propagated.
                .message_id_fn(message_id_fn)
                .build()
                // Temporary hack because `build` does not return a proper `std::error::Error`.
                .map_err(|msg| io::Error::new(io::ErrorKind::Other, msg))?;

            // build a gossipsub network behaviour
            let gossipsub = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gossipsub_config,
            )?;

            let mdns =
                mdns::tokio::Behaviour::new(mdns::Config::default(), key.public().to_peer_id())?;

            Ok(MyBehaviour { gossipsub, mdns })
        })? // end of behaviour
        .build();

    // Create a Gossipsub topic
    // Open the Git repository
    let repo = Repository::discover(".")?; // Opens the repository in the current directory

    // Get the reference to HEAD
    let head = repo.head()?;

    // Print the name of HEAD (e.g., "refs/heads/main" or "HEAD")
    debug!("HEAD: {}", head.name().unwrap_or("HEAD"));

    // Get the commit object that HEAD points to
    let commit = head.peel_to_commit()?;

    // Print the commit ID (SHA-1 hash)
    debug!("Commit ID: {}", commit.id());

    // Optionally, print other commit information
    debug!(
        "Commit message: {}",
        commit.message().unwrap_or("No message")
    );

    //TODO add cli topic arg
    //commit.id is padded to fit sha256/nostr privkey context
    let topic = gossipsub::IdentTopic::new(format!("{:0>64}", commit.id()));
    println!("TOPIC> {:0>64}", topic);

    // subscribes to our topic
    // TODO check if commit.id HEAD changed within the loop
    swarm.behaviour_mut().gossipsub.subscribe(&topic)?;
    // Listen on all interfaces and whatever port the OS assigns
    swarm.listen_on("/ip4/0.0.0.0/udp/0/quic-v1".parse()?)?;
    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    // Read full lines from stdin
    // https://doc.rust-lang.org/std/io/trait.BufRead.html#method.read_line
    // https://doc.rust-lang.org/std/io/fn.stdin.html#examples
    let mut stdin = io::BufReader::new(io::stdin()).lines();
    println!("Enter messages via STDIN");

    let mut mempool_url = "https://mempool.space/api/blocks/tip/height";
    let sweetsats_url = "https://mempool.sweetsats.io/api/blocks/tip/height";
    let gob_sv_url = "https://bitcoin.gob.sv/api/blocks/tip/height";

    let mempool_url_vec = vec![
        "https://mempool.space/api/blocks/tip/height",
        "https://mempool.sweetsats.io/api/blocks/tip/height",
        "https://bitcoin.gob.sv/api/blocks/tip/height",
    ];

    let prompt = async_prompt(mempool_url.to_string()).await;
    println!("A_GNOSTR/{prompt}> ");

    let mut handles = Vec::new();
    let ureq_test = tokio::spawn(async move {
        match ureq::get(mempool_url).call() {
            Ok(response) => {
                /* it worked */

                debug!("{response:?}");
            }
            Err(Error::Status(code, response)) => {
                debug!("{response:?}");
                mempool_url = sweetsats_url;
                //mempool_url = gob_sv_url;
                /* the server returned an unexpected status
                code (such as 400, 500 etc) */
                error!("{mempool_url:?}/{code:?}/{response:?}");
            }
            Err(_) => { /* some kind of io/transport error */ }
        }
    });
    handles.push(ureq_test);
    for i in handles {
        i.await.unwrap();
    }

    let mut handles = Vec::new();
    let ureq_test = tokio::spawn(async move {
        match ureq::get(mempool_url).call() {
            Ok(response) => {
                /* it worked */

                debug!("{response:?}");
            }
            Err(Error::Status(code, response)) => {
                debug!("{response:?}");
                //mempool_url = sweetsats_url;
                mempool_url = gob_sv_url;
                /* the server returned an unexpected status
                code (such as 400, 500 etc) */
                error!("{mempool_url:?}/{code:?}/{response:?}");
            }
            Err(_) => { /* some kind of io/transport error */ }
        }
    });
    handles.push(ureq_test);
    for i in handles {
        i.await.unwrap();
    }

    let mut handles = Vec::new();
    let s = tokio::spawn(async move {
        let agent: Agent = ureq::AgentBuilder::new()
            .timeout_read(Duration::from_secs(10))
            .timeout_write(Duration::from_secs(10))
            .build();
        let body: String = agent
            .get(mempool_url)
            .call()
            .expect("")
            .into_string()
            .expect("mempool_url:body:into_string:fail!");

        print!("{body}> ",)
    });
    //let mut handles = Vec::new();
    handles.push(s);

    for i in handles {
        i.await.unwrap();
    }
    // Kick it off
    loop {
        let prompt = async_prompt(mempool_url.to_string()).await;
        //print!("308:GNOSTR/{prompt}>");

        select! {
            Ok(Some(line)) = stdin.next_line() => {
                if line.len() == 0 {
                        print!("\nGNOSTR/{prompt}> print help message");

                } else {
                if line.len() == 1 {
                    //print!("334:line.len()={}", line.len());

                    let char_index = 0;
                    for c in line.chars() {
                        if c == ':' && char_index == 0 {
                        println!("\ndetected command prompt");
                        println!("\nc={c}:{char_index}");
                        //enter another mode
                        } else
                        if c == '\\' && char_index == 0 {
                        println!("\ndetected command prompt");
                        println!("\nc={c}:{char_index}");
                        //enter another mode
                        } else {}
                    }
                    //line = String::from("");
                } else {
                    //print!("350line.len()={}", line.len());

                    //detect compose/send command
                    let mut char_index = 0;
                    for c in line.chars() {
                        if c == ':' && char_index == 0 {
                            println!("\nc={c}:{char_index}");
                            char_index += 1;
                        } else


                        if c == 'c' && char_index == 1 {
                            //compose command
                            println!("\ndetected command prompt");
                            println!("\nc={c}:{char_index}");
                            char_index += 1;
                            let line = "detected compose prompt".to_string();
                        } else



                        if c == 's' && char_index == 1 {
                            //send command
                            println!("\ndetected command prompt");
                            println!("\nc={c}:{char_index}");
                            char_index += 1;
                            let line = String::from("detected compose prompt");
                        } else {

                            if let Err(e) = swarm
                                .behaviour_mut().gossipsub
                                //SEND
                                .publish(topic.clone(), line.as_bytes()) { /*error!("{e}");*/ }

                            }

                        }

                    }

                //if let Err(e) = swarm
                //    .behaviour_mut().gossipsub
                //    //SEND
                //    .publish(topic.clone(), line.as_bytes()) { error!("{e}"); }
                //}
            }
            }
            event = swarm.select_next_some() => match event {
                //NOTE MyBehaviour
                SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                    for (peer_id, _multiaddr) in list {
                        debug!("mDNS discovered a new peer: {peer_id}");
                        swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                    }
                },
                //NOTE MyBehaviour
                SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
                    for (peer_id, _multiaddr) in list {
                        debug!("mDNS discover peer has expired: {peer_id}");
                        swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);
                    }
                },
                //we recieve message
                //and print immediately
                //no formatting
                //NOTE MyBehaviour
                SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                    propagation_source: peer_id,
                    message_id: id,
                    message,
                })) => {

                      print!(
                          "\n{} <{peer_id}", String::from_utf8_lossy(&message.data),
                      );

                      print!("\nGNOSTR/{prompt}> ");

                },
                //NOTE NOT! MyBehaviour
                SwarmEvent::ConnectionEstablished { peer_id, connection_id, endpoint, .. } => {
                    debug!("\nSwarmEvent::NewExtrernalAddrOfPeer:{peer_id}/{connection_id}\n{endpoint:?}");
                }
                //NOTE NOT! MyBehaviour
                SwarmEvent::NewListenAddr { address, .. } => {

                        print!("\n<{address}");

                }
                _ => {
                        debug!("424:GNOSTR/{prompt}> ");
                }
            }
        }
    }
}
