use ckb_types::{
    bytes::Bytes,
    core::{Capacity, TransactionBuilder},
    packed::{CellInput, OutPoint, ProposalShortId},
    prelude::*,
};
use std::mem::size_of;

use crate::component::{
    container::{AncestorsScoreSortKey, SortedTxMap},
    entry::TxEntry,
};

const DEFAULT_MAX_ANCESTORS_COUNT: usize = 125;

#[test]
fn test_min_fee_and_weight() {
    let result = vec![
        (0, 0, 0, 0),
        (1, 0, 1, 0),
        (500, 10, 1000, 30),
        (10, 500, 30, 1000),
        (500, 10, 1000, 20),
        (std::u64::MAX, 0, std::u64::MAX, 0),
        (std::u64::MAX, 100, std::u64::MAX, 2000),
        (std::u64::MAX, std::u64::MAX, std::u64::MAX, std::u64::MAX),
    ]
    .into_iter()
    .map(|(fee, weight, ancestors_fee, ancestors_weight)| {
        let key = AncestorsScoreSortKey {
            fee: Capacity::shannons(fee),
            weight,
            id: ProposalShortId::new([0u8; 10]),
            ancestors_fee: Capacity::shannons(ancestors_fee),
            ancestors_weight,
            ancestors_size: 0,
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
            (Capacity::shannons(std::u64::MAX), 0),
            (Capacity::shannons(std::u64::MAX), 2000),
            (Capacity::shannons(std::u64::MAX), std::u64::MAX),
        ]
    );
}

#[test]
fn test_ancestors_sorted_key_order() {
    let mut keys = vec![
        (0, 0, 0, 0),
        (1, 0, 1, 0),
        (500, 10, 1000, 30),
        (10, 500, 30, 1000),
        (500, 10, 1000, 30),
        (10, 500, 30, 1000),
        (500, 10, 1000, 20),
        (std::u64::MAX, 0, std::u64::MAX, 0),
        (std::u64::MAX, 100, std::u64::MAX, 2000),
        (std::u64::MAX, std::u64::MAX, std::u64::MAX, std::u64::MAX),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (fee, weight, ancestors_fee, ancestors_weight))| {
        let mut id = [0u8; 10];
        id[..size_of::<u32>()].copy_from_slice(&(i as u32).to_be_bytes());
        AncestorsScoreSortKey {
            fee: Capacity::shannons(fee),
            weight,
            id: ProposalShortId::new(id),
            ancestors_fee: Capacity::shannons(ancestors_fee),
            ancestors_weight,
            ancestors_size: 0,
        }
    })
    .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(
        keys.into_iter().map(|k| k.id).collect::<Vec<_>>(),
        [0, 3, 5, 9, 2, 4, 6, 8, 1, 7]
            .iter()
            .map(|&i| {
                let mut id = [0u8; 10];
                id[..size_of::<u32>()].copy_from_slice(&(i as u32).to_be_bytes());
                ProposalShortId::new(id)
            })
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_remove_entry() {
    let mut map = SortedTxMap::new(DEFAULT_MAX_ANCESTORS_COUNT);
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
                            .index(0u32.pack())
                            .build(),
                    )
                    .build(),
            )
            .witness(Bytes::new().pack())
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
                            .index(0u32.pack())
                            .build(),
                    )
                    .build(),
            )
            .witness(Bytes::new().pack())
            .build(),
        200,
        Capacity::shannons(200),
        200,
    );
    let tx1_id = tx1.proposal_short_id();
    let tx2_id = tx2.proposal_short_id();
    let tx3_id = tx3.proposal_short_id();
    map.add_entry(tx1).unwrap();
    map.add_entry(tx2).unwrap();
    map.add_entry(tx3).unwrap();
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
    let mut map = SortedTxMap::new(DEFAULT_MAX_ANCESTORS_COUNT);
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
                            .index(0u32.pack())
                            .build(),
                    )
                    .build(),
            )
            .witness(Bytes::new().pack())
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
                            .index(0u32.pack())
                            .build(),
                    )
                    .build(),
            )
            .witness(Bytes::new().pack())
            .build(),
        200,
        Capacity::shannons(200),
        200,
    );
    let tx1_id = tx1.proposal_short_id();
    let tx2_id = tx2.proposal_short_id();
    let tx3_id = tx3.proposal_short_id();
    map.add_entry(tx1).unwrap();
    map.add_entry(tx2).unwrap();
    map.add_entry(tx3).unwrap();
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
