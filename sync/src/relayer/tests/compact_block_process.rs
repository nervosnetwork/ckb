use crate::block_status::BlockStatus;
use crate::relayer::compact_block_process::{CompactBlockProcess, Status};
use crate::relayer::error::{Error, Ignored, Internal, Misbehavior};
use crate::relayer::tests::helper::{build_chain, new_header_builder, MockProtocalContext};
use crate::types::InflightBlocks;
use crate::NetworkProtocol;
use crate::MAX_PEERS_PER_BLOCK;
use ckb_network::PeerIndex;
use ckb_types::prelude::*;
use ckb_types::{
    bytes::Bytes,
    core::{BlockBuilder, Capacity, HeaderBuilder, TransactionBuilder},
    packed::{self, CellOutputBuilder, CompactBlock, ProposalShortId},
    H256,
};
use faketime::unix_time_as_millis;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::iter::FromIterator;
use std::sync::Arc;

#[test]
fn test_in_block_status_map() {
    let (relayer, _) = build_chain(5);

    let block = BlockBuilder::default()
        .number(5.pack())
        .timestamp(unix_time_as_millis().pack())
        .transaction(TransactionBuilder::default().build())
        .build();

    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let compact_block = CompactBlock::build_from_block(&block, &prefilled_transactions_indexes);

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);
    let peer_index: PeerIndex = 1.into();

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    // BLOCK_INVALID in block_status_map
    {
        relayer
            .shared
            .insert_block_status(block.header().hash().to_owned(), BlockStatus::BLOCK_INVALID);
    }

    let r = compact_block_process.execute();
    assert_eq!(
        r.unwrap_err().downcast::<Error>().unwrap(),
        Error::Misbehavior(Misbehavior::BlockInvalid)
    );

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    // BLOCK_STORED in block_status_map
    {
        relayer
            .shared
            .insert_block_status(block.header().hash().clone(), BlockStatus::BLOCK_STORED);
    }

    let r = compact_block_process.execute();
    assert_eq!(
        r.unwrap_err().downcast::<Error>().unwrap(),
        Error::Ignored(Ignored::AlreadyStored)
    );
}

// send_getheaders_to_peer when UnknownParent
#[test]
fn test_unknow_parent() {
    let (relayer, _) = build_chain(5);

    // UnknownParent
    let block = BlockBuilder::default()
        .header(
            HeaderBuilder::default()
                .number(5.pack())
                .timestamp(unix_time_as_millis().pack())
                .build(),
        )
        .transaction(TransactionBuilder::default().build())
        .build();

    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let compact_block = CompactBlock::build_from_block(&block, &prefilled_transactions_indexes);

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);
    let peer_index: PeerIndex = 1.into();

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    let r = compact_block_process.execute();
    assert_eq!(r.ok(), Some(Status::UnknownParent));

    let chain_state = relayer.shared.lock_chain_state();
    let header = chain_state.tip_header();
    let locator_hash = relayer.shared.get_locator(header);

    let content = packed::GetHeaders::new_builder()
        .block_locator_hashes(locator_hash.pack())
        .hash_stop(H256::zero().pack())
        .build();
    let message = packed::SyncMessage::new_builder().set(content).build();
    let data = message.as_slice().into();

    // send_getheaders_to_peer
    assert_eq!(
        nc.as_ref().sent_messages,
        RefCell::new(vec![(NetworkProtocol::SYNC.into(), peer_index, data)])
    );
}

#[test]
fn test_accept_not_a_better_block() {
    let (relayer, _) = build_chain(5);
    let header = {
        let chain_state = relayer.shared.lock_chain_state();
        chain_state.tip_header().clone()
    };

    // The timestamp is random, so it may be not a better block.
    let not_sure_a_better_header = header
        .as_advanced_builder()
        .timestamp((header.timestamp() + 1).pack())
        .build();

    let block = BlockBuilder::default()
        .header(not_sure_a_better_header)
        .transaction(TransactionBuilder::default().build())
        .build();

    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let compact_block = CompactBlock::build_from_block(&block, &prefilled_transactions_indexes);

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);
    let peer_index: PeerIndex = 1.into();

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    let r = compact_block_process.execute();
    assert_eq!(r.ok(), Some(Status::AcceptBlock));
}

#[test]
fn test_already_in_flight() {
    let (relayer, _) = build_chain(5);
    let parent = {
        let chain_state = relayer.shared.lock_chain_state();
        chain_state.tip_header().clone()
    };

    // Better block
    let header = new_header_builder(relayer.shared.shared(), &parent).build();

    // Better block
    let block = BlockBuilder::default()
        .header(header)
        .transaction(TransactionBuilder::default().build())
        .build();

    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let compact_block = CompactBlock::build_from_block(&block, &prefilled_transactions_indexes);

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);
    let peer_index: PeerIndex = 1.into();

    // Already in flight
    let mut in_flight_blocks = InflightBlocks::default();
    in_flight_blocks.insert(peer_index, block.header().hash().clone());
    *relayer.shared.write_inflight_blocks() = in_flight_blocks;

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    let r = compact_block_process.execute();
    assert_eq!(
        r.unwrap_err().downcast::<Error>().unwrap(),
        Error::Ignored(Ignored::AlreadyInFlight)
    );
}

#[test]
fn test_already_pending() {
    let (relayer, _) = build_chain(5);
    let parent = {
        let chain_state = relayer.shared.lock_chain_state();
        chain_state.tip_header().clone()
    };

    // Better block
    let header = new_header_builder(relayer.shared.shared(), &parent).build();

    let block = BlockBuilder::default()
        .header(header)
        .transaction(TransactionBuilder::default().build())
        .build();

    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let compact_block = CompactBlock::build_from_block(&block, &prefilled_transactions_indexes);

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);
    let peer_index: PeerIndex = 1.into();

    // Already in pending
    {
        let mut pending_compact_blocks = relayer.shared.pending_compact_blocks();
        pending_compact_blocks.insert(
            compact_block.header().into_view().hash().clone(),
            (
                compact_block.clone(),
                HashMap::from_iter(vec![(1.into(), vec![0])]),
            ),
        );
    }

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    let r = compact_block_process.execute();
    assert_eq!(
        r.unwrap_err().downcast::<Error>().unwrap(),
        Error::Ignored(Ignored::AlreadyPending)
    );
}

#[test]
fn test_header_invalid() {
    let (relayer, _) = build_chain(5);
    let parent = {
        let chain_state = relayer.shared.lock_chain_state();
        chain_state.tip_header().clone()
    };

    // Better block but block number is invalid
    let header = new_header_builder(relayer.shared.shared(), &parent)
        .number(4.pack())
        .build();

    let block = BlockBuilder::default()
        .header(header)
        .transaction(TransactionBuilder::default().build())
        .build();

    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let compact_block = CompactBlock::build_from_block(&block, &prefilled_transactions_indexes);

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);
    let peer_index: PeerIndex = 1.into();

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    let r = compact_block_process.execute();
    assert_eq!(
        r.unwrap_err().downcast::<Error>().unwrap(),
        Error::Misbehavior(Misbehavior::HeaderInvalid)
    );
    // Assert block_status_map update
    assert_eq!(
        relayer.shared().get_block_status(&block.header().hash()),
        BlockStatus::BLOCK_INVALID
    );
}

#[test]
fn test_inflight_blocks_reach_limit() {
    let (relayer, _) = build_chain(5);
    let parent = {
        let chain_state = relayer.shared.lock_chain_state();
        chain_state.tip_header().clone()
    };

    let header = new_header_builder(relayer.shared.shared(), &parent).build();

    // Better block including one missing transaction
    let block = BlockBuilder::default()
        .header(header.clone())
        .transaction(TransactionBuilder::default().build())
        .transaction(
            TransactionBuilder::default()
                .output(
                    CellOutputBuilder::default()
                        .capacity(Capacity::bytes(1).unwrap().pack())
                        .build(),
                )
                .output_data(Bytes::new().pack())
                .build(),
        )
        .build();

    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let compact_block = CompactBlock::build_from_block(&block, &prefilled_transactions_indexes);

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);
    let peer_index: PeerIndex = 100.into();

    // in_flight_blocks is full
    {
        let mut in_flight_blocks = InflightBlocks::default();
        for i in 0..=MAX_PEERS_PER_BLOCK {
            in_flight_blocks.insert(i.into(), block.header().hash().clone());
        }
        *relayer.shared.write_inflight_blocks() = in_flight_blocks;
    }

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    let r = compact_block_process.execute();
    assert_eq!(
        r.unwrap_err().downcast::<Error>().unwrap(),
        Error::Internal(Internal::InflightBlocksReachLimit)
    );
}

#[test]
fn test_send_missing_indexes() {
    let (relayer, _) = build_chain(5);
    let parent = {
        let chain_state = relayer.shared.lock_chain_state();
        chain_state.tip_header().clone()
    };

    let header = new_header_builder(relayer.shared.shared(), &parent).build();

    let proposal_id = ProposalShortId::new([1u8; 10]);

    // Better block including one missing transaction
    let block = BlockBuilder::default()
        .header(header.clone())
        .transaction(TransactionBuilder::default().build())
        .transaction(
            TransactionBuilder::default()
                .output(
                    CellOutputBuilder::default()
                        .capacity(Capacity::bytes(1).unwrap().pack())
                        .build(),
                )
                .output_data(Bytes::new().pack())
                .build(),
        )
        .proposal(proposal_id.clone())
        .build();

    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let compact_block = CompactBlock::build_from_block(&block, &prefilled_transactions_indexes);

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);
    let peer_index: PeerIndex = 100.into();

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    assert!(!relayer.shared.inflight_proposals().contains(&proposal_id));

    let r = compact_block_process.execute();
    assert_eq!(r.ok(), Some(Status::SendMissingIndexes));

    let content = packed::GetBlockTransactions::new_builder()
        .block_hash(block.header().hash())
        .indexes([1u32].pack())
        .build();
    let message = packed::RelayMessage::new_builder().set(content).build();
    let data = message.as_slice().into();

    // send missing indexes messages
    assert!(nc
        .as_ref()
        .sent_messages_to
        .borrow()
        .contains(&(peer_index, data)));

    // insert inflight proposal
    assert!(relayer.shared.inflight_proposals().contains(&proposal_id));

    let content = packed::GetBlockProposal::new_builder()
        .block_hash(block.header().hash())
        .proposals(vec![proposal_id].into_iter().pack())
        .build();
    let message = packed::RelayMessage::new_builder().set(content).build();
    let data = message.as_slice().into();

    // send proposal request
    assert!(nc
        .as_ref()
        .sent_messages_to
        .borrow()
        .contains(&(peer_index, data)));
}

#[test]
fn test_accept_block() {
    let (relayer, _) = build_chain(5);
    let parent = {
        let chain_state = relayer.shared.lock_chain_state();
        chain_state.tip_header().clone()
    };

    let header = new_header_builder(relayer.shared.shared(), &parent).build();

    // Better block without missing txs
    let block = BlockBuilder::default()
        .header(header.clone())
        .transaction(TransactionBuilder::default().build())
        .build();

    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let compact_block = CompactBlock::build_from_block(&block, &prefilled_transactions_indexes);

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);
    let peer_index: PeerIndex = 100.into();

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    let r = compact_block_process.execute();
    assert_eq!(r.ok(), Some(Status::AcceptBlock));
}

#[test]
fn test_ignore_a_too_old_block() {
    let (relayer, _) = build_chain(1804);
    let parent = {
        let chain_state = relayer.shared.lock_chain_state();
        chain_state.tip_header().clone()
    };
    let parent = relayer.shared.get_ancestor(&parent.hash(), 2).unwrap();

    let too_old_block = new_header_builder(relayer.shared.shared(), &parent).build();

    let block = BlockBuilder::default()
        .header(too_old_block)
        .transaction(TransactionBuilder::default().build())
        .build();

    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let compact_block = CompactBlock::build_from_block(&block, &prefilled_transactions_indexes);

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);
    let peer_index: PeerIndex = 1.into();

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    let r = compact_block_process.execute();
    assert_eq!(
        r.unwrap_err().downcast::<Error>().unwrap(),
        Error::Ignored(Ignored::TooOldBlock)
    );
}

#[test]
fn test_invalid_transaction_root() {
    let (relayer, _) = build_chain(5);
    let parent = {
        let chain_state = relayer.shared.lock_chain_state();
        chain_state.tip_header().clone()
    };

    let header = new_header_builder(relayer.shared.shared(), &parent).build();

    let block = BlockBuilder::default()
        .header(header)
        .transaction(TransactionBuilder::default().build())
        .build_unchecked();

    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let compact_block = CompactBlock::build_from_block(&block, &prefilled_transactions_indexes);

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);
    let peer_index: PeerIndex = 100.into();

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    let r = compact_block_process.execute();
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
fn test_collision() {
    let (relayer, _) = build_chain(5);

    let missing_tx = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(1).unwrap().pack())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();

    let parent = {
        let chain_state = relayer.shared.lock_chain_state();
        chain_state
            .add_tx_to_pool(missing_tx.clone(), 100u16.into())
            .unwrap();
        chain_state.tip_header().clone()
    };

    let header = new_header_builder(relayer.shared.shared(), &parent).build();

    let proposal_id = ProposalShortId::new([1u8; 10]);

    let block = BlockBuilder::default()
        .header(header)
        .transaction(TransactionBuilder::default().build())
        .transaction(missing_tx)
        .proposal(proposal_id.clone())
        .build_unchecked();

    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let compact_block = CompactBlock::build_from_block(&block, &prefilled_transactions_indexes);

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);
    let peer_index: PeerIndex = 100.into();

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    assert!(!relayer.shared.inflight_proposals().contains(&proposal_id));

    let r = compact_block_process.execute();
    assert_eq!(r.ok(), Some(Status::CollisionAndSendMissingIndexes));

    let content = packed::GetBlockTransactions::new_builder()
        .block_hash(block.header().hash())
        .indexes([1u32].pack())
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
