use crate::{Cycle, FeeRate};
use serde::{Deserialize, Serialize};

/// Response result of the RPC method `dry_run_transaction`.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct DryRunResult {
    /// The count of cycles that the VM has consumed to verify this transaction.
    pub cycles: Cycle,
}

/// The estimated fee rate.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct EstimateResult {
    /// The estimated fee rate.
    pub fee_rate: FeeRate,
}
