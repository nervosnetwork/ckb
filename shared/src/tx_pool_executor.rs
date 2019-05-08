use crate::shared::Shared;
use crate::tx_pool::PoolError;
use ckb_core::{transaction::Transaction, BlockNumber, Cycle};
use ckb_store::ChainStore;
use ckb_traits::BlockMedianTimeContext;
use ckb_verification::TransactionVerifier;
use fnv::FnvHashMap;
use numext_fixed_hash::H256;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use std::sync::Arc;

struct StoreBlockMedianTimeContext<CS> {
    store: Arc<CS>,
    median_time_block_count: u64,
}

impl<CS: ChainStore> BlockMedianTimeContext for StoreBlockMedianTimeContext<CS> {
    fn median_block_count(&self) -> u64 {
        self.median_time_block_count
    }

    fn timestamp(&self, number: BlockNumber) -> Option<u64> {
        self.store.get_block_hash(number).and_then(|hash| {
            self.store
                .get_header(&hash)
                .map(|header| header.timestamp())
        })
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
        let (resolved_txs, cached_txs, unresolvable_txs, consensus, tip_number) = {
            let chain_state = self.shared.chain_state().lock();
            let txs_verify_cache = self.shared.txs_verify_cache().lock();
            let consensus = chain_state.consensus();
            let tip_number = chain_state.tip_number();
            let mut resolved_txs = Vec::with_capacity(txs.len());
            let mut unresolvable_txs = Vec::with_capacity(txs.len());
            let mut cached_txs = Vec::with_capacity(txs.len());
            for tx in &txs {
                if let Some(cycles) = txs_verify_cache.get(tx.hash()) {
                    cached_txs.push((tx.hash().to_owned(), Ok(*cycles)));
                } else {
                    match chain_state.resolve_tx_from_pending_and_staging(tx) {
                        Ok(resolved_tx) => resolved_txs.push((tx.hash().to_owned(), resolved_tx)),
                        Err(err) => unresolvable_txs.push(err),
                    }
                }
            }
            (
                resolved_txs,
                cached_txs,
                unresolvable_txs,
                consensus,
                tip_number,
            )
        };

        // immediately return if resolved_txs is empty
        if resolved_txs.is_empty() {
            match unresolvable_txs.get(0) {
                Some(err) => return Err(PoolError::UnresolvableTransaction(err.to_owned())),
                None => return Ok(Vec::new()),
            }
        }

        let store = Arc::clone(&self.shared.store());
        let max_block_cycles = consensus.max_block_cycles();
        let block_median_time_context = StoreBlockMedianTimeContext {
            store: Arc::clone(&store),
            median_time_block_count: consensus.median_time_block_count() as u64,
        };
        // parallet verify txs
        let cycles_vec = resolved_txs
            .par_iter()
            .map(|(tx_hash, tx)| {
                let verified_result = TransactionVerifier::new(
                    &tx,
                    Arc::clone(&store),
                    &block_median_time_context,
                    tip_number,
                    consensus.cellbase_maturity(),
                    self.shared.script_config(),
                )
                .verify(max_block_cycles)
                .map(|cycles| (tx, cycles))
                .map_err(PoolError::InvalidTx);
                (tx_hash.to_owned(), verified_result)
            })
            .collect::<Vec<_>>();

        // write cache
        let cycles_vec = {
            let mut txs_verify_cache = self.shared.txs_verify_cache().lock();
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

        // join verified result with cached_txs
        let cycles_vec = {
            let mut cycles_vec = cycles_vec
                .into_iter()
                .chain(cached_txs)
                .collect::<FnvHashMap<H256, Result<Cycle, PoolError>>>();
            txs.iter()
                .filter_map(|tx| {
                    cycles_vec
                        .remove(tx.hash())
                        .map(|result| result.map(|cycles| (cycles, tx.to_owned())))
                })
                .collect::<Vec<Result<(Cycle, Transaction), PoolError>>>()
        };
        // add verified txs to pool
        let chain_state = self.shared.chain_state().lock();
        cycles_vec
            .into_iter()
            .map(|result| match result {
                Ok((cycles, tx)) => chain_state.add_tx_to_pool(tx, cycles),
                Err(err) => Err(err),
            })
            .collect()
    }
}
