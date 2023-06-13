use ckb_types::{core::Capacity, packed::ProposalShortId};
use std::cmp::Ordering;

/// A struct to use as a sorted key
#[derive(Eq, PartialEq, Clone, Debug)]
pub struct AncestorsScoreSortKey {
    pub fee: Capacity,
    pub weight: u64,
    pub id: ProposalShortId,
    pub ancestors_fee: Capacity,
    pub ancestors_weight: u64,
    pub timestamp: u64,
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
            if self.ancestors_weight == other.ancestors_weight {
                if self.timestamp == other.timestamp {
                    self.id.raw_data().cmp(&other.id.raw_data())
                } else {
                    // NOTE: we use timestamp to compare, so the order is reversed
                    self.timestamp.cmp(&other.timestamp).reverse()
                }
            } else {
                self.ancestors_weight.cmp(&other.ancestors_weight)
            }
        } else {
            self_weight.cmp(&other_weight)
        }
    }
}
