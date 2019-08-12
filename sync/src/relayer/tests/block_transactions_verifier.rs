use super::helper::new_index_transaction;
use crate::relayer::block_transactions_verifier::BlockTransactionsVerifier;
use crate::relayer::compact_block::CompactBlock;
use crate::StatusCode;
use ckb_core::transaction::IndexTransaction;

// block_short_ids: vec![None, Some(1), None, Some(3), Some(4), None]
fn build_compact_block() -> CompactBlock {
    let mut block = CompactBlock::default();

    let prefilled: Vec<IndexTransaction> = vec![0, 2, 5]
        .into_iter()
        .map(new_index_transaction)
        .collect();

    let short_ids = vec![1, 3, 4]
        .into_iter()
        .map(new_index_transaction)
        .clone()
        .map(|tx| tx.transaction.proposal_short_id())
        .collect();
    block.prefilled_transactions = prefilled;
    block.short_ids = short_ids;

    block
}

#[test]
fn test_invalid() {
    let block = build_compact_block();
    let indexes = vec![1, 3, 4];

    // Invalid len
    let block_txs: Vec<_> = vec![1, 3]
        .into_iter()
        .map(|i| new_index_transaction(i).transaction)
        .collect();
    assert_eq!(
        BlockTransactionsVerifier::verify(&block, &indexes, block_txs.as_slice()),
        StatusCode::UnmatchedBlockTransactionsLength.into(),
    );

    // Unordered txs
    let block_txs: Vec<_> = vec![1, 4, 3]
        .into_iter()
        .map(|i| new_index_transaction(i).transaction)
        .collect();

    assert_eq!(
        BlockTransactionsVerifier::verify(&block, &indexes, &block_txs),
        StatusCode::UnmatchedBlockTransactions.into(),
    );
}

#[test]
fn test_ok() {
    let block = build_compact_block();

    let indexes = vec![1, 3, 4];
    let block_txs: Vec<_> = vec![1, 3, 4]
        .into_iter()
        .map(|i| new_index_transaction(i).transaction)
        .collect();

    let ret = BlockTransactionsVerifier::verify(&block, &indexes, &block_txs);
    assert!(ret.is_ok());
}
