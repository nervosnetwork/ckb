use serde::{Deserialize, Serialize};

/// Fee estimator config options.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The algorithm for fee estimator.
    pub algorithm: Option<Algorithm>,
}

/// Specifies the fee estimates algorithm.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Eq)]
pub enum Algorithm {
    /// Confirmation Fraction Fee Estimator
    ConfirmationFraction,
    /// Weight-Units Flow Fee Estimator
    WeightUnitsFlow,
}
