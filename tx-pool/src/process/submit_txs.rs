use crate::component::entry::TxEntry;
use crate::error::PoolError;
use crate::pool::TxPool;
use ckb_script::ScriptConfig;
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
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::prelude::{Async, Poll};
use tokio::sync::lock::Lock;

pub struct PreResolveTxsProcess {
    pub tx_pool: Lock<TxPool>,
    pub txs: Option<Vec<TransactionView>>,
}

impl PreResolveTxsProcess {
    pub fn new(tx_pool: Lock<TxPool>, txs: Vec<TransactionView>) -> PreResolveTxsProcess {
        PreResolveTxsProcess {
            tx_pool,
            txs: Some(txs),
        }
    }
}

impl Future for PreResolveTxsProcess {
    type Item = (
        Byte32,
        Arc<Snapshot>,
        Vec<ResolvedTransaction>,
        Vec<(usize, Capacity, TxStatus)>,
    );
    type Error = PoolError;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.tx_pool.poll_lock() {
            Async::Ready(tx_pool) => {
                let txs = self.txs.take().expect("cannot execute twice");
                debug_assert!(!txs.is_empty(), "txs should not be empty!");
                let snapshot = tx_pool.cloned_snapshot();
                let tip_hash = snapshot.tip_hash();

                let mut txs_provider = TransactionsProvider::default();
                let resolved = txs
                    .iter()
                    .map(|tx| {
                        let ret = resolve_tx(&tx_pool, &snapshot, &txs_provider, tx.clone());
                        txs_provider.insert(tx);
                        ret
                    })
                    .collect::<Result<Vec<(ResolvedTransaction, usize, Capacity, TxStatus)>, _>>(
                    )?;

                let (rtxs, status) = resolved
                    .into_iter()
                    .map(|(rtx, tx_size, fee, status)| (rtx, (tx_size, fee, status)))
                    .unzip();

                Ok(Async::Ready((tip_hash, snapshot, rtxs, status)))
            }
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}

pub struct VerifyTxsProcess {
    pub snapshot: Arc<Snapshot>,
    pub txs_verify_cache: HashMap<Byte32, Cycle>,
    pub txs: Option<Vec<ResolvedTransaction>>,
    pub script_config: ScriptConfig,
}

impl VerifyTxsProcess {
    pub fn new(
        snapshot: Arc<Snapshot>,
        txs_verify_cache: HashMap<Byte32, Cycle>,
        txs: Vec<ResolvedTransaction>,
        script_config: ScriptConfig,
    ) -> VerifyTxsProcess {
        VerifyTxsProcess {
            snapshot,
            txs_verify_cache,
            script_config,
            txs: Some(txs),
        }
    }
}

impl Future for VerifyTxsProcess {
    type Item = Vec<(ResolvedTransaction, Cycle)>;
    type Error = PoolError;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let txs = self.txs.take().expect("cannot execute twice");

        Ok(Async::Ready(verify_rtxs(
            &self.snapshot,
            txs,
            &self.txs_verify_cache,
            &self.script_config,
        )?))
    }
}

pub struct SubmitTxsProcess {
    pub tx_pool: Lock<TxPool>,
    pub txs: Option<Vec<(ResolvedTransaction, Cycle)>>,
    pub pre_resolve_tip: Byte32,
    pub status: Option<Vec<(usize, Capacity, TxStatus)>>,
}

impl SubmitTxsProcess {
    pub fn new(
        tx_pool: Lock<TxPool>,
        txs: Vec<(ResolvedTransaction, Cycle)>,
        pre_resolve_tip: Byte32,
        status: Vec<(usize, Capacity, TxStatus)>,
    ) -> SubmitTxsProcess {
        SubmitTxsProcess {
            tx_pool,
            pre_resolve_tip,
            status: Some(status),
            txs: Some(txs),
        }
    }
}

impl Future for SubmitTxsProcess {
    type Item = (HashMap<Byte32, Cycle>, Vec<Cycle>);
    type Error = PoolError;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.tx_pool.poll_lock() {
            Async::Ready(mut guard) => {
                let executor = SubmitTxsExecutor {
                    tx_pool: &mut guard,
                };
                let txs = self.txs.take().expect("cannot execute twice");
                let status = self.status.take().expect("cannot execute twice");
                Ok(Async::Ready(executor.execute(
                    &self.pre_resolve_tip,
                    txs,
                    status,
                )?))
            }
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}

pub enum TxStatus {
    Fresh,
    Gap,
    Proposed,
}

struct SubmitTxsExecutor<'a> {
    tx_pool: &'a mut TxPool,
}

impl<'a> SubmitTxsExecutor<'a> {
    fn execute(
        self,
        pre_resolve_tip: &Byte32,
        txs: Vec<(ResolvedTransaction, Cycle)>,
        status: Vec<(usize, Capacity, TxStatus)>,
    ) -> Result<(HashMap<Byte32, Cycle>, Vec<Cycle>), PoolError> {
        let snapshot = self.tx_pool.snapshot();

        if pre_resolve_tip != &snapshot.tip_hash() {
            let mut txs_provider = TransactionsProvider::default();

            for (tx, cycles) in &txs {
                resolve_tx(
                    self.tx_pool,
                    snapshot,
                    &txs_provider,
                    tx.transaction.clone(),
                )?;
                txs_provider.insert(&tx.transaction);
            }
        }

        let cache = txs
            .iter()
            .map(|(tx, cycles)| (tx.transaction.hash(), *cycles))
            .collect();
        let cycles_vec = txs.iter().map(|(tx, cycles)| *cycles).collect();

        for ((rtx, cycles), (tx_size, fee, status)) in txs.into_iter().zip(status.into_iter()) {
            if self.tx_pool.reach_cycles_limit(cycles) {
                return Err(PoolError::LimitReached);
            }

            let related_dep_out_points = rtx.related_dep_out_points();
            let entry = TxEntry::new(
                rtx.transaction,
                cycles,
                fee,
                tx_size,
                related_dep_out_points,
            );
            if match status {
                TxStatus::Fresh => self.tx_pool.add_pending(entry),
                TxStatus::Gap => self.tx_pool.add_gap(entry),
                TxStatus::Proposed => self.tx_pool.add_proposed(entry),
            } {
                self.tx_pool.update_statics_for_add_tx(tx_size, cycles);
            }
        }

        Ok((cache, cycles_vec))
    }
}

fn resolve_tx<'a>(
    tx_pool: &TxPool,
    snapshot: &Snapshot,
    txs_provider: &'a TransactionsProvider<'a>,
    tx: TransactionView,
) -> Result<(ResolvedTransaction, usize, Capacity, TxStatus), PoolError> {
    let tx_size = tx.serialized_size();
    if tx_pool.reach_size_limit(tx_size) {
        return Err(PoolError::LimitReached);
    }

    let short_id = tx.proposal_short_id();
    if snapshot.proposals().contains_proposed(&short_id) {
        resolve_tx_from_proposed(tx_pool, snapshot, txs_provider, tx).and_then(|rtx| {
            let fee = tx_pool.calculate_transaction_fee(snapshot, &rtx);
            fee.map(|fee| (rtx, tx_size, fee, TxStatus::Proposed))
        })
    } else {
        resolve_tx_from_pending_and_proposed(tx_pool, snapshot, txs_provider, tx).and_then(|rtx| {
            let status = if snapshot.proposals().contains_gap(&short_id) {
                TxStatus::Gap
            } else {
                TxStatus::Fresh
            };
            let fee = tx_pool.calculate_transaction_fee(snapshot, &rtx);
            fee.map(|fee| (rtx, tx_size, fee, status))
        })
    }
}

fn resolve_tx_from_proposed<'a>(
    tx_pool: &TxPool,
    snapshot: &Snapshot,
    txs_provider: &'a TransactionsProvider<'a>,
    tx: TransactionView,
) -> Result<ResolvedTransaction, PoolError> {
    let cell_provider = OverlayCellProvider::new(&tx_pool.proposed, snapshot);
    let provider = OverlayCellProvider::new(txs_provider, &cell_provider);
    resolve_transaction(tx, &mut HashSet::new(), &provider, snapshot)
        .map_err(PoolError::UnresolvableTransaction)
}

fn resolve_tx_from_pending_and_proposed<'a>(
    tx_pool: &TxPool,
    snapshot: &Snapshot,
    txs_provider: &'a TransactionsProvider<'a>,
    tx: TransactionView,
) -> Result<ResolvedTransaction, PoolError> {
    let proposed_provider = OverlayCellProvider::new(&tx_pool.proposed, snapshot);
    let gap_and_proposed_provider = OverlayCellProvider::new(&tx_pool.gap, &proposed_provider);
    let pending_and_proposed_provider =
        OverlayCellProvider::new(&tx_pool.pending, &gap_and_proposed_provider);
    let provider = OverlayCellProvider::new(txs_provider, &pending_and_proposed_provider);
    resolve_transaction(tx, &mut HashSet::new(), &provider, snapshot)
        .map_err(PoolError::UnresolvableTransaction)
}

fn verify_rtxs(
    snapshot: &Snapshot,
    txs: Vec<ResolvedTransaction>,
    txs_verify_cache: &HashMap<Byte32, Cycle>,
    script_config: &ScriptConfig,
) -> Result<Vec<(ResolvedTransaction, Cycle)>, PoolError> {
    let tip_header = snapshot.tip_header();
    let tip_number = tip_header.number();
    let epoch_number = tip_header.epoch();
    let consensus = snapshot.consensus();

    txs.into_iter()
        .map(|tx| {
            let tx_hash = tx.transaction.hash();
            if let Some(cycles) = txs_verify_cache.get(&tx_hash) {
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
                .map(|_| (tx, *cycles))
            } else {
                TransactionVerifier::new(
                    &tx,
                    snapshot,
                    tip_number + 1,
                    epoch_number,
                    tip_header.hash(),
                    consensus,
                    &script_config,
                    snapshot,
                )
                .verify(consensus.max_block_cycles())
                .map_err(PoolError::InvalidTx)
                .map(|cycles| (tx, cycles))
            }
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}
