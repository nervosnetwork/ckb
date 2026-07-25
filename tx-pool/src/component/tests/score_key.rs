use ckb_types::{
    bytes::Bytes,
    core::{Capacity, TransactionBuilder},
    packed::{CellDep, CellInput, CellOutput, OutPoint},
    prelude::*,
};

use crate::component::{entry::TxEntry, pool_map::PoolMap, sort_key::AncestorsScoreSortKey};
use std::collections::HashSet;

const DEFAULT_MAX_ANCESTORS_COUNT: usize = 125;

#[test]
fn test_min_fee_and_weight() {
    let result = vec![
        (0, 0, 0, 0),
        (1, 0, 1, 0),
        (500, 10, 1000, 30),
        (10, 500, 30, 1000),
        (500, 10, 1000, 20),
        (u64::MAX, 0, u64::MAX, 0),
        (u64::MAX, 100, u64::MAX, 2000),
        (u64::MAX, u64::MAX, u64::MAX, u64::MAX),
    ]
    .into_iter()
    .map(|(fee, weight, ancestors_fee, ancestors_weight)| {
        let key = AncestorsScoreSortKey {
            fee: Capacity::shannons(fee),
            weight,
            ancestors_fee: Capacity::shannons(ancestors_fee),
            ancestors_weight,
        };
        key.min_fee_and_weight()
    })
    .collect::<Vec<_>>();
    assert_eq!(
        result,
        vec![
            (Capacity::shannons(0), 0),
            (Capacity::shannons(1), 0),
            (Capacity::shannons(1000), 30),
            (Capacity::shannons(10), 500),
            (Capacity::shannons(1000), 20),
            (Capacity::shannons(u64::MAX), 0),
            (Capacity::shannons(u64::MAX), 2000),
            (Capacity::shannons(u64::MAX), u64::MAX),
        ]
    );
}

#[test]
fn test_ancestors_sorted_key_order() {
    let table = vec![
        (0, 0, 0, 0),
        (1, 0, 1, 0),
        (500, 10, 1000, 30),
        (10, 500, 30, 1000),
        (500, 10, 1000, 30),
        (10, 500, 30, 1000),
        (500, 10, 1000, 20),
        (u64::MAX, 0, u64::MAX, 0),
        (u64::MAX, 100, u64::MAX, 2000),
        (u64::MAX, u64::MAX, u64::MAX, u64::MAX),
    ];
    let mut keys = table
        .clone()
        .into_iter()
        .map(
            |(fee, weight, ancestors_fee, ancestors_weight)| AncestorsScoreSortKey {
                fee: Capacity::shannons(fee),
                weight,
                ancestors_fee: Capacity::shannons(ancestors_fee),
                ancestors_weight,
            },
        )
        .collect::<Vec<_>>();
    keys.sort();
    let now = keys
        .into_iter()
        .map(|k| (k.fee, k.weight, k.ancestors_fee, k.ancestors_weight))
        .collect::<Vec<_>>();
    let expect = [0, 3, 5, 9, 2, 4, 6, 8, 1, 7]
        .iter()
        .map(|&i| {
            let key = table[i as usize];
            (
                Capacity::shannons(key.0),
                key.1,
                Capacity::shannons(key.2),
                key.3,
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(now, expect);
}

#[test]
fn test_remove_entry() {
    let mut map = PoolMap::new(DEFAULT_MAX_ANCESTORS_COUNT);
    let tx1 = TxEntry::dummy_resolve(
        TransactionBuilder::default().build(),
        100,
        Capacity::shannons(100),
        100,
    );
    let tx2 = TxEntry::dummy_resolve(
        TransactionBuilder::default()
            .input(
                CellInput::new_builder()
                    .previous_output(
                        OutPoint::new_builder()
                            .tx_hash(tx1.transaction().hash())
                            .index(0u32)
                            .build(),
                    )
                    .build(),
            )
            .witness(Bytes::new())
            .build(),
        200,
        Capacity::shannons(200),
        200,
    );
    let tx3 = TxEntry::dummy_resolve(
        TransactionBuilder::default()
            .input(
                CellInput::new_builder()
                    .previous_output(
                        OutPoint::new_builder()
                            .tx_hash(tx2.transaction().hash())
                            .index(0u32)
                            .build(),
                    )
                    .build(),
            )
            .witness(Bytes::new())
            .build(),
        200,
        Capacity::shannons(200),
        200,
    );
    let tx1_id = tx1.proposal_short_id();
    let tx2_id = tx2.proposal_short_id();
    let tx3_id = tx3.proposal_short_id();
    map.add_proposed(tx1).unwrap();
    map.add_proposed(tx2).unwrap();
    map.add_proposed(tx3).unwrap();
    map.audit().unwrap();
    let descendants_set = map.calc_descendants(&tx1_id);
    assert!(descendants_set.contains(&tx2_id));
    assert!(descendants_set.contains(&tx3_id));

    let tx3_entry = map.get(&tx3_id);
    assert!(tx3_entry.is_some());
    let tx3_entry = tx3_entry.unwrap();
    assert_eq!(tx3_entry.ancestors_count, 3);

    map.remove_entry(&tx1_id);
    map.audit().unwrap();
    assert!(!map.contains_key(&tx1_id));
    assert!(map.contains_key(&tx2_id));
    assert!(map.contains_key(&tx3_id));

    let tx3_entry = map.get(&tx3_id).unwrap();
    assert_eq!(tx3_entry.ancestors_count, 2);
    assert_eq!(
        map.calc_ancestors(&tx3_id),
        vec![tx2_id].into_iter().collect()
    );
}

/// Reverse-index membership is set-valued even when resolution yields the
/// same dep out-point more than once. Removal must consume the canonical
/// membership once instead of replaying duplicate source occurrences.
#[test]
fn duplicate_deps_publish_and_remove_one_index_membership() {
    let mut map = PoolMap::new(DEFAULT_MAX_ANCESTORS_COUNT);
    let out_point = OutPoint::new(ckb_types::packed::Byte32::new([0x42; 32]), 0);
    let dep = CellDep::new_builder().out_point(out_point.clone()).build();
    let entry = TxEntry::dummy_resolve(
        TransactionBuilder::default()
            .cell_dep(dep.clone())
            .cell_dep(dep)
            .build(),
        100,
        Capacity::shannons(100),
        100,
    );
    let id = entry.proposal_short_id();

    map.add_proposed(entry).unwrap();
    map.audit().unwrap();
    assert_eq!(
        map.out_point_index.get_deps_ref(&out_point),
        Some(&HashSet::from([id.clone()]))
    );

    map.remove_entry(&id).expect("accepted entry is removable");
    map.audit().unwrap();
    assert!(map.out_point_index.get_deps_ref(&out_point).is_none());
}

#[test]
fn test_remove_entry_and_descendants() {
    let mut map = PoolMap::new(DEFAULT_MAX_ANCESTORS_COUNT);
    let tx1 = TxEntry::dummy_resolve(
        TransactionBuilder::default().build(),
        100,
        Capacity::shannons(100),
        100,
    );
    let tx2 = TxEntry::dummy_resolve(
        TransactionBuilder::default()
            .input(
                CellInput::new_builder()
                    .previous_output(
                        OutPoint::new_builder()
                            .tx_hash(tx1.transaction().hash())
                            .index(0u32)
                            .build(),
                    )
                    .build(),
            )
            .witness(Bytes::new())
            .build(),
        200,
        Capacity::shannons(200),
        200,
    );
    let tx3 = TxEntry::dummy_resolve(
        TransactionBuilder::default()
            .input(
                CellInput::new_builder()
                    .previous_output(
                        OutPoint::new_builder()
                            .tx_hash(tx2.transaction().hash())
                            .index(0u32)
                            .build(),
                    )
                    .build(),
            )
            .witness(Bytes::new())
            .build(),
        200,
        Capacity::shannons(200),
        200,
    );
    let tx1_id = tx1.proposal_short_id();
    let tx2_id = tx2.proposal_short_id();
    let tx3_id = tx3.proposal_short_id();
    map.add_proposed(tx1).unwrap();
    map.add_proposed(tx2).unwrap();
    map.add_proposed(tx3).unwrap();
    map.audit().unwrap();
    let descendants_set = map.calc_descendants(&tx1_id);
    assert!(descendants_set.contains(&tx2_id));
    assert!(descendants_set.contains(&tx3_id));
    map.remove_entry_and_descendants(&tx2_id);
    map.audit().unwrap();
    assert!(!map.contains_key(&tx2_id));
    assert!(!map.contains_key(&tx3_id));
    let descendants_set = map.calc_descendants(&tx1_id);
    assert!(!descendants_set.contains(&tx2_id));
    assert!(!descendants_set.contains(&tx3_id));
}

/// Conditional reader-before-spender ordering is not ancestry. A genuine causal chain still enforces
/// the ancestor limit without displacing any accepted entry.
#[test]
fn causal_ancestor_limit_never_evicts_existing_entries() {
    let mut map = PoolMap::new(2);

    let base = |n: u8| {
        OutPoint::new_builder()
            .tx_hash(ckb_types::packed::Byte32::new([n; 32]))
            .index(0u32)
            .build()
    };

    // C1 cell-deps on cell O (DepType::Code is the default, so dummy_resolve
    // records the dep and `deps[O]` points at C1).
    let cell_o = base(0x0f);
    let c1 = TxEntry::dummy_resolve(
        TransactionBuilder::default()
            .input(CellInput::new_builder().previous_output(base(0xa1)).build())
            .cell_dep(
                ckb_types::packed::CellDep::new_builder()
                    .out_point(cell_o.clone())
                    .build(),
            )
            .witness(Bytes::new())
            .build(),
        100,
        Capacity::shannons(100),
        100,
    );
    // C2 spends C1's output: C2 is C1's child and is cascade-removed with it.
    let c1_tx = c1.transaction().clone();
    let c2 = TxEntry::dummy_resolve(
        TransactionBuilder::default()
            .input(
                CellInput::new_builder()
                    .previous_output(OutPoint::new(c1_tx.hash(), 0))
                    .build(),
            )
            .witness(Bytes::new())
            .build(),
        100,
        Capacity::shannons(100),
        100,
    );
    let c1_id = c1.proposal_short_id();
    let c2_id = c2.proposal_short_id();
    let c2_tx = c2.transaction().clone();
    map.add_proposed(c1).unwrap();
    map.add_proposed(c2).unwrap();

    // X spends cell O and C2's output. Only C1 -> C2 -> X is causal; the
    // reader-before-spender relation on O does not create another graph edge.
    let x = TxEntry::dummy_resolve(
        TransactionBuilder::default()
            .input(CellInput::new_builder().previous_output(cell_o).build())
            .input(
                CellInput::new_builder()
                    .previous_output(OutPoint::new(c2_tx.hash(), 0))
                    .build(),
            )
            .witness(Bytes::new())
            .build(),
        100,
        Capacity::shannons(1000),
        100,
    );
    assert!(
        matches!(
            map.add_entry(x, crate::component::pool_map::Status::Proposed),
            Err(crate::error::Reject::ExceededMaximumAncestorsCount)
        ),
        "the causal chain exceeds the configured ancestor limit"
    );
    assert!(map.contains_key(&c1_id));
    assert!(map.contains_key(&c2_id));
    map.audit().unwrap();
}

#[test]
fn dep_readers_do_not_count_as_spender_ancestors() {
    let mut map = PoolMap::new(4);
    let base = |n: u8| {
        OutPoint::new_builder()
            .tx_hash(ckb_types::packed::Byte32::new([n; 32]))
            .index(0u32)
            .build()
    };
    let cell_o = base(0x31);
    let cell_p = base(0x32);
    let with_dep = |input: OutPoint, dep: OutPoint, fee: u64| {
        TxEntry::dummy_resolve(
            TransactionBuilder::default()
                .input(CellInput::new_builder().previous_output(input).build())
                .cell_dep(
                    ckb_types::packed::CellDep::new_builder()
                        .out_point(dep)
                        .build(),
                )
                .witness(Bytes::new())
                .build(),
            100,
            Capacity::shannons(fee),
            100,
        )
    };

    let make_tx = |input: OutPoint, fee: u64| {
        TxEntry::dummy_resolve(
            TransactionBuilder::default()
                .input(CellInput::new_builder().previous_output(input).build())
                .witness(Bytes::new())
                .build(),
            100,
            Capacity::shannons(fee),
            100,
        )
    };
    let child =
        |parent: &ckb_types::core::TransactionView| make_tx(OutPoint::new(parent.hash(), 0), 1);

    // P1 -> P2 -> C1 gives the low-fee dep reader C1 a deep ancestor
    // chain. C2 is its unrelated descendant, so evicting C1 has a two-entry
    // physical cascade but does not invalidate X.
    let p1 = make_tx(base(0x41), 1);
    let p1_tx = p1.transaction().clone();
    let p2 = child(&p1_tx);
    let p2_tx = p2.transaction().clone();
    let c1 = with_dep(OutPoint::new(p2_tx.hash(), 0), cell_o.clone(), 1);
    let c1_tx = c1.transaction().clone();
    let c2 = child(&c1_tx);
    let d1 = with_dep(base(0x42), cell_p.clone(), 1_000_000);
    let p1_id = p1.proposal_short_id();
    let p2_id = p2.proposal_short_id();
    let c1_id = c1.proposal_short_id();
    let c2_id = c2.proposal_short_id();
    let d1_id = d1.proposal_short_id();
    map.add_proposed(p1).unwrap();
    map.add_proposed(p2).unwrap();
    map.add_proposed(c1).unwrap();
    map.add_proposed(c2).unwrap();
    map.add_proposed(d1).unwrap();

    // C1 and D1 read cells consumed by X, but neither is a causal producer of
    // X. Admission therefore retains every reader and its causal relatives.
    let x = TxEntry::dummy_resolve(
        TransactionBuilder::default()
            .input(CellInput::new_builder().previous_output(cell_o).build())
            .input(CellInput::new_builder().previous_output(cell_p).build())
            .witness(Bytes::new())
            .build(),
        100,
        Capacity::shannons(2_000_000),
        100,
    );
    let inserted = map
        .add_entry(x, crate::component::pool_map::Status::Proposed)
        .unwrap();
    assert!(inserted);
    assert!(map.contains_key(&p1_id));
    assert!(map.contains_key(&p2_id));
    assert!(map.contains_key(&c1_id));
    assert!(map.contains_key(&c2_id));
    assert!(
        map.contains_key(&d1_id),
        "unrelated high-fee parent survives"
    );
}

/// A popular cell dep must not turn one spender admission into work or
/// displacement proportional to the number of readers.
#[test]
fn popular_dep_readers_coexist_with_spender() {
    use crate::constants::MAX_POOL_MUTATION_CANDIDATES;

    let mut map = PoolMap::new(2);
    let shared_dep = OutPoint::new(ckb_types::packed::Byte32::new([0x91; 32]), 0);
    for seed in 0..=MAX_POOL_MUTATION_CANDIDATES.saturating_add(1) {
        let mut hash = [0u8; 32];
        hash[..8].copy_from_slice(&(seed as u64).to_le_bytes());
        let entry = TxEntry::dummy_resolve(
            TransactionBuilder::default()
                .input(
                    CellInput::new_builder()
                        .previous_output(OutPoint::new(ckb_types::packed::Byte32::new(hash), 0))
                        .build(),
                )
                .cell_dep(
                    ckb_types::packed::CellDep::new_builder()
                        .out_point(shared_dep.clone())
                        .build(),
                )
                .build(),
            100,
            Capacity::shannons(100),
            100,
        );
        map.add_proposed(entry).unwrap();
    }
    let before = map.entries.len();

    let consuming = TxEntry::dummy_resolve(
        TransactionBuilder::default()
            .input(CellInput::new_builder().previous_output(shared_dep).build())
            .build(),
        100,
        Capacity::shannons(1_000_000),
        100,
    );
    let inserted = map
        .add_entry(consuming, crate::component::pool_map::Status::Pending)
        .expect("spender admission is independent of reader fanout");
    assert!(inserted);
    assert_eq!(map.entries.len(), before + 1);
    map.audit().unwrap();
}

/// Child-before-parent: linking the parent must fold the child's weight
/// into the parent's *own* descendant statistics. Without the fold, the
/// day the child leaves, `sub_descendant_weight` saturates the
/// self-only-initialized counters down to zero and the parent's evict key
/// is permanently corrupted (CPFP protection lost, eviction order broken).
#[test]
fn parent_added_after_child_gets_descendant_weight() {
    let mut map = PoolMap::new(DEFAULT_MAX_ANCESTORS_COUNT);
    // Build a three-entry chain, then add it in exact reverse order. Folding
    // only direct children would leave the late grandparent at count 2 even
    // though its authoritative descendant closure contains all three entries.
    let grandparent_tx = TransactionBuilder::default()
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::bytes(100).unwrap())
                .build(),
        )
        .output_data(Bytes::default().pack())
        .build();
    let parent_tx = TransactionBuilder::default()
        .input(
            CellInput::new_builder()
                .previous_output(OutPoint::new(grandparent_tx.hash(), 0))
                .build(),
        )
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::bytes(100).unwrap())
                .build(),
        )
        .output_data(Bytes::default().pack())
        .build();
    let child = TxEntry::dummy_resolve(
        TransactionBuilder::default()
            .input(
                CellInput::new_builder()
                    .previous_output(OutPoint::new(parent_tx.hash(), 0))
                    .build(),
            )
            .witness(Bytes::new())
            .build(),
        200,
        Capacity::shannons(200),
        200,
    );
    let parent = TxEntry::dummy_resolve(parent_tx, 100, Capacity::shannons(100), 100);
    let grandparent = TxEntry::dummy_resolve(grandparent_tx, 50, Capacity::shannons(50), 50);
    let grandparent_id = grandparent.proposal_short_id();
    let parent_id = parent.proposal_short_id();
    let child_id = child.proposal_short_id();

    map.add_proposed(child).unwrap();
    map.add_proposed(parent).unwrap();
    map.add_proposed(grandparent).unwrap();
    map.audit().unwrap();

    let parent_entry = map.get(&parent_id).unwrap();
    assert_eq!(parent_entry.ancestors_count, 2);
    assert_eq!(
        parent_entry.descendants_count, 2,
        "parent's own descendant stats must include the pre-existing child"
    );
    assert_eq!(parent_entry.descendants_size, 300);
    assert_eq!(
        map.entries
            .get_by_id(&parent_id)
            .unwrap()
            .evict_key
            .descendants_count,
        2
    );
    let grandparent_entry = map.get(&grandparent_id).unwrap();
    assert_eq!(grandparent_entry.ancestors_count, 1);
    assert_eq!(grandparent_entry.descendants_count, 3);
    assert_eq!(grandparent_entry.descendants_size, 350);

    // When the child leaves, both ancestors must subtract it exactly.
    map.remove_entry(&child_id);
    map.audit().unwrap();
    let parent_entry = map.get(&parent_id).unwrap();
    assert_eq!(parent_entry.descendants_count, 1);
    assert_eq!(parent_entry.descendants_size, 100);
    assert_eq!(
        map.entries
            .get_by_id(&parent_id)
            .unwrap()
            .evict_key
            .descendants_count,
        1
    );
    let grandparent_entry = map.get(&grandparent_id).unwrap();
    assert_eq!(grandparent_entry.descendants_count, 2);
    assert_eq!(grandparent_entry.descendants_size, 150);
}

/// A links node with no matching entry is a ghost: it must never count
/// against `conflict_closure`'s limit (the traversal and the removal plan
/// still pass through it, and removal skips it via `remove_entry`'s `None`).
#[test]
fn conflict_closure_ignores_ghost_link_nodes() {
    let mut map = PoolMap::new(DEFAULT_MAX_ANCESTORS_COUNT);
    let root = TxEntry::dummy_resolve(
        TransactionBuilder::default().build(),
        100,
        Capacity::shannons(100),
        100,
    );
    let root_id = root.proposal_short_id();
    map.add_proposed(root).unwrap();

    // Plant a ghost child: a links node with no entry.
    let ghost = ckb_types::packed::ProposalShortId::from_tx_hash(&ckb_types::packed::Byte32::new(
        [7u8; 32],
    ));
    map.links.add_link(ghost.clone(), Default::default());
    map.links.add_child(&root_id, ghost);

    let roots = std::collections::HashSet::from([root_id.clone()]);
    match map.conflict_closure(&roots, 1) {
        crate::component::pool_map::ConflictClosure::Complete { removal_set, .. } => {
            assert_eq!(
                removal_set.len(),
                1,
                "the ghost must not count against the limit"
            );
            assert!(removal_set.contains(&root_id));
        }
        crate::component::pool_map::ConflictClosure::Exceeded { .. } => {
            panic!("ghost link node must not inflate the closure count")
        }
    }
    assert!(
        map.audit().is_err(),
        "the exhaustive invariant oracle must reject a planted ghost link"
    );
}
