use crate::{BlockNumber, Byte32, Timestamp, Uint64};
use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct LocalNode {
    pub version: String,
    pub node_id: String,
    pub active: bool,
    pub addresses: Vec<NodeAddress>,
    pub protocols: Vec<LocalNodeProtocol>,
    pub connections: Uint64,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct LocalNodeProtocol {
    pub id: Uint64,
    pub name: String,
    pub support_versions: Vec<String>,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct RemoteNode {
    pub version: String,
    pub node_id: String,
    pub addresses: Vec<NodeAddress>,
    pub is_outbound: bool,
    pub connected_duration: Uint64,
    pub last_ping_duration: Option<Uint64>,
    pub sync_state: Option<PeerSyncState>,
    pub protocols: Vec<RemoteNodeProtocol>,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct RemoteNodeProtocol {
    pub id: Uint64,
    pub version: String,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct PeerSyncState {
    pub best_known_header_hash: Option<Byte32>,
    pub best_known_header_number: Option<Uint64>,
    pub last_common_header_hash: Option<Byte32>,
    pub last_common_header_number: Option<Uint64>,
    pub unknown_header_list_size: Uint64,
    pub inflight_count: Uint64,
    pub can_fetch_count: Uint64,
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
