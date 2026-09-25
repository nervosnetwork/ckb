use super::super::graph_tests::{closure, dag, fixture, reference_order};
use super::*;

#[test]
fn residual_packages_preserve_full_closure_order_and_failed_ancestors() {
    let snapshot = crate::test_support::genesis_snapshot();
    for mask in 0..(1 << 6) {
        let parents = dag(4, mask);
        let owners = fixture(&parents, |index, _| {
            ([1, 100_000, 60_000, 1_000_000][index], 1, index as u64)
        });
        let selection = Selection::new(&owners, &snapshot, 4).unwrap();
        let ordered = reference_order(&owners, &parents);
        for selected in 0..(1 << 4) {
            // Production selection is ancestor-closed. Include Failed states:
            // a previously non-fitting ancestor can still be pulled by a child.
            if (0..4).any(|index| {
                selected & (1 << index) != 0
                    && parents[index]
                        .iter()
                        .any(|parent| selected & (1 << parent) == 0)
            }) {
                continue;
            }
            let states: Vec<_> = (0..4)
                .map(|index| {
                    if selected & (1 << index) != 0 {
                        CandidatePackingState::Selected
                    } else if index % 2 == 0 {
                        CandidatePackingState::Failed
                    } else {
                        CandidatePackingState::Original
                    }
                })
                .collect();
            let limits = TemplatePackingLimits::new(usize::MAX, u64::MAX);
            let mut run = PackingRun::new(&selection, limits).unwrap().unwrap();
            run.states.clone_from(&states);
            for index in 0..4 {
                if states[index] == CandidatePackingState::Selected {
                    continue;
                }
                let actual = run.collect_package(index, limits).unwrap().unwrap();
                let required = closure(index, &parents);
                let expected: Vec<_> = ordered
                    .iter()
                    .copied()
                    .filter(|member| {
                        required.contains(member)
                            && states[*member] != CandidatePackingState::Selected
                    })
                    .collect();
                assert_eq!(
                    run.package, expected,
                    "mask={mask} selected={selected} index={index}"
                );
                assert_eq!(actual.entries, expected.len());
            }
        }
    }
}

#[test]
fn residual_chain_omits_a_large_selected_shared_prefix() {
    let mut parents = vec![vec![]];
    for index in 1..500 {
        parents.push(vec![index - 1]);
    }
    for _ in 0..8 {
        parents.push(vec![499]);
        parents.push(vec![parents.len() - 1]);
    }
    let owners = fixture(&parents, |index, _| (index as u64 + 1, 1, index as u64));
    let snapshot = crate::test_support::genesis_snapshot();
    let selection = Selection::new(&owners, &snapshot, 1000).unwrap();
    let limits = TemplatePackingLimits::new(usize::MAX, u64::MAX);
    let mut run = PackingRun::new(&selection, limits).unwrap().unwrap();
    run.states = vec![CandidatePackingState::Selected; 500];
    run.states
        .resize(owners.len(), CandidatePackingState::Original);
    for index in (501..owners.len()).step_by(2) {
        let aggregate = run.collect_package(index, limits).unwrap().unwrap();
        assert_eq!(aggregate.entries, 2);
        assert_eq!(run.package, [index - 1, index]);
    }
}

#[test]
fn retired_child_counts_match_fresh_reachability_in_every_four_node_dag() {
    let mut orders = Vec::new();
    for a in 0..4 {
        for b in 0..4 {
            for c in 0..4 {
                for d in 0..4 {
                    if BTreeSet::from([a, b, c, d]).len() == 4 {
                        orders.push([a, b, c, d]);
                    }
                }
            }
        }
    }
    for mask in 0..(1 << 6) {
        let parents = dag(4, mask);
        let links = Links::from_lists(parents.clone()).unwrap();
        let children: Vec<Vec<usize>> = (0..4)
            .map(|parent| {
                (0..4)
                    .filter(|child| parents[*child].contains(&parent))
                    .collect()
            })
            .collect();
        let reference = |states: &[CandidatePackingState]| -> Vec<usize> {
            children
                .iter()
                .map(|children_of_parent| {
                    children_of_parent
                        .iter()
                        .filter(|child| {
                            closure(**child, &children).iter().any(|index| {
                                matches!(
                                    states[*index],
                                    CandidatePackingState::Original
                                        | CandidatePackingState::Modified
                                )
                            })
                        })
                        .count()
                })
                .collect()
        };
        for queued in 0..16 {
            for order in &orders {
                let mut states: Vec<_> = (0..4)
                    .map(|index| {
                        if queued & (1 << index) == 0 {
                            CandidatePackingState::Ineligible
                        } else if index % 2 == 0 {
                            CandidatePackingState::Original
                        } else {
                            CandidatePackingState::Modified
                        }
                    })
                    .collect();
                let mut live = reference(&states);
                let mut stack = Vec::new();
                for &index in order {
                    if queued & (1 << index) != 0 {
                        states[index] = if index % 2 == 0 {
                            CandidatePackingState::Selected
                        } else {
                            CandidatePackingState::Failed
                        };
                        retire_candidate(index, &states, &mut live, &links, &mut stack).unwrap();
                        assert_eq!(
                            live,
                            reference(&states),
                            "mask={mask} queued={queued} order={order:?} index={index}"
                        );
                    }
                }
                assert_eq!(live, vec![0; 4]);
            }
        }
    }
}
