use ckb_types::core::{
    Capacity, FeeRate, tx_pool::AncestorsScoreSortKey as CoreAncestorsScoreSortKey,
};
use std::cmp::Ordering;

/// A struct to use as a sorted key
#[derive(Eq, PartialEq, Clone, Debug)]
pub struct AncestorsScoreSortKey {
    pub fee: Capacity,
    pub weight: u64,
    pub ancestors_fee: Capacity,
    pub ancestors_weight: u64,
}

impl AncestorsScoreSortKey {
    /// compare tx fee rate with ancestors fee rate and return the min one
    pub(crate) fn min_fee_and_weight(&self) -> (Capacity, u64) {
        // avoid division a_fee/a_weight > b_fee/b_weight
        let tx_weight = u128::from(self.fee.as_u64()) * u128::from(self.ancestors_weight);
        let ancestors_weight = u128::from(self.ancestors_fee.as_u64()) * u128::from(self.weight);

        if tx_weight < ancestors_weight {
            (self.fee, self.weight)
        } else {
            (self.ancestors_fee, self.ancestors_weight)
        }
    }
}

impl PartialOrd for AncestorsScoreSortKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AncestorsScoreSortKey {
    fn cmp(&self, other: &Self) -> Ordering {
        // avoid division a_fee/a_weight > b_fee/b_weight
        let (fee, weight) = self.min_fee_and_weight();
        let (other_fee, other_weight) = other.min_fee_and_weight();
        let self_weight = u128::from(fee.as_u64()) * u128::from(other_weight);
        let other_weight = u128::from(other_fee.as_u64()) * u128::from(weight);
        if self_weight == other_weight {
            // if fee rate weight is same, then compare with ancestor weight
            self.ancestors_weight.cmp(&other.ancestors_weight)
        } else {
            self_weight.cmp(&other_weight)
        }
    }
}

impl From<AncestorsScoreSortKey> for CoreAncestorsScoreSortKey {
    fn from(val: AncestorsScoreSortKey) -> Self {
        CoreAncestorsScoreSortKey {
            fee: val.fee,
            weight: val.weight,
            ancestors_fee: val.ancestors_fee,
            ancestors_weight: val.ancestors_weight,
        }
    }
}

impl std::fmt::Display for AncestorsScoreSortKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "fee: {:#02X}, ancestors_fee: {:#02X}, weight: {:#02X}, ancestors_weight: {:#02X}",
            self.fee.as_u64(),
            self.ancestors_fee.as_u64(),
            self.weight,
            self.ancestors_weight
        )
    }
}

/// First compare fee_rate, select the smallest fee_rate,
/// and then select the latest timestamp, for eviction,
/// the latest timestamp which also means that the fewer descendants may exist.
#[derive(Eq, PartialEq, Clone, Debug)]
pub struct EvictKey {
    pub fee_rate: FeeRate,
    pub timestamp: u64,
    pub descendants_count: usize,
}

impl PartialOrd for EvictKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EvictKey {
    fn cmp(&self, other: &Self) -> Ordering {
        if self.fee_rate == other.fee_rate {
            if self.descendants_count == other.descendants_count {
                self.timestamp.cmp(&other.timestamp)
            } else {
                self.descendants_count.cmp(&other.descendants_count)
            }
        } else {
            self.fee_rate.cmp(&other.fee_rate)
        }
    }
}
