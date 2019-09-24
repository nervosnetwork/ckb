use ckb_occupied_capacity::Capacity;
use serde_derive::{Deserialize, Serialize};

/// shannons per kilobytes
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FeeRate(u64);

impl FeeRate {
    pub const fn from_u64(fee_per_kb: u64) -> Self {
        FeeRate(fee_per_kb)
    }

    pub const fn zero() -> Self {
        Self::from_u64(0)
    }

    pub fn fee(self, size: usize) -> Capacity {
        let fee = self.0.saturating_mul(size as u64) / 1000;
        Capacity::shannons(fee)
    }
}

impl ::std::fmt::Display for FeeRate {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
