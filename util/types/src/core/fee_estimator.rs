use crate::core::FeeRate;

/// Recommended fee rates.
#[derive(Clone, Copy, Debug)]
pub struct RecommendedFeeRates {
    /// Default fee rate.
    pub default: FeeRate,
    /// Low-priority fee rate.
    pub low: FeeRate,
    /// Medium-priority fee rate.
    pub medium: FeeRate,
    /// High-priority fee rate.
    pub high: FeeRate,
}
