use crate::relayer::block_transactions_process::{BlockTransactionsProcess, Status};
use crate::relayer::error::{Error, Misbehavior};
use crate::relayer::tests::helper::{build_chain, MockProtocalContext};
use ckb_network::PeerIndex;
use ckb_types::prelude::*;
use ckb_types::{
    bytes::Bytes,
    core::{BlockBuilder, Capacity, TransactionBuilder},
    packed::{self, BlockTransactions, CellOutputBuilder, CompactBlock, Header, IndexTransaction},
    H256,
};
use std::collections::HashMap;
use std::collections::HashSet;
use std::iter::FromIterator;
use std::sync::Arc;

#[test]
fn test_accept_block() {
    let (relayer, _) = build_chain(5);
    let peer_index: PeerIndex = 100.into();

    let tx1 = TransactionBuilder::default().build();
    let tx2 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(1).unwrap().pack())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();

    let block = BlockBuilder::default()
        .transactions(vec![tx1.clone(), tx2.clone()])
        .build();
    let prefilled = HashSet::from_iter(vec![0usize].into_iter());

    let compact_block = CompactBlock::build_from_block(&block, &prefilled);

    {
        let mut pending_compact_blocks = relayer.shared.pending_compact_blocks();
        pending_compact_blocks.insert(
            compact_block.header().calc_header_hash().pack(),
            (
                compact_block,
                HashMap::from_iter(vec![(peer_index, vec![1])]),
            ),
        );
    }

    let block_transactions: BlockTransactions = packed::BlockTransactions::new_builder()
        .block_hash(block.header().hash())
        .transactions(vec![tx2.data()].pack())
        .build();

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);

    let process = BlockTransactionsProcess::new(
        block_transactions.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    let r = process.execute();

    assert_eq!(r.ok(), Some(Status::Accept));
}

#[test]
fn test_unknown_request() {
    let (relayer, _) = build_chain(5);
    let peer_index: PeerIndex = 100.into();

    let tx1 = TransactionBuilder::default().build();
    let tx2 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(1).unwrap().pack())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();

    let block = BlockBuilder::default()
        .transactions(vec![tx1.clone(), tx2.clone()])
        .build();

    let prefilled = HashSet::from_iter(vec![0usize].into_iter());

    let compact_block = CompactBlock::build_from_block(&block, &prefilled);

    let foo_peer_index: PeerIndex = 998.into();
    {
        let mut pending_compact_blocks = relayer.shared.pending_compact_blocks();
        pending_compact_blocks.insert(
            compact_block.header().calc_header_hash().pack(),
            (
                compact_block,
                HashMap::from_iter(vec![(foo_peer_index, vec![1])]),
            ),
        );
    }

    let block_transactions: BlockTransactions = packed::BlockTransactions::new_builder()
        .block_hash(block.header().hash())
        .transactions(vec![tx2.data()].pack())
        .build();

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);

    let process = BlockTransactionsProcess::new(
        block_transactions.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    let r = process.execute();
    assert_eq!(r.ok(), Some(Status::UnkownRequest));
}

#[test]
fn test_invalid_transaction_root() {
    let (relayer, _) = build_chain(5);
    let peer_index: PeerIndex = 100.into();

    let tx1 = TransactionBuilder::default().build();
    let tx2 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(1).unwrap().pack())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();

    let prefilled = IndexTransaction::new_builder()
        .index(0u32.pack())
        .transaction(tx1.data())
        .build();

    let header_with_invalid_tx_root = Header::new_builder()
        .raw(
            packed::RawHeader::new_builder()
                .transactions_root(H256::zero().pack())
                .build(),
        )
        .build();

    let compact_block = packed::CompactBlock::new_builder()
        .header(header_with_invalid_tx_root)
        .short_ids(vec![tx2.proposal_short_id()].pack())
        .prefilled_transactions(vec![prefilled].pack())
        .build();

    let block_hash = compact_block.header().calc_header_hash();

    {
        let mut pending_compact_blocks = relayer.shared.pending_compact_blocks();
        pending_compact_blocks.insert(
            block_hash.clone().pack(),
            (
                compact_block,
                HashMap::from_iter(vec![(peer_index, vec![1])]),
            ),
        );
    }

    let block_transactions: BlockTransactions = packed::BlockTransactions::new_builder()
        .block_hash(block_hash.pack())
        .transactions(vec![tx2.data()].pack())
        .build();

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);

    let process = BlockTransactionsProcess::new(
        block_transactions.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    let r = process.execute();
    assert_eq!(
        r.unwrap_err().downcast::<Error>().unwrap(),
        Error::Misbehavior(Misbehavior::InvalidTransactionRoot)
    );
}

// Generate a transaction T, and add that transaction
// to the proposed pool, as usual.
// Generate a block, which includes the transaction.
// Change the merkle root to other value.
// Send the block as compact block, which does not prefill T.
// The test should work because from the peer's perspective,
// it cannot tell the differences between a collision and
// a real unmatched merkle root.
#[test]
fn test_collision_and_send_missing_indexes() {
    let (relayer, _) = build_chain(5);
    let peer_index: PeerIndex = 100.into();

    let tx1 = TransactionBuilder::default().build();
    let tx2 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(1).unwrap().pack())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();
    let tx3 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(2).unwrap().pack())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();

    let block = BlockBuilder::default()
        .transactions(vec![tx1.clone(), tx2.clone(), tx3.clone()])
        .build_unchecked();

    let prefilled = HashSet::from_iter(vec![0usize].into_iter());

    let compact_block = CompactBlock::build_from_block(&block, &prefilled);

    {
        let chain_state = relayer.shared.lock_chain_state();
        chain_state.add_tx_to_pool(tx3, 100u16.into()).unwrap();
    }

    {
        let mut pending_compact_blocks = relayer.shared.pending_compact_blocks();
        pending_compact_blocks.insert(
            compact_block.header().calc_header_hash().pack(),
            (
                compact_block,
                HashMap::from_iter(vec![(peer_index, vec![1])]),
            ),
        );
    }

    let block_transactions = packed::BlockTransactions::new_builder()
        .block_hash(block.header().hash())
        .transactions(vec![tx2.data()].pack())
        .build();

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);

    let process = BlockTransactionsProcess::new(
        block_transactions.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    let r = process.execute();
    assert_eq!(r.ok(), Some(Status::CollisionAndSendMissingIndexes));

    let content = packed::GetBlockTransactions::new_builder()
        .block_hash(block.header().hash())
        .indexes(vec![1u32, 2u32].pack())
        .build();
    let message = packed::RelayMessage::new_builder().set(content).build();
    let data = message.as_slice().into();

    // send missing indexes messages
    assert!(nc
        .as_ref()
        .sent_messages_to
        .borrow()
        .contains(&(peer_index, data)));
}

#[test]
fn test_missing() {
    let (relayer, _) = build_chain(5);
    let peer_index: PeerIndex = 100.into();

    let tx1 = TransactionBuilder::default().build();
    let tx2 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(1).unwrap().pack())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();
    let tx3 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(2).unwrap().pack())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();

    let block = BlockBuilder::default()
        .transactions(vec![tx1.clone(), tx2.clone(), tx3.clone()])
        .build();

    let prefilled = HashSet::from_iter(vec![0usize].into_iter());

    let compact_block = CompactBlock::build_from_block(&block, &prefilled);

    // tx3 should be in tx_pool already, but it's not.
    // so the reconstruct block will fail
    {
        let mut pending_compact_blocks = relayer.shared.pending_compact_blocks();
        pending_compact_blocks.insert(
            compact_block.header().calc_header_hash().pack(),
            (
                compact_block,
                HashMap::from_iter(vec![(peer_index, vec![1])]),
            ),
        );
    }

    let block_transactions = packed::BlockTransactions::new_builder()
        .block_hash(block.header().hash())
        .transactions(vec![tx2.data()].pack())
        .build();

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);

    let process = BlockTransactionsProcess::new(
        block_transactions.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    let r = process.execute();
    assert_eq!(r.ok(), Some(Status::Missing));

    let content = packed::GetBlockTransactions::new_builder()
        .block_hash(block.header().hash())
        .indexes(vec![2u32].pack())
        .build();
    let message = packed::RelayMessage::new_builder().set(content).build();
    let data = message.as_slice().into();

    // send missing indexes messages
    assert!(nc
        .as_ref()
        .sent_messages_to
        .borrow()
        .contains(&(peer_index, data)));
}
