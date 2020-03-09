use crate::relayer::block_transactions_process::BlockTransactionsProcess;
use crate::relayer::tests::helper::{build_chain, MockProtocalContext};
use crate::{Status, StatusCode};
use ckb_network::PeerIndex;
use ckb_store::ChainStore;
use ckb_tx_pool::{PlugTarget, TxEntry};
use ckb_types::prelude::*;
use ckb_types::{
    bytes::Bytes,
    core::{BlockBuilder, Capacity, TransactionBuilder},
    packed::{
        self, BlockTransactions, CellInput, CellOutputBuilder, CompactBlock, Header,
        IndexTransaction, OutPoint,
    },
};
use std::collections::{HashMap, HashSet};
use std::iter::FromIterator;
use std::sync::Arc;

#[test]
fn test_accept_block() {
    let (relayer, _) = build_chain(5);
    let peer_index: PeerIndex = 100.into();
    let other_peer_index: PeerIndex = 101.into();

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
    let uncle = BlockBuilder::default()
        .proposals(vec![tx3.proposal_short_id()].pack())
        .build();

    let block = BlockBuilder::default()
        .transactions(vec![tx1, tx2.clone()])
        .uncle(uncle.clone().as_uncle())
        .build();
    let prefilled = HashSet::from_iter(vec![0usize].into_iter());

    let compact_block = CompactBlock::build_from_block(&block, &prefilled);
    let hash = compact_block.header().calc_header_hash();

    {
        let mut pending_compact_blocks = relayer.shared.state().pending_compact_blocks();
        pending_compact_blocks.insert(
            hash.clone(),
            (
                compact_block,
                HashMap::from_iter(vec![
                    (peer_index, (vec![1], vec![0])),
                    (other_peer_index, (vec![1], vec![])),
                ]),
            ),
        );
    }

    let block_transactions: BlockTransactions = packed::BlockTransactions::new_builder()
        .block_hash(block.header().hash())
        .transactions(vec![tx2.data()].pack())
        .uncles(vec![uncle.as_uncle().data()].pack())
        .build();

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);

    let process = BlockTransactionsProcess::new(
        block_transactions.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    assert_eq!(process.execute(), Status::ok());

    let pending_compact_blocks = relayer.shared.state().pending_compact_blocks();
    assert!(pending_compact_blocks.get(&hash).is_none());

    assert!(relayer
        .shared
        .state()
        .inflight_proposals()
        .contains(&tx3.proposal_short_id()));
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
        .transactions(vec![tx1, tx2.clone()])
        .build();

    let prefilled = HashSet::from_iter(vec![0usize].into_iter());

    let compact_block = CompactBlock::build_from_block(&block, &prefilled);

    let foo_peer_index: PeerIndex = 998.into();
    {
        let mut pending_compact_blocks = relayer.shared.state().pending_compact_blocks();
        pending_compact_blocks.insert(
            compact_block.header().calc_header_hash(),
            (
                compact_block,
                HashMap::from_iter(vec![(foo_peer_index, (vec![1], vec![]))]),
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
    assert_eq!(process.execute(), Status::ignored());
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
                .transactions_root(packed::Byte32::zero())
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
        let mut pending_compact_blocks = relayer.shared.state().pending_compact_blocks();
        pending_compact_blocks.insert(
            block_hash.clone(),
            (
                compact_block,
                HashMap::from_iter(vec![(peer_index, (vec![1], vec![]))]),
            ),
        );
    }

    let block_transactions: BlockTransactions = packed::BlockTransactions::new_builder()
        .block_hash(block_hash)
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
    assert_eq!(
        process.execute(),
        StatusCode::CompactBlockHasUnmatchedTransactionRootWithReconstructedBlock.into(),
    );
}

#[test]
fn test_collision_and_send_missing_indexes() {
    let (relayer, _) = build_chain(5);

    let active_chain = relayer.shared.active_chain();
    let last_block = relayer
        .shared
        .store()
        .get_block(&active_chain.tip_hash())
        .unwrap();
    let last_cellbase = last_block.transactions().first().cloned().unwrap();

    let peer_index: PeerIndex = 100.into();

    let tx1 = TransactionBuilder::default().build();
    let tx2 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(1000).unwrap().pack())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();
    let tx3 = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new(last_cellbase.hash(), 0), 0))
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(2).unwrap().pack())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();

    let fake_hash = tx3
        .hash()
        .as_builder()
        .nth31(0u8.into())
        .nth30(0u8.into())
        .nth29(0u8.into())
        .nth28(0u8.into())
        .build();
    // Fake tx with the same ProposalShortId but different hash with tx3
    let fake_tx = tx3.clone().fake_hash(fake_hash);

    assert_eq!(tx3.proposal_short_id(), fake_tx.proposal_short_id());
    assert_ne!(tx3.hash(), fake_tx.hash());

    let block = BlockBuilder::default()
        .transactions(vec![tx1, tx2.clone(), fake_tx])
        .build_unchecked();

    let prefilled = HashSet::from_iter(vec![0usize].into_iter());

    let compact_block = CompactBlock::build_from_block(&block, &prefilled);

    {
        let tx_pool = relayer.shared.shared().tx_pool_controller();
        let entry = TxEntry::new(tx3.clone(), 0, Capacity::shannons(0), 0, vec![]);
        tx_pool
            .plug_entry(vec![entry], PlugTarget::Pending)
            .unwrap();
    }

    let hash = compact_block.header().calc_header_hash();
    {
        let mut pending_compact_blocks = relayer.shared.state().pending_compact_blocks();
        pending_compact_blocks.insert(
            hash.clone(),
            (
                compact_block,
                HashMap::from_iter(vec![(peer_index, (vec![1], vec![]))]),
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
    assert_eq!(
        process.execute(),
        StatusCode::CompactBlockMeetsShortIdsCollision.into()
    );

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

    // update cached missing_index
    {
        let pending_compact_blocks = relayer.shared.state().pending_compact_blocks();
        assert_eq!(
            pending_compact_blocks
                .get(&hash)
                .unwrap()
                .1
                .get(&peer_index),
            Some(&(vec![1, 2], vec![]))
        );
    }

    // resend BlockTransactions with all the transactions without prefilled
    let new_block_transactions = packed::BlockTransactions::new_builder()
        .block_hash(block.header().hash())
        .transactions(vec![tx2.data(), tx3.data()].pack())
        .build();

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);

    let process = BlockTransactionsProcess::new(
        new_block_transactions.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );
    assert_eq!(
        process.execute(),
        StatusCode::CompactBlockHasUnmatchedTransactionRootWithReconstructedBlock.into(),
    );
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
        .transactions(vec![tx1, tx2.clone(), tx3])
        .build();

    let prefilled = HashSet::from_iter(vec![0usize].into_iter());

    let compact_block = CompactBlock::build_from_block(&block, &prefilled);

    // tx3 should be in tx_pool already, but it's not.
    // so the reconstruct block will fail
    {
        let mut pending_compact_blocks = relayer.shared.state().pending_compact_blocks();
        pending_compact_blocks.insert(
            compact_block.header().calc_header_hash(),
            (
                compact_block,
                HashMap::from_iter(vec![(peer_index, (vec![1], vec![]))]),
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
    assert_eq!(
        process.execute(),
        StatusCode::CompactBlockRequiresFreshTransactions.into()
    );

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
