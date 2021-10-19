use crate::component::tests::util::{
    build_tx, build_tx_with_dep, build_tx_with_header_dep, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE,
};
use crate::component::{entry::TxEntry, pending::PendingQueue};
use ckb_types::{h256, packed::Byte32, prelude::*};
use std::collections::HashSet;
use std::iter::FromIterator;

#[test]
fn test_basic() {
    let mut queue = PendingQueue::new();
    let tx1 = build_tx(vec![(&Byte32::zero(), 1), (&Byte32::zero(), 2)], 1);
    let tx2 = build_tx(
        vec![(&h256!("0x2").pack(), 1), (&h256!("0x3").pack(), 2)],
        3,
    );
    let entry1 = TxEntry::dummy_resolve(tx1.clone(), MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    let entry2 = TxEntry::dummy_resolve(tx2.clone(), MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    assert!(queue.add_entry(entry1.clone()));
    assert!(queue.add_entry(entry2));
    assert!(queue.size() == 2);
    assert!(queue.contains_key(&tx1.proposal_short_id()));
    assert!(queue.contains_key(&tx2.proposal_short_id()));

    assert_eq!(queue.get(&tx1.proposal_short_id()).unwrap(), &entry1);
    assert_eq!(queue.get_tx(&tx2.proposal_short_id()).unwrap(), &tx2);

    let txs = queue.drain();
    assert!(queue.inner.is_empty());
    assert!(queue.deps.is_empty());
    assert!(queue.inputs.is_empty());
    assert_eq!(txs, vec![tx1, tx2]);
}

#[test]
fn test_resolve_conflict() {
    let mut queue = PendingQueue::new();
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
    assert!(queue.add_entry(entry1.clone()));
    assert!(queue.add_entry(entry2.clone()));
    assert!(queue.add_entry(entry3.clone()));

    let conflicts = queue.resolve_conflict(&tx4);
    assert_eq!(
        conflicts.into_iter().map(|i| i.0).collect::<HashSet<_>>(),
        HashSet::from_iter(vec![entry1, entry2])
    );

    let conflicts = queue.resolve_conflict(&tx5);
    assert_eq!(
        conflicts.into_iter().map(|i| i.0).collect::<HashSet<_>>(),
        HashSet::from_iter(vec![entry3])
    );
}

#[test]
fn test_resolve_conflict_header_dep() {
    let mut queue = PendingQueue::new();

    let header: Byte32 = h256!("0x1").pack();
    let tx = build_tx_with_header_dep(
        vec![(&Byte32::zero(), 1), (&h256!("0x1").pack(), 1)],
        vec![header.clone()],
        1,
    );

    let entry = TxEntry::dummy_resolve(tx, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    assert!(queue.add_entry(entry.clone()));

    let mut headers = HashSet::new();
    headers.insert(header);

    let conflicts = queue.resolve_conflict_header_dep(&headers);
    assert_eq!(
        conflicts.into_iter().map(|i| i.0).collect::<HashSet<_>>(),
        HashSet::from_iter(vec![entry])
    );
}

#[test]
fn test_remove_committed_tx() {
    let mut queue = PendingQueue::new();
    let tx1 = build_tx(vec![(&Byte32::zero(), 1), (&h256!("0x1").pack(), 1)], 1);
    let header: Byte32 = h256!("0x1").pack();
    let tx2 = build_tx_with_header_dep(vec![(&h256!("0x2").pack(), 1)], vec![header], 1);

    let entry1 = TxEntry::dummy_resolve(tx1.clone(), MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    let entry2 = TxEntry::dummy_resolve(tx2.clone(), MOCK_CYCLES, MOCK_FEE, MOCK_SIZE);
    assert!(queue.add_entry(entry1.clone()));
    assert!(queue.add_entry(entry2.clone()));

    let related_dep1: Vec<_> = entry1.related_dep_out_points().cloned().collect();
    let related_dep2: Vec<_> = entry2.related_dep_out_points().cloned().collect();

    let removed = queue.remove_committed_tx(&tx1, &related_dep1);
    assert_eq!(removed, Some(entry1));
    let removed = queue.remove_committed_tx(&tx2, &related_dep2);
    assert_eq!(removed, Some(entry2));
    assert!(queue.inner.is_empty());
    assert!(queue.deps.is_empty());
    assert!(queue.inputs.is_empty());
    assert!(queue.header_deps.is_empty());
}

#[test]
fn test_remove_entries_by_filter() {
    let mut queue = PendingQueue::new();
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
    assert!(queue.add_entry(entry1));
    assert!(queue.add_entry(entry2));
    assert!(queue.add_entry(entry3));

    queue.remove_entries_by_filter(|id, _tx_entry| id == &tx1.proposal_short_id());

    assert!(!queue.contains_key(&tx1.proposal_short_id()));
    assert!(queue.contains_key(&tx2.proposal_short_id()));
    assert!(queue.contains_key(&tx3.proposal_short_id()));
}

#[test]
fn test_fill_proposals() {
    let mut queue = PendingQueue::new();
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
    assert!(queue.add_entry(entry1));
    assert!(queue.add_entry(entry2));
    assert!(queue.add_entry(entry3));

    let id1 = tx1.proposal_short_id();
    let id2 = tx2.proposal_short_id();
    let id3 = tx3.proposal_short_id();

    let mut ret = HashSet::new();
    queue.fill_proposals(10, &HashSet::new(), &mut ret);
    assert_eq!(
        ret,
        HashSet::from_iter(vec![id1.clone(), id2.clone(), id3.clone()])
    );

    let mut ret = HashSet::new();
    queue.fill_proposals(1, &HashSet::new(), &mut ret);
    assert_eq!(ret, HashSet::from_iter(vec![id1.clone()]));

    let mut ret = HashSet::new();
    queue.fill_proposals(2, &HashSet::new(), &mut ret);
    assert_eq!(ret, HashSet::from_iter(vec![id1.clone(), id2.clone()]));

    let mut ret = HashSet::new();
    let mut exclusion = HashSet::new();
    exclusion.insert(id2);
    queue.fill_proposals(2, &exclusion, &mut ret);
    assert_eq!(ret, HashSet::from_iter(vec![id1, id3]));
}
