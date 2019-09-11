use crate::{Timestamp, Uint64};
use serde_derive::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug)]
pub struct PeerState {
    // TODO use peer_id
    // peer session id
    peer: Uint64,
    // last updated timestamp
    last_updated: Timestamp,
    // blocks count has request but not receive response yet
    blocks_in_flight: Uint64,
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
