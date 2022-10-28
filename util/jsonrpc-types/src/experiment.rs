use crate::{Cycle, OutPoint};
use ckb_types::H256;
use serde::{Deserialize, Serialize};

/// Response result of the RPC method `estimate_cycles`.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct EstimateCycles {
    /// The count of cycles that the VM has consumed to verify this transaction.
    pub cycles: Cycle,
}

/// An enum to represent the two kinds of dao withdrawal amount calculation option.
/// `DaoWithdrawingCalculationKind` is equivalent to [`H256`] `|` [`OutPoint`].
///
/// [`H256`]: struct.H256.html
/// [`OutPoint`]: struct.OutPoint.html
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(untagged)]
pub enum DaoWithdrawingCalculationKind {
    /// the assumed reference block hash for withdrawing phase 1 transaction
    WithdrawingHeaderHash(H256),
    /// the out point of the withdrawing phase 1 transaction
    WithdrawingOutPoint(OutPoint),
}
