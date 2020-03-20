use crate::{Timestamp, Uint64};
use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct TxPoolInfo {
    pub pending: Uint64,
    pub proposed: Uint64,
    pub orphan: Uint64,
    pub total_tx_size: Uint64,
    pub total_tx_cycles: Uint64,
    pub min_fee_rate: Uint64,
    pub last_txs_updated_at: Timestamp,
}
