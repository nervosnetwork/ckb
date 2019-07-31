use crate::relayer::compact_block::CompactBlock;
use ckb_core::transaction::{IndexTransaction, ProposalShortId, TransactionBuilder};

#[test]
fn test_block_short_ids() {
    let mut compact_block = CompactBlock::default();
    let short_ids = vec![
        ProposalShortId::new([1u8; 10]),
        ProposalShortId::new([2u8; 10]),
    ];
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
        vec![
            None,
            Some(ProposalShortId::new([1u8; 10])),
            None,
            Some(ProposalShortId::new([2u8; 10]))
        ]
    );
}
