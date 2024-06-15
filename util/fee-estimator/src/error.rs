//! The error type for the fee estimator.

use thiserror::Error;

/// A list specifying general categories of fee estimator errors.
#[derive(Error, Debug)]
pub enum Error {
    /// Not ready for do estimate.
    #[error("not ready")]
    NotReady,
    /// Lack of empirical data.
    #[error("lack of empirical data")]
    LackData,
}
