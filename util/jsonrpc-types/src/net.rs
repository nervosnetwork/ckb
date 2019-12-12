use crate::{Timestamp, Uint64};
use serde::{Deserialize, Serialize};

// TODO add more fields from PeerIdentifyInfo
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Node {
    pub version: String,
    pub node_id: String,
    pub addresses: Vec<NodeAddress>,
    pub is_outbound: Option<bool>,
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
