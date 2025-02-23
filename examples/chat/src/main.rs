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
    error::Error,
    hash::{Hash, Hasher},
    thread,
    time::Duration,
};

use futures::stream::StreamExt;
use git2::Config;
use git2::ErrorCode;
use git2::Repository;

use ureq::{Agent, AgentBuilder, Error as UreqError};

use libp2p::{
    gossipsub, mdns, noise,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux,
};
use tokio::{io, io::AsyncBufReadExt, select};
use tracing_subscriber::EnvFilter;
use std::env;
use tracing_subscriber::fmt::format;

use tracing_subscriber::fmt::format::Format;

use env_logger::{Builder, Env};
use log::{debug, error, info, trace, warn};
use std::env::args;

// We create a custom network behaviour that combines Gossipsub and Mdns.
#[derive(NetworkBehaviour)]
struct MyBehaviour {
    gossipsub: gossipsub::Behaviour,
    mdns: mdns::tokio::Behaviour,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    //let _ = tracing_subscriber::fmt()
    //    .with_env_filter(EnvFilter::from_default_env())
    //    .try_init();

    let args_vec: Vec<String> = env::args().collect();

    if args_vec.len() < 2 {
        info!("Please provide at least one argument.");
        ()
    }

    if let Some(log_level) = args().nth(1) {
        Builder::from_env(Env::default().default_filter_or(log_level)).init();
    } else {
        Builder::from_env(Env::default().default_filter_or("warn")).init();
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
        .with_behaviour(|key| {
            // To content-address message, we can take the hash of message and use it as an ID.
            let message_id_fn = |message: &gossipsub::Message| {
                let mut s = DefaultHasher::new();
                message.data.hash(&mut s);
                gossipsub::MessageId::from(s.finish().to_string())
            };

            // Set a custom gossipsub configuration
            let gossipsub_config = gossipsub::ConfigBuilder::default()
                .heartbeat_interval(Duration::from_secs(1)) // This is set to aid debugging by not cluttering the log space
                //REF https://docs.rs/gossipsub/latest/gossipsub/enum.ValidationMode.html
                .validation_mode(gossipsub::ValidationMode::Permissive)
                .message_id_fn(message_id_fn) // content-address messages. No two messages of the same content will be propagated.
                .build()
                .map_err(|msg| io::Error::new(io::ErrorKind::Other, msg))?; // Temporary hack because `build` does not return a proper `std::error::Error`.

            // build a gossipsub network behaviour
            let gossipsub = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gossipsub_config,
            )?;

            let mdns =
                mdns::tokio::Behaviour::new(mdns::Config::default(), key.public().to_peer_id())?;
            Ok(MyBehaviour { gossipsub, mdns })
        })?
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
    let topic = gossipsub::IdentTopic::new(format!("{:0>64}", commit.id()));
    println!("TOPIC> {:0>64}", topic);
    // subscribes to our topic
    swarm.behaviour_mut().gossipsub.subscribe(&topic)?;

    // Read full lines from stdin
    let mut stdin = io::BufReader::new(io::stdin()).lines();

    // Listen on all interfaces and whatever port the OS assigns
    swarm.listen_on("/ip4/0.0.0.0/udp/0/quic-v1".parse()?)?;
    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    info!("Enter messages via STDIN and they will be sent to connected peers using Gossipsub");

    let mut mempool_url = "https://mempool.space/api/blocks/tip/height";
    let sweetsats_url = "https://mempool.sweetsats.io/api/blocks/tip/height";
    let gob_sv_url = "https://bitcoin.gob.sv/api/blocks/tip/height";

    let mut handles = Vec::new();
    let ureq_test = tokio::spawn(async move {
        match ureq::get(mempool_url).call() {
            Ok(response) => {
                /* it worked */

                debug!("{response:?}");
            }
            Err(UreqError::Status(code, response)) => {
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
            Err(UreqError::Status(code, response)) => {
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
        let mut handles = Vec::new();
        let ureq_test = tokio::spawn(async move {
            match ureq::get(mempool_url).call() {
                Ok(response) => {
                    /* it worked */
                    debug!("{response:?}");
                }
                Err(UreqError::Status(code, response)) => {
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

                    debug!("{response:?}");
                }
                Err(UreqError::Status(code, response)) => {
                    //debug!("{response:?}");
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

        select! {
            Ok(Some(line)) = stdin.next_line() => {
                if let Err(e) = swarm
                    .behaviour_mut().gossipsub
                    //SEND
                    .publish(topic.clone(), line.as_bytes()) {

                    //formatting for error prompt
                    //let s = tokio::spawn(async move {
                    //    let agent: Agent = ureq::AgentBuilder::new()
                    //        .timeout_read(Duration::from_secs(1))
                    //        .timeout_write(Duration::from_secs(1))
                    //        .build();
                    //    let body: String = agent
                    //        .get(mempool_url)
                    //        .call()
                    //        .expect("")
                    //        .into_string()
                    //        .expect("");

                    //    print!("\n304:{body}> {e:?}",);
                    //});
                    //let mut handles = Vec::new();
                    //handles.push(s);

                    //for i in handles {
                    //    i.await.unwrap(); //write to term
                    //}
                }

            //let s = tokio::spawn(async move {
            //    let agent: Agent = ureq::AgentBuilder::new()
            //        .timeout_read(Duration::from_secs(1))
            //        .timeout_write(Duration::from_secs(1))
            //        .build();
            //    let body: String = agent
            //        .get("https://mempool.space/api/blocks/tip/height")
            //        .call()
            //        .expect("")
            //        .into_string()
            //        .expect("");

            //    print!("\n326:{body}> ");
            //});
            //let mut handles = Vec::new();
            //handles.push(s);

            //for i in handles {
            //    i.await.unwrap();
            //}


            //}

            //let s = tokio::spawn(async move {
            //    let agent: Agent = ureq::AgentBuilder::new()
            //        .timeout_read(Duration::from_secs(1))
            //        .timeout_write(Duration::from_secs(1))
            //        .build();
            //    let body: String = agent
            //        .get(mempool_url)
            //        .call()
            //        .expect("")
            //        .into_string()
            //        .expect("");

            //    print!("\n350:{body}> ");
            //});
            //let mut handles = Vec::new();
            //handles.push(s);

            //for i in handles {
            //    i.await.unwrap();
            //}

            }
            event = swarm.select_next_some() => match event {
                SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                    for (peer_id, _multiaddr) in list {
                        debug!("mDNS discovered a new peer: {peer_id}");
                        swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                    }
                },
                SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
                    for (peer_id, _multiaddr) in list {
                        debug!("mDNS discover peer has expired: {peer_id}");
                        swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);
                    }
                },
                //we recieve message
                //and print immediately
                //no formatting
                SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                    propagation_source: peer_id,
                    message_id: id,
                    message,
                })) => {

                      print!(
                          "{} <{peer_id}", String::from_utf8_lossy(&message.data),
                      );

                    let s = tokio::spawn(async move {
                        let agent: Agent = ureq::AgentBuilder::new()
                            .timeout_read(Duration::from_secs(1))
                            .timeout_write(Duration::from_secs(1))
                            .build();
                        let body: String = agent.get(mempool_url)
                            .call().expect("")
                            .into_string().expect("");

                        //immediately print a new prompt
                        print!(
                            "\n{body}/{id}> ",
                        )
                    });

                    let mut handles = Vec::new();
                    handles.push(s);

                    for i in handles {
                        i.await.unwrap();
                    }
                },
                SwarmEvent::NewListenAddr { address, .. } => {
                    let s = tokio::spawn(async move {
                        let agent: Agent = ureq::AgentBuilder::new()
                            .timeout_read(Duration::from_secs(1))
                            .timeout_write(Duration::from_secs(1))
                            .build();
                        let body: String = agent.get(mempool_url)
                            .call().expect("")
                            .into_string().expect("");

                        print!(
                            "\n{address:?}/{body}> ",
                        )
                    });

                    let mut handles = Vec::new();
                    handles.push(s);

                    for i in handles {
                        i.await.unwrap();
                    }

                    //info!("Local node is listening on {address}");
                }
                _ => {
                    let s = tokio::spawn(async move {
                        let agent: Agent = ureq::AgentBuilder::new()
                            .timeout_read(Duration::from_secs(1))
                            .timeout_write(Duration::from_secs(1))
                            .build();
                        let body: String = agent.get(mempool_url)
                            .call().expect("")
                            .into_string().expect("");

                        print!(
                            "\n{body}> ",
                        )
                    });

                    let mut handles = Vec::new();
                    handles.push(s);

                    for i in handles {
                        i.await.unwrap();
                    }




                }
            }
        }
    }
}
