use super::eviction_quotient::{
    EvictionRefinementInput, EvictionRefinementMetrics, EvictionRefinementStatus,
    eviction_observation, eviction_status_witness,
};

fn input(
    status: EvictionRefinementStatus,
    own: EvictionRefinementMetrics,
    descendants: EvictionRefinementMetrics,
    descendants_count: usize,
    arrival: u128,
    identity: u8,
) -> EvictionRefinementInput {
    let mut hash = [0; 32];
    hash[31] = identity;
    EvictionRefinementInput {
        status: eviction_status_witness(status),
        own,
        descendants,
        descendants_count,
        arrival,
        identity: hash,
    }
}

#[test]
fn model_eviction_weight_and_fee_rate_preserve_ckb_rounding_and_saturation() {
    let size_dominated = eviction_observation(input(
        EvictionRefinementStatus::Pending,
        EvictionRefinementMetrics::new(1, 3, 0),
        EvictionRefinementMetrics::new(1, 3, 0),
        1,
        1,
        1,
    ));
    assert_eq!(size_dominated.fee_rate, 333);

    let cycle_dominated = eviction_observation(input(
        EvictionRefinementStatus::Pending,
        EvictionRefinementMetrics::new(596, 4, 3_500_000),
        EvictionRefinementMetrics::new(596, 4, 3_500_000),
        1,
        1,
        1,
    ));
    assert_eq!(cycle_dominated.fee_rate, 1_000);

    let saturated = eviction_observation(input(
        EvictionRefinementStatus::Pending,
        EvictionRefinementMetrics::new(u64::MAX, 1, 0),
        EvictionRefinementMetrics::new(u64::MAX, 1, 0),
        1,
        1,
        1,
    ));
    assert_eq!(saturated.fee_rate, u64::MAX);
}

#[test]
fn model_eviction_uses_the_stronger_of_self_and_descendant_fee_rate() {
    let self_dominates = eviction_observation(input(
        EvictionRefinementStatus::Pending,
        EvictionRefinementMetrics::new(10, 4, 0),
        EvictionRefinementMetrics::new(11, 8, 0),
        2,
        1,
        1,
    ));
    assert_eq!(self_dominates.fee_rate, 2_500);

    let descendants_dominate = eviction_observation(input(
        EvictionRefinementStatus::Pending,
        EvictionRefinementMetrics::new(10, 8, 0),
        EvictionRefinementMetrics::new(30, 12, 0),
        2,
        1,
        1,
    ));
    assert_eq!(descendants_dominate.fee_rate, 2_500);
}

#[test]
fn model_eviction_key_is_the_exact_status_rate_count_arrival_identity_order() {
    let base_metrics = EvictionRefinementMetrics::new(10, 4, 0);
    let pending = eviction_observation(input(
        EvictionRefinementStatus::Pending,
        base_metrics,
        base_metrics,
        1,
        1,
        1,
    ));
    let gap = eviction_observation(input(
        EvictionRefinementStatus::Gap,
        base_metrics,
        base_metrics,
        1,
        1,
        1,
    ));
    let proposed = eviction_observation(input(
        EvictionRefinementStatus::Proposed,
        base_metrics,
        base_metrics,
        1,
        1,
        1,
    ));
    assert!(pending < gap && gap < proposed);

    let lower_rate = eviction_observation(input(
        EvictionRefinementStatus::Pending,
        EvictionRefinementMetrics::new(9, 4, 0),
        EvictionRefinementMetrics::new(9, 4, 0),
        usize::MAX,
        u128::MAX,
        u8::MAX,
    ));
    assert!(lower_rate < pending);

    let more_descendants = eviction_observation(input(
        EvictionRefinementStatus::Pending,
        base_metrics,
        base_metrics,
        2,
        0,
        0,
    ));
    assert!(pending < more_descendants);

    let later = eviction_observation(input(
        EvictionRefinementStatus::Pending,
        base_metrics,
        base_metrics,
        1,
        2,
        0,
    ));
    assert!(pending < later);

    let higher_identity = eviction_observation(input(
        EvictionRefinementStatus::Pending,
        base_metrics,
        base_metrics,
        1,
        1,
        2,
    ));
    assert!(pending < higher_identity);
}
