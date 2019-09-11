use crate::{Timestamp, Unsigned};
use serde_derive::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug)]
pub struct PeerState {
    // TODO use peer_id
    // peer session id
    peer: Unsigned,
    // last updated timestamp
    last_updated: Timestamp,
    // blocks count has request but not receive response yet
    blocks_in_flight: Unsigned,
}

impl PeerState {
    pub fn new(peer: usize, last_updated: u64, blocks_in_flight: usize) -> Self {
        Self {
            peer: (peer as u64).into(),
            last_updated: last_updated.into(),
            blocks_in_flight: (blocks_in_flight as u64).into(),
        }
    }
}
