use super::*;
use crate::authority::{model::Status, tests::common};
use ckb_snapshot::Snapshot;
use ckb_types::packed::Byte32;
use ckb_types::packed::OutPoint;
use std::{collections::BTreeSet, sync::Arc};

fn mixed_owners() -> (
    Vec<Arc<crate::authority::model::Entry>>,
    Arc<Snapshot>,
    Vec<Byte32>,
) {
    let store = common::store();
    let points: Vec<_> = (31..35)
        .map(|seed| OutPoint::new(Byte32::new([seed; 32]), 0))
        .collect();
    let a = common::spend(6700, &points[..1], &points[1..2]);
    let a = common::accept(&store, a, 1, 1, Status::Proposed);
    let b = common::spend(
        6701,
        &points[1..2],
        &[points[0].clone(), OutPoint::new(a.clone(), 0)],
    );
    let b = common::accept(&store, b, 2, 1, Status::Proposed);
    let c = common::spend(6702, &points[2..3], &points[3..4]);
    let c = common::accept(&store, c, 3, 1, Status::Proposed);
    let d = common::spend(6703, &points[3..4], &points[2..3]);
    let d = common::accept(&store, d, 4, 1, Status::Proposed);
    let e = common::spend(6704, &[], &[OutPoint::new(b.clone(), 0)]);
    let e = common::accept(&store, e, 5, 1, Status::Proposed);
    let f = common::accept(&store, common::output_tx(6705), 6, 1, Status::Proposed);
    let (_, snapshot, owners, _) = store.capture(true);
    (owners, snapshot, vec![a, b, c, d, e, f])
}

#[test]
fn precedence_subsets_match_transaction_relationships() {
    let (owners, snapshot, _) = mixed_owners();
    let selection =
        Selection::new(&owners, &snapshot, common::config().max_ancestors_count).unwrap();
    let count = selection.candidates.len();
    let original = selection.precedence_graph().unwrap();
    // Every subset, including non-package-closed subsets, checks the stated
    // edge-source invariant independently of the SCC traversal implementation.
    for mask in 0..(1usize << count) {
        let active: Vec<_> = (0..count).map(|index| mask & (1 << index) != 0).collect();
        for parent in 0..count {
            let induced: Vec<_> = original
                .get(parent)
                .unwrap()
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
    let (owners, snapshot, hashes) = mixed_owners();
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
fn inactive_endpoints_are_skipped_but_out_of_range_edges_are_rejected() {
    let active = [true, false, true];
    let priority = |index| Reverse([1, 2, 0][index]);
    let children = Links::from_lists([vec![1, 2], vec![2], Vec::new()]);
    assert_eq!(
        topological_active_order(&active, &children, priority).unwrap(),
        vec![0, 2]
    );
    let mut components =
        strongly_connected_active(&active, &Links::from_lists([vec![1, 2], vec![2], vec![0]]))
            .unwrap();
    components.sort_unstable();
    assert_eq!(components, vec![vec![0, 2]]);
    let mut remaining = active;
    drop_package_descendants(
        &mut remaining,
        vec![true, false, false],
        &Links::from_lists([vec![1], vec![2], Vec::new()]),
    )
    .unwrap();
    assert_eq!(remaining, [false, false, true]);

    let malformed = Links::from_lists([vec![3], Vec::new(), Vec::new()]);
    assert_eq!(
        topological_active_order(&active, &malformed, priority),
        Err(PackingError::Projection)
    );
    assert_eq!(
        strongly_connected_active(&active, &malformed),
        Err(PackingError::Projection)
    );
    let mut remaining = active;
    assert_eq!(
        drop_package_descendants(&mut remaining, vec![true, false, false], &malformed),
        Err(PackingError::Projection)
    );
    assert_eq!(remaining, active);
}
