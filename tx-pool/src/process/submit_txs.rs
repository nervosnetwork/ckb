use crate::component::entry::TxEntry;
use crate::error::PoolError;
use crate::pool::TxPool;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{
        cell::{
            resolve_transaction, OverlayCellProvider, ResolvedTransaction, TransactionsProvider,
        },
        Capacity, Cycle, TransactionView,
    },
    packed::Byte32,
};
use ckb_verification::{ContextualTransactionVerifier, TransactionVerifier};
use futures::future::Future;
use std::collections::HashMap;
use std::collections::HashSet;
use tokio::prelude::{Async, Poll};
use tokio::sync::lock::Lock;

pub struct SubmitTxsProcess {
    pub tx_pool: Lock<TxPool>,
    pub txs_verify_cache: HashMap<Byte32, Cycle>,
    pub txs: Option<Vec<TransactionView>>,
}

impl SubmitTxsProcess {
    pub fn new(
        tx_pool: Lock<TxPool>,
        txs_verify_cache: HashMap<Byte32, Cycle>,
        txs: Vec<TransactionView>,
    ) -> SubmitTxsProcess {
        SubmitTxsProcess {
            tx_pool,
            txs_verify_cache,
            txs: Some(txs),
        }
    }
}

impl Future for SubmitTxsProcess {
    type Item = Result<(HashMap<Byte32, Cycle>, Vec<Cycle>), PoolError>;
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.tx_pool.poll_lock() {
            Async::Ready(mut guard) => {
                let executor = SubmitTxsExecutor {
                    tx_pool: &mut guard,
                    txs_verify_cache: &self.txs_verify_cache,
                };

                Ok(Async::Ready(
                    executor.execute(self.txs.take().expect("cannot execute twice")),
                ))
            }
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}

enum TxStatus {
    Fresh,
    Gap,
    Proposed,
}

struct SubmitTxsExecutor<'a> {
    tx_pool: &'a mut TxPool,
    txs_verify_cache: &'a HashMap<Byte32, Cycle>,
}

impl<'a> SubmitTxsExecutor<'a> {
    fn execute(
        self,
        txs: Vec<TransactionView>,
    ) -> Result<(HashMap<Byte32, Cycle>, Vec<Cycle>), PoolError> {
        debug_assert!(!txs.is_empty(), "txs should not be empty!");
        let snapshot = self.tx_pool.snapshot();
        let mut txs_provider = TransactionsProvider::default();
        let resolved = txs
            .iter()
            .map(|tx| {
                let ret = self.resolve_tx(snapshot, &txs_provider, tx);
                txs_provider.insert(tx);
                ret
            })
            .collect::<Result<Vec<(ResolvedTransaction<'_>, usize, Capacity, TxStatus)>, _>>()?;

        let (cache, verified_cycles) =
            verify_rtxs(self.tx_pool, snapshot, &resolved[..], self.txs_verify_cache)?;

        for ((tx, (rtx, tx_size, fee, status)), cycle) in txs
            .iter()
            .zip(resolved.into_iter())
            .zip(verified_cycles.iter())
        {
            let related_dep_out_points = rtx.related_dep_out_points();
            let entry = TxEntry::new(tx.clone(), *cycle, fee, tx_size, related_dep_out_points);
            if match status {
                TxStatus::Fresh => self.tx_pool.add_pending(entry),
                TxStatus::Gap => self.tx_pool.add_gap(entry),
                TxStatus::Proposed => self.tx_pool.add_proposed(entry),
            } {
                self.tx_pool.update_statics_for_add_tx(tx_size, *cycle);
            }
        }

        Ok((cache, verified_cycles))
    }

    fn resolve_tx<'b, 'c>(
        &self,
        snapshot: &Snapshot,
        txs_provider: &'c TransactionsProvider<'c>,
        tx: &'b TransactionView,
    ) -> Result<(ResolvedTransaction<'b>, usize, Capacity, TxStatus), PoolError> {
        let tx_size = tx.serialized_size();
        if self.tx_pool.reach_size_limit(tx_size) {
            return Err(PoolError::LimitReached);
        }

        let short_id = tx.proposal_short_id();
        if snapshot.proposals().contains_proposed(&short_id) {
            self.resolve_tx_from_proposed(snapshot, txs_provider, tx)
                .and_then(|rtx| {
                    let fee = self.tx_pool.calculate_transaction_fee(snapshot, &rtx);
                    fee.map(|fee| (rtx, tx_size, fee, TxStatus::Proposed))
                })
        } else {
            self.resolve_tx_from_pending_and_proposed(snapshot, txs_provider, tx)
                .and_then(|rtx| {
                    let status = if snapshot.proposals().contains_gap(&short_id) {
                        TxStatus::Gap
                    } else {
                        TxStatus::Fresh
                    };
                    let fee = self.tx_pool.calculate_transaction_fee(snapshot, &rtx);
                    fee.map(|fee| (rtx, tx_size, fee, status))
                })
        }
    }

    fn resolve_tx_from_proposed<'b, 'c>(
        &self,
        snapshot: &Snapshot,
        txs_provider: &'c TransactionsProvider<'c>,
        tx: &'b TransactionView,
    ) -> Result<ResolvedTransaction<'b>, PoolError> {
        let cell_provider = OverlayCellProvider::new(&self.tx_pool.proposed, snapshot);
        let provider = OverlayCellProvider::new(txs_provider, &cell_provider);
        resolve_transaction(tx, &mut HashSet::new(), &provider, snapshot)
            .map_err(PoolError::UnresolvableTransaction)
    }

    fn resolve_tx_from_pending_and_proposed<'b, 'c>(
        &self,
        snapshot: &Snapshot,
        txs_provider: &'c TransactionsProvider<'c>,
        tx: &'b TransactionView,
    ) -> Result<ResolvedTransaction<'b>, PoolError> {
        let proposed_provider = OverlayCellProvider::new(&self.tx_pool.proposed, snapshot);
        let gap_and_proposed_provider =
            OverlayCellProvider::new(&self.tx_pool.gap, &proposed_provider);
        let pending_and_proposed_provider =
            OverlayCellProvider::new(&self.tx_pool.pending, &gap_and_proposed_provider);
        let provider = OverlayCellProvider::new(txs_provider, &pending_and_proposed_provider);
        resolve_transaction(tx, &mut HashSet::new(), &provider, snapshot)
            .map_err(PoolError::UnresolvableTransaction)
    }
}

fn verify_rtxs<'b>(
    tx_pool: &TxPool,
    snapshot: &Snapshot,
    rtxs: &'b [(ResolvedTransaction<'_>, usize, Capacity, TxStatus)],
    txs_verify_cache: &HashMap<Byte32, Cycle>,
) -> Result<(HashMap<Byte32, Cycle>, Vec<Cycle>), PoolError> {
    let tip_header = snapshot.tip_header();
    let tip_number = tip_header.number();
    let epoch_number = tip_header.epoch();
    let consensus = snapshot.consensus();
    let mut cache = HashMap::new();
    let verified = rtxs
        .iter()
        .map(|(tx, _, _, _)| {
            let tx_hash = tx.transaction.hash();
            if let Some(cycles) = txs_verify_cache.get(&tx_hash) {
                if tx_pool.reach_cycles_limit(*cycles) {
                    Err(PoolError::LimitReached)
                } else {
                    ContextualTransactionVerifier::new(
                        &tx,
                        snapshot,
                        tip_number + 1,
                        epoch_number,
                        tip_header.hash(),
                        consensus,
                    )
                    .verify()
                    .map_err(PoolError::InvalidTx)
                    .map(|_| (tx_hash, *cycles))
                }
            } else {
                TransactionVerifier::new(
                    &tx,
                    snapshot,
                    tip_number + 1,
                    epoch_number,
                    tip_header.hash(),
                    consensus,
                    &tx_pool.script_config,
                    snapshot,
                )
                .verify(consensus.max_block_cycles())
                .map_err(PoolError::InvalidTx)
                .and_then(|cycles| {
                    if tx_pool.reach_cycles_limit(cycles) {
                        Err(PoolError::LimitReached)
                    } else {
                        Ok((tx_hash, cycles))
                    }
                })
            }
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut ret = Vec::with_capacity(verified.len());
    for (hash, cycles) in verified {
        cache.insert(hash, cycles);
        ret.push(cycles);
    }

    Ok((cache, ret))
}
