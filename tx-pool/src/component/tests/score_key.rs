use ckb_types::{
    bytes::Bytes,
    core::{Capacity, TransactionBuilder},
    packed::{CellInput, CellOutput, OutPoint},
    prelude::*,
};

use crate::component::{entry::TxEntry, pool_map::PoolMap, sort_key::AncestorsScoreSortKey};

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
    let descendants_set = map.calc_descendants(&tx1_id);
    assert!(descendants_set.contains(&tx2_id));
    assert!(descendants_set.contains(&tx3_id));

    let tx3_entry = map.get(&tx3_id);
    assert!(tx3_entry.is_some());
    let tx3_entry = tx3_entry.unwrap();
    assert_eq!(tx3_entry.ancestors_count, 3);

    map.remove_entry(&tx1_id);
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
    let descendants_set = map.calc_descendants(&tx1_id);
    assert!(descendants_set.contains(&tx2_id));
    assert!(descendants_set.contains(&tx3_id));
    map.remove_entry_and_descendants(&tx2_id);
    assert!(!map.contains_key(&tx2_id));
    assert!(!map.contains_key(&tx3_id));
    let descendants_set = map.calc_descendants(&tx1_id);
    assert!(!descendants_set.contains(&tx2_id));
    assert!(!descendants_set.contains(&tx3_id));
}

/// Regression for the cell-ref escape hatch in `check_and_prepare_ancestors`:
/// evicting a candidate also removes its descendants, and any of them may be
/// a *direct parent* of the new entry. A cascade-removed parent lingering in
/// the parent set used to be recounted as an ancestor (links' relation walk
/// never checks that the id is still linked) and failed the weight fold with
/// a spurious `Malformed` — or tripped the old `assert!` inside the write
/// lock, unwinding past the `evicted_journal` recovery protocol.
#[test]
fn escape_hatch_eviction_drops_cascaded_parents_from_parent_set() {
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

    // X spends cell O (so C1 is a cell-ref parent through `deps[O]`) and C2's
    // output (so C2 is a direct parent). ancestors = {C1, C2}, count 3 > 2,
    // and 3 - 1 cell-ref parent == 2, so the escape hatch evicts C1 — which
    // cascades to C2. Both must leave the parent set.
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
    let (added, evicted) = map
        .add_entry(x, crate::component::pool_map::Status::Proposed)
        .expect("escape-hatch submit must not fail with a ghost-parent Malformed");
    assert!(added);
    let evicted_ids: std::collections::HashSet<_> = evicted
        .iter()
        .map(|entry| entry.proposal_short_id())
        .collect();
    assert!(evicted_ids.contains(&c1_id));
    assert!(evicted_ids.contains(&c2_id));
    assert!(!map.contains_key(&c1_id));
    assert!(!map.contains_key(&c2_id));
}

/// Child-before-parent: linking the parent must fold the child's weight
/// into the parent's *own* descendant statistics. Without the fold, the
/// day the child leaves, `sub_descendant_weight` saturates the
/// self-only-initialized counters down to zero and the parent's evict key
/// is permanently corrupted (CPFP protection lost, eviction order broken).
#[test]
fn parent_added_after_child_gets_descendant_weight() {
    let mut map = PoolMap::new(DEFAULT_MAX_ANCESTORS_COUNT);
    // The parent produces the output the child spends. Build the parent
    // first for its hash, but add the *child* first (child-before-parent).
    let parent_tx = TransactionBuilder::default()
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
    let parent_id = parent.proposal_short_id();
    let child_id = child.proposal_short_id();

    map.add_proposed(child).unwrap();
    map.add_proposed(parent).unwrap();

    let parent_entry = map.get(&parent_id).unwrap();
    assert_eq!(parent_entry.ancestors_count, 1);
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

    // When the child leaves, the parent must drop back to self-only — not
    // saturate down to zero.
    map.remove_entry(&child_id);
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
}
