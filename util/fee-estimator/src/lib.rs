//! CKB builtin fee estimator.

mod estimator;
mod fee_rate;
mod tx_confirm_stat;

pub use estimator::{Estimator, MAX_CONFIRM_BLOCKS};
pub use fee_rate::FeeRate;
