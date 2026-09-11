use super::*;
use crate::authority::{model::Status, tests::common};

fn mixed_selection() -> (Selection, Vec<Byte32>) {
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
    (
        Selection::new(owners, &snapshot, common::config().max_ancestors_count).unwrap(),
        vec![a, b, c, d, e, f],
    )
}

#[test]
fn conditional_graph_subsets_equal_fresh_rebuilds() {
    let (selection, _) = mixed_selection();
    let by_hash = selection.candidate_index().unwrap();
    let count = selection.candidates.len();
    let original = selection
        .conditional_graph(&vec![true; count], &by_hash)
        .unwrap();
    // Every subset, including non-package-closed subsets, checks the stated
    // edge-source invariant independently of the SCC traversal implementation.
    for mask in 0..(1usize << count) {
        let active: Vec<_> = (0..count).map(|index| mask & (1 << index) != 0).collect();
        let rebuilt = selection.conditional_graph(&active, &by_hash).unwrap();
        for (old, new) in [
            (&original.children, &rebuilt.children),
            (&original.package_children, &rebuilt.package_children),
        ] {
            for parent in 0..count {
                let induced: Vec<_> = old[parent]
                    .iter()
                    .copied()
                    .filter(|child| active[parent] && active[*child])
                    .collect();
                assert_eq!(induced, new[parent], "mask={mask} parent={parent}");
            }
        }
    }
}

#[test]
fn reused_graph_drops_multiple_sccs_and_complete_causal_packages() {
    let (selection, hashes) = mixed_selection();
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
    let rank = [Some(1), None, Some(0)];
    let children = vec![vec![1, 2], vec![2], Vec::new()];
    assert_eq!(
        topological_active_order(&active, &rank, &children).unwrap(),
        vec![0, 2]
    );
    let mut components =
        strongly_connected_active(&active, &[vec![1, 2], vec![2], vec![0]]).unwrap();
    components.sort_unstable();
    assert_eq!(components, vec![vec![0, 2]]);
    let mut remaining = active;
    drop_package_descendants(
        &mut remaining,
        vec![true, false, false],
        &[vec![1], vec![2], Vec::new()],
    )
    .unwrap();
    assert_eq!(remaining, [false, false, true]);

    let malformed = vec![vec![3], Vec::new(), Vec::new()];
    assert_eq!(
        topological_active_order(&active, &rank, &malformed),
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
