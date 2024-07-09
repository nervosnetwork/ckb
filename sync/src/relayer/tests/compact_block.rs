use ckb_types::prelude::*;
use ckb_types::{
    core::TransactionBuilder,
    packed::{CompactBlockBuilder, IndexTransactionBuilder, ProposalShortId},
};

#[test]
fn test_block_short_ids() {
    let compact_block_builder = CompactBlockBuilder::default();
    let short_ids = vec![
        ProposalShortId::new([1u8; 10]),
        ProposalShortId::new([2u8; 10]),
    ];
    let prefilled_transactions = vec![
        IndexTransactionBuilder::default()
            .index(0u32.into())
            .transaction(TransactionBuilder::default().build().data())
            .build(),
        IndexTransactionBuilder::default()
            .index(2u32.into())
            .transaction(TransactionBuilder::default().build().data())
            .build(),
    ];

    let compact_block = compact_block_builder
        .short_ids(short_ids.into())
        .prefilled_transactions(prefilled_transactions.into())
        .build();

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
