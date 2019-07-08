use crate::relayer::compact_block::{CompactBlock, ShortTransactionID};
use crate::{Relayer, SyncSharedState};
use ckb_chain::chain::ChainService;
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::header::HeaderBuilder;
use ckb_core::script::Script;
use ckb_core::transaction::{
    CellInput, CellOutput, IndexTransaction, OutPoint, Transaction, TransactionBuilder,
};
use ckb_core::{capacity_bytes, BlockNumber, Bytes, Capacity};
use ckb_db::memorydb::MemoryKeyValueDB;
use ckb_notify::NotifyService;
use ckb_protocol::{short_transaction_id, short_transaction_id_keys};
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_store::{ChainKVStore, ChainStore};
use ckb_test_chain_utils::create_always_success_cell;
use ckb_traits::ChainProvider;
use faketime::{self, unix_time_as_millis};
use numext_fixed_uint::U256;
use std::sync::Arc;

fn new_header_builder(
    shared: &Shared<ChainKVStore<MemoryKeyValueDB>>,
    parent: &Block,
) -> HeaderBuilder {
    let parent_hash = parent.header().hash();
    let parent_epoch = shared.get_block_epoch(&parent_hash).unwrap();
    let epoch = shared
        .next_epoch_ext(&parent_epoch, parent.header())
        .unwrap_or(parent_epoch);
    HeaderBuilder::default()
        .parent_hash(parent_hash.to_owned())
        .number(parent.header().number() + 1)
        .timestamp(parent.header().timestamp() + 1)
        .epoch(epoch.number())
        .difficulty(epoch.difficulty().to_owned())
}

fn new_transaction(
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

fn build_chain(tip: BlockNumber) -> (Relayer<ChainKVStore<MemoryKeyValueDB>>, OutPoint) {
    let (always_success_cell, always_success_script) = create_always_success_cell();
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
        let block = BlockBuilder::from_header_builder(new_header_builder(&shared, &parent))
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

#[test]
fn test_reconstruct_block() {
    let (relayer, always_success_out_point) = build_chain(5);
    let prepare: Vec<Transaction> = (0..20)
        .map(|i| new_transaction(&relayer, i, &always_success_out_point))
        .collect();

    // Case: miss tx.0
    {
        let mut compact = CompactBlock {
            nonce: 2,
            ..Default::default()
        };
        let (key0, key1) = short_transaction_id_keys(compact.header.nonce(), compact.nonce);
        let short_ids = prepare
            .iter()
            .map(|tx| short_transaction_id(key0, key1, &tx.witness_hash()))
            .collect();
        let transactions: Vec<Transaction> = prepare.iter().skip(1).cloned().collect();
        compact.short_ids = short_ids;
        let chain_state = relayer.shared.lock_chain_state();
        assert_eq!(
            relayer.reconstruct_block(&chain_state, &compact, transactions),
            Err(vec![0]),
        );
    }

    // Case: miss multiple txs
    {
        let mut compact = CompactBlock {
            nonce: 2,
            ..Default::default()
        };
        let (key0, key1) = short_transaction_id_keys(compact.header.nonce(), compact.nonce);
        let short_ids = prepare
            .iter()
            .map(|tx| short_transaction_id(key0, key1, &tx.witness_hash()))
            .collect();
        let transactions: Vec<Transaction> = prepare.iter().skip(1).step_by(2).cloned().collect();
        let missing = prepare
            .iter()
            .enumerate()
            .step_by(2)
            .map(|(i, _)| i)
            .collect();
        compact.short_ids = short_ids;
        let chain_state = relayer.shared.lock_chain_state();
        assert_eq!(
            relayer.reconstruct_block(&chain_state, &compact, transactions),
            Err(missing),
        );
    }

    // Case: short transactions lie on pool but not proposed, cannot be used to reconstruct block
    {
        let mut compact = CompactBlock {
            nonce: 3,
            ..Default::default()
        };
        let (key0, key1) = short_transaction_id_keys(compact.header.nonce(), compact.nonce);
        let (short_transactions, prefilled) = {
            let short_transactions: Vec<Transaction> = prepare.iter().step_by(2).cloned().collect();
            let prefilled: Vec<IndexTransaction> = prepare
                .iter()
                .enumerate()
                .skip(1)
                .step_by(2)
                .map(|(i, tx)| IndexTransaction {
                    index: i,
                    transaction: tx.clone(),
                })
                .collect();
            (short_transactions, prefilled)
        };
        let short_ids: Vec<ShortTransactionID> = short_transactions
            .iter()
            .map(|tx| short_transaction_id(key0, key1, &tx.witness_hash()))
            .collect();
        compact.short_ids = short_ids;
        compact.prefilled_transactions = prefilled;

        // Split first 2 short transactions and move into pool. These pool transactions are not
        // proposed, so it will not be acquired inside `reconstruct_block`
        let (pool_transactions, short_transactions) = short_transactions.split_at(2);
        let short_transactions: Vec<Transaction> = short_transactions.to_vec();
        pool_transactions.iter().for_each(|tx| {
            // `tx` is added into pool but not be proposed, since `tx` has not been proposal yet
            relayer
                .tx_pool_executor
                .verify_and_add_tx_to_pool(tx.clone())
                .expect("adding transaction into pool");
        });

        let chain_state = relayer.shared.lock_chain_state();
        assert_eq!(
            relayer.reconstruct_block(&chain_state, &compact, short_transactions),
            Err(vec![0, 2]),
        );
    }
}
