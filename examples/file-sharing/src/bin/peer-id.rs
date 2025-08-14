use libp2p::PeerId;
use std::collections::HashSet;
use std::str::FromStr;

fn main() {
    // 1. Create a mutable HashSet to hold our PeerIds.
    // The type is inferred, but it's good practice to be explicit.
    let mut peer_ids: HashSet<PeerId> = HashSet::new();

    // 2. We need a PeerId to add. Let's create one from a valid string.
    // Replace this with a real PeerId from your network.
    let peer_id_string = "QmYyQz4k9T... (your valid PeerId string)";

    // Use from_str to parse the string into a PeerId, handling potential errors.
    let new_peer_id = match PeerId::from_str(peer_id_string) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("Failed to parse PeerId from string: {}", e);
            return; // Exit the program if the PeerId string is invalid
        }
    };

    // 3. Add the new PeerId to the HashSet using the `.insert()` method.
    // The insert method returns true if the value was not already present.
    let was_inserted = peer_ids.insert(new_peer_id.clone());

    if was_inserted {
        println!("Successfully added the new PeerId to the set.");
    } else {
        println!("The PeerId was already in the set.");
    }

    // You can also add other PeerIds
    let another_peer_id = PeerId::random(); // Create a random PeerId
    peer_ids.insert(another_peer_id);

    // Let's check the contents of our set
    println!("The HashSet now contains {} PeerId(s).", peer_ids.len());

    // You can also iterate over the set's contents
    for peer in peer_ids.iter() {
        println!("- {}", peer);
    }
}
