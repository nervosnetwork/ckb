use super::*;
use crate::authority::store::Captured;
use crate::authority::{model::Status, tests::common};
use ckb_snapshot::Snapshot;
use ckb_types::packed::Byte32;
use ckb_types::packed::OutPoint;
use std::{collections::BTreeSet, sync::Arc};

fn mixed_owners(
    status: Status,
) -> (
    Vec<Arc<crate::authority::model::Entry>>,
    Arc<Snapshot>,
    Vec<Byte32>,
) {
    let store = common::store();
    let points: Vec<_> = (31..35)
        .map(|seed| OutPoint::new(Byte32::new([seed; 32]), 0))
        .collect();
    let a = common::spend(6700, &points[..1], &points[1..2]);
    let a = common::accept(&store, a, 1, 1, status);
    let b = common::spend(
        6701,
        &points[1..2],
        &[points[0].clone(), OutPoint::new(a.clone(), 0)],
    );
    let b = common::accept(&store, b, 2, 1, status);
    let c = common::spend(6702, &points[2..3], &points[3..4]);
    let c = common::accept(&store, c, 3, 1, status);
    let d = common::spend(6703, &points[3..4], &points[2..3]);
    let d = common::accept(&store, d, 4, 1, status);
    let e = common::spend(6704, &[], &[OutPoint::new(b.clone(), 0)]);
    let e = common::accept(&store, e, 5, 1, status);
    let f = common::accept(&store, common::output_tx(6705), 6, 1, status);
    let Captured {
        snapshot, owners, ..
    } = store.capture_accepted();
    (owners, snapshot, vec![a, b, c, d, e, f])
}

#[test]
fn precedence_subsets_match_transaction_relationships() {
    let (owners, snapshot, _) = mixed_owners(Status::Proposed);
    let selection =
        Selection::new(&owners, &snapshot, common::config().max_ancestors_count).unwrap();
    let count = selection.candidates.len();
    let original = selection.precedence_graph().unwrap();
    // Every subset, including non-package-closed subsets, checks the stated
    // edge-source invariant independently of the SCC traversal implementation.
    for mask in 0..(1usize << count) {
        let active: Vec<_> = (0..count).map(|index| mask & (1 << index) != 0).collect();
        for parent in 0..count {
            let induced: Vec<_> = original[parent]
                .iter()
                .copied()
                .filter(|child| active[parent] && active[*child])
                .collect();
            let reader = owners[parent].accepted().unwrap();
            let expected: Vec<_> = (0..count)
                .filter(|child| {
                    if !active[parent] || !active[*child] || parent == *child {
                        return false;
                    }
                    let consumer = owners[*child].accepted().unwrap();
                    let causal = consumer
                        .transaction
                        .transaction
                        .input_pts_iter()
                        .chain(consumer.transaction.related_dep_out_points().cloned())
                        .any(|point| point.tx_hash() == owners[parent].hash());
                    let read_before_spend =
                        reader
                            .transaction
                            .related_dep_out_points()
                            .any(|dependency| {
                                consumer
                                    .transaction
                                    .transaction
                                    .input_pts_iter()
                                    .any(|input| input == *dependency)
                            });
                    causal || read_before_spend
                })
                .collect();
            assert_eq!(induced, expected, "mask={mask} parent={parent}");
        }
    }
}

#[test]
fn reused_graph_drops_multiple_sccs_and_complete_causal_packages() {
    let (owners, snapshot, hashes) = mixed_owners(Status::Proposed);
    let selection =
        Selection::new(&owners, &snapshot, common::config().max_ancestors_count).unwrap();
    let packed = selection
        .pack_transactions(super::super::TemplatePackingLimits::new(
            usize::MAX,
            u64::MAX,
        ))
        .unwrap();
    let actual: BTreeSet<_> = packed
        .iter()
        .map(|entry| entry.transaction().hash())
        .collect();
    // B is the package leaf in A<->B and E depends on B. C loses to D in
    // the independent C<->D SCC. A, D and F remain complete legal packages.
    let expected = BTreeSet::from([hashes[0].clone(), hashes[3].clone(), hashes[5].clone()]);
    assert_eq!(actual, expected);
}

#[test]
fn cycle_round_boundary_uses_causal_leaves_then_roots_and_keeps_first_ties() {
    let (owners, snapshot, hashes) = mixed_owners(Status::Proposed);
    let selection =
        Selection::new(&owners, &snapshot, common::config().max_ancestors_count).unwrap();
    let index = |hash: &Byte32| {
        selection
            .candidates
            .iter()
            .position(|candidate| candidate.hash() == hash)
            .unwrap()
    };
    let a = index(&hashes[0]);
    let b = index(&hashes[1]);
    let mut causal = vec![a, b];
    causal.sort_unstable();
    let mut independent = vec![index(&hashes[2]), index(&hashes[3])];
    independent.sort_unstable();
    let components = vec![causal, independent.clone()];
    // Synthetic equal ranks exercise first-in-component selection independently
    // of the unique hash tie-breaker used by ordinary accepted candidates.
    let mut ranks = selection.eviction_ranks().unwrap();
    let equal_rank = ranks[0].clone();
    ranks.fill(equal_rank);
    for round in [1, MAX_CONDITIONAL_CYCLE_ROUNDS] {
        assert_eq!(
            selection
                .cycle_drop_roots(&ranks, components.clone(), round)
                .unwrap(),
            vec![b, independent[0]],
            "ordinary round {round} drops the causal leaf and first tied leaf"
        );
    }
    assert_eq!(
        selection
            .cycle_drop_roots(&ranks, components, MAX_CONDITIONAL_CYCLE_ROUNDS + 1)
            .unwrap(),
        vec![b, independent[1]],
        "fallback retains causal root A and the first tied independent root"
    );
}

#[test]
fn replay_retains_conditional_cycles_in_causal_order_in_every_phase() {
    for phase in [Status::Pending, Status::Gap, Status::Proposed] {
        let (owners, snapshot, hashes) = mixed_owners(phase);
        let selection =
            Selection::new(&owners, &snapshot, common::config().max_ancestors_count).unwrap();
        let replay = selection.replay_transactions().unwrap();
        assert_eq!(replay.len(), owners.len());
        for owner in &owners {
            assert!(replay.contains(owner.transaction.as_ref()));
        }
        let position = |hash: &Byte32| {
            replay
                .iter()
                .position(|transaction| transaction.hash() == *hash)
                .unwrap()
        };
        // Packing drops B, C and E, but persistence retains both conditional
        // cycles and still orders the complete causal chain A -> B -> E.
        assert!(position(&hashes[0]) < position(&hashes[1]));
        assert!(position(&hashes[1]) < position(&hashes[4]));
    }
}

#[test]
fn active_graph_operations_skip_inactive_nodes_and_reject_mismatched_masks() {
    let active = [true, false, true];
    let priority = |index| Reverse([1, 2, 0][index]);
    let children = Links::from_lists([vec![1, 2], vec![2], Vec::new()]).unwrap();
    assert_eq!(
        children
            .reversed(&active)
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        vec![&[][..], &[][..], &[0][..]]
    );
    assert!(matches!(
        children.reversed(&active[..2]),
        Err(PackingError::Projection)
    ));
    assert_eq!(
        topological_active_order(&active, &children, priority).unwrap(),
        vec![0, 2]
    );
    let mut components = strongly_connected_active(
        &active,
        &Links::from_lists([vec![1, 2], vec![2], vec![0]]).unwrap(),
    )
    .unwrap();
    components.sort_unstable();
    assert_eq!(components, vec![vec![0, 2]]);
    let mut remaining = active;
    drop_package_descendants(
        &mut remaining,
        vec![0],
        &Links::from_lists([vec![1], vec![2], Vec::new()]).unwrap(),
    )
    .unwrap();
    assert_eq!(remaining, [false, false, true]);

    assert_eq!(
        topological_active_order(&active[..2], &children, priority),
        Err(PackingError::Projection)
    );
    assert_eq!(
        strongly_connected_active(&active[..2], &children),
        Err(PackingError::Projection)
    );
    for roots in [vec![], vec![0, 1], vec![0, 3], vec![0, usize::MAX]] {
        let mut remaining = active;
        assert_eq!(
            drop_package_descendants(&mut remaining, roots, &children),
            Err(PackingError::Projection)
        );
        assert_eq!(remaining, active, "all roots are validated before mutation");
    }
    let mut remaining = active[..2].to_vec();
    assert_eq!(
        drop_package_descendants(&mut remaining, vec![0], &children),
        Err(PackingError::Projection)
    );
    assert_eq!(remaining, active[..2]);
}

#[test]
fn graph_construction_rejects_invalid_endpoints_and_preserves_valid_edges() {
    for edge in [(3, 0), (0, 3), (usize::MAX, 0), (0, usize::MAX)] {
        assert!(matches!(
            Links::from_edges(3, &[edge]),
            Err(PackingError::Projection)
        ));
    }
    assert!(matches!(
        Links::from_edges(0, &[(0, 0)]),
        Err(PackingError::Projection)
    ));
    assert!(matches!(
        Links::from_lists([vec![3], Vec::new(), Vec::new()]),
        Err(PackingError::Projection)
    ));
    let empty = Links::from_edges(0, &[]).unwrap();
    assert_eq!(empty.len(), 0);
    assert_eq!(empty.iter().count(), 0);
    assert_eq!(empty.reversed(&[]).unwrap().len(), 0);

    // Isolated nodes, parallel edges and self edges are valid adjacency. The
    // causal graph and precedence policy decide whether cycles are acceptable.
    let links = Links::from_edges(3, &[(2, 1), (0, 0), (2, 1), (0, 2)]).unwrap();
    assert_eq!(links.len(), 3);
    assert_eq!(
        links.iter().collect::<Vec<_>>(),
        vec![&[0, 2][..], &[][..], &[1, 1][..]]
    );
    assert_eq!(
        links
            .reversed(&[true; 3])
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        vec![&[0][..], &[2, 2][..], &[0][..]]
    );
}

#[test]
fn small_graphs_match_reachability_and_ready_node_oracles() {
    // Enumerate every directed three-node graph and active subset, including
    // cycles, self edges and isolated nodes. The oracles use a reachability
    // matrix and repeated ready-node search, independently of compact adjacency,
    // DFS finishing order and the production indegree heap.
    const N: usize = 3;
    for edge_bits in 0..1 << (N * N) {
        let edges: Vec<_> = (0..N)
            .flat_map(|from| (0..N).map(move |to| (from, to)))
            .filter(|(from, to)| edge_bits & (1 << (from * N + to)) != 0)
            .collect();
        let graph = Links::from_edges(N, &edges).unwrap();
        for mask in 0..1 << N {
            let active: [bool; N] = std::array::from_fn(|index| mask & (1 << index) != 0);
            let mut reachable = [[false; N]; N];
            for index in 0..N {
                reachable[index][index] = active[index];
            }
            for &(from, to) in &edges {
                reachable[from][to] = active[from] && active[to];
            }
            for via in 0..N {
                for from in 0..N {
                    for to in 0..N {
                        reachable[from][to] |= reachable[from][via] && reachable[via][to];
                    }
                }
            }

            let reversed = graph.reversed(&active).unwrap();
            for to in 0..N {
                let expected: Vec<_> = (0..N)
                    .filter(|from| active[*from] && active[to] && edges.contains(&(*from, to)))
                    .collect();
                assert_eq!(reversed[to], expected, "edges={edge_bits} mask={mask}");
            }

            let mut remaining = active;
            let mut expected_order = Vec::new();
            while let Some(next) = (0..N).find(|to| {
                remaining[*to]
                    && edges
                        .iter()
                        .all(|(from, end)| end != to || !remaining[*from])
            }) {
                remaining[next] = false;
                expected_order.push(next);
            }
            assert_eq!(
                topological_active_order(&active, &graph, Reverse).unwrap(),
                expected_order,
                "edges={edge_bits} mask={mask}"
            );

            let mut expected_components = Vec::new();
            let mut assigned = [false; N];
            for start in 0..N {
                if !active[start] || assigned[start] {
                    continue;
                }
                let component: Vec<_> = (0..N)
                    .filter(|to| reachable[start][*to] && reachable[*to][start])
                    .collect();
                for &member in &component {
                    assigned[member] = true;
                }
                expected_components.push(component);
            }
            let mut components = strongly_connected_active(&active, &graph).unwrap();
            components.sort_unstable();
            assert_eq!(
                components, expected_components,
                "edges={edge_bits} mask={mask}"
            );

            // Every nonempty active root subset includes overlapping roots,
            // shared descendants, and roots reached through another root.
            for roots_mask in 1..1 << N {
                if roots_mask & mask != roots_mask {
                    continue;
                }
                let roots: Vec<_> = (0..N)
                    .filter(|root| roots_mask & (1 << root) != 0)
                    .collect();
                let expected: [bool; N] = std::array::from_fn(|node| {
                    active[node] && roots.iter().all(|root| !reachable[*root][node])
                });
                let mut remaining = active;
                drop_package_descendants(&mut remaining, roots, &graph).unwrap();
                assert_eq!(
                    remaining, expected,
                    "edges={edge_bits} mask={mask} roots={roots_mask}"
                );
            }
        }
    }
}
