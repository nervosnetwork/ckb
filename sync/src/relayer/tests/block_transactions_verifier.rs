use super::helper::new_index_transaction;
use crate::relayer::block_transactions_verifier::BlockTransactionsVerifier;
use crate::relayer::error::{Error, Misbehavior};
use ckb_types::packed::{CompactBlock, CompactBlockBuilder, IndexTransaction};
use ckb_types::prelude::*;

// block_short_ids: vec![None, Some(1), None, Some(3), Some(4), None]
fn build_compact_block() -> CompactBlock {
    let prefilled: Vec<IndexTransaction> = vec![0, 2, 5]
        .into_iter()
        .map(new_index_transaction)
        .collect();

    let short_ids = vec![1, 3, 4]
        .into_iter()
        .map(new_index_transaction)
        .map(|tx| tx.transaction().proposal_short_id());

    CompactBlockBuilder::default()
        .short_ids(short_ids.pack())
        .prefilled_transactions(prefilled.into_iter().pack())
        .build()
}

#[test]
fn test_invalid() {
    let block = build_compact_block();
    let indexes = vec![1, 3, 4];

    // Invalid len
    let block_txs: Vec<_> = vec![1, 3]
        .into_iter()
        .map(|i| new_index_transaction(i).transaction().into_view())
        .collect();

    let ret = BlockTransactionsVerifier::verify(&block, &indexes, block_txs.as_slice());

    assert_eq!(
        ret.err(),
        Some(Error::Misbehavior(
            Misbehavior::InvalidBlockTransactionsLength {
                expected: 3,
                actual: 2
            }
        ))
    );

    // Unordered txs
    let block_txs: Vec<_> = vec![1, 4, 3]
        .into_iter()
        .map(|i| new_index_transaction(i).transaction().into_view())
        .collect();

    let expected = new_index_transaction(3).transaction().proposal_short_id();
    let actual = new_index_transaction(4).transaction().proposal_short_id();

    let ret = BlockTransactionsVerifier::verify(&block, &indexes, &block_txs);

    assert_eq!(
        ret.err(),
        Some(Error::Misbehavior(Misbehavior::InvalidBlockTransactions {
            expected,
            actual
        }))
    );
}

#[test]
fn test_ok() {
    let block = build_compact_block();

    let indexes = vec![1, 3, 4];
    let block_txs: Vec<_> = vec![1, 3, 4]
        .into_iter()
        .map(|i| new_index_transaction(i).transaction().into_view())
        .collect();

    let ret = BlockTransactionsVerifier::verify(&block, &indexes, &block_txs);

    assert!(ret.is_ok());
}
