use ckb_types::core;
use serde::{Deserialize, Serialize};

use schemars::JsonSchema;

/// Recommended fee rates.
#[derive(Clone, Copy, Default, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RecommendedFeeRates {
    /// Default fee rate.
    #[serde(rename = "no_priority")]
    pub default: u64,
    /// Low-priority fee rate.
    #[serde(rename = "low_priority")]
    pub low: u64,
    /// Medium-priority fee rate.
    #[serde(rename = "medium_priority")]
    pub medium: u64,
    /// High-priority fee rate.
    #[serde(rename = "high_priority")]
    pub high: u64,
}

impl From<RecommendedFeeRates> for core::RecommendedFeeRates {
    fn from(json: RecommendedFeeRates) -> Self {
        core::RecommendedFeeRates {
            default: core::FeeRate::from_u64(json.default),
            low: core::FeeRate::from_u64(json.low),
            medium: core::FeeRate::from_u64(json.medium),
            high: core::FeeRate::from_u64(json.high),
        }
    }
}

impl From<core::RecommendedFeeRates> for RecommendedFeeRates {
    fn from(data: core::RecommendedFeeRates) -> Self {
        RecommendedFeeRates {
            default: data.default.as_u64(),
            low: data.low.as_u64(),
            medium: data.medium.as_u64(),
            high: data.high.as_u64(),
        }
    }
}
