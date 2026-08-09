//! Finite falsifiers for contractual observations that own no policy state.

use super::contract_observation::{
    accepted_expiry_fault, bounded_prefix_len, cumulative_effect_region_projection,
    exact_operational_projection, indexed_expiry_cut_is_current,
    reservation_capacity_is_sufficient,
};

#[test]
fn model_successful_reservation_and_bounded_prefix_have_exact_observations() {
    for requested in 0usize..=4 {
        for observed_capacity in 0usize..=4 {
            assert_eq!(
                reservation_capacity_is_sufficient(requested, observed_capacity),
                observed_capacity >= requested
            );
        }
    }

    for due in 0usize..=4 {
        for caller_limit in 0usize..=4 {
            for effect_limit in 0usize..=4 {
                let selected = bounded_prefix_len(due, caller_limit, effect_limit);
                assert_eq!(selected, due.min(caller_limit).min(effect_limit));
                assert!(selected <= due);
                assert!(selected <= caller_limit);
                assert!(selected <= effect_limit);
            }
        }
    }
}

#[test]
fn model_expiry_index_producer_is_the_exact_equivalence_premise() {
    let mut distinguished_without_producer_premise = false;
    for owner_deadline in 0u8..=3 {
        for indexed_deadline in 0u8..=3 {
            for cutoff in 0u8..=3 {
                let produced =
                    indexed_expiry_cut_is_current(owner_deadline, indexed_deadline, cutoff);
                let reference_fault =
                    accepted_expiry_fault(owner_deadline, indexed_deadline, cutoff);
                let conjunctive_fault =
                    owner_deadline != indexed_deadline && owner_deadline > cutoff;
                distinguished_without_producer_premise |= reference_fault != conjunctive_fault;
                assert_eq!(
                    produced,
                    owner_deadline == indexed_deadline && indexed_deadline <= cutoff
                );
                if produced {
                    assert!(owner_deadline <= cutoff);
                    assert!(!accepted_expiry_fault(
                        owner_deadline,
                        indexed_deadline,
                        cutoff
                    ));
                    assert_eq!(reference_fault, conjunctive_fault);
                }
            }
        }
    }

    assert!(distinguished_without_producer_premise);
}

#[test]
fn model_operational_metrics_projection_preserves_every_owned_counter() {
    for basis in 0usize..13 {
        let mut kernel = [0usize; 7];
        let mut effects = [0usize; 6];
        if basis < kernel.len() {
            kernel[basis] = basis + 1;
        } else {
            effects[basis - kernel.len()] = basis + 1;
        }
        let projected = exact_operational_projection(kernel, effects);
        for (index, value) in projected.into_iter().enumerate() {
            assert_eq!(value, usize::from(index == basis) * (basis + 1));
        }
    }

    assert_eq!(
        cumulative_effect_region_projection([1, 10], [2, 20], [3, 30]),
        Some([1, 10, 3, 30, 6, 60])
    );
    assert_eq!(
        cumulative_effect_region_projection([usize::MAX, 0], [1, 0], [0, 0]),
        None
    );
}
