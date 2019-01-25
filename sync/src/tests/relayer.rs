use crate::relayer::TX_PROPOSAL_TOKEN;
use crate::tests::TestNode;
use crate::{Relayer, RELAY_PROTOCOL_ID};
use ckb_chain::chain::{ChainBuilder, ChainController};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::BlockBuilder;
use ckb_core::header::HeaderBuilder;
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, TransactionBuilder};
use ckb_db::memorydb::MemoryKeyValueDB;
use ckb_notify::NotifyService;
use ckb_pool::txs_pool::{PoolConfig, TransactionPoolController, TransactionPoolService};
use ckb_protocol::RelayMessage;
use ckb_shared::shared::{ChainProvider, Shared, SharedBuilder};
use ckb_shared::store::ChainKVStore;
use faketime::{self, unix_time_as_millis};
use flatbuffers::get_root;
use flatbuffers::FlatBufferBuilder;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::channel;
use std::sync::{Arc, Barrier};
use std::{thread, time};

#[test]
fn relay_compact_block_with_one_tx() {
    let faketime_file = faketime::millis_tempfile(0).expect("create faketime file");
    faketime::enable(&faketime_file);
    let thread_name = format!("FAKETIME={}", faketime_file.display());

    // Use the same thread name for all child threads, so the time is mocked in all these threads.
    // This is required because the test relies on the determined timestamp. Now all the threads
    // freeze the timestamp at UNIX EPOCH.
    let (mut node1, shared1, chain_controller1) = setup_node(&thread_name, 3);
    let (mut node2, shared2, _chain_controller2) = setup_node(&thread_name, 3);
    let barrier = Arc::new(Barrier::new(2));

    node1.connect(&mut node2, RELAY_PROTOCOL_ID);

    let (signal_tx1, _) = channel();
    let barrier1 = Arc::clone(&barrier);
    thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let last_block = shared1
                .block(&shared1.chain_state().read().tip_hash())
                .unwrap();
            let last_cellbase = last_block.commit_transactions().first().unwrap();

            // building tx and broadcast it
            let tx = TransactionBuilder::default()
                .input(CellInput::new(
                    OutPoint::new(last_cellbase.hash().clone(), 0),
                    create_valid_script(),
                ))
                .output(CellOutput::new(50, Vec::new(), H256::zero(), None))
                .build();

            {
                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_transaction(fbb, &tx);
                fbb.finish(message, None);
                node1.broadcast(RELAY_PROTOCOL_ID, &fbb.finished_data().to_vec());
            }

            // building 1st compact block with tx proposal and broadcast it
            let block = {
                let number = last_block.header().number() + 1;
                let timestamp = last_block.header().timestamp() + 1;
                let difficulty = shared1.calculate_difficulty(&last_block.header()).unwrap();
                let cellbase = TransactionBuilder::default()
                    .input(CellInput::new_cellbase_input(number))
                    .output(CellOutput::default())
                    .build();

                let header_builder = HeaderBuilder::default()
                    .parent_hash(last_block.header().hash().clone())
                    .number(number)
                    .timestamp(timestamp)
                    .difficulty(difficulty)
                    .cellbase_id(cellbase.hash().clone());

                BlockBuilder::default()
                    .commit_transaction(cellbase)
                    .proposal_transaction(tx.proposal_short_id())
                    .with_header_builder(header_builder)
            };

            {
                chain_controller1
                    .process_block(Arc::new(block.clone()))
                    .expect("process block should be OK");

                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_compact_block(fbb, &block, &HashSet::new());
                fbb.finish(message, None);
                node1.broadcast(RELAY_PROTOCOL_ID, &fbb.finished_data().to_vec());
            }

            // building 2nd compact block with tx and broadcast it
            let last_block = block;

            let block = {
                let number = last_block.header().number() + 1;
                let timestamp = last_block.header().timestamp() + 1;
                let difficulty = shared1.calculate_difficulty(&last_block.header()).unwrap();
                let cellbase = TransactionBuilder::default()
                    .input(CellInput::new_cellbase_input(number))
                    .output(CellOutput::default())
                    .build();

                let header_builder = HeaderBuilder::default()
                    .parent_hash(last_block.header().hash().clone())
                    .number(number)
                    .timestamp(timestamp)
                    .difficulty(difficulty)
                    .cellbase_id(cellbase.hash().clone());

                BlockBuilder::default()
                    .commit_transaction(cellbase)
                    .commit_transaction(tx)
                    .with_header_builder(header_builder)
            };

            {
                chain_controller1
                    .process_block(Arc::new(block.clone()))
                    .expect("process block should be OK");

                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_compact_block(fbb, &block, &HashSet::new());
                fbb.finish(message, None);
                node1.broadcast(RELAY_PROTOCOL_ID, &fbb.finished_data().to_vec());
            }

            node1.start(&signal_tx1, |_| false);
            barrier1.wait();
        })
        .expect("thread spawn");

    let barrier2 = Arc::clone(&barrier);
    let (signal_tx2, signal_rx2) = channel();
    thread::spawn(move || {
        node2.start(&signal_tx2, |data| {
            let msg = get_root::<RelayMessage>(data);
            // terminate thread 2 compact block
            msg.payload_as_compact_block()
                .map(|block| block.header().unwrap().number() == 5)
                .unwrap_or(false)
        });
        barrier2.wait();
    });

    // Wait node2 receive transaction and block from node1
    let _ = signal_rx2.recv();

    // workaround for the delay of notification btween chain and pool
    // find a solution to remove this line after pool refactoring
    thread::sleep(time::Duration::from_secs(2));

    assert_eq!(shared2.chain_state().read().tip_number(), 5);
}

#[test]
fn relay_compact_block_with_missing_indexs() {
    let faketime_file = faketime::millis_tempfile(0).expect("create faketime file");
    faketime::enable(&faketime_file);
    let thread_name = format!("FAKETIME={}", faketime_file.display());

    // Use the same thread name for all child threads, so the time is mocked in all these threads.
    // This is required because the test relies on the determined timestamp. Now all the threads
    // freeze the timestamp at UNIX EPOCH.
    let (mut node1, shared1, chain_controller1) = setup_node(&thread_name, 3);
    let (mut node2, shared2, _chain_controller2) = setup_node(&thread_name, 3);

    node1.connect(&mut node2, RELAY_PROTOCOL_ID);

    let (signal_tx1, _) = channel();
    thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let last_block = shared1
                .block(&shared1.chain_state().read().tip_hash())
                .unwrap();
            let last_cellbase = last_block.commit_transactions().first().unwrap();

            // building 10 txs and broadcast some
            let txs = (0..10u8)
                .map(|i| {
                    TransactionBuilder::default()
                        .input(CellInput::new(
                            OutPoint::new(last_cellbase.hash().clone(), u32::from(i)),
                            create_valid_script(),
                        ))
                        .output(CellOutput::new(50, vec![i], H256::zero(), None))
                        .build()
                })
                .collect::<Vec<_>>();

            [3, 5].iter().for_each(|i| {
                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_transaction(fbb, &txs[*i]);
                fbb.finish(message, None);
                node1.broadcast(RELAY_PROTOCOL_ID, &fbb.finished_data().to_vec());
            });

            // building 1st compact block with tx proposal and broadcast it
            let block = {
                let number = last_block.header().number() + 1;
                let timestamp = last_block.header().timestamp() + 1;
                let difficulty = shared1.calculate_difficulty(&last_block.header()).unwrap();
                let cellbase = TransactionBuilder::default()
                    .input(CellInput::new_cellbase_input(number))
                    .output(CellOutput::default())
                    .build();

                let header_builder = HeaderBuilder::default()
                    .parent_hash(last_block.header().hash().clone())
                    .number(number)
                    .timestamp(timestamp)
                    .difficulty(difficulty)
                    .cellbase_id(cellbase.hash().clone());

                BlockBuilder::default()
                    .commit_transaction(cellbase)
                    .proposal_transactions(txs.iter().map(|tx| tx.proposal_short_id()).collect())
                    .with_header_builder(header_builder)
            };

            {
                chain_controller1
                    .process_block(Arc::new(block.clone()))
                    .expect("process block should be OK");

                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_compact_block(fbb, &block, &HashSet::new());
                fbb.finish(message, None);
                node1.broadcast(RELAY_PROTOCOL_ID, &fbb.finished_data().to_vec());
            }

            // building 2nd compact block with txs and broadcast it
            let last_block = block;

            let block = {
                let number = last_block.header().number() + 1;
                let timestamp = last_block.header().timestamp() + 1;
                let difficulty = shared1.calculate_difficulty(&last_block.header()).unwrap();
                let cellbase = TransactionBuilder::default()
                    .input(CellInput::new_cellbase_input(number))
                    .output(CellOutput::default())
                    .build();

                let header_builder = HeaderBuilder::default()
                    .parent_hash(last_block.header().hash().clone())
                    .number(number)
                    .timestamp(timestamp)
                    .difficulty(difficulty)
                    .cellbase_id(cellbase.hash().clone());

                BlockBuilder::default()
                    .commit_transaction(cellbase)
                    .commit_transactions(txs)
                    .with_header_builder(header_builder)
            };

            {
                chain_controller1
                    .process_block(Arc::new(block.clone()))
                    .expect("process block should be OK");

                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_compact_block(fbb, &block, &HashSet::new());
                fbb.finish(message, None);
                node1.broadcast(RELAY_PROTOCOL_ID, &fbb.finished_data().to_vec());
            }

            node1.start(&signal_tx1, |_| false);
        })
        .expect("thread spawn");

    let (signal_tx2, signal_rx2) = channel();
    thread::spawn(move || {
        node2.start(&signal_tx2, |data| {
            let msg = get_root::<RelayMessage>(data);
            // terminate thread after processing block transactions
            msg.payload_as_block_transactions()
                .map(|_| true)
                .unwrap_or(false)
        });
    });

    // Wait node2 receive transaction and block from node1
    let _ = signal_rx2.recv();

    assert_eq!(shared2.chain_state().read().tip_number(), 5);
}

fn setup_node(
    thread_name: &str,
    height: u64,
) -> (
    TestNode,
    Shared<ChainKVStore<MemoryKeyValueDB>>,
    ChainController,
) {
    let mut block = BlockBuilder::default().with_header_builder(
        HeaderBuilder::default()
            .timestamp(unix_time_as_millis())
            .difficulty(U256::from(1000u64)),
    );
    let consensus = Consensus::default().set_genesis_block(block.clone());

    let shared = SharedBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
        .consensus(consensus)
        .build();
    let (chain_controller, chain_receivers) = ChainController::build();

    let last_tx_updated_at = Arc::new(AtomicUsize::new(0));
    let (tx_pool_controller, tx_pool_receivers) =
        TransactionPoolController::build(Arc::clone(&last_tx_updated_at));

    let (_handle, notify) = NotifyService::default().start(Some(thread_name));

    let tx_pool_service = TransactionPoolService::new(
        PoolConfig::default(),
        shared.clone(),
        notify.clone(),
        last_tx_updated_at,
    );
    let _handle = tx_pool_service.start(Some(thread_name), tx_pool_receivers);

    let chain_service = ChainBuilder::new(shared.clone())
        .notify(notify.clone())
        .build();
    let _handle = chain_service.start(Some(thread_name), chain_receivers);

    for _i in 0..height {
        let number = block.header().number() + 1;
        let timestamp = block.header().timestamp() + 1;
        let difficulty = shared.calculate_difficulty(&block.header()).unwrap();
        let outputs = (0..20)
            .map(|_| CellOutput::new(50, Vec::new(), create_valid_script().type_hash(), None))
            .collect::<Vec<_>>();
        let cellbase = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(number))
            .outputs(outputs)
            .build();

        let header_builder = HeaderBuilder::default()
            .parent_hash(block.header().hash().clone())
            .number(number)
            .timestamp(timestamp)
            .difficulty(difficulty)
            .cellbase_id(cellbase.hash().clone());

        block = BlockBuilder::default()
            .commit_transaction(cellbase)
            .with_header_builder(header_builder);

        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("process block should be OK");
    }

    let relayer = Relayer::new(
        chain_controller.clone(),
        shared.clone(),
        tx_pool_controller,
        Arc::new(Default::default()),
    );

    let mut node = TestNode::default();
    let protocol = Arc::new(relayer) as Arc<_>;
    node.add_protocol(RELAY_PROTOCOL_ID, &protocol, &[TX_PROPOSAL_TOKEN]);
    (node, shared, chain_controller)
}

// This helper is copied from pool test
// TODO should provide some helper or add validation option to pool / chain for testing
fn create_valid_script() -> Script {
    let mut file = File::open(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../nodes_template/spec/cells/always_success"),
    )
    .unwrap();
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).unwrap();

    Script::new(0, Vec::new(), None, Some(buffer), Vec::new())
}
