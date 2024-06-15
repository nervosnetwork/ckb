//! CKB's built-in fee estimator, which shares data with the ckb node through the tx-pool service.

pub mod constants;
pub(crate) mod error;
pub(crate) mod estimator;

pub use error::Error;
pub use estimator::FeeEstimator;
