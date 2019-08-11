use ckb_occupied_capacity::Capacity;
use serde_derive::{Deserialize, Serialize};

/// shannons per kilobytes
#[derive(Default, Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct FeeRate(u64);

impl FeeRate {
    pub const fn new(fee_per_kilobyte: Capacity) -> Self {
        FeeRate(fee_per_kilobyte.as_u64())
    }

    pub const fn zero() -> Self {
        Self::new(Capacity::zero())
    }

    pub fn fee(self, size: usize) -> Capacity {
        let fee = self.0.saturating_mul(size as u64) / 1000;
        Capacity::shannons(fee)
    }

    pub fn saturating_add(self, other: FeeRate) -> Self {
        FeeRate(self.0.saturating_add(other.0))
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }

    pub fn from_u64(fee_rate: u64) -> Self {
        FeeRate(fee_rate)
    }
}

impl ::std::fmt::Display for FeeRate {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
