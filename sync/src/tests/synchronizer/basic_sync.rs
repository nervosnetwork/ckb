use crate::synchronizer::{
    IBD_BLOCK_FETCH_TOKEN, NOT_IBD_BLOCK_FETCH_TOKEN, SEND_GET_HEADERS_TOKEN,
    TIMEOUT_EVICTION_TOKEN,
};
use crate::tests::TestNode;
use crate::{SyncShared, Synchronizer};
use ckb_chain::start_chain_services;
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_channel::bounded;
use ckb_dao::DaoCalculator;
use ckb_dao_utils::genesis_dao_data;
use ckb_logger::info;
use ckb_logger_service::LoggerInitGuard;
use ckb_network::SupportProtocols;
use ckb_reward_calculator::RewardCalculator;
use ckb_shared::{Shared, SharedBuilder};
use ckb_store::ChainStore;
use ckb_systemtime::{self, unix_time_as_millis};
use ckb_test_chain_utils::always_success_cell;
use ckb_types::prelude::*;
use ckb_types::{
    bytes::Bytes,
    core::{cell::resolve_transaction, BlockBuilder, EpochNumberWithFraction, TransactionBuilder},
    packed::{self, CellInput, CellOutputBuilder, OutPoint},
    utilities::difficulty_to_compact,
    U256,
};
use ckb_util::RwLock;
use ckb_verification_traits::Switch;
use std::collections::HashSet;
use std::sync::Arc;

const DEFAULT_CHANNEL: usize = 128;

#[test]
fn basic_sync() {
    let _log_guard: LoggerInitGuard = ckb_logger_service::init_for_test("debug").expect("init log");
    let _faketime_guard = ckb_systemtime::faketime();
    _faketime_guard.set_faketime(0);
    let thread_name = "fake_time=0".to_string();

    let (mut node1, shared1) = setup_node(1);
    info!("finished setup node1");
    let (mut node2, shared2) = setup_node(3);
    info!("finished setup node2");

    info!("connnectiong node1 and node2");
    node1.connect(&mut node2, SupportProtocols::Sync.protocol_id());
    info!("node1 and node2 connected");

    let now = std::time::Instant::now();
    let (signal_tx1, signal_rx1) = bounded(DEFAULT_CHANNEL);
    node1.start(thread_name.clone(), signal_tx1, move |data| {
        let msg = packed::SyncMessage::from_slice(&data)
            .expect("sync message")
            .to_enum();

        assert!(
            now.elapsed().as_secs() <= 10,
            "node1 should got block(3)'s SendBlock message within 10 seconds"
        );
        // terminate thread after 3 blocks
        if let packed::SyncMessageUnionReader::SendBlock(reader) = msg.as_reader() {
            let block = reader.block().to_entity().into_view();
            block.header().number() == 3
        } else {
            false
        }
    });

    let (signal_tx2, _) = bounded(DEFAULT_CHANNEL);
    node2.start(thread_name, signal_tx2, |_| false);

    // Wait node1 receive block from node2
    let _ = signal_rx1.recv();

    let test_start = std::time::Instant::now();
    while test_start.elapsed().as_secs() < 3 {
        info!("node1 tip_number: {}", shared1.snapshot().tip_number());
        if shared1.snapshot().tip_number() == 3 {
            assert_eq!(shared1.snapshot().tip_number(), 3);
            assert_eq!(
                shared1.snapshot().tip_number(),
                shared2.snapshot().tip_number()
            );

            node1.stop();
            node2.stop();
            return;
        }
    }
    panic!("node1 and node2 should sync in 3 seconds");
}

fn setup_node(height: u64) -> (TestNode, Shared) {
    let (always_success_cell, always_success_cell_data, always_success_script) =
        always_success_cell();
    let always_success_tx = TransactionBuilder::default()
        .witness(always_success_script.clone().into_witness())
        .input(CellInput::new(OutPoint::null(), 0))
        .output(always_success_cell.clone())
        .output_data(always_success_cell_data.pack())
        .build();

    let dao = genesis_dao_data(vec![&always_success_tx]).unwrap();

    let mut block = BlockBuilder::default()
        .timestamp(unix_time_as_millis().pack())
        .compact_target(difficulty_to_compact(U256::from(1000u64)).pack())
        .dao(dao)
        .transaction(always_success_tx)
        .build();

    let consensus = ConsensusBuilder::default()
        .genesis_block(block.clone())
        .cellbase_maturity(EpochNumberWithFraction::new(0, 0, 1))
        .build();
    let (shared, mut pack) = SharedBuilder::with_temp_db()
        .consensus(consensus)
        .build()
        .unwrap();

    let chain_controller = start_chain_services(pack.take_chain_services_builder());

    for _i in 0..height {
        let number = block.header().number() + 1;
        let timestamp = block.header().timestamp() + 1;

        let snapshot = shared.snapshot();
        let epoch = snapshot
            .consensus()
            .next_epoch_ext(&block.header(), &snapshot.borrow_as_data_loader())
            .unwrap()
            .epoch();

        let (_, reward) = RewardCalculator::new(snapshot.consensus(), snapshot.as_ref())
            .block_reward_to_finalize(&block.header())
            .unwrap();

        let builder = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(number))
            .witness(always_success_script.to_owned().into_witness());

        let cellbase = if number <= snapshot.consensus().finalization_delay_length() {
            builder.build()
        } else {
            builder
                .output(
                    CellOutputBuilder::default()
                        .capacity(reward.total.pack())
                        .lock(always_success_script.to_owned())
                        .build(),
                )
                .output_data(Bytes::default().pack())
                .build()
        };

        let dao = {
            let resolved_cellbase = resolve_transaction(
                cellbase.clone(),
                &mut HashSet::new(),
                snapshot.as_ref(),
                snapshot.as_ref(),
            )
            .unwrap();
            let data_loader = snapshot.borrow_as_data_loader();
            DaoCalculator::new(shared.consensus(), &data_loader)
                .dao_field([resolved_cellbase].iter(), &block.header())
                .unwrap()
        };

        block = BlockBuilder::default()
            .transaction(cellbase)
            .parent_hash(block.header().hash())
            .number(number.pack())
            .epoch(epoch.number_with_fraction(number).pack())
            .timestamp(timestamp.pack())
            .compact_target(epoch.compact_target().pack())
            .dao(dao)
            .build();

        chain_controller
            .blocking_process_block_with_switch(Arc::new(block.clone()), Switch::DISABLE_ALL)
            .expect("process block should be OK");
    }

    let sync_shared = Arc::new(SyncShared::new(
        shared.clone(),
        Default::default(),
        pack.take_relay_tx_receiver(),
    ));
    let synchronizer = Synchronizer::new(
        chain_controller,
        sync_shared,
        pack.take_verify_failed_block_rx(),
    );
    let mut node = TestNode::new();
    let protocol = Arc::new(RwLock::new(synchronizer)) as Arc<_>;
    node.add_protocol(
        SupportProtocols::Sync.protocol_id(),
        &protocol,
        &[
            SEND_GET_HEADERS_TOKEN,
            IBD_BLOCK_FETCH_TOKEN,
            NOT_IBD_BLOCK_FETCH_TOKEN,
            TIMEOUT_EVICTION_TOKEN,
        ],
    );
    (node, shared)
}
