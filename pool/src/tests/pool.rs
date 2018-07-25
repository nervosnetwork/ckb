use bigint::H256;
use std::sync::Arc;

use txs_pool::pool::*;
use txs_pool::types::*;

use ckb_chain::chain::{ChainBuilder, ChainProvider};
use ckb_chain::store::ChainKVStore;
use ckb_db::memorydb::MemoryKeyValueDB;
use ckb_notify::Notify;
use core::block::{Block, IndexedBlock};
use core::cell::{CellProvider, CellState};
use core::difficulty::cal_difficulty;
use core::header::{Header, RawHeader, Seal};
use core::script::Script;
use core::transaction::*;
use time::now_ms;

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

#[test]
/// A basic test; add a pair of transactions to the pool.
fn test_basic_pool_add() {
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

    // First, add the transaction rooted in the blockchain
    let result = pool.add_to_memory_pool(parent_transaction);
    if result.is_err() {
        panic!("got an error adding parent tx: {:?}", result.err().unwrap());
    }

    // Now, add the transaction connected as a child to the first
    let child_result = pool.add_to_memory_pool(child_transaction);

    if child_result.is_err() {
        panic!(
            "got an error adding child tx: {:?}",
            child_result.err().unwrap()
        );
    }

    assert_eq!(pool.total_size(), 2);
    expect_output_parent!(pool, CellState::Pool(_), OutPoint::new(child_tx_hash, 0));
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
/// Testing various expected error conditions
pub fn test_pool_add_error() {
    let (_chain, pool, tx_hash) = test_setup();
    assert_eq!(pool.total_size(), 0);

    // let duplicate_tx = test_transaction(vec![OutPoint::new(tx_hash, 5), OutPoint::new(tx_hash, 6)], 1);

    // To test DoubleSpend and AlreadyInPool conditions, we need to add
    // a valid transaction.
    let valid_transaction = test_transaction(
        vec![OutPoint::new(tx_hash, 5), OutPoint::new(tx_hash, 6)],
        2,
    );

    match pool.add_to_memory_pool(valid_transaction.clone()) {
        Ok(_) => {}
        Err(_) => panic!("Unexpected error while adding a valid transaction"),
    };

    // Now, test a DoubleSpend by consuming the same blockchain unspent
    // as valid_transaction:
    let double_spend_transaction = test_transaction(vec![OutPoint::new(tx_hash, 6)], 2);

    match pool.add_to_memory_pool(double_spend_transaction) {
        Ok(_) => panic!("Expected error when adding double spend, got Ok"),
        Err(x) => {
            match x {
                PoolError::InvalidTx(_) => {}
                _ => panic!("Unexpected error when adding double spend transaction"),
            };
        }
    };

    // Note, this used to work as expected, but after aggsig implementation
    // creating another transaction with the same inputs/outputs doesn't create
    // the same hash ID due to the random nonces in an aggsig. This
    // will instead throw a (correct as well) Already spent error. An AlreadyInPool
    // error can only come up in the case of the exact same transaction being
    // added
    //let already_in_pool = test_transaction(vec![5, 6], vec![9]);

    match pool.add_to_memory_pool(valid_transaction) {
        Ok(_) => panic!("Expected error when adding already in pool, got Ok"),
        Err(x) => {
            match x {
                PoolError::AlreadyInPool => {}
                _ => panic!("Unexpected error when adding already in pool tx: {:?}", x),
            };
        }
    };

    assert_eq!(pool.total_size(), 1);
}

// // #[test]
// // /// Testing an expected orphan
// // fn test_add_orphan() {
// //     // TODO we need a test here
// // }

#[test]
fn test_zero_confirmation_reconciliation() {
    let (_chain, pool, tx_hash) = test_setup();

    // now create two txs
    // tx1 spends the Output
    // tx2 spends output from tx1
    let tx1 = test_transaction(vec![OutPoint::new(tx_hash, 0)], 1);
    let tx1_hash = tx1.hash();
    let tx2 = test_transaction(vec![OutPoint::new(tx1_hash, 0)], 1);

    // now add both txs to the pool (tx2 spends tx1 with zero confirmations)
    // both should be accepted if tx1 added before tx2
    pool.add_to_memory_pool(tx1).unwrap();
    pool.add_to_memory_pool(tx2).unwrap();

    assert_eq!(pool.pool_size(), 2);

    let mut block: IndexedBlock = Block::default().into();

    let txs = pool.prepare_mineable_transactions(3);

    // confirm we can preparing both txs for mining here
    // one root tx in the pool, and one non-root vertex in the pool
    assert_eq!(txs.len(), 2);
    block.transactions = txs;

    // now reconcile the block
    pool.reconcile_block(&block);

    // check the pool is consistent after reconciling the block
    // we should have zero txs in the pool (neither roots nor non-roots)

    assert_eq!(pool.pool_size(), 0);
}

#[test]
/// Testing block reconciliation
fn test_block_reconciliation() {
    let (chain, pool, tx_hash) = test_setup();
    // Preparation: We will introduce a three root pool transactions.
    // 1. A transaction that should be invalidated because it is exactly
    //  contained in the block.
    // 2. A transaction that should be invalidated because the input is
    //  consumed in the block, although it is not exactly consumed.
    // 3. A transaction that should remain after block reconciliation.
    let block_transaction = test_transaction(vec![OutPoint::new(tx_hash, 10)], 1);
    let conflict_transaction = test_transaction(vec![OutPoint::new(tx_hash, 20)], 2);
    let valid_transaction = test_transaction(vec![OutPoint::new(tx_hash, 30)], 2);

    // We will also introduce a few children:
    // 4. A transaction that descends from transaction 1, that is in
    //  turn exactly contained in the block.
    let block_tx_hash = block_transaction.hash();
    let block_child = test_transaction(vec![OutPoint::new(block_tx_hash, 0)], 2);
    // 5. A transaction that descends from transaction 4, that is not
    //  contained in the block at all and should be valid after
    //  reconciliation.
    let block_child_tx_hash = block_child.hash();
    let pool_child = test_transaction(vec![OutPoint::new(block_child_tx_hash, 0)], 1);
    // 6. A transaction that descends from transaction 2 that does not
    //  conflict with anything in the block in any way, but should be
    //  invalidated (orphaned).
    let conflict_tx_hash = conflict_transaction.hash();
    let conflict_child = test_transaction(vec![OutPoint::new(conflict_tx_hash, 0)], 1);
    // 7. A transaction that descends from transaction 2 that should be
    //  valid due to its inputs being satisfied by the block.
    let conflict_valid_child = test_transaction(vec![OutPoint::new(conflict_tx_hash, 1)], 1);
    // 8. A transaction that descends from transaction 3 that should be
    //  invalidated due to an output conflict.
    let valid_tx_hash = valid_transaction.hash();
    let valid_child_conflict = test_transaction(vec![OutPoint::new(valid_tx_hash, 0)], 1);
    // 9. A transaction that descends from transaction 3 that should remain
    //  valid after reconciliation.
    let valid_child_valid = test_transaction(vec![OutPoint::new(valid_tx_hash, 1)], 1);
    // 10. A transaction that descends from both transaction 6 and
    //  transaction 9
    let conflict_child_tx_hash = conflict_child.hash();
    let valid_child_valid_tx_hash = valid_child_valid.hash();
    let mixed_child = test_transaction(
        vec![
            OutPoint::new(conflict_child_tx_hash, 0),
            OutPoint::new(valid_child_valid_tx_hash, 0),
        ],
        1,
    );
    let mixed_child_tx_hash = mixed_child.hash();
    // Add transactions.
    // Note: There are some ordering constraints that must be followed here
    // until orphans is 100% implemented. Once the orphans process has
    // stabilized, we can mix these up to exercise that path a bit.
    let mut txs_to_add = vec![
        block_transaction,
        conflict_transaction,
        valid_transaction,
        block_child,
        pool_child,
        conflict_child,
        conflict_valid_child,
        valid_child_conflict,
        valid_child_valid,
        mixed_child,
    ];

    let expected_pool_size = txs_to_add.len();

    // First we add the above transactions to the pool; all should be
    // accepted.
    assert_eq!(pool.total_size(), 0);

    for tx in txs_to_add.drain(..) {
        pool.add_to_memory_pool(tx).unwrap();
    }

    assert_eq!(pool.total_size(), expected_pool_size);

    // Now we prepare the block that will cause the above condition.
    // First, the transactions we want in the block:
    // - Copy of 1
    let block_tx_1 = test_transaction(vec![OutPoint::new(tx_hash, 10)], 1);
    // - Conflict w/ 2, satisfies 7
    let block_tx_2 = test_transaction(vec![OutPoint::new(tx_hash, 20)], 1);
    // - Copy of 4
    let block_tx_3 = test_transaction(vec![OutPoint::new(block_tx_hash, 0)], 2);

    let block = apply_transactions(vec![block_tx_1, block_tx_2, block_tx_3], &chain);

    // Block reconciliation
    pool.reconcile_block(&block);

    // Using the pool's methods to validate a few end conditions.
    assert_eq!(pool.total_size(), 4);

    // We should have available blockchain outputs
    expect_output_parent!(
        pool,
        CellState::Head(_),
        OutPoint::new(block_child_tx_hash, 1)
    );

    // We should have spent blockchain outputs
    expect_output_parent!(pool, CellState::Tail, OutPoint::new(block_child_tx_hash, 0));

    // We should have spent pool references
    expect_output_parent!(pool, CellState::Tail, OutPoint::new(valid_tx_hash, 1));

    // We should have unspent pool references
    expect_output_parent!(
        pool,
        CellState::Pool(_),
        OutPoint::new(valid_child_valid_tx_hash, 0)
    );

    expect_output_parent!(pool, CellState::Tail, OutPoint::new(block_tx_hash, 0));

    // Evicted transactions should have unknown outputs
    expect_output_parent!(
        pool,
        CellState::Unknown,
        OutPoint::new(mixed_child_tx_hash, 0)
    );
}

#[test]
/// Test transaction selection and block building.
fn test_block_building() {
    let (_chain, pool, tx_hash) = test_setup();

    let root_tx_1 = test_transaction(
        vec![OutPoint::new(tx_hash, 10), OutPoint::new(tx_hash, 20)],
        1,
    );
    let root_tx_2 = test_transaction(vec![OutPoint::new(tx_hash, 30)], 1);
    let root_tx_3 = test_transaction(vec![OutPoint::new(tx_hash, 40)], 1);

    let root_tx_hash_1 = root_tx_1.hash();
    let root_tx_hash_3 = root_tx_3.hash();
    let child_tx_1 = test_transaction(vec![OutPoint::new(root_tx_hash_1, 0)], 1);
    let child_tx_2 = test_transaction(vec![OutPoint::new(root_tx_hash_3, 0)], 1);

    assert_eq!(pool.total_size(), 0);

    assert!(pool.add_to_memory_pool(root_tx_1).is_ok());
    assert!(pool.add_to_memory_pool(root_tx_2).is_ok());
    assert!(pool.add_to_memory_pool(root_tx_3).is_ok());
    assert!(pool.add_to_memory_pool(child_tx_1).is_ok());
    assert!(pool.add_to_memory_pool(child_tx_2).is_ok());

    assert_eq!(pool.total_size(), 5);

    // Request blocks
    let mut block: IndexedBlock = Block::default().into();

    let txs = pool.prepare_mineable_transactions(3);
    assert_eq!(txs.len(), 3);

    block.transactions = txs;

    // Reconcile block

    pool.reconcile_block(&block);

    assert_eq!(pool.total_size(), 2);
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
    let pool = TransactionPool::new(PoolConfig::default(), Arc::clone(&chain), notify);

    let tx = Transaction::new(
        0,
        Vec::new(),
        vec![CellInput::new(OutPoint::null(), Default::default())],
        vec![CellOutput::new(0, 50, Vec::new(), H256::default()); 100],
    );
    let transactions = vec![tx.clone()];
    apply_transactions(transactions, &chain);
    (chain, pool, tx.hash())
}

fn apply_transactions(
    transactions: Vec<Transaction>,
    chain: &Arc<impl ChainProvider>,
) -> IndexedBlock {
    let time = now_ms();
    let header = Header {
        raw: RawHeader::new(
            &chain.tip_header().read().header,
            transactions.iter(),
            time,
            cal_difficulty(&chain.tip_header().read().header, time),
        ),
        seal: Seal {
            nonce: 0,
            mix_hash: H256::from(0),
        },
    };

    let block = IndexedBlock {
        header: header.into(),
        transactions,
    };
    chain.process_block(&block).unwrap();
    block
}

fn test_transaction(input_values: Vec<OutPoint>, output_num: usize) -> Transaction {
    let inputs: Vec<CellInput> = input_values
        .iter()
        .map(|x| CellInput::new(x.clone(), Script::new(1, Vec::new(), Vec::new())))
        .collect();

    let mut output = CellOutput::default();
    output.capacity = 100_000;
    let outputs: Vec<CellOutput> = vec![output.clone(); output_num];

    Transaction::new(0, Vec::new(), inputs, outputs)
}
