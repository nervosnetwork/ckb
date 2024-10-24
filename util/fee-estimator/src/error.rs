//! The error type for the fee estimator.

use thiserror::Error;

/// A list specifying general categories of fee estimator errors.
#[derive(Error, Debug, PartialEq)]
pub enum Error {
    /// Dummy fee estimator is used.
    #[error("dummy fee estimator is used")]
    Dummy,
    /// Not ready for do estimate.
    #[error("not ready")]
    NotReady,
    /// Lack of empirical data.
    #[error("lack of empirical data")]
    LackData,
    /// No proper fee rate.
    #[error("no proper fee rate")]
    NoProperFeeRate,
}
