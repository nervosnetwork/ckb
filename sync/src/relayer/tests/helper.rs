use crate::{Relayer, SyncSharedState};
use ckb_chain::chain::ChainService;
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::BlockBuilder;
use ckb_core::header::{Header, HeaderBuilder};
use ckb_core::script::Script;
use ckb_core::transaction::{
    CellInput, CellOutput, IndexTransaction, OutPoint, Transaction, TransactionBuilder,
};
use ckb_core::{capacity_bytes, BlockNumber, Bytes, Capacity};
use ckb_db::memorydb::MemoryKeyValueDB;
use ckb_notify::NotifyService;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_store::{ChainKVStore, ChainStore};
use ckb_test_chain_utils::always_success_cell;
use ckb_traits::ChainProvider;
use faketime::{self, unix_time_as_millis};
use numext_fixed_uint::U256;
use std::sync::Arc;

use ckb_network::{Behaviour, Error, Peer};
use ckb_network::{CKBProtocolContext, PeerIndex};
use p2p::{service::TargetSession, ProtocolId};
use std::cell::RefCell;
use std::time::Duration;

pub(crate) fn new_index_transaction(index: usize) -> IndexTransaction {
    let transaction = TransactionBuilder::default()
        .output(CellOutput::new(
            Capacity::bytes(index).unwrap(),
            Default::default(),
            Default::default(),
            None,
        ))
        .build();
    IndexTransaction { index, transaction }
}

pub(crate) fn new_header_builder(
    shared: &Shared<ChainKVStore<MemoryKeyValueDB>>,
    parent: &Header,
) -> HeaderBuilder {
    let parent_hash = parent.hash();
    let parent_epoch = shared.get_block_epoch(&parent_hash).unwrap();
    let epoch = shared
        .next_epoch_ext(&parent_epoch, parent)
        .unwrap_or(parent_epoch);
    HeaderBuilder::default()
        .parent_hash(parent_hash.to_owned())
        .number(parent.number() + 1)
        .timestamp(parent.timestamp() + 1)
        .epoch(epoch.number())
        .difficulty(epoch.difficulty().to_owned())
}

pub(crate) fn new_transaction(
    relayer: &Relayer<ChainKVStore<MemoryKeyValueDB>>,
    index: usize,
    always_success_out_point: &OutPoint,
) -> Transaction {
    let previous_output = {
        let chain_state = relayer.shared.shared().lock_chain_state();
        let tip_hash = chain_state.tip_hash();
        let block = relayer
            .shared
            .shared()
            .store()
            .get_block(&tip_hash)
            .expect("getting tip block");
        let cellbase = block
            .transactions()
            .first()
            .expect("getting cellbase from tip block");
        cellbase.output_pts()[0].clone()
    };

    TransactionBuilder::default()
        .input(CellInput::new(previous_output, 0))
        .output(CellOutput::new(
            Capacity::bytes(500 + index).unwrap(), // use capacity to identify transactions
            Default::default(),
            Default::default(),
            None,
        ))
        .dep(always_success_out_point.to_owned())
        .build()
}

pub(crate) fn build_chain(tip: BlockNumber) -> (Relayer<ChainKVStore<MemoryKeyValueDB>>, OutPoint) {
    let (always_success_cell, always_success_script) = always_success_cell();
    let always_success_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .output(always_success_cell.clone())
        .witness(always_success_script.clone().into_witness())
        .build();
    let always_success_out_point = OutPoint::new_cell(always_success_tx.hash().to_owned(), 0);

    let shared = {
        let genesis = BlockBuilder::from_header_builder(
            HeaderBuilder::default()
                .timestamp(unix_time_as_millis())
                .difficulty(U256::from(1000u64)),
        )
        .transaction(always_success_tx)
        .build();
        let consensus = Consensus::default()
            .set_genesis_block(genesis)
            .set_cellbase_maturity(0);
        SharedBuilder::<MemoryKeyValueDB>::new()
            .consensus(consensus)
            .build()
            .unwrap()
    };
    let chain_controller = {
        let notify_controller = NotifyService::default().start::<&str>(None);
        let chain_service = ChainService::new(shared.clone(), notify_controller);
        chain_service.start::<&str>(None)
    };

    // Build 1 ~ (tip-1) heights
    for i in 0..tip {
        let parent = shared
            .store()
            .get_block_hash(i)
            .and_then(|block_hash| shared.store().get_block(&block_hash))
            .unwrap();
        let cellbase = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(parent.header().number() + 1))
            .output(CellOutput::new(
                capacity_bytes!(50000),
                Bytes::default(),
                always_success_script.to_owned(),
                None,
            ))
            .witness(Script::default().into_witness())
            .build();
        let block = BlockBuilder::from_header_builder(new_header_builder(&shared, parent.header()))
            .transaction(cellbase)
            .build();
        chain_controller
            .process_block(Arc::new(block), false)
            .expect("processing block should be ok");
    }

    let sync_shared_state = Arc::new(SyncSharedState::new(shared));
    (
        Relayer::new(chain_controller, sync_shared_state),
        always_success_out_point,
    )
}

#[derive(Default)]
pub(crate) struct MockProtocalContext {
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
        _task: Box<
            (dyn futures::future::Future<Item = (), Error = ()> + std::marker::Send + 'static),
        >,
        _blocking: bool,
    ) -> Result<(), Error> {
        Ok(())
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
    fn disconnect(&self, _peer_index: PeerIndex, _message: &str) -> Result<(), Error> {
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
    fn send_paused(&self) -> bool {
        false
    }
}
