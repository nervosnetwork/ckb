use serde_derive::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct TxPoolInfo {
    pub pending: u32,
    pub staging: u32,
    pub orphan: u32,
    // timestamp(u64)
    pub last_txs_updated_at: String,
}
