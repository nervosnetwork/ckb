use crate::synchronizer::{
    IBD_BLOCK_FETCH_TOKEN, NOT_IBD_BLOCK_FETCH_TOKEN, SEND_GET_HEADERS_TOKEN,
    TIMEOUT_EVICTION_TOKEN,
};
use crate::tests::TestNode;
use crate::{NetworkProtocol, SyncSharedState, Synchronizer};
use ckb_chain::chain::ChainService;
use ckb_chain_spec::consensus::Consensus;
use ckb_dao::DaoCalculator;
use ckb_dao_utils::genesis_dao_data;
use ckb_notify::NotifyService;
use ckb_shared::{
    shared::{Shared, SharedBuilder},
    Snapshot,
};
use ckb_store::ChainStore;
use ckb_test_chain_utils::always_success_cell;
use ckb_traits::ChainProvider;
use ckb_types::prelude::*;
use ckb_types::{
    bytes::Bytes,
    core::{cell::resolve_transaction, BlockBuilder, TransactionBuilder},
    packed::{self, CellInput, CellOutputBuilder, OutPoint},
    U256,
};
use ckb_util::RwLock;
use faketime::{self, unix_time_as_millis};
use std::collections::HashSet;
use std::sync::mpsc::sync_channel;
use std::sync::Arc;
use std::thread;

const DEFAULT_CHANNEL: usize = 128;

#[test]
fn basic_sync() {
    ckb_store::set_cache_enable(false);
    let faketime_file = faketime::millis_tempfile(0).expect("create faketime file");
    faketime::enable(&faketime_file);
    let thread_name = format!("FAKETIME={}", faketime_file.display());

    let (mut node1, shared1) = setup_node(&thread_name, 1);
    let (mut node2, shared2) = setup_node(&thread_name, 3);

    node1.connect(&mut node2, NetworkProtocol::SYNC.into());

    let (signal_tx1, signal_rx1) = sync_channel(DEFAULT_CHANNEL);
    thread::Builder::new()
        .name(thread_name.clone())
        .spawn(move || {
            node1.start(&signal_tx1, |data| {
                let msg = packed::SyncMessage::from_slice(&data)
                    .expect("sync message")
                    .to_enum();
                // terminate thread after 3 blocks
                if let packed::SyncMessageUnionReader::SendBlock(reader) = msg.as_reader() {
                    let block = reader.block().to_entity().into_view();
                    block.header().number() == 3
                } else {
                    false
                }
            });
        })
        .expect("thread spawn");

    let (signal_tx2, _) = sync_channel(DEFAULT_CHANNEL);
    thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            node2.start(&signal_tx2, |_| false);
        })
        .expect("thread spawn");

    // Wait node1 receive block from node2
    let _ = signal_rx1.recv();

    assert_eq!(shared1.snapshot().tip_number(), 3);
    assert_eq!(
        shared1.snapshot().tip_number(),
        shared2.snapshot().tip_number()
    );
}

fn setup_node(thread_name: &str, height: u64) -> (TestNode, Shared) {
    let (always_success_cell, always_success_cell_data, always_success_script) =
        always_success_cell();
    let always_success_tx = TransactionBuilder::default()
        .witness(always_success_script.clone().into_witness())
        .input(CellInput::new(OutPoint::null(), 0))
        .output(always_success_cell.clone())
        .output_data(always_success_cell_data.pack())
        .build();

    let dao = genesis_dao_data(&always_success_tx).unwrap();

    let mut block = BlockBuilder::default()
        .timestamp(unix_time_as_millis().pack())
        .difficulty(U256::from(1000u64).pack())
        .dao(dao)
        .transaction(always_success_tx)
        .build();

    let consensus = Consensus::default()
        .set_genesis_block(block.clone())
        .set_cellbase_maturity(0);
    let (shared, table) = SharedBuilder::default()
        .consensus(consensus)
        .build()
        .unwrap();
    let notify = NotifyService::default().start(Some(thread_name));

    let chain_service = ChainService::new(shared.clone(), table, notify);
    let chain_controller = chain_service.start::<&str>(None);

    for _i in 0..height {
        let number = block.header().number() + 1;
        let timestamp = block.header().timestamp() + 1;

        let last_epoch = shared
            .store()
            .get_block_epoch(&block.header().hash())
            .unwrap();
        let epoch = shared
            .next_epoch_ext(&last_epoch, &block.header())
            .unwrap_or(last_epoch);

        let (_, reward) = shared.finalize_block_reward(&block.header()).unwrap();

        let cellbase = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(number))
            .output(
                CellOutputBuilder::default()
                    .capacity(reward.total.pack())
                    .lock(always_success_script.to_owned())
                    .build(),
            )
            .output_data(Bytes::default().pack())
            .witness(always_success_script.to_owned().into_witness())
            .build();

        let dao = {
            let snapshot: &Snapshot = &shared.snapshot();
            let resolved_cellbase =
                resolve_transaction(&cellbase, &mut HashSet::new(), snapshot, snapshot).unwrap();
            DaoCalculator::new(shared.consensus(), shared.store())
                .dao_field(&[resolved_cellbase], &block.header())
                .unwrap()
        };

        block = BlockBuilder::default()
            .transaction(cellbase)
            .parent_hash(block.header().hash().to_owned())
            .number(number.pack())
            .epoch(epoch.number().pack())
            .timestamp(timestamp.pack())
            .difficulty(epoch.difficulty().pack())
            .dao(dao)
            .build();

        chain_controller
            .process_block(Arc::new(block.clone()), false)
            .expect("process block should be OK");
    }

    let sync_shared_state = Arc::new(SyncSharedState::new(shared.clone()));
    let synchronizer = Synchronizer::new(chain_controller, sync_shared_state);
    let mut node = TestNode::default();
    let protocol = Arc::new(RwLock::new(synchronizer)) as Arc<_>;
    node.add_protocol(
        NetworkProtocol::SYNC.into(),
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
