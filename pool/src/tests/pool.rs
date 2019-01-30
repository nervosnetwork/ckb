use crate::txs_pool::pool::TransactionPoolService;
use crate::txs_pool::trace::{Action, TxTrace};
use crate::txs_pool::types::*;
use ckb_chain::chain::{ChainBuilder, ChainController};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::cell::{CellProvider, CellStatus};
use ckb_core::extras::BlockExt;
use ckb_core::header::HeaderBuilder;
use ckb_core::script::Script;
use ckb_core::transaction::*;
use ckb_core::Cycle;
use ckb_db::memorydb::MemoryKeyValueDB;
use ckb_notify::{ForkBlocks, MsgSwitchFork, NotifyService};
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::{ChainProvider, Shared, SharedBuilder};
use ckb_shared::store::ChainKVStore;
use ckb_shared::store::ChainStore;
use crossbeam_channel::select;
use crossbeam_channel::{self, Receiver};
use faketime::unix_time_as_millis;
use log::error;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::time;
use tempfile::TempPath;

macro_rules! expect_output_parent {
    ($pool:expr, $expected:pat, $( $output:expr ),+ ) => {
        $(
            match $pool
            .cell(&$output) {
                $expected => {},
                x => panic!(
                    "Unexpected result from output search for {:?}, got {:?}",
                    $output,
                    x,
                ),
            };
        )*
    }
}

// Work only when TRANSACTION_PROPAGATION_TIME = 1, TRANSACTION_PROPAGATION_TIMEOUT = 10
#[test]
fn test_proposal_pool() {
    let mut pool = TestPool::<ChainKVStore<MemoryKeyValueDB>>::simple();

    assert_eq!(pool.service.total_size(), 0);

    let block_number = { pool.shared.chain_state().read().tip_number() };

    let mut txs = vec![];

    for _ in 0..200 {
        let tx = test_transaction(
            &[
                OutPoint::new(pool.tx_hash.clone(), 0),
                OutPoint::new(pool.tx_hash.clone(), 1),
            ],
            2,
        );
        pool.tx_hash = tx.transaction.hash().clone();
        txs.push(tx);
    }

    for tx in &txs[1..20] {
        pool.service
            .add_transaction(tx.transaction.clone())
            .unwrap();
    }

    pool.service
        .add_transaction(txs[21].transaction.clone())
        .unwrap();

    assert_eq!(pool.service.pending_size(), 20);

    let mut prop_ids = pool.service.prepare_proposal(22);

    assert_eq!(20, prop_ids.len());

    prop_ids.push(txs[20].transaction.proposal_short_id());
    prop_ids.push(txs[0].transaction.proposal_short_id());

    let header = HeaderBuilder::default().number(block_number + 1).build();
    let block = BlockBuilder::default()
        .header(header)
        .proposal_transactions(prop_ids)
        .build();

    pool.service.reconcile_block(&block);

    assert_eq!(0, pool.service.pool_size());
    assert_eq!(20, pool.service.orphan_size());
    assert_eq!(0, pool.service.proposed_size());

    pool.service
        .add_transaction(txs[0].transaction.clone())
        .unwrap();
    assert_eq!(20, pool.service.pool_size());
    assert_eq!(1, pool.service.orphan_size());

    pool.service
        .propose_transaction(block_number + 1, txs[20].clone());

    assert_eq!(22, pool.service.pool_size());
    assert_eq!(0, pool.service.orphan_size());

    pool.service
        .propose_transaction(block_number + 1, txs[25].clone());

    assert_eq!(1, pool.service.pending_size());
}

#[test]
/// A basic test; add a pair of transactions to the pool.
fn test_add_pool() {
    let mut pool = TestPool::<ChainKVStore<MemoryKeyValueDB>>::simple();
    assert_eq!(pool.service.total_size(), 0);

    let parent_transaction = test_transaction(
        &[
            OutPoint::new(pool.tx_hash.clone(), 5),
            OutPoint::new(pool.tx_hash.clone(), 6),
            OutPoint::new(pool.tx_hash.clone(), 7),
        ],
        2,
    );

    let parent_tx_hash = parent_transaction.transaction.hash().clone();

    // Prepare a second transaction, connected to the first.
    let child_transaction = test_transaction(
        &[
            OutPoint::new(parent_tx_hash.clone(), 0),
            OutPoint::new(parent_tx_hash.clone(), 1),
        ],
        1,
    );

    let child_tx_hash = child_transaction.transaction.hash().clone();

    let result = pool.service.add_to_pool(parent_transaction);
    if result.is_err() {
        panic!("got an error adding parent tx: {:?}", result.err().unwrap());
    }

    let child_result = pool.service.add_to_pool(child_transaction);

    if child_result.is_err() {
        panic!(
            "got an error adding child tx: {:?}",
            child_result.err().unwrap()
        );
    }

    assert_eq!(pool.service.total_size(), 2);
    expect_output_parent!(
        pool.service,
        CellStatus::Live(_),
        OutPoint::new(child_tx_hash.clone(), 0)
    );
    expect_output_parent!(
        pool.service,
        CellStatus::Dead,
        OutPoint::new(parent_tx_hash.clone(), 0),
        OutPoint::new(parent_tx_hash.clone(), 1)
    );
    expect_output_parent!(
        pool.service,
        CellStatus::Live(_),
        OutPoint::new(pool.tx_hash.clone(), 8)
    );
    expect_output_parent!(
        pool.service,
        CellStatus::Dead,
        OutPoint::new(pool.tx_hash.clone(), 200)
    );
}

#[test]
pub fn test_cellbase_spent() {
    let mut pool = TestPool::<ChainKVStore<MemoryKeyValueDB>>::simple();
    let cellbase_tx: Transaction = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(
            pool.shared.chain_state().read().tip_number() + 1,
        ))
        .output(CellOutput::new(
            50000,
            Vec::new(),
            create_valid_script().type_hash(),
            None,
        ))
        .build();

    apply_transactions(vec![cellbase_tx.clone()], vec![], &mut pool);

    let valid_tx = TransactionBuilder::default()
        .input(CellInput::new(
            OutPoint::new(cellbase_tx.hash().clone(), 0),
            create_valid_script(),
        ))
        .output(CellOutput::new(50000, Vec::new(), H256::default(), None))
        .build();

    match pool
        .service
        .add_to_pool(PoolEntry::new(valid_tx, 0, Cycle::default()))
    {
        Ok(_) => {}
        Err(err) => panic!(
            "Unexpected error while adding a valid transaction: {:?}",
            err
        ),
    };
}

#[test]
/// Testing various expected error conditions
pub fn test_add_pool_error() {
    let mut pool = TestPool::<ChainKVStore<MemoryKeyValueDB>>::simple();
    assert_eq!(pool.service.total_size(), 0);

    // To test DoubleSpend and AlreadyInPool conditions, we need to add
    // a valid transaction.
    let valid_transaction = test_transaction(
        &[
            OutPoint::new(pool.tx_hash.clone(), 5),
            OutPoint::new(pool.tx_hash.clone(), 6),
        ],
        2,
    );

    match pool.service.add_to_pool(valid_transaction.clone()) {
        Ok(_) => {}
        Err(_all) => panic!("Unexpected error while adding a valid transaction"),
    };

    let double_spent_transaction = test_transaction(&[OutPoint::new(pool.tx_hash, 6)], 2);

    match pool.service.add_to_pool(double_spent_transaction) {
        Ok(_) => panic!("Expected error when adding DoubleSpent tx, got Ok"),
        Err(x) => {
            match x {
                PoolError::DoubleSpent => {}
                _ => panic!("Unexpected error when adding doubleSpent transaction"),
            };
        }
    };

    match pool.service.add_to_pool(valid_transaction) {
        Ok(_) => panic!("Expected error when adding already_in_pool, got Ok"),
        Err(x) => {
            match x {
                PoolError::AlreadyInPool => {}
                _ => panic!("Unexpected error when adding already_in_pool tx: {:?}", x),
            };
        }
    };

    assert_eq!(pool.service.total_size(), 1);
}

// #[test]
// /// Testing an expected orphan
// fn test_add_orphan() {
//     // TODO we need a test here
// }

#[test]
fn test_get_mineable_transactions() {
    let mut pool = TestPool::<ChainKVStore<MemoryKeyValueDB>>::simple();

    let tx1 = test_transaction_with_capacity(
        &[
            OutPoint::new(pool.tx_hash.clone(), 0),
            OutPoint::new(pool.tx_hash.clone(), 1),
            OutPoint::new(pool.tx_hash.clone(), 2),
            OutPoint::new(pool.tx_hash.clone(), 3),
            OutPoint::new(pool.tx_hash.clone(), 4),
        ],
        5,
        1_000_000,
    );
    let tx1_hash = tx1.transaction.hash().clone();
    let tx2 = test_transaction(
        &[
            OutPoint::new(tx1_hash.clone(), 3),
            OutPoint::new(tx1_hash.clone(), 4),
        ],
        2,
    );
    let tx2_hash = tx2.transaction.hash().clone();
    let tx3 = test_transaction(
        &[
            OutPoint::new(tx1_hash.clone(), 2),
            OutPoint::new(tx2_hash.clone(), 1),
        ],
        2,
    );
    let tx3_hash = tx3.transaction.hash().clone();
    let tx4 = test_transaction(
        &[
            OutPoint::new(tx1_hash.clone(), 1),
            OutPoint::new(tx2_hash.clone(), 0),
            OutPoint::new(tx3_hash.clone(), 1),
        ],
        2,
    );

    pool.service.add_to_pool(tx3.clone()).unwrap();
    assert_eq!(pool.service.orphan_size(), 1);
    pool.service.add_to_pool(tx4.clone()).unwrap();
    assert_eq!(pool.service.orphan_size(), 2);
    pool.service.add_to_pool(tx1.clone()).unwrap();
    assert_eq!(pool.service.orphan_size(), 2);
    pool.service.add_to_pool(tx2.clone()).unwrap();

    assert_eq!(pool.service.pool_size(), 4);

    let txs: Vec<Transaction> = pool
        .service
        .get_mineable_transactions(10)
        .into_iter()
        .map(|x| x.transaction)
        .collect();
    assert_eq!(
        txs,
        vec![
            tx1.transaction,
            tx2.transaction,
            tx3.transaction,
            tx4.transaction
        ]
    )
}

#[test]
/// Testing block reconciliation
fn test_block_reconciliation() {
    let mut pool = TestPool::<ChainKVStore<MemoryKeyValueDB>>::simple();

    let tx0 = test_transaction(&[OutPoint::new(pool.tx_hash.clone(), 0)], 2);
    // tx1 is conflict
    let tx1 = test_transaction_with_capacity(
        &[
            OutPoint::new(pool.tx_hash.clone(), 1),
            OutPoint::new(pool.tx_hash.clone(), 2),
            OutPoint::new(pool.tx_hash.clone(), 3),
            OutPoint::new(pool.tx_hash.clone(), 4),
        ],
        5,
        1_000_000,
    );
    let tx1_hash = tx1.transaction.hash().clone();
    let tx2 = test_transaction(
        &[
            OutPoint::new(tx1_hash.clone(), 3),
            OutPoint::new(tx1_hash.clone(), 4),
        ],
        2,
    );
    let tx2_hash = tx2.transaction.hash().clone();
    let tx3 = test_transaction(
        &[
            OutPoint::new(tx1_hash.clone(), 2),
            OutPoint::new(tx2_hash.clone(), 1),
        ],
        2,
    );
    let tx3_hash = tx3.transaction.hash().clone();
    let tx4 = test_transaction(
        &[
            OutPoint::new(tx1_hash.clone(), 1),
            OutPoint::new(tx2_hash.clone(), 0),
            OutPoint::new(tx3_hash.clone(), 1),
        ],
        2,
    );

    let block_tx0 = tx0.clone();
    let block_tx1 = test_transaction(
        &[
            OutPoint::new(pool.tx_hash.clone(), 1),
            OutPoint::new(pool.tx_hash.clone(), 2),
        ],
        2,
    );
    let block_tx5 = test_transaction(&[OutPoint::new(pool.tx_hash.clone(), 5)], 1);
    let block_tx5_hash = block_tx5.transaction.hash().clone();
    let block_tx6 = test_transaction(
        &[
            OutPoint::new(block_tx5_hash.clone(), 0),
            OutPoint::new(pool.tx_hash.clone(), 6),
        ],
        1,
    );

    //tx5 is conflict, in orphan
    let tx5 = test_transaction(&[OutPoint::new(block_tx5_hash.clone(), 0)], 2);

    //next block: tx6 is conflict, in pool
    let tx6 = test_transaction(&[OutPoint::new(pool.tx_hash.clone(), 6)], 2);

    pool.service.add_to_pool(tx5.clone()).unwrap();
    pool.service.add_to_pool(tx4.clone()).unwrap();
    pool.service.add_to_pool(tx3.clone()).unwrap();
    pool.service.add_to_pool(tx2.clone()).unwrap();
    pool.service.add_to_pool(tx1.clone()).unwrap();
    pool.service.add_to_pool(tx0.clone()).unwrap();

    pool.service
        .add_transaction(tx6.transaction.clone())
        .unwrap();

    assert_eq!(5, pool.service.pool_size());
    assert_eq!(1, pool.service.orphan_size());
    assert_eq!(1, pool.service.pending_size());

    let txs = vec![
        block_tx0.transaction,
        block_tx1.transaction,
        block_tx5.transaction,
        block_tx6.transaction,
    ];
    let prop_ids = vec![tx6.transaction.proposal_short_id()];

    apply_transactions(txs, prop_ids, &mut pool);

    assert_eq!(0, pool.service.pending_size());
    assert_eq!(0, pool.service.proposed_size());
    assert_eq!(0, pool.service.pool_size());
    assert_eq!(0, pool.service.orphan_size());
    // when TRANSACTION_PROPAGATION_TIME = 1
    assert_eq!(1, pool.service.cache_size());
}

// Work only when TRANSACTION_PROPAGATION_TIME = 1, TRANSACTION_PROPAGATION_TIMEOUT = 10
#[test]
fn test_switch_fork() {
    let mut pool = TestPool::<ChainKVStore<MemoryKeyValueDB>>::simple();

    assert_eq!(pool.service.total_size(), 0);

    let block_number = { pool.shared.chain_state().read().tip_number() };

    let mut txs = vec![];

    for i in 0..20 {
        let tx = test_transaction(
            &[
                OutPoint::new(pool.tx_hash.clone(), i),
                OutPoint::new(pool.tx_hash.clone(), i + 20),
            ],
            2,
        );

        txs.push(tx);
    }

    for tx in &txs[0..20] {
        pool.service
            .add_transaction(tx.transaction.clone())
            .unwrap();
    }

    assert_eq!(pool.service.pending_size(), 20);

    let prop_ids: Vec<ProposalShortId> = txs
        .iter()
        .map(|x| x.transaction.proposal_short_id())
        .collect();

    let block01 = BlockBuilder::default()
        .proposal_transactions(vec![prop_ids[0], prop_ids[1]])
        .with_header_builder(HeaderBuilder::default().number(block_number + 1));

    let block02 = BlockBuilder::default()
        .proposal_transactions(vec![prop_ids[2], prop_ids[3]])
        .commit_transaction(txs[0].transaction.clone())
        .with_header_builder(HeaderBuilder::default().number(block_number + 2));

    let block11 = BlockBuilder::default()
        .proposal_transactions(vec![prop_ids[3], prop_ids[4]])
        .with_header_builder(HeaderBuilder::default().number(block_number + 1));

    let block12 = BlockBuilder::default()
        .proposal_transactions(vec![prop_ids[5], prop_ids[6]])
        .commit_transaction(txs[4].transaction.clone())
        .with_header_builder(HeaderBuilder::default().number(block_number + 2));

    pool.service.reconcile_block(&block01);
    pool.service.reconcile_block(&block02);

    pool.shared
        .store()
        .save_with_batch(|batch| {
            let hash1 = block01.header().hash();
            let hash2 = block02.header().hash();
            let txs_len2 = block02.commit_transactions().len();
            let txs_len1 = block01.commit_transactions().len();
            let cycles_set2 = vec![Cycle::default(); txs_len2];
            let cycles_set1 = vec![Cycle::default(); txs_len1];

            let mut ext = BlockExt {
                received_at: 0,
                total_difficulty: U256::default(),
                total_uncles_count: 0,
                valid: None,
                cycles_set: cycles_set1,
            };

            pool.shared.store().insert_block_ext(batch, &hash1, &ext);
            ext.cycles_set = cycles_set2;
            pool.shared.store().insert_block_ext(batch, &hash2, &ext);
            Ok(())
        })
        .unwrap();

    let olds = vec![block02, block01];
    let news = vec![block12, block11];

    let fb = ForkBlocks::new(olds, news);

    pool.service.switch_fork(&fb);

    let mtxs: Vec<Transaction> = pool
        .service
        .get_mineable_transactions(10)
        .into_iter()
        .map(|x| x.transaction)
        .collect();

    assert_eq!(
        mtxs,
        vec![
            txs[3].transaction.clone(),
            txs[6].transaction.clone(),
            txs[5].transaction.clone()
        ]
    );
}

fn prepare_trace(
    pool: &mut TestPool<ChainKVStore<MemoryKeyValueDB>>,
    faketime_file: &TempPath,
) -> PoolEntry {
    let tx = test_transaction(&[OutPoint::new(pool.tx_hash.clone(), 0)], 2);

    let block_number = { pool.shared.chain_state().read().tip_number() };

    pool.service
        .trace_transaction(tx.transaction.clone())
        .unwrap();
    let prop_ids = pool.service.prepare_proposal(10);

    assert_eq!(1, prop_ids.len());
    assert_eq!(prop_ids[0], tx.transaction.proposal_short_id());

    let header = HeaderBuilder::default().number(block_number + 1).build();
    let block = BlockBuilder::default()
        .header(header)
        .proposal_transactions(vec![tx.transaction.proposal_short_id()])
        .build();

    faketime::write_millis(faketime_file, 9102).expect("write millis");

    pool.service.reconcile_block(&block);
    tx
}

#[cfg(not(disable_faketime))]
#[test]
fn test_get_transaction_traces() {
    let mut pool = TestPool::<ChainKVStore<MemoryKeyValueDB>>::simple();
    let faketime_file = faketime::millis_tempfile(8102).expect("create faketime file");
    faketime::enable(&faketime_file);

    let tx = prepare_trace(&mut pool, &faketime_file);
    let tx_hash = tx.transaction.hash();

    let trace = pool.service.get_transaction_traces(&tx_hash);
    match trace.map(|t| t.as_slice()) {
        Some(
            [TxTrace {
                action: Action::AddPending,
                time: 8102,
                ..
            }, TxTrace {
                action: Action::Proposed,
                info: proposal_info,
                time: 9102,
            }, TxTrace {
                action: Action::AddCommit,
                time: 9102,
                ..
            }],
        ) => assert_eq!(
            proposal_info,
            concat!("ProposalShortId(0xda495f694cac79513d00) proposed ",
            "in block number(2)-hash(0xb42c5305777987f80112e862a3e722c1d0e68c671f1d8920d16ebfc6783a6467)")
        ),
        _ => assert!(false),
    }

    faketime::write_millis(&faketime_file, 9103).expect("write millis");
    let block = apply_transactions(vec![tx.transaction.clone()], vec![], &mut pool);
    let trace = pool.service.get_transaction_traces(&tx_hash);
    match trace.map(|t| t.as_slice()) {
        Some(
            [TxTrace {
                action: Action::AddPending,
                time: 8102,
                ..
            }, TxTrace {
                action: Action::Proposed,
                time: 9102,
                ..
            }, TxTrace {
                action: Action::AddCommit,
                time: 9102,
                ..
            }, TxTrace {
                action: Action::Committed,
                info: committed_info,
                time: 9103,
            }],
        ) => assert_eq!(
            committed_info,
            &format!(
                "committed in block number({:?})-hash({:#x})",
                block.header().number(),
                block.header().hash()
            )
        ),
        _ => assert!(false),
    }
}

struct TestPool<CI> {
    service: TransactionPoolService<CI>,
    chain: ChainController,
    shared: Shared<CI>,
    tx_hash: H256,
    switch_fork_receiver: Receiver<MsgSwitchFork>,
}

impl<CI: ChainIndex + 'static> TestPool<CI> {
    fn simple() -> TestPool<ChainKVStore<MemoryKeyValueDB>> {
        let notify = NotifyService::default().start::<&str>(None);
        let switch_fork_receiver = notify.subscribe_switch_fork("txs_pool");
        let shared = SharedBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .consensus(Consensus::default())
            .build();
        let chain_service = ChainBuilder::new(shared.clone(), notify.clone())
            .verification(false)
            .build();
        let chain_controller = chain_service.start::<&str>(None);

        let tx_pool_service = TransactionPoolService::new(
            PoolConfig {
                max_pool_size: 1000,
                max_orphan_size: 1000,
                max_proposal_size: 1000,
                max_cache_size: 1000,
                max_pending_size: 1000,
                trace: Some(100),
            },
            shared.clone(),
            notify.clone(),
        );

        let default_script_hash = create_valid_script().type_hash();
        let tx = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::null(), Default::default()))
            .outputs(vec![
                CellOutput::new(
                    100_000_000,
                    Vec::new(),
                    default_script_hash.clone(),
                    None,
                );
                100
            ])
            .build();

        let transactions = vec![tx.clone()];
        let mut pool = TestPool {
            service: tx_pool_service,
            chain: chain_controller,
            shared,
            tx_hash: tx.hash().clone(),
            switch_fork_receiver,
        };
        apply_transactions(transactions, vec![], &mut pool);
        pool
    }

    fn handle_notify_messages(&mut self) {
        loop {
            select! {
                recv(self.switch_fork_receiver) -> msg => match msg {
                    Ok(blocks) => self.service.switch_fork(&blocks),
                    _ => {
                        error!(target: "txs_pool", "channel switch_fork_receiver closed");
                        break;
                    }
                },
                recv(crossbeam_channel::after(time::Duration::from_millis(100))) -> _ => {
                    break;
                }
            }
        }
    }
}

fn apply_transactions<CI: ChainIndex + 'static>(
    transactions: Vec<Transaction>,
    prop_ids: Vec<ProposalShortId>,
    pool: &mut TestPool<CI>,
) -> Block {
    let cellbase_id = if let Some(cellbase) = transactions.first() {
        cellbase.hash().clone()
    } else {
        H256::zero()
    };

    let parent = pool.shared.chain_state().read().tip_header().clone();

    let header_builder = HeaderBuilder::default()
        .parent_hash(parent.hash().clone())
        .number(parent.number() + 1)
        .timestamp(unix_time_as_millis())
        .cellbase_id(cellbase_id)
        .difficulty(pool.shared.calculate_difficulty(&parent).unwrap());

    let block = BlockBuilder::default()
        .commit_transactions(transactions)
        .proposal_transactions(prop_ids)
        .with_header_builder(header_builder);

    let txs_len = block.commit_transactions().len();
    pool.chain
        .process_block(Arc::new(block.clone()), vec![Cycle::default(); txs_len])
        .unwrap();
    pool.handle_notify_messages();
    block
}

fn test_transaction(input_values: &[OutPoint], output_num: usize) -> PoolEntry {
    test_transaction_with_capacity(input_values, output_num, 100_000)
}

fn test_transaction_with_capacity(
    input_values: &[OutPoint],
    output_num: usize,
    capacity: u64,
) -> PoolEntry {
    let inputs: Vec<CellInput> = input_values
        .iter()
        .map(|x| CellInput::new(x.clone(), create_valid_script()))
        .collect();

    let mut output = CellOutput::default();
    output.capacity = capacity / output_num as u64;
    output.lock = create_valid_script().type_hash();
    let outputs: Vec<CellOutput> = vec![output.clone(); output_num];

    let tx = TransactionBuilder::default()
        .inputs(inputs)
        .outputs(outputs)
        .build();

    PoolEntry::new(tx, 0, Cycle::default())
}

// Since the main point here is to test pool functionality, not scripting
// behavior, we use a dummy script here that always passes in testing
fn create_valid_script() -> Script {
    let mut file = File::open(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../nodes_template/spec/cells/always_success"),
    )
    .unwrap();
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).unwrap();

    Script::new(0, Vec::new(), None, Some(buffer), Vec::new())
}
