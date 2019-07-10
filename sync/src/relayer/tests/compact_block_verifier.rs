use super::helper::new_index_transaction;
use crate::relayer::compact_block::{CompactBlock, ShortTransactionID};
use crate::relayer::compact_block_verifier::{PrefilledVerifier, ShortIdsVerifier};
use crate::relayer::error::{Error, Misbehavior};
use ckb_core::transaction::IndexTransaction;
use ckb_protocol::{short_transaction_id, short_transaction_id_keys};

#[test]
fn test_unordered_prefilled() {
    let mut block = CompactBlock::default();
    let prefilled: Vec<IndexTransaction> = vec![0, 1, 2, 4, 3]
        .into_iter()
        .map(new_index_transaction)
        .collect();
    block.prefilled_transactions = prefilled;
    assert_eq!(
        PrefilledVerifier::new().verify(&block),
        Err(Error::Misbehavior(
            Misbehavior::UnorderedPrefilledTransactions
        )),
    );
}

#[test]
fn test_ordered_prefilled() {
    let mut block = CompactBlock::default();
    let prefilled: Vec<IndexTransaction> = (0..5).map(new_index_transaction).collect();
    block.prefilled_transactions = prefilled;
    assert_eq!(PrefilledVerifier::new().verify(&block), Ok(()));
}

#[test]
fn test_overflow_prefilled() {
    let mut block = CompactBlock::default();
    let prefilled: Vec<IndexTransaction> = vec![0, 1, 2, 5]
        .into_iter()
        .map(new_index_transaction)
        .collect();
    block.prefilled_transactions = prefilled;
    assert_eq!(
        PrefilledVerifier::new().verify(&block),
        Err(Error::Misbehavior(
            Misbehavior::OverflowPrefilledTransactions
        )),
    );
}

#[test]
fn test_cellbase_not_prefilled() {
    let block = CompactBlock::default();
    assert_eq!(
        PrefilledVerifier::new().verify(&block),
        Err(Error::Misbehavior(Misbehavior::CellbaseNotPrefilled)),
    );

    let mut block = CompactBlock::default();
    let prefilled: Vec<IndexTransaction> = (1..5).map(new_index_transaction).collect();
    block.prefilled_transactions = prefilled;
    assert_eq!(
        PrefilledVerifier::new().verify(&block),
        Err(Error::Misbehavior(Misbehavior::CellbaseNotPrefilled))
    );
}

#[test]
fn test_duplicated_short_ids() {
    let mut block = CompactBlock::default();
    let mut short_ids: Vec<ShortTransactionID> = (1..5)
        .map(new_index_transaction)
        .map(|tx| {
            let (key0, key1) = short_transaction_id_keys(block.header.nonce(), block.nonce);
            short_transaction_id(key0, key1, &tx.transaction.witness_hash())
        })
        .collect();
    short_ids.push(short_ids[0]);
    block.short_ids = short_ids;
    assert_eq!(
        ShortIdsVerifier::new().verify(&block),
        Err(Error::Misbehavior(Misbehavior::DuplicatedShortIds)),
    );
}

#[test]
fn test_intersected_short_ids() {
    let mut block = CompactBlock::default();
    let prefilled: Vec<IndexTransaction> = (0..=5).map(new_index_transaction).collect();
    let short_ids: Vec<ShortTransactionID> = (5..9)
        .map(new_index_transaction)
        .map(|tx| {
            let (key0, key1) = short_transaction_id_keys(block.header.nonce(), block.nonce);
            short_transaction_id(key0, key1, &tx.transaction.witness_hash())
        })
        .collect();
    block.prefilled_transactions = prefilled;
    block.short_ids = short_ids;
    assert_eq!(
        ShortIdsVerifier::new().verify(&block),
        Err(Error::Misbehavior(
            Misbehavior::IntersectedPrefilledTransactions
        )),
    );
}

#[test]
fn test_normal() {
    let mut block = CompactBlock::default();
    let prefilled: Vec<IndexTransaction> = vec![1, 2, 5]
        .into_iter()
        .map(new_index_transaction)
        .collect();
    let short_ids: Vec<ShortTransactionID> = vec![0, 3, 4]
        .into_iter()
        .map(new_index_transaction)
        .map(|tx| {
            let (key0, key1) = short_transaction_id_keys(block.header.nonce(), block.nonce);
            short_transaction_id(key0, key1, &tx.transaction.witness_hash())
        })
        .collect();
    block.prefilled_transactions = prefilled;
    block.short_ids = short_ids;
    assert_eq!(ShortIdsVerifier::new().verify(&block), Ok(()));
}
