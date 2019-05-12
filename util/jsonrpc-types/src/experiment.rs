use crate::Cycle;
use serde_derive::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct DryRunResult {
    pub cycles: Cycle,
}
