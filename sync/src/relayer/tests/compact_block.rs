use crate::relayer::compact_block::CompactBlock;
use ckb_core::transaction::{IndexTransaction, TransactionBuilder};

#[test]
fn test_block_short_ids() {
    let mut compact_block = CompactBlock::default();
    let short_ids = vec![[1u8; 6], [2u8; 6]];
    let prefilled_transactions = vec![
        IndexTransaction {
            index: 0,
            transaction: TransactionBuilder::default().build(),
        },
        IndexTransaction {
            index: 2,
            transaction: TransactionBuilder::default().build(),
        },
    ];

    compact_block.short_ids = short_ids;
    compact_block.prefilled_transactions = prefilled_transactions;

    assert_eq!(
        compact_block.block_short_ids(),
        vec![None, Some([1u8; 6]), None, Some([2u8; 6])]
    );
}
