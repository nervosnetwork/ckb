use crate::{Relayer, SyncSharedState};
use ckb_chain::chain::ChainService;
use ckb_chain_spec::consensus::Consensus;
use ckb_network::{
    Behaviour, CKBProtocolContext, Error, Peer, PeerIndex, ProtocolId, TargetSession,
};
use ckb_notify::NotifyService;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_store::ChainStore;
use ckb_test_chain_utils::always_success_cell;
use ckb_traits::ChainProvider;
use ckb_types::prelude::*;
use ckb_types::{
    bytes::Bytes,
    core::{
        capacity_bytes, BlockBuilder, BlockNumber, Capacity, HeaderBuilder, HeaderView,
        TransactionBuilder, TransactionView,
    },
    packed::{
        CellDep, CellInput, CellOutputBuilder, IndexTransaction, IndexTransactionBuilder, OutPoint,
        Script,
    },
    U256,
};
use faketime::{self, unix_time_as_millis};
use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

pub(crate) fn new_index_transaction(index: usize) -> IndexTransaction {
    let transaction = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(index).unwrap().pack())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();

    IndexTransactionBuilder::default()
        .index(index.pack())
        .transaction(transaction.data())
        .build()
}

pub(crate) fn new_header_builder(shared: &Shared, parent: &HeaderView) -> HeaderBuilder {
    let parent_hash = parent.hash();
    let parent_epoch = shared.store().get_block_epoch(&parent_hash).unwrap();
    let epoch = shared
        .next_epoch_ext(&parent_epoch, parent)
        .unwrap_or(parent_epoch);
    HeaderBuilder::default()
        .parent_hash(parent_hash.to_owned())
        .number((parent.number() + 1).pack())
        .timestamp((parent.timestamp() + 1).pack())
        .epoch(epoch.number().pack())
        .difficulty(epoch.difficulty().pack())
}

pub(crate) fn new_transaction(
    relayer: &Relayer,
    index: usize,
    always_success_out_point: &OutPoint,
) -> TransactionView {
    let previous_output = {
        let snapshot = relayer.shared.shared().snapshot();
        let tip_hash = snapshot.tip_header().hash();
        let block = relayer
            .shared
            .shared()
            .store()
            .get_block(&tip_hash)
            .expect("getting tip block");
        let txs = block.transactions();
        let cellbase = txs.first().expect("getting cellbase from tip block");
        cellbase.output_pts()[0].clone()
    };

    TransactionBuilder::default()
        .input(CellInput::new(previous_output, 0))
        .output(
            CellOutputBuilder::default()
            .capacity(Capacity::bytes(500 + index).unwrap().pack()) // use capacity to identify transactions
            .build(),
        )
        .output_data(Bytes::new().pack())
        .cell_dep(
            CellDep::new_builder()
                .out_point(always_success_out_point.to_owned())
                .build(),
        )
        .build()
}

pub(crate) fn build_chain(tip: BlockNumber) -> (Relayer, OutPoint) {
    let (always_success_cell, always_success_cell_data, always_success_script) =
        always_success_cell();
    let always_success_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .output(always_success_cell.clone())
        .output_data(always_success_cell_data.pack())
        .witness(always_success_script.clone().into_witness())
        .build();
    let always_success_out_point = OutPoint::new(always_success_tx.hash(), 0);

    let (shared, table) = {
        let genesis = BlockBuilder::default()
            .timestamp(unix_time_as_millis().pack())
            .difficulty(U256::from(1000u64).pack())
            .transaction(always_success_tx)
            .build();
        let consensus = Consensus::default()
            .set_genesis_block(genesis)
            .set_cellbase_maturity(0);
        SharedBuilder::default()
            .consensus(consensus)
            .build()
            .unwrap()
    };
    let chain_controller = {
        let chain_service = ChainService::new(shared.clone(), table);
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
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(50000).pack())
                    .lock(always_success_script.to_owned())
                    .build(),
            )
            .output_data(Bytes::new().pack())
            .witness(Script::default().into_witness())
            .build();
        let header = new_header_builder(&shared, &parent.header()).build();
        let block = BlockBuilder::default()
            .header(header)
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
