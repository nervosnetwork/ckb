use bigint::H256;
use ckb_chain::chain::{ChainBuilder, ChainProvider};
use ckb_chain::store::ChainKVStore;
use ckb_db::memorydb::MemoryKeyValueDB;
use ckb_notify::{ForkBlocks, Notify};
use core::block::{Block, IndexedBlock};
use core::cell::{CellProvider, CellState};
use core::header::{Header, RawHeader};
use core::script::Script;
use core::transaction::*;
use std::fs::File;
use std::io::Read;
use std::sync::Arc;
use std::{thread, time};
use time::now_ms;
use txs_pool::pool::*;
use txs_pool::types::*;

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
    let (chain, pool, mut tx_hash) = test_setup();

    assert_eq!(pool.total_size(), 0);

    let block_number = { chain.tip_header().read().number() };

    let mut txs = vec![];

    for _ in 0..200 {
        let tx = test_transaction(
            vec![OutPoint::new(tx_hash, 0), OutPoint::new(tx_hash, 1)],
            2,
        );
        tx_hash = tx.hash();
        txs.push(tx);
    }

    for tx in &txs[1..20] {
        pool.add_transaction(tx.clone()).unwrap();
    }

    pool.add_transaction(txs[21].clone()).unwrap();

    assert_eq!(pool.pending_size(), 20);

    let mut prop_ids = pool.prepare_proposal(22);

    assert_eq!(20, prop_ids.len());

    prop_ids.push(txs[20].proposal_short_id());
    prop_ids.push(txs[0].proposal_short_id());

    let mut block: IndexedBlock = Block::default().into();
    block.header.number = block_number + 1;
    block.proposal_transactions = prop_ids.clone();

    pool.reconcile_block(&block);

    assert_eq!(0, pool.pool_size());
    assert_eq!(20, pool.orphan_size());
    assert_eq!(0, pool.proposed_size());

    pool.add_transaction(txs[0].clone()).unwrap();
    assert_eq!(20, pool.pool_size());
    assert_eq!(1, pool.orphan_size());

    pool.propose_transaction(block_number + 1, txs[20].clone());

    assert_eq!(22, pool.pool_size());
    assert_eq!(0, pool.orphan_size());

    pool.propose_transaction(block_number + 1, txs[25].clone());

    assert_eq!(1, pool.pending_size());
}

#[test]
/// A basic test; add a pair of transactions to the pool.
fn test_add_pool() {
    let (_chain, pool, tx_hash) = test_setup();
    assert_eq!(pool.total_size(), 0);

    let parent_transaction = test_transaction(
        vec![
            OutPoint::new(tx_hash, 5),
            OutPoint::new(tx_hash, 6),
            OutPoint::new(tx_hash, 7),
        ],
        2,
    );

    let parent_tx_hash = parent_transaction.hash();

    // Prepare a second transaction, connected to the first.
    let child_transaction = test_transaction(
        vec![
            OutPoint::new(parent_tx_hash, 0),
            OutPoint::new(parent_tx_hash, 1),
        ],
        1,
    );

    let child_tx_hash = child_transaction.hash();

    let result = pool.add_to_pool(parent_transaction);
    if result.is_err() {
        panic!("got an error adding parent tx: {:?}", result.err().unwrap());
    }

    let child_result = pool.add_to_pool(child_transaction);

    if child_result.is_err() {
        panic!(
            "got an error adding child tx: {:?}",
            child_result.err().unwrap()
        );
    }

    assert_eq!(pool.total_size(), 2);
    expect_output_parent!(pool, CellState::Head(_), OutPoint::new(child_tx_hash, 0));
    expect_output_parent!(
        pool,
        CellState::Tail,
        OutPoint::new(parent_tx_hash, 0),
        OutPoint::new(parent_tx_hash, 1)
    );
    expect_output_parent!(pool, CellState::Head(_), OutPoint::new(tx_hash, 8));
    expect_output_parent!(pool, CellState::Unknown, OutPoint::new(tx_hash, 200));
}

#[test]
pub fn test_cellbase_spent() {
    let (chain, pool, _tx_hash) = test_setup();
    let cellbase_tx: IndexedTransaction = Transaction::new(
        0,
        Vec::new(),
        vec![CellInput::new_cellbase_input(
            chain.tip_header().read().header.raw.number + 1,
        )],
        vec![CellOutput::new(
            50000,
            Vec::new(),
            create_valid_script().redeem_script_hash(),
        )],
    ).into();
    apply_transactions(vec![cellbase_tx.clone()], vec![], &chain);

    let valid_tx = Transaction::new(
        0,
        Vec::new(),
        vec![CellInput::new(
            OutPoint::new(cellbase_tx.hash(), 0),
            create_valid_script(),
        )],
        vec![CellOutput::new(50000, Vec::new(), H256::default())],
    );

    match pool.add_to_pool(valid_tx.into()) {
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
    let (_chain, pool, tx_hash) = test_setup();
    assert_eq!(pool.total_size(), 0);

    // To test DoubleSpend and AlreadyInPool conditions, we need to add
    // a valid transaction.
    let valid_transaction = test_transaction(
        vec![OutPoint::new(tx_hash, 5), OutPoint::new(tx_hash, 6)],
        2,
    );

    match pool.add_to_pool(valid_transaction.clone()) {
        Ok(_) => {}
        Err(_) => panic!("Unexpected error while adding a valid transaction"),
    };

    let double_spent_transaction = test_transaction(vec![OutPoint::new(tx_hash, 6)], 2);

    match pool.add_to_pool(double_spent_transaction) {
        Ok(_) => panic!("Expected error when adding DoubleSpent tx, got Ok"),
        Err(x) => {
            match x {
                PoolError::DoubleSpent => {}
                _ => panic!("Unexpected error when adding doubleSpent transaction"),
            };
        }
    };

    match pool.add_to_pool(valid_transaction) {
        Ok(_) => panic!("Expected error when adding already_in_pool, got Ok"),
        Err(x) => {
            match x {
                PoolError::AlreadyInPool => {}
                _ => panic!("Unexpected error when adding already_in_pool tx: {:?}", x),
            };
        }
    };

    assert_eq!(pool.total_size(), 1);
}

// #[test]
// /// Testing an expected orphan
// fn test_add_orphan() {
//     // TODO we need a test here
// }

#[test]
fn test_get_mineable_transactions() {
    let (_chain, pool, tx_hash) = test_setup();

    let tx1 = test_transaction_with_capacity(
        vec![
            OutPoint::new(tx_hash, 0),
            OutPoint::new(tx_hash, 1),
            OutPoint::new(tx_hash, 2),
            OutPoint::new(tx_hash, 3),
            OutPoint::new(tx_hash, 4),
        ],
        5,
        1000_000,
    );
    let tx1_hash = tx1.hash();
    let tx2 = test_transaction(
        vec![OutPoint::new(tx1_hash, 3), OutPoint::new(tx1_hash, 4)],
        2,
    );
    let tx2_hash = tx2.hash();
    let tx3 = test_transaction(
        vec![OutPoint::new(tx1_hash, 2), OutPoint::new(tx2_hash, 1)],
        2,
    );
    let tx3_hash = tx3.hash();
    let tx4 = test_transaction(
        vec![
            OutPoint::new(tx1_hash, 1),
            OutPoint::new(tx2_hash, 0),
            OutPoint::new(tx3_hash, 1),
        ],
        2,
    );

    pool.add_to_pool(tx3.clone()).unwrap();
    assert_eq!(pool.orphan_size(), 1);
    pool.add_to_pool(tx4.clone()).unwrap();
    assert_eq!(pool.orphan_size(), 2);
    pool.add_to_pool(tx1.clone()).unwrap();
    assert_eq!(pool.orphan_size(), 2);
    pool.add_to_pool(tx2.clone()).unwrap();

    assert_eq!(pool.pool_size(), 4);

    let txs = pool.get_mineable_transactions(10);
    assert_eq!(txs, vec![tx1, tx2, tx3, tx4])
}

#[test]
/// Testing block reconciliation
fn test_block_reconciliation() {
    let (chain, pool, tx_hash) = test_setup();

    let tx0 = test_transaction(vec![OutPoint::new(tx_hash, 0)], 2);
    // tx1 is conflict
    let tx1 = test_transaction_with_capacity(
        vec![
            OutPoint::new(tx_hash, 1),
            OutPoint::new(tx_hash, 2),
            OutPoint::new(tx_hash, 3),
            OutPoint::new(tx_hash, 4),
        ],
        5,
        1000_000,
    );
    let tx1_hash = tx1.hash();
    let tx2 = test_transaction(
        vec![OutPoint::new(tx1_hash, 3), OutPoint::new(tx1_hash, 4)],
        2,
    );
    let tx2_hash = tx2.hash();
    let tx3 = test_transaction(
        vec![OutPoint::new(tx1_hash, 2), OutPoint::new(tx2_hash, 1)],
        2,
    );
    let tx3_hash = tx3.hash();
    let tx4 = test_transaction(
        vec![
            OutPoint::new(tx1_hash, 1),
            OutPoint::new(tx2_hash, 0),
            OutPoint::new(tx3_hash, 1),
        ],
        2,
    );

    let block_tx0 = tx0.clone();
    let block_tx1 = test_transaction(
        vec![OutPoint::new(tx_hash, 1), OutPoint::new(tx_hash, 2)],
        2,
    );
    let block_tx5 = test_transaction(vec![OutPoint::new(tx_hash, 5)], 1);
    let block_tx5_hash = block_tx5.hash();
    let block_tx6 = test_transaction(
        vec![OutPoint::new(block_tx5_hash, 0), OutPoint::new(tx_hash, 6)],
        1,
    );

    //tx5 is conflict, in orphan
    let tx5 = test_transaction(vec![OutPoint::new(block_tx5_hash, 0)], 2);

    //next block: tx6 is conflict, in pool
    let tx6 = test_transaction(vec![OutPoint::new(tx_hash, 6)], 2);

    pool.add_to_pool(tx5.clone()).unwrap();
    pool.add_to_pool(tx4.clone()).unwrap();
    pool.add_to_pool(tx3.clone()).unwrap();
    pool.add_to_pool(tx2.clone()).unwrap();
    pool.add_to_pool(tx1.clone()).unwrap();
    pool.add_to_pool(tx0.clone()).unwrap();

    pool.add_transaction(tx6.clone()).unwrap();

    assert_eq!(5, pool.pool_size());
    assert_eq!(1, pool.orphan_size());
    assert_eq!(1, pool.pending_size());

    let txs = vec![block_tx0, block_tx1, block_tx5, block_tx6];
    let prop_ids = vec![tx6.proposal_short_id()];

    apply_transactions(txs, prop_ids, &chain);

    let t = time::Duration::from_millis(1000);
    thread::sleep(t);

    assert_eq!(0, pool.pending_size());
    assert_eq!(0, pool.proposed_size());
    assert_eq!(0, pool.pool_size());
    assert_eq!(0, pool.orphan_size());
    // when TRANSACTION_PROPAGATION_TIME = 1
    assert_eq!(1, pool.cache_size());
}

// Work only when TRANSACTION_PROPAGATION_TIME = 1, TRANSACTION_PROPAGATION_TIMEOUT = 10
#[test]
fn test_switch_fork() {
    let (chain, pool, tx_hash) = test_setup();

    assert_eq!(pool.total_size(), 0);

    let block_number = { chain.tip_header().read().number() };

    let mut txs = vec![];

    for i in 0..20 {
        let tx = test_transaction(
            vec![OutPoint::new(tx_hash, i), OutPoint::new(tx_hash, i + 20)],
            2,
        );

        txs.push(tx);
    }

    for tx in &txs[0..20] {
        pool.add_transaction(tx.clone()).unwrap();
    }

    assert_eq!(pool.pending_size(), 20);

    let prop_ids: Vec<ProposalShortId> = txs.iter().map(|x| x.proposal_short_id()).collect();

    let mut block01: IndexedBlock = Block::default().into();
    block01.header.number = block_number + 1;
    block01.proposal_transactions = vec![prop_ids[0], prop_ids[1]];
    block01.commit_transactions = vec![];

    let mut block02: IndexedBlock = Block::default().into();
    block02.header.number = block_number + 2;
    block02.proposal_transactions = vec![prop_ids[2], prop_ids[3]];
    block02.commit_transactions = vec![txs[0].clone()];

    let mut block11: IndexedBlock = Block::default().into();
    block11.header.number = block_number + 1;
    block11.proposal_transactions = vec![prop_ids[3], prop_ids[4]];
    block11.commit_transactions = vec![];

    let mut block12: IndexedBlock = Block::default().into();
    block12.header.number = block_number + 2;
    block12.proposal_transactions = vec![prop_ids[5], prop_ids[6]];
    block12.commit_transactions = vec![txs[4].clone()];

    pool.reconcile_block(&block01);
    pool.reconcile_block(&block02);

    let olds = vec![block02, block01];
    let news = vec![block11, block12];

    let fb = ForkBlocks::new(olds, news);

    pool.switch_fork(&fb);

    let mtxs = pool.get_mineable_transactions(10);

    assert_eq!(mtxs, vec![txs[3].clone(), txs[5].clone(), txs[6].clone()]);
}

fn test_setup() -> (
    Arc<impl ChainProvider>,
    Arc<TransactionPool<impl ChainProvider>>,
    H256,
) {
    let notify = Notify::new();
    let builder = ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
        // .verification_level("NoVerification")
        .notify(notify.clone());
    let chain = Arc::new(builder.build().unwrap());
    let pool = TransactionPool::new(
        PoolConfig {
            max_pool_size: 1000,
            max_orphan_size: 1000,
            max_proposal_size: 1000,
            max_cache_size: 1000,
            max_pending_size: 1000,
        },
        Arc::clone(&chain),
        notify,
    );

    let default_script_hash = create_valid_script().redeem_script_hash();
    let tx: IndexedTransaction = Transaction::new(
        0,
        Vec::new(),
        vec![CellInput::new(OutPoint::null(), Default::default())],
        vec![CellOutput::new(100_000_000, Vec::new(), default_script_hash.clone()); 100],
    ).into();
    let transactions = vec![tx.clone()];
    apply_transactions(transactions, vec![], &chain);
    (chain, pool, tx.hash())
}

fn apply_transactions(
    transactions: Vec<IndexedTransaction>,
    prop_ids: Vec<ProposalShortId>,
    chain: &Arc<impl ChainProvider>,
) -> IndexedBlock {
    let time = now_ms();

    let cellbase_id = if let Some(cellbase) = transactions.first() {
        cellbase.hash()
    } else {
        H256::zero()
    };

    let parent = { chain.tip_header().read().header.clone() };

    let header = Header {
        raw: RawHeader::new(
            &parent,
            transactions.iter(),
            vec![].iter(),
            time,
            chain.calculate_difficulty(&parent).unwrap(),
            cellbase_id,
            H256::zero(),
        ),
        seal: Default::default(),
    };

    let block = IndexedBlock {
        header: header.into(),
        uncles: vec![],
        commit_transactions: transactions,
        proposal_transactions: prop_ids,
    };
    chain.process_block(&block).unwrap();
    block
}

fn test_transaction(input_values: Vec<OutPoint>, output_num: usize) -> IndexedTransaction {
    test_transaction_with_capacity(input_values, output_num, 100_000)
}

fn test_transaction_with_capacity(
    input_values: Vec<OutPoint>,
    output_num: usize,
    capacity: u64,
) -> IndexedTransaction {
    let inputs: Vec<CellInput> = input_values
        .iter()
        .map(|x| CellInput::new(x.clone(), create_valid_script()))
        .collect();

    let mut output = CellOutput::default();
    output.capacity = capacity / output_num as u64;
    output.lock = create_valid_script().redeem_script_hash();
    let outputs: Vec<CellOutput> = vec![output.clone(); output_num];

    Transaction::new(0, Vec::new(), inputs, outputs).into()
}

// Since the main point here is to test pool functionality, not scripting
// behavior, we use a dummy script here that always passes in testing
fn create_valid_script() -> Script {
    let mut file = File::open("../spec/res/cells/always_success").unwrap();
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).unwrap();

    Script::new(0, Vec::new(), None, Some(buffer), Vec::new())
}
