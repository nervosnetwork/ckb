use crate::{Cycle, FeeRate};
use serde_derive::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct DryRunResult {
    pub cycles: Cycle,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct EstimateResult {
    pub fee_rate: FeeRate,
}
