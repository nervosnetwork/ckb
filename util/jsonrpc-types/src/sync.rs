use crate::{Timestamp, Uint32};
use serde::{Deserialize, Serialize};

/// Peer (remote node) state.
#[derive(Deserialize, Serialize, Debug)]
pub struct PeerState {
    // TODO use peer_id
    /// Peer session id.
    pub peer: Uint32,
    /// last updated timestamp.
    pub last_updated: Timestamp,
    /// blocks count has request but not receive response yet.
    pub blocks_in_flight: Uint32,
}

impl PeerState {
    /// TODO(doc): @doitian
    pub fn new(peer: usize, last_updated: u64, blocks_in_flight: usize) -> Self {
        Self {
            peer: (peer as u32).into(),
            last_updated: last_updated.into(),
            blocks_in_flight: (blocks_in_flight as u32).into(),
        }
    }
}
