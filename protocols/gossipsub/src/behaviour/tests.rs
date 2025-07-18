// Copyright 2020 Sigma Prime Pty Ltd.
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

// Collection of tests for the gossipsub network behaviour

use std::{future, net::Ipv4Addr, thread::sleep};

use asynchronous_codec::{Decoder, Encoder};
use byteorder::{BigEndian, ByteOrder};
use bytes::BytesMut;
use libp2p_core::ConnectedPoint;
use rand::Rng;

use super::*;
use crate::{
    config::{ConfigBuilder, TopicMeshConfig},
    protocol::GossipsubCodec,
    rpc::Receiver,
    subscription_filter::WhitelistSubscriptionFilter,
    types::RpcIn,
    IdentTopic as Topic,
};

#[derive(Default, Debug)]
struct InjectNodes<D, F> {
    peer_no: usize,
    topics: Vec<String>,
    to_subscribe: bool,
    gs_config: Config,
    explicit: usize,
    outbound: usize,
    scoring: Option<(PeerScoreParams, PeerScoreThresholds)>,
    data_transform: D,
    subscription_filter: F,
    peer_kind: Option<PeerKind>,
}

impl<D, F> InjectNodes<D, F>
where
    D: DataTransform + Default + Clone + Send + 'static,
    F: TopicSubscriptionFilter + Clone + Default + Send + 'static,
{
    #[allow(clippy::type_complexity)]
    pub(crate) fn create_network(
        self,
    ) -> (
        Behaviour<D, F>,
        Vec<PeerId>,
        HashMap<PeerId, Receiver>,
        Vec<TopicHash>,
    ) {
        let keypair = libp2p_identity::Keypair::generate_ed25519();
        // create a gossipsub struct
        let mut gs: Behaviour<D, F> = Behaviour::new_with_subscription_filter_and_transform(
            MessageAuthenticity::Signed(keypair),
            self.gs_config,
            self.subscription_filter,
            self.data_transform,
        )
        .unwrap();

        if let Some((scoring_params, scoring_thresholds)) = self.scoring {
            gs.with_peer_score(scoring_params, scoring_thresholds)
                .unwrap();
        }

        let mut topic_hashes = vec![];

        // subscribe to the topics
        for t in self.topics {
            let topic = Topic::new(t);
            gs.subscribe(&topic).unwrap();
            topic_hashes.push(topic.hash().clone());
        }

        // build and connect peer_no random peers
        let mut peers = vec![];
        let mut receivers = HashMap::new();

        let empty = vec![];
        for i in 0..self.peer_no {
            let (peer, receiver) = add_peer_with_addr_and_kind(
                &mut gs,
                if self.to_subscribe {
                    &topic_hashes
                } else {
                    &empty
                },
                i < self.outbound,
                i < self.explicit,
                Multiaddr::empty(),
                self.peer_kind.or(Some(PeerKind::Gossipsubv1_1)),
            );
            peers.push(peer);
            receivers.insert(peer, receiver);
        }

        (gs, peers, receivers, topic_hashes)
    }

    fn peer_no(mut self, peer_no: usize) -> Self {
        self.peer_no = peer_no;
        self
    }

    fn topics(mut self, topics: Vec<String>) -> Self {
        self.topics = topics;
        self
    }

    #[allow(clippy::wrong_self_convention)]
    fn to_subscribe(mut self, to_subscribe: bool) -> Self {
        self.to_subscribe = to_subscribe;
        self
    }

    fn gs_config(mut self, gs_config: Config) -> Self {
        self.gs_config = gs_config;
        self
    }

    fn explicit(mut self, explicit: usize) -> Self {
        self.explicit = explicit;
        self
    }

    fn outbound(mut self, outbound: usize) -> Self {
        self.outbound = outbound;
        self
    }

    fn scoring(mut self, scoring: Option<(PeerScoreParams, PeerScoreThresholds)>) -> Self {
        self.scoring = scoring;
        self
    }

    fn subscription_filter(mut self, subscription_filter: F) -> Self {
        self.subscription_filter = subscription_filter;
        self
    }

    fn peer_kind(mut self, peer_kind: PeerKind) -> Self {
        self.peer_kind = Some(peer_kind);
        self
    }
}

fn inject_nodes<D, F>() -> InjectNodes<D, F>
where
    D: DataTransform + Default + Clone + Send + 'static,
    F: TopicSubscriptionFilter + Clone + Default + Send + 'static,
{
    InjectNodes::default()
}

fn inject_nodes1() -> InjectNodes<IdentityTransform, AllowAllSubscriptionFilter> {
    InjectNodes::<IdentityTransform, AllowAllSubscriptionFilter>::default()
}

// helper functions for testing

fn add_peer<D, F>(
    gs: &mut Behaviour<D, F>,
    topic_hashes: &[TopicHash],
    outbound: bool,
    explicit: bool,
) -> (PeerId, Receiver)
where
    D: DataTransform + Default + Clone + Send + 'static,
    F: TopicSubscriptionFilter + Clone + Default + Send + 'static,
{
    add_peer_with_addr(gs, topic_hashes, outbound, explicit, Multiaddr::empty())
}

fn add_peer_with_addr<D, F>(
    gs: &mut Behaviour<D, F>,
    topic_hashes: &[TopicHash],
    outbound: bool,
    explicit: bool,
    address: Multiaddr,
) -> (PeerId, Receiver)
where
    D: DataTransform + Default + Clone + Send + 'static,
    F: TopicSubscriptionFilter + Clone + Default + Send + 'static,
{
    add_peer_with_addr_and_kind(
        gs,
        topic_hashes,
        outbound,
        explicit,
        address,
        Some(PeerKind::Gossipsubv1_1),
    )
}

fn add_peer_with_addr_and_kind<D, F>(
    gs: &mut Behaviour<D, F>,
    topic_hashes: &[TopicHash],
    outbound: bool,
    explicit: bool,
    address: Multiaddr,
    kind: Option<PeerKind>,
) -> (PeerId, Receiver)
where
    D: DataTransform + Default + Clone + Send + 'static,
    F: TopicSubscriptionFilter + Clone + Default + Send + 'static,
{
    let peer = PeerId::random();
    let endpoint = if outbound {
        ConnectedPoint::Dialer {
            address,
            role_override: Endpoint::Dialer,
            port_use: PortUse::Reuse,
        }
    } else {
        ConnectedPoint::Listener {
            local_addr: Multiaddr::empty(),
            send_back_addr: address,
        }
    };

    let sender = Sender::new(gs.config.connection_handler_queue_len());
    let receiver = sender.new_receiver();
    let connection_id = ConnectionId::new_unchecked(0);
    gs.connected_peers.insert(
        peer,
        PeerDetails {
            kind: kind.unwrap_or(PeerKind::Floodsub),
            outbound,
            connections: vec![connection_id],
            topics: Default::default(),
            sender,
            dont_send: LinkedHashMap::new(),
        },
    );

    gs.on_swarm_event(FromSwarm::ConnectionEstablished(ConnectionEstablished {
        peer_id: peer,
        connection_id,
        endpoint: &endpoint,
        failed_addresses: &[],
        other_established: 0, // first connection
    }));
    if let Some(kind) = kind {
        gs.on_connection_handler_event(
            peer,
            ConnectionId::new_unchecked(0),
            HandlerEvent::PeerKind(kind),
        );
    }
    if explicit {
        gs.add_explicit_peer(&peer);
    }
    if !topic_hashes.is_empty() {
        gs.handle_received_subscriptions(
            &topic_hashes
                .iter()
                .cloned()
                .map(|t| Subscription {
                    action: SubscriptionAction::Subscribe,
                    topic_hash: t,
                })
                .collect::<Vec<_>>(),
            &peer,
        );
    }
    (peer, receiver)
}

fn disconnect_peer<D, F>(gs: &mut Behaviour<D, F>, peer_id: &PeerId)
where
    D: DataTransform + Default + Clone + Send + 'static,
    F: TopicSubscriptionFilter + Clone + Default + Send + 'static,
{
    if let Some(peer_connections) = gs.connected_peers.get(peer_id) {
        let fake_endpoint = ConnectedPoint::Dialer {
            address: Multiaddr::empty(),
            role_override: Endpoint::Dialer,
            port_use: PortUse::Reuse,
        }; // this is not relevant
           // peer_connections.connections should never be empty.

        let mut active_connections = peer_connections.connections.len();
        for connection_id in peer_connections.connections.clone() {
            active_connections = active_connections.checked_sub(1).unwrap();

            gs.on_swarm_event(FromSwarm::ConnectionClosed(ConnectionClosed {
                peer_id: *peer_id,
                connection_id,
                endpoint: &fake_endpoint,
                remaining_established: active_connections,
                cause: None,
            }));
        }
    }
}

// Converts a protobuf message into a gossipsub message for reading the Gossipsub event queue.
fn proto_to_message(rpc: &proto::RPC) -> RpcIn {
    // Store valid messages.
    let mut messages = Vec::with_capacity(rpc.publish.len());
    let rpc = rpc.clone();
    for message in rpc.publish.into_iter() {
        messages.push(RawMessage {
            source: message.from.map(|x| PeerId::from_bytes(&x).unwrap()),
            data: message.data.unwrap_or_default(),
            sequence_number: message.seqno.map(|x| BigEndian::read_u64(&x)), /* don't inform the
                                                                              * application */
            topic: TopicHash::from_raw(message.topic),
            signature: message.signature, // don't inform the application
            key: None,
            validated: false,
        });
    }
    let mut control_msgs = Vec::new();
    if let Some(rpc_control) = rpc.control {
        // Collect the gossipsub control messages
        let ihave_msgs: Vec<ControlAction> = rpc_control
            .ihave
            .into_iter()
            .map(|ihave| {
                ControlAction::IHave(IHave {
                    topic_hash: TopicHash::from_raw(ihave.topic_id.unwrap_or_default()),
                    message_ids: ihave
                        .message_ids
                        .into_iter()
                        .map(MessageId::from)
                        .collect::<Vec<_>>(),
                })
            })
            .collect();

        let iwant_msgs: Vec<ControlAction> = rpc_control
            .iwant
            .into_iter()
            .map(|iwant| {
                ControlAction::IWant(IWant {
                    message_ids: iwant
                        .message_ids
                        .into_iter()
                        .map(MessageId::from)
                        .collect::<Vec<_>>(),
                })
            })
            .collect();

        let graft_msgs: Vec<ControlAction> = rpc_control
            .graft
            .into_iter()
            .map(|graft| {
                ControlAction::Graft(Graft {
                    topic_hash: TopicHash::from_raw(graft.topic_id.unwrap_or_default()),
                })
            })
            .collect();

        let mut prune_msgs = Vec::new();

        for prune in rpc_control.prune {
            // filter out invalid peers
            let peers = prune
                .peers
                .into_iter()
                .filter_map(|info| {
                    info.peer_id
                        .and_then(|id| PeerId::from_bytes(&id).ok())
                        .map(|peer_id|
                            //TODO signedPeerRecord, see https://github.com/libp2p/specs/pull/217
                            PeerInfo {
                                peer_id: Some(peer_id),
                            })
                })
                .collect::<Vec<PeerInfo>>();

            let topic_hash = TopicHash::from_raw(prune.topic_id.unwrap_or_default());
            prune_msgs.push(ControlAction::Prune(Prune {
                topic_hash,
                peers,
                backoff: prune.backoff,
            }));
        }

        control_msgs.extend(ihave_msgs);
        control_msgs.extend(iwant_msgs);
        control_msgs.extend(graft_msgs);
        control_msgs.extend(prune_msgs);
    }

    RpcIn {
        messages,
        subscriptions: rpc
            .subscriptions
            .into_iter()
            .map(|sub| Subscription {
                action: if Some(true) == sub.subscribe {
                    SubscriptionAction::Subscribe
                } else {
                    SubscriptionAction::Unsubscribe
                },
                topic_hash: TopicHash::from_raw(sub.topic_id.unwrap_or_default()),
            })
            .collect(),
        control_msgs,
    }
}

impl Behaviour {
    fn as_peer_score_mut(&mut self) -> &mut PeerScore {
        match self.peer_score {
            PeerScoreState::Active(ref mut peer_score) => peer_score,
            PeerScoreState::Disabled => panic!("PeerScore is deactivated"),
        }
    }
}
#[test]
/// Test local node subscribing to a topic
fn test_subscribe() {
    // The node should:
    // - Create an empty vector in mesh[topic]
    // - Send subscription request to all peers
    // - run JOIN(topic)

    let subscribe_topic = vec![String::from("test_subscribe")];
    let (gs, _, receivers, topic_hashes) = inject_nodes1()
        .peer_no(20)
        .topics(subscribe_topic)
        .to_subscribe(true)
        .create_network();

    assert!(
        gs.mesh.contains_key(&topic_hashes[0]),
        "Subscribe should add a new entry to the mesh[topic] hashmap"
    );

    // collect all the subscriptions
    let subscriptions = receivers
        .into_values()
        .fold(0, |mut collected_subscriptions, c| {
            let priority = c.priority.get_ref();
            while !priority.is_empty() {
                if let Ok(RpcOut::Subscribe(_)) = priority.try_recv() {
                    collected_subscriptions += 1
                }
            }
            collected_subscriptions
        });

    // we sent a subscribe to all known peers
    assert_eq!(subscriptions, 20);
}

/// Test unsubscribe.
#[test]
fn test_unsubscribe() {
    // Unsubscribe should:
    // - Remove the mesh entry for topic
    // - Send UNSUBSCRIBE to all known peers
    // - Call Leave

    let topic_strings = vec![String::from("topic1"), String::from("topic2")];
    let topics = topic_strings
        .iter()
        .map(|t| Topic::new(t.clone()))
        .collect::<Vec<Topic>>();

    // subscribe to topic_strings
    let (mut gs, _, receivers, topic_hashes) = inject_nodes1()
        .peer_no(20)
        .topics(topic_strings)
        .to_subscribe(true)
        .create_network();

    for topic_hash in &topic_hashes {
        assert!(
            gs.connected_peers
                .values()
                .any(|p| p.topics.contains(topic_hash)),
            "Topic_peers contain a topic entry"
        );
        assert!(
            gs.mesh.contains_key(topic_hash),
            "mesh should contain a topic entry"
        );
    }

    // unsubscribe from both topics
    assert!(
        gs.unsubscribe(&topics[0]),
        "should be able to unsubscribe successfully from each topic",
    );
    assert!(
        gs.unsubscribe(&topics[1]),
        "should be able to unsubscribe successfully from each topic",
    );

    // collect all the subscriptions
    let subscriptions = receivers
        .into_values()
        .fold(0, |mut collected_subscriptions, c| {
            let priority = c.priority.get_ref();
            while !priority.is_empty() {
                if let Ok(RpcOut::Subscribe(_)) = priority.try_recv() {
                    collected_subscriptions += 1
                }
            }
            collected_subscriptions
        });

    // we sent a unsubscribe to all known peers, for two topics
    assert_eq!(subscriptions, 40);

    // check we clean up internal structures
    for topic_hash in &topic_hashes {
        assert!(
            !gs.mesh.contains_key(topic_hash),
            "All topics should have been removed from the mesh"
        );
    }
}

/// Test JOIN(topic) functionality.
#[test]
fn test_join() {
    // The Join function should:
    // - Remove peers from fanout[topic]
    // - Add any fanout[topic] peers to the mesh (up to mesh_n)
    // - Fill up to mesh_n peers from known gossipsub peers in the topic
    // - Send GRAFT messages to all nodes added to the mesh

    // This test is not an isolated unit test, rather it uses higher level,
    // subscribe/unsubscribe to perform the test.

    let topic_strings = vec![String::from("topic1"), String::from("topic2")];
    let topics = topic_strings
        .iter()
        .map(|t| Topic::new(t.clone()))
        .collect::<Vec<Topic>>();

    let (mut gs, _, mut receivers, topic_hashes) = inject_nodes1()
        .peer_no(20)
        .topics(topic_strings)
        .to_subscribe(true)
        .create_network();

    // Flush previous GRAFT messages.
    receivers = flush_events(&mut gs, receivers);

    // unsubscribe, then call join to invoke functionality
    assert!(
        gs.unsubscribe(&topics[0]),
        "should be able to unsubscribe successfully"
    );
    assert!(
        gs.unsubscribe(&topics[1]),
        "should be able to unsubscribe successfully"
    );

    // re-subscribe - there should be peers associated with the topic
    assert!(
        gs.subscribe(&topics[0]).unwrap(),
        "should be able to subscribe successfully"
    );

    // should have added mesh_n nodes to the mesh
    assert!(
        gs.mesh.get(&topic_hashes[0]).unwrap().len() == 6,
        "Should have added 6 nodes to the mesh"
    );

    fn count_grafts(receivers: HashMap<PeerId, Receiver>) -> (usize, HashMap<PeerId, Receiver>) {
        let mut new_receivers = HashMap::new();
        let mut acc = 0;

        for (peer_id, c) in receivers.into_iter() {
            let priority = c.priority.get_ref();
            while !priority.is_empty() {
                if let Ok(RpcOut::Graft(_)) = priority.try_recv() {
                    acc += 1;
                }
            }
            new_receivers.insert(
                peer_id,
                Receiver {
                    priority_queue_len: c.priority_queue_len,
                    priority: c.priority,
                    non_priority: c.non_priority,
                },
            );
        }
        (acc, new_receivers)
    }

    // there should be mesh_n GRAFT messages.
    let (graft_messages, mut receivers) = count_grafts(receivers);

    assert_eq!(
        graft_messages, 6,
        "There should be 6 grafts messages sent to peers"
    );

    // verify fanout nodes
    // add 3 random peers to the fanout[topic1]
    gs.fanout
        .insert(topic_hashes[1].clone(), Default::default());
    let mut new_peers: Vec<PeerId> = vec![];

    for _ in 0..3 {
        let random_peer = PeerId::random();
        // inform the behaviour of a new peer
        let address = "/ip4/127.0.0.1".parse::<Multiaddr>().unwrap();
        gs.handle_established_inbound_connection(
            ConnectionId::new_unchecked(0),
            random_peer,
            &address,
            &address,
        )
        .unwrap();
        let sender = Sender::new(gs.config.connection_handler_queue_len());
        let receiver = sender.new_receiver();
        let connection_id = ConnectionId::new_unchecked(0);
        gs.connected_peers.insert(
            random_peer,
            PeerDetails {
                kind: PeerKind::Floodsub,
                outbound: false,
                connections: vec![connection_id],
                topics: Default::default(),
                sender,
                dont_send: LinkedHashMap::new(),
            },
        );
        receivers.insert(random_peer, receiver);

        gs.on_swarm_event(FromSwarm::ConnectionEstablished(ConnectionEstablished {
            peer_id: random_peer,
            connection_id,
            endpoint: &ConnectedPoint::Dialer {
                address,
                role_override: Endpoint::Dialer,
                port_use: PortUse::Reuse,
            },
            failed_addresses: &[],
            other_established: 0,
        }));

        // add the new peer to the fanout
        let fanout_peers = gs.fanout.get_mut(&topic_hashes[1]).unwrap();
        fanout_peers.insert(random_peer);
        new_peers.push(random_peer);
    }

    // subscribe to topic1
    gs.subscribe(&topics[1]).unwrap();

    // the three new peers should have been added, along with 3 more from the pool.
    assert!(
        gs.mesh.get(&topic_hashes[1]).unwrap().len() == 6,
        "Should have added 6 nodes to the mesh"
    );
    let mesh_peers = gs.mesh.get(&topic_hashes[1]).unwrap();
    for new_peer in new_peers {
        assert!(
            mesh_peers.contains(&new_peer),
            "Fanout peer should be included in the mesh"
        );
    }

    // there should now 6 graft messages to be sent
    let (graft_messages, _) = count_grafts(receivers);

    assert_eq!(
        graft_messages, 6,
        "There should be 6 grafts messages sent to peers"
    );
}

/// Test local node publish to subscribed topic
#[test]
fn test_publish_without_flood_publishing() {
    // node should:
    // - Send publish message to all peers
    // - Insert message into gs.mcache and gs.received

    // turn off flood publish to test old behaviour
    let config = ConfigBuilder::default()
        .flood_publish(false)
        .build()
        .unwrap();

    let publish_topic = String::from("test_publish");
    let (mut gs, _, receivers, topic_hashes) = inject_nodes1()
        .peer_no(20)
        .topics(vec![publish_topic.clone()])
        .to_subscribe(true)
        .gs_config(config)
        .create_network();

    assert!(
        gs.mesh.contains_key(&topic_hashes[0]),
        "Subscribe should add a new entry to the mesh[topic] hashmap"
    );

    // all peers should be subscribed to the topic
    assert_eq!(
        gs.connected_peers
            .values()
            .filter(|p| p.topics.contains(&topic_hashes[0]))
            .count(),
        20,
        "Peers should be subscribed to the topic"
    );

    // publish on topic
    let publish_data = vec![0; 42];
    gs.publish(Topic::new(publish_topic), publish_data).unwrap();

    // Collect all publish messages
    let publishes = receivers
        .into_values()
        .fold(vec![], |mut collected_publish, c| {
            let priority = c.priority.get_ref();
            while !priority.is_empty() {
                if let Ok(RpcOut::Publish { message, .. }) = priority.try_recv() {
                    collected_publish.push(message);
                }
            }
            collected_publish
        });

    // Transform the inbound message
    let message = &gs
        .data_transform
        .inbound_transform(
            publishes
                .first()
                .expect("Should contain > 0 entries")
                .clone(),
        )
        .unwrap();

    let msg_id = gs.config.message_id(message);

    let config: Config = Config::default();
    assert_eq!(
        publishes.len(),
        config.mesh_n(),
        "Should send a publish message to at least mesh_n peers"
    );

    assert!(
        gs.mcache.get(&msg_id).is_some(),
        "Message cache should contain published message"
    );
}

/// Test local node publish to unsubscribed topic
#[test]
fn test_fanout() {
    // node should:
    // - Populate fanout peers
    // - Send publish message to fanout peers
    // - Insert message into gs.mcache and gs.received

    // turn off flood publish to test fanout behaviour
    let config = ConfigBuilder::default()
        .flood_publish(false)
        .build()
        .unwrap();

    let fanout_topic = String::from("test_fanout");
    let (mut gs, _, receivers, topic_hashes) = inject_nodes1()
        .peer_no(20)
        .topics(vec![fanout_topic.clone()])
        .to_subscribe(true)
        .gs_config(config)
        .create_network();

    assert!(
        gs.mesh.contains_key(&topic_hashes[0]),
        "Subscribe should add a new entry to the mesh[topic] hashmap"
    );
    // Unsubscribe from topic
    assert!(
        gs.unsubscribe(&Topic::new(fanout_topic.clone())),
        "should be able to unsubscribe successfully from topic"
    );

    // Publish on unsubscribed topic
    let publish_data = vec![0; 42];
    gs.publish(Topic::new(fanout_topic.clone()), publish_data)
        .unwrap();

    assert_eq!(
        gs.fanout
            .get(&TopicHash::from_raw(fanout_topic))
            .unwrap()
            .len(),
        gs.config.mesh_n(),
        "Fanout should contain `mesh_n` peers for fanout topic"
    );

    // Collect all publish messages
    let publishes = receivers
        .into_values()
        .fold(vec![], |mut collected_publish, c| {
            let priority = c.priority.get_ref();
            while !priority.is_empty() {
                if let Ok(RpcOut::Publish { message, .. }) = priority.try_recv() {
                    collected_publish.push(message);
                }
            }
            collected_publish
        });

    // Transform the inbound message
    let message = &gs
        .data_transform
        .inbound_transform(
            publishes
                .first()
                .expect("Should contain > 0 entries")
                .clone(),
        )
        .unwrap();

    let msg_id = gs.config.message_id(message);

    assert_eq!(
        publishes.len(),
        gs.config.mesh_n(),
        "Should send a publish message to `mesh_n` fanout peers"
    );

    assert!(
        gs.mcache.get(&msg_id).is_some(),
        "Message cache should contain published message"
    );
}

/// Test the gossipsub NetworkBehaviour peer connection logic.
#[test]
fn test_inject_connected() {
    let (gs, peers, receivers, topic_hashes) = inject_nodes1()
        .peer_no(20)
        .topics(vec![String::from("topic1"), String::from("topic2")])
        .to_subscribe(true)
        .create_network();

    // check that our subscriptions are sent to each of the peers
    // collect all the SendEvents
    let subscriptions = receivers.into_iter().fold(
        HashMap::<PeerId, Vec<String>>::new(),
        |mut collected_subscriptions, (peer, c)| {
            let priority = c.priority.get_ref();
            while !priority.is_empty() {
                if let Ok(RpcOut::Subscribe(topic)) = priority.try_recv() {
                    let mut peer_subs = collected_subscriptions.remove(&peer).unwrap_or_default();
                    peer_subs.push(topic.into_string());
                    collected_subscriptions.insert(peer, peer_subs);
                }
            }
            collected_subscriptions
        },
    );

    // check that there are two subscriptions sent to each peer
    for peer_subs in subscriptions.values() {
        assert!(peer_subs.contains(&String::from("topic1")));
        assert!(peer_subs.contains(&String::from("topic2")));
        assert_eq!(peer_subs.len(), 2);
    }

    // check that there are 20 send events created
    assert_eq!(subscriptions.len(), 20);

    // should add the new peers to `peer_topics` with an empty vec as a gossipsub node
    for peer in peers {
        let peer = gs.connected_peers.get(&peer).unwrap();
        assert!(
            peer.topics == topic_hashes.iter().cloned().collect(),
            "The topics for each node should all topics"
        );
    }
}

/// Test subscription handling
#[test]
fn test_handle_received_subscriptions() {
    // For every subscription:
    // SUBSCRIBE:   - Add subscribed topic to peer_topics for peer.
    //              - Add peer to topics_peer.
    // UNSUBSCRIBE  - Remove topic from peer_topics for peer.
    //              - Remove peer from topic_peers.

    let topics = ["topic1", "topic2", "topic3", "topic4"]
        .iter()
        .map(|&t| String::from(t))
        .collect();
    let (mut gs, peers, _receivers, topic_hashes) = inject_nodes1()
        .peer_no(20)
        .topics(topics)
        .to_subscribe(false)
        .create_network();

    // The first peer sends 3 subscriptions and 1 unsubscription
    let mut subscriptions = topic_hashes[..3]
        .iter()
        .map(|topic_hash| Subscription {
            action: SubscriptionAction::Subscribe,
            topic_hash: topic_hash.clone(),
        })
        .collect::<Vec<Subscription>>();

    subscriptions.push(Subscription {
        action: SubscriptionAction::Unsubscribe,
        topic_hash: topic_hashes[topic_hashes.len() - 1].clone(),
    });

    let unknown_peer = PeerId::random();
    // process the subscriptions
    // first and second peers send subscriptions
    gs.handle_received_subscriptions(&subscriptions, &peers[0]);
    gs.handle_received_subscriptions(&subscriptions, &peers[1]);
    // unknown peer sends the same subscriptions
    gs.handle_received_subscriptions(&subscriptions, &unknown_peer);

    // verify the result

    let peer = gs.connected_peers.get(&peers[0]).unwrap();
    assert!(
        peer.topics
            == topic_hashes
                .iter()
                .take(3)
                .cloned()
                .collect::<BTreeSet<_>>(),
        "First peer should be subscribed to three topics"
    );
    let peer1 = gs.connected_peers.get(&peers[1]).unwrap();
    assert!(
        peer1.topics
            == topic_hashes
                .iter()
                .take(3)
                .cloned()
                .collect::<BTreeSet<_>>(),
        "Second peer should be subscribed to three topics"
    );

    assert!(
        !gs.connected_peers.contains_key(&unknown_peer),
        "Unknown peer should not have been added"
    );

    for topic_hash in topic_hashes[..3].iter() {
        let topic_peers = gs
            .connected_peers
            .iter()
            .filter(|(_, p)| p.topics.contains(topic_hash))
            .map(|(peer_id, _)| *peer_id)
            .collect::<BTreeSet<PeerId>>();
        assert!(
            topic_peers == peers[..2].iter().cloned().collect(),
            "Two peers should be added to the first three topics"
        );
    }

    // Peer 0 unsubscribes from the first topic

    gs.handle_received_subscriptions(
        &[Subscription {
            action: SubscriptionAction::Unsubscribe,
            topic_hash: topic_hashes[0].clone(),
        }],
        &peers[0],
    );

    let peer = gs.connected_peers.get(&peers[0]).unwrap();
    assert!(
        peer.topics == topic_hashes[1..3].iter().cloned().collect::<BTreeSet<_>>(),
        "Peer should be subscribed to two topics"
    );

    // only gossipsub at the moment
    let topic_peers = gs
        .connected_peers
        .iter()
        .filter(|(_, p)| p.topics.contains(&topic_hashes[0]))
        .map(|(peer_id, _)| *peer_id)
        .collect::<BTreeSet<PeerId>>();

    assert!(
        topic_peers == peers[1..2].iter().cloned().collect(),
        "Only the second peers should be in the first topic"
    );
}

/// Test Gossipsub.get_random_peers() function
#[test]
fn test_get_random_peers() {
    // generate a default Config
    let gs_config = ConfigBuilder::default()
        .validation_mode(ValidationMode::Anonymous)
        .build()
        .unwrap();
    // create a gossipsub struct
    let mut gs: Behaviour = Behaviour::new(MessageAuthenticity::Anonymous, gs_config).unwrap();

    // create a topic and fill it with some peers
    let topic_hash = Topic::new("Test").hash();
    let mut peers = vec![];
    let mut topics = BTreeSet::new();
    topics.insert(topic_hash.clone());

    for _ in 0..20 {
        let peer_id = PeerId::random();
        peers.push(peer_id);
        gs.connected_peers.insert(
            peer_id,
            PeerDetails {
                kind: PeerKind::Gossipsubv1_1,
                connections: vec![ConnectionId::new_unchecked(0)],
                outbound: false,
                topics: topics.clone(),
                sender: Sender::new(gs.config.connection_handler_queue_len()),
                dont_send: LinkedHashMap::new(),
            },
        );
    }

    let random_peers = get_random_peers(&gs.connected_peers, &topic_hash, 5, |_| true);
    assert_eq!(random_peers.len(), 5, "Expected 5 peers to be returned");
    let random_peers = get_random_peers(&gs.connected_peers, &topic_hash, 30, |_| true);
    assert!(random_peers.len() == 20, "Expected 20 peers to be returned");
    assert!(
        random_peers == peers.iter().cloned().collect(),
        "Expected no shuffling"
    );
    let random_peers = get_random_peers(&gs.connected_peers, &topic_hash, 20, |_| true);
    assert!(random_peers.len() == 20, "Expected 20 peers to be returned");
    assert!(
        random_peers == peers.iter().cloned().collect(),
        "Expected no shuffling"
    );
    let random_peers = get_random_peers(&gs.connected_peers, &topic_hash, 0, |_| true);
    assert!(random_peers.is_empty(), "Expected 0 peers to be returned");
    // test the filter
    let random_peers = get_random_peers(&gs.connected_peers, &topic_hash, 5, |_| false);
    assert!(random_peers.is_empty(), "Expected 0 peers to be returned");
    let random_peers = get_random_peers(&gs.connected_peers, &topic_hash, 10, {
        |peer| peers.contains(peer)
    });
    assert!(random_peers.len() == 10, "Expected 10 peers to be returned");
}

/// Tests that the correct message is sent when a peer asks for a message in our cache.
#[test]
fn test_handle_iwant_msg_cached() {
    let (mut gs, peers, receivers, _) = inject_nodes1()
        .peer_no(20)
        .topics(Vec::new())
        .to_subscribe(true)
        .create_network();

    let raw_message = RawMessage {
        source: Some(peers[11]),
        data: vec![1, 2, 3, 4],
        sequence_number: Some(1u64),
        topic: TopicHash::from_raw("topic"),
        signature: None,
        key: None,
        validated: true,
    };

    // Transform the inbound message
    let message = &gs
        .data_transform
        .inbound_transform(raw_message.clone())
        .unwrap();

    let msg_id = gs.config.message_id(message);
    gs.mcache.put(&msg_id, raw_message);

    gs.handle_iwant(&peers[7], vec![msg_id.clone()]);

    // the messages we are sending
    let sent_messages = receivers
        .into_values()
        .fold(vec![], |mut collected_messages, c| {
            let non_priority = c.non_priority.get_ref();
            while !non_priority.is_empty() {
                if let Ok(RpcOut::Forward { message, .. }) = non_priority.try_recv() {
                    collected_messages.push(message)
                }
            }
            collected_messages
        });

    assert!(
        sent_messages
            .iter()
            .map(|msg| gs.data_transform.inbound_transform(msg.clone()).unwrap())
            .any(|msg| gs.config.message_id(&msg) == msg_id),
        "Expected the cached message to be sent to an IWANT peer"
    );
}

/// Tests that messages are sent correctly depending on the shifting of the message cache.
#[test]
fn test_handle_iwant_msg_cached_shifted() {
    let (mut gs, peers, mut receivers, _) = inject_nodes1()
        .peer_no(20)
        .topics(Vec::new())
        .to_subscribe(true)
        .create_network();

    // perform 10 memshifts and check that it leaves the cache
    for shift in 1..10 {
        let raw_message = RawMessage {
            source: Some(peers[11]),
            data: vec![1, 2, 3, 4],
            sequence_number: Some(shift),
            topic: TopicHash::from_raw("topic"),
            signature: None,
            key: None,
            validated: true,
        };

        // Transform the inbound message
        let message = &gs
            .data_transform
            .inbound_transform(raw_message.clone())
            .unwrap();

        let msg_id = gs.config.message_id(message);
        gs.mcache.put(&msg_id, raw_message);
        for _ in 0..shift {
            gs.mcache.shift();
        }

        gs.handle_iwant(&peers[7], vec![msg_id.clone()]);

        // is the message is being sent?
        let mut message_exists = false;
        receivers = receivers.into_iter().map(|(peer_id, c)| {
            let non_priority = c.non_priority.get_ref();
            while !non_priority.is_empty() {
                if matches!(non_priority.try_recv(), Ok(RpcOut::Forward{message, timeout: _ }) if
                        gs.config.message_id(
                            &gs.data_transform
                                .inbound_transform(message.clone())
                                .unwrap(),
                        ) == msg_id)
                {
                    message_exists = true;
                }
            }
            (
                peer_id,
                Receiver {
                    priority_queue_len: c.priority_queue_len,
                    priority: c.priority,
                    non_priority: c.non_priority,
                },
            )
        }).collect();
        // default history_length is 5, expect no messages after shift > 5
        if shift < 5 {
            assert!(
                message_exists,
                "Expected the cached message to be sent to an IWANT peer before 5 shifts"
            );
        } else {
            assert!(
                !message_exists,
                "Expected the cached message to not be sent to an IWANT peer after 5 shifts"
            );
        }
    }
}

/// tests that an event is not created when a peers asks for a message not in our cache
#[test]
fn test_handle_iwant_msg_not_cached() {
    let (mut gs, peers, _, _) = inject_nodes1()
        .peer_no(20)
        .topics(Vec::new())
        .to_subscribe(true)
        .create_network();

    let events_before = gs.events.len();
    gs.handle_iwant(&peers[7], vec![MessageId::new(b"unknown id")]);
    let events_after = gs.events.len();

    assert_eq!(
        events_before, events_after,
        "Expected event count to stay the same"
    );
}

#[test]
fn test_handle_iwant_msg_but_already_sent_idontwant() {
    let (mut gs, peers, receivers, _) = inject_nodes1()
        .peer_no(20)
        .topics(Vec::new())
        .to_subscribe(true)
        .create_network();

    let raw_message = RawMessage {
        source: Some(peers[11]),
        data: vec![1, 2, 3, 4],
        sequence_number: Some(1u64),
        topic: TopicHash::from_raw("topic"),
        signature: None,
        key: None,
        validated: true,
    };

    // Transform the inbound message
    let message = &gs
        .data_transform
        .inbound_transform(raw_message.clone())
        .unwrap();

    let msg_id = gs.config.message_id(message);
    gs.mcache.put(&msg_id, raw_message);

    // Receive IDONTWANT from Peer 1.
    let rpc = RpcIn {
        messages: vec![],
        subscriptions: vec![],
        control_msgs: vec![ControlAction::IDontWant(IDontWant {
            message_ids: vec![msg_id.clone()],
        })],
    };
    gs.on_connection_handler_event(
        peers[1],
        ConnectionId::new_unchecked(0),
        HandlerEvent::Message {
            rpc,
            invalid_messages: vec![],
        },
    );

    // Receive IWANT from Peer 1.
    gs.handle_iwant(&peers[1], vec![msg_id.clone()]);

    // Check that no messages are sent.
    receivers.iter().for_each(|(_, receiver)| {
        assert!(receiver.non_priority.get_ref().is_empty());
    });
}

/// tests that an event is created when a peer shares that it has a message we want
#[test]
fn test_handle_ihave_subscribed_and_msg_not_cached() {
    let (mut gs, peers, mut receivers, topic_hashes) = inject_nodes1()
        .peer_no(20)
        .topics(vec![String::from("topic1")])
        .to_subscribe(true)
        .create_network();

    gs.handle_ihave(
        &peers[7],
        vec![(topic_hashes[0].clone(), vec![MessageId::new(b"unknown id")])],
    );

    // check that we sent an IWANT request for `unknown id`
    let mut iwant_exists = false;
    let receiver = receivers.remove(&peers[7]).unwrap();
    let non_priority = receiver.non_priority.get_ref();
    while !non_priority.is_empty() {
        if let Ok(RpcOut::IWant(IWant { message_ids })) = non_priority.try_recv() {
            if message_ids
                .iter()
                .any(|m| *m == MessageId::new(b"unknown id"))
            {
                iwant_exists = true;
                break;
            }
        }
    }

    assert!(
        iwant_exists,
        "Expected to send an IWANT control message for unknown message id"
    );
}

/// tests that an event is not created when a peer shares that it has a message that
/// we already have
#[test]
fn test_handle_ihave_subscribed_and_msg_cached() {
    let (mut gs, peers, _, topic_hashes) = inject_nodes1()
        .peer_no(20)
        .topics(vec![String::from("topic1")])
        .to_subscribe(true)
        .create_network();

    let msg_id = MessageId::new(b"known id");

    let events_before = gs.events.len();
    gs.handle_ihave(&peers[7], vec![(topic_hashes[0].clone(), vec![msg_id])]);
    let events_after = gs.events.len();

    assert_eq!(
        events_before, events_after,
        "Expected event count to stay the same"
    )
}

/// test that an event is not created when a peer shares that it has a message in
/// a topic that we are not subscribed to
#[test]
fn test_handle_ihave_not_subscribed() {
    let (mut gs, peers, _, _) = inject_nodes1()
        .peer_no(20)
        .topics(vec![])
        .to_subscribe(true)
        .create_network();

    let events_before = gs.events.len();
    gs.handle_ihave(
        &peers[7],
        vec![(
            TopicHash::from_raw(String::from("unsubscribed topic")),
            vec![MessageId::new(b"irrelevant id")],
        )],
    );
    let events_after = gs.events.len();

    assert_eq!(
        events_before, events_after,
        "Expected event count to stay the same"
    )
}

/// tests that a peer is added to our mesh when we are both subscribed
/// to the same topic
#[test]
fn test_handle_graft_is_subscribed() {
    let (mut gs, peers, _, topic_hashes) = inject_nodes1()
        .peer_no(20)
        .topics(vec![String::from("topic1")])
        .to_subscribe(true)
        .create_network();

    gs.handle_graft(&peers[7], topic_hashes.clone());

    assert!(
        gs.mesh.get(&topic_hashes[0]).unwrap().contains(&peers[7]),
        "Expected peer to have been added to mesh"
    );
}

/// tests that a peer is not added to our mesh when they are subscribed to
/// a topic that we are not
#[test]
fn test_handle_graft_is_not_subscribed() {
    let (mut gs, peers, _, topic_hashes) = inject_nodes1()
        .peer_no(20)
        .topics(vec![String::from("topic1")])
        .to_subscribe(true)
        .create_network();

    gs.handle_graft(
        &peers[7],
        vec![TopicHash::from_raw(String::from("unsubscribed topic"))],
    );

    assert!(
        !gs.mesh.get(&topic_hashes[0]).unwrap().contains(&peers[7]),
        "Expected peer to have been added to mesh"
    );
}

/// tests multiple topics in a single graft message
#[test]
fn test_handle_graft_multiple_topics() {
    let topics: Vec<String> = ["topic1", "topic2", "topic3", "topic4"]
        .iter()
        .map(|&t| String::from(t))
        .collect();

    let (mut gs, peers, _, topic_hashes) = inject_nodes1()
        .peer_no(20)
        .topics(topics)
        .to_subscribe(true)
        .create_network();

    let mut their_topics = topic_hashes.clone();
    // their_topics = [topic1, topic2, topic3]
    // our_topics = [topic1, topic2, topic4]
    their_topics.pop();
    gs.leave(&their_topics[2]);

    gs.handle_graft(&peers[7], their_topics.clone());

    for hash in topic_hashes.iter().take(2) {
        assert!(
            gs.mesh.get(hash).unwrap().contains(&peers[7]),
            "Expected peer to be in the mesh for the first 2 topics"
        );
    }

    assert!(
        !gs.mesh.contains_key(&topic_hashes[2]),
        "Expected the second topic to not be in the mesh"
    );
}

/// tests that a peer is removed from our mesh
#[test]
fn test_handle_prune_peer_in_mesh() {
    let (mut gs, peers, _, topic_hashes) = inject_nodes1()
        .peer_no(20)
        .topics(vec![String::from("topic1")])
        .to_subscribe(true)
        .create_network();

    // insert peer into our mesh for 'topic1'
    gs.mesh
        .insert(topic_hashes[0].clone(), peers.iter().cloned().collect());
    assert!(
        gs.mesh.get(&topic_hashes[0]).unwrap().contains(&peers[7]),
        "Expected peer to be in mesh"
    );

    gs.handle_prune(
        &peers[7],
        topic_hashes
            .iter()
            .map(|h| (h.clone(), vec![], None))
            .collect(),
    );
    assert!(
        !gs.mesh.get(&topic_hashes[0]).unwrap().contains(&peers[7]),
        "Expected peer to be removed from mesh"
    );
}

fn count_control_msgs(
    receivers: HashMap<PeerId, Receiver>,
    mut filter: impl FnMut(&PeerId, &RpcOut) -> bool,
) -> (usize, HashMap<PeerId, Receiver>) {
    let mut new_receivers = HashMap::new();
    let mut collected_messages = 0;
    for (peer_id, c) in receivers.into_iter() {
        let priority = c.priority.get_ref();
        let non_priority = c.non_priority.get_ref();
        while !priority.is_empty() || !non_priority.is_empty() {
            if let Ok(rpc) = priority.try_recv() {
                if filter(&peer_id, &rpc) {
                    collected_messages += 1;
                }
            }
            if let Ok(rpc) = non_priority.try_recv() {
                if filter(&peer_id, &rpc) {
                    collected_messages += 1;
                }
            }
        }
        new_receivers.insert(
            peer_id,
            Receiver {
                priority_queue_len: c.priority_queue_len,
                priority: c.priority,
                non_priority: c.non_priority,
            },
        );
    }
    (collected_messages, new_receivers)
}

fn flush_events<D: DataTransform, F: TopicSubscriptionFilter>(
    gs: &mut Behaviour<D, F>,
    receivers: HashMap<PeerId, Receiver>,
) -> HashMap<PeerId, Receiver> {
    gs.events.clear();
    let mut new_receivers = HashMap::new();
    for (peer_id, c) in receivers.into_iter() {
        let priority = c.priority.get_ref();
        let non_priority = c.non_priority.get_ref();
        while !priority.is_empty() || !non_priority.is_empty() {
            let _ = priority.try_recv();
            let _ = non_priority.try_recv();
        }
        new_receivers.insert(
            peer_id,
            Receiver {
                priority_queue_len: c.priority_queue_len,
                priority: c.priority,
                non_priority: c.non_priority,
            },
        );
    }
    new_receivers
}

/// tests that a peer added as explicit peer gets connected to
#[test]
fn test_explicit_peer_gets_connected() {
    let (mut gs, _, _, _) = inject_nodes1()
        .peer_no(0)
        .topics(Vec::new())
        .to_subscribe(true)
        .create_network();

    // create new peer
    let peer = PeerId::random();

    // add peer as explicit peer
    gs.add_explicit_peer(&peer);

    let num_events = gs
        .events
        .iter()
        .filter(|e| match e {
            ToSwarm::Dial { opts } => opts.get_peer_id() == Some(peer),
            _ => false,
        })
        .count();

    assert_eq!(
        num_events, 1,
        "There was no dial peer event for the explicit peer"
    );
}

#[test]
fn test_explicit_peer_reconnects() {
    let config = ConfigBuilder::default()
        .check_explicit_peers_ticks(2)
        .build()
        .unwrap();
    let (mut gs, others, receivers, _) = inject_nodes1()
        .peer_no(1)
        .topics(Vec::new())
        .to_subscribe(true)
        .gs_config(config)
        .create_network();

    let peer = others.first().unwrap();

    // add peer as explicit peer
    gs.add_explicit_peer(peer);

    flush_events(&mut gs, receivers);

    // disconnect peer
    disconnect_peer(&mut gs, peer);

    gs.heartbeat();

    // check that no reconnect after first heartbeat since `explicit_peer_ticks == 2`
    assert_eq!(
        gs.events
            .iter()
            .filter(|e| match e {
                ToSwarm::Dial { opts } => opts.get_peer_id() == Some(*peer),
                _ => false,
            })
            .count(),
        0,
        "There was a dial peer event before explicit_peer_ticks heartbeats"
    );

    gs.heartbeat();

    // check that there is a reconnect after second heartbeat
    assert!(
        gs.events
            .iter()
            .filter(|e| match e {
                ToSwarm::Dial { opts } => opts.get_peer_id() == Some(*peer),
                _ => false,
            })
            .count()
            >= 1,
        "There was no dial peer event for the explicit peer"
    );
}

#[test]
fn test_handle_graft_explicit_peer() {
    let (mut gs, peers, receivers, topic_hashes) = inject_nodes1()
        .peer_no(1)
        .topics(vec![String::from("topic1"), String::from("topic2")])
        .to_subscribe(true)
        .gs_config(Config::default())
        .explicit(1)
        .create_network();

    let peer = peers.first().unwrap();

    gs.handle_graft(peer, topic_hashes.clone());

    // peer got not added to mesh
    assert!(gs.mesh[&topic_hashes[0]].is_empty());
    assert!(gs.mesh[&topic_hashes[1]].is_empty());

    // check prunes
    let (control_msgs, _) = count_control_msgs(receivers, |peer_id, m| {
        peer_id == peer
            && match m {
                RpcOut::Prune(Prune { topic_hash, .. }) => {
                    topic_hash == &topic_hashes[0] || topic_hash == &topic_hashes[1]
                }
                _ => false,
            }
    });
    assert!(
        control_msgs >= 2,
        "Not enough prunes sent when grafting from explicit peer"
    );
}

#[test]
fn explicit_peers_not_added_to_mesh_on_receiving_subscription() {
    let (gs, peers, receivers, topic_hashes) = inject_nodes1()
        .peer_no(2)
        .topics(vec![String::from("topic1")])
        .to_subscribe(true)
        .gs_config(Config::default())
        .explicit(1)
        .create_network();

    // only peer 1 is in the mesh not peer 0 (which is an explicit peer)
    assert_eq!(
        gs.mesh[&topic_hashes[0]],
        vec![peers[1]].into_iter().collect()
    );

    // assert that graft gets created to non-explicit peer
    let (control_msgs, receivers) = count_control_msgs(receivers, |peer_id, m| {
        peer_id == &peers[1] && matches!(m, RpcOut::Graft { .. })
    });
    assert!(
        control_msgs >= 1,
        "No graft message got created to non-explicit peer"
    );

    // assert that no graft gets created to explicit peer
    let (control_msgs, _) = count_control_msgs(receivers, |peer_id, m| {
        peer_id == &peers[0] && matches!(m, RpcOut::Graft { .. })
    });
    assert_eq!(
        control_msgs, 0,
        "A graft message got created to an explicit peer"
    );
}

#[test]
fn do_not_graft_explicit_peer() {
    let (mut gs, others, receivers, topic_hashes) = inject_nodes1()
        .peer_no(1)
        .topics(vec![String::from("topic")])
        .to_subscribe(true)
        .gs_config(Config::default())
        .explicit(1)
        .create_network();

    gs.heartbeat();

    // mesh stays empty
    assert_eq!(gs.mesh[&topic_hashes[0]], BTreeSet::new());

    // assert that no graft gets created to explicit peer
    let (control_msgs, _) = count_control_msgs(receivers, |peer_id, m| {
        peer_id == &others[0] && matches!(m, RpcOut::Graft { .. })
    });
    assert_eq!(
        control_msgs, 0,
        "A graft message got created to an explicit peer"
    );
}

#[test]
fn do_forward_messages_to_explicit_peers() {
    let (mut gs, peers, receivers, topic_hashes) = inject_nodes1()
        .peer_no(2)
        .topics(vec![String::from("topic1"), String::from("topic2")])
        .to_subscribe(true)
        .gs_config(Config::default())
        .explicit(1)
        .create_network();

    let local_id = PeerId::random();

    let message = RawMessage {
        source: Some(peers[1]),
        data: vec![12],
        sequence_number: Some(0),
        topic: topic_hashes[0].clone(),
        signature: None,
        key: None,
        validated: true,
    };
    gs.handle_received_message(message.clone(), &local_id);
    assert_eq!(
        receivers.into_iter().fold(0, |mut fwds, (peer_id, c)| {
            let non_priority = c.non_priority.get_ref();
            while !non_priority.is_empty() {
                if matches!(non_priority.try_recv(), Ok(RpcOut::Forward{message: m, timeout: _}) if peer_id == peers[0] && m.data == message.data) {
        fwds +=1;
        }
                }
            fwds
        }),
        1,
        "The message did not get forwarded to the explicit peer"
    );
}

#[test]
fn explicit_peers_not_added_to_mesh_on_subscribe() {
    let (mut gs, peers, receivers, _) = inject_nodes1()
        .peer_no(2)
        .topics(Vec::new())
        .to_subscribe(true)
        .gs_config(Config::default())
        .explicit(1)
        .create_network();

    // create new topic, both peers subscribing to it but we do not subscribe to it
    let topic = Topic::new(String::from("t"));
    let topic_hash = topic.hash();
    for peer in peers.iter().take(2) {
        gs.handle_received_subscriptions(
            &[Subscription {
                action: SubscriptionAction::Subscribe,
                topic_hash: topic_hash.clone(),
            }],
            peer,
        );
    }

    // subscribe now to topic
    gs.subscribe(&topic).unwrap();

    // only peer 1 is in the mesh not peer 0 (which is an explicit peer)
    assert_eq!(gs.mesh[&topic_hash], vec![peers[1]].into_iter().collect());

    // assert that graft gets created to non-explicit peer
    let (control_msgs, receivers) = count_control_msgs(receivers, |peer_id, m| {
        peer_id == &peers[1] && matches!(m, RpcOut::Graft { .. })
    });
    assert!(
        control_msgs > 0,
        "No graft message got created to non-explicit peer"
    );

    // assert that no graft gets created to explicit peer
    let (control_msgs, _) = count_control_msgs(receivers, |peer_id, m| {
        peer_id == &peers[0] && matches!(m, RpcOut::Graft { .. })
    });
    assert_eq!(
        control_msgs, 0,
        "A graft message got created to an explicit peer"
    );
}

#[test]
fn explicit_peers_not_added_to_mesh_from_fanout_on_subscribe() {
    let (mut gs, peers, receivers, _) = inject_nodes1()
        .peer_no(2)
        .topics(Vec::new())
        .to_subscribe(true)
        .gs_config(Config::default())
        .explicit(1)
        .create_network();

    // create new topic, both peers subscribing to it but we do not subscribe to it
    let topic = Topic::new(String::from("t"));
    let topic_hash = topic.hash();
    for peer in peers.iter().take(2) {
        gs.handle_received_subscriptions(
            &[Subscription {
                action: SubscriptionAction::Subscribe,
                topic_hash: topic_hash.clone(),
            }],
            peer,
        );
    }

    // we send a message for this topic => this will initialize the fanout
    gs.publish(topic.clone(), vec![1, 2, 3]).unwrap();

    // subscribe now to topic
    gs.subscribe(&topic).unwrap();

    // only peer 1 is in the mesh not peer 0 (which is an explicit peer)
    assert_eq!(gs.mesh[&topic_hash], vec![peers[1]].into_iter().collect());

    // assert that graft gets created to non-explicit peer
    let (control_msgs, receivers) = count_control_msgs(receivers, |peer_id, m| {
        peer_id == &peers[1] && matches!(m, RpcOut::Graft { .. })
    });
    assert!(
        control_msgs >= 1,
        "No graft message got created to non-explicit peer"
    );

    // assert that no graft gets created to explicit peer
    let (control_msgs, _) = count_control_msgs(receivers, |peer_id, m| {
        peer_id == &peers[0] && matches!(m, RpcOut::Graft { .. })
    });
    assert_eq!(
        control_msgs, 0,
        "A graft message got created to an explicit peer"
    );
}

#[test]
fn no_gossip_gets_sent_to_explicit_peers() {
    let (mut gs, peers, mut receivers, topic_hashes) = inject_nodes1()
        .peer_no(2)
        .topics(vec![String::from("topic1"), String::from("topic2")])
        .to_subscribe(true)
        .gs_config(Config::default())
        .explicit(1)
        .create_network();

    let local_id = PeerId::random();

    let message = RawMessage {
        source: Some(peers[1]),
        data: vec![],
        sequence_number: Some(0),
        topic: topic_hashes[0].clone(),
        signature: None,
        key: None,
        validated: true,
    };

    // forward the message
    gs.handle_received_message(message, &local_id);

    // simulate multiple gossip calls (for randomness)
    for _ in 0..3 {
        gs.emit_gossip();
    }

    // assert that no gossip gets sent to explicit peer
    let receiver = receivers.remove(&peers[0]).unwrap();
    let mut gossips = 0;
    let non_priority = receiver.non_priority.get_ref();
    while !non_priority.is_empty() {
        if let Ok(RpcOut::IHave(_)) = non_priority.try_recv() {
            gossips += 1;
        }
    }
    assert_eq!(gossips, 0, "Gossip got emitted to explicit peer");
}

/// Tests the mesh maintenance addition
#[test]
fn test_mesh_addition() {
    let config: Config = Config::default();

    // Adds mesh_low peers and PRUNE 2 giving us a deficit.
    let (mut gs, peers, _receivers, topics) = inject_nodes1()
        .peer_no(config.mesh_n() + 1)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .create_network();

    let to_remove_peers = config.mesh_n() + 1 - config.mesh_n_low() - 1;

    for peer in peers.iter().take(to_remove_peers) {
        gs.handle_prune(
            peer,
            topics.iter().map(|h| (h.clone(), vec![], None)).collect(),
        );
    }

    // Verify the pruned peers are removed from the mesh.
    assert_eq!(
        gs.mesh.get(&topics[0]).unwrap().len(),
        config.mesh_n_low() - 1
    );

    // run a heartbeat
    gs.heartbeat();

    // Peers should be added to reach mesh_n
    assert_eq!(gs.mesh.get(&topics[0]).unwrap().len(), config.mesh_n());
}

/// Tests the mesh maintenance subtraction
#[test]
fn test_mesh_subtraction() {
    let config = Config::default();

    // Adds mesh_low peers and PRUNE 2 giving us a deficit.
    let n = config.mesh_n_high() + 10;
    // make all outbound connections so that we allow grafting to all
    let (mut gs, peers, _receivers, topics) = inject_nodes1()
        .peer_no(n)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config.clone())
        .outbound(n)
        .create_network();

    // graft all the peers
    for peer in peers {
        gs.handle_graft(&peer, topics.clone());
    }

    // run a heartbeat
    gs.heartbeat();

    // Peers should be removed to reach mesh_n
    assert_eq!(gs.mesh.get(&topics[0]).unwrap().len(), config.mesh_n());
}

#[test]
fn test_connect_to_px_peers_on_handle_prune() {
    let config: Config = Config::default();

    let (mut gs, peers, _, topics) = inject_nodes1()
        .peer_no(1)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .create_network();

    // handle prune from single peer with px peers

    let mut px = Vec::new();
    // propose more px peers than config.prune_peers()
    for _ in 0..config.prune_peers() + 5 {
        px.push(PeerInfo {
            peer_id: Some(PeerId::random()),
        });
    }

    gs.handle_prune(
        &peers[0],
        vec![(
            topics[0].clone(),
            px.clone(),
            Some(config.prune_backoff().as_secs()),
        )],
    );

    // Check DialPeer events for px peers
    let dials: Vec<_> = gs
        .events
        .iter()
        .filter_map(|e| match e {
            ToSwarm::Dial { opts } => opts.get_peer_id(),
            _ => None,
        })
        .collect();

    // Exactly config.prune_peers() many random peers should be dialled
    assert_eq!(dials.len(), config.prune_peers());

    let dials_set: HashSet<_> = dials.into_iter().collect();

    // No duplicates
    assert_eq!(dials_set.len(), config.prune_peers());

    // all dial peers must be in px
    assert!(dials_set.is_subset(
        &px.iter()
            .map(|i| *i.peer_id.as_ref().unwrap())
            .collect::<HashSet<_>>()
    ));
}

#[test]
fn test_send_px_and_backoff_in_prune() {
    let config: Config = Config::default();

    // build mesh with enough peers for px
    let (mut gs, peers, receivers, topics) = inject_nodes1()
        .peer_no(config.prune_peers() + 1)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .create_network();

    // send prune to peer
    gs.send_graft_prune(
        HashMap::new(),
        vec![(peers[0], vec![topics[0].clone()])]
            .into_iter()
            .collect(),
        HashSet::new(),
    );

    // check prune message
    let (control_msgs, _) = count_control_msgs(receivers, |peer_id, m| {
        peer_id == &peers[0]
            && match m {
                RpcOut::Prune(Prune {
                    topic_hash,
                    peers,
                    backoff,
                }) => {
                    topic_hash == &topics[0] &&
                    peers.len() == config.prune_peers() &&
                    //all peers are different
                    peers.iter().collect::<HashSet<_>>().len() ==
                        config.prune_peers() &&
                    backoff.unwrap() == config.prune_backoff().as_secs()
                }
                _ => false,
            }
    });
    assert_eq!(control_msgs, 1);
}

#[test]
fn test_prune_backoffed_peer_on_graft() {
    let config: Config = Config::default();

    // build mesh with enough peers for px
    let (mut gs, peers, receivers, topics) = inject_nodes1()
        .peer_no(config.prune_peers() + 1)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .create_network();

    // remove peer from mesh and send prune to peer => this adds a backoff for this peer
    gs.mesh.get_mut(&topics[0]).unwrap().remove(&peers[0]);
    gs.send_graft_prune(
        HashMap::new(),
        vec![(peers[0], vec![topics[0].clone()])]
            .into_iter()
            .collect(),
        HashSet::new(),
    );

    // ignore all messages until now
    let receivers = flush_events(&mut gs, receivers);

    // handle graft
    gs.handle_graft(&peers[0], vec![topics[0].clone()]);

    // check prune message
    let (control_msgs, _) = count_control_msgs(receivers, |peer_id, m| {
        peer_id == &peers[0]
            && match m {
                RpcOut::Prune(Prune {
                    topic_hash,
                    peers,
                    backoff,
                }) => {
                    topic_hash == &topics[0] &&
                    //no px in this case
                    peers.is_empty() &&
                    backoff.unwrap() == config.prune_backoff().as_secs()
                }
                _ => false,
            }
    });
    assert_eq!(control_msgs, 1);
}

#[test]
fn test_do_not_graft_within_backoff_period() {
    let config = ConfigBuilder::default()
        .backoff_slack(1)
        .heartbeat_interval(Duration::from_millis(100))
        .build()
        .unwrap();
    // only one peer => mesh too small and will try to regraft as early as possible
    let (mut gs, peers, receivers, topics) = inject_nodes1()
        .peer_no(1)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config)
        .create_network();

    // handle prune from peer with backoff of one second
    gs.handle_prune(&peers[0], vec![(topics[0].clone(), Vec::new(), Some(1))]);

    // forget all events until now
    let receivers = flush_events(&mut gs, receivers);

    // call heartbeat
    gs.heartbeat();

    // Sleep for one second and apply 10 regular heartbeats (interval = 100ms).
    for _ in 0..10 {
        sleep(Duration::from_millis(100));
        gs.heartbeat();
    }

    // Check that no graft got created (we have backoff_slack = 1 therefore one more heartbeat
    // is needed).
    let (control_msgs, receivers) =
        count_control_msgs(receivers, |_, m| matches!(m, RpcOut::Graft { .. }));
    assert_eq!(
        control_msgs, 0,
        "Graft message created too early within backoff period"
    );

    // Heartbeat one more time this should graft now
    sleep(Duration::from_millis(100));
    gs.heartbeat();

    // check that graft got created
    let (control_msgs, _) = count_control_msgs(receivers, |_, m| matches!(m, RpcOut::Graft { .. }));
    assert!(
        control_msgs > 0,
        "No graft message was created after backoff period"
    );
}

#[test]
fn test_do_not_graft_within_default_backoff_period_after_receiving_prune_without_backoff() {
    // set default backoff period to 1 second
    let config = ConfigBuilder::default()
        .prune_backoff(Duration::from_millis(90))
        .backoff_slack(1)
        .heartbeat_interval(Duration::from_millis(100))
        .build()
        .unwrap();
    // only one peer => mesh too small and will try to regraft as early as possible
    let (mut gs, peers, receivers, topics) = inject_nodes1()
        .peer_no(1)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config)
        .create_network();

    // handle prune from peer without a specified backoff
    gs.handle_prune(&peers[0], vec![(topics[0].clone(), Vec::new(), None)]);

    // forget all events until now
    let receivers = flush_events(&mut gs, receivers);

    // call heartbeat
    gs.heartbeat();

    // Apply one more heartbeat
    sleep(Duration::from_millis(100));
    gs.heartbeat();

    // Check that no graft got created (we have backoff_slack = 1 therefore one more heartbeat
    // is needed).
    let (control_msgs, receivers) =
        count_control_msgs(receivers, |_, m| matches!(m, RpcOut::Graft { .. }));
    assert_eq!(
        control_msgs, 0,
        "Graft message created too early within backoff period"
    );

    // Heartbeat one more time this should graft now
    sleep(Duration::from_millis(100));
    gs.heartbeat();

    // check that graft got created
    let (control_msgs, _) = count_control_msgs(receivers, |_, m| matches!(m, RpcOut::Graft { .. }));
    assert!(
        control_msgs > 0,
        "No graft message was created after backoff period"
    );
}

#[test]
fn test_unsubscribe_backoff() {
    const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(100);
    let config = ConfigBuilder::default()
        .backoff_slack(1)
        // ensure a prune_backoff > unsubscribe_backoff
        .prune_backoff(Duration::from_secs(5))
        .unsubscribe_backoff(1)
        .heartbeat_interval(HEARTBEAT_INTERVAL)
        .build()
        .unwrap();

    let topic = String::from("test");
    // only one peer => mesh too small and will try to regraft as early as possible
    let (mut gs, _, receivers, topics) = inject_nodes1()
        .peer_no(1)
        .topics(vec![topic.clone()])
        .to_subscribe(true)
        .gs_config(config)
        .create_network();

    let _ = gs.unsubscribe(&Topic::new(topic));

    let (control_msgs, receivers) = count_control_msgs(receivers, |_, m| match m {
        RpcOut::Prune(Prune { backoff, .. }) => backoff == &Some(1),
        _ => false,
    });
    assert_eq!(
        control_msgs, 1,
        "Peer should be pruned with `unsubscribe_backoff`."
    );

    let _ = gs.subscribe(&Topic::new(topics[0].to_string()));

    // forget all events until now
    let receivers = flush_events(&mut gs, receivers);

    // call heartbeat
    gs.heartbeat();

    // Sleep for one second and apply 10 regular heartbeats (interval = 100ms).
    for _ in 0..10 {
        sleep(HEARTBEAT_INTERVAL);
        gs.heartbeat();
    }

    // Check that no graft got created (we have backoff_slack = 1 therefore one more heartbeat
    // is needed).
    let (control_msgs, receivers) =
        count_control_msgs(receivers, |_, m| matches!(m, RpcOut::Graft { .. }));
    assert_eq!(
        control_msgs, 0,
        "Graft message created too early within backoff period"
    );

    // Heartbeat one more time this should graft now
    sleep(HEARTBEAT_INTERVAL);
    gs.heartbeat();

    // check that graft got created
    let (control_msgs, _) = count_control_msgs(receivers, |_, m| matches!(m, RpcOut::Graft { .. }));
    assert!(
        control_msgs > 0,
        "No graft message was created after backoff period"
    );
}

#[test]
fn test_flood_publish() {
    let config: Config = Config::default();

    let topic = "test";
    // Adds more peers than mesh can hold to test flood publishing
    let (mut gs, _, receivers, _) = inject_nodes1()
        .peer_no(config.mesh_n_high() + 10)
        .topics(vec![topic.into()])
        .to_subscribe(true)
        .create_network();

    // publish message
    let publish_data = vec![0; 42];
    gs.publish(Topic::new(topic), publish_data).unwrap();

    // Collect all publish messages
    let publishes = receivers
        .into_values()
        .fold(vec![], |mut collected_publish, c| {
            let priority = c.priority.get_ref();
            while !priority.is_empty() {
                if let Ok(RpcOut::Publish { message, .. }) = priority.try_recv() {
                    collected_publish.push(message);
                }
            }
            collected_publish
        });

    // Transform the inbound message
    let message = &gs
        .data_transform
        .inbound_transform(
            publishes
                .first()
                .expect("Should contain > 0 entries")
                .clone(),
        )
        .unwrap();

    let msg_id = gs.config.message_id(message);

    let config: Config = Config::default();
    assert_eq!(
        publishes.len(),
        config.mesh_n_high() + 10,
        "Should send a publish message to all known peers"
    );

    assert!(
        gs.mcache.get(&msg_id).is_some(),
        "Message cache should contain published message"
    );
}

#[test]
fn test_gossip_to_at_least_gossip_lazy_peers() {
    let config: Config = Config::default();

    // add more peers than in mesh to test gossipping
    // by default only mesh_n_low peers will get added to mesh
    let (mut gs, _, receivers, topic_hashes) = inject_nodes1()
        .peer_no(config.mesh_n_low() + config.gossip_lazy() + 1)
        .topics(vec!["topic".into()])
        .to_subscribe(true)
        .create_network();

    // receive message
    let raw_message = RawMessage {
        source: Some(PeerId::random()),
        data: vec![],
        sequence_number: Some(0),
        topic: topic_hashes[0].clone(),
        signature: None,
        key: None,
        validated: true,
    };
    gs.handle_received_message(raw_message.clone(), &PeerId::random());

    // emit gossip
    gs.emit_gossip();

    // Transform the inbound message
    let message = &gs.data_transform.inbound_transform(raw_message).unwrap();

    let msg_id = gs.config.message_id(message);

    // check that exactly config.gossip_lazy() many gossip messages were sent.
    let (control_msgs, _) = count_control_msgs(receivers, |_, action| match action {
        RpcOut::IHave(IHave {
            topic_hash,
            message_ids,
        }) => topic_hash == &topic_hashes[0] && message_ids.iter().any(|id| id == &msg_id),
        _ => false,
    });
    assert_eq!(control_msgs, config.gossip_lazy());
}

#[test]
fn test_gossip_to_at_most_gossip_factor_peers() {
    let config: Config = Config::default();

    // add a lot of peers
    let m = config.mesh_n_low() + config.gossip_lazy() * (2.0 / config.gossip_factor()) as usize;
    let (mut gs, _, receivers, topic_hashes) = inject_nodes1()
        .peer_no(m)
        .topics(vec!["topic".into()])
        .to_subscribe(true)
        .create_network();

    // receive message
    let raw_message = RawMessage {
        source: Some(PeerId::random()),
        data: vec![],
        sequence_number: Some(0),
        topic: topic_hashes[0].clone(),
        signature: None,
        key: None,
        validated: true,
    };
    gs.handle_received_message(raw_message.clone(), &PeerId::random());

    // emit gossip
    gs.emit_gossip();

    // Transform the inbound message
    let message = &gs.data_transform.inbound_transform(raw_message).unwrap();

    let msg_id = gs.config.message_id(message);
    // check that exactly config.gossip_lazy() many gossip messages were sent.
    let (control_msgs, _) = count_control_msgs(receivers, |_, action| match action {
        RpcOut::IHave(IHave {
            topic_hash,
            message_ids,
        }) => topic_hash == &topic_hashes[0] && message_ids.iter().any(|id| id == &msg_id),
        _ => false,
    });
    assert_eq!(
        control_msgs,
        ((m - config.mesh_n_low()) as f64 * config.gossip_factor()) as usize
    );
}

#[test]
fn test_accept_only_outbound_peer_grafts_when_mesh_full() {
    let config: Config = Config::default();

    // enough peers to fill the mesh
    let (mut gs, peers, _, topics) = inject_nodes1()
        .peer_no(config.mesh_n_high())
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .create_network();

    // graft all the peers => this will fill the mesh
    for peer in peers {
        gs.handle_graft(&peer, topics.clone());
    }

    // assert current mesh size
    assert_eq!(gs.mesh[&topics[0]].len(), config.mesh_n_high());

    // create an outbound and an inbound peer
    let (inbound, _in_receiver) = add_peer(&mut gs, &topics, false, false);
    let (outbound, _out_receiver) = add_peer(&mut gs, &topics, true, false);

    // send grafts
    gs.handle_graft(&inbound, vec![topics[0].clone()]);
    gs.handle_graft(&outbound, vec![topics[0].clone()]);

    // assert mesh size
    assert_eq!(gs.mesh[&topics[0]].len(), config.mesh_n_high() + 1);

    // inbound is not in mesh
    assert!(!gs.mesh[&topics[0]].contains(&inbound));

    // outbound is in mesh
    assert!(gs.mesh[&topics[0]].contains(&outbound));
}

#[test]
fn test_do_not_remove_too_many_outbound_peers() {
    // use an extreme case to catch errors with high probability
    let m = 50;
    let n = 2 * m;
    let config = ConfigBuilder::default()
        .mesh_n_high(n)
        .mesh_n(n)
        .mesh_n_low(n)
        .mesh_outbound_min(m)
        .build()
        .unwrap();

    // fill the mesh with inbound connections
    let (mut gs, peers, _receivers, topics) = inject_nodes1()
        .peer_no(n)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config)
        .create_network();

    // graft all the peers
    for peer in peers {
        gs.handle_graft(&peer, topics.clone());
    }

    // create m outbound connections and graft (we will accept the graft)
    let mut outbound = HashSet::new();
    for _ in 0..m {
        let (peer, _) = add_peer(&mut gs, &topics, true, false);
        outbound.insert(peer);
        gs.handle_graft(&peer, topics.clone());
    }

    // mesh is overly full
    assert_eq!(gs.mesh.get(&topics[0]).unwrap().len(), n + m);

    // run a heartbeat
    gs.heartbeat();

    // Peers should be removed to reach n
    assert_eq!(gs.mesh.get(&topics[0]).unwrap().len(), n);

    // all outbound peers are still in the mesh
    assert!(outbound.iter().all(|p| gs.mesh[&topics[0]].contains(p)));
}

#[test]
fn test_add_outbound_peers_if_min_is_not_satisfied() {
    let config: Config = Config::default();

    // Fill full mesh with inbound peers
    let (mut gs, peers, _, topics) = inject_nodes1()
        .peer_no(config.mesh_n_high())
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .create_network();

    // graft all the peers
    for peer in peers {
        gs.handle_graft(&peer, topics.clone());
    }

    // create config.mesh_outbound_min() many outbound connections without grafting
    let mut peers = vec![];
    for _ in 0..config.mesh_outbound_min() {
        peers.push(add_peer(&mut gs, &topics, true, false));
    }

    // Nothing changed in the mesh yet
    assert_eq!(gs.mesh[&topics[0]].len(), config.mesh_n_high());

    // run a heartbeat
    gs.heartbeat();

    // The outbound peers got additionally added
    assert_eq!(
        gs.mesh[&topics[0]].len(),
        config.mesh_n_high() + config.mesh_outbound_min()
    );
}

#[test]
fn test_prune_negative_scored_peers() {
    let config = Config::default();

    // build mesh with one peer
    let (mut gs, peers, receivers, topics) = inject_nodes1()
        .peer_no(1)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config.clone())
        .explicit(0)
        .outbound(0)
        .scoring(Some((
            PeerScoreParams::default(),
            PeerScoreThresholds::default(),
        )))
        .create_network();

    // add penalty to peer
    gs.as_peer_score_mut().add_penalty(&peers[0], 1);

    // execute heartbeat
    gs.heartbeat();

    // peer should not be in mesh anymore
    assert!(gs.mesh[&topics[0]].is_empty());

    // check prune message
    let (control_msgs, _) = count_control_msgs(receivers, |peer_id, m| {
        peer_id == &peers[0]
            && match m {
                RpcOut::Prune(Prune {
                    topic_hash,
                    peers,
                    backoff,
                }) => {
                    topic_hash == &topics[0] &&
                    //no px in this case
                    peers.is_empty() &&
                    backoff.unwrap() == config.prune_backoff().as_secs()
                }
                _ => false,
            }
    });
    assert_eq!(control_msgs, 1);
}

#[test]
fn test_dont_graft_to_negative_scored_peers() {
    let config = Config::default();
    // init full mesh
    let (mut gs, peers, _, topics) = inject_nodes1()
        .peer_no(config.mesh_n_high())
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config)
        .scoring(Some((
            PeerScoreParams::default(),
            PeerScoreThresholds::default(),
        )))
        .create_network();

    // add two additional peers that will not be part of the mesh
    let (p1, _receiver1) = add_peer(&mut gs, &topics, false, false);
    let (p2, _receiver2) = add_peer(&mut gs, &topics, false, false);

    // reduce score of p1 to negative
    gs.as_peer_score_mut().add_penalty(&p1, 1);

    // handle prunes of all other peers
    for p in peers {
        gs.handle_prune(&p, vec![(topics[0].clone(), Vec::new(), None)]);
    }

    // heartbeat
    gs.heartbeat();

    // assert that mesh only contains p2
    assert_eq!(gs.mesh.get(&topics[0]).unwrap().len(), 1);
    assert!(gs.mesh.get(&topics[0]).unwrap().contains(&p2));
}

/// Note that in this test also without a penalty the px would be ignored because of the
/// acceptPXThreshold, but the spec still explicitly states the rule that px from negative
/// peers should get ignored, therefore we test it here.
#[test]
fn test_ignore_px_from_negative_scored_peer() {
    let config = Config::default();

    // build mesh with one peer
    let (mut gs, peers, _, topics) = inject_nodes1()
        .peer_no(1)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config.clone())
        .scoring(Some((
            PeerScoreParams::default(),
            PeerScoreThresholds::default(),
        )))
        .create_network();

    // penalize peer
    gs.as_peer_score_mut().add_penalty(&peers[0], 1);

    // handle prune from single peer with px peers
    let px = vec![PeerInfo {
        peer_id: Some(PeerId::random()),
    }];

    gs.handle_prune(
        &peers[0],
        vec![(
            topics[0].clone(),
            px,
            Some(config.prune_backoff().as_secs()),
        )],
    );

    // assert no dials
    assert_eq!(
        gs.events
            .iter()
            .filter(|e| matches!(e, ToSwarm::Dial { .. }))
            .count(),
        0
    );
}

#[test]
fn test_only_send_nonnegative_scoring_peers_in_px() {
    let config = ConfigBuilder::default()
        .prune_peers(16)
        .do_px()
        .build()
        .unwrap();

    // Build mesh with three peer
    let (mut gs, peers, receivers, topics) = inject_nodes1()
        .peer_no(3)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config)
        .explicit(0)
        .outbound(0)
        .scoring(Some((
            PeerScoreParams::default(),
            PeerScoreThresholds::default(),
        )))
        .create_network();

    // Penalize first peer
    gs.as_peer_score_mut().add_penalty(&peers[0], 1);

    // Prune second peer
    gs.send_graft_prune(
        HashMap::new(),
        vec![(peers[1], vec![topics[0].clone()])]
            .into_iter()
            .collect(),
        HashSet::new(),
    );

    // Check that px in prune message only contains third peer
    let (control_msgs, _) = count_control_msgs(receivers, |peer_id, m| {
        peer_id == &peers[1]
            && match m {
                RpcOut::Prune(Prune {
                    topic_hash,
                    peers: px,
                    ..
                }) => {
                    topic_hash == &topics[0]
                        && px.len() == 1
                        && px[0].peer_id.as_ref().unwrap() == &peers[2]
                }
                _ => false,
            }
    });
    assert_eq!(control_msgs, 1);
}

#[test]
fn test_do_not_gossip_to_peers_below_gossip_threshold() {
    let config = Config::default();
    let peer_score_params = PeerScoreParams::default();
    let peer_score_thresholds = PeerScoreThresholds {
        gossip_threshold: 3.0 * peer_score_params.behaviour_penalty_weight,
        ..PeerScoreThresholds::default()
    };

    // Build full mesh
    let (mut gs, peers, mut receivers, topics) = inject_nodes1()
        .peer_no(config.mesh_n_high())
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config)
        .scoring(Some((peer_score_params, peer_score_thresholds)))
        .create_network();

    // Graft all the peer
    for peer in peers {
        gs.handle_graft(&peer, topics.clone());
    }

    // Add two additional peers that will not be part of the mesh
    let (p1, receiver1) = add_peer(&mut gs, &topics, false, false);
    receivers.insert(p1, receiver1);
    let (p2, receiver2) = add_peer(&mut gs, &topics, false, false);
    receivers.insert(p2, receiver2);

    // Reduce score of p1 below peer_score_thresholds.gossip_threshold
    // note that penalties get squared so two penalties means a score of
    // 4 * peer_score_params.behaviour_penalty_weight.
    gs.as_peer_score_mut().add_penalty(&p1, 2);

    // Reduce score of p2 below 0 but not below peer_score_thresholds.gossip_threshold
    gs.as_peer_score_mut().add_penalty(&p2, 1);

    // Receive message
    let raw_message = RawMessage {
        source: Some(PeerId::random()),
        data: vec![],
        sequence_number: Some(0),
        topic: topics[0].clone(),
        signature: None,
        key: None,
        validated: true,
    };
    gs.handle_received_message(raw_message.clone(), &PeerId::random());

    // Transform the inbound message
    let message = &gs.data_transform.inbound_transform(raw_message).unwrap();

    let msg_id = gs.config.message_id(message);

    // Emit gossip
    gs.emit_gossip();

    // Check that exactly one gossip messages got sent and it got sent to p2
    let (control_msgs, _) = count_control_msgs(receivers, |peer, action| match action {
        RpcOut::IHave(IHave {
            topic_hash,
            message_ids,
        }) => {
            if topic_hash == &topics[0] && message_ids.iter().any(|id| id == &msg_id) {
                assert_eq!(peer, &p2);
                true
            } else {
                false
            }
        }
        _ => false,
    });
    assert_eq!(control_msgs, 1);
}

#[test]
fn test_iwant_msg_from_peer_below_gossip_threshold_gets_ignored() {
    let config = Config::default();
    let peer_score_params = PeerScoreParams::default();
    let peer_score_thresholds = PeerScoreThresholds {
        gossip_threshold: 3.0 * peer_score_params.behaviour_penalty_weight,
        ..PeerScoreThresholds::default()
    };

    // Build full mesh
    let (mut gs, peers, mut receivers, topics) = inject_nodes1()
        .peer_no(config.mesh_n_high())
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config)
        .explicit(0)
        .outbound(0)
        .scoring(Some((peer_score_params, peer_score_thresholds)))
        .create_network();

    // Graft all the peer
    for peer in peers {
        gs.handle_graft(&peer, topics.clone());
    }

    // Add two additional peers that will not be part of the mesh
    let (p1, receiver1) = add_peer(&mut gs, &topics, false, false);
    receivers.insert(p1, receiver1);
    let (p2, receiver2) = add_peer(&mut gs, &topics, false, false);
    receivers.insert(p2, receiver2);

    // Reduce score of p1 below peer_score_thresholds.gossip_threshold
    // note that penalties get squared so two penalties means a score of
    // 4 * peer_score_params.behaviour_penalty_weight.
    gs.as_peer_score_mut().add_penalty(&p1, 2);

    // Reduce score of p2 below 0 but not below peer_score_thresholds.gossip_threshold
    gs.as_peer_score_mut().add_penalty(&p2, 1);

    // Receive message
    let raw_message = RawMessage {
        source: Some(PeerId::random()),
        data: vec![],
        sequence_number: Some(0),
        topic: topics[0].clone(),
        signature: None,
        key: None,
        validated: true,
    };
    gs.handle_received_message(raw_message.clone(), &PeerId::random());

    // Transform the inbound message
    let message = &gs.data_transform.inbound_transform(raw_message).unwrap();

    let msg_id = gs.config.message_id(message);

    gs.handle_iwant(&p1, vec![msg_id.clone()]);
    gs.handle_iwant(&p2, vec![msg_id.clone()]);

    // the messages we are sending
    let sent_messages =
        receivers
            .into_iter()
            .fold(vec![], |mut collected_messages, (peer_id, c)| {
                let non_priority = c.non_priority.get_ref();
                while !non_priority.is_empty() {
                    if let Ok(RpcOut::Forward { message, .. }) = non_priority.try_recv() {
                        collected_messages.push((peer_id, message));
                    }
                }
                collected_messages
            });

    // the message got sent to p2
    assert!(sent_messages
        .iter()
        .map(|(peer_id, msg)| (
            peer_id,
            gs.data_transform.inbound_transform(msg.clone()).unwrap()
        ))
        .any(|(peer_id, msg)| peer_id == &p2 && gs.config.message_id(&msg) == msg_id));
    // the message got not sent to p1
    assert!(sent_messages
        .iter()
        .map(|(peer_id, msg)| (
            peer_id,
            gs.data_transform.inbound_transform(msg.clone()).unwrap()
        ))
        .all(|(peer_id, msg)| !(peer_id == &p1 && gs.config.message_id(&msg) == msg_id)));
}

#[test]
fn test_ihave_msg_from_peer_below_gossip_threshold_gets_ignored() {
    let config = Config::default();
    let peer_score_params = PeerScoreParams::default();
    let peer_score_thresholds = PeerScoreThresholds {
        gossip_threshold: 3.0 * peer_score_params.behaviour_penalty_weight,
        ..PeerScoreThresholds::default()
    };
    // build full mesh
    let (mut gs, peers, mut receivers, topics) = inject_nodes1()
        .peer_no(config.mesh_n_high())
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config)
        .explicit(0)
        .outbound(0)
        .scoring(Some((peer_score_params, peer_score_thresholds)))
        .create_network();

    // graft all the peer
    for peer in peers {
        gs.handle_graft(&peer, topics.clone());
    }

    // add two additional peers that will not be part of the mesh
    let (p1, receiver1) = add_peer(&mut gs, &topics, false, false);
    receivers.insert(p1, receiver1);
    let (p2, receiver2) = add_peer(&mut gs, &topics, false, false);
    receivers.insert(p2, receiver2);

    // reduce score of p1 below peer_score_thresholds.gossip_threshold
    // note that penalties get squared so two penalties means a score of
    // 4 * peer_score_params.behaviour_penalty_weight.
    gs.as_peer_score_mut().add_penalty(&p1, 2);

    // reduce score of p2 below 0 but not below peer_score_thresholds.gossip_threshold
    gs.as_peer_score_mut().add_penalty(&p2, 1);

    // message that other peers have
    let raw_message = RawMessage {
        source: Some(PeerId::random()),
        data: vec![],
        sequence_number: Some(0),
        topic: topics[0].clone(),
        signature: None,
        key: None,
        validated: true,
    };

    // Transform the inbound message
    let message = &gs.data_transform.inbound_transform(raw_message).unwrap();

    let msg_id = gs.config.message_id(message);

    gs.handle_ihave(&p1, vec![(topics[0].clone(), vec![msg_id.clone()])]);
    gs.handle_ihave(&p2, vec![(topics[0].clone(), vec![msg_id.clone()])]);

    // check that we sent exactly one IWANT request to p2
    let (control_msgs, _) = count_control_msgs(receivers, |peer, c| match c {
        RpcOut::IWant(IWant { message_ids }) => {
            if message_ids.iter().any(|m| m == &msg_id) {
                assert_eq!(peer, &p2);
                true
            } else {
                false
            }
        }
        _ => false,
    });
    assert_eq!(control_msgs, 1);
}

#[test]
fn test_do_not_publish_to_peer_below_publish_threshold() {
    let config = ConfigBuilder::default()
        .flood_publish(false)
        .build()
        .unwrap();
    let peer_score_params = PeerScoreParams::default();
    let peer_score_thresholds = PeerScoreThresholds {
        gossip_threshold: 0.5 * peer_score_params.behaviour_penalty_weight,
        publish_threshold: 3.0 * peer_score_params.behaviour_penalty_weight,
        ..PeerScoreThresholds::default()
    };

    // build mesh with no peers and no subscribed topics
    let (mut gs, _, mut receivers, _) = inject_nodes1()
        .gs_config(config)
        .scoring(Some((peer_score_params, peer_score_thresholds)))
        .create_network();

    // create a new topic for which we are not subscribed
    let topic = Topic::new("test");
    let topics = vec![topic.hash()];

    // add two additional peers that will be added to the mesh
    let (p1, receiver1) = add_peer(&mut gs, &topics, false, false);
    receivers.insert(p1, receiver1);
    let (p2, receiver2) = add_peer(&mut gs, &topics, false, false);
    receivers.insert(p2, receiver2);

    // reduce score of p1 below peer_score_thresholds.publish_threshold
    // note that penalties get squared so two penalties means a score of
    // 4 * peer_score_params.behaviour_penalty_weight.
    gs.as_peer_score_mut().add_penalty(&p1, 2);

    // reduce score of p2 below 0 but not below peer_score_thresholds.publish_threshold
    gs.as_peer_score_mut().add_penalty(&p2, 1);

    // a heartbeat will remove the peers from the mesh
    gs.heartbeat();

    // publish on topic
    let publish_data = vec![0; 42];
    gs.publish(topic, publish_data).unwrap();

    // Collect all publish messages
    let publishes = receivers
        .into_iter()
        .fold(vec![], |mut collected_publish, (peer_id, c)| {
            let priority = c.priority.get_ref();
            while !priority.is_empty() {
                if let Ok(RpcOut::Publish { message, .. }) = priority.try_recv() {
                    collected_publish.push((peer_id, message));
                }
            }
            collected_publish
        });

    // assert only published to p2
    assert_eq!(publishes.len(), 1);
    assert_eq!(publishes[0].0, p2);
}

#[test]
fn test_do_not_flood_publish_to_peer_below_publish_threshold() {
    let config = Config::default();
    let peer_score_params = PeerScoreParams::default();
    let peer_score_thresholds = PeerScoreThresholds {
        gossip_threshold: 0.5 * peer_score_params.behaviour_penalty_weight,
        publish_threshold: 3.0 * peer_score_params.behaviour_penalty_weight,
        ..PeerScoreThresholds::default()
    };
    // build mesh with no peers
    let (mut gs, _, mut receivers, topics) = inject_nodes1()
        .topics(vec!["test".into()])
        .gs_config(config)
        .scoring(Some((peer_score_params, peer_score_thresholds)))
        .create_network();

    // add two additional peers that will be added to the mesh
    let (p1, receiver1) = add_peer(&mut gs, &topics, false, false);
    receivers.insert(p1, receiver1);
    let (p2, receiver2) = add_peer(&mut gs, &topics, false, false);
    receivers.insert(p2, receiver2);

    // reduce score of p1 below peer_score_thresholds.publish_threshold
    // note that penalties get squared so two penalties means a score of
    // 4 * peer_score_params.behaviour_penalty_weight.
    gs.as_peer_score_mut().add_penalty(&p1, 2);

    // reduce score of p2 below 0 but not below peer_score_thresholds.publish_threshold
    gs.as_peer_score_mut().add_penalty(&p2, 1);

    // a heartbeat will remove the peers from the mesh
    gs.heartbeat();

    // publish on topic
    let publish_data = vec![0; 42];
    gs.publish(Topic::new("test"), publish_data).unwrap();

    // Collect all publish messages
    let publishes = receivers
        .into_iter()
        .fold(vec![], |mut collected_publish, (peer_id, c)| {
            let priority = c.priority.get_ref();
            while !priority.is_empty() {
                if let Ok(RpcOut::Publish { message, .. }) = priority.try_recv() {
                    collected_publish.push((peer_id, message))
                }
            }
            collected_publish
        });

    // assert only published to p2
    assert_eq!(publishes.len(), 1);
    assert!(publishes[0].0 == p2);
}

#[test]
fn test_ignore_rpc_from_peers_below_graylist_threshold() {
    let config = Config::default();
    let peer_score_params = PeerScoreParams::default();
    let peer_score_thresholds = PeerScoreThresholds {
        gossip_threshold: 0.5 * peer_score_params.behaviour_penalty_weight,
        publish_threshold: 0.5 * peer_score_params.behaviour_penalty_weight,
        graylist_threshold: 3.0 * peer_score_params.behaviour_penalty_weight,
        ..PeerScoreThresholds::default()
    };

    // build mesh with no peers
    let (mut gs, _, _, topics) = inject_nodes1()
        .topics(vec!["test".into()])
        .gs_config(config.clone())
        .scoring(Some((peer_score_params, peer_score_thresholds)))
        .create_network();

    // add two additional peers that will be added to the mesh
    let (p1, _receiver1) = add_peer(&mut gs, &topics, false, false);
    let (p2, _receiver2) = add_peer(&mut gs, &topics, false, false);

    // reduce score of p1 below peer_score_thresholds.graylist_threshold
    // note that penalties get squared so two penalties means a score of
    // 4 * peer_score_params.behaviour_penalty_weight.
    gs.as_peer_score_mut().add_penalty(&p1, 2);

    // reduce score of p2 below publish_threshold but not below graylist_threshold
    gs.as_peer_score_mut().add_penalty(&p2, 1);

    let raw_message1 = RawMessage {
        source: Some(PeerId::random()),
        data: vec![1, 2, 3, 4],
        sequence_number: Some(1u64),
        topic: topics[0].clone(),
        signature: None,
        key: None,
        validated: true,
    };

    let raw_message2 = RawMessage {
        source: Some(PeerId::random()),
        data: vec![1, 2, 3, 4, 5],
        sequence_number: Some(2u64),
        topic: topics[0].clone(),
        signature: None,
        key: None,
        validated: true,
    };

    let raw_message3 = RawMessage {
        source: Some(PeerId::random()),
        data: vec![1, 2, 3, 4, 5, 6],
        sequence_number: Some(3u64),
        topic: topics[0].clone(),
        signature: None,
        key: None,
        validated: true,
    };

    let raw_message4 = RawMessage {
        source: Some(PeerId::random()),
        data: vec![1, 2, 3, 4, 5, 6, 7],
        sequence_number: Some(4u64),
        topic: topics[0].clone(),
        signature: None,
        key: None,
        validated: true,
    };

    // Transform the inbound message
    let message2 = &gs.data_transform.inbound_transform(raw_message2).unwrap();

    // Transform the inbound message
    let message4 = &gs.data_transform.inbound_transform(raw_message4).unwrap();

    let subscription = Subscription {
        action: SubscriptionAction::Subscribe,
        topic_hash: topics[0].clone(),
    };

    let control_action = ControlAction::IHave(IHave {
        topic_hash: topics[0].clone(),
        message_ids: vec![config.message_id(message2)],
    });

    // clear events
    gs.events.clear();

    // receive from p1
    gs.on_connection_handler_event(
        p1,
        ConnectionId::new_unchecked(0),
        HandlerEvent::Message {
            rpc: RpcIn {
                messages: vec![raw_message1],
                subscriptions: vec![subscription.clone()],
                control_msgs: vec![control_action],
            },
            invalid_messages: Vec::new(),
        },
    );

    // only the subscription event gets processed, the rest is dropped
    assert_eq!(gs.events.len(), 1);
    assert!(matches!(
        gs.events[0],
        ToSwarm::GenerateEvent(Event::Subscribed { .. })
    ));

    let control_action = ControlAction::IHave(IHave {
        topic_hash: topics[0].clone(),
        message_ids: vec![config.message_id(message4)],
    });

    // receive from p2
    gs.on_connection_handler_event(
        p2,
        ConnectionId::new_unchecked(0),
        HandlerEvent::Message {
            rpc: RpcIn {
                messages: vec![raw_message3],
                subscriptions: vec![subscription],
                control_msgs: vec![control_action],
            },
            invalid_messages: Vec::new(),
        },
    );

    // events got processed
    assert!(gs.events.len() > 1);
}

#[test]
fn test_ignore_px_from_peers_below_accept_px_threshold() {
    let config = ConfigBuilder::default().prune_peers(16).build().unwrap();
    let peer_score_params = PeerScoreParams::default();
    let peer_score_thresholds = PeerScoreThresholds {
        accept_px_threshold: peer_score_params.app_specific_weight,
        ..PeerScoreThresholds::default()
    };
    // Build mesh with two peers
    let (mut gs, peers, _, topics) = inject_nodes1()
        .peer_no(2)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config.clone())
        .scoring(Some((peer_score_params, peer_score_thresholds)))
        .create_network();

    // Decrease score of first peer to less than accept_px_threshold
    gs.set_application_score(&peers[0], 0.99);

    // Increase score of second peer to accept_px_threshold
    gs.set_application_score(&peers[1], 1.0);

    // Handle prune from peer peers[0] with px peers
    let px = vec![PeerInfo {
        peer_id: Some(PeerId::random()),
    }];
    gs.handle_prune(
        &peers[0],
        vec![(
            topics[0].clone(),
            px,
            Some(config.prune_backoff().as_secs()),
        )],
    );

    // Assert no dials
    assert_eq!(
        gs.events
            .iter()
            .filter(|e| matches!(e, ToSwarm::Dial { .. }))
            .count(),
        0
    );

    // handle prune from peer peers[1] with px peers
    let px = vec![PeerInfo {
        peer_id: Some(PeerId::random()),
    }];
    gs.handle_prune(
        &peers[1],
        vec![(
            topics[0].clone(),
            px,
            Some(config.prune_backoff().as_secs()),
        )],
    );

    // assert there are dials now
    assert!(
        gs.events
            .iter()
            .filter(|e| matches!(e, ToSwarm::Dial { .. }))
            .count()
            > 0
    );
}

#[test]
fn test_keep_best_scoring_peers_on_oversubscription() {
    let config = ConfigBuilder::default()
        .mesh_n_low(15)
        .mesh_n(30)
        .mesh_n_high(60)
        .retain_scores(29)
        .build()
        .unwrap();

    // build mesh with more peers than mesh can hold
    let n = config.mesh_n_high() + 1;
    let (mut gs, peers, _receivers, topics) = inject_nodes1()
        .peer_no(n)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config.clone())
        .explicit(0)
        .outbound(n)
        .scoring(Some((
            PeerScoreParams::default(),
            PeerScoreThresholds::default(),
        )))
        .create_network();

    // graft all, will be accepted since the are outbound
    for peer in &peers {
        gs.handle_graft(peer, topics.clone());
    }

    // assign scores to peers equalling their index

    // set random positive scores
    for (index, peer) in peers.iter().enumerate() {
        gs.set_application_score(peer, index as f64);
    }

    assert_eq!(gs.mesh[&topics[0]].len(), n);

    // heartbeat to prune some peers
    gs.heartbeat();

    assert_eq!(gs.mesh[&topics[0]].len(), config.mesh_n());

    // mesh contains retain_scores best peers
    assert!(gs.mesh[&topics[0]].is_superset(
        &peers[(n - config.retain_scores())..]
            .iter()
            .cloned()
            .collect()
    ));
}

#[test]
fn test_scoring_p1() {
    let config = Config::default();
    let mut peer_score_params = PeerScoreParams::default();
    let topic = Topic::new("test");
    let topic_hash = topic.hash();
    let topic_params = TopicScoreParams {
        time_in_mesh_weight: 2.0,
        time_in_mesh_quantum: Duration::from_millis(50),
        time_in_mesh_cap: 10.0,
        topic_weight: 0.7,
        ..TopicScoreParams::default()
    };
    peer_score_params
        .topics
        .insert(topic_hash, topic_params.clone());
    let peer_score_thresholds = PeerScoreThresholds::default();

    // build mesh with one peer
    let (mut gs, peers, _, _) = inject_nodes1()
        .peer_no(1)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config)
        .explicit(0)
        .outbound(0)
        .scoring(Some((peer_score_params, peer_score_thresholds)))
        .create_network();

    // sleep for 2 times the mesh_quantum
    sleep(topic_params.time_in_mesh_quantum * 2);
    // refresh scores
    gs.as_peer_score_mut().refresh_scores();
    assert!(
        gs.as_peer_score_mut().score_report(&peers[0]).score
            >= 2.0 * topic_params.time_in_mesh_weight * topic_params.topic_weight,
        "score should be at least 2 * time_in_mesh_weight * topic_weight"
    );
    assert!(
        gs.as_peer_score_mut().score_report(&peers[0]).score
            < 3.0 * topic_params.time_in_mesh_weight * topic_params.topic_weight,
        "score should be less than 3 * time_in_mesh_weight * topic_weight"
    );

    // sleep again for 2 times the mesh_quantum
    sleep(topic_params.time_in_mesh_quantum * 2);
    // refresh scores
    gs.as_peer_score_mut().refresh_scores();
    assert!(
        gs.as_peer_score_mut().score_report(&peers[0]).score
            >= 2.0 * topic_params.time_in_mesh_weight * topic_params.topic_weight,
        "score should be at least 4 * time_in_mesh_weight * topic_weight"
    );

    // sleep for enough periods to reach maximum
    sleep(topic_params.time_in_mesh_quantum * (topic_params.time_in_mesh_cap - 3.0) as u32);
    // refresh scores
    gs.as_peer_score_mut().refresh_scores();
    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[0]).score,
        topic_params.time_in_mesh_cap
            * topic_params.time_in_mesh_weight
            * topic_params.topic_weight,
        "score should be exactly time_in_mesh_cap * time_in_mesh_weight * topic_weight"
    );
}

fn random_message(seq: &mut u64, topics: &[TopicHash]) -> RawMessage {
    let mut rng = rand::thread_rng();
    *seq += 1;
    RawMessage {
        source: Some(PeerId::random()),
        data: (0..rng.gen_range(10..30)).map(|_| rng.gen()).collect(),
        sequence_number: Some(*seq),
        topic: topics[rng.gen_range(0..topics.len())].clone(),
        signature: None,
        key: None,
        validated: true,
    }
}

#[test]
fn test_scoring_p2() {
    let config = Config::default();
    let mut peer_score_params = PeerScoreParams::default();
    let topic = Topic::new("test");
    let topic_hash = topic.hash();
    let topic_params = TopicScoreParams {
        time_in_mesh_weight: 0.0, // deactivate time in mesh
        first_message_deliveries_weight: 2.0,
        first_message_deliveries_cap: 10.0,
        first_message_deliveries_decay: 0.9,
        topic_weight: 0.7,
        ..TopicScoreParams::default()
    };
    peer_score_params
        .topics
        .insert(topic_hash, topic_params.clone());
    let peer_score_thresholds = PeerScoreThresholds::default();

    // build mesh with one peer
    let (mut gs, peers, _, topics) = inject_nodes1()
        .peer_no(2)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config)
        .explicit(0)
        .outbound(0)
        .scoring(Some((peer_score_params, peer_score_thresholds)))
        .create_network();

    let mut seq = 0;
    let deliver_message = |gs: &mut Behaviour, index: usize, msg: RawMessage| {
        gs.handle_received_message(msg, &peers[index]);
    };

    let m1 = random_message(&mut seq, &topics);
    // peer 0 delivers message first
    deliver_message(&mut gs, 0, m1.clone());
    // peer 1 delivers message second
    deliver_message(&mut gs, 1, m1);

    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[0]).score,
        1.0 * topic_params.first_message_deliveries_weight * topic_params.topic_weight,
        "score should be exactly first_message_deliveries_weight * topic_weight"
    );

    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[1]).score,
        0.0,
        "there should be no score for second message deliveries * topic_weight"
    );

    // peer 2 delivers two new messages
    deliver_message(&mut gs, 1, random_message(&mut seq, &topics));
    deliver_message(&mut gs, 1, random_message(&mut seq, &topics));
    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[1]).score,
        2.0 * topic_params.first_message_deliveries_weight * topic_params.topic_weight,
        "score should be exactly 2 * first_message_deliveries_weight * topic_weight"
    );

    // test decaying
    gs.as_peer_score_mut().refresh_scores();

    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[0]).score,
        1.0 * topic_params.first_message_deliveries_decay
            * topic_params.first_message_deliveries_weight
            * topic_params.topic_weight,
        "score should be exactly first_message_deliveries_decay * \
               first_message_deliveries_weight * topic_weight"
    );

    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[1]).score,
        2.0 * topic_params.first_message_deliveries_decay
            * topic_params.first_message_deliveries_weight
            * topic_params.topic_weight,
        "score should be exactly 2 * first_message_deliveries_decay * \
               first_message_deliveries_weight * topic_weight"
    );

    // test cap
    for _ in 0..topic_params.first_message_deliveries_cap as u64 {
        deliver_message(&mut gs, 1, random_message(&mut seq, &topics));
    }

    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[1]).score,
        topic_params.first_message_deliveries_cap
            * topic_params.first_message_deliveries_weight
            * topic_params.topic_weight,
        "score should be exactly first_message_deliveries_cap * \
               first_message_deliveries_weight * topic_weight"
    );
}

#[test]
fn test_scoring_p3() {
    let config = Config::default();
    let mut peer_score_params = PeerScoreParams::default();
    let topic = Topic::new("test");
    let topic_hash = topic.hash();
    let topic_params = TopicScoreParams {
        time_in_mesh_weight: 0.0,             // deactivate time in mesh
        first_message_deliveries_weight: 0.0, // deactivate first time deliveries
        mesh_message_deliveries_weight: -2.0,
        mesh_message_deliveries_decay: 0.9,
        mesh_message_deliveries_cap: 10.0,
        mesh_message_deliveries_threshold: 5.0,
        mesh_message_deliveries_activation: Duration::from_secs(1),
        mesh_message_deliveries_window: Duration::from_millis(100),
        topic_weight: 0.7,
        ..TopicScoreParams::default()
    };
    peer_score_params.topics.insert(topic_hash, topic_params);
    let peer_score_thresholds = PeerScoreThresholds::default();

    // build mesh with two peers
    let (mut gs, peers, _, topics) = inject_nodes1()
        .peer_no(2)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config)
        .explicit(0)
        .outbound(0)
        .scoring(Some((peer_score_params, peer_score_thresholds)))
        .create_network();

    let mut seq = 0;
    let deliver_message = |gs: &mut Behaviour, index: usize, msg: RawMessage| {
        gs.handle_received_message(msg, &peers[index]);
    };

    let mut expected_message_deliveries = 0.0;

    // messages used to test window
    let m1 = random_message(&mut seq, &topics);
    let m2 = random_message(&mut seq, &topics);

    // peer 1 delivers m1
    deliver_message(&mut gs, 1, m1.clone());

    // peer 0 delivers two message
    deliver_message(&mut gs, 0, random_message(&mut seq, &topics));
    deliver_message(&mut gs, 0, random_message(&mut seq, &topics));
    expected_message_deliveries += 2.0;

    sleep(Duration::from_millis(60));

    // peer 1 delivers m2
    deliver_message(&mut gs, 1, m2.clone());

    sleep(Duration::from_millis(70));
    // peer 0 delivers m1 and m2 only m2 gets counted
    deliver_message(&mut gs, 0, m1);
    deliver_message(&mut gs, 0, m2);
    expected_message_deliveries += 1.0;

    sleep(Duration::from_millis(900));

    // message deliveries penalties get activated, peer 0 has only delivered 3 messages and
    // therefore gets a penalty
    gs.as_peer_score_mut().refresh_scores();
    expected_message_deliveries *= 0.9; // decay

    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[0]).score,
        (5f64 - expected_message_deliveries).powi(2) * -2.0 * 0.7
    );

    // peer 0 delivers a lot of messages => message_deliveries should be capped at 10
    for _ in 0..20 {
        deliver_message(&mut gs, 0, random_message(&mut seq, &topics));
    }

    expected_message_deliveries = 10.0;

    assert_eq!(gs.as_peer_score_mut().score_report(&peers[0]).score, 0.0);

    // apply 10 decays
    for _ in 0..10 {
        gs.as_peer_score_mut().refresh_scores();
        expected_message_deliveries *= 0.9; // decay
    }

    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[0]).score,
        (5f64 - expected_message_deliveries).powi(2) * -2.0 * 0.7
    );
}

#[test]
fn test_scoring_p3b() {
    let config = ConfigBuilder::default()
        .prune_backoff(Duration::from_millis(100))
        .build()
        .unwrap();
    let mut peer_score_params = PeerScoreParams::default();
    let topic = Topic::new("test");
    let topic_hash = topic.hash();
    let topic_params = TopicScoreParams {
        time_in_mesh_weight: 0.0,             // deactivate time in mesh
        first_message_deliveries_weight: 0.0, // deactivate first time deliveries
        mesh_message_deliveries_weight: -2.0,
        mesh_message_deliveries_decay: 0.9,
        mesh_message_deliveries_cap: 10.0,
        mesh_message_deliveries_threshold: 5.0,
        mesh_message_deliveries_activation: Duration::from_secs(1),
        mesh_message_deliveries_window: Duration::from_millis(100),
        mesh_failure_penalty_weight: -3.0,
        mesh_failure_penalty_decay: 0.95,
        topic_weight: 0.7,
        ..Default::default()
    };
    peer_score_params.topics.insert(topic_hash, topic_params);
    peer_score_params.app_specific_weight = 1.0;
    let peer_score_thresholds = PeerScoreThresholds::default();

    // build mesh with one peer
    let (mut gs, peers, _, topics) = inject_nodes1()
        .peer_no(1)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config)
        .explicit(0)
        .outbound(0)
        .scoring(Some((peer_score_params, peer_score_thresholds)))
        .create_network();

    let mut seq = 0;
    let deliver_message = |gs: &mut Behaviour, index: usize, msg: RawMessage| {
        gs.handle_received_message(msg, &peers[index]);
    };

    let mut expected_message_deliveries = 0.0;

    // add some positive score
    gs.as_peer_score_mut()
        .set_application_score(&peers[0], 100.0);

    // peer 0 delivers two message
    deliver_message(&mut gs, 0, random_message(&mut seq, &topics));
    deliver_message(&mut gs, 0, random_message(&mut seq, &topics));
    expected_message_deliveries += 2.0;

    sleep(Duration::from_millis(1050));

    // activation kicks in
    gs.as_peer_score_mut().refresh_scores();
    expected_message_deliveries *= 0.9; // decay

    // prune peer
    gs.handle_prune(&peers[0], vec![(topics[0].clone(), vec![], None)]);

    // wait backoff
    sleep(Duration::from_millis(130));

    // regraft peer
    gs.handle_graft(&peers[0], topics.clone());

    // the score should now consider p3b
    let mut expected_b3 = (5f64 - expected_message_deliveries).powi(2);
    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[0]).score,
        100.0 + expected_b3 * -3.0 * 0.7
    );

    // we can also add a new p3 to the score

    // peer 0 delivers one message
    deliver_message(&mut gs, 0, random_message(&mut seq, &topics));
    expected_message_deliveries += 1.0;

    sleep(Duration::from_millis(1050));
    gs.as_peer_score_mut().refresh_scores();
    expected_message_deliveries *= 0.9; // decay
    expected_b3 *= 0.95;

    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[0]).score,
        100.0 + (expected_b3 * -3.0 + (5f64 - expected_message_deliveries).powi(2) * -2.0) * 0.7
    );
}

#[test]
fn test_scoring_p4_valid_message() {
    let config = ConfigBuilder::default()
        .validate_messages()
        .build()
        .unwrap();
    let mut peer_score_params = PeerScoreParams::default();
    let topic = Topic::new("test");
    let topic_hash = topic.hash();
    let topic_params = TopicScoreParams {
        // deactivate time in mesh
        time_in_mesh_weight: 0.0,
        // deactivate first time deliveries
        first_message_deliveries_weight: 0.0,
        // deactivate message deliveries
        mesh_message_deliveries_weight: 0.0,
        // deactivate mesh failure penalties
        mesh_failure_penalty_weight: 0.0,
        invalid_message_deliveries_weight: -2.0,
        invalid_message_deliveries_decay: 0.9,
        topic_weight: 0.7,
        ..Default::default()
    };
    peer_score_params.topics.insert(topic_hash, topic_params);
    peer_score_params.app_specific_weight = 1.0;
    let peer_score_thresholds = PeerScoreThresholds::default();

    // build mesh with two peers
    let (mut gs, peers, _, topics) = inject_nodes1()
        .peer_no(1)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config.clone())
        .explicit(0)
        .outbound(0)
        .scoring(Some((peer_score_params, peer_score_thresholds)))
        .create_network();

    let mut seq = 0;
    let deliver_message = |gs: &mut Behaviour, index: usize, msg: RawMessage| {
        gs.handle_received_message(msg, &peers[index]);
    };

    // peer 0 delivers valid message
    let m1 = random_message(&mut seq, &topics);
    deliver_message(&mut gs, 0, m1.clone());

    // Transform the inbound message
    let message1 = &gs.data_transform.inbound_transform(m1).unwrap();

    assert_eq!(gs.as_peer_score_mut().score_report(&peers[0]).score, 0.0);

    // message m1 gets validated
    gs.report_message_validation_result(
        &config.message_id(message1),
        &peers[0],
        MessageAcceptance::Accept,
    );

    assert_eq!(gs.as_peer_score_mut().score_report(&peers[0]).score, 0.0);
}

#[test]
fn test_scoring_p4_invalid_signature() {
    let config = ConfigBuilder::default()
        .validate_messages()
        .build()
        .unwrap();
    let mut peer_score_params = PeerScoreParams::default();
    let topic = Topic::new("test");
    let topic_hash = topic.hash();
    let topic_params = TopicScoreParams {
        // deactivate time in mesh
        time_in_mesh_weight: 0.0,
        // deactivate first time deliveries
        first_message_deliveries_weight: 0.0,
        // deactivate message deliveries
        mesh_message_deliveries_weight: 0.0,
        // deactivate mesh failure penalties
        mesh_failure_penalty_weight: 0.0,
        invalid_message_deliveries_weight: -2.0,
        invalid_message_deliveries_decay: 0.9,
        topic_weight: 0.7,
        ..Default::default()
    };
    peer_score_params.topics.insert(topic_hash, topic_params);
    peer_score_params.app_specific_weight = 1.0;
    let peer_score_thresholds = PeerScoreThresholds::default();

    // build mesh with one peer
    let (mut gs, peers, _, topics) = inject_nodes1()
        .peer_no(1)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config)
        .explicit(0)
        .outbound(0)
        .scoring(Some((peer_score_params, peer_score_thresholds)))
        .create_network();

    let mut seq = 0;

    // peer 0 delivers message with invalid signature
    let m = random_message(&mut seq, &topics);

    gs.on_connection_handler_event(
        peers[0],
        ConnectionId::new_unchecked(0),
        HandlerEvent::Message {
            rpc: RpcIn {
                messages: vec![],
                subscriptions: vec![],
                control_msgs: vec![],
            },
            invalid_messages: vec![(m, ValidationError::InvalidSignature)],
        },
    );

    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[0]).score,
        -2.0 * 0.7
    );
}

#[test]
fn test_scoring_p4_message_from_self() {
    let config = ConfigBuilder::default()
        .validate_messages()
        .build()
        .unwrap();
    let mut peer_score_params = PeerScoreParams::default();
    let topic = Topic::new("test");
    let topic_hash = topic.hash();
    let topic_params = TopicScoreParams {
        // deactivate time in mesh
        time_in_mesh_weight: 0.0,
        // deactivate first time deliveries
        first_message_deliveries_weight: 0.0,
        // deactivate message deliveries
        mesh_message_deliveries_weight: 0.0,
        // deactivate mesh failure penalties
        mesh_failure_penalty_weight: 0.0,
        invalid_message_deliveries_weight: -2.0,
        invalid_message_deliveries_decay: 0.9,
        topic_weight: 0.7,
        ..Default::default()
    };
    peer_score_params.topics.insert(topic_hash, topic_params);
    peer_score_params.app_specific_weight = 1.0;
    let peer_score_thresholds = PeerScoreThresholds::default();

    // build mesh with two peers
    let (mut gs, peers, _, topics) = inject_nodes1()
        .peer_no(1)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config)
        .explicit(0)
        .outbound(0)
        .scoring(Some((peer_score_params, peer_score_thresholds)))
        .create_network();

    let mut seq = 0;
    let deliver_message = |gs: &mut Behaviour, index: usize, msg: RawMessage| {
        gs.handle_received_message(msg, &peers[index]);
    };

    // peer 0 delivers invalid message from self
    let mut m = random_message(&mut seq, &topics);
    m.source = Some(*gs.publish_config.get_own_id().unwrap());

    deliver_message(&mut gs, 0, m);
    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[0]).score,
        -2.0 * 0.7
    );
}

#[test]
fn test_scoring_p4_ignored_message() {
    let config = ConfigBuilder::default()
        .validate_messages()
        .build()
        .unwrap();
    let mut peer_score_params = PeerScoreParams::default();
    let topic = Topic::new("test");
    let topic_hash = topic.hash();
    let topic_params = TopicScoreParams {
        // deactivate time in mesh
        time_in_mesh_weight: 0.0,
        // deactivate first time deliveries
        first_message_deliveries_weight: 0.0,
        // deactivate message deliveries
        mesh_message_deliveries_weight: 0.0,
        // deactivate mesh failure penalties
        mesh_failure_penalty_weight: 0.0,
        invalid_message_deliveries_weight: -2.0,
        invalid_message_deliveries_decay: 0.9,
        topic_weight: 0.7,
        ..Default::default()
    };
    peer_score_params.topics.insert(topic_hash, topic_params);
    peer_score_params.app_specific_weight = 1.0;
    let peer_score_thresholds = PeerScoreThresholds::default();

    // build mesh with two peers
    let (mut gs, peers, _, topics) = inject_nodes1()
        .peer_no(1)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config.clone())
        .explicit(0)
        .outbound(0)
        .scoring(Some((peer_score_params, peer_score_thresholds)))
        .create_network();

    let mut seq = 0;
    let deliver_message = |gs: &mut Behaviour, index: usize, msg: RawMessage| {
        gs.handle_received_message(msg, &peers[index]);
    };

    // peer 0 delivers ignored message
    let m1 = random_message(&mut seq, &topics);
    deliver_message(&mut gs, 0, m1.clone());

    assert_eq!(gs.as_peer_score_mut().score_report(&peers[0]).score, 0.0);

    // Transform the inbound message
    let message1 = &gs.data_transform.inbound_transform(m1).unwrap();

    // message m1 gets ignored
    gs.report_message_validation_result(
        &config.message_id(message1),
        &peers[0],
        MessageAcceptance::Ignore,
    );

    assert_eq!(gs.as_peer_score_mut().score_report(&peers[0]).score, 0.0);
}

#[test]
fn test_scoring_p4_application_invalidated_message() {
    let config = ConfigBuilder::default()
        .validate_messages()
        .build()
        .unwrap();
    let mut peer_score_params = PeerScoreParams::default();
    let topic = Topic::new("test");
    let topic_hash = topic.hash();
    let topic_params = TopicScoreParams {
        // deactivate time in mesh
        time_in_mesh_weight: 0.0,
        // deactivate first time deliveries
        first_message_deliveries_weight: 0.0,
        // deactivate message deliveries
        mesh_message_deliveries_weight: 0.0,
        // deactivate mesh failure penalties
        mesh_failure_penalty_weight: 0.0,
        invalid_message_deliveries_weight: -2.0,
        invalid_message_deliveries_decay: 0.9,
        topic_weight: 0.7,
        ..Default::default()
    };
    peer_score_params.topics.insert(topic_hash, topic_params);
    peer_score_params.app_specific_weight = 1.0;
    let peer_score_thresholds = PeerScoreThresholds::default();

    // build mesh with two peers
    let (mut gs, peers, _, topics) = inject_nodes1()
        .peer_no(1)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config.clone())
        .explicit(0)
        .outbound(0)
        .scoring(Some((peer_score_params, peer_score_thresholds)))
        .create_network();

    let mut seq = 0;
    let deliver_message = |gs: &mut Behaviour, index: usize, msg: RawMessage| {
        gs.handle_received_message(msg, &peers[index]);
    };

    // peer 0 delivers invalid message
    let m1 = random_message(&mut seq, &topics);
    deliver_message(&mut gs, 0, m1.clone());

    assert_eq!(gs.as_peer_score_mut().score_report(&peers[0]).score, 0.0);

    // Transform the inbound message
    let message1 = &gs.data_transform.inbound_transform(m1).unwrap();

    // message m1 gets rejected
    gs.report_message_validation_result(
        &config.message_id(message1),
        &peers[0],
        MessageAcceptance::Reject,
    );

    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[0]).score,
        -2.0 * 0.7
    );
}

#[test]
fn test_scoring_p4_application_invalid_message_from_two_peers() {
    let config = ConfigBuilder::default()
        .validate_messages()
        .build()
        .unwrap();
    let mut peer_score_params = PeerScoreParams::default();
    let topic = Topic::new("test");
    let topic_hash = topic.hash();
    let topic_params = TopicScoreParams {
        // deactivate time in mesh
        time_in_mesh_weight: 0.0,
        // deactivate first time deliveries
        first_message_deliveries_weight: 0.0,
        // deactivate message deliveries
        mesh_message_deliveries_weight: 0.0,
        // deactivate mesh failure penalties
        mesh_failure_penalty_weight: 0.0,
        invalid_message_deliveries_weight: -2.0,
        invalid_message_deliveries_decay: 0.9,
        topic_weight: 0.7,
        ..Default::default()
    };
    peer_score_params.topics.insert(topic_hash, topic_params);
    peer_score_params.app_specific_weight = 1.0;
    let peer_score_thresholds = PeerScoreThresholds::default();

    // build mesh with two peers
    let (mut gs, peers, _, topics) = inject_nodes1()
        .peer_no(2)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config.clone())
        .explicit(0)
        .outbound(0)
        .scoring(Some((peer_score_params, peer_score_thresholds)))
        .create_network();

    let mut seq = 0;
    let deliver_message = |gs: &mut Behaviour, index: usize, msg: RawMessage| {
        gs.handle_received_message(msg, &peers[index]);
    };

    // peer 0 delivers invalid message
    let m1 = random_message(&mut seq, &topics);
    deliver_message(&mut gs, 0, m1.clone());

    // Transform the inbound message
    let message1 = &gs.data_transform.inbound_transform(m1.clone()).unwrap();

    // peer 1 delivers same message
    deliver_message(&mut gs, 1, m1);

    assert_eq!(gs.as_peer_score_mut().score_report(&peers[0]).score, 0.0);
    assert_eq!(gs.as_peer_score_mut().score_report(&peers[1]).score, 0.0);

    // message m1 gets rejected
    gs.report_message_validation_result(
        &config.message_id(message1),
        &peers[0],
        MessageAcceptance::Reject,
    );

    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[0]).score,
        -2.0 * 0.7
    );
    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[1]).score,
        -2.0 * 0.7
    );
}

#[test]
fn test_scoring_p4_three_application_invalid_messages() {
    let config = ConfigBuilder::default()
        .validate_messages()
        .build()
        .unwrap();
    let mut peer_score_params = PeerScoreParams::default();
    let topic = Topic::new("test");
    let topic_hash = topic.hash();
    let topic_params = TopicScoreParams {
        // deactivate time in mesh
        time_in_mesh_weight: 0.0,
        // deactivate first time deliveries
        first_message_deliveries_weight: 0.0,
        // deactivate message deliveries
        mesh_message_deliveries_weight: 0.0,
        // deactivate mesh failure penalties
        mesh_failure_penalty_weight: 0.0,
        invalid_message_deliveries_weight: -2.0,
        invalid_message_deliveries_decay: 0.9,
        topic_weight: 0.7,
        ..Default::default()
    };
    peer_score_params.topics.insert(topic_hash, topic_params);
    peer_score_params.app_specific_weight = 1.0;
    let peer_score_thresholds = PeerScoreThresholds::default();

    // build mesh with one peer
    let (mut gs, peers, _, topics) = inject_nodes1()
        .peer_no(1)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config.clone())
        .explicit(0)
        .outbound(0)
        .scoring(Some((peer_score_params, peer_score_thresholds)))
        .create_network();

    let mut seq = 0;
    let deliver_message = |gs: &mut Behaviour, index: usize, msg: RawMessage| {
        gs.handle_received_message(msg, &peers[index]);
    };

    // peer 0 delivers two invalid message
    let m1 = random_message(&mut seq, &topics);
    let m2 = random_message(&mut seq, &topics);
    let m3 = random_message(&mut seq, &topics);
    deliver_message(&mut gs, 0, m1.clone());
    deliver_message(&mut gs, 0, m2.clone());
    deliver_message(&mut gs, 0, m3.clone());

    // Transform the inbound message
    let message1 = &gs.data_transform.inbound_transform(m1).unwrap();

    // Transform the inbound message
    let message2 = &gs.data_transform.inbound_transform(m2).unwrap();
    // Transform the inbound message
    let message3 = &gs.data_transform.inbound_transform(m3).unwrap();

    assert_eq!(gs.as_peer_score_mut().score_report(&peers[0]).score, 0.0);

    // messages gets rejected
    gs.report_message_validation_result(
        &config.message_id(message1),
        &peers[0],
        MessageAcceptance::Reject,
    );

    gs.report_message_validation_result(
        &config.message_id(message2),
        &peers[0],
        MessageAcceptance::Reject,
    );

    gs.report_message_validation_result(
        &config.message_id(message3),
        &peers[0],
        MessageAcceptance::Reject,
    );

    // number of invalid messages gets squared
    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[0]).score,
        9.0 * -2.0 * 0.7
    );
}

#[test]
fn test_scoring_p4_decay() {
    let config = ConfigBuilder::default()
        .validate_messages()
        .build()
        .unwrap();
    let mut peer_score_params = PeerScoreParams::default();
    let topic = Topic::new("test");
    let topic_hash = topic.hash();
    let topic_params = TopicScoreParams {
        // deactivate time in mesh
        time_in_mesh_weight: 0.0,
        // deactivate first time deliveries
        first_message_deliveries_weight: 0.0,
        // deactivate message deliveries
        mesh_message_deliveries_weight: 0.0,
        // deactivate mesh failure penalties
        mesh_failure_penalty_weight: 0.0,
        invalid_message_deliveries_weight: -2.0,
        invalid_message_deliveries_decay: 0.9,
        topic_weight: 0.7,
        ..Default::default()
    };
    peer_score_params.topics.insert(topic_hash, topic_params);
    peer_score_params.app_specific_weight = 1.0;
    let peer_score_thresholds = PeerScoreThresholds::default();

    // build mesh with one peer
    let (mut gs, peers, _, topics) = inject_nodes1()
        .peer_no(1)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(config.clone())
        .explicit(0)
        .outbound(0)
        .scoring(Some((peer_score_params, peer_score_thresholds)))
        .create_network();

    let mut seq = 0;
    let deliver_message = |gs: &mut Behaviour, index: usize, msg: RawMessage| {
        gs.handle_received_message(msg, &peers[index]);
    };

    // peer 0 delivers invalid message
    let m1 = random_message(&mut seq, &topics);
    deliver_message(&mut gs, 0, m1.clone());

    // Transform the inbound message
    let message1 = &gs.data_transform.inbound_transform(m1).unwrap();
    assert_eq!(gs.as_peer_score_mut().score_report(&peers[0]).score, 0.0);

    // message m1 gets rejected
    gs.report_message_validation_result(
        &config.message_id(message1),
        &peers[0],
        MessageAcceptance::Reject,
    );

    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[0]).score,
        -2.0 * 0.7
    );

    // we decay
    gs.as_peer_score_mut().refresh_scores();

    // the number of invalids gets decayed to 0.9 and then squared in the score
    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[0]).score,
        0.9 * 0.9 * -2.0 * 0.7
    );
}

#[test]
fn test_scoring_p5() {
    let peer_score_params = PeerScoreParams {
        app_specific_weight: 2.0,
        ..PeerScoreParams::default()
    };

    // build mesh with one peer
    let (mut gs, peers, _, _) = inject_nodes1()
        .peer_no(1)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .gs_config(Config::default())
        .explicit(0)
        .outbound(0)
        .scoring(Some((peer_score_params, PeerScoreThresholds::default())))
        .create_network();

    gs.set_application_score(&peers[0], 1.1);

    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[0]).score,
        1.1 * 2.0
    );
}

#[test]
fn test_scoring_p6() {
    let peer_score_params = PeerScoreParams {
        ip_colocation_factor_threshold: 5.0,
        ip_colocation_factor_weight: -2.0,
        ..Default::default()
    };

    let (mut gs, _, _, _) = inject_nodes1()
        .peer_no(0)
        .topics(vec![])
        .to_subscribe(false)
        .gs_config(Config::default())
        .explicit(0)
        .outbound(0)
        .scoring(Some((peer_score_params, PeerScoreThresholds::default())))
        .create_network();

    // create 5 peers with the same ip
    let addr = Multiaddr::from(Ipv4Addr::new(10, 1, 2, 3));
    let peers = vec![
        add_peer_with_addr(&mut gs, &[], false, false, addr.clone()).0,
        add_peer_with_addr(&mut gs, &[], false, false, addr.clone()).0,
        add_peer_with_addr(&mut gs, &[], true, false, addr.clone()).0,
        add_peer_with_addr(&mut gs, &[], true, false, addr.clone()).0,
        add_peer_with_addr(&mut gs, &[], true, true, addr.clone()).0,
    ];

    // create 4 other peers with other ip
    let addr2 = Multiaddr::from(Ipv4Addr::new(10, 1, 2, 4));
    let others = vec![
        add_peer_with_addr(&mut gs, &[], false, false, addr2.clone()).0,
        add_peer_with_addr(&mut gs, &[], false, false, addr2.clone()).0,
        add_peer_with_addr(&mut gs, &[], true, false, addr2.clone()).0,
        add_peer_with_addr(&mut gs, &[], true, false, addr2.clone()).0,
    ];

    // no penalties yet
    for peer in peers.iter().chain(others.iter()) {
        assert_eq!(gs.as_peer_score_mut().score_report(peer).score, 0.0);
    }

    // add additional connection for 3 others with addr
    for id in others.iter().take(3) {
        gs.on_swarm_event(FromSwarm::ConnectionEstablished(ConnectionEstablished {
            peer_id: *id,
            connection_id: ConnectionId::new_unchecked(0),
            endpoint: &ConnectedPoint::Dialer {
                address: addr.clone(),
                role_override: Endpoint::Dialer,
                port_use: PortUse::Reuse,
            },
            failed_addresses: &[],
            other_established: 0,
        }));
    }

    // penalties apply squared
    for peer in peers.iter().chain(others.iter().take(3)) {
        assert_eq!(gs.as_peer_score_mut().score_report(peer).score, 9.0 * -2.0);
    }
    // fourth other peer still no penalty
    assert_eq!(gs.as_peer_score_mut().score_report(&others[3]).score, 0.0);

    // add additional connection for 3 of the peers to addr2
    for peer in peers.iter().take(3) {
        gs.on_swarm_event(FromSwarm::ConnectionEstablished(ConnectionEstablished {
            peer_id: *peer,
            connection_id: ConnectionId::new_unchecked(0),
            endpoint: &ConnectedPoint::Dialer {
                address: addr2.clone(),
                role_override: Endpoint::Dialer,
                port_use: PortUse::Reuse,
            },
            failed_addresses: &[],
            other_established: 1,
        }));
    }

    // double penalties for the first three of each
    for peer in peers.iter().take(3).chain(others.iter().take(3)) {
        assert_eq!(
            gs.as_peer_score_mut().score_report(peer).score,
            (9.0 + 4.0) * -2.0
        );
    }

    // single penalties for the rest
    for peer in peers.iter().skip(3) {
        assert_eq!(gs.as_peer_score_mut().score_report(peer).score, 9.0 * -2.0);
    }
    assert_eq!(
        gs.as_peer_score_mut().score_report(&others[3]).score,
        4.0 * -2.0
    );

    // two times same ip doesn't count twice
    gs.on_swarm_event(FromSwarm::ConnectionEstablished(ConnectionEstablished {
        peer_id: peers[0],
        connection_id: ConnectionId::new_unchecked(0),
        endpoint: &ConnectedPoint::Dialer {
            address: addr,
            role_override: Endpoint::Dialer,
            port_use: PortUse::Reuse,
        },
        failed_addresses: &[],
        other_established: 2,
    }));

    // nothing changed
    // double penalties for the first three of each
    for peer in peers.iter().take(3).chain(others.iter().take(3)) {
        assert_eq!(
            gs.as_peer_score_mut().score_report(peer).score,
            (9.0 + 4.0) * -2.0
        );
    }

    // single penalties for the rest
    for peer in peers.iter().skip(3) {
        assert_eq!(gs.as_peer_score_mut().score_report(peer).score, 9.0 * -2.0);
    }
    assert_eq!(
        gs.as_peer_score_mut().score_report(&others[3]).score,
        4.0 * -2.0
    );
}

#[test]
fn test_scoring_p7_grafts_before_backoff() {
    let config = ConfigBuilder::default()
        .prune_backoff(Duration::from_millis(200))
        .graft_flood_threshold(Duration::from_millis(100))
        .build()
        .unwrap();
    let peer_score_params = PeerScoreParams {
        behaviour_penalty_weight: -2.0,
        behaviour_penalty_decay: 0.9,
        ..Default::default()
    };

    let (mut gs, peers, _receivers, topics) = inject_nodes1()
        .peer_no(2)
        .topics(vec!["test".into()])
        .to_subscribe(false)
        .gs_config(config)
        .explicit(0)
        .outbound(0)
        .scoring(Some((peer_score_params, PeerScoreThresholds::default())))
        .create_network();

    // remove peers from mesh and send prune to them => this adds a backoff for the peers
    for peer in peers.iter().take(2) {
        gs.mesh.get_mut(&topics[0]).unwrap().remove(peer);
        gs.send_graft_prune(
            HashMap::new(),
            HashMap::from([(*peer, vec![topics[0].clone()])]),
            HashSet::new(),
        );
    }

    // wait 50 millisecs
    sleep(Duration::from_millis(50));

    // first peer tries to graft
    gs.handle_graft(&peers[0], vec![topics[0].clone()]);

    // double behaviour penalty for first peer (squared)
    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[0]).score,
        4.0 * -2.0
    );

    // wait 100 millisecs
    sleep(Duration::from_millis(100));

    // second peer tries to graft
    gs.handle_graft(&peers[1], vec![topics[0].clone()]);

    // single behaviour penalty for second peer
    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[1]).score,
        1.0 * -2.0
    );

    // test decay
    gs.as_peer_score_mut().refresh_scores();

    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[0]).score,
        4.0 * 0.9 * 0.9 * -2.0
    );
    assert_eq!(
        gs.as_peer_score_mut().score_report(&peers[1]).score,
        1.0 * 0.9 * 0.9 * -2.0
    );
}

#[test]
fn test_opportunistic_grafting() {
    let config = ConfigBuilder::default()
        .mesh_n_low(3)
        .mesh_n(5)
        .mesh_n_high(7)
        .mesh_outbound_min(0) // deactivate outbound handling
        .opportunistic_graft_ticks(2)
        .opportunistic_graft_peers(2)
        .build()
        .unwrap();
    let peer_score_params = PeerScoreParams {
        app_specific_weight: 1.0,
        ..Default::default()
    };
    let thresholds = PeerScoreThresholds {
        opportunistic_graft_threshold: 2.0,
        ..Default::default()
    };

    let (mut gs, peers, _receivers, topics) = inject_nodes1()
        .peer_no(5)
        .topics(vec!["test".into()])
        .to_subscribe(false)
        .gs_config(config)
        .explicit(0)
        .outbound(0)
        .scoring(Some((peer_score_params, thresholds)))
        .create_network();

    // fill mesh with 5 peers
    for peer in &peers {
        gs.handle_graft(peer, topics.clone());
    }

    // add additional 5 peers
    let others: Vec<_> = (0..5)
        .map(|_| add_peer(&mut gs, &topics, false, false))
        .collect();

    // currently mesh equals peers
    assert_eq!(gs.mesh[&topics[0]], peers.iter().cloned().collect());

    // give others high scores (but the first two have not high enough scores)
    for (i, peer) in peers.iter().enumerate().take(5) {
        gs.set_application_score(peer, 0.0 + i as f64);
    }

    // set scores for peers in the mesh
    for (i, (peer, _receiver)) in others.iter().enumerate().take(5) {
        gs.set_application_score(peer, 0.0 + i as f64);
    }

    // this gives a median of exactly 2.0 => should not apply opportunistic grafting
    gs.heartbeat();
    gs.heartbeat();

    assert_eq!(
        gs.mesh[&topics[0]].len(),
        5,
        "should not apply opportunistic grafting"
    );

    // reduce middle score to 1.0 giving a median of 1.0
    gs.set_application_score(&peers[2], 1.0);

    // opportunistic grafting after two heartbeats

    gs.heartbeat();
    assert_eq!(
        gs.mesh[&topics[0]].len(),
        5,
        "should not apply opportunistic grafting after first tick"
    );

    gs.heartbeat();

    assert_eq!(
        gs.mesh[&topics[0]].len(),
        7,
        "opportunistic grafting should have added 2 peers"
    );

    assert!(
        gs.mesh[&topics[0]].is_superset(&peers.iter().cloned().collect()),
        "old peers are still part of the mesh"
    );

    assert!(
        gs.mesh[&topics[0]].is_disjoint(&others.iter().map(|(p, _)| p).cloned().take(2).collect()),
        "peers below or equal to median should not be added in opportunistic grafting"
    );
}

#[test]
fn test_ignore_graft_from_unknown_topic() {
    // build gossipsub without subscribing to any topics
    let (mut gs, peers, receivers, _) = inject_nodes1()
        .peer_no(1)
        .topics(vec![])
        .to_subscribe(false)
        .create_network();

    // handle an incoming graft for some topic
    gs.handle_graft(&peers[0], vec![Topic::new("test").hash()]);

    // assert that no prune got created
    let (control_msgs, _) = count_control_msgs(receivers, |_, a| matches!(a, RpcOut::Prune { .. }));
    assert_eq!(
        control_msgs, 0,
        "we should not prune after graft in unknown topic"
    );
}

#[test]
fn test_ignore_too_many_iwants_from_same_peer_for_same_message() {
    let config = Config::default();
    // build gossipsub with full mesh
    let (mut gs, _, mut receivers, topics) = inject_nodes1()
        .peer_no(config.mesh_n_high())
        .topics(vec!["test".into()])
        .to_subscribe(false)
        .create_network();

    // add another peer not in the mesh
    let (peer, receiver) = add_peer(&mut gs, &topics, false, false);
    receivers.insert(peer, receiver);

    // receive a message
    let mut seq = 0;
    let m1 = random_message(&mut seq, &topics);

    // Transform the inbound message
    let message1 = &gs.data_transform.inbound_transform(m1.clone()).unwrap();

    let id = config.message_id(message1);

    gs.handle_received_message(m1, &PeerId::random());

    // clear events
    let receivers = flush_events(&mut gs, receivers);

    // the first gossip_retransimission many iwants return the valid message, all others are
    // ignored.
    for _ in 0..(2 * config.gossip_retransimission() + 10) {
        gs.handle_iwant(&peer, vec![id.clone()]);
    }

    assert_eq!(
        receivers.into_values().fold(0, |mut fwds, c| {
            let non_priority = c.non_priority.get_ref();
            while !non_priority.is_empty() {
                if let Ok(RpcOut::Forward { .. }) = non_priority.try_recv() {
                    fwds += 1;
                }
            }
            fwds
        }),
        config.gossip_retransimission() as usize,
        "not more then gossip_retransmission many messages get sent back"
    );
}

#[test]
fn test_ignore_too_many_ihaves() {
    let config = ConfigBuilder::default()
        .max_ihave_messages(10)
        .build()
        .unwrap();
    // build gossipsub with full mesh
    let (mut gs, _, mut receivers, topics) = inject_nodes1()
        .peer_no(config.mesh_n_high())
        .topics(vec!["test".into()])
        .to_subscribe(false)
        .gs_config(config.clone())
        .create_network();

    // add another peer not in the mesh
    let (peer, receiver) = add_peer(&mut gs, &topics, false, false);
    receivers.insert(peer, receiver);

    // peer has 20 messages
    let mut seq = 0;
    let messages: Vec<_> = (0..20).map(|_| random_message(&mut seq, &topics)).collect();

    // peer sends us one ihave for each message in order
    for raw_message in &messages {
        // Transform the inbound message
        let message = &gs
            .data_transform
            .inbound_transform(raw_message.clone())
            .unwrap();

        gs.handle_ihave(
            &peer,
            vec![(topics[0].clone(), vec![config.message_id(message)])],
        );
    }

    let first_ten: HashSet<_> = messages
        .iter()
        .take(10)
        .map(|msg| gs.data_transform.inbound_transform(msg.clone()).unwrap())
        .map(|m| config.message_id(&m))
        .collect();

    // we send iwant only for the first 10 messages
    let (control_msgs, receivers) = count_control_msgs(receivers, |p, action| {
        p == &peer
            && matches!(action, RpcOut::IWant(IWant { message_ids }) if message_ids.len() == 1 && first_ten.contains(&message_ids[0]))
    });
    assert_eq!(
        control_msgs, 10,
        "exactly the first ten ihaves should be processed and one iwant for each created"
    );

    // after a heartbeat everything is forgotten
    gs.heartbeat();

    for raw_message in messages[10..].iter() {
        // Transform the inbound message
        let message = &gs
            .data_transform
            .inbound_transform(raw_message.clone())
            .unwrap();

        gs.handle_ihave(
            &peer,
            vec![(topics[0].clone(), vec![config.message_id(message)])],
        );
    }

    // we sent iwant for all 10 messages
    let (control_msgs, _) = count_control_msgs(receivers, |p, action| {
        p == &peer
            && matches!(action, RpcOut::IWant(IWant { message_ids }) if message_ids.len() == 1)
    });
    assert_eq!(control_msgs, 10, "all 20 should get sent");
}

#[test]
fn test_ignore_too_many_messages_in_ihave() {
    let config = ConfigBuilder::default()
        .max_ihave_messages(10)
        .max_ihave_length(10)
        .build()
        .unwrap();
    // build gossipsub with full mesh
    let (mut gs, _, mut receivers, topics) = inject_nodes1()
        .peer_no(config.mesh_n_high())
        .topics(vec!["test".into()])
        .to_subscribe(false)
        .gs_config(config.clone())
        .create_network();

    // add another peer not in the mesh
    let (peer, receiver) = add_peer(&mut gs, &topics, false, false);
    receivers.insert(peer, receiver);

    // peer has 30 messages
    let mut seq = 0;
    let message_ids: Vec<_> = (0..30)
        .map(|_| random_message(&mut seq, &topics))
        .map(|msg| gs.data_transform.inbound_transform(msg).unwrap())
        .map(|msg| config.message_id(&msg))
        .collect();

    // peer sends us three ihaves
    gs.handle_ihave(&peer, vec![(topics[0].clone(), message_ids[0..8].to_vec())]);
    gs.handle_ihave(
        &peer,
        vec![(topics[0].clone(), message_ids[0..12].to_vec())],
    );
    gs.handle_ihave(
        &peer,
        vec![(topics[0].clone(), message_ids[0..20].to_vec())],
    );

    let first_twelve: HashSet<_> = message_ids.iter().take(12).collect();

    // we send iwant only for the first 10 messages
    let mut sum = 0;
    let (control_msgs, receivers) = count_control_msgs(receivers, |p, rpc| match rpc {
        RpcOut::IWant(IWant { message_ids }) => {
            p == &peer && {
                assert!(first_twelve.is_superset(&message_ids.iter().collect()));
                sum += message_ids.len();
                true
            }
        }
        _ => false,
    });
    assert_eq!(
        control_msgs, 2,
        "the third ihave should get ignored and no iwant sent"
    );

    assert_eq!(sum, 10, "exactly the first ten ihaves should be processed");

    // after a heartbeat everything is forgotten
    gs.heartbeat();
    gs.handle_ihave(
        &peer,
        vec![(topics[0].clone(), message_ids[20..30].to_vec())],
    );

    // we sent 10 iwant messages ids via a IWANT rpc.
    let mut sum = 0;
    let (control_msgs, _) = count_control_msgs(receivers, |p, rpc| match rpc {
        RpcOut::IWant(IWant { message_ids }) => {
            p == &peer && {
                sum += message_ids.len();
                true
            }
        }
        _ => false,
    });
    assert_eq!(control_msgs, 1);
    assert_eq!(sum, 10, "exactly 20 iwants should get sent");
}

#[test]
fn test_limit_number_of_message_ids_inside_ihave() {
    let config = ConfigBuilder::default()
        .max_ihave_messages(10)
        .max_ihave_length(100)
        .build()
        .unwrap();
    // build gossipsub with full mesh
    let (mut gs, peers, mut receivers, topics) = inject_nodes1()
        .peer_no(config.mesh_n_high())
        .topics(vec!["test".into()])
        .to_subscribe(false)
        .gs_config(config)
        .create_network();

    // graft to all peers to really fill the mesh with all the peers
    for peer in peers {
        gs.handle_graft(&peer, topics.clone());
    }

    // add two other peers not in the mesh
    let (p1, receiver1) = add_peer(&mut gs, &topics, false, false);
    receivers.insert(p1, receiver1);
    let (p2, receiver2) = add_peer(&mut gs, &topics, false, false);
    receivers.insert(p2, receiver2);

    // receive 200 messages from another peer
    let mut seq = 0;
    for _ in 0..200 {
        gs.handle_received_message(random_message(&mut seq, &topics), &PeerId::random());
    }

    // emit gossip
    gs.emit_gossip();

    // both peers should have gotten 100 random ihave messages, to assert the randomness, we
    // assert that both have not gotten the same set of messages, but have an intersection
    // (which is the case with very high probability, the probabiltity of failure is < 10^-58).

    let mut ihaves1 = HashSet::new();
    let mut ihaves2 = HashSet::new();

    let (control_msgs, _) = count_control_msgs(receivers, |p, action| match action {
        RpcOut::IHave(IHave { message_ids, .. }) => {
            if p == &p1 {
                ihaves1 = message_ids.iter().cloned().collect();
                true
            } else if p == &p2 {
                ihaves2 = message_ids.iter().cloned().collect();
                true
            } else {
                false
            }
        }
        _ => false,
    });
    assert_eq!(
        control_msgs, 2,
        "should have emitted one ihave to p1 and one to p2"
    );

    assert_eq!(
        ihaves1.len(),
        100,
        "should have sent 100 message ids in ihave to p1"
    );
    assert_eq!(
        ihaves2.len(),
        100,
        "should have sent 100 message ids in ihave to p2"
    );
    assert!(
        ihaves1 != ihaves2,
        "should have sent different random messages to p1 and p2 \
        (this may fail with a probability < 10^-58"
    );
    assert!(
        ihaves1.intersection(&ihaves2).count() > 0,
        "should have sent random messages with some common messages to p1 and p2 \
            (this may fail with a probability < 10^-58"
    );
}

#[test]
fn test_iwant_penalties() {
    // use tracing_subscriber::EnvFilter;
    // let _ = tracing_subscriber::fmt()
    // .with_env_filter(EnvFilter::from_default_env())
    // .try_init();
    let config = ConfigBuilder::default()
        .iwant_followup_time(Duration::from_secs(4))
        .build()
        .unwrap();
    let peer_score_params = PeerScoreParams {
        behaviour_penalty_weight: -1.0,
        ..Default::default()
    };

    // fill the mesh
    let (mut gs, peers, _, topics) = inject_nodes1()
        .peer_no(2)
        .topics(vec!["test".into()])
        .to_subscribe(false)
        .gs_config(config.clone())
        .explicit(0)
        .outbound(0)
        .scoring(Some((peer_score_params, PeerScoreThresholds::default())))
        .create_network();

    // graft to all peers to really fill the mesh with all the peers
    for peer in peers {
        gs.handle_graft(&peer, topics.clone());
    }

    // add 100 more peers
    let other_peers: Vec<_> = (0..100)
        .map(|_| add_peer(&mut gs, &topics, false, false))
        .collect();

    // each peer sends us an ihave containing each two message ids
    let mut first_messages = Vec::new();
    let mut second_messages = Vec::new();
    let mut seq = 0;
    for (peer, _receiver) in &other_peers {
        let msg1 = random_message(&mut seq, &topics);
        let msg2 = random_message(&mut seq, &topics);

        // Decompress the raw message and calculate the message id.
        // Transform the inbound message
        let message1 = &gs.data_transform.inbound_transform(msg1.clone()).unwrap();

        // Transform the inbound message
        let message2 = &gs.data_transform.inbound_transform(msg2.clone()).unwrap();

        first_messages.push(msg1.clone());
        second_messages.push(msg2.clone());
        gs.handle_ihave(
            peer,
            vec![(
                topics[0].clone(),
                vec![config.message_id(message1), config.message_id(message2)],
            )],
        );
    }

    // the peers send us all the first message ids in time
    for (index, (peer, _receiver)) in other_peers.iter().enumerate() {
        gs.handle_received_message(first_messages[index].clone(), peer);
    }

    // now we do a heartbeat no penalization should have been applied yet
    gs.heartbeat();

    for (peer, _receiver) in &other_peers {
        assert_eq!(gs.as_peer_score_mut().score_report(peer).score, 0.0);
    }

    // receive the first twenty of the other peers then send their response
    for (index, (peer, _receiver)) in other_peers.iter().enumerate().take(20) {
        gs.handle_received_message(second_messages[index].clone(), peer);
    }

    // sleep for the promise duration
    sleep(Duration::from_secs(4));

    // now we do a heartbeat to apply penalization
    gs.heartbeat();

    // now we get the second messages from the last 80 peers.
    for (index, (peer, _receiver)) in other_peers.iter().enumerate() {
        if index > 19 {
            gs.handle_received_message(second_messages[index].clone(), peer);
        }
    }

    // no further penalizations should get applied
    gs.heartbeat();

    // Only the last 80 peers should be penalized for not responding in time
    let mut not_penalized = 0;
    let mut single_penalized = 0;
    let mut double_penalized = 0;

    for (i, (peer, _receiver)) in other_peers.iter().enumerate() {
        let score = gs.as_peer_score_mut().score_report(peer).score;
        if score == 0.0 {
            not_penalized += 1;
        } else if score == -1.0 {
            assert!(i > 9);
            single_penalized += 1;
        } else if score == -4.0 {
            assert!(i > 9);
            double_penalized += 1
        } else {
            println!("{peer}");
            println!("{score}");
            panic!("Invalid score of peer");
        }
    }

    assert_eq!(not_penalized, 20);
    assert_eq!(single_penalized, 80);
    assert_eq!(double_penalized, 0);
}

#[test]
fn test_publish_to_floodsub_peers_without_flood_publish() {
    let config = ConfigBuilder::default()
        .flood_publish(false)
        .build()
        .unwrap();
    let (mut gs, _, mut receivers, topics) = inject_nodes1()
        .peer_no(config.mesh_n_low() - 1)
        .topics(vec!["test".into()])
        .to_subscribe(false)
        .gs_config(config)
        .create_network();

    // add two floodsub peer, one explicit, one implicit
    let (p1, receiver1) = add_peer_with_addr_and_kind(
        &mut gs,
        &topics,
        false,
        false,
        Multiaddr::empty(),
        Some(PeerKind::Floodsub),
    );
    receivers.insert(p1, receiver1);

    let (p2, receiver2) =
        add_peer_with_addr_and_kind(&mut gs, &topics, false, false, Multiaddr::empty(), None);
    receivers.insert(p2, receiver2);

    // p1 and p2 are not in the mesh
    assert!(!gs.mesh[&topics[0]].contains(&p1) && !gs.mesh[&topics[0]].contains(&p2));

    // publish a message
    let publish_data = vec![0; 42];
    gs.publish(Topic::new("test"), publish_data).unwrap();

    // Collect publish messages to floodsub peers
    let publishes = receivers
        .into_iter()
        .fold(0, |mut collected_publish, (peer_id, c)| {
            let priority = c.priority.get_ref();
            while !priority.is_empty() {
                if matches!(priority.try_recv(),
            Ok(RpcOut::Publish{..}) if peer_id == p1 || peer_id == p2)
                {
                    collected_publish += 1;
                }
            }
            collected_publish
        });

    assert_eq!(
        publishes, 2,
        "Should send a publish message to all floodsub peers"
    );
}

#[test]
fn test_do_not_use_floodsub_in_fanout() {
    let config = ConfigBuilder::default()
        .flood_publish(false)
        .build()
        .unwrap();
    let (mut gs, _, mut receivers, _) = inject_nodes1()
        .peer_no(config.mesh_n_low() - 1)
        .topics(Vec::new())
        .to_subscribe(false)
        .gs_config(config)
        .create_network();

    let topic = Topic::new("test");
    let topics = vec![topic.hash()];

    // add two floodsub peer, one explicit, one implicit
    let (p1, receiver1) = add_peer_with_addr_and_kind(
        &mut gs,
        &topics,
        false,
        false,
        Multiaddr::empty(),
        Some(PeerKind::Floodsub),
    );

    receivers.insert(p1, receiver1);
    let (p2, receiver2) =
        add_peer_with_addr_and_kind(&mut gs, &topics, false, false, Multiaddr::empty(), None);

    receivers.insert(p2, receiver2);
    // publish a message
    let publish_data = vec![0; 42];
    gs.publish(Topic::new("test"), publish_data).unwrap();

    // Collect publish messages to floodsub peers
    let publishes = receivers
        .into_iter()
        .fold(0, |mut collected_publish, (peer_id, c)| {
            let priority = c.priority.get_ref();
            while !priority.is_empty() {
                if matches!(priority.try_recv(),
            Ok(RpcOut::Publish{..}) if peer_id == p1 || peer_id == p2)
                {
                    collected_publish += 1;
                }
            }
            collected_publish
        });

    assert_eq!(
        publishes, 2,
        "Should send a publish message to all floodsub peers"
    );

    assert!(
        !gs.fanout[&topics[0]].contains(&p1) && !gs.fanout[&topics[0]].contains(&p2),
        "Floodsub peers are not allowed in fanout"
    );
}

#[test]
fn test_dont_add_floodsub_peers_to_mesh_on_join() {
    let (mut gs, _, _, _) = inject_nodes1()
        .peer_no(0)
        .topics(Vec::new())
        .to_subscribe(false)
        .create_network();

    let topic = Topic::new("test");
    let topics = vec![topic.hash()];

    // add two floodsub peer, one explicit, one implicit
    let _p1 = add_peer_with_addr_and_kind(
        &mut gs,
        &topics,
        false,
        false,
        Multiaddr::empty(),
        Some(PeerKind::Floodsub),
    );
    let _p2 = add_peer_with_addr_and_kind(&mut gs, &topics, false, false, Multiaddr::empty(), None);

    gs.join(&topics[0]);

    assert!(
        gs.mesh[&topics[0]].is_empty(),
        "Floodsub peers should not get added to mesh"
    );
}

#[test]
fn test_dont_send_px_to_old_gossipsub_peers() {
    let (mut gs, _, receivers, topics) = inject_nodes1()
        .peer_no(0)
        .topics(vec!["test".into()])
        .to_subscribe(false)
        .create_network();

    // add an old gossipsub peer
    let (p1, _receiver1) = add_peer_with_addr_and_kind(
        &mut gs,
        &topics,
        false,
        false,
        Multiaddr::empty(),
        Some(PeerKind::Gossipsub),
    );

    // prune the peer
    gs.send_graft_prune(
        HashMap::new(),
        vec![(p1, topics.clone())].into_iter().collect(),
        HashSet::new(),
    );

    // check that prune does not contain px
    let (control_msgs, _) = count_control_msgs(receivers, |_, m| match m {
        RpcOut::Prune(Prune { peers: px, .. }) => !px.is_empty(),
        _ => false,
    });
    assert_eq!(control_msgs, 0, "Should not send px to floodsub peers");
}

#[test]
fn test_dont_send_floodsub_peers_in_px() {
    // build mesh with one peer
    let (mut gs, peers, receivers, topics) = inject_nodes1()
        .peer_no(1)
        .topics(vec!["test".into()])
        .to_subscribe(true)
        .create_network();

    // add two floodsub peers
    let _p1 = add_peer_with_addr_and_kind(
        &mut gs,
        &topics,
        false,
        false,
        Multiaddr::empty(),
        Some(PeerKind::Floodsub),
    );
    let _p2 = add_peer_with_addr_and_kind(&mut gs, &topics, false, false, Multiaddr::empty(), None);

    // prune only mesh node
    gs.send_graft_prune(
        HashMap::new(),
        vec![(peers[0], topics.clone())].into_iter().collect(),
        HashSet::new(),
    );

    // check that px in prune message is empty
    let (control_msgs, _) = count_control_msgs(receivers, |_, m| match m {
        RpcOut::Prune(Prune { peers: px, .. }) => !px.is_empty(),
        _ => false,
    });
    assert_eq!(control_msgs, 0, "Should not include floodsub peers in px");
}

#[test]
fn test_dont_add_floodsub_peers_to_mesh_in_heartbeat() {
    let (mut gs, _, _, topics) = inject_nodes1()
        .peer_no(0)
        .topics(vec!["test".into()])
        .to_subscribe(false)
        .create_network();

    // add two floodsub peer, one explicit, one implicit
    let _p1 = add_peer_with_addr_and_kind(
        &mut gs,
        &topics,
        true,
        false,
        Multiaddr::empty(),
        Some(PeerKind::Floodsub),
    );
    let _p2 = add_peer_with_addr_and_kind(&mut gs, &topics, true, false, Multiaddr::empty(), None);

    gs.heartbeat();

    assert!(
        gs.mesh[&topics[0]].is_empty(),
        "Floodsub peers should not get added to mesh"
    );
}

// Some very basic test of public api methods.
#[test]
fn test_public_api() {
    let (gs, peers, _, topic_hashes) = inject_nodes1()
        .peer_no(4)
        .topics(vec![String::from("topic1")])
        .to_subscribe(true)
        .create_network();
    let peers = peers.into_iter().collect::<BTreeSet<_>>();

    assert_eq!(
        gs.topics().cloned().collect::<Vec<_>>(),
        topic_hashes,
        "Expected topics to match registered topic."
    );

    assert_eq!(
        gs.mesh_peers(&TopicHash::from_raw("topic1"))
            .cloned()
            .collect::<BTreeSet<_>>(),
        peers,
        "Expected peers for a registered topic to contain all peers."
    );

    assert_eq!(
        gs.all_mesh_peers().cloned().collect::<BTreeSet<_>>(),
        peers,
        "Expected all_peers to contain all peers."
    );
}

#[test]
fn test_subscribe_to_invalid_topic() {
    let t1 = Topic::new("t1");
    let t2 = Topic::new("t2");
    let (mut gs, _, _, _) = inject_nodes::<IdentityTransform, _>()
        .subscription_filter(WhitelistSubscriptionFilter(
            vec![t1.hash()].into_iter().collect(),
        ))
        .to_subscribe(false)
        .create_network();

    assert!(gs.subscribe(&t1).is_ok());
    assert!(gs.subscribe(&t2).is_err());
}

#[test]
fn test_subscribe_and_graft_with_negative_score() {
    // simulate a communication between two gossipsub instances
    let (mut gs1, _, _, topic_hashes) = inject_nodes1()
        .topics(vec!["test".into()])
        .scoring(Some((
            PeerScoreParams::default(),
            PeerScoreThresholds::default(),
        )))
        .create_network();

    let (mut gs2, _, receivers, _) = inject_nodes1().create_network();

    let connection_id = ConnectionId::new_unchecked(0);

    let topic = Topic::new("test");

    let (p2, _receiver1) = add_peer(&mut gs1, &Vec::new(), true, false);
    let (p1, _receiver2) = add_peer(&mut gs2, &topic_hashes, false, false);

    // add penalty to peer p2
    gs1.as_peer_score_mut().add_penalty(&p2, 1);

    let original_score = gs1.as_peer_score_mut().score_report(&p2).score;

    // subscribe to topic in gs2
    gs2.subscribe(&topic).unwrap();

    let forward_messages_to_p1 = |gs1: &mut Behaviour<_, _>,
                                  p1: PeerId,
                                  p2: PeerId,
                                  connection_id: ConnectionId,
                                  receivers: HashMap<PeerId, Receiver>|
     -> HashMap<PeerId, Receiver> {
        let new_receivers = HashMap::new();
        for (peer_id, receiver) in receivers.into_iter() {
            let non_priority = receiver.non_priority.get_ref();
            match non_priority.try_recv() {
                Ok(rpc) if peer_id == p1 => {
                    gs1.on_connection_handler_event(
                        p2,
                        connection_id,
                        HandlerEvent::Message {
                            rpc: proto_to_message(&rpc.into_protobuf()),
                            invalid_messages: vec![],
                        },
                    );
                }
                _ => {}
            }
        }
        new_receivers
    };

    // forward the subscribe message
    let receivers = forward_messages_to_p1(&mut gs1, p1, p2, connection_id, receivers);

    // heartbeats on both
    gs1.heartbeat();
    gs2.heartbeat();

    // forward messages again
    forward_messages_to_p1(&mut gs1, p1, p2, connection_id, receivers);

    // nobody got penalized
    assert!(gs1.as_peer_score_mut().score_report(&p2).score >= original_score);
}

#[test]
/// Test nodes that send grafts without subscriptions.
fn test_graft_without_subscribe() {
    // The node should:
    // - Create an empty vector in mesh[topic]
    // - Send subscription request to all peers
    // - run JOIN(topic)

    let topic = String::from("test_subscribe");
    let subscribe_topic = vec![topic.clone()];
    let subscribe_topic_hash = vec![Topic::new(topic.clone()).hash()];
    let (mut gs, peers, _, topic_hashes) = inject_nodes1()
        .peer_no(1)
        .topics(subscribe_topic)
        .to_subscribe(false)
        .create_network();

    assert!(
        gs.mesh.contains_key(&topic_hashes[0]),
        "Subscribe should add a new entry to the mesh[topic] hashmap"
    );

    // The node sends a graft for the subscribe topic.
    gs.handle_graft(&peers[0], subscribe_topic_hash);

    // The node disconnects
    disconnect_peer(&mut gs, &peers[0]);

    // We unsubscribe from the topic.
    let _ = gs.unsubscribe(&Topic::new(topic));
}

/// Test that a node sends IDONTWANT messages to the mesh peers
/// that run Gossipsub v1.2.
#[test]
fn sends_idontwant() {
    let (mut gs, peers, receivers, topic_hashes) = inject_nodes1()
        .peer_no(5)
        .topics(vec![String::from("topic1")])
        .to_subscribe(true)
        .gs_config(Config::default())
        .explicit(1)
        .peer_kind(PeerKind::Gossipsubv1_2)
        .create_network();

    let local_id = PeerId::random();

    let message = RawMessage {
        source: Some(peers[1]),
        data: vec![12u8; 1024],
        sequence_number: Some(0),
        topic: topic_hashes[0].clone(),
        signature: None,
        key: None,
        validated: true,
    };
    gs.handle_received_message(message.clone(), &local_id);
    assert_eq!(
        receivers
            .into_iter()
            .fold(0, |mut idontwants, (peer_id, c)| {
                let non_priority = c.non_priority.get_ref();
                while !non_priority.is_empty() {
                    if let Ok(RpcOut::IDontWant(_)) = non_priority.try_recv() {
                        assert_ne!(peer_id, peers[1]);
                        idontwants += 1;
                    }
                }
                idontwants
            }),
        3,
        "IDONTWANT was not sent"
    );
}

#[test]
fn doesnt_sends_idontwant_for_lower_message_size() {
    let (mut gs, peers, receivers, topic_hashes) = inject_nodes1()
        .peer_no(5)
        .topics(vec![String::from("topic1")])
        .to_subscribe(true)
        .gs_config(Config::default())
        .explicit(1)
        .peer_kind(PeerKind::Gossipsubv1_2)
        .create_network();

    let local_id = PeerId::random();

    let message = RawMessage {
        source: Some(peers[1]),
        data: vec![12],
        sequence_number: Some(0),
        topic: topic_hashes[0].clone(),
        signature: None,
        key: None,
        validated: true,
    };

    gs.handle_received_message(message.clone(), &local_id);
    assert_eq!(
        receivers
            .into_iter()
            .fold(0, |mut idontwants, (peer_id, c)| {
                let non_priority = c.non_priority.get_ref();
                while !non_priority.is_empty() {
                    if let Ok(RpcOut::IDontWant(_)) = non_priority.try_recv() {
                        assert_ne!(peer_id, peers[1]);
                        idontwants += 1;
                    }
                }
                idontwants
            }),
        0,
        "IDONTWANT was sent"
    );
}

/// Test that a node doesn't send IDONTWANT messages to the mesh peers
/// that don't run Gossipsub v1.2.
#[test]
fn doesnt_send_idontwant() {
    let (mut gs, peers, receivers, topic_hashes) = inject_nodes1()
        .peer_no(5)
        .topics(vec![String::from("topic1")])
        .to_subscribe(true)
        .gs_config(Config::default())
        .explicit(1)
        .peer_kind(PeerKind::Gossipsubv1_1)
        .create_network();

    let local_id = PeerId::random();

    let message = RawMessage {
        source: Some(peers[1]),
        data: vec![12],
        sequence_number: Some(0),
        topic: topic_hashes[0].clone(),
        signature: None,
        key: None,
        validated: true,
    };
    gs.handle_received_message(message.clone(), &local_id);
    assert_eq!(
        receivers
            .into_iter()
            .fold(0, |mut idontwants, (peer_id, c)| {
                let non_priority = c.non_priority.get_ref();
                while !non_priority.is_empty() {
                    if matches!(non_priority.try_recv(), Ok(RpcOut::IDontWant(_)) if peer_id != peers[1]) {
                        idontwants += 1;
                    }
                }
                idontwants
            }),
        0,
        "IDONTWANT were sent"
    );
}

/// Test that a node doesn't forward a messages to the mesh peers
/// that sent IDONTWANT.
#[test]
fn doesnt_forward_idontwant() {
    let (mut gs, peers, receivers, topic_hashes) = inject_nodes1()
        .peer_no(4)
        .topics(vec![String::from("topic1")])
        .to_subscribe(true)
        .gs_config(Config::default())
        .explicit(1)
        .peer_kind(PeerKind::Gossipsubv1_2)
        .create_network();

    let local_id = PeerId::random();

    let raw_message = RawMessage {
        source: Some(peers[1]),
        data: vec![12],
        sequence_number: Some(0),
        topic: topic_hashes[0].clone(),
        signature: None,
        key: None,
        validated: true,
    };
    let message = gs
        .data_transform
        .inbound_transform(raw_message.clone())
        .unwrap();
    let message_id = gs.config.message_id(&message);
    let peer = gs.connected_peers.get_mut(&peers[2]).unwrap();
    peer.dont_send.insert(message_id, Instant::now());

    gs.handle_received_message(raw_message.clone(), &local_id);
    assert_eq!(
        receivers.into_iter().fold(0, |mut fwds, (peer_id, c)| {
            let non_priority = c.non_priority.get_ref();
            while !non_priority.is_empty() {
                if let Ok(RpcOut::Forward { .. }) = non_priority.try_recv() {
                    assert_ne!(peer_id, peers[2]);
                    fwds += 1;
                }
            }
            fwds
        }),
        2,
        "IDONTWANT was not sent"
    );
}

/// Test that a node parses an
/// IDONTWANT message to the respective peer.
#[test]
fn parses_idontwant() {
    let (mut gs, peers, _receivers, _topic_hashes) = inject_nodes1()
        .peer_no(2)
        .topics(vec![String::from("topic1")])
        .to_subscribe(true)
        .gs_config(Config::default())
        .explicit(1)
        .peer_kind(PeerKind::Gossipsubv1_2)
        .create_network();

    let message_id = MessageId::new(&[0, 1, 2, 3]);
    let rpc = RpcIn {
        messages: vec![],
        subscriptions: vec![],
        control_msgs: vec![ControlAction::IDontWant(IDontWant {
            message_ids: vec![message_id.clone()],
        })],
    };
    gs.on_connection_handler_event(
        peers[1],
        ConnectionId::new_unchecked(0),
        HandlerEvent::Message {
            rpc,
            invalid_messages: vec![],
        },
    );
    let peer = gs.connected_peers.get_mut(&peers[1]).unwrap();
    assert!(peer.dont_send.get(&message_id).is_some());
}

/// Test that a node clears stale IDONTWANT messages.
#[test]
fn clear_stale_idontwant() {
    let (mut gs, peers, _receivers, _topic_hashes) = inject_nodes1()
        .peer_no(4)
        .topics(vec![String::from("topic1")])
        .to_subscribe(true)
        .gs_config(Config::default())
        .explicit(1)
        .peer_kind(PeerKind::Gossipsubv1_2)
        .create_network();

    let peer = gs.connected_peers.get_mut(&peers[2]).unwrap();
    peer.dont_send
        .insert(MessageId::new(&[1, 2, 3, 4]), Instant::now());
    std::thread::sleep(Duration::from_secs(3));
    gs.heartbeat();
    let peer = gs.connected_peers.get_mut(&peers[2]).unwrap();
    assert!(peer.dont_send.is_empty());
}

#[test]
fn test_all_queues_full() {
    let gs_config = ConfigBuilder::default()
        .validation_mode(ValidationMode::Permissive)
        .build()
        .unwrap();

    let mut gs: Behaviour = Behaviour::new(MessageAuthenticity::RandomAuthor, gs_config).unwrap();

    let topic_hash = Topic::new("Test").hash();
    let mut peers = vec![];
    let mut topics = BTreeSet::new();
    topics.insert(topic_hash.clone());

    let peer_id = PeerId::random();
    peers.push(peer_id);
    gs.connected_peers.insert(
        peer_id,
        PeerDetails {
            kind: PeerKind::Gossipsubv1_1,
            connections: vec![ConnectionId::new_unchecked(0)],
            outbound: false,
            topics: topics.clone(),
            sender: Sender::new(2),
            dont_send: LinkedHashMap::new(),
        },
    );

    let publish_data = vec![0; 42];
    gs.publish(topic_hash.clone(), publish_data.clone())
        .unwrap();
    let publish_data = vec![2; 59];
    let err = gs.publish(topic_hash, publish_data).unwrap_err();
    assert!(matches!(err, PublishError::AllQueuesFull(f) if f == 1));
}

#[test]
fn test_slow_peer_returns_failed_publish() {
    let gs_config = ConfigBuilder::default()
        .validation_mode(ValidationMode::Permissive)
        .build()
        .unwrap();

    let mut gs: Behaviour = Behaviour::new(MessageAuthenticity::RandomAuthor, gs_config).unwrap();

    let topic_hash = Topic::new("Test").hash();
    let mut peers = vec![];
    let mut topics = BTreeSet::new();
    topics.insert(topic_hash.clone());

    let slow_peer_id = PeerId::random();
    peers.push(slow_peer_id);
    gs.connected_peers.insert(
        slow_peer_id,
        PeerDetails {
            kind: PeerKind::Gossipsubv1_1,
            connections: vec![ConnectionId::new_unchecked(0)],
            outbound: false,
            topics: topics.clone(),
            sender: Sender::new(2),
            dont_send: LinkedHashMap::new(),
        },
    );
    let peer_id = PeerId::random();
    peers.push(peer_id);
    gs.connected_peers.insert(
        peer_id,
        PeerDetails {
            kind: PeerKind::Gossipsubv1_1,
            connections: vec![ConnectionId::new_unchecked(0)],
            outbound: false,
            topics: topics.clone(),
            sender: Sender::new(gs.config.connection_handler_queue_len()),
            dont_send: LinkedHashMap::new(),
        },
    );

    let publish_data = vec![0; 42];
    gs.publish(topic_hash.clone(), publish_data.clone())
        .unwrap();
    let publish_data = vec![2; 59];
    gs.publish(topic_hash.clone(), publish_data).unwrap();
    gs.heartbeat();

    gs.heartbeat();

    let slow_peer_failed_messages = match gs.events.pop_front().unwrap() {
        ToSwarm::GenerateEvent(Event::SlowPeer {
            peer_id,
            failed_messages,
        }) if peer_id == slow_peer_id => failed_messages,
        _ => panic!("invalid event"),
    };

    let failed_messages = FailedMessages {
        publish: 1,
        forward: 0,
        priority: 1,
        non_priority: 0,
        timeout: 0,
    };

    assert_eq!(slow_peer_failed_messages.priority, failed_messages.priority);
    assert_eq!(
        slow_peer_failed_messages.non_priority,
        failed_messages.non_priority
    );
    assert_eq!(slow_peer_failed_messages.publish, failed_messages.publish);
    assert_eq!(slow_peer_failed_messages.forward, failed_messages.forward);
}

#[test]
fn test_slow_peer_returns_failed_ihave_handling() {
    let gs_config = ConfigBuilder::default()
        .validation_mode(ValidationMode::Permissive)
        .build()
        .unwrap();

    let mut gs: Behaviour = Behaviour::new(MessageAuthenticity::RandomAuthor, gs_config).unwrap();

    let topic_hash = Topic::new("Test").hash();
    let mut peers = vec![];
    let mut topics = BTreeSet::new();
    topics.insert(topic_hash.clone());

    let slow_peer_id = PeerId::random();
    gs.connected_peers.insert(
        slow_peer_id,
        PeerDetails {
            kind: PeerKind::Gossipsubv1_1,
            connections: vec![ConnectionId::new_unchecked(0)],
            outbound: false,
            topics: topics.clone(),
            sender: Sender::new(2),
            dont_send: LinkedHashMap::new(),
        },
    );
    peers.push(slow_peer_id);
    let mesh = gs.mesh.entry(topic_hash.clone()).or_default();
    mesh.insert(slow_peer_id);

    let peer_id = PeerId::random();
    peers.push(peer_id);
    gs.connected_peers.insert(
        peer_id,
        PeerDetails {
            kind: PeerKind::Gossipsubv1_1,
            connections: vec![ConnectionId::new_unchecked(0)],
            outbound: false,
            topics: topics.clone(),
            sender: Sender::new(gs.config.connection_handler_queue_len()),
            dont_send: LinkedHashMap::new(),
        },
    );

    // First message.
    let publish_data = vec![1; 59];
    let transformed = gs
        .data_transform
        .outbound_transform(&topic_hash, publish_data.clone())
        .unwrap();
    let raw_message = gs
        .build_raw_message(topic_hash.clone(), transformed)
        .unwrap();
    let msg_id = gs.config.message_id(&Message {
        source: raw_message.source,
        data: publish_data,
        sequence_number: raw_message.sequence_number,
        topic: raw_message.topic.clone(),
    });

    gs.handle_ihave(
        &slow_peer_id,
        vec![(topic_hash.clone(), vec![msg_id.clone()])],
    );

    // Second message.
    let publish_data = vec![2; 59];
    let transformed = gs
        .data_transform
        .outbound_transform(&topic_hash, publish_data.clone())
        .unwrap();
    let raw_message = gs
        .build_raw_message(topic_hash.clone(), transformed)
        .unwrap();
    let msg_id = gs.config.message_id(&Message {
        source: raw_message.source,
        data: publish_data,
        sequence_number: raw_message.sequence_number,
        topic: raw_message.topic.clone(),
    });
    gs.handle_ihave(&slow_peer_id, vec![(topic_hash, vec![msg_id.clone()])]);

    gs.heartbeat();

    let slow_peer_failed_messages = gs
        .events
        .into_iter()
        .find_map(|e| match e {
            ToSwarm::GenerateEvent(Event::SlowPeer {
                peer_id,
                failed_messages,
            }) if peer_id == slow_peer_id => Some(failed_messages),
            _ => None,
        })
        .unwrap();

    let failed_messages = FailedMessages {
        publish: 0,
        forward: 0,
        priority: 0,
        non_priority: 1,
        timeout: 0,
    };

    assert_eq!(slow_peer_failed_messages.priority, failed_messages.priority);
    assert_eq!(
        slow_peer_failed_messages.non_priority,
        failed_messages.non_priority
    );
    assert_eq!(slow_peer_failed_messages.publish, failed_messages.publish);
    assert_eq!(slow_peer_failed_messages.forward, failed_messages.forward);
}

#[test]
fn test_slow_peer_returns_failed_iwant_handling() {
    let gs_config = ConfigBuilder::default()
        .validation_mode(ValidationMode::Permissive)
        .build()
        .unwrap();

    let mut gs: Behaviour = Behaviour::new(MessageAuthenticity::RandomAuthor, gs_config).unwrap();

    let topic_hash = Topic::new("Test").hash();
    let mut peers = vec![];
    let mut topics = BTreeSet::new();
    topics.insert(topic_hash.clone());

    let slow_peer_id = PeerId::random();
    peers.push(slow_peer_id);
    gs.connected_peers.insert(
        slow_peer_id,
        PeerDetails {
            kind: PeerKind::Gossipsubv1_1,
            connections: vec![ConnectionId::new_unchecked(0)],
            outbound: false,
            topics: topics.clone(),
            sender: Sender::new(2),
            dont_send: LinkedHashMap::new(),
        },
    );
    peers.push(slow_peer_id);
    let mesh = gs.mesh.entry(topic_hash.clone()).or_default();
    mesh.insert(slow_peer_id);

    let peer_id = PeerId::random();
    peers.push(peer_id);
    gs.connected_peers.insert(
        peer_id,
        PeerDetails {
            kind: PeerKind::Gossipsubv1_1,
            connections: vec![ConnectionId::new_unchecked(0)],
            outbound: false,
            topics: topics.clone(),
            sender: Sender::new(gs.config.connection_handler_queue_len()),
            dont_send: LinkedHashMap::new(),
        },
    );

    let publish_data = vec![1; 59];
    let transformed = gs
        .data_transform
        .outbound_transform(&topic_hash, publish_data.clone())
        .unwrap();
    let raw_message = gs
        .build_raw_message(topic_hash.clone(), transformed)
        .unwrap();
    let msg_id = gs.config.message_id(&Message {
        source: raw_message.source,
        data: publish_data,
        sequence_number: raw_message.sequence_number,
        topic: raw_message.topic.clone(),
    });

    gs.mcache.put(&msg_id, raw_message);
    gs.handle_iwant(&slow_peer_id, vec![msg_id.clone(), msg_id]);

    gs.heartbeat();

    let slow_peer_failed_messages = gs
        .events
        .into_iter()
        .find_map(|e| match e {
            ToSwarm::GenerateEvent(Event::SlowPeer {
                peer_id,
                failed_messages,
            }) if peer_id == slow_peer_id => Some(failed_messages),
            _ => None,
        })
        .unwrap();

    let failed_messages = FailedMessages {
        publish: 0,
        forward: 1,
        priority: 0,
        non_priority: 1,
        timeout: 0,
    };

    assert_eq!(slow_peer_failed_messages.priority, failed_messages.priority);
    assert_eq!(
        slow_peer_failed_messages.non_priority,
        failed_messages.non_priority
    );
    assert_eq!(slow_peer_failed_messages.publish, failed_messages.publish);
    assert_eq!(slow_peer_failed_messages.forward, failed_messages.forward);
}

#[test]
fn test_slow_peer_returns_failed_forward() {
    let gs_config = ConfigBuilder::default()
        .validation_mode(ValidationMode::Permissive)
        .build()
        .unwrap();

    let mut gs: Behaviour = Behaviour::new(MessageAuthenticity::RandomAuthor, gs_config).unwrap();

    let topic_hash = Topic::new("Test").hash();
    let mut peers = vec![];
    let mut topics = BTreeSet::new();
    topics.insert(topic_hash.clone());

    let slow_peer_id = PeerId::random();
    peers.push(slow_peer_id);
    gs.connected_peers.insert(
        slow_peer_id,
        PeerDetails {
            kind: PeerKind::Gossipsubv1_1,
            connections: vec![ConnectionId::new_unchecked(0)],
            outbound: false,
            topics: topics.clone(),
            sender: Sender::new(2),
            dont_send: LinkedHashMap::new(),
        },
    );
    peers.push(slow_peer_id);
    let mesh = gs.mesh.entry(topic_hash.clone()).or_default();
    mesh.insert(slow_peer_id);

    let peer_id = PeerId::random();
    peers.push(peer_id);
    gs.connected_peers.insert(
        peer_id,
        PeerDetails {
            kind: PeerKind::Gossipsubv1_1,
            connections: vec![ConnectionId::new_unchecked(0)],
            outbound: false,
            topics: topics.clone(),
            sender: Sender::new(gs.config.connection_handler_queue_len()),
            dont_send: LinkedHashMap::new(),
        },
    );

    let publish_data = vec![1; 59];
    let transformed = gs
        .data_transform
        .outbound_transform(&topic_hash, publish_data.clone())
        .unwrap();
    let raw_message = gs
        .build_raw_message(topic_hash.clone(), transformed)
        .unwrap();
    let msg_id = gs.config.message_id(&Message {
        source: raw_message.source,
        data: publish_data,
        sequence_number: raw_message.sequence_number,
        topic: raw_message.topic.clone(),
    });

    gs.forward_msg(&msg_id, raw_message.clone(), None, HashSet::new());
    gs.forward_msg(&msg_id, raw_message, None, HashSet::new());

    gs.heartbeat();

    let slow_peer_failed_messages = gs
        .events
        .into_iter()
        .find_map(|e| match e {
            ToSwarm::GenerateEvent(Event::SlowPeer {
                peer_id,
                failed_messages,
            }) if peer_id == slow_peer_id => Some(failed_messages),
            _ => None,
        })
        .unwrap();

    let failed_messages = FailedMessages {
        publish: 0,
        forward: 1,
        priority: 0,
        non_priority: 1,
        timeout: 0,
    };

    assert_eq!(slow_peer_failed_messages.priority, failed_messages.priority);
    assert_eq!(
        slow_peer_failed_messages.non_priority,
        failed_messages.non_priority
    );
    assert_eq!(slow_peer_failed_messages.publish, failed_messages.publish);
    assert_eq!(slow_peer_failed_messages.forward, failed_messages.forward);
}

#[test]
fn test_slow_peer_is_downscored_on_publish() {
    let gs_config = ConfigBuilder::default()
        .validation_mode(ValidationMode::Permissive)
        .build()
        .unwrap();

    let mut gs: Behaviour = Behaviour::new(MessageAuthenticity::RandomAuthor, gs_config).unwrap();
    let slow_peer_params = PeerScoreParams::default();
    gs.with_peer_score(slow_peer_params.clone(), PeerScoreThresholds::default())
        .unwrap();

    let topic_hash = Topic::new("Test").hash();
    let mut peers = vec![];
    let mut topics = BTreeSet::new();
    topics.insert(topic_hash.clone());

    let slow_peer_id = PeerId::random();
    peers.push(slow_peer_id);
    let mesh = gs.mesh.entry(topic_hash.clone()).or_default();
    mesh.insert(slow_peer_id);
    gs.connected_peers.insert(
        slow_peer_id,
        PeerDetails {
            kind: PeerKind::Gossipsubv1_1,
            connections: vec![ConnectionId::new_unchecked(0)],
            outbound: false,
            topics: topics.clone(),
            sender: Sender::new(2),
            dont_send: LinkedHashMap::new(),
        },
    );
    gs.as_peer_score_mut().add_peer(slow_peer_id);
    let peer_id = PeerId::random();
    peers.push(peer_id);
    gs.connected_peers.insert(
        peer_id,
        PeerDetails {
            kind: PeerKind::Gossipsubv1_1,
            connections: vec![ConnectionId::new_unchecked(0)],
            outbound: false,
            topics: topics.clone(),
            sender: Sender::new(gs.config.connection_handler_queue_len()),
            dont_send: LinkedHashMap::new(),
        },
    );

    let publish_data = vec![0; 42];
    gs.publish(topic_hash.clone(), publish_data.clone())
        .unwrap();
    let publish_data = vec![2; 59];
    gs.publish(topic_hash.clone(), publish_data).unwrap();
    gs.heartbeat();
    let slow_peer_score = gs.as_peer_score_mut().score_report(&slow_peer_id).score;
    assert_eq!(slow_peer_score, slow_peer_params.slow_peer_weight);
}

#[tokio::test]
async fn test_timedout_messages_are_reported() {
    let gs_config = ConfigBuilder::default()
        .validation_mode(ValidationMode::Permissive)
        .build()
        .unwrap();

    let mut gs: Behaviour = Behaviour::new(MessageAuthenticity::RandomAuthor, gs_config).unwrap();

    let sender = Sender::new(2);
    let topic_hash = Topic::new("Test").hash();
    let publish_data = vec![2; 59];
    let raw_message = gs.build_raw_message(topic_hash, publish_data).unwrap();

    sender
        .send_message(RpcOut::Publish {
            message: raw_message,
            timeout: Delay::new(Duration::from_nanos(1)),
        })
        .unwrap();
    let mut receiver = sender.new_receiver();
    let stale = future::poll_fn(|cx| receiver.poll_stale(cx)).await.unwrap();
    assert!(matches!(stale, RpcOut::Publish { .. }));
}

#[test]
fn test_priority_messages_are_always_sent() {
    let sender = Sender::new(2);
    let topic_hash = Topic::new("Test").hash();
    // Fill the buffer with the first message.
    assert!(sender
        .send_message(RpcOut::Subscribe(topic_hash.clone()))
        .is_ok());
    assert!(sender
        .send_message(RpcOut::Subscribe(topic_hash.clone()))
        .is_ok());
    assert!(sender.send_message(RpcOut::Unsubscribe(topic_hash)).is_ok());
}

/// Test that specific topic configurations are correctly applied
#[test]
fn test_topic_specific_config() {
    let topic_hash1 = Topic::new("topic1").hash();
    let topic_hash2 = Topic::new("topic2").hash();

    let topic_config1 = TopicMeshConfig {
        mesh_n: 5,
        mesh_n_low: 3,
        mesh_n_high: 10,
        mesh_outbound_min: 2,
    };

    let topic_config2 = TopicMeshConfig {
        mesh_n: 8,
        mesh_n_low: 4,
        mesh_n_high: 12,
        mesh_outbound_min: 3,
    };

    let config = ConfigBuilder::default()
        .set_topic_config(topic_hash1.clone(), topic_config1)
        .set_topic_config(topic_hash2.clone(), topic_config2)
        .build()
        .unwrap();

    assert_eq!(config.mesh_n_for_topic(&topic_hash1), 5);
    assert_eq!(config.mesh_n_low_for_topic(&topic_hash1), 3);
    assert_eq!(config.mesh_n_high_for_topic(&topic_hash1), 10);
    assert_eq!(config.mesh_outbound_min_for_topic(&topic_hash1), 2);

    assert_eq!(config.mesh_n_for_topic(&topic_hash2), 8);
    assert_eq!(config.mesh_n_low_for_topic(&topic_hash2), 4);
    assert_eq!(config.mesh_n_high_for_topic(&topic_hash2), 12);
    assert_eq!(config.mesh_outbound_min_for_topic(&topic_hash2), 3);

    let topic_hash3 = TopicHash::from_raw("topic3");

    assert_eq!(config.mesh_n_for_topic(&topic_hash3), config.mesh_n());
    assert_eq!(
        config.mesh_n_low_for_topic(&topic_hash3),
        config.mesh_n_low()
    );
    assert_eq!(
        config.mesh_n_high_for_topic(&topic_hash3),
        config.mesh_n_high()
    );
    assert_eq!(
        config.mesh_outbound_min_for_topic(&topic_hash3),
        config.mesh_outbound_min()
    );
}

/// Test mesh maintenance with topic-specific configurations
#[test]
fn test_topic_mesh_maintenance_with_specific_config() {
    let topic1_hash = TopicHash::from_raw("topic1");
    let topic2_hash = TopicHash::from_raw("topic2");

    let topic_config1 = TopicMeshConfig {
        mesh_n: 4,
        mesh_n_low: 2,
        mesh_n_high: 6,
        mesh_outbound_min: 1,
    };

    let topic_config2 = TopicMeshConfig {
        mesh_n: 8,
        mesh_n_low: 4,
        mesh_n_high: 12,
        mesh_outbound_min: 3,
    };

    let config = ConfigBuilder::default()
        .set_topic_config(topic1_hash, topic_config1)
        .set_topic_config(topic2_hash, topic_config2)
        .build()
        .unwrap();

    let (mut gs, _, _, topic_hashes) = inject_nodes1()
        .peer_no(15)
        .topics(vec!["topic1".into(), "topic2".into()])
        .to_subscribe(true)
        .gs_config(config)
        .create_network();

    assert_eq!(
        gs.mesh.get(&topic_hashes[0]).unwrap().len(),
        2,
        "topic1 should have mesh_n 2 peers"
    );
    assert_eq!(
        gs.mesh.get(&topic_hashes[1]).unwrap().len(),
        4,
        "topic2 should have mesh_n 4 peers"
    );

    // run a heartbeat
    gs.heartbeat();

    assert_eq!(
        gs.mesh.get(&topic_hashes[0]).unwrap().len(),
        2,
        "topic1 should maintain mesh_n 2 peers after heartbeat"
    );
    assert_eq!(
        gs.mesh.get(&topic_hashes[1]).unwrap().len(),
        4,
        "topic2 should maintain mesh_n 4 peers after heartbeat"
    );
}

/// Test mesh addition with topic-specific configuration
#[test]
fn test_mesh_addition_with_topic_config() {
    let topic = String::from("topic1");
    let topic_hash = TopicHash::from_raw(topic.clone());

    let topic_config = TopicMeshConfig {
        mesh_n: 6,
        mesh_n_low: 3,
        mesh_n_high: 9,
        mesh_outbound_min: 2,
    };

    let config = ConfigBuilder::default()
        .set_topic_config(topic_hash.clone(), topic_config.clone())
        .build()
        .unwrap();

    let (mut gs, peers, _, topics) = inject_nodes1()
        .peer_no(config.mesh_n_for_topic(&topic_hash) + 1)
        .topics(vec![topic])
        .to_subscribe(true)
        .gs_config(config.clone())
        .create_network();

    let to_remove_peers = 1;

    for peer in peers.iter().take(to_remove_peers) {
        gs.handle_prune(
            peer,
            topics.iter().map(|h| (h.clone(), vec![], None)).collect(),
        );
    }

    assert_eq!(
        gs.mesh.get(&topics[0]).unwrap().len(),
        config.mesh_n_low_for_topic(&topic_hash) - 1
    );

    // run a heartbeat
    gs.heartbeat();

    // Peers should be added to reach mesh_n
    assert_eq!(
        gs.mesh.get(&topics[0]).unwrap().len(),
        config.mesh_n_for_topic(&topic_hash)
    );
}

/// Test mesh subtraction with topic-specific configuration
#[test]
fn test_mesh_subtraction_with_topic_config() {
    let topic = String::from("topic1");
    let topic_hash = TopicHash::from_raw(topic.clone());

    let topic_config = TopicMeshConfig {
        mesh_n: 5,
        mesh_n_low: 3,
        mesh_n_high: 7,
        mesh_outbound_min: 2,
    };

    let config = ConfigBuilder::default()
        .set_topic_config(topic_hash.clone(), topic_config)
        .build()
        .unwrap();

    let peer_no = 12;

    // make all outbound connections so grafting to all will be allowed
    let (mut gs, peers, _, topics) = inject_nodes1()
        .peer_no(peer_no)
        .topics(vec![topic])
        .to_subscribe(true)
        .gs_config(config.clone())
        .outbound(peer_no)
        .create_network();

    // graft all peers
    for peer in peers {
        gs.handle_graft(&peer, topics.clone());
    }

    assert_eq!(
        gs.mesh.get(&topics[0]).unwrap().len(),
        peer_no,
        "Initially all peers should be in the mesh"
    );

    // run a heartbeat
    gs.heartbeat();

    // Peers should be removed to reach mesh_n
    assert_eq!(
        gs.mesh.get(&topics[0]).unwrap().len(),
        5,
        "After heartbeat, mesh should be reduced to mesh_n 5 peers"
    );
}

/// Test behavior with multiple topics having different configs
#[test]
fn test_multiple_topics_with_different_configs() {
    let topic1 = String::from("topic1");
    let topic2 = String::from("topic2");
    let topic3 = String::from("topic3");

    let topic_hash1 = TopicHash::from_raw(topic1.clone());
    let topic_hash2 = TopicHash::from_raw(topic2.clone());
    let topic_hash3 = TopicHash::from_raw(topic3.clone());

    let config1 = TopicMeshConfig {
        mesh_n: 4,
        mesh_n_low: 3,
        mesh_n_high: 6,
        mesh_outbound_min: 1,
    };

    let config2 = TopicMeshConfig {
        mesh_n: 6,
        mesh_n_low: 4,
        mesh_n_high: 9,
        mesh_outbound_min: 2,
    };

    let config3 = TopicMeshConfig {
        mesh_n: 9,
        mesh_n_low: 6,
        mesh_n_high: 13,
        mesh_outbound_min: 3,
    };

    let config = ConfigBuilder::default()
        .set_topic_config(topic_hash1.clone(), config1)
        .set_topic_config(topic_hash2.clone(), config2)
        .set_topic_config(topic_hash3.clone(), config3)
        .build()
        .unwrap();

    // Create network with many peers and three topics
    let (mut gs, _, _, topic_hashes) = inject_nodes1()
        .peer_no(35)
        .topics(vec![topic1, topic2, topic3])
        .to_subscribe(true)
        .gs_config(config)
        .create_network();

    // Check that mesh sizes match each topic's config
    assert_eq!(
        gs.mesh.get(&topic_hashes[0]).unwrap().len(),
        3,
        "topic1 should have 3 peers"
    );
    assert_eq!(
        gs.mesh.get(&topic_hashes[1]).unwrap().len(),
        4,
        "topic2 should have 4 peers"
    );
    assert_eq!(
        gs.mesh.get(&topic_hashes[2]).unwrap().len(),
        6,
        "topic3 should have 6 peers"
    );

    // run a heartbeat
    gs.heartbeat();

    // Verify mesh sizes remain correct after maintenance. The mesh parameters are > mesh_n_low and
    // < mesh_n_high so the implementation will maintain the mesh at mesh_n_low.
    assert_eq!(
        gs.mesh.get(&topic_hashes[0]).unwrap().len(),
        3,
        "topic1 should maintain 3 peers after heartbeat"
    );
    assert_eq!(
        gs.mesh.get(&topic_hashes[1]).unwrap().len(),
        4,
        "topic2 should maintain 4 peers after heartbeat"
    );
    assert_eq!(
        gs.mesh.get(&topic_hashes[2]).unwrap().len(),
        6,
        "topic3 should maintain 6 peers after heartbeat"
    );

    // Unsubscribe from topic1
    assert!(
        gs.unsubscribe(&Topic::new(topic_hashes[0].to_string())),
        "Should unsubscribe successfully"
    );

    // verify it's removed from mesh
    assert!(
        !gs.mesh.contains_key(&topic_hashes[0]),
        "topic1 should be removed from mesh after unsubscribe"
    );

    // re-subscribe to topic1
    assert!(
        gs.subscribe(&Topic::new(topic_hashes[0].to_string()))
            .unwrap(),
        "Should subscribe successfully"
    );

    // Verify mesh is recreated with correct size
    assert_eq!(
        gs.mesh.get(&topic_hashes[0]).unwrap().len(),
        4,
        "topic1 should have mesh_n 4 peers after re-subscribe"
    );
}

/// Test fanout behavior with topic-specific configuration
#[test]
fn test_fanout_with_topic_config() {
    let topic = String::from("topic1");
    let topic_hash = TopicHash::from_raw(topic.clone());

    let topic_config = TopicMeshConfig {
        mesh_n: 4,
        mesh_n_low: 2,
        mesh_n_high: 7,
        mesh_outbound_min: 1,
    };

    // turn off flood publish to test fanout behaviour
    let config = ConfigBuilder::default()
        .flood_publish(false)
        .set_topic_config(topic_hash.clone(), topic_config)
        .build()
        .unwrap();

    let (mut gs, _, receivers, topic_hashes) = inject_nodes1()
        .peer_no(10) // More than mesh_n
        .topics(vec![topic.clone()])
        .to_subscribe(true)
        .gs_config(config)
        .create_network();

    assert!(
        gs.unsubscribe(&Topic::new(topic.clone())),
        "Should unsubscribe successfully"
    );

    let publish_data = vec![0; 42];
    gs.publish(Topic::new(topic), publish_data).unwrap();

    // Check that fanout size matches the topic-specific mesh_n
    assert_eq!(
        gs.fanout.get(&topic_hashes[0]).unwrap().len(),
        4,
        "Fanout should contain topic-specific mesh_n 4 peers for this topic"
    );

    // Collect publish messages
    let publishes = receivers
        .into_values()
        .fold(vec![], |mut collected_publish, c| {
            let priority = c.priority.get_ref();
            while !priority.is_empty() {
                if let Ok(RpcOut::Publish { message, .. }) = priority.try_recv() {
                    collected_publish.push(message);
                }
            }
            collected_publish
        });

    // Verify sent to topic-specific mesh_n number of peers
    assert_eq!(
        publishes.len(),
        4,
        "Should send a publish message to topic-specific mesh_n 4 fanout peers"
    );
}

#[test]
fn test_publish_message_with_default_transmit_size_config() {
    let topic = Topic::new("test");
    let topic_hash = topic.hash();

    let config = ConfigBuilder::default()
        .set_topic_max_transmit_size(topic_hash.clone(), Config::default_max_transmit_size())
        .validation_mode(ValidationMode::Strict)
        .build()
        .unwrap();

    let (mut gs, _, _, _) = inject_nodes1()
        .peer_no(10)
        .topics(vec!["test".to_string()])
        .to_subscribe(true)
        .gs_config(config)
        .create_network();

    let data = vec![0; 1024];

    let result = gs.publish(topic.clone(), data);
    assert!(
        result.is_ok(),
        "Expected successful publish within size limit"
    );
    let msg_id = result.unwrap();
    assert!(
        gs.mcache.get(&msg_id).is_some(),
        "Message should be in cache"
    );
}

#[test]
fn test_publish_large_message_with_default_transmit_size_config() {
    let topic = Topic::new("test");
    let topic_hash = topic.hash();

    let config = ConfigBuilder::default()
        .set_topic_max_transmit_size(topic_hash.clone(), Config::default_max_transmit_size())
        .validation_mode(ValidationMode::Strict)
        .build()
        .unwrap();

    let (mut gs, _, _, _) = inject_nodes1()
        .peer_no(10)
        .topics(vec!["test".to_string()])
        .to_subscribe(true)
        .gs_config(config)
        .create_network();

    let data = vec![0; Config::default_max_transmit_size() + 1];

    let result = gs.publish(topic.clone(), data);
    assert!(
        matches!(result, Err(PublishError::MessageTooLarge)),
        "Expected MessageTooLarge error for oversized message"
    );
}

#[test]
fn test_publish_message_with_specific_transmit_size_config() {
    let topic = Topic::new("test");
    let topic_hash = topic.hash();

    let max_topic_transmit_size = 2000;
    let config = ConfigBuilder::default()
        .set_topic_max_transmit_size(topic_hash.clone(), max_topic_transmit_size)
        .validation_mode(ValidationMode::Strict)
        .build()
        .unwrap();

    let (mut gs, _, _, _) = inject_nodes1()
        .peer_no(10)
        .topics(vec!["test".to_string()])
        .to_subscribe(true)
        .gs_config(config)
        .create_network();

    let data = vec![0; 1024];

    let result = gs.publish(topic.clone(), data);
    assert!(
        result.is_ok(),
        "Expected successful publish within topic-specific size limit"
    );
    let msg_id = result.unwrap();
    assert!(
        gs.mcache.get(&msg_id).is_some(),
        "Message should be in cache"
    );
}

#[test]
fn test_publish_large_message_with_specific_transmit_size_config() {
    let topic = Topic::new("test");
    let topic_hash = topic.hash();

    let max_topic_transmit_size = 2048;
    let config = ConfigBuilder::default()
        .set_topic_max_transmit_size(topic_hash.clone(), max_topic_transmit_size)
        .validation_mode(ValidationMode::Strict)
        .build()
        .unwrap();

    let (mut gs, _, _, _) = inject_nodes1()
        .peer_no(10)
        .topics(vec!["test".to_string()])
        .to_subscribe(true)
        .gs_config(config)
        .create_network();

    let data = vec![0; 2049];

    let result = gs.publish(topic.clone(), data);
    assert!(
        matches!(result, Err(PublishError::MessageTooLarge)),
        "Expected MessageTooLarge error for oversized message with topic-specific config"
    );
}

#[test]
fn test_validation_error_message_size_too_large_topic_specific() {
    let topic = Topic::new("test");
    let topic_hash = topic.hash();
    let max_size = 2048;

    let config = ConfigBuilder::default()
        .set_topic_max_transmit_size(topic_hash.clone(), max_size)
        .validation_mode(ValidationMode::None)
        .build()
        .unwrap();

    let (mut gs, peers, _, topic_hashes) = inject_nodes1()
        .peer_no(1)
        .topics(vec![String::from("test")])
        .to_subscribe(true)
        .gs_config(config)
        .create_network();

    let data = vec![0u8; max_size + 1];
    let raw_message = RawMessage {
        source: Some(peers[0]),
        data,
        sequence_number: Some(1u64),
        topic: topic_hashes[0].clone(),
        signature: None,
        key: None,
        validated: false,
    };

    gs.on_connection_handler_event(
        peers[0],
        ConnectionId::new_unchecked(0),
        HandlerEvent::Message {
            rpc: RpcIn {
                messages: vec![raw_message],
                subscriptions: vec![],
                control_msgs: vec![],
            },
            invalid_messages: vec![],
        },
    );

    let event = gs.events.pop_front().expect("Event should be generated");
    match event {
        ToSwarm::GenerateEvent(Event::Message {
            propagation_source,
            message_id: _,
            message,
        }) => {
            assert_eq!(propagation_source, peers[0]);
            assert_eq!(message.data.len(), max_size + 1);
        }
        ToSwarm::NotifyHandler { peer_id, .. } => {
            assert_eq!(peer_id, peers[0]);
        }
        _ => panic!("Unexpected event"),
    }

    // Simulate a peer sending a message exceeding the topic-specific max_transmit_size (2048
    // bytes). The codec's max_length is set high to allow encoding/decoding the RPC, while
    // max_transmit_sizes enforces the custom topic limit.
    let mut max_transmit_size_map = HashMap::new();
    max_transmit_size_map.insert(topic_hash, max_size);

    let mut codec = GossipsubCodec::new(
        Config::default_max_transmit_size() * 2,
        ValidationMode::None,
        max_transmit_size_map,
    );
    let mut buf = BytesMut::new();
    let rpc = proto::RPC {
        publish: vec![proto::Message {
            from: Some(peers[0].to_bytes()),
            data: Some(vec![0u8; max_size + 1]),
            seqno: Some(1u64.to_be_bytes().to_vec()),
            topic: topic_hashes[0].to_string(),
            signature: None,
            key: None,
        }],
        subscriptions: vec![],
        control: None,
    };
    codec.encode(rpc, &mut buf).unwrap();

    let decoded = codec.decode(&mut buf).unwrap().unwrap();
    match decoded {
        HandlerEvent::Message {
            rpc,
            invalid_messages,
        } => {
            assert!(
                rpc.messages.is_empty(),
                "No valid messages should be present"
            );
            assert_eq!(invalid_messages.len(), 1, "One message should be invalid");
            let (invalid_msg, error) = &invalid_messages[0];
            assert_eq!(invalid_msg.data.len(), max_size + 1);
            assert_eq!(error, &ValidationError::MessageSizeTooLargeForTopic);
        }
        _ => panic!("Unexpected event"),
    }
}

#[test]
fn test_validation_message_size_within_topic_specific() {
    let topic = Topic::new("test");
    let topic_hash = topic.hash();
    let max_size = 2048;

    let config = ConfigBuilder::default()
        .set_topic_max_transmit_size(topic_hash.clone(), max_size)
        .validation_mode(ValidationMode::None)
        .build()
        .unwrap();

    let (mut gs, peers, _, topic_hashes) = inject_nodes1()
        .peer_no(1)
        .topics(vec![String::from("test")])
        .to_subscribe(true)
        .gs_config(config)
        .create_network();

    let data = vec![0u8; max_size - 100];
    let raw_message = RawMessage {
        source: Some(peers[0]),
        data,
        sequence_number: Some(1u64),
        topic: topic_hashes[0].clone(),
        signature: None,
        key: None,
        validated: false,
    };

    gs.on_connection_handler_event(
        peers[0],
        ConnectionId::new_unchecked(0),
        HandlerEvent::Message {
            rpc: RpcIn {
                messages: vec![raw_message],
                subscriptions: vec![],
                control_msgs: vec![],
            },
            invalid_messages: vec![],
        },
    );

    let event = gs.events.pop_front().expect("Event should be generated");
    match event {
        ToSwarm::GenerateEvent(Event::Message {
            propagation_source,
            message_id: _,
            message,
        }) => {
            assert_eq!(propagation_source, peers[0]);
            assert_eq!(message.data.len(), max_size - 100);
        }
        ToSwarm::NotifyHandler { peer_id, .. } => {
            assert_eq!(peer_id, peers[0]);
        }
        _ => panic!("Unexpected event"),
    }

    // Simulate a peer sending a message within the topic-specific max_transmit_size (2048 bytes).
    // The codec's max_length allows encoding/decoding the RPC, and max_transmit_sizes confirms
    // the message size is acceptable for the topic.
    let mut max_transmit_size_map = HashMap::new();
    max_transmit_size_map.insert(topic_hash, max_size);

    let mut codec = GossipsubCodec::new(
        Config::default_max_transmit_size() * 2,
        ValidationMode::None,
        max_transmit_size_map,
    );
    let mut buf = BytesMut::new();
    let rpc = proto::RPC {
        publish: vec![proto::Message {
            from: Some(peers[0].to_bytes()),
            data: Some(vec![0u8; max_size - 100]),
            seqno: Some(1u64.to_be_bytes().to_vec()),
            topic: topic_hashes[0].to_string(),
            signature: None,
            key: None,
        }],
        subscriptions: vec![],
        control: None,
    };
    codec.encode(rpc, &mut buf).unwrap();

    let decoded = codec.decode(&mut buf).unwrap().unwrap();
    match decoded {
        HandlerEvent::Message {
            rpc,
            invalid_messages,
        } => {
            assert_eq!(rpc.messages.len(), 1, "One valid message should be present");
            assert!(invalid_messages.is_empty(), "No messages should be invalid");
            assert_eq!(rpc.messages[0].data.len(), max_size - 100);
        }
        _ => panic!("Unexpected event"),
    }
}
