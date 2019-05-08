use serde_derive::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug)]
pub struct PeerState {
    // TODO use peer_id
    // peer session id
    peer: String,
    // last updated timestamp
    last_updated: String,
    // blocks count has request but not receive response yet
    blocks_in_flight: String,
}

impl PeerState {
    pub fn new(peer: usize, last_updated: u64, blocks_in_flight: usize) -> Self {
        Self {
            peer: peer.to_string(),
            last_updated: last_updated.to_string(),
            blocks_in_flight: blocks_in_flight.to_string(),
        }
    }
}
