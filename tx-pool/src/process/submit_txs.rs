use crate::component::entry::TxEntry;
use crate::error::SubmitTxError;
use crate::pool::TxPool;
use crate::FeeRate;
use ckb_error::{Error, InternalErrorKind};
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
use ckb_verification::cache::CacheEntry;
use ckb_verification::{ContextualTransactionVerifier, TransactionVerifier};
use futures::future::Future;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::prelude::{Async, Poll};
use tokio::sync::lock::Lock;

type ResolveResult = Result<(ResolvedTransaction, usize, Capacity, TxStatus), Error>;

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

type PreResolveTxsItem = (
    Byte32,
    Arc<Snapshot>,
    Vec<ResolvedTransaction>,
    Vec<(usize, Capacity, TxStatus)>,
);

impl Future for PreResolveTxsProcess {
    type Item = PreResolveTxsItem;
    type Error = Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.tx_pool.poll_lock() {
            Async::Ready(tx_pool) => {
                let txs = self.txs.take().expect("cannot execute twice");
                debug_assert!(!txs.is_empty(), "txs should not be empty!");
                let snapshot = tx_pool.cloned_snapshot();
                let tip_hash = snapshot.tip_hash();

                check_transaction_hash_collision(&tx_pool, &txs)?;

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
    pub txs_verify_cache: HashMap<Byte32, CacheEntry>,
    pub txs: Option<Vec<ResolvedTransaction>>,
    pub max_tx_verify_cycles: Cycle,
}

impl VerifyTxsProcess {
    pub fn new(
        snapshot: Arc<Snapshot>,
        txs_verify_cache: HashMap<Byte32, CacheEntry>,
        txs: Vec<ResolvedTransaction>,
        max_tx_verify_cycles: Cycle,
    ) -> VerifyTxsProcess {
        VerifyTxsProcess {
            snapshot,
            txs_verify_cache,
            txs: Some(txs),
            max_tx_verify_cycles,
        }
    }
}

impl Future for VerifyTxsProcess {
    type Item = Vec<(ResolvedTransaction, CacheEntry)>;
    type Error = Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let txs = self.txs.take().expect("cannot execute twice");

        Ok(Async::Ready(verify_rtxs(
            &self.snapshot,
            txs,
            &self.txs_verify_cache,
            self.max_tx_verify_cycles,
        )?))
    }
}

pub struct SubmitTxsProcess {
    pub tx_pool: Lock<TxPool>,
    pub txs: Option<Vec<(ResolvedTransaction, CacheEntry)>>,
    pub pre_resolve_tip: Byte32,
    pub status: Option<Vec<(usize, Capacity, TxStatus)>>,
}

impl SubmitTxsProcess {
    pub fn new(
        tx_pool: Lock<TxPool>,
        txs: Vec<(ResolvedTransaction, CacheEntry)>,
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
    type Item = (HashMap<Byte32, CacheEntry>, Vec<CacheEntry>);
    type Error = Error;

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
        txs: Vec<(ResolvedTransaction, CacheEntry)>,
        status: Vec<(usize, Capacity, TxStatus)>,
    ) -> Result<(HashMap<Byte32, CacheEntry>, Vec<CacheEntry>), Error> {
        let snapshot = self.tx_pool.snapshot();

        if pre_resolve_tip != &snapshot.tip_hash() {
            let mut txs_provider = TransactionsProvider::default();

            for (tx, _) in &txs {
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
        let cycles_vec = txs.iter().map(|(_, cycles)| *cycles).collect();

        for ((rtx, cache_entry), (tx_size, fee, status)) in txs.into_iter().zip(status.into_iter())
        {
            if self.tx_pool.reach_cycles_limit(cache_entry.cycles) {
                return Err(InternalErrorKind::TransactionPoolFull.into());
            }

            let min_fee = self.tx_pool.config.min_fee_rate.fee(tx_size);
            // reject txs which fee lower than min fee rate
            if fee < min_fee {
                return Err(SubmitTxError::LowFeeRate(min_fee.as_u64()).into());
            }

            let related_dep_out_points = rtx.related_dep_out_points();
            let entry = TxEntry::new(
                rtx.transaction,
                cache_entry.cycles,
                fee,
                tx_size,
                related_dep_out_points,
            );
            let inserted = match status {
                TxStatus::Fresh => {
                    let tx_hash = entry.transaction.hash();
                    let inserted = self.tx_pool.add_pending(entry)?;
                    if inserted {
                        let height = self.tx_pool.snapshot().tip_number();
                        let fee_rate = FeeRate::calculate(fee, tx_size);
                        self.tx_pool
                            .fee_estimator
                            .track_tx(tx_hash, fee_rate, height);
                    }
                    inserted
                }
                TxStatus::Gap => self.tx_pool.add_gap(entry)?,
                TxStatus::Proposed => self.tx_pool.add_proposed(entry)?,
            };
            if inserted {
                self.tx_pool
                    .update_statics_for_add_tx(tx_size, cache_entry.cycles);
            }
        }

        Ok((cache, cycles_vec))
    }
}

fn check_transaction_hash_collision(
    tx_pool: &TxPool,
    txs: &[TransactionView],
) -> Result<(), Error> {
    for tx in txs {
        let short_id = tx.proposal_short_id();
        if tx_pool.contains_proposal_id(&short_id) {
            return Err(InternalErrorKind::PoolTransactionDuplicated.into());
        }
    }
    Ok(())
}

fn resolve_tx<'a>(
    tx_pool: &TxPool,
    snapshot: &Snapshot,
    txs_provider: &'a TransactionsProvider<'a>,
    tx: TransactionView,
) -> ResolveResult {
    let tx_size = tx.data().serialized_size_in_block();
    if tx_pool.reach_size_limit(tx_size) {
        return Err(InternalErrorKind::TransactionPoolFull.into());
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
) -> Result<ResolvedTransaction, Error> {
    let cell_provider = OverlayCellProvider::new(&tx_pool.proposed, snapshot);
    let provider = OverlayCellProvider::new(txs_provider, &cell_provider);
    resolve_transaction(tx, &mut HashSet::new(), &provider, snapshot)
}

fn resolve_tx_from_pending_and_proposed<'a>(
    tx_pool: &TxPool,
    snapshot: &Snapshot,
    txs_provider: &'a TransactionsProvider<'a>,
    tx: TransactionView,
) -> Result<ResolvedTransaction, Error> {
    let proposed_provider = OverlayCellProvider::new(&tx_pool.proposed, snapshot);
    let gap_and_proposed_provider = OverlayCellProvider::new(&tx_pool.gap, &proposed_provider);
    let pending_and_proposed_provider =
        OverlayCellProvider::new(&tx_pool.pending, &gap_and_proposed_provider);
    let provider = OverlayCellProvider::new(txs_provider, &pending_and_proposed_provider);
    resolve_transaction(tx, &mut HashSet::new(), &provider, snapshot)
}

fn verify_rtxs(
    snapshot: &Snapshot,
    txs: Vec<ResolvedTransaction>,
    txs_verify_cache: &HashMap<Byte32, CacheEntry>,
    max_tx_verify_cycles: Cycle,
) -> Result<Vec<(ResolvedTransaction, CacheEntry)>, Error> {
    let tip_header = snapshot.tip_header();
    let tip_number = tip_header.number();
    let epoch = tip_header.epoch();
    let consensus = snapshot.consensus();

    txs.into_iter()
        .map(|tx| {
            let tx_hash = tx.transaction.hash();
            if let Some(cache_entry) = txs_verify_cache.get(&tx_hash) {
                ContextualTransactionVerifier::new(
                    &tx,
                    snapshot,
                    tip_number + 1,
                    epoch,
                    tip_header.hash(),
                    consensus,
                )
                .verify()
                .map(|_| (tx, *cache_entry))
            } else {
                TransactionVerifier::new(
                    &tx,
                    snapshot,
                    tip_number + 1,
                    epoch,
                    tip_header.hash(),
                    consensus,
                    snapshot,
                )
                .verify(max_tx_verify_cycles)
                .map(|cycles| (tx, cycles))
            }
        })
        .collect::<Result<Vec<_>, _>>()
}
