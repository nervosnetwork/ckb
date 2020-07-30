use crate::{BlockNumber, Timestamp, Uint64};
use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct LocalNode {
    pub version: String,
    pub node_id: String,
    pub addresses: Vec<NodeAddress>,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct RemoteNode {
    pub version: String,
    pub node_id: String,
    pub addresses: Vec<NodeAddress>,
    pub is_outbound: bool,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct NodeAddress {
    pub address: String,
    pub score: Uint64,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct BannedAddr {
    pub address: String,
    pub ban_until: Timestamp,
    pub ban_reason: String,
    pub created_at: Timestamp,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct SyncState {
    pub ibd: bool,
    pub best_known_block_number: BlockNumber,
    pub best_known_block_timestamp: Timestamp,
    pub orphan_blocks_count: Uint64,
    pub inflight_blocks_count: Uint64,
    pub fast_time: Uint64,
    pub normal_time: Uint64,
    pub low_time: Uint64,
}
