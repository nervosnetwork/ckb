use crate::relayer::compact_block::CompactBlock;
use crate::relayer::compact_block_process::{CompactBlockProcess, Status};
use ckb_core::block::BlockBuilder;
use ckb_core::header::HeaderBuilder;
use ckb_core::transaction::{CellOutput, TransactionBuilder};
use ckb_core::Capacity;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{get_root, CompactBlock as FbsCompactBlock, RelayMessage, SyncMessage};
use flatbuffers::FlatBufferBuilder;
use futures::Future;
use std::collections::HashSet;

use p2p::{bytes::Bytes, service::TargetSession, ProtocolId};
use std::iter::FromIterator;
use std::time::Duration;

use ckb_network::{Behaviour, Error, Peer};

use crate::relayer::tests::helper::{build_chain, new_header_builder};
use crate::types::InflightBlocks;
use crate::NetworkProtocol;
use crate::MAX_PEERS_PER_BLOCK;
use faketime::unix_time_as_millis;
use fnv::FnvHashSet;
use numext_fixed_uint::U256;
use std::cell::RefCell;
use std::convert::TryInto;
use std::sync::Arc;

#[derive(Default)]
struct MockProtocalContext {
    pub sent_messages: RefCell<Vec<(ProtocolId, PeerIndex, Bytes)>>,
    pub sent_messages_to: RefCell<Vec<(PeerIndex, Bytes)>>,
}

impl CKBProtocolContext for MockProtocalContext {
    fn set_notify(&self, _interval: Duration, _token: u64) -> Result<(), Error> {
        unimplemented!()
    }
    fn quick_send_message(
        &self,
        _proto_id: ProtocolId,
        _peer_index: PeerIndex,
        _data: Bytes,
    ) -> Result<(), Error> {
        unimplemented!();
    }
    fn quick_send_message_to(&self, _peer_index: PeerIndex, _data: Bytes) -> Result<(), Error> {
        unimplemented!();
    }
    fn quick_filter_broadcast(&self, _target: TargetSession, _data: Bytes) -> Result<(), Error> {
        unimplemented!();
    }

    fn future_task(
        &self,
        _task: Box<Future<Item = (), Error = ()> + 'static + Send>,
    ) -> Result<(), Error> {
        unimplemented!();
    }
    fn send_message(
        &self,
        proto_id: ProtocolId,
        peer_index: PeerIndex,
        data: Bytes,
    ) -> Result<(), Error> {
        self.sent_messages
            .borrow_mut()
            .push((proto_id, peer_index, data));
        Ok(())
    }
    fn send_message_to(&self, peer_index: PeerIndex, data: Bytes) -> Result<(), Error> {
        self.sent_messages_to.borrow_mut().push((peer_index, data));
        Ok(())
    }

    fn filter_broadcast(&self, _target: TargetSession, _data: Bytes) -> Result<(), Error> {
        unimplemented!();
    }
    fn disconnect(&self, _peer_index: PeerIndex) -> Result<(), Error> {
        unimplemented!();
    }
    fn get_peer(&self, _peer_index: PeerIndex) -> Option<Peer> {
        unimplemented!();
    }
    fn connected_peers(&self) -> Vec<PeerIndex> {
        unimplemented!();
    }
    fn report_peer(&self, _peer_index: PeerIndex, _behaviour: Behaviour) {
        unimplemented!();
    }
    fn ban_peer(&self, _peer_index: PeerIndex, _duration: Duration) {
        unimplemented!();
    }
    fn protocol_id(&self) -> ProtocolId {
        unimplemented!();
    }
}

// send_getheaders_to_peer when UnknownParent
#[test]
fn test_unknow_parent() {
    let (relayer, _) = build_chain(5);

    // UnknownParent
    let block = BlockBuilder::default()
        .header(
            HeaderBuilder::default()
                .number(5)
                .timestamp(unix_time_as_millis())
                .build(),
        )
        .transaction(TransactionBuilder::default().build())
        .build();
    let builder = &mut FlatBufferBuilder::new();
    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let b = FbsCompactBlock::build(builder, &block, &prefilled_transactions_indexes);
    builder.finish(b, None);

    let fbs_compact_block = get_root::<FbsCompactBlock>(builder.finished_data()).unwrap();

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);
    let peer_index: PeerIndex = 1.into();

    let compact_block_process = CompactBlockProcess::new(
        &fbs_compact_block,
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    let r = compact_block_process.execute();
    assert_eq!(r.ok(), Some(Status::UnknownParent));

    let chain_state = relayer.shared.lock_chain_state();
    let header = chain_state.tip_header();
    let locator_hash = relayer.shared.get_locator(header);
    let fbb = &mut FlatBufferBuilder::new();
    let message = SyncMessage::build_get_headers(fbb, &locator_hash);
    fbb.finish(message, None);

    // send_getheaders_to_peer
    assert_eq!(
        nc.as_ref().sent_messages,
        RefCell::new(vec![(
            NetworkProtocol::SYNC.into(),
            peer_index,
            fbb.finished_data().into()
        )])
    );
}

#[test]
fn test_not_a_better_block() {
    let (relayer, _) = build_chain(5);
    let chain_state = relayer.shared.lock_chain_state();
    let header = chain_state.tip_header();

    // Less difficulty
    let block = BlockBuilder::default()
        .header(
            HeaderBuilder::default()
                .parent_hash(header.parent_hash().clone())
                .difficulty(header.difficulty() - U256::from(1u8))
                .number(5)
                .timestamp(unix_time_as_millis())
                .build(),
        )
        .transaction(TransactionBuilder::default().build())
        .build();
    let builder = &mut FlatBufferBuilder::new();
    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let b = FbsCompactBlock::build(builder, &block, &prefilled_transactions_indexes);
    builder.finish(b, None);

    let fbs_compact_block = get_root::<FbsCompactBlock>(builder.finished_data()).unwrap();

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);
    let peer_index: PeerIndex = 1.into();

    let compact_block_process = CompactBlockProcess::new(
        &fbs_compact_block,
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    let r = compact_block_process.execute();
    assert_eq!(r.ok(), Some(Status::NotBetter));
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

    let builder = &mut FlatBufferBuilder::new();
    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let b = FbsCompactBlock::build(builder, &block, &prefilled_transactions_indexes);
    builder.finish(b, None);

    let fbs_compact_block = get_root::<FbsCompactBlock>(builder.finished_data()).unwrap();

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);
    let peer_index: PeerIndex = 1.into();

    // Already in flight
    let mut in_flight_blocks = InflightBlocks::default();
    in_flight_blocks.insert(peer_index, block.header().hash().clone());
    *relayer.shared.write_inflight_blocks() = in_flight_blocks;

    let compact_block_process = CompactBlockProcess::new(
        &fbs_compact_block,
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    let r = compact_block_process.execute();
    assert_eq!(r.ok(), Some(Status::AlreadyInFlight));
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

    let builder = &mut FlatBufferBuilder::new();
    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let b = FbsCompactBlock::build(builder, &block, &prefilled_transactions_indexes);
    builder.finish(b, None);

    let fbs_compact_block = get_root::<FbsCompactBlock>(builder.finished_data()).unwrap();

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);
    let peer_index: PeerIndex = 1.into();

    // Already in pending
    {
        let compact_block: CompactBlock = fbs_compact_block.clone().try_into().unwrap();
        let mut pending_compact_blocks = relayer.shared.pending_compact_blocks();
        pending_compact_blocks.insert(
            compact_block.header.hash().clone(),
            (compact_block, FnvHashSet::from_iter(vec![1.into()])),
        );
    }

    let compact_block_process = CompactBlockProcess::new(
        &fbs_compact_block,
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    let r = compact_block_process.execute();
    assert_eq!(r.ok(), Some(Status::AlreadyPending));
}

#[test]
fn test_header_verify_failed() {
    let (relayer, _) = build_chain(5);
    let parent = {
        let chain_state = relayer.shared.lock_chain_state();
        chain_state.tip_header().clone()
    };

    // Better block but block number is invalid
    let header = new_header_builder(relayer.shared.shared(), &parent)
        .number(4)
        .build();

    let block = BlockBuilder::default()
        .header(header)
        .transaction(TransactionBuilder::default().build())
        .build();

    let builder = &mut FlatBufferBuilder::new();
    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let b = FbsCompactBlock::build(builder, &block, &prefilled_transactions_indexes);
    builder.finish(b, None);

    let fbs_compact_block = get_root::<FbsCompactBlock>(builder.finished_data()).unwrap();

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);
    let peer_index: PeerIndex = 1.into();

    let compact_block_process = CompactBlockProcess::new(
        &fbs_compact_block,
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    let r = compact_block_process.execute();
    assert_eq!(r.ok(), Some(Status::HeaderVerifyFailed));
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
                .output(CellOutput::new(
                    Capacity::bytes(1).unwrap(),
                    Default::default(),
                    Default::default(),
                    None,
                ))
                .build(),
        )
        .build();

    let builder = &mut FlatBufferBuilder::new();
    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let b = FbsCompactBlock::build(builder, &block, &prefilled_transactions_indexes);
    builder.finish(b, None);

    let fbs_compact_block = get_root::<FbsCompactBlock>(builder.finished_data()).unwrap();

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
        &fbs_compact_block,
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    let r = compact_block_process.execute();
    assert_eq!(r.ok(), Some(Status::InflightBlocksReachLimit));
}

#[test]
fn test_send_missing_indexes() {
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
                .output(CellOutput::new(
                    Capacity::bytes(1).unwrap(),
                    Default::default(),
                    Default::default(),
                    None,
                ))
                .build(),
        )
        .build();

    let builder = &mut FlatBufferBuilder::new();
    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let b = FbsCompactBlock::build(builder, &block, &prefilled_transactions_indexes);
    builder.finish(b, None);

    let fbs_compact_block = get_root::<FbsCompactBlock>(builder.finished_data()).unwrap();

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);
    let peer_index: PeerIndex = 100.into();

    let compact_block_process = CompactBlockProcess::new(
        &fbs_compact_block,
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    let r = compact_block_process.execute();
    assert_eq!(r.ok(), Some(Status::SendMissingIndexes));

    let fbb = &mut FlatBufferBuilder::new();
    let message = RelayMessage::build_get_block_transactions(fbb, &block.header().hash(), &[1u32]);
    fbb.finish(message, None);

    // send missing indexes messages
    assert_eq!(
        nc.as_ref().sent_messages_to,
        RefCell::new(vec![(peer_index, fbb.finished_data().into())])
    );
}

#[test]
fn test_no_missing_indexes() {
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

    let builder = &mut FlatBufferBuilder::new();
    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let b = FbsCompactBlock::build(builder, &block, &prefilled_transactions_indexes);
    builder.finish(b, None);

    let fbs_compact_block = get_root::<FbsCompactBlock>(builder.finished_data()).unwrap();

    let mock_protocal_context = MockProtocalContext::default();
    let nc = Arc::new(mock_protocal_context);
    let peer_index: PeerIndex = 100.into();

    let compact_block_process = CompactBlockProcess::new(
        &fbs_compact_block,
        &relayer,
        Arc::<MockProtocalContext>::clone(&nc),
        peer_index,
    );

    let r = compact_block_process.execute();
    assert_eq!(r.ok(), Some(Status::NoMissingIndexes));
}
