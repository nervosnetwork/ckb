use crate::relayer::TX_PROPOSAL_TOKEN;
use crate::tests::TestNode;
use crate::{NetworkProtocol, Relayer, SyncSharedState};
use ckb_chain::chain::{ChainBuilder, ChainController};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::BlockBuilder;
use ckb_core::header::HeaderBuilder;
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, Transaction, TransactionBuilder};
use ckb_core::{capacity_bytes, Bytes, Capacity};
use ckb_db::memorydb::MemoryKeyValueDB;
use ckb_notify::NotifyService;
use ckb_protocol::RelayMessage;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_store::ChainKVStore;
use ckb_traits::ChainProvider;
use ckb_util::RwLock;
use faketime::{self, unix_time_as_millis};
use flatbuffers::get_root;
use flatbuffers::FlatBufferBuilder;
use numext_fixed_uint::U256;
use std::collections::HashSet;
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

    node1.connect(&mut node2, NetworkProtocol::RELAY.into());

    let (signal_tx1, _) = channel();
    let barrier1 = Arc::clone(&barrier);
    thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let last_block = shared1
                .block(&shared1.chain_state().lock().tip_hash())
                .unwrap();
            let last_cellbase = last_block.transactions().first().unwrap();

            // building tx and broadcast it
            let tx = TransactionBuilder::default()
                .input(CellInput::new(
                    OutPoint::new_cell(last_cellbase.hash().to_owned(), 0),
                    0,
                    vec![],
                ))
                .output(CellOutput::new(
                    capacity_bytes!(50),
                    Bytes::default(),
                    Script::default(),
                    None,
                ))
                .build();

            {
                let chain_state = shared1.chain_state().lock();
                let _cycles = chain_state
                    .add_tx_to_pool(tx.clone(), None)
                    .expect("verify relay tx");
                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_transaction_hash(fbb, &tx.hash());
                fbb.finish(message, None);
                node1.broadcast(NetworkProtocol::RELAY.into(), fbb.finished_data());
            }

            // building 1st compact block with tx proposal and broadcast it
            let block = {
                let number = last_block.header().number() + 1;
                let timestamp = last_block.header().timestamp() + 1;

                let last_epoch = shared1.get_epoch_ext(&last_block.header().hash()).unwrap();
                let epoch = shared1
                    .next_epoch_ext(&last_epoch, last_block.header())
                    .unwrap_or(last_epoch);

                let cellbase = TransactionBuilder::default()
                    .input(CellInput::new_cellbase_input(number))
                    .output(CellOutput::default())
                    .build();

                let header_builder = HeaderBuilder::default()
                    .parent_hash(last_block.header().hash().to_owned())
                    .number(number)
                    .epoch(epoch.number())
                    .timestamp(timestamp)
                    .difficulty(epoch.difficulty().clone());

                BlockBuilder::default()
                    .transaction(cellbase)
                    .proposal(tx.proposal_short_id())
                    .header_builder(header_builder)
                    .build()
            };

            {
                chain_controller1
                    .process_block(Arc::new(block.clone()))
                    .expect("process block should be OK");

                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_compact_block(fbb, &block, &HashSet::new());
                fbb.finish(message, None);
                node1.broadcast(NetworkProtocol::RELAY.into(), fbb.finished_data());
            }

            // building 2nd compact block with tx and broadcast it
            let last_block = block;

            let block = {
                let number = last_block.header().number() + 1;
                let timestamp = last_block.header().timestamp() + 1;

                let last_epoch = shared1.get_epoch_ext(&last_block.header().hash()).unwrap();
                let epoch = shared1
                    .next_epoch_ext(&last_epoch, last_block.header())
                    .unwrap_or(last_epoch);

                let cellbase = TransactionBuilder::default()
                    .input(CellInput::new_cellbase_input(number))
                    .output(CellOutput::default())
                    .build();

                let header_builder = HeaderBuilder::default()
                    .parent_hash(last_block.header().hash().to_owned())
                    .number(number)
                    .epoch(epoch.number())
                    .timestamp(timestamp)
                    .difficulty(epoch.difficulty().clone());

                BlockBuilder::default()
                    .transaction(cellbase)
                    .transaction(tx)
                    .header_builder(header_builder)
                    .build()
            };

            {
                chain_controller1
                    .process_block(Arc::new(block.clone()))
                    .expect("process block should be OK");

                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_compact_block(fbb, &block, &HashSet::new());
                fbb.finish(message, None);
                node1.broadcast(NetworkProtocol::RELAY.into(), &fbb.finished_data().to_vec());
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

    assert_eq!(shared2.chain_state().lock().tip_number(), 5);
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

    node1.connect(&mut node2, NetworkProtocol::RELAY.into());

    let (signal_tx1, _) = channel();
    thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let last_block = shared1
                .block(&shared1.chain_state().lock().tip_hash())
                .unwrap();
            let last_cellbase = last_block.transactions().first().unwrap();

            // building 10 txs and broadcast some
            let txs = (0..10u8)
                .map(|i| {
                    TransactionBuilder::default()
                        .input(CellInput::new(
                            OutPoint::new_cell(last_cellbase.hash().to_owned(), u32::from(i)),
                            0,
                            vec![],
                        ))
                        .output(CellOutput::new(
                            capacity_bytes!(50),
                            Bytes::from(vec![i]),
                            Script::default(),
                            None,
                        ))
                        .build()
                })
                .collect::<Vec<_>>();

            [3, 5].iter().for_each(|i| {
                let tx = &txs[*i];
                let _cycles = {
                    let chain_state = shared1.chain_state().lock();
                    chain_state
                        .add_tx_to_pool(tx.clone(), None)
                        .expect("verify relay tx")
                };
                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_transaction_hash(fbb, &tx.hash());
                fbb.finish(message, None);
                node1.broadcast(NetworkProtocol::RELAY.into(), fbb.finished_data());
            });

            // building 1st compact block with tx proposal and broadcast it
            let block = {
                let number = last_block.header().number() + 1;
                let timestamp = last_block.header().timestamp() + 1;

                let last_epoch = shared1.get_epoch_ext(&last_block.header().hash()).unwrap();
                let epoch = shared1
                    .next_epoch_ext(&last_epoch, last_block.header())
                    .unwrap_or(last_epoch);

                let cellbase = TransactionBuilder::default()
                    .input(CellInput::new_cellbase_input(number))
                    .output(CellOutput::default())
                    .build();

                let header_builder = HeaderBuilder::default()
                    .parent_hash(last_block.header().hash().to_owned())
                    .epoch(epoch.number())
                    .number(number)
                    .timestamp(timestamp)
                    .difficulty(epoch.difficulty().clone());

                BlockBuilder::default()
                    .transaction(cellbase)
                    .proposals(txs.iter().map(Transaction::proposal_short_id).collect())
                    .header_builder(header_builder)
                    .build()
            };

            {
                chain_controller1
                    .process_block(Arc::new(block.clone()))
                    .expect("process block should be OK");

                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_compact_block(fbb, &block, &HashSet::new());
                fbb.finish(message, None);
                node1.broadcast(NetworkProtocol::RELAY.into(), &fbb.finished_data().to_vec());
            }

            // building 2nd compact block with txs and broadcast it
            let last_block = block;

            let block = {
                let number = last_block.header().number() + 1;
                let timestamp = last_block.header().timestamp() + 1;

                let last_epoch = shared1.get_epoch_ext(&last_block.header().hash()).unwrap();
                let epoch = shared1
                    .next_epoch_ext(&last_epoch, last_block.header())
                    .unwrap_or(last_epoch);

                let cellbase = TransactionBuilder::default()
                    .input(CellInput::new_cellbase_input(number))
                    .output(CellOutput::default())
                    .build();

                let header_builder = HeaderBuilder::default()
                    .parent_hash(last_block.header().hash().to_owned())
                    .number(number)
                    .epoch(epoch.number())
                    .timestamp(timestamp)
                    .difficulty(epoch.difficulty().clone());

                BlockBuilder::default()
                    .transaction(cellbase)
                    .transactions(txs)
                    .header_builder(header_builder)
                    .build()
            };

            {
                chain_controller1
                    .process_block(Arc::new(block.clone()))
                    .expect("process block should be OK");

                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_compact_block(fbb, &block, &HashSet::new());
                fbb.finish(message, None);
                node1.broadcast(NetworkProtocol::RELAY.into(), &fbb.finished_data().to_vec());
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

    assert_eq!(shared2.chain_state().lock().tip_number(), 5);
}

fn setup_node(
    thread_name: &str,
    height: u64,
) -> (
    TestNode,
    Shared<ChainKVStore<MemoryKeyValueDB>>,
    ChainController,
) {
    let mut block = BlockBuilder::default()
        .header_builder(
            HeaderBuilder::default()
                .timestamp(unix_time_as_millis())
                .difficulty(U256::from(1000u64)),
        )
        .build();
    let consensus = Consensus::default()
        .set_genesis_block(block.clone())
        .set_cellbase_maturity(0);

    let shared = SharedBuilder::<MemoryKeyValueDB>::new()
        .consensus(consensus)
        .build()
        .unwrap();

    let notify = NotifyService::default().start(Some(thread_name));

    let chain_service = ChainBuilder::new(shared.clone(), notify)
        .verification(false)
        .build();
    let chain_controller = chain_service.start::<&str>(None);

    for _i in 0..height {
        let number = block.header().number() + 1;
        let timestamp = block.header().timestamp() + 1;

        let last_epoch = shared.get_epoch_ext(&block.header().hash()).unwrap();
        let epoch = shared
            .next_epoch_ext(&last_epoch, block.header())
            .unwrap_or(last_epoch);

        let outputs = (0..20)
            .map(|_| {
                CellOutput::new(
                    capacity_bytes!(50),
                    Bytes::default(),
                    Script::always_success(),
                    None,
                )
            })
            .collect::<Vec<_>>();
        let cellbase = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(number))
            .outputs(outputs)
            .build();

        let header_builder = HeaderBuilder::default()
            .parent_hash(block.header().hash().to_owned())
            .number(number)
            .epoch(epoch.number())
            .timestamp(timestamp)
            .difficulty(epoch.difficulty().clone());

        block = BlockBuilder::default()
            .transaction(cellbase)
            .header_builder(header_builder)
            .build();

        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("process block should be OK");
    }

    let sync_shared_state = Arc::new(SyncSharedState::new(shared.clone()));
    let relayer = Relayer::new(
        chain_controller.clone(),
        sync_shared_state,
        Arc::new(Default::default()),
    );

    let mut node = TestNode::default();
    let protocol = Arc::new(RwLock::new(relayer)) as Arc<_>;
    node.add_protocol(
        NetworkProtocol::RELAY.into(),
        &protocol,
        &[TX_PROPOSAL_TOKEN],
    );
    (node, shared, chain_controller)
}
