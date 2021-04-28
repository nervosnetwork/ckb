use crate::Cycle;
use serde::{Deserialize, Serialize};

/// Response result of the RPC method `dry_run_transaction`.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct DryRunResult {
    /// The count of cycles that the VM has consumed to verify this transaction.
    pub cycles: Cycle,
}
