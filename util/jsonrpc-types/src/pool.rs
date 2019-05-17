use crate::{Timestamp, Unsigned};
use serde_derive::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct TxPoolInfo {
    pub pending: Unsigned,
    pub proposed: Unsigned,
    pub orphan: Unsigned,
    pub last_txs_updated_at: Timestamp,
}
