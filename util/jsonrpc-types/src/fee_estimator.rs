use ckb_types::core;
use serde::{Deserialize, Serialize};

use schemars::JsonSchema;

/// The fee estimate mode.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum EstimateMode {
    /// No priority, expect the transaction to be committed in 1 hour.
    #[default]
    NoPriority,
    /// Low priority, expect the transaction to be committed in 30 minutes.
    LowPriority,
    /// Medium priority, expect the transaction to be committed in 10 minutes.
    MediumPriority,
    /// High priority, expect the transaction to be committed as soon as possible.
    HighPriority,
}

impl From<EstimateMode> for core::EstimateMode {
    fn from(json: EstimateMode) -> Self {
        match json {
            EstimateMode::NoPriority => core::EstimateMode::NoPriority,
            EstimateMode::LowPriority => core::EstimateMode::LowPriority,
            EstimateMode::MediumPriority => core::EstimateMode::MediumPriority,
            EstimateMode::HighPriority => core::EstimateMode::HighPriority,
        }
    }
}

impl From<core::EstimateMode> for EstimateMode {
    fn from(data: core::EstimateMode) -> Self {
        match data {
            core::EstimateMode::NoPriority => EstimateMode::NoPriority,
            core::EstimateMode::LowPriority => EstimateMode::LowPriority,
            core::EstimateMode::MediumPriority => EstimateMode::MediumPriority,
            core::EstimateMode::HighPriority => EstimateMode::HighPriority,
        }
    }
}
