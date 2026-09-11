//! Independent graph, greedy-policy and lifetime checks for compiled packing.
use super::*;
use crate::authority::model::{Entry, Phase, Source};
use crate::error::Reject;
use ckb_proposal_table::ProposalView;
use ckb_test_chain_utils::MockStore;
use ckb_types::{
    bytes::Bytes,
    core::{TransactionBuilder, cell::ResolvedTransaction},
    packed::{CellDep, CellInput, CellOutput, OutPoint},
    prelude::*,
};
use std::collections::BTreeMap;

fn fixture(
    parents: &[Vec<usize>],
    metrics: impl Fn(usize, usize) -> (u64, u64, u64),
) -> Vec<Arc<Entry>> {
    let mut owners: Vec<Arc<Entry>> = Vec::new();
    for (index, incoming) in parents.iter().enumerate() {
        let mut transaction = TransactionBuilder::default().version(index as u32);
        for &parent in incoming {
            // Distinct child outputs avoid introducing read/spend constraints
            // between siblings. Both kinds of causal edge are represented.
            let point = OutPoint::new(owners[parent].hash(), index as u32);
            transaction = if (index + parent) % 2 == 0 {
                transaction.input(CellInput::new(point, 0))
            } else {
                transaction.cell_dep(CellDep::new_builder().out_point(point).build())
            };
        }
        for _ in parents {
            transaction = transaction
                .output(CellOutput::default())
                .output_data(Bytes::new().pack());
        }
        let transaction = Arc::new(transaction.build());
        let size = transaction.data().serialized_size_in_block();
        let (fee, cycles, arrival) = metrics(index, size);
        let accepted = Accepted {
            transaction: Arc::new(ResolvedTransaction::dummy_resolve(
                transaction.as_ref().clone(),
            )),
            cycles,
            fee: Capacity::shannons(fee),
            size,
            timestamp: index as u64,
            parents: incoming
                .iter()
                .map(|parent| owners[*parent].hash())
                .collect(),
            context_sensitive: false,
            forced_status: Some(Status::Proposed),
        };
        owners.push(Arc::new(Entry {
            transaction,
            arrival,
            source: Source::Local,
            phase: Phase::Accepted(accepted),
        }));
    }
    owners
}

fn dag(len: usize, mask: usize) -> Vec<Vec<usize>> {
    let mut bit = 0;
    (0..len)
        .map(|child| {
            (0..child)
                .filter(|_| {
                    let present = mask & (1 << bit) != 0;
                    bit += 1;
                    present
                })
                .collect()
        })
        .collect()
}

// The oracle derives edges from transaction inputs and resolved dependencies,
// independently of Accepted.parents and every compiled graph field.
fn semantic_parents(owners: &[Arc<Entry>]) -> Vec<Vec<usize>> {
    let indices: BTreeMap<_, _> = owners
        .iter()
        .enumerate()
        .map(|(index, owner)| (owner.hash(), index))
        .collect();
    owners
        .iter()
        .map(|owner| {
            let accepted = owner.accepted().unwrap();
            accepted
                .transaction
                .transaction
                .input_pts_iter()
                .chain(accepted.transaction.related_dep_out_points().cloned())
                .filter_map(|point| indices.get(&point.tx_hash()).copied())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        })
        .collect()
}

fn closure(index: usize, edges: &[Vec<usize>]) -> BTreeSet<usize> {
    let mut seen = BTreeSet::new();
    let mut pending = vec![index];
    while let Some(index) = pending.pop() {
        if seen.insert(index) {
            pending.extend(&edges[index]);
        }
    }
    seen
}

fn totals(owners: &[Arc<Entry>], package: &BTreeSet<usize>) -> (usize, u64, u64) {
    package
        .iter()
        .fold((0, 0, 0), |(bytes, cycles, fee), index| {
            let value = owners[*index].accepted().unwrap();
            (
                bytes + value.size,
                cycles + value.cycles,
                fee + value.fee.as_u64(),
            )
        })
}

fn fits((bytes, cycles, _): (usize, u64, u64), limits: TemplatePackingLimits) -> bool {
    bytes <= limits.serialized_bytes && cycles <= limits.cycles
}

// Compare rational rates directly. Do not use PackageOrderKey, the production
// sort key, or cached aggregates to construct expected choices.
fn preference(
    owners: &[Arc<Entry>],
    left: (usize, &BTreeSet<usize>),
    right: (usize, &BTreeSet<usize>),
) -> Ordering {
    let score = |(index, package): (usize, &BTreeSet<usize>)| {
        let own = owners[index].accepted().unwrap();
        let (bytes, cycles, fee) = totals(owners, package);
        let package_weight = get_transaction_weight(bytes, cycles);
        let own_weight = get_transaction_weight(own.size, own.cycles);
        let own_fee = own.fee.as_u64();
        let (fee, weight) = if u128::from(own_fee) * u128::from(package_weight)
            < u128::from(fee) * u128::from(own_weight)
        {
            (own_fee, own_weight)
        } else {
            (fee, package_weight)
        };
        (u128::from(fee), u128::from(weight), package_weight)
    };
    let (fee, weight, package_weight) = score(left);
    let (other_fee, other_weight, other_package_weight) = score(right);
    (fee * other_weight)
        .cmp(&(other_fee * weight))
        .then_with(|| package_weight.cmp(&other_package_weight))
        .then_with(|| owners[right.0].arrival.cmp(&owners[left.0].arrival))
        .then_with(|| owners[right.0].hash().cmp(&owners[left.0].hash()))
}

fn reference_pack(
    owners: &[Arc<Entry>],
    limits: TemplatePackingLimits,
    failure_bound: usize,
) -> Vec<Byte32> {
    let parents = semantic_parents(owners);
    let mut ordered = Vec::new();
    let mut visited = BTreeSet::new();
    while visited.len() < owners.len() {
        // Global preference ordering by rescanning the ready set: no heap,
        // indegree counter, local closure ordering or graph cache.
        let next = (0..owners.len())
            .filter(|index| !visited.contains(index))
            .filter(|index| {
                parents[*index]
                    .iter()
                    .all(|parent| visited.contains(parent))
            })
            .max_by(|left, right| {
                preference(
                    owners,
                    (*left, &closure(*left, &parents)),
                    (*right, &closure(*right, &parents)),
                )
            })
            .unwrap();
        visited.insert(next);
        ordered.push(next);
    }
    let mut queued: BTreeSet<_> = (0..owners.len())
        .filter(|index| fits(totals(owners, &closure(*index, &parents)), limits))
        .collect();
    let mut selected = BTreeSet::new();
    let mut output = Vec::new();
    let mut failures = 0;
    while !queued.is_empty() {
        // Recompute every remaining package at each choice. A failed root is
        // terminal, as required by the existing bounded greedy policy.
        let remaining = |index| {
            closure(index, &parents)
                .difference(&selected)
                .copied()
                .collect()
        };
        let index = *queued
            .iter()
            .max_by(|left, right| {
                preference(
                    owners,
                    (**left, &remaining(**left)),
                    (**right, &remaining(**right)),
                )
            })
            .unwrap();
        queued.remove(&index);
        let package: BTreeSet<_> = remaining(index);
        let combined = selected.union(&package).copied().collect();
        if !fits(totals(owners, &combined), limits) {
            failures += 1;
            if failures > failure_bound {
                break;
            }
            continue;
        }
        for &member in &ordered {
            if package.contains(&member) {
                output.push(owners[member].hash());
                selected.insert(member);
                queued.remove(&member);
            }
        }
        failures = 0;
    }
    output
}

fn packed(selection: &Selection<'_>, limits: TemplatePackingLimits, bound: usize) -> Vec<Byte32> {
    selection
        .pack_transactions_with_failure_bound(limits, bound)
        .unwrap()
        .iter()
        .map(|entry| entry.transaction().hash())
        .collect()
}

fn assert_graph(owners: &[Arc<Entry>], selection: &Selection<'_>) {
    let parents = semantic_parents(owners);
    for (index, incoming) in parents.iter().enumerate() {
        assert_eq!(
            selection
                .graph
                .parents
                .get(index)
                .unwrap()
                .iter()
                .copied()
                .collect::<BTreeSet<_>>(),
            incoming.iter().copied().collect()
        );
        let expected = closure(index, &parents);
        let (bytes, cycles, fee) = totals(owners, &expected);
        let actual = selection.graph.ancestors[index];
        assert_eq!(actual.entries, expected.len());
        assert_eq!(
            (actual.serialized_bytes, actual.cycles, actual.fee.as_u64()),
            (bytes, cycles, fee)
        );
    }
}

#[test]
fn compiled_packing_matches_stateless_policy_on_every_five_node_dag() {
    let snapshot = crate::test_support::genesis_snapshot();
    for mask in 0..(1 << 10) {
        for pattern in 0..3 {
            let owners = fixture(&dag(5, mask), |index, size| match pattern {
                0 => (size as u64 * 17, size as u64 * 100, 0),
                1 => (
                    [1, 10_000, 1, 100_000_000, 20_000][index],
                    (index as u64 % 3) + 1,
                    index as u64,
                ),
                _ => (
                    (index as u64 * 17 % 7 + 1) * 1000,
                    (index as u64 + 1) * 10_000_000,
                    index as u64 % 2,
                ),
            });
            let selection = Selection::new(&owners, &snapshot, 5).unwrap();
            assert_graph(&owners, &selection);
            let mut reversed = owners.clone();
            reversed.reverse();
            let reversed_selection = Selection::new(&reversed, &snapshot, 5).unwrap();
            assert_graph(&reversed, &reversed_selection);
            let (bytes, cycles, _) = totals(&owners, &(0..5).collect());
            for (bytes, cycles) in [
                (0, 0),
                (bytes, cycles),
                (bytes - 1, cycles),
                (usize::MAX, cycles / 2),
                (bytes / 2, u64::MAX),
                (bytes * 3 / 5, cycles * 3 / 5),
            ] {
                let limits = TemplatePackingLimits::new(bytes, cycles);
                for bound in [0, 1, MAX_CONSECUTIVE_PACKING_FAILURES] {
                    let expected = reference_pack(&owners, limits, bound);
                    assert_eq!(
                        packed(&selection, limits, bound),
                        expected,
                        "mask={mask} pattern={pattern} limits={limits:?} bound={bound}"
                    );
                    assert_eq!(
                        packed(&reversed_selection, limits, bound),
                        expected,
                        "source order must not change selection"
                    );
                }
            }
        }
    }
}

#[test]
fn already_selected_ancestors_still_determine_local_package_order() {
    let owners = fixture(
        &[vec![], vec![], vec![1], vec![0, 2], vec![1]],
        |index, _| {
            (
                [60_000, 1, 100_000_000, 800_000_000, 1_000_000_000][index],
                1,
                index as u64,
            )
        },
    );
    let snapshot = crate::test_support::genesis_snapshot();
    let selection = Selection::new(&owners, &snapshot, 5).unwrap();
    let limits = TemplatePackingLimits::new(usize::MAX, u64::MAX);
    let expected: Vec<_> = [1, 4, 0, 2, 3].map(|index| owners[index].hash()).into();
    assert_eq!(reference_pack(&owners, limits, 4000), expected);
    assert_eq!(packed(&selection, limits, 4000), expected);
}

#[test]
fn residual_chains_preserve_full_closure_order_and_failed_ancestors() {
    let snapshot = crate::test_support::genesis_snapshot();
    let mut chains = 0;
    let mut forks = 0;
    for mask in 0..(1 << 6) {
        let parents = dag(4, mask);
        let owners = fixture(&parents, |index, _| {
            ([1, 100_000, 60_000, 1_000_000][index], 1, index as u64)
        });
        let selection = Selection::new(&owners, &snapshot, 4).unwrap();
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
            for index in 0..4 {
                if states[index] == CandidatePackingState::Selected {
                    continue;
                }
                let mut chain = Vec::new();
                if selection.chain_package(index, &states, &mut chain).unwrap() {
                    let mut expected = Vec::new();
                    selection
                        .ordered_package(
                            index,
                            &states,
                            &mut Traversal::new(4),
                            &mut [0; 4],
                            &mut expected,
                        )
                        .unwrap();
                    assert_eq!(
                        chain, expected,
                        "mask={mask} selected={selected} index={index}"
                    );
                    chains += 1;
                } else {
                    forks += 1;
                }
            }
        }
    }
    assert!(chains > 0 && forks > 0);
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
    let mut states = vec![CandidatePackingState::Selected; 500];
    states.resize(owners.len(), CandidatePackingState::Original);
    let mut package = Vec::new();
    for index in (501..owners.len()).step_by(2) {
        assert!(
            selection
                .chain_package(index, &states, &mut package)
                .unwrap()
        );
        assert_eq!(package, [index - 1, index]);
    }
}

#[test]
fn descendant_updates_cross_selected_intermediates_with_live_consumers() {
    let owners = fixture(&[vec![], vec![0], vec![1], vec![1], vec![]], |index, _| {
        ([1, 1, 1_000_000, 100_000, 60_000][index], 1, index as u64)
    });
    let snapshot = crate::test_support::genesis_snapshot();
    let selection = Selection::new(&owners, &snapshot, 5).unwrap();
    let limits = TemplatePackingLimits::new(usize::MAX, 4);
    let expected: Vec<_> = [0, 1, 2, 3].map(|index| owners[index].hash()).into();
    assert_eq!(reference_pack(&owners, limits, 4000), expected);
    assert_eq!(packed(&selection, limits, 4000), expected);
}

#[test]
fn overlapping_ancestor_closures_preserve_the_thousand_entry_boundary() {
    let mut owners: Vec<Arc<Entry>> = Vec::new();
    for index in 0..1001usize {
        let mut transaction = TransactionBuilder::default()
            .version(index as u32)
            .input(CellInput::new(
                OutPoint::new(Byte32::new([0xee; 32]), index as u32),
                0,
            ))
            .output(CellOutput::default())
            .output_data(Bytes::new().pack());
        for parent in owners.iter().rev().take(2) {
            transaction = transaction.cell_dep(
                CellDep::new_builder()
                    .out_point(OutPoint::new(parent.hash(), 0))
                    .build(),
            );
        }
        let transaction = Arc::new(transaction.build());
        let accepted = Accepted {
            transaction: Arc::new(ResolvedTransaction::dummy_resolve(
                transaction.as_ref().clone(),
            )),
            cycles: index as u64 + 1,
            fee: Capacity::shannons(index as u64 + 1),
            size: transaction.data().serialized_size_in_block(),
            timestamp: index as u64,
            parents: owners
                .iter()
                .rev()
                .take(2)
                .map(|parent| parent.hash())
                .collect(),
            context_sensitive: false,
            forced_status: Some(Status::Proposed),
        };
        owners.push(Arc::new(Entry {
            transaction,
            arrival: index as u64,
            source: Source::Local,
            phase: Phase::Accepted(accepted),
        }));
    }
    let snapshot = crate::test_support::genesis_snapshot();
    let exact = &owners[..1000];
    let expected: Vec<_> = exact.iter().map(|owner| owner.hash()).collect();
    let selection = Selection::new(exact, &snapshot, 1000).unwrap();
    assert_graph(exact, &selection);
    assert_eq!(
        packed(
            &selection,
            TemplatePackingLimits::new(usize::MAX, u64::MAX),
            4000
        ),
        expected
    );
    let mut reversed = exact.to_vec();
    reversed.reverse();
    let selection = Selection::new(&reversed, &snapshot, 1000).unwrap();
    assert_graph(&reversed, &selection);
    assert_eq!(
        packed(
            &selection,
            TemplatePackingLimits::new(usize::MAX, u64::MAX),
            4000
        ),
        expected
    );
    for (owners, limit) in [(exact, 999), (owners.as_slice(), 1000)] {
        assert!(matches!(
            Selection::new(owners, &snapshot, limit),
            Err(Error::Rejected(Reject::ExceededMaximumAncestorsCount))
        ));
    }
    let gap: Vec<_> = owners
        .iter()
        .map(|owner| {
            let mut accepted = owner.accepted().unwrap().clone();
            accepted.forced_status = Some(Status::Gap);
            owner.with_phase(Phase::Accepted(accepted))
        })
        .collect();
    assert!(matches!(
        Selection::new(&gap, &snapshot, 1000),
        Err(Error::Rejected(Reject::ExceededMaximumAncestorsCount))
    ));
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
        let links = Links::from_lists(parents.clone());
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
                                        | CandidatePackingState::Examining
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
                            CandidatePackingState::Examining
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

fn change(owners: &mut [Arc<Entry>], index: usize, update: impl FnOnce(&mut Accepted)) {
    let mut value = owners[index].accepted().unwrap().clone();
    update(&mut value);
    owners[index] = owners[index].with_phase(Phase::Accepted(value));
}

#[test]
fn budget_exhaustion_keeps_a_small_child_after_its_large_parent_is_selected() {
    let snapshot = crate::test_support::genesis_snapshot();
    let mut owners = fixture(&[vec![], vec![0], vec![]], |index, _| {
        ([1_000_000, 100, 1][index], 1, index as u64)
    });
    for (index, witness_size) in [(0, 4096), (2, 1024)] {
        let owner = &owners[index];
        // Witnesses enlarge the parent without changing its children's out points.
        let transaction = Arc::new(
            owner
                .transaction
                .as_ref()
                .clone()
                .as_advanced_builder()
                .witness(Bytes::from(vec![0xaa; witness_size]).pack())
                .build(),
        );
        assert_eq!(transaction.hash(), owner.hash());
        let mut value = owner.accepted().unwrap().clone();
        value.size = transaction.data().serialized_size_in_block();
        value.transaction = Arc::new(ResolvedTransaction::dummy_resolve(
            transaction.as_ref().clone(),
        ));
        owners[index] = Arc::new(Entry {
            transaction,
            arrival: owner.arrival,
            source: owner.source,
            phase: Phase::Accepted(value),
        });
    }
    let bytes = owners[0].accepted().unwrap().size + owners[1].accepted().unwrap().size;
    let limits = TemplatePackingLimits::new(bytes, 10);
    let expected = vec![owners[0].hash(), owners[1].hash()];
    assert_eq!(reference_pack(&owners, limits, 4000), expected);
    for _ in 0..2 {
        let selection = Selection::new(&owners, &snapshot, 3).unwrap();
        assert_eq!(packed(&selection, limits, 4000), expected);
        owners.reverse();
    }
}

#[test]
fn budget_exhaustion_preserves_projected_arithmetic_rejection() {
    let snapshot = crate::test_support::genesis_snapshot();
    for bytes in [true, false] {
        let mut owners = fixture(&[vec![], vec![]], |_, _| (1, 1, 0));
        for index in 0..2 {
            change(&mut owners, index, |value| {
                value.size = if bytes { usize::MAX / 2 + 1 } else { 1 };
                value.cycles = if bytes { 1 } else { u64::MAX / 2 + 1 };
            });
        }
        let selection = Selection::new(&owners, &snapshot, 2).unwrap();
        let limits = TemplatePackingLimits::new(if bytes { usize::MAX } else { 1 }, u64::MAX);
        assert_eq!(
            selection.pack_transactions(limits),
            Err(PackingError::Arithmetic)
        );
    }
}

#[test]
fn conditional_eviction_ranks_match_independent_descendant_sets() {
    let snapshot = crate::test_support::genesis_snapshot();
    for mask in 0..(1 << 10) {
        let mut owners = fixture(&dag(5, mask), |index, size| {
            (
                size as u64 * (index as u64 + 1) * 1000,
                100,
                4 - index as u64,
            )
        });
        for index in 0..owners.len() {
            change(&mut owners, index, |value| {
                value.forced_status =
                    Some([Status::Pending, Status::Gap, Status::Proposed][index % 3]);
            });
        }
        for _ in 0..2 {
            let parents = semantic_parents(&owners);
            let children: Vec<Vec<usize>> = (0..owners.len())
                .map(|parent| {
                    (0..owners.len())
                        .filter(|child| parents[*child].contains(&parent))
                        .collect()
                })
                .collect();
            let expected: Vec<_> = owners
                .iter()
                .enumerate()
                .map(|(index, owner)| {
                    let value = owner.accepted().unwrap();
                    let descendants = closure(index, &children);
                    let (bytes, cycles, fee) = totals(&owners, &descendants);
                    (
                        value.status(&snapshot),
                        FeeRate::calculate(
                            value.fee,
                            get_transaction_weight(value.size, value.cycles),
                        )
                        .max(FeeRate::calculate(
                            Capacity::shannons(fee),
                            get_transaction_weight(bytes, cycles),
                        )),
                        descendants.len(),
                        owner.arrival,
                        owner.hash(),
                    )
                })
                .collect();
            assert_eq!(
                Selection::new(&owners, &snapshot, 5)
                    .unwrap()
                    .eviction_ranks(),
                Ok(expected),
                "mask={mask}"
            );
            owners.reverse();
        }
    }
}

#[test]
fn conditional_eviction_clamps_wide_fees_and_rejects_resource_overflow() {
    let snapshot = crate::test_support::genesis_snapshot();
    let parents = [vec![], vec![0], vec![0]];
    let owners = fixture(&parents, |index, _| {
        (if index == 0 { 1 } else { u64::MAX - 1 }, 1, 0)
    });
    let ranks = Selection::new(&owners, &snapshot, 3)
        .unwrap()
        .eviction_ranks()
        .unwrap();
    let bytes = owners
        .iter()
        .map(|owner| owner.accepted().unwrap().size)
        .sum();
    assert_eq!(
        ranks[0].1,
        FeeRate::calculate(
            Capacity::shannons(u64::MAX),
            get_transaction_weight(bytes, 3)
        )
    );
    assert_eq!(ranks[0].2, 3);
    for bytes in [true, false] {
        let mut owners = fixture(&parents, |_, _| (1, 1, 0));
        for child in 1..3 {
            change(&mut owners, child, |value| {
                if bytes {
                    value.size = usize::MAX / 2;
                } else {
                    value.cycles = u64::MAX / 2 + 1;
                }
            });
        }
        // Every ancestor package fits its integer type; only the root's
        // descendant total overflows, so graph construction must succeed.
        assert_eq!(
            Selection::new(&owners, &snapshot, 3)
                .unwrap()
                .eviction_ranks(),
            Err(PackingError::Arithmetic)
        );
    }
}

#[test]
fn compiled_graph_checks_limits_sources_cycles_and_arithmetic() {
    let snapshot = crate::test_support::genesis_snapshot();
    let chain: Vec<_> = (0..64)
        .map(|index| if index == 0 { vec![] } else { vec![index - 1] })
        .collect();
    let owners = fixture(&chain, |_, _| (1, 1, 0));
    assert_graph(&owners, &Selection::new(&owners, &snapshot, 64).unwrap());
    assert!(matches!(
        Selection::new(&owners, &snapshot, 63),
        Err(Error::Rejected(Reject::ExceededMaximumAncestorsCount))
    ));
    assert!(
        Selection::new(&[], &snapshot, 0)
            .unwrap()
            .candidates
            .is_empty()
    );
    assert!(matches!(
        Selection::new(&owners[..1], &snapshot, 0),
        Err(Error::Rejected(Reject::ExceededMaximumAncestorsCount))
    ));
    assert!(matches!(
        Selection::new(&owners[1..], &snapshot, 64),
        Err(Error::Stale)
    ));
    let mut duplicate = owners[..1].to_vec();
    duplicate.push(Arc::clone(&owners[0]));
    assert!(matches!(
        Selection::new(&duplicate, &snapshot, 64),
        Err(Error::Fault("template graph"))
    ));
    let mut cycle = owners[..2].to_vec();
    let child = cycle[1].hash();
    change(&mut cycle, 0, |value| {
        value.parents.insert(child);
    });
    assert!(matches!(
        Selection::new(&cycle, &snapshot, 64),
        Err(Error::Fault("template graph"))
    ));
    for field in 0..3 {
        let mut overflow = owners[..2].to_vec();
        change(&mut overflow, 0, |value| match field {
            0 => value.size = usize::MAX,
            1 => value.cycles = u64::MAX,
            _ => value.fee = Capacity::shannons(u64::MAX),
        });
        assert!(matches!(
            Selection::new(&overflow, &snapshot, 64),
            Err(Error::Full(_))
        ));
    }
}

#[test]
fn graph_cache_rebuilds_for_replacement_order_limit_and_membership() {
    let snapshot = crate::test_support::genesis_snapshot();
    let mut owners = fixture(&[vec![], vec![0], vec![]], |index, _| {
        ([1, 1, 1000][index], 1, 0)
    });
    let mut cache = Cache::default();
    let limits = TemplatePackingLimits::new(usize::MAX, 2);
    let first = cache.selection(&owners, &snapshot, 3).unwrap();
    assert_eq!(
        packed(&first, limits, 4000),
        reference_pack(&owners, limits, 4000)
    );
    let old_graph = Arc::downgrade(&first.graph);
    assert!(Arc::ptr_eq(
        &first.graph,
        &cache.selection(&owners, &snapshot, 3).unwrap().graph
    ));
    let old_result = packed(&first, limits, 4000);
    drop(first);
    let old_hash = owners[1].hash();
    change(&mut owners, 1, |value| {
        value.fee = Capacity::shannons(100_000_000)
    });
    assert_eq!(owners[1].hash(), old_hash);
    let replacement = cache.selection(&owners, &snapshot, 3).unwrap();
    assert_eq!(
        replacement.pack_transactions(limits).map(|entries| entries
            .iter()
            .map(|entry| entry.transaction().hash())
            .collect::<Vec<_>>()),
        Ok(reference_pack(&owners, limits, 4000))
    );
    assert!(old_graph.upgrade().is_none());
    assert_ne!(packed(&replacement, limits, 4000), old_result);
    drop(replacement);
    owners.reverse();
    let reordered = cache.selection(&owners, &snapshot, 3).unwrap();
    assert_eq!(
        packed(&reordered, limits, 4000),
        reference_pack(&owners, limits, 4000)
    );
    let graph = Arc::downgrade(&reordered.graph);
    drop(reordered);
    assert!(matches!(
        cache.selection(&owners, &snapshot, 1),
        Err(Error::Rejected(Reject::ExceededMaximumAncestorsCount))
    ));
    assert!(graph.upgrade().is_none());
    assert!(cache.graph.is_none());
    cache.selection(&owners, &snapshot, 3).unwrap();
    let graph = Arc::downgrade(&cache.graph.as_ref().unwrap().graph);
    cache.selection(&[], &snapshot, 3).unwrap();
    assert!(graph.upgrade().is_none());
}

#[test]
fn one_time_selection_borrows_owners_without_retaining_source_allocations() {
    let snapshot = crate::test_support::genesis_snapshot();
    let owners = fixture(&[vec![], vec![0]], |_, _| (1, 1, 0));
    let selection = Selection::new(&owners, &snapshot, 2).unwrap();
    for owner in &owners {
        assert_eq!(Arc::strong_count(owner), 1);
        assert_eq!(Arc::weak_count(owner), 0);
    }
    let graph = Arc::clone(&selection.graph);
    let retired = Arc::downgrade(&owners[0]);
    drop(selection);
    drop(owners);
    assert!(retired.upgrade().is_none());
    assert_eq!(graph.parents.len(), 2);
}

#[test]
fn graph_cache_keeps_no_retired_owner_or_resolved_payload_alive() {
    let snapshot = crate::test_support::genesis_snapshot();
    let owners = fixture(&[vec![], vec![0]], |_, _| (1, 1, 0));
    let old_owner = Arc::downgrade(&owners[0]);
    let old_payload = Arc::downgrade(&owners[0].accepted().unwrap().transaction);
    let old_transaction = Arc::downgrade(&owners[0].transaction);
    let mut cache = Cache::default();
    cache.selection(&owners, &snapshot, 2).unwrap();
    let old_graph = Arc::downgrade(&cache.graph.as_ref().unwrap().graph);
    drop(owners);
    assert!(old_owner.upgrade().is_none());
    assert!(old_payload.upgrade().is_none());
    assert!(old_transaction.upgrade().is_none());
    assert!(old_graph.upgrade().is_some());
    drop(cache);
    assert!(old_graph.upgrade().is_none());
}

#[test]
fn cached_graph_uses_the_current_proposal_view_with_unchanged_owners() {
    let base = crate::test_support::genesis_snapshot();
    let store = MockStore::default();
    let mut owners = fixture(&[vec![], vec![0]], |_, _| (1, 1, 0));
    for index in 0..owners.len() {
        change(&mut owners, index, |value| value.forced_status = None);
    }
    let mut cache = Cache::default();
    let mut graph = None;
    for status in [
        Status::Pending,
        Status::Gap,
        Status::Proposed,
        Status::Pending,
    ] {
        let ids = || owners.iter().map(|owner| owner.proposal());
        let proposals = match status {
            Status::Pending => ProposalView::default(),
            Status::Gap => ProposalView::new(ids(), []),
            Status::Proposed => ProposalView::new([], ids()),
        };
        let snapshot = Snapshot::new(
            base.tip_header().clone(),
            base.total_difficulty().clone(),
            base.epoch_ext().clone(),
            store.store().get_snapshot(),
            proposals,
            base.cloned_consensus(),
        );
        let selection = cache.selection(&owners, &snapshot, 2).unwrap();
        match &graph {
            None => graph = Some(Arc::clone(&selection.graph)),
            Some(graph) => assert!(Arc::ptr_eq(graph, &selection.graph)),
        }
        assert!(
            selection
                .candidates
                .iter()
                .all(|candidate| candidate.status == status)
        );
        assert_eq!(
            selection.proposal_short_ids(2).len(),
            if status == Status::Pending { 2 } else { 0 }
        );
        assert_eq!(
            packed(&selection, TemplatePackingLimits::new(usize::MAX, 2), 4000).len(),
            if status == Status::Proposed { 2 } else { 0 }
        );
    }
}

#[test]
fn duplicate_selected_inputs_are_rejected_even_when_no_reordering_is_needed() {
    let snapshot = crate::test_support::genesis_snapshot();
    let mut owners = fixture(&[vec![], vec![]], |_, _| (1, 1, 0));
    let input = CellInput::new(OutPoint::new(Byte32::new([0xff; 32]), 0), 0);
    for owner in &mut owners {
        let transaction = Arc::new(
            owner
                .transaction
                .as_ref()
                .clone()
                .as_advanced_builder()
                .input(input.clone())
                .build(),
        );
        let mut value = owner.accepted().unwrap().clone();
        value.size = transaction.data().serialized_size_in_block();
        value.transaction = Arc::new(ResolvedTransaction::dummy_resolve(
            transaction.as_ref().clone(),
        ));
        *owner = Arc::new(Entry {
            transaction,
            arrival: owner.arrival,
            source: owner.source,
            phase: Phase::Accepted(value),
        });
    }
    let selection = Selection::new(&owners, &snapshot, 2).unwrap();
    assert_eq!(
        selection.pack_transactions(TemplatePackingLimits::new(usize::MAX, 2)),
        Err(PackingError::Projection)
    );
}
