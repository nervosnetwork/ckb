use crate::core::Capacity;

/// shannons per kilo-weight
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FeeRate(pub u64);

const KW: u64 = 1000;

impl FeeRate {
    /// TODO(doc): @doitian
    pub fn calculate(fee: Capacity, weight: u64) -> Self {
        if weight == 0 {
            return FeeRate::zero();
        }
        FeeRate::from_u64(fee.as_u64().saturating_mul(KW) / weight)
    }

    /// TODO(doc): @doitian
    pub const fn from_u64(fee_per_kw: u64) -> Self {
        FeeRate(fee_per_kw)
    }

    /// TODO(doc): @doitian
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// TODO(doc): @doitian
    pub const fn zero() -> Self {
        Self::from_u64(0)
    }

    /// TODO(doc): @doitian
    pub fn fee(self, weight: u64) -> Capacity {
        let fee = self.0.saturating_mul(weight) / KW;
        Capacity::shannons(fee)
    }
}

impl ::std::fmt::Display for FeeRate {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        write!(f, "{} shannons/KW", self.0)
    }
}
