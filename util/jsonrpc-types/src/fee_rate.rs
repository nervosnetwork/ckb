#![doc(hidden)]

// #[doc(hidden)] attribute
// serde remote hitting https://github.com/rust-lang/rust/issues/42008

use ckb_types::core::FeeRate;
use serde::{Deserialize, Serialize};

/// Serialize and Deserialize implementations for FeeRate
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(remote = "FeeRate")]
pub struct FeeRateDef(u64);
