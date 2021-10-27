use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder, ProposalWindow};
use ckb_db::RocksDB;
use ckb_db_schema::COLUMNS;
use ckb_occupied_capacity::IntoCapacity;
use ckb_store::{ChainDB, ChainStore};
use ckb_types::{
    core::{BlockBuilder, BlockExt, HeaderBuilder, TransactionBuilder},
    packed::ProposalShortId,
    prelude::*,
};
use std::collections::HashSet;
use std::iter::FromIterator;
use tempfile::TempDir;

use crate::RewardCalculator;

#[test]
fn get_proposal_ids_by_hash() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let store = ChainDB::new(db, Default::default());

    let proposal1 = ProposalShortId::new([1; 10]);
    let proposal2 = ProposalShortId::new([2; 10]);
    let proposal3 = ProposalShortId::new([3; 10]);

    let expected = HashSet::from_iter(vec![
        proposal1.clone(),
        proposal2.clone(),
        proposal3.clone(),
    ]);

    let uncle1 = BlockBuilder::default()
        .proposal(proposal1.clone())
        .proposal(proposal2.clone())
        .build()
        .as_uncle();
    let uncle2 = BlockBuilder::default()
        .proposal(proposal2)
        .proposal(proposal3)
        .build()
        .as_uncle();

    let block = BlockBuilder::default()
        .proposal(proposal1)
        .uncles(vec![uncle1, uncle2])
        .build();

    let hash = block.hash();
    let txn = store.begin_transaction();
    txn.insert_block(&block).unwrap();
    txn.commit().unwrap();
    assert_eq!(block, store.get_block(&hash).unwrap());

    let consensus = Consensus::default();
    let reward_calculator = RewardCalculator::new(&consensus, &store);
    let ids = reward_calculator.get_proposal_ids_by_hash(&block.hash());

    assert_eq!(ids, expected);
}

#[test]
fn test_txs_fees() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let store = ChainDB::new(db, Default::default());

    // Default PROPOSER_REWARD_RATIO is Ratio(4, 10)
    let consensus = Consensus::default();

    let block = BlockBuilder::default().build();
    let ext_tx_fees = vec![
        100u32.into_capacity(),
        20u32.into_capacity(),
        33u32.into_capacity(),
        34u32.into_capacity(),
    ];
    let ext = BlockExt {
        received_at: block.timestamp(),
        total_difficulty: block.difficulty(),
        total_uncles_count: block.data().uncles().len() as u64,
        verified: Some(true),
        txs_fees: ext_tx_fees,
    };

    let txn = store.begin_transaction();
    txn.insert_block(&block).unwrap();
    txn.insert_block_ext(&block.hash(), &ext).unwrap();
    txn.commit().unwrap();

    let reward_calculator = RewardCalculator::new(&consensus, &store);
    let txs_fees = reward_calculator.txs_fees(&block.header()).unwrap();

    let expected: u32 = [100u32, 20u32, 33u32, 34u32]
        .iter()
        .map(|x| x - x * 4 / 10)
        .sum();

    assert_eq!(txs_fees, expected.into_capacity());
}

// Earliest proposer get 40% of tx fee as reward when tx committed
//  block H(19) target H(13) ProposalWindow(2, 5)
//                 target                    current
//                  /                        /
//     10  11  12  13  14  15  16  17  18  19
//      \   \   \   \______/___/___/___/
//       \   \   \________/___/___/
//        \   \__________/___/
//         \____________/
//
// pn denotes poposal
// block-10: p1
// block-11: p2, uncles-proposals: p3
// block-13 [target]: p1, p3, p4, p5, uncles-proposals: p6
// block-14: p4, txs(p1, p2, p3)
// block-15: txs(p4)
// block-18: txs(p5, p6)
// block-19 [current]
// target's earliest proposals: p4, p5, p6
#[test]
fn test_proposal_reward() {
    let tmp_dir = TempDir::new().unwrap();
    let db = RocksDB::open_in(&tmp_dir, COLUMNS);
    let store = ChainDB::new(db, Default::default());

    let consensus = ConsensusBuilder::default()
        .tx_proposal_window(ProposalWindow(2, 5))
        .build();

    let tx1 = TransactionBuilder::default().version(100u32.pack()).build();
    let tx2 = TransactionBuilder::default().version(200u32.pack()).build();
    let tx3 = TransactionBuilder::default().version(300u32.pack()).build();
    let tx4 = TransactionBuilder::default().version(400u32.pack()).build();
    let tx5 = TransactionBuilder::default().version(500u32.pack()).build();
    let tx6 = TransactionBuilder::default().version(600u32.pack()).build();

    let p1 = tx1.proposal_short_id();
    let p2 = tx2.proposal_short_id();
    let p3 = tx3.proposal_short_id();
    let p4 = tx4.proposal_short_id();
    let p5 = tx5.proposal_short_id();
    let p6 = tx6.proposal_short_id();

    let block_10 = BlockBuilder::default()
        .header(HeaderBuilder::default().number(10u64.pack()).build())
        .proposal(p1.clone())
        .build();

    let uncle = BlockBuilder::default()
        .proposal(p3.clone())
        .build()
        .as_uncle();
    let block_11 = BlockBuilder::default()
        .header(
            HeaderBuilder::default()
                .number(11u64.pack())
                .parent_hash(block_10.hash())
                .build(),
        )
        .proposal(p2)
        .uncle(uncle)
        .build();

    let block_12 = BlockBuilder::default()
        .header(
            HeaderBuilder::default()
                .number(12u64.pack())
                .parent_hash(block_11.hash())
                .build(),
        )
        .build();

    let uncle = BlockBuilder::default().proposal(p6).build().as_uncle();
    let block_13 = BlockBuilder::default()
        .header(
            HeaderBuilder::default()
                .number(13u64.pack())
                .parent_hash(block_12.hash())
                .build(),
        )
        .proposals(vec![p1, p3, p4.clone(), p5])
        .uncle(uncle)
        .build();

    let block_14 = BlockBuilder::default()
        .header(
            HeaderBuilder::default()
                .number(14u64.pack())
                .parent_hash(block_13.hash())
                .build(),
        )
        .proposal(p4)
        .transaction(TransactionBuilder::default().build())
        .transactions(vec![tx1, tx2, tx3])
        .build();

    let block_15 = BlockBuilder::default()
        .header(
            HeaderBuilder::default()
                .number(15u64.pack())
                .parent_hash(block_14.hash())
                .build(),
        )
        .transaction(TransactionBuilder::default().build())
        .transaction(tx4)
        .build();
    let block_16 = BlockBuilder::default()
        .header(
            HeaderBuilder::default()
                .number(16u64.pack())
                .parent_hash(block_15.hash())
                .build(),
        )
        .build();
    let block_17 = BlockBuilder::default()
        .header(
            HeaderBuilder::default()
                .number(17u64.pack())
                .parent_hash(block_16.hash())
                .build(),
        )
        .build();
    let block_18 = BlockBuilder::default()
        .header(
            HeaderBuilder::default()
                .number(18u64.pack())
                .parent_hash(block_17.hash())
                .build(),
        )
        .transaction(TransactionBuilder::default().build())
        .transactions(vec![tx5, tx6])
        .build();

    let ext_tx_fees_14 = vec![
        100u32.into_capacity(),
        20u32.into_capacity(),
        33u32.into_capacity(),
    ];

    let ext_14 = BlockExt {
        received_at: block_14.timestamp(),
        total_difficulty: block_14.difficulty(),
        total_uncles_count: block_14.data().uncles().len() as u64,
        verified: Some(true),
        txs_fees: ext_tx_fees_14,
    };

    // txs(p4)
    let ext_tx_fees_15 = vec![300u32.into_capacity()];

    let ext_15 = BlockExt {
        received_at: block_15.timestamp(),
        total_difficulty: block_15.difficulty(),
        total_uncles_count: block_15.data().uncles().len() as u64,
        verified: Some(true),
        txs_fees: ext_tx_fees_15,
    };

    // txs(p5, p6)
    let ext_tx_fees_18 = vec![41u32.into_capacity(), 999u32.into_capacity()];

    let ext_18 = BlockExt {
        received_at: block_18.timestamp(),
        total_difficulty: block_18.difficulty(),
        total_uncles_count: block_18.data().uncles().len() as u64,
        verified: Some(true),
        txs_fees: ext_tx_fees_18,
    };

    let txn = store.begin_transaction();
    for block in vec![
        block_10,
        block_11,
        block_12.clone(),
        block_13.clone(),
        block_14.clone(),
        block_15.clone(),
        block_16,
        block_17,
        block_18.clone(),
    ] {
        txn.insert_block(&block).unwrap();
        txn.attach_block(&block).unwrap();
    }

    txn.insert_block_ext(&block_14.hash(), &ext_14).unwrap();
    txn.insert_block_ext(&block_15.hash(), &ext_15).unwrap();
    txn.insert_block_ext(&block_18.hash(), &ext_18).unwrap();
    txn.commit().unwrap();

    assert_eq!(block_12.hash(), store.get_block_hash(12).unwrap());

    let reward_calculator = RewardCalculator::new(&consensus, &store);
    let proposal_reward = reward_calculator
        .proposal_reward(&block_18.header(), &block_13.header())
        .unwrap();

    // target's earliest proposals: p4, p5, p6
    let expected: u32 = [300u32, 41u32, 999u32].iter().map(|x| x * 4 / 10).sum();

    assert_eq!(proposal_reward, expected.into_capacity());
}
