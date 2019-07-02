use ckb_core::{transaction::Transaction, BlockNumber, Cycle};
use ckb_shared::shared::Shared;
use ckb_shared::tx_pool::PoolError;
use ckb_store::ChainStore;
use ckb_traits::chain_provider::ChainProvider;
use ckb_traits::BlockMedianTimeContext;
use ckb_verification::TransactionVerifier;
use fnv::FnvHashMap;
use numext_fixed_hash::H256;
use std::sync::Arc;

struct StoreBlockMedianTimeContext<CS> {
    store: Arc<CS>,
    median_time_block_count: u64,
}

impl<CS: ChainStore> BlockMedianTimeContext for StoreBlockMedianTimeContext<CS> {
    fn median_block_count(&self) -> u64 {
        self.median_time_block_count
    }

    fn timestamp_and_parent(&self, block_hash: &H256) -> (u64, H256) {
        let header = self
            .store
            .get_block_header(block_hash)
            .expect("[StoreBlockMedianTimeContext] blocks used for median time exist");
        (header.timestamp(), header.parent_hash().to_owned())
    }

    fn get_block_hash(&self, block_number: BlockNumber) -> Option<H256> {
        self.store.get_block_hash(block_number)
    }
}

/// TxPoolExecutor
/// execute txs in parallel then add them to tx_pool
pub struct TxPoolExecutor<CS> {
    shared: Shared<CS>,
}

impl<CS: ChainStore> TxPoolExecutor<CS> {
    pub fn new(shared: Shared<CS>) -> TxPoolExecutor<CS> {
        TxPoolExecutor { shared }
    }

    pub fn verify_and_add_tx_to_pool(&self, tx: Transaction) -> Result<Cycle, PoolError> {
        self.verify_and_add_txs_to_pool(vec![tx])
            .map(|cycles_vec| *cycles_vec.get(0).expect("tx verified cycles"))
    }

    pub fn verify_and_add_txs_to_pool(
        &self,
        txs: Vec<Transaction>,
    ) -> Result<Vec<Cycle>, PoolError> {
        if txs.is_empty() {
            return Ok(Vec::new());
        }
        // resolve txs
        // early release the chain_state lock because tx verification is slow
        let (resolved_txs, cached_txs, unresolvable_txs, consensus, block_number, epoch_number) = {
            let chain_state = self.shared.lock_chain_state();
            let txs_verify_cache = self.shared.lock_txs_verify_cache();
            let consensus = chain_state.consensus();
            let block_number = chain_state.tip_number() + 1;
            let epoch_number = chain_state.current_epoch_ext().number();
            let mut resolved_txs = Vec::with_capacity(txs.len());
            let mut unresolvable_txs = Vec::with_capacity(txs.len());
            let mut cached_txs = Vec::with_capacity(txs.len());
            for tx in &txs {
                if let Some(cycles) = txs_verify_cache.get(tx.hash()) {
                    cached_txs.push((tx.hash().to_owned(), Ok(*cycles)));
                } else {
                    match chain_state.resolve_tx_from_pending_and_proposed(tx) {
                        Ok(resolved_tx) => resolved_txs.push((tx.hash().to_owned(), resolved_tx)),
                        Err(err) => unresolvable_txs.push((
                            tx.hash().to_owned(),
                            PoolError::UnresolvableTransaction(err),
                        )),
                    }
                }
            }
            (
                resolved_txs,
                cached_txs,
                unresolvable_txs,
                consensus,
                block_number,
                epoch_number,
            )
        };

        // immediately return if resolved_txs is empty
        if resolved_txs.is_empty() && cached_txs.is_empty() {
            let (_tx, err) = unresolvable_txs.get(0).expect("unresolved tx exists");
            return Err(err.to_owned());
        }

        let store = Arc::clone(&self.shared.store());
        let max_block_cycles = consensus.max_block_cycles();
        let block_median_time_context = StoreBlockMedianTimeContext {
            store: Arc::clone(&store),
            median_time_block_count: consensus.median_time_block_count() as u64,
        };

        // parallet verify txs
        let cycles_vec = resolved_txs
            .iter()
            .map(|(tx_hash, tx)| {
                let verified_result = TransactionVerifier::new(
                    &tx,
                    &block_median_time_context,
                    block_number,
                    epoch_number,
                    &consensus,
                    self.shared.script_config(),
                    &store,
                )
                .verify(max_block_cycles)
                .map(|cycles| (tx, cycles))
                .map_err(PoolError::InvalidTx);
                (tx_hash.to_owned(), verified_result)
            })
            .collect::<Vec<_>>();

        // Add verified txs to pool
        // must lock chain_state before txs_verify_cache to avoid dead lock.
        let chain_state = self.shared.lock_chain_state();
        // write cache
        let cycles_vec = {
            let mut txs_verify_cache = self.shared.lock_txs_verify_cache();
            cycles_vec
                .into_iter()
                .map(|(i, result)| {
                    let result = match result {
                        Ok((rtx, cycles)) => {
                            let tx_hash = rtx.transaction.hash().to_owned();
                            txs_verify_cache.insert(tx_hash, cycles);
                            Ok(cycles)
                        }
                        Err(err) => Err(err),
                    };
                    (i, result)
                })
                .collect::<Vec<(H256, Result<Cycle, _>)>>()
        };

        // join all txs
        let cycles_vec = {
            let mut cycles_vec = cycles_vec
                .into_iter()
                .chain(cached_txs)
                .chain(unresolvable_txs.into_iter().map(|(tx, err)| (tx, Err(err))))
                .collect::<FnvHashMap<H256, Result<Cycle, PoolError>>>();
            txs.iter()
                .map(|tx| {
                    cycles_vec
                        .remove(tx.hash())
                        .expect("verified tx should exists")
                        .map(|cycles| (cycles, tx.to_owned()))
                })
                .collect::<Vec<Result<(Cycle, Transaction), PoolError>>>()
        };
        cycles_vec
            .into_iter()
            .map(|result| match result {
                Ok((cycles, tx)) => chain_state.add_tx_to_pool(tx, cycles),
                Err(err) => Err(err),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_chain::chain::ChainService;
    use ckb_chain_spec::consensus::Consensus;
    use ckb_core::block::BlockBuilder;
    use ckb_core::cell::UnresolvableError;
    use ckb_core::header::HeaderBuilder;
    use ckb_core::script::Script;
    use ckb_core::transaction::{CellInput, CellOutput, OutPoint, TransactionBuilder};
    use ckb_core::{capacity_bytes, Bytes, Capacity};
    use ckb_db::memorydb::MemoryKeyValueDB;
    use ckb_notify::NotifyService;
    use ckb_shared::shared::{Shared, SharedBuilder};
    use ckb_store::ChainKVStore;
    use ckb_test_chain_utils::create_always_success_cell;
    use ckb_traits::ChainProvider;
    use ckb_verification::TransactionError;
    use faketime::{self, unix_time_as_millis};
    use numext_fixed_uint::U256;

    fn setup(height: u64) -> (Shared<ChainKVStore<MemoryKeyValueDB>>, OutPoint) {
        let (always_success_cell, always_success_script) = create_always_success_cell();
        let always_success_tx = TransactionBuilder::default()
            .witness(always_success_script.clone().into_witness())
            .input(CellInput::new(OutPoint::null(), 0))
            .output(always_success_cell.clone())
            .build();
        let always_success_out_point = OutPoint::new_cell(always_success_tx.hash().to_owned(), 0);

        let mut block = BlockBuilder::default()
            .header_builder(
                HeaderBuilder::default()
                    .timestamp(unix_time_as_millis())
                    .difficulty(U256::from(1000u64)),
            )
            .transaction(always_success_tx)
            .build();
        let consensus = Consensus::default()
            .set_genesis_block(block.clone())
            .set_cellbase_maturity(0);

        let shared = SharedBuilder::<MemoryKeyValueDB>::new()
            .consensus(consensus)
            .build()
            .unwrap();

        let notify = NotifyService::default().start(Some("tx pool executor"));

        let chain_service = ChainService::new(shared.clone(), notify);
        let chain_controller = chain_service.start::<&str>(None);

        for _i in 0..height {
            let number = block.header().number() + 1;
            let timestamp = block.header().timestamp() + 1;

            let last_epoch = shared.get_block_epoch(&block.header().hash()).unwrap();
            let epoch = shared
                .next_epoch_ext(&last_epoch, block.header())
                .unwrap_or(last_epoch);

            let outputs = (0..20)
                .map(|_| {
                    CellOutput::new(
                        capacity_bytes!(50),
                        Bytes::default(),
                        always_success_script.to_owned(),
                        None,
                    )
                })
                .collect::<Vec<_>>();
            let cellbase = TransactionBuilder::default()
                .input(CellInput::new_cellbase_input(number))
                .outputs(outputs)
                .build();

            let txs = (10..20).map(|i| {
                TransactionBuilder::default()
                    .input(CellInput::new(
                        OutPoint::new_cell(cellbase.hash().to_owned(), i),
                        0,
                    ))
                    .output(CellOutput::new(
                        capacity_bytes!(50),
                        Bytes::default(),
                        always_success_script.to_owned(),
                        None,
                    ))
                    .dep(always_success_out_point.to_owned())
                    .build()
            });

            let header_builder = HeaderBuilder::default()
                .parent_hash(block.header().hash().to_owned())
                .number(number)
                .epoch(epoch.number())
                .timestamp(timestamp)
                .difficulty(epoch.difficulty().clone());

            block = BlockBuilder::default()
                .transaction(cellbase.clone())
                .transactions(txs)
                .header_builder(header_builder)
                .build();

            chain_controller
                .process_block(Arc::new(block.clone()), false)
                .expect("process block should be OK");
        }

        (shared, always_success_out_point)
    }

    #[test]
    fn test_verify_and_add_tx_to_pool() {
        let (shared, always_success_out_point) = setup(10);
        let last_block = shared
            .store()
            .get_block(&shared.lock_chain_state().tip_hash())
            .unwrap();
        let last_cellbase = last_block.transactions().first().unwrap();

        // building 10 txs and broadcast some
        let txs = (0..20u8)
            .map(|i| {
                TransactionBuilder::default()
                    .input(CellInput::new(
                        OutPoint::new_cell(last_cellbase.hash().to_owned(), u32::from(i)),
                        0,
                    ))
                    .output(CellOutput::new(
                        capacity_bytes!(50),
                        Bytes::from(vec![i]),
                        Script::default(),
                        None,
                    ))
                    .dep(always_success_out_point.to_owned())
                    .build()
            })
            .collect::<Vec<_>>();

        let tx_pool_executor = TxPoolExecutor::new(shared.clone());

        // spent cell
        let result = tx_pool_executor
            .verify_and_add_txs_to_pool(txs[1..=5].to_vec())
            .expect("verify relay tx");
        assert_eq!(result, vec![12; 5]);
        // spent conflict cell
        let result = tx_pool_executor.verify_and_add_txs_to_pool(txs[10..15].to_vec());
        assert_eq!(
            result,
            Err(PoolError::UnresolvableTransaction(UnresolvableError::Dead(
                txs[10].inputs()[0].previous_output.to_owned()
            )))
        );
        // spent half available half conflict cells
        let result = tx_pool_executor.verify_and_add_txs_to_pool(txs[6..=15].to_vec());
        assert_eq!(
            result,
            Err(PoolError::UnresolvableTransaction(UnresolvableError::Dead(
                txs[10].inputs()[0].previous_output.to_owned()
            )))
        );
        // spent one cell
        let result = tx_pool_executor
            .verify_and_add_tx_to_pool(txs[1].to_owned())
            .expect("verify relay tx");
        assert_eq!(result, 12);
        // spent one conflict cell
        let result = tx_pool_executor.verify_and_add_tx_to_pool(txs[13].to_owned());
        assert_eq!(
            result,
            Err(PoolError::UnresolvableTransaction(UnresolvableError::Dead(
                txs[13].inputs()[0].previous_output.to_owned()
            )))
        );
    }

    #[test]
    fn test_verify_and_add_invalid_since_tx_to_pool() {
        let (shared, always_success_out_point) = setup(10);
        let last_block = shared
            .store()
            .get_block(&shared.lock_chain_state().tip_hash())
            .unwrap();
        let last_cellbase = last_block.transactions().first().unwrap();
        let tip_number = shared.lock_chain_state().tip_number();

        let transactions: Vec<Transaction> = (tip_number - 1..=tip_number + 2)
            .map(|number| {
                let since = number;
                TransactionBuilder::default()
                    .input(CellInput::new(
                        OutPoint::new_cell(last_cellbase.hash().to_owned(), 0),
                        since,
                    ))
                    .output(CellOutput::new(
                        capacity_bytes!(50),
                        Bytes::default(),
                        Script::default(),
                        None,
                    ))
                    .dep(always_success_out_point.to_owned())
                    .build()
            })
            .collect();

        let tx_pool_executor = TxPoolExecutor::new(shared.clone());

        assert_eq!(
            tx_pool_executor.verify_and_add_tx_to_pool(transactions[0].clone()),
            Ok(12),
        );
        assert_eq!(
            tx_pool_executor.verify_and_add_tx_to_pool(transactions[1].clone()),
            Ok(12),
        );
        assert_eq!(
            tx_pool_executor.verify_and_add_tx_to_pool(transactions[2].clone()),
            Ok(12),
        );
        assert_eq!(
            tx_pool_executor.verify_and_add_tx_to_pool(transactions[3].clone()),
            Err(PoolError::InvalidTx(TransactionError::Immature)),
        );
    }
}
