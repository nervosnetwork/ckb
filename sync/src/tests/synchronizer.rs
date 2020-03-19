use crate::synchronizer::{
    IBD_BLOCK_FETCH_TOKEN, NOT_IBD_BLOCK_FETCH_TOKEN, SEND_GET_HEADERS_TOKEN,
    TIMEOUT_EVICTION_TOKEN,
};
use crate::tests::TestNode;
use crate::{NetworkProtocol, SyncShared, Synchronizer};
use ckb_chain::{chain::ChainService, switch::Switch};
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_dao::DaoCalculator;
use ckb_dao_utils::genesis_dao_data;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_store::ChainStore;
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
use faketime::{self, unix_time_as_millis};
use std::collections::HashSet;
use std::sync::mpsc::sync_channel;
use std::sync::Arc;
use std::thread;

const DEFAULT_CHANNEL: usize = 128;

#[test]
fn basic_sync() {
    let faketime_file = faketime::millis_tempfile(0).expect("create faketime file");
    faketime::enable(&faketime_file);
    let thread_name = format!("FAKETIME={}", faketime_file.display());

    let (mut node1, shared1) = setup_node(1);
    let (mut node2, shared2) = setup_node(3);

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
    let (shared, table) = SharedBuilder::default()
        .consensus(consensus)
        .build()
        .unwrap();

    let chain_service = ChainService::new(shared.clone(), table);
    let chain_controller = chain_service.start::<&str>(None);

    for _i in 0..height {
        let number = block.header().number() + 1;
        let timestamp = block.header().timestamp() + 1;

        let snapshot = shared.snapshot();
        let last_epoch = snapshot.get_block_epoch(&block.header().hash()).unwrap();
        let epoch = snapshot
            .next_epoch_ext(snapshot.consensus(), &last_epoch, &block.header())
            .unwrap_or(last_epoch);

        let (_, reward) = snapshot.finalize_block_reward(&block.header()).unwrap();

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
            DaoCalculator::new(shared.consensus(), snapshot.as_ref())
                .dao_field(&[resolved_cellbase], &block.header())
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
            .internal_process_block(Arc::new(block.clone()), Switch::DISABLE_ALL)
            .expect("process block should be OK");
    }

    let sync_shared = Arc::new(SyncShared::new(shared.clone()));
    let synchronizer = Synchronizer::new(chain_controller, sync_shared);
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
