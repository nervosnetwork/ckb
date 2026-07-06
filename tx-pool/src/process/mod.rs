use crate::callback::Callbacks;
use crate::component::entry::TxEntry;
use crate::component::pipeline_queue::PipelineQueue;
use crate::component::pre_check_queue::PreCheckJob;
use crate::constants::{GAP_PROPOSAL_INDEX, MALFORMED_TX_BAN_SECONDS, PROPOSED_PROPOSAL_INDEX};
use crate::error::Reject;
use crate::pool::TxPool;
use crate::service::{BlockAssemblerMessage, TxPoolService, TxVerificationResult};
use crate::try_or_return_with_snapshot;
use crate::util::{
    check_tx_fee, check_tx_fee_with_min_fee_rate, check_txid_collision, is_missing_input,
    non_contextual_verify, time_relative_verify, verify_rtx,
};
use ckb_error::{AnyError, InternalErrorKind};
use ckb_fee_estimator::FeeEstimator;
use ckb_jsonrpc_types::BlockTemplate;
use ckb_logger::Level::Trace;
use ckb_logger::{debug, error, info, log_enabled_target, trace_target, warn};
use ckb_network::PeerIndex;
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
use ckb_types::core::error::OutPointError;
use ckb_types::packed::OutPoint;
use ckb_types::{
    core::{
        BlockView, Capacity, Cycle, EstimateMode, FeeRate, HeaderView, TransactionView,
        cell::{ResolvedTransaction, resolve_transaction},
    },
    packed::{Byte32, ProposalShortId},
};
use ckb_util::LinkedHashSet;
use ckb_verification::{
    TxVerifyEnv,
    cache::{CacheEntry, Completed},
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;

mod orphan;
mod rbf;
mod reorg;

/// A list for plug target for `plug_entry` method
pub enum PlugTarget {
    /// Pending pool
    Pending,
    /// Proposed pool
    Proposed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxStatus {
    Fresh,
    Gap,
    Proposed,
}

impl TxStatus {
    fn with_env(self, header: &HeaderView) -> TxVerifyEnv {
        match self {
            TxStatus::Fresh => TxVerifyEnv::new_submit(header),
            TxStatus::Gap => TxVerifyEnv::new_proposed(header, GAP_PROPOSAL_INDEX),
            TxStatus::Proposed => TxVerifyEnv::new_proposed(header, PROPOSED_PROPOSAL_INDEX),
        }
    }
}

impl TxPoolService {
    pub(crate) async fn get_block_template(&self) -> Result<BlockTemplate, AnyError> {
        if let Some(ref block_assembler) = self.block_assembler {
            Ok(block_assembler.get_current().await)
        } else {
            Err(InternalErrorKind::Config
                .other("BlockAssembler disabled")
                .into())
        }
    }

    pub(crate) async fn fetch_tx_verify_cache(&self, tx: &TransactionView) -> Option<CacheEntry> {
        let guard = self.txs_verify_cache.read().await;
        guard.peek(&tx.witness_hash()).cloned()
    }

    fn try_submit_entry(
        &self,
        tx_pool: &mut TxPool,
        snapshot: Arc<Snapshot>,
        pre_resolve_tip: Byte32,
        entry: TxEntry,
        mut status: TxStatus,
        entry_id: ProposalShortId,
    ) -> SubmitEntryOutcome {
        let mut reject_events = Vec::new();
        let mut recovered = Vec::new();

        let mut removed_old_txs = Vec::new();
        let result = (|| -> Result<(), Reject> {
            // check_rbf must be invoked in `write` lock to avoid concurrent issues.
            let conflicts = if tx_pool.enable_rbf() {
                tx_pool.check_rbf(&snapshot, &entry)?
            } else {
                // RBF is disabled but we found conflicts, return error here
                // after_process will put this tx into conflicts_pool
                let conflicted_outpoint =
                    tx_pool.pool_map.find_conflict_outpoint(entry.transaction());
                if let Some(outpoint) = conflicted_outpoint {
                    return Err(Reject::Resolve(OutPointError::Dead(outpoint)));
                }
                HashSet::new()
            };

            // if snapshot changed by context switch we need redo time_relative verify
            let tip_hash = snapshot.tip_hash();
            if pre_resolve_tip != tip_hash {
                debug!(
                    "submit_entry {} context changed. previous:{} now:{}",
                    entry.proposal_short_id(),
                    pre_resolve_tip,
                    tip_hash
                );

                status = check_rtx(tx_pool, &snapshot, &entry.rtx)?;

                let tip_header = snapshot.tip_header();
                let tx_env = status.with_env(tip_header);
                time_relative_verify(snapshot, Arc::clone(&entry.rtx), tx_env)?;
            }

            removed_old_txs = self.process_rbf(tx_pool, &entry, &conflicts, &mut reject_events);

            // Txs whose inputs are not consumed by the new tx can be
            // recovered immediately, regardless of whether the new tx
            // ultimately succeeds.
            let mut available_inputs: HashSet<OutPoint> = HashSet::new();
            available_inputs.extend(
                removed_old_txs
                    .iter()
                    .flat_map(|removed| removed.transaction().input_pts_iter()),
            );
            for input in entry.transaction().input_pts_iter() {
                available_inputs.remove(&input);
            }
            recovered.extend(tx_pool.get_conflicted_txs_from_inputs(available_inputs.into_iter()));

            // Parents must be recovered before children so that the
            // ordered resolver can re-resolve and accept them in the
            // correct order.
            Self::sort_txs_by_dependencies(&mut recovered);

            let evicted = commit_entry_to_pool(tx_pool, status, &entry, &self.callbacks)?;

            // in a corner case, a tx with lower fee rate may be rejected immediately
            // after inserting into pool, return proper reject error here
            for evict in evicted {
                let reject = Reject::Invalidated(format!(
                    "invalidated by tx {}",
                    evict.transaction().hash()
                ));
                reject_events.push((evict, reject));
            }

            tx_pool.remove_conflict(&entry.proposal_short_id());
            tx_pool
                .limit_size(Some(&entry.proposal_short_id()), &mut reject_events)
                .map_or(Ok(()), Err)?;

            Ok(())
        })();

        // If the replacement was rejected after `process_rbf` removed the
        // old conflicting transactions, the new tx's inputs are free
        // again. Recover any txs stored in the conflict pool for those
        // inputs (in particular the old tx itself) so that a failed
        // RBF attempt cannot be used to evict in-pool transactions.
        //
        // IMPORTANT: exclude the entry transaction itself from recovery.
        // It was just rejected by RBF; re-enqueueing it would cause a
        // cycle where both the entry and the in-pool tx keep being
        // recovered and failing RBF against each other indefinitely.
        if result.is_err() {
            recovered.extend(
                tx_pool
                    .get_conflicted_txs_from_inputs(entry.transaction().input_pts_iter())
                    .into_iter()
                    .filter(|tx| tx.proposal_short_id() != entry_id),
            );
            for tx in &recovered {
                tx_pool.remove_conflict(&tx.proposal_short_id());
            }
            for old in removed_old_txs {
                tx_pool.remove_conflict(&old.proposal_short_id());
            }
        }

        (result, recovered, reject_events)
    }

    pub(crate) async fn submit_entry(
        &self,
        pre_resolve_tip: Byte32,
        entry: TxEntry,
        status: TxStatus,
    ) -> (Result<(), Reject>, Arc<Snapshot>) {
        let (conflict_inputs, early_snapshot) =
            self.find_conflict_inputs(entry.transaction()).await;

        // If a higher-fee RBF candidate appeared while this tx was waiting in
        // the verify queue, abort before replacing anything.  This prevents a
        // lower-fee candidate from front-running a higher-fee one.
        {
            if !conflict_inputs.is_empty() {
                let id = entry.proposal_short_id();
                let fee = entry.fee;
                if self
                    .rbf_candidates
                    .read()
                    .await
                    .is_superseded(&id, fee, &conflict_inputs)
                {
                    self.rbf_candidates.write().await.remove(&id);
                    return (
                        Err(Reject::RBFRejected(
                            "superseded by higher-fee in-flight candidate".to_string(),
                        )),
                        early_snapshot,
                    );
                }
            }
        }

        let entry_id = entry.proposal_short_id();

        // Separate the successful result from the collected reject events and
        // recovered txs. Reject callbacks must be dispatched and displaced txs
        // must be recovered even if the closure returns an error after
        // `process_rbf` has already removed old transactions (e.g. the
        // replacement fails the pool ancestor/size limits). Without this, a
        // remote peer can evict in-pool txs via a crafted RBF replacement that
        // is itself rejected, leaving the node with neither transaction.
        let ((result, recovered, reject_events), snapshot) = self
            .with_tx_pool_write_lock(|tx_pool, snapshot| {
                self.try_submit_entry(
                    tx_pool,
                    snapshot,
                    pre_resolve_tip.clone(),
                    entry.clone(),
                    status,
                    entry_id.clone(),
                )
            })
            .await;

        // Dispatch reject callbacks outside the write lock, regardless of
        // whether the submission itself succeeded.
        for (entry, reject) in reject_events {
            self.callbacks.call_reject(&entry, reject);
        }

        // Send recovered txs to the deferred worker after the write lock is
        // released. Use .send().await rather than try_send so that recovery
        // txs are never silently dropped under high RBF frequency.
        // Recovery is attempted even if the replacement ultimately failed,
        // because the old conflicting txs have already been removed from the
        // pool and may now be valid again.
        if !recovered.is_empty()
            && let Err(e) = self
                .deferred_sender
                .send(crate::service::DeferredTask::RecoverTxs(recovered))
                .await
        {
            warn!("failed to enqueue recovered txs for re-processing: {}", e);
        }

        // The RBF candidate has either been accepted or definitively rejected;
        // remove it from the in-flight fee-ordering gate.
        self.rbf_candidates.write().await.remove(&entry_id);

        (result, snapshot)
    }

    pub(crate) async fn notify_block_assembler(&self, status: TxStatus) {
        if self.should_notify_block_assembler() {
            let message = match status {
                TxStatus::Fresh => Some(BlockAssemblerMessage::Pending),
                TxStatus::Proposed => Some(BlockAssemblerMessage::Proposed),
                _ => None,
            };

            if let Some(message) = message
                && self.block_assembler_sender.send(message).await.is_err()
            {
                error!("block_assembler receiver dropped");
            }
        }
    }

    pub(crate) async fn verify_queue_contains(&self, tx: &TransactionView) -> bool {
        let queue = self.verify_queue.read().await;
        queue.contains_key(&tx.proposal_short_id())
    }

    pub(crate) async fn with_tx_pool_read_lock<U, F: FnMut(&TxPool, Arc<Snapshot>) -> U>(
        &self,
        mut f: F,
    ) -> (U, Arc<Snapshot>) {
        let tx_pool = self.tx_pool.read().await;
        let snapshot = tx_pool.cloned_snapshot();

        let ret = f(&tx_pool, Arc::clone(&snapshot));
        (ret, snapshot)
    }

    /// Find the transaction inputs that are currently consumed by in-pool txs.
    /// These are the "conflict inputs" that matter for RBF ordering.
    pub(crate) async fn find_conflict_inputs(
        &self,
        tx: &TransactionView,
    ) -> (Vec<OutPoint>, Arc<Snapshot>) {
        self.with_tx_pool_read_lock(|tx_pool, _snapshot| {
            tx.input_pts_iter()
                .filter(|out_point| tx_pool.pool_map.edges.get_input_ref(out_point).is_some())
                .collect()
        })
        .await
    }

    pub(crate) async fn with_tx_pool_write_lock<U, F: FnMut(&mut TxPool, Arc<Snapshot>) -> U>(
        &self,
        mut f: F,
    ) -> (U, Arc<Snapshot>) {
        let mut tx_pool = self.tx_pool.write().await;
        let snapshot = tx_pool.cloned_snapshot();

        let ret = f(&mut tx_pool, Arc::clone(&snapshot));
        (ret, snapshot)
    }

    pub(crate) async fn pre_check(
        &self,
        tx: &TransactionView,
        tx_size: usize,
    ) -> (Result<PreCheckedTx, Reject>, Arc<Snapshot>) {
        // Fast path: for transactions whose inputs and cell deps all come from the
        // chain (not from any tx currently in the pool), we can resolve and compute
        // the fee without holding the tx_pool read lock.  We only take the lock
        // briefly to check for txid collisions.
        let (collision, snapshot) = self
            .with_tx_pool_read_lock(|tx_pool, _snapshot| check_txid_collision(tx_pool, tx).err())
            .await;
        if let Some(reject) = collision {
            return (Err(reject), snapshot);
        }

        let short_id = tx.proposal_short_id();
        let mut seen_inputs = HashSet::new();
        match resolve_transaction(
            tx.clone(),
            &mut seen_inputs,
            snapshot.as_ref(),
            snapshot.as_ref(),
        ) {
            Ok(rtx) => {
                let rtx = Arc::new(rtx);
                let fee = match check_tx_fee_with_min_fee_rate(
                    &snapshot,
                    &rtx,
                    tx_size,
                    self.tx_pool_config.min_fee_rate,
                ) {
                    Ok(fee) => fee,
                    Err(reject) => return (Err(reject), snapshot),
                };
                let status = get_tx_status(&snapshot, &short_id);
                (
                    Ok(PreCheckedTx {
                        pre_resolve_tip: snapshot.tip_hash(),
                        rtx,
                        status,
                        fee,
                        tx_size,
                    }),
                    snapshot,
                )
            }
            Err(OutPointError::Unknown(_)) => {
                // At least one input/cell dep is not in the chain snapshot.  It may
                // be an output of a tx currently in the pool, so fall back to the
                // locked path which can resolve through the pool.
                self.pre_check_with_pool_lock(tx, tx_size).await
            }
            Err(err) => (Err(Reject::Resolve(err)), snapshot),
        }
    }

    async fn pre_check_with_pool_lock(
        &self,
        tx: &TransactionView,
        tx_size: usize,
    ) -> (Result<PreCheckedTx, Reject>, Arc<Snapshot>) {
        let (ret, snapshot) = self
            .with_tx_pool_read_lock(|tx_pool, snapshot| {
                let tip_hash = snapshot.tip_hash();

                // Same txid means exactly the same transaction, including inputs, outputs, witnesses, etc.
                // It's also not possible for RBF, reject it directly
                check_txid_collision(tx_pool, tx)?;

                // Try normal path first, if double-spending check success we don't need RBF check
                // this make sure RBF won't introduce extra performance cost for hot path
                let res = resolve_tx(tx_pool, &snapshot, tx.clone(), false);
                match res {
                    Ok((rtx, status)) => {
                        let fee = check_tx_fee(tx_pool, &snapshot, &rtx, tx_size)?;
                        Ok(PreCheckedTx {
                            pre_resolve_tip: tip_hash,
                            rtx,
                            status,
                            fee,
                            tx_size,
                        })
                    }
                    Err(Reject::Resolve(OutPointError::Dead(out))) => {
                        let (rtx, status) = resolve_tx(tx_pool, &snapshot, tx.clone(), true)?;
                        let fee = check_tx_fee(tx_pool, &snapshot, &rtx, tx_size)?;
                        let conflicts = tx_pool.pool_map.find_conflict_outpoint(tx);
                        if conflicts.is_none() {
                            // this mean one input's outpoint is dead, but there is no direct conflicted tx in tx_pool
                            // we should reject it directly and don't need to put it into conflicts pool
                            error!(
                                "{} is resolved as Dead, but there is no direct conflicted tx",
                                rtx.transaction.proposal_short_id()
                            );
                            return Err(Reject::Resolve(OutPointError::Dead(out)));
                        }
                        // we also return Ok here, so that the entry will be continue to be verified before submit
                        // we only want to put it into conflicts pool after the verification stage passed
                        // then we will double-check conflicts txs in `submit_entry`

                        Ok(PreCheckedTx {
                            pre_resolve_tip: tip_hash,
                            rtx,
                            status,
                            fee,
                            tx_size,
                        })
                    }
                    Err(err) => Err(err),
                }
            })
            .await;
        (ret, snapshot)
    }

    pub(crate) async fn non_contextual_verify(
        &self,
        tx: &TransactionView,
        remote: Option<(Cycle, PeerIndex)>,
    ) -> Result<(), Reject> {
        if let Err(reject) = non_contextual_verify(&self.consensus, tx) {
            if reject.is_malformed_tx()
                && let Some(remote) = remote
            {
                self.ban_malformed(remote.1, format!("reject {reject}"))
                    .await;
            }
            return Err(reject);
        }
        Ok(())
    }

    /// Common pre-flight checks shared by all transaction submission paths.
    ///
    /// Runs non-contextual verification and rejects duplicates that are already
    /// in the verify queue or the orphan pool.
    pub(crate) async fn check_tx_basic_validity(
        &self,
        tx: &TransactionView,
        remote: Option<(Cycle, PeerIndex)>,
    ) -> Result<(), Reject> {
        self.non_contextual_verify(tx, remote).await?;

        if self.verify_queue_contains(tx).await {
            return Err(Reject::Duplicated(tx.hash()));
        }

        if self.orphan_contains(tx).await {
            debug!("reject tx {} already in orphan pool", tx.hash());
            return Err(Reject::Duplicated(tx.hash()));
        }

        Ok(())
    }

    async fn classify_and_enqueue_with_full_reject_notification(
        &self,
        tx: TransactionView,
        is_proposal_tx: bool,
        remote: Option<(Cycle, PeerIndex)>,
    ) -> Result<bool, Reject> {
        let ret = self
            .classify_and_enqueue_tx_spawn(tx.clone(), is_proposal_tx, remote)
            .await;

        if matches!(ret, Err(Reject::Full(_))) {
            self.send_result_to_relayer(TxVerificationResult::Reject { tx_hash: tx.hash() });
        }

        ret
    }

    pub(crate) async fn submit_remote_tx(
        &self,
        tx: TransactionView,
        declared_cycles: Cycle,
        peer: PeerIndex,
    ) -> Result<bool, Reject> {
        let remote = Some((declared_cycles, peer));
        self.check_tx_basic_validity(&tx, remote).await?;
        self.classify_and_enqueue_with_full_reject_notification(tx, false, remote)
            .await
    }

    pub(crate) async fn notify_tx(&self, tx: TransactionView) -> Result<bool, Reject> {
        self.check_tx_basic_validity(&tx, None).await?;
        self.classify_and_enqueue_with_full_reject_notification(tx, true, None)
            .await
    }

    pub(crate) async fn test_accept_tx(&self, tx: TransactionView) -> Result<Completed, Reject> {
        self.check_tx_basic_validity(&tx, None).await?;
        self.test_accept_tx_core(tx.clone()).await
    }

    pub(crate) async fn process_tx(
        &self,
        tx: TransactionView,
        remote: Option<(Cycle, PeerIndex)>,
    ) -> Result<Completed, Reject> {
        self.check_tx_basic_validity(&tx, remote).await?;

        let (ret, snapshot) = self
            .process_tx_direct(tx.clone(), remote.map(|r| r.0), None)
            .await
            .expect("process_tx_direct always returns Some");
        self.after_process(tx, remote, &snapshot, &ret).await;
        ret
    }

    pub(crate) fn put_recent_reject(&self, tx_hash: &Byte32, reject: &Reject) {
        if let Some(ref recent_reject) = self.recent_reject
            && let Err(e) = recent_reject.put(tx_hash, reject.clone())
        {
            error!(
                "Failed to record recent_reject {} {} {}",
                tx_hash, reject, e
            );
        }
    }

    /// Notify the ordered resolver if there are jobs waiting.
    ///
    /// Must be called after a transaction is removed from the verify queue or
    /// the in-pool set: the removed tx may have had descendants waiting in the
    /// ordered resolve queue, and waking the resolver lets them be retried
    /// (and rejected if the parent is gone) promptly.
    async fn wake_ordered_resolver_if_needed(&self) {
        let ordered = self.ordered_resolve_queue.read().await;
        if !ordered.is_empty() {
            ordered.subscribe().notify_one();
        }
    }

    pub(crate) async fn remove_tx(&self, tx_hash: Byte32) -> bool {
        let id = ProposalShortId::from_tx_hash(&tx_hash);
        if self.pre_check_queue.remove_by_id(&id).is_some() {
            return true;
        }
        {
            let mut queue = self.ordered_resolve_queue.write().await;
            if queue.remove_tx(&id).is_some() {
                return true;
            }
        }
        {
            let mut queue = self.verify_queue.write().await;
            if queue.remove_tx(&id).is_some() {
                // Release verify_queue write lock before acquiring other locks
                // to respect the documented lock ordering convention.
                drop(queue);
                // The removed tx may have had descendants waiting in the
                // ordered resolve queue. Wake the resolver so they can be
                // retried (and rejected if the parent is gone) promptly.
                self.rbf_candidates.write().await.remove(&id);
                self.wake_ordered_resolver_if_needed().await;
                return true;
            }
        }
        {
            let mut orphan = self.orphan.write().await;
            if orphan.remove_orphan_tx(&id).is_some() {
                return true;
            }
        }
        let removed = {
            let mut tx_pool = self.tx_pool.write().await;
            tx_pool.remove_tx(&id)
        };
        if removed {
            self.wake_ordered_resolver_if_needed().await;
        }
        removed
    }

    pub(crate) async fn after_process(
        &self,
        tx: TransactionView,
        remote: Option<(Cycle, PeerIndex)>,
        _snapshot: &Snapshot,
        ret: &Result<Completed, Reject>,
    ) {
        let tx_hash = tx.hash();

        // log tx verification result for monitor node
        if log_enabled_target!("ckb_tx_monitor", Trace)
            && let Ok(c) = ret
        {
            trace_target!(
                "ckb_tx_monitor",
                r#"{{"tx_hash":"{:#x}","cycles":{}}}"#,
                tx_hash,
                c.cycles
            );
        }

        if matches!(
            ret,
            Err(Reject::RBFRejected(..) | Reject::Resolve(OutPointError::Dead(_)))
        ) {
            let mut tx_pool = self.tx_pool.write().await;
            if tx_pool.pool_map.find_conflict_outpoint(&tx).is_some() {
                tx_pool.record_conflict(tx.clone());
            }
        }

        match remote {
            Some((declared_cycle, peer)) => {
                self.after_process_remote(tx, declared_cycle, peer, ret)
                    .await;
            }
            None => {
                self.after_process_local(tx, tx_hash, ret).await;
            }
        }
    }

    async fn after_process_remote(
        &self,
        tx: TransactionView,
        declared_cycle: Cycle,
        peer: PeerIndex,
        ret: &Result<Completed, Reject>,
    ) {
        let tx_hash = tx.hash();
        match ret {
            Ok(_) => {
                debug!(
                    "after_process remote send_result_to_relayer {} {}",
                    tx_hash, peer
                );
                self.handle_verify_success(&tx, Some(peer)).await;
            }
            Err(reject) => {
                debug!(
                    "after_process {} {} remote reject: {} ",
                    tx_hash, peer, reject
                );
                if is_missing_input(reject) {
                    let parents = tx.unique_parents();
                    self.handle_missing_input_orphan(tx, peer, declared_cycle, parents)
                        .await;
                } else {
                    self.handle_remote_reject(&tx_hash, reject, peer).await;
                }
            }
        }
    }

    async fn after_process_local(
        &self,
        tx: TransactionView,
        tx_hash: Byte32,
        ret: &Result<Completed, Reject>,
    ) {
        match ret {
            Ok(_) | Err(Reject::Duplicated(_)) => {
                if matches!(ret, Err(Reject::Duplicated(_))) {
                    debug!("after_process {} duplicated", tx_hash);
                } else {
                    debug!("after_process local send_result_to_relayer {}", tx_hash);
                }
                // Re-broadcast tx when it's duplicated and submitted
                // through local rpc, or notify on fresh success.
                self.handle_verify_success(&tx, None).await;
            }
            Err(reject) => {
                debug!("after_process {} reject: {} ", tx_hash, reject);
                if reject.should_recorded() {
                    self.put_recent_reject(&tx_hash, reject);
                }
            }
        }
    }

    /// Common success handler: relay the result and trigger orphan processing.
    ///
    /// Box::pin is required because after_process and process_orphan_tx are
    /// mutually recursive async fns; without boxing the compiler cannot prove
    /// the resulting future has a finite size.
    async fn handle_verify_success(&self, tx: &TransactionView, original_peer: Option<PeerIndex>) {
        self.send_result_to_relayer(TxVerificationResult::Ok {
            original_peer,
            tx_hash: tx.hash(),
        });
        Box::pin(self.process_orphan_tx(tx)).await;
    }

    /// Post-processing for a rejected remote transaction: ban the peer if the
    /// tx is malformed, relay the rejection if allowed, and record it in the
    /// recent-reject database if applicable.
    ///
    /// This is the single source of truth for the "remote error triple" used
    /// by both [`Self::after_process`] and [`Self::process_orphan_tx`].
    pub(crate) async fn handle_remote_reject(
        &self,
        tx_hash: &Byte32,
        reject: &Reject,
        peer: PeerIndex,
    ) {
        if reject.is_malformed_tx() {
            self.ban_malformed(peer, format!("reject {reject}")).await;
        }
        if reject.is_allowed_relay() {
            self.send_result_to_relayer(TxVerificationResult::Reject {
                tx_hash: tx_hash.clone(),
            });
        }
        if reject.should_recorded() {
            self.put_recent_reject(tx_hash, reject);
        }
    }

    pub(crate) fn send_result_to_relayer(&self, result: TxVerificationResult) {
        if let Err(e) = self.tx_relay_sender.send(result) {
            error!("tx-pool tx_relay_sender internal error {}", e);
        }
    }

    async fn ban_malformed(&self, peer: PeerIndex, reason: String) {
        const DEFAULT_BAN_TIME: Duration = Duration::from_secs(MALFORMED_TX_BAN_SECONDS);

        #[cfg(feature = "with_sentry")]
        use sentry::{Level, capture_message, with_scope};

        #[cfg(feature = "with_sentry")]
        with_scope(
            |scope| scope.set_fingerprint(Some(&["ckb-tx-pool", "receive-invalid-remote-tx"])),
            || {
                capture_message(
                    &format!(
                        "Ban peer {} for {} seconds, reason: \
                        {}",
                        peer,
                        DEFAULT_BAN_TIME.as_secs(),
                        reason
                    ),
                    Level::Info,
                )
            },
        );
        self.network.ban_peer(peer, DEFAULT_BAN_TIME, reason);
        self.ordered_resolve_queue
            .write()
            .await
            .remove_txs_by_peer(&peer);
        let removed_ids = self.verify_queue.write().await.remove_txs_by_peer(&peer);
        // Remove orphan txs from the banned peer so they are not re-processed
        // after the ban.
        self.orphan.write().await.remove_by_peer(peer);
        self.pre_check_queue.remove_by_peer(&peer);
        let mut rbf = self.rbf_candidates.write().await;
        for id in removed_ids {
            rbf.remove(&id);
        }
    }

    pub(crate) async fn process_tx_direct(
        &self,
        tx: TransactionView,
        declared_cycles: Option<Cycle>,
        command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    ) -> Option<(Result<Completed, Reject>, Arc<Snapshot>)> {
        let tx_size = tx.data().serialized_size_in_block();
        let (ret, snapshot) = self.pre_check(&tx, tx_size).await;

        let PreCheckedTx {
            pre_resolve_tip,
            rtx,
            status,
            fee,
            tx_size,
        } = try_or_return_with_snapshot!(ret, snapshot);

        self.verify_and_submit_core(
            VerifyAndSubmitInput {
                tx,
                rtx,
                status,
                fee,
                tx_size,
                pre_resolve_tip,
                snapshot,
                declared_cycles,
            },
            command_rx,
        )
        .await
    }

    /// Verify and submit a transaction whose inputs have already been resolved.
    ///
    /// This is the second stage of the tx-pool pipeline: the resolver has
    /// already produced a [`ResolvedTx`], and this function runs the CPU-heavy
    /// contextual verification and the final write-locked submit.
    pub(crate) async fn verify_and_submit_tx(
        &self,
        resolved: crate::resolved_tx::ResolvedTx,
        command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    ) -> Option<(Result<Completed, Reject>, Arc<Snapshot>)> {
        let crate::resolved_tx::ResolvedTx {
            tx,
            rtx,
            status,
            fee,
            tx_size,
            pre_resolve_tip,
            snapshot,
            remote,
            is_proposal_tx: _,
        } = resolved;

        let declared_cycles = remote.map(|(cycles, _)| cycles);

        self.verify_and_submit_core(
            VerifyAndSubmitInput {
                tx,
                rtx,
                status,
                fee,
                tx_size,
                pre_resolve_tip,
                snapshot,
                declared_cycles,
            },
            command_rx,
        )
        .await
    }

    /// Shared core: verify a resolved transaction and submit it to the pool.
    ///
    /// Both `process_tx_direct` (reorg recovery / local RPC path) and
    /// `verify_and_submit_tx` (pipeline verify path) converge here after the
    /// resolve step.
    async fn verify_and_submit_core(
        &self,
        input: VerifyAndSubmitInput,
        command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    ) -> Option<(Result<Completed, Reject>, Arc<Snapshot>)> {
        let VerifyAndSubmitInput {
            tx,
            rtx,
            status,
            fee,
            tx_size,
            pre_resolve_tip,
            snapshot,
            declared_cycles,
        } = input;
        let wtx_hash = tx.witness_hash();
        let instant = Instant::now();
        let is_sync_process = command_rx.is_none();

        let verify_cache = self.fetch_tx_verify_cache(&tx).await;
        let max_cycles = declared_cycles.unwrap_or_else(|| self.consensus.max_block_cycles());
        let tip_header = snapshot.tip_header();
        let tx_env = Arc::new(status.with_env(tip_header));

        let verified_ret = verify_rtx(
            Arc::clone(&snapshot),
            Arc::clone(&rtx),
            tx_env,
            &verify_cache,
            max_cycles,
            command_rx,
        )
        .await;

        let verified = try_or_return_with_snapshot!(verified_ret, snapshot);

        if let Some(declared) = declared_cycles
            && declared != verified.cycles
        {
            info!(
                "declared cycles not match verified cycles, declared: {}, verified: {}, tx_hash: {}",
                declared,
                verified.cycles,
                tx.hash()
            );
            return Some((
                Err(Reject::DeclaredWrongCycles(declared, verified.cycles)),
                snapshot,
            ));
        }

        let entry = TxEntry::new(rtx, verified.cycles, fee, tx_size);

        let (ret, submit_snapshot) = self.submit_entry(pre_resolve_tip, entry, status).await;
        try_or_return_with_snapshot!(ret, submit_snapshot);

        self.notify_block_assembler(status).await;

        // A newly submitted transaction may resolve dependent transactions that
        // are waiting in the ordered resolve queue (e.g. children of a parent
        // that was just re-added after a reorg). Wake the ordered resolver so
        // those children can be retried promptly.
        let queue = self.ordered_resolve_queue.read().await;
        if !queue.is_empty() {
            queue.subscribe().notify_one();
        }

        if verify_cache.is_none() {
            // Defer cache update to the background worker instead of
            // spawning a fire-and-forget task.
            if let Err(e) =
                self.deferred_sender
                    .try_send(crate::service::DeferredTask::CacheUpdate {
                        wtx_hash: wtx_hash.clone(),
                        verified,
                    })
            {
                warn!(
                    "failed to enqueue verify cache update for {}: {}",
                    wtx_hash, e
                );
            }
        }

        if let Some(metrics) = ckb_metrics::handle() {
            let elapsed = instant.elapsed().as_secs_f64();
            if is_sync_process {
                metrics.ckb_tx_pool_sync_process.observe(elapsed);
            } else {
                metrics.ckb_tx_pool_async_process.observe(elapsed);
            }
        }

        Some((Ok(verified), submit_snapshot))
    }

    pub(crate) async fn test_accept_tx_core(
        &self,
        tx: TransactionView,
    ) -> Result<Completed, Reject> {
        let tx_size = tx.data().serialized_size_in_block();
        let (pre_check_ret, snapshot) = self.pre_check(&tx, tx_size).await;

        let PreCheckedTx { rtx, status, .. } = pre_check_ret?;

        // skip check the delay window

        let verify_cache = self.fetch_tx_verify_cache(&tx).await;
        let max_cycles = self.consensus.max_block_cycles();
        let tip_header = snapshot.tip_header();
        let tx_env = Arc::new(status.with_env(tip_header));

        verify_rtx(
            Arc::clone(&snapshot),
            Arc::clone(&rtx),
            tx_env,
            &verify_cache,
            max_cycles,
            None,
        )
        .await
    }

    /// Topologically sort transactions so that parents are placed before their
    /// children. This is required when re-adding detached transactions into the
    /// pipeline: a child must not be classified before its parent has had a
    /// chance to enter the in-flight pipeline, otherwise it will be treated as a
    /// local orphan and have to wait for a retry.
    pub(crate) fn sort_txs_by_dependencies(txs: &mut Vec<TransactionView>) {
        if txs.len() <= 1 {
            return;
        }

        let mut output_to_index: HashMap<OutPoint, usize> =
            HashMap::with_capacity(txs.len().saturating_mul(2));
        for (i, tx) in txs.iter().enumerate() {
            let tx_hash = tx.hash();
            for idx in 0..tx.outputs().len() {
                output_to_index.insert(OutPoint::new(tx_hash.clone(), idx as u32), i);
            }
        }

        let mut in_degree = vec![0usize; txs.len()];
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); txs.len()];
        for (i, tx) in txs.iter().enumerate() {
            for input in tx.input_pts_iter() {
                if let Some(&parent) = output_to_index.get(&input)
                    && parent != i
                {
                    in_degree[i] += 1;
                    children[parent].push(i);
                }
            }
            for dep in tx.cell_deps_iter() {
                let out_point = dep.out_point();
                if let Some(&parent) = output_to_index.get(&out_point)
                    && parent != i
                {
                    in_degree[i] += 1;
                    children[parent].push(i);
                }
            }
        }

        let mut ready: VecDeque<usize> = (0..txs.len()).filter(|&i| in_degree[i] == 0).collect();
        let mut sorted = Vec::with_capacity(txs.len());
        while let Some(i) = ready.pop_front() {
            sorted.push(i);
            for &child in &children[i] {
                in_degree[child] -= 1;
                if in_degree[child] == 0 {
                    ready.push_back(child);
                }
            }
        }

        if sorted.len() != txs.len() {
            // A cycle should never happen in valid detached blocks, but if it
            // does we keep the original order rather than losing transactions.
            return;
        }

        let mut remaining: Vec<Option<TransactionView>> = txs.drain(..).map(Some).collect();
        txs.extend(
            sorted
                .into_iter()
                .map(|i| remaining[i].take().expect("index valid")),
        );
    }

    pub(crate) async fn update_tx_pool_for_reorg(
        &self,
        detached_blocks: VecDeque<BlockView>,
        attached_blocks: VecDeque<BlockView>,
        detached_proposal_id: HashSet<ProposalShortId>,
        snapshot: Arc<Snapshot>,
    ) {
        let mine_mode = self.block_assembler.is_some();
        let mut detached = LinkedHashSet::default();
        let mut attached = LinkedHashSet::default();

        let detached_headers: HashSet<Byte32> = detached_blocks
            .iter()
            .map(|blk| blk.header().hash())
            .collect();

        for blk in detached_blocks {
            detached.extend(blk.transactions().into_iter().skip(1))
        }

        for blk in attached_blocks {
            self.fee_estimator.commit_block(&blk);
            attached.extend(blk.transactions().into_iter().skip(1));
        }
        let mut retain: Vec<TransactionView> = detached.difference(&attached).cloned().collect();

        let reject_events;
        {
            // This closure is used to limit the lifetime of mutable tx_pool.
            let mut tx_pool = self.tx_pool.write().await;

            reject_events = reorg::update_tx_pool_for_reorg(
                &mut tx_pool,
                &attached,
                &detached_headers,
                detached_proposal_id,
                snapshot,
                &self.callbacks,
                mine_mode,
            );
        }

        // Dispatch reject callbacks outside the write lock
        for (entry, reject) in reject_events {
            self.callbacks.call_reject(&entry, reject);
        }

        self.remove_orphan_txs_by_attach(&attached).await;
        {
            let mut queue = self.verify_queue.write().await;
            queue.remove_txs(attached.iter().map(|tx| tx.proposal_short_id()));
        }

        // Recover detached transactions through the direct per-tx entry point.
        // Dependent transactions must be processed after their parents have
        // already been submitted to the pool; a topological sort guarantees this
        // ordering. Using `process_tx_direct` here keeps the recovery logic simple
        // and correct while still releasing the tx-pool write lock between
        // transactions.
        {
            Self::sort_txs_by_dependencies(&mut retain);
            let mut chunk_rx = self.chunk_rx.clone();
            for tx in retain {
                if let Some((ret, snapshot)) = self
                    .process_tx_direct(tx.clone(), None, Some(&mut chunk_rx))
                    .await
                    && let Err(ref reject) = ret
                {
                    debug!("reorg re-add failed: {}", reject);
                    self.after_process(tx, None, &snapshot, &ret).await;
                } else {
                    // The detached tx is now back in the pool. Wake up any
                    // orphans that depend on it (including via cell dep).
                    self.process_orphan_tx(&tx).await;
                }
            }
        }
    }

    /// Check if a transaction depends on any in-flight pipeline transaction
    /// (ordered resolve queue, verify queue, or pre-check queue).
    pub(crate) async fn depends_on_pipeline(&self, tx: &TransactionView) -> bool {
        let ordered = self.ordered_resolve_queue.read().await;
        if ordered.depends_on(tx) {
            return true;
        }
        drop(ordered);
        let verify_queue = self.verify_queue.read().await;
        if verify_queue.depends_on(tx) {
            return true;
        }
        drop(verify_queue);
        self.pre_check_queue.depends_on(tx)
    }

    /// Check if a transaction depends on any in-flight pipeline transaction.
    /// If so, route it to the ordered resolve queue.
    async fn check_and_route_dependent(
        &self,
        tx: &TransactionView,
        is_proposal_tx: bool,
        remote: Option<(Cycle, PeerIndex)>,
    ) -> Result<RouteDecision, Reject> {
        let id = tx.proposal_short_id();

        if self.depends_on_pipeline(tx).await {
            let mut ordered = self.ordered_resolve_queue.write().await;
            if ordered.contains_key(&id) {
                return Ok(RouteDecision::Duplicate);
            }
            return ordered
                .add_tx(crate::resolved_tx::ResolveJob {
                    tx: tx.clone(),
                    remote,
                    is_proposal_tx,
                    attempts: 0,
                })
                .map(|_| RouteDecision::Enqueued);
        }

        Ok(RouteDecision::Independent)
    }

    /// Classify a transaction and enqueue it for verification or ordered resolve.
    ///
    /// This is the core entry-point classifier.  It checks whether the tx
    /// depends on an in-flight pipeline tx, runs `pre_check`, and routes the
    /// result to the appropriate queue.
    pub(crate) async fn classify_and_enqueue_tx(
        &self,
        tx: TransactionView,
        is_proposal_tx: bool,
        remote: Option<(Cycle, PeerIndex)>,
    ) -> Result<bool, Reject> {
        let id = tx.proposal_short_id();

        match self
            .check_and_route_dependent(&tx, is_proposal_tx, remote)
            .await?
        {
            RouteDecision::Independent => {}
            RouteDecision::Enqueued => return Ok(true),
            RouteDecision::Duplicate => return Ok(false),
        }

        // Run pre_check once at the entry point.
        let tx_size = tx.data().serialized_size_in_block();
        let (pre_check_ret, snapshot) = self.pre_check(&tx, tx_size).await;

        match pre_check_ret {
            Ok(PreCheckedTx {
                pre_resolve_tip,
                rtx,
                status,
                fee,
                tx_size,
            }) => {
                // For RBF replacements, register the candidate before it enters
                // the verify queue so lower-fee candidates can be rejected while
                // a higher-fee candidate is already in flight.
                let conflict_inputs = if remote.is_some() {
                    self.find_conflict_inputs(&tx).await.0
                } else {
                    Vec::new()
                };
                let rbf_registered = !conflict_inputs.is_empty();
                if rbf_registered {
                    let mut rbf = self.rbf_candidates.write().await;
                    match rbf.register(id.clone(), fee, &conflict_inputs) {
                        Ok(displaced_ids) => {
                            // Higher-fee candidate(s) displaced lower-fee one(s)
                            // still waiting in the verify queue.  Drop all
                            // displaced candidates so only the highest-fee tx
                            // reaches submit_entry.
                            if !displaced_ids.is_empty() {
                                drop(rbf);
                                let mut verify_queue = self.verify_queue.write().await;
                                for displaced_id in &displaced_ids {
                                    verify_queue.remove_tx(displaced_id);
                                }
                            }
                        }
                        Err(reason) => {
                            drop(rbf);
                            let reject = Reject::RBFRejected(reason);
                            self.after_process(tx, remote, &snapshot, &Err(reject.clone()))
                                .await;
                            return Err(reject);
                        }
                    }
                }

                let resolved = crate::resolved_tx::ResolvedTx {
                    tx: tx.clone(),
                    rtx,
                    status,
                    fee,
                    tx_size,
                    pre_resolve_tip,
                    snapshot: Arc::clone(&snapshot),
                    remote,
                    is_proposal_tx,
                };
                let reject = {
                    let mut verify_queue = self.verify_queue.write().await;
                    match verify_queue.add_tx(resolved) {
                        Ok(added) => return Ok(added),
                        Err(reject) => reject,
                    }
                };
                // The verify queue rejected the tx (e.g., it is full). Clean up
                // the RBF registration so the input is not blocked forever.
                if rbf_registered {
                    self.rbf_candidates.write().await.remove(&id);
                }
                self.after_process(tx, remote, &snapshot, &Err(reject.clone()))
                    .await;
                Err(reject)
            }
            Err(reject) if crate::util::is_missing_input(&reject) => {
                let mut ordered = self.ordered_resolve_queue.write().await;
                if ordered.contains_key(&id) {
                    return Ok(false);
                }
                ordered.add_tx(crate::resolved_tx::ResolveJob {
                    tx,
                    remote,
                    is_proposal_tx,
                    attempts: 0,
                })
            }
            Err(reject) => {
                self.after_process(tx, remote, &snapshot, &Err(reject.clone()))
                    .await;
                Err(reject)
            }
        }
    }

    /// Entry-point classifier used by remote/local submission.
    ///
    /// Dependent transactions (those that spend an output currently in flight)
    /// are handled synchronously so they land in the ordered resolve queue in
    /// arrival order and errors propagate to the caller.  Independent
    /// transactions are sent to a fixed-size worker pool so that the expensive
    /// `pre_check` work does not serialize inside the service actor.
    pub(crate) async fn classify_and_enqueue_tx_spawn(
        &self,
        tx: TransactionView,
        is_proposal_tx: bool,
        remote: Option<(Cycle, PeerIndex)>,
    ) -> Result<bool, Reject> {
        match self
            .check_and_route_dependent(&tx, is_proposal_tx, remote)
            .await?
        {
            RouteDecision::Independent => {}
            RouteDecision::Enqueued => return Ok(true),
            RouteDecision::Duplicate => return Ok(false),
        }

        let job = PreCheckJob {
            tx,
            is_proposal_tx,
            remote,
        };
        self.pre_check_queue.push(job)?;

        // Returning Ok(true) only means the tx was accepted into the pipeline;
        // actual classification/verification happens in the worker pool.
        Ok(true)
    }

    /// Clear all pipeline queues without touching the already-accepted pool.
    pub(crate) async fn clear_pipeline_queues(&self) {
        self.ordered_resolve_queue.write().await.clear();
        self.verify_queue.write().await.clear();
        self.orphan.write().await.clear();
        self.pre_check_queue.clear();
        self.rbf_candidates.write().await.clear();
    }

    pub(crate) async fn clear_pool(&mut self, new_snapshot: Arc<Snapshot>) {
        {
            let mut tx_pool = self.tx_pool.write().await;
            tx_pool.clear(Arc::clone(&new_snapshot));
        }
        self.clear_pipeline_queues().await;
        // reset block_assembler
        if self
            .block_assembler_sender
            .send(BlockAssemblerMessage::Reset(new_snapshot))
            .await
            .is_err()
        {
            error!("block_assembler receiver dropped");
        }
    }

    pub(crate) async fn save_pool(&self) {
        let mut tx_pool = self.tx_pool.write().await;
        if let Err(err) = tx_pool.save_into_file() {
            error!("failed to save pool, error: {:?}", err)
        } else {
            info!("TxPool saved successfully")
        }
    }

    pub(crate) async fn update_ibd_state(&self, in_ibd: bool) {
        self.fee_estimator.update_ibd_state(in_ibd);
    }

    pub(crate) async fn estimate_fee_rate(
        &self,
        estimate_mode: EstimateMode,
        enable_fallback: bool,
    ) -> Result<FeeRate, AnyError> {
        let all_entry_info = self.tx_pool.read().await.get_all_entry_info();
        match self
            .fee_estimator
            .estimate_fee_rate(estimate_mode, all_entry_info)
        {
            Ok(fee_rate) => Ok(fee_rate),
            Err(err) => {
                if enable_fallback {
                    let target_blocks =
                        FeeEstimator::target_blocks_for_estimate_mode(estimate_mode);
                    self.tx_pool
                        .read()
                        .await
                        .estimate_fee_rate(target_blocks)
                        .map_err(Into::into)
                } else {
                    Err(err.into())
                }
            }
        }
    }
}

pub(crate) struct PreCheckedTx {
    /// Tip hash at the time the transaction was pre-checked.
    pub(crate) pre_resolve_tip: Byte32,
    /// Fully resolved transaction.
    pub(crate) rtx: Arc<ResolvedTransaction>,
    /// Current status (fresh / gap / proposed) relative to the proposal window.
    pub(crate) status: TxStatus,
    /// Transaction fee.
    pub(crate) fee: Capacity,
    /// Transaction size in bytes as serialized in a block.
    pub(crate) tx_size: usize,
}

/// Outcome of [`TxPoolService::try_submit_entry`].
pub(crate) type SubmitEntryOutcome = (
    Result<(), Reject>,
    Vec<TransactionView>,
    Vec<(TxEntry, Reject)>,
);

/// Input bundle for [`TxPoolService::verify_and_submit_core`].
pub(crate) struct VerifyAndSubmitInput {
    pub(crate) tx: TransactionView,
    pub(crate) rtx: Arc<ResolvedTransaction>,
    pub(crate) status: TxStatus,
    pub(crate) fee: Capacity,
    pub(crate) tx_size: usize,
    pub(crate) pre_resolve_tip: Byte32,
    pub(crate) snapshot: Arc<Snapshot>,
    pub(crate) declared_cycles: Option<Cycle>,
}

/// Decision made by [`TxPoolService::check_and_route_dependent`].
pub(crate) enum RouteDecision {
    /// Transaction is independent; caller should proceed with pre_check.
    Independent,
    /// Transaction depends on an in-flight tx and was enqueued for ordered resolve.
    Enqueued,
    /// Transaction depends on an in-flight tx but is a duplicate of one already queued.
    Duplicate,
}

type ResolveResult = Result<(Arc<ResolvedTransaction>, TxStatus), Reject>;

fn get_tx_status(snapshot: &Snapshot, short_id: &ProposalShortId) -> TxStatus {
    if snapshot.proposals().contains_proposed(short_id) {
        TxStatus::Proposed
    } else if snapshot.proposals().contains_gap(short_id) {
        TxStatus::Gap
    } else {
        TxStatus::Fresh
    }
}

fn check_rtx(
    tx_pool: &TxPool,
    snapshot: &Snapshot,
    rtx: &ResolvedTransaction,
) -> Result<TxStatus, Reject> {
    let short_id = rtx.transaction.proposal_short_id();
    let tx_status = get_tx_status(snapshot, &short_id);
    tx_pool.check_rtx_from_pool(rtx).map(|_| tx_status)
}

fn resolve_tx(
    tx_pool: &TxPool,
    snapshot: &Snapshot,
    tx: TransactionView,
    rbf: bool,
) -> ResolveResult {
    let short_id = tx.proposal_short_id();
    let tx_status = get_tx_status(snapshot, &short_id);
    tx_pool
        .resolve_tx_from_pool(tx, rbf)
        .map(|rtx| (rtx, tx_status))
}

fn commit_entry_to_pool(
    tx_pool: &mut TxPool,
    status: TxStatus,
    entry: &TxEntry,
    callbacks: &Callbacks,
) -> Result<HashSet<TxEntry>, Reject> {
    let tx_hash = entry.transaction().hash();
    debug!("submit_entry {:?} {}", status, tx_hash);
    let (succ, evicts) = match status {
        TxStatus::Fresh => tx_pool.add_pending(entry.clone())?,
        TxStatus::Gap => tx_pool.add_gap(entry.clone())?,
        TxStatus::Proposed => tx_pool.add_proposed(entry.clone())?,
    };
    if succ {
        match status {
            TxStatus::Fresh => callbacks.call_pending(entry),
            TxStatus::Gap => callbacks.call_pending(entry),
            TxStatus::Proposed => callbacks.call_proposed(entry),
        }
    }
    Ok(evicts)
}
