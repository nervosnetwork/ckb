use crate::{
    component::{
        entry::TxEntry,
        orphan::OrphanPool,
        pool_map::{PoolMap, Status},
        verify_queue::VerifyQueue,
    },
    pool_cell::PoolCell,
};
use ckb_types::{
    core::cell::{CellChecker, CellProvider, CellStatus},
    packed::{Byte32, OutPoint},
    prelude::*,
};

use super::util::{
    DEFAULT_MAX_ANCESTORS_COUNT, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE, build_tx, build_tx_with_dep,
};

fn alternate_hash(tx_hash: &Byte32) -> Byte32 {
    let mut bytes = tx_hash.as_slice().to_vec();
    bytes[31] ^= 1;
    Byte32::from_slice(&bytes).expect("valid transaction hash")
}

#[test]
fn lookup_by_transaction_hash() {
    let parent = build_tx(vec![(&Byte32::zero(), 0)], 1);
    let parent_hash = parent.hash();
    let alternate_hash = alternate_hash(&parent_hash);
    let alternate_tx = parent.clone().fake_hash(alternate_hash.clone());

    assert_eq!(parent.proposal_short_id(), alternate_tx.proposal_short_id());
    assert_ne!(parent_hash, alternate_hash);

    let mut pool = PoolMap::new(DEFAULT_MAX_ANCESTORS_COUNT);
    pool.add_entry(
        TxEntry::dummy_resolve(parent.clone(), MOCK_CYCLES, MOCK_FEE, MOCK_SIZE),
        Status::Pending,
    )
    .unwrap();

    // Proposal queries use IDs; transaction queries use hashes.
    assert!(pool.get_by_id(&alternate_tx.proposal_short_id()).is_some());
    assert!(pool.get_by_tx_hash(&parent_hash).is_some());
    assert!(pool.get_by_tx_hash(&alternate_hash).is_none());

    let parent_out_point = OutPoint::new(parent_hash, 0);
    let alternate_out_point = OutPoint::new(alternate_hash, 0);
    assert_eq!(
        pool.get_output_with_data(&parent_out_point),
        parent.output_with_data(0)
    );
    assert!(pool.get_output_with_data(&alternate_out_point).is_none());

    let pool_cell = PoolCell::new(&pool, false);
    assert!(pool_cell.cell(&parent_out_point, false).is_live());
    assert_eq!(pool_cell.is_live(&parent_out_point), Some(true));
    assert_eq!(
        pool_cell.cell(&alternate_out_point, false),
        CellStatus::Unknown
    );
    assert_eq!(pool_cell.is_live(&alternate_out_point), None);
}

#[test]
fn link_pool_entries_by_transaction_hash() {
    let parent = build_tx(vec![(&Byte32::zero(), 0)], 1);
    let parent_hash = parent.hash();
    let parent_id = parent.proposal_short_id();
    let alternate_hash = alternate_hash(&parent_hash);
    let alternate_tx = parent.clone().fake_hash(alternate_hash.clone());
    let child = build_tx_with_dep(vec![(&alternate_hash, 0)], vec![(&alternate_hash, 0)], 1);
    let child_id = child.proposal_short_id();

    assert_eq!(parent.proposal_short_id(), alternate_tx.proposal_short_id());

    let mut pool = PoolMap::new(DEFAULT_MAX_ANCESTORS_COUNT);
    pool.add_entry(
        TxEntry::dummy_resolve(parent.clone(), MOCK_CYCLES, MOCK_FEE, MOCK_SIZE),
        Status::Pending,
    )
    .unwrap();
    pool.add_entry(
        TxEntry::dummy_resolve(child, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE),
        Status::Pending,
    )
    .unwrap();

    assert!(pool.links.get_parents(&child_id).unwrap().is_empty());
    assert!(pool.calc_ancestors(&child_id).is_empty());
    assert_eq!(
        pool.get_by_tx_hash(&parent_hash)
            .unwrap()
            .inner
            .descendants_count,
        1
    );

    let matching_child = build_tx_with_dep(vec![(&parent_hash, 0)], vec![(&parent_hash, 0)], 2);
    let matching_child_id = matching_child.proposal_short_id();
    let mut matching_pool = PoolMap::new(DEFAULT_MAX_ANCESTORS_COUNT);
    matching_pool
        .add_entry(
            TxEntry::dummy_resolve(parent, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE),
            Status::Pending,
        )
        .unwrap();
    matching_pool
        .add_entry(
            TxEntry::dummy_resolve(matching_child, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE),
            Status::Pending,
        )
        .unwrap();
    let matching_parents = matching_pool.links.get_parents(&matching_child_id).unwrap();
    assert_eq!(matching_parents.len(), 1);
    assert!(matching_parents.contains(&parent_id));
}

#[test]
fn remove_entries_by_transaction_hash() {
    let tx = build_tx(vec![(&Byte32::zero(), 0)], 1);
    let tx_hash = tx.hash();
    let child = build_tx(vec![(&tx_hash, 0)], 1);
    let alternate_hash = alternate_hash(&tx_hash);
    let alternate_tx = tx.clone().fake_hash(alternate_hash.clone());

    assert_eq!(tx.proposal_short_id(), alternate_tx.proposal_short_id());

    let mut queue = VerifyQueue::new(u64::MAX);
    assert!(queue.add_tx(tx.clone(), false, None).unwrap());
    assert!(queue.get_tx_by_hash(&tx_hash).is_some());
    assert!(queue.get_tx_by_hash(&alternate_hash).is_none());
    assert!(queue.remove_tx_by_hash(&alternate_hash).is_none());
    assert_eq!(queue.len(), 1);
    assert!(queue.remove_tx_by_hash(&tx_hash).is_some());

    let mut orphan = OrphanPool::new();
    assert!(orphan.add_orphan_tx(tx.clone(), 0.into(), 0).is_empty());
    assert!(orphan.get_by_tx_hash(&tx_hash).is_some());
    assert!(orphan.get_by_tx_hash(&alternate_hash).is_none());
    assert!(orphan.remove_orphan_tx_by_hash(&alternate_hash).is_none());
    assert_eq!(orphan.len(), 1);
    orphan.remove_orphan_txs_by_hash([alternate_hash.clone()].into_iter());
    assert_eq!(orphan.len(), 1);
    orphan.remove_orphan_txs_by_hash([tx_hash.clone()].into_iter());
    assert_eq!(orphan.len(), 0);

    let mut single_pool = PoolMap::new(DEFAULT_MAX_ANCESTORS_COUNT);
    single_pool
        .add_entry(
            TxEntry::dummy_resolve(tx.clone(), MOCK_CYCLES, MOCK_FEE, MOCK_SIZE),
            Status::Pending,
        )
        .unwrap();
    assert!(
        single_pool
            .remove_entry_by_tx_hash(&alternate_hash)
            .is_none()
    );
    assert_eq!(single_pool.size(), 1);
    assert!(single_pool.remove_entry_by_tx_hash(&tx_hash).is_some());
    assert_eq!(single_pool.size(), 0);

    let mut pool = PoolMap::new(DEFAULT_MAX_ANCESTORS_COUNT);
    pool.add_entry(
        TxEntry::dummy_resolve(tx, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE),
        Status::Pending,
    )
    .unwrap();
    pool.add_entry(
        TxEntry::dummy_resolve(child, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE),
        Status::Pending,
    )
    .unwrap();
    assert!(
        pool.remove_entry_and_descendants_by_tx_hash(&alternate_hash)
            .is_empty()
    );
    assert_eq!(pool.size(), 2);
    assert_eq!(
        pool.remove_entry_and_descendants_by_tx_hash(&tx_hash).len(),
        2
    );
}
