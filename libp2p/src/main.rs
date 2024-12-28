use std::vec;

use libp2p::client::Client;
use libp2p::ws::req::ReqFilter;

use tungstenite::Message;

fn handle_message(relay_url: &String, message: &Message) -> Result<(), String> {
    println!("Received message from {}: {:?}", relay_url, message);

    println!("Events: {:?}", message);

    Ok(())
}

#[tokio::main]
async fn main() {
    let mut nostr_client = Client::new(vec!["wss://relay.damus.io"])
        .await
        .unwrap();

    // Run a new thread to handle messages

    println!("Listening...");
    let events = nostr_client.next_data().await.unwrap();
    println!("Events: {:?}", events);
    for (relay_url, message) in events.iter() {
        handle_message(relay_url, message).unwrap();
    }

    // Subscribe to my last text note
    let subscription_id = nostr_client
        .subscribe(vec![ReqFilter {
            ids: None,
            authors: None,
            kinds: Some(vec![0]),
            e: None,
            p: None,
            since: None,
            until: None,
            limit: Some(10),
        }])
        .await
        .unwrap();

    // Unsubscribe
    nostr_client.unsubscribe(&subscription_id).await.unwrap();
}
