use crate::component::tests::util::{
    build_tx, build_tx_with_dep, build_tx_with_header_dep, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE,
};
use crate::component::{
    entry::TxEntry,
    pool_map::{PoolEntry, PoolMap, Status},
};
use ckb_types::{h256, packed::Byte32, prelude::*};
use std::collections::HashSet;

#[test]
fn test_basic() {
    let mut pool = PoolMap::new(100);
    assert_eq!(pool.size(), 0);
    let tx1 = build_tx(vec![(&Byte32::zero(), 1), (&Byte32::zero(), 2)], 1);
    let tx2 = build_tx(
        vec![(&h256!("0x2").pack(), 1), (&h256!("0x3").pack(), 2)],
        3,
    );
    let entry1 = TxEntry::dummy_resolve(tx1.clone(), MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    let entry2 = TxEntry::dummy_resolve(tx2.clone(), MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    assert!(pool.add_entry(entry1.clone(), Status::Pending));
    assert!(pool.add_entry(entry2, Status::Pending));
    assert!(pool.size() == 2);
    assert!(pool.contains_key(&tx1.proposal_short_id()));
    assert!(pool.contains_key(&tx2.proposal_short_id()));

    assert_eq!(pool.inputs_len(), 4);
    assert_eq!(pool.outputs_len(), 4);

    assert_eq!(pool.entries.get_by_id(&tx1.proposal_short_id()).unwrap().inner, entry1);
    assert_eq!(pool.get_tx(&tx2.proposal_short_id()).unwrap(), &tx2);

    let txs = pool.drain();
    assert!(pool.entries.is_empty());
    assert!(pool.deps.is_empty());
    assert!(pool.inputs.is_empty());
    assert!(pool.header_deps.is_empty());
    assert!(pool.outputs.is_empty());
    assert_eq!(txs, vec![tx1, tx2]);
}

#[test]
fn test_resolve_conflict() {
    let mut pool = PoolMap::new(100);
    let tx1 = build_tx(vec![(&Byte32::zero(), 1), (&h256!("0x1").pack(), 1)], 1);
    let tx2 = build_tx(
        vec![(&h256!("0x2").pack(), 1), (&h256!("0x3").pack(), 1)],
        3,
    );
    let tx3 = build_tx_with_dep(
        vec![(&h256!("0x4").pack(), 1)],
        vec![(&h256!("0x5").pack(), 1)],
        3,
    );
    let tx4 = build_tx(
        vec![(&h256!("0x2").pack(), 1), (&h256!("0x1").pack(), 1)],
        3,
    );
    let tx5 = build_tx(vec![(&h256!("0x5").pack(), 1)], 3);

    let entry1 = TxEntry::dummy_resolve(tx1, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    let entry2 = TxEntry::dummy_resolve(tx2, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    let entry3 = TxEntry::dummy_resolve(tx3, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    assert!(pool.add_entry(entry1.clone(), Status::Pending));
    assert!(pool.add_entry(entry2.clone(), Status::Pending));
    assert!(pool.add_entry(entry3.clone(), Status::Pending));

    let conflicts = pool.resolve_conflict(&tx4);
    assert_eq!(
        conflicts.into_iter().map(|i| i.0).collect::<HashSet<_>>(),
        HashSet::from_iter(vec![entry1, entry2])
    );

    let conflicts = pool.resolve_conflict(&tx5);
    assert_eq!(
        conflicts.into_iter().map(|i| i.0).collect::<HashSet<_>>(),
        HashSet::from_iter(vec![entry3])
    );
}

#[test]
fn test_resolve_conflict_descendants() {
    let mut pool = PoolMap::new(1000);
    let tx1 = build_tx(vec![(&Byte32::zero(), 1)], 1);
    let tx3 = build_tx(vec![(&tx1.hash(), 0)], 2);
    let tx4 = build_tx(vec![(&tx3.hash(), 0)], 1);

    let tx2 = build_tx(vec![(&tx1.hash(), 0)], 1);

    let entry1 = TxEntry::dummy_resolve(tx1, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    let entry3 = TxEntry::dummy_resolve(tx3, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    let entry4 = TxEntry::dummy_resolve(tx4, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    assert!(pool.add_entry(entry1, Status::Pending));
    assert!(pool.add_entry(entry3.clone(), Status::Pending));
    assert!(pool.add_entry(entry4.clone(), Status::Pending));

    let conflicts = pool.resolve_conflict(&tx2);
    assert_eq!(
        conflicts.into_iter().map(|i| i.0).collect::<HashSet<_>>(),
        HashSet::from_iter(vec![entry3, entry4])
    );
}

#[test]
fn test_resolve_conflict_header_dep() {
    let mut pool = PoolMap::new(1000);

    let header: Byte32 = h256!("0x1").pack();
    let tx = build_tx_with_header_dep(
        vec![(&Byte32::zero(), 1), (&h256!("0x1").pack(), 1)],
        vec![header.clone()],
        1,
    );
    let tx1 = build_tx(vec![(&tx.hash(), 0)], 1);

    let entry = TxEntry::dummy_resolve(tx, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    let entry1 = TxEntry::dummy_resolve(tx1, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    assert!(pool.add_entry(entry.clone(), Status::Pending));
    assert!(pool.add_entry(entry1.clone(), Status::Pending));

    assert_eq!(pool.inputs_len(), 3);
    assert_eq!(pool.header_deps_len(), 1);
    assert_eq!(pool.outputs_len(), 2);

    let mut headers = HashSet::new();
    headers.insert(header);

    let conflicts = pool.resolve_conflict_header_dep(&headers);
    assert_eq!(
        conflicts.into_iter().map(|i| i.0).collect::<HashSet<_>>(),
        HashSet::from_iter(vec![entry, entry1])
    );
}


#[test]
fn test_remove_entry() {
    let mut pool = PoolMap::new(1000);
    let tx1 = build_tx(vec![(&Byte32::zero(), 1), (&h256!("0x1").pack(), 1)], 1);
    let header: Byte32 = h256!("0x1").pack();
    let tx2 = build_tx_with_header_dep(vec![(&h256!("0x2").pack(), 1)], vec![header], 1);

    let entry1 = TxEntry::dummy_resolve(tx1.clone(), MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    let entry2 = TxEntry::dummy_resolve(tx2.clone(), MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    assert!(pool.add_entry(entry1.clone(), Status::Pending));
    assert!(pool.add_entry(entry2.clone(), Status::Pending));

    let removed = pool.remove_entry(&tx1.proposal_short_id());
    assert_eq!(removed, Some(entry1));
    let removed = pool.remove_entry(&tx2.proposal_short_id());
    assert_eq!(removed, Some(entry2));
    assert!(pool.entries.is_empty());
    assert!(pool.deps.is_empty());
    assert!(pool.inputs.is_empty());
    assert!(pool.header_deps.is_empty());
}


#[test]
fn test_remove_entries_by_filter() {
    let mut pool = PoolMap::new(1000);
    let tx1 = build_tx(vec![(&Byte32::zero(), 1), (&h256!("0x1").pack(), 1)], 1);
    let tx2 = build_tx(
        vec![(&h256!("0x2").pack(), 1), (&h256!("0x3").pack(), 1)],
        3,
    );
    let tx3 = build_tx_with_dep(
        vec![(&h256!("0x4").pack(), 1)],
        vec![(&h256!("0x5").pack(), 1)],
        3,
    );
    let entry1 = TxEntry::dummy_resolve(tx1.clone(), MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    let entry2 = TxEntry::dummy_resolve(tx2.clone(), MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    let entry3 = TxEntry::dummy_resolve(tx3.clone(), MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    assert!(pool.add_entry(entry1, Status::Pending));
    assert!(pool.add_entry(entry2, Status::Pending));
    assert!(pool.add_entry(entry3, Status::Pending));

    pool.remove_entries_by_filter(|id, _tx_entry| id == &tx1.proposal_short_id());

    assert!(!pool.contains_key(&tx1.proposal_short_id()));
    assert!(pool.contains_key(&tx2.proposal_short_id()));
    assert!(pool.contains_key(&tx3.proposal_short_id()));
}


#[test]
fn test_fill_proposals() {
    let mut pool = PoolMap::new(1000);
    let tx1 = build_tx(vec![(&Byte32::zero(), 1), (&h256!("0x1").pack(), 1)], 1);
    let tx2 = build_tx(
        vec![(&h256!("0x2").pack(), 1), (&h256!("0x3").pack(), 1)],
        3,
    );
    let tx3 = build_tx_with_dep(
        vec![(&h256!("0x4").pack(), 1)],
        vec![(&h256!("0x5").pack(), 1)],
        3,
    );
    let entry1 = TxEntry::dummy_resolve(tx1.clone(), MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    let entry2 = TxEntry::dummy_resolve(tx2.clone(), MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    let entry3 = TxEntry::dummy_resolve(tx3.clone(), MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    assert!(pool.add_entry(entry1, Status::Pending));
    assert!(pool.add_entry(entry2, Status::Pending));
    assert!(pool.add_entry(entry3, Status::Pending));

    assert_eq!(pool.inputs_len(), 5);
    assert_eq!(pool.deps_len(), 1);
    assert_eq!(pool.outputs_len(), 7);

    let id1 = tx1.proposal_short_id();
    let id2 = tx2.proposal_short_id();
    let id3 = tx3.proposal_short_id();

    let mut ret = HashSet::new();
    pool.fill_proposals(10, &HashSet::new(), &mut ret, &Status::Pending);
    assert_eq!(
        ret,
        HashSet::from_iter(vec![id1.clone(), id2.clone(), id3.clone()])
    );

    let mut ret = HashSet::new();
    pool.fill_proposals(1, &HashSet::new(), &mut ret, &Status::Pending);
    assert_eq!(ret, HashSet::from_iter(vec![id1.clone()]));

    let mut ret = HashSet::new();
    pool.fill_proposals(2, &HashSet::new(), &mut ret, &Status::Pending);
    assert_eq!(ret, HashSet::from_iter(vec![id1.clone(), id2.clone()]));

    let mut ret = HashSet::new();
    let mut exclusion = HashSet::new();
    exclusion.insert(id2);
    pool.fill_proposals(2, &exclusion, &mut ret, &Status::Pending);
    assert_eq!(ret, HashSet::from_iter(vec![id1, id3]));
}
