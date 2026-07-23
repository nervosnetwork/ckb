use crate::component::pool_map::Status;
use crate::constants::{GAP_PROPOSAL_INDEX, PROPOSED_PROPOSAL_INDEX};
use crate::error::Reject;
use crate::pool::TxPool;
use crate::service::{BlockAssemblerMessage, TxPoolService, TxVerificationResult};
use crate::tx_source::TxSource;
use crate::util::non_contextual_verify;
use ckb_error::{AnyError, InternalErrorKind};
use ckb_fee_estimator::FeeEstimator;
use ckb_jsonrpc_types::BlockTemplate;
use ckb_logger::{debug, error, info};
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::packed::OutPoint;
use ckb_types::prelude::Entity;
use ckb_types::{
    core::{
        BlockView, Capacity, EstimateMode, FeeRate, HeaderView, TransactionView,
        cell::ResolvedTransaction,
    },
    packed::{Byte32, ProposalShortId},
};
use ckb_util::LinkedHashSet;
use ckb_verification::{TxVerifyEnv, cache::Completed};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::mpsc;

mod classify;
mod post_process;
pub(crate) mod recover;
pub(crate) mod reorg;
pub(crate) mod submit;

/// A list for plug target for `plug_entry` method
pub enum PlugTarget {
    /// Pending pool
    Pending,
    /// Proposed pool
    Proposed,
}

/// Map a pool status to the verification environment used for contextual checks.
fn status_to_verify_env(status: Status, header: &HeaderView) -> TxVerifyEnv {
    match status {
        Status::Pending => TxVerifyEnv::new_submit(header),
        Status::Gap => TxVerifyEnv::new_proposed(header, GAP_PROPOSAL_INDEX),
        Status::Proposed => TxVerifyEnv::new_proposed(header, PROPOSED_PROPOSAL_INDEX),
    }
}

/// Map a pool status to the block-assembler notification that should be sent
/// when a transaction reaches this status.
fn status_to_block_assembler_message(status: Status) -> Option<BlockAssemblerMessage> {
    match status {
        Status::Pending => Some(BlockAssemblerMessage::Pending),
        Status::Proposed => Some(BlockAssemblerMessage::Proposed),
        Status::Gap => None,
    }
}

impl TxPoolService {
    /// The reject reason used when an in-flight candidate is superseded by a
    /// higher-fee-rate one. One constant so the register-time and
    /// submit-time paths produce the identical message.
    pub(crate) const SUPERSEDED_BY_HIGHER_FEE_CANDIDATE: &'static str =
        "superseded by higher-fee-rate in-flight candidate";

    pub(crate) async fn get_block_template(&self) -> Result<BlockTemplate, AnyError> {
        if let Some(ref block_assembler) = self.block_assembler {
            Ok(block_assembler.get_current().await)
        } else {
            Err(InternalErrorKind::Config
                .other("BlockAssembler disabled")
                .into())
        }
    }

    /// Record the assembler delta synchronously at the same linearization
    /// point as the pool mutation. The channel is only a wake edge; the dirty
    /// bit is the level-triggered authority and therefore cannot be left to a
    /// cancellable post-commit future.
    pub(crate) fn journal_block_assembler_update(&self, status: Status) {
        if !self.should_notify_block_assembler() {
            return;
        }
        if let Some(message) = status_to_block_assembler_message(status) {
            // Record level-triggered state before the best-effort wake. The
            // consumer merges this journal on every pass, so even the *only*
            // Pending/Proposed transition cannot be lost when the bounded
            // channel is full.
            self.relay.mark_block_assembler_dirty(&message);
            // try_send on purpose: the channel is only a wake edge now.
            // Using send().await here would backpressure verify workers
            // whenever the block-assembler loop is slow.
            if let Err(err) = self.relay.block_assembler_sender.try_send(message) {
                match err {
                    mpsc::error::TrySendError::Full(_) => {
                        debug!("block_assembler channel full; dirty update retained")
                    }
                    mpsc::error::TrySendError::Closed(_) => {
                        error!("block_assembler receiver dropped")
                    }
                }
            }
        }
    }

    /// Read-lock the pool and run `f` without cloning the snapshot.
    pub(crate) async fn read_tx_pool<U, F: FnOnce(&TxPool) -> U>(&self, f: F) -> U {
        let tx_pool = self.pool.tx_pool.read().await;
        f(&tx_pool)
    }

    /// Read-lock the pool and return the result along with a cloned snapshot.
    pub(crate) async fn read_tx_pool_with_snapshot<U, F: FnMut(&TxPool, Arc<Snapshot>) -> U>(
        &self,
        mut f: F,
    ) -> (U, Arc<Snapshot>) {
        let tx_pool = self.pool.tx_pool.read().await;
        let snapshot = tx_pool.cloned_snapshot();

        let ret = f(&tx_pool, Arc::clone(&snapshot));
        (ret, snapshot)
    }

    pub(crate) async fn non_contextual_verify(&self, tx: &TransactionView) -> Result<(), Reject> {
        // Malformed peer banning lives in the after_process remote-reject
        // path (`handle_remote_reject`); keeping it out of this shared
        // check keeps the ban decision in exactly one place.
        non_contextual_verify(&self.pool.consensus, tx)
    }

    /// Common pre-flight checks shared by all transaction submission paths.
    ///
    /// Runs non-contextual verification and rejects duplicates that are
    /// already in any pipeline queue (ordered resolve, pre-check, verify)
    /// or the orphan pool.
    pub(crate) async fn check_tx_basic_validity(&self, tx: &TransactionView) -> Result<(), Reject> {
        self.non_contextual_verify(tx).await?;

        let dup = || Reject::Duplicated(tx.hash());
        let id = tx.proposal_short_id();

        // Membership and pre-pool ownership are checked in the universal
        // TxPool -> coordinator order. This rejects replay of already accepted
        // transactions before it can consume bounded pipeline residency/CPU,
        // while remaining gap-free across a concurrent commit handoff.
        let duplicate = {
            let pool = self.pool.tx_pool.read().await;
            let accepted = pool
                .get_tx_from_pool(&id)
                .is_some_and(|resident| resident.hash() == tx.hash());
            accepted
                || self
                    .pipeline
                    .runtime
                    .read(|coordinator| coordinator.contains_hash(&tx.hash()))
        };
        if duplicate {
            return Err(dup());
        }

        Ok(())
    }

    async fn classify_and_enqueue_with_full_reject_notification(
        &self,
        tx: TransactionView,
        source: TxSource,
    ) -> Result<bool, Reject> {
        let tx_hash = tx.hash();
        let ret = self.classify_and_enqueue_tx_spawn(tx, source).await;

        if matches!(ret, Err(Reject::Full(_))) {
            self.send_result_to_relayer(TxVerificationResult::Reject { tx_hash })
                .await;
        }

        ret
    }

    pub(crate) async fn submit_remote_tx(
        &self,
        tx: TransactionView,
        source: TxSource,
    ) -> Result<bool, Reject> {
        // Preflight failures go through after_process too: the remote error
        // triple (ban / relay / recent_reject) must apply here exactly like
        // it does to failures deeper in the pipeline, otherwise a peer can
        // resubmit the same malformed tx forever with no consequence.
        if let Err(reject) = self.check_tx_basic_validity(&tx).await {
            self.reject_with_after_process(tx, source, reject.clone())
                .await;
            return Err(reject);
        }
        self.classify_and_enqueue_with_full_reject_notification(tx, source)
            .await
    }

    pub(crate) async fn notify_tx(&self, tx: TransactionView) -> Result<bool, Reject> {
        // Proposal is a trusted source promotion, not an ordinary duplicate.
        // Admission must reach the coordinator so an existing Remote owner is
        // upgraded in place (priority, peer budget and ban attribution) while
        // any active versioned lease remains valid.
        if let Err(reject) = self.non_contextual_verify(&tx).await {
            self.reject_with_after_process(tx, TxSource::Proposal, reject.clone())
                .await;
            return Err(reject);
        }
        self.classify_and_enqueue_with_full_reject_notification(tx, TxSource::Proposal)
            .await
    }

    pub(crate) async fn process_tx(
        &self,
        tx: TransactionView,
        source: TxSource,
    ) -> Result<Completed, Reject> {
        // Local RPC is deliberately synchronous. A matching remote/proposal
        // coordinator entry must not turn it into an asynchronous duplicate:
        // run the authoritative checks directly and let a successful pool
        // commit invalidate the older coordinator lease.
        if let Err(reject) = self.non_contextual_verify(&tx).await {
            self.reject_with_after_process(tx, source, reject.clone())
                .await;
            return Err(reject);
        }

        let ret = self.process_tx_direct(tx.clone(), source, None).await;
        // A fresh commit journals its Ok relay result inside the authoritative
        // pool transaction. Only pre-commit failures (including a Local
        // duplicate, which intentionally re-broadcasts Ok) remain here.
        if ret.is_err() {
            self.after_process(tx, source, &ret).await;
        }
        ret
    }

    pub(crate) async fn send_result_to_relayer(&self, result: TxVerificationResult) {
        self.publish_relay_result(result).await;
    }
    pub(crate) fn sort_txs_by_dependencies(txs: &mut Vec<TransactionView>) {
        Self::sort_by_dependencies(txs, |tx| tx);
    }

    /// Topologically sort a list of items that wrap transactions so that
    /// parents are placed before their children. `tx_of` extracts the
    /// transaction reference from each item.
    pub(crate) fn sort_by_dependencies<T>(
        items: &mut Vec<T>,
        tx_of: impl Fn(&T) -> &TransactionView,
    ) {
        if items.len() <= 1 {
            return;
        }

        let mut output_to_index: HashMap<OutPoint, usize> =
            HashMap::with_capacity(items.len().saturating_mul(2));
        for (i, item) in items.iter().enumerate() {
            let tx_hash = tx_of(item).hash();
            for idx in 0..tx_of(item).outputs().len() {
                output_to_index.insert(OutPoint::new(tx_hash.clone(), idx as u32), i);
            }
        }

        let mut in_degree = vec![0usize; items.len()];
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); items.len()];
        for (i, item) in items.iter().enumerate() {
            let tx = tx_of(item);
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

        let mut ready: VecDeque<usize> = (0..items.len()).filter(|&i| in_degree[i] == 0).collect();
        let mut sorted = Vec::with_capacity(items.len());
        while let Some(i) = ready.pop_front() {
            sorted.push(i);
            for &child in &children[i] {
                in_degree[child] -= 1;
                if in_degree[child] == 0 {
                    ready.push_back(child);
                }
            }
        }

        if sorted.len() != items.len() {
            // A cycle should never happen in valid detached blocks, but if it
            // does we keep the original order rather than losing transactions.
            return;
        }

        let mut remaining: Vec<Option<T>> = items.drain(..).map(Some).collect();
        items.extend(
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
        // Reserve before taking `recovery_lock`: publication backpressure
        // must never form recovery_lock -> effect queue -> callback ->
        // save_pool -> recovery_lock. The conservative whole-pool credit is
        // shrunk to the actual batch while the pool mutation is still locked.
        let reorg_permit = match self
            .reserve_critical_effects(self.max_reorg_effect_bytes())
            .await
        {
            Ok(permit) => permit,
            Err(error) => {
                error!("reorg effect reservation failed: {:?}", error);
                return;
            }
        };
        // Hold the recovery lock for the *whole* reorg — the write-lock
        // section and the retained-transaction recovery —
        // so `save_pool` can never persist a half-updated pool: a snapshot
        // taken between the write-lock section and the recovery would
        // silently lose the detached transactions that have not been
        // re-added yet. Lock order: recovery_lock before tx_pool, and
        // nothing holding tx_pool ever acquires recovery_lock.
        let _recovery_guard = self.recovery_lock.lock().await;

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
            self.aux.fee_estimator.commit_block(&blk);
            attached.extend(blk.transactions().into_iter().skip(1));
        }
        let mut retain = reorg::detached_not_attached(&detached, &attached);

        let reorg::ReorgOutcome {
            reject_events,
            silently_removed,
            notify_events,
        } = {
            // This closure is used to limit the lifetime of mutable tx_pool.
            let mut tx_pool = self.pool.tx_pool.write().await;

            let outcome = reorg::update_tx_pool_for_reorg(
                &mut tx_pool,
                &attached,
                &detached_headers,
                detached_proposal_id,
                snapshot,
                mine_mode,
            );
            // Apply the complete membership delta under the same pool write
            // guard. Attached/on-chain parents remain available; every other
            // physical removal demotes its already-resolved coordinator
            // consumers. The reorg worker retains and retries this whole delta
            // on panic, so a coordinator failure must fail closed rather than
            // release an incoherent pool/coordinator pair.
            let mut committed: HashSet<_> = attached.iter().map(TransactionView::hash).collect();
            let mut unavailable = HashSet::new();
            for entry in outcome
                .reject_events
                .iter()
                .map(|(entry, _)| entry)
                .chain(outcome.silently_removed.iter())
            {
                let hash = entry.transaction().hash();
                if tx_pool.snapshot().transaction_exists(&hash) {
                    committed.insert(hash);
                } else {
                    unavailable.insert(hash);
                }
            }
            if let Err(error) = self.pipeline.runtime.mutate(|coordinator| {
                coordinator.external_commits_with_unavailable_parents(&committed, &unavailable)
            }) {
                panic!("reorg coordinator membership transaction failed: {error:?}");
            }
            let scheduled_recoveries = tx_pool.schedule_conflicted_txs_from_inputs(
                outcome
                    .reject_events
                    .iter()
                    .map(|(entry, _)| entry)
                    .chain(outcome.silently_removed.iter())
                    .flat_map(|entry| entry.transaction().input_pts_iter()),
            );
            if scheduled_recoveries != 0 {
                self.pipeline.runtime.request_maintenance();
            }
            let mut effects = Vec::new();
            for (entry, reject) in &outcome.reject_events {
                effects.extend(self.rejected_effects(entry.clone(), reject.clone()));
            }
            for (entry, status) in &outcome.notify_events {
                if let Some(effect) = self.accepted_effect(entry.clone(), *status) {
                    effects.push(effect);
                }
            }
            if let Err(error) = self.publish_reserved_effects(reorg_permit, effects) {
                panic!("reserved reorg effect journal failed inside pool transaction: {error:?}");
            }
            outcome
        };
        // Publication was bound before the pool guard opened. The publisher
        // may run while detached transactions are recovered, but every
        // callback observes a complete pool-mutation slice;
        // `recovery_lock` protects persistence, not ordinary RPC visibility.
        let _ = (reject_events, notify_events);
        let _ = silently_removed;

        // Recover detached transactions through the direct per-tx entry point.
        // Dependent transactions must be processed after their parents have
        // already been submitted to the pool; a topological sort guarantees this
        // ordering. Using `process_tx_direct` here keeps the recovery logic simple
        // and correct while still releasing the tx-pool write lock between
        // transactions.
        {
            Self::sort_txs_by_dependencies(&mut retain);
            let mut chunk_rx = self.pipeline.chunk_rx.clone();
            for tx in retain {
                let outcome = self
                    .process_tx_direct_outcome(tx.clone(), TxSource::Local, Some(&mut chunk_rx))
                    .await;
                // Only a definitive failure may cascade-remove dependents.
                // `Duplicated` means it is already back in the pool
                // (concurrent resubmission, or this same reorg retried after
                // a panic) — cascading on either would evict healthy
                // dependents and emit spurious Dead rejections.
                let reject = match outcome {
                    Ok(crate::process::submit::VerifySubmitOutcome::Committed(_))
                    | Err(Reject::Duplicated(_)) => {
                        // The detached tx is back in the pool. Wake up any
                        // orphans that depend on it (including via cell dep).
                        continue;
                    }
                    Ok(crate::process::submit::VerifySubmitOutcome::Cleared) => {
                        debug!("reorg recovery tx {} invalidated by clear", tx.hash());
                        continue;
                    }
                    Err(reject) => reject,
                };
                {
                    debug!("reorg re-add failed: {}", reject);
                    let cascade_permit = self
                        .reserve_effects(self.max_reorg_effect_bytes())
                        .await
                        .unwrap_or_else(|error| {
                            panic!("reorg cascade effect reservation failed: {error:?}")
                        });
                    // The detached tx could not be re-added: any in-pool
                    // transactions referencing its outputs — as inputs *or*
                    // as cell deps — can never resolve now and would sit in
                    // the pool as zombies until expiry (the template
                    // builder filters them out every round). Cascade-remove
                    // them and journal their terminal effects before opening
                    // the same pool write lock.
                    {
                        let mut tx_pool = self.pool.tx_pool.write().await;
                        let mut roots: HashMap<ProposalShortId, OutPoint> = HashMap::new();
                        for out_point in tx.output_pts() {
                            if let Some(id) = tx_pool
                                .pool_map
                                .out_point_index
                                .get_input_ref(&out_point)
                                .cloned()
                            {
                                roots.entry(id).or_insert_with(|| out_point.clone());
                            }
                            if let Some(ids) =
                                tx_pool.pool_map.out_point_index.get_deps_ref(&out_point)
                            {
                                for id in ids {
                                    roots.entry(id.clone()).or_insert_with(|| out_point.clone());
                                }
                            }
                        }
                        let mut removal_ids: HashSet<_> = roots.keys().cloned().collect();
                        for root in roots.keys() {
                            removal_ids.extend(tx_pool.pool_map.calc_descendants(root));
                        }
                        let removal_hashes: HashSet<_> = removal_ids
                            .iter()
                            .filter_map(|id| {
                                tx_pool
                                    .pool_map
                                    .get_by_id(id)
                                    .map(|entry| entry.inner.transaction().hash())
                            })
                            .collect();
                        if let Err(error) = self
                            .pipeline
                            .runtime
                            .mutate(|coordinator| coordinator.parents_unavailable(&removal_hashes))
                        {
                            panic!(
                                "reorg failed-recovery dependency transaction failed: {error:?}"
                            );
                        }
                        let mut ordered_roots: Vec<_> = roots.into_iter().collect();
                        ordered_roots
                            .sort_by(|(left, _), (right, _)| left.as_slice().cmp(right.as_slice()));
                        let mut effects = Vec::new();
                        for (child_id, out_point) in ordered_roots {
                            let removed = tx_pool.pool_map.remove_entry_and_descendants(&child_id);
                            for entry in removed {
                                debug!(
                                    "cascade-remove pool tx {}: its reference {:?} died with the failed re-add",
                                    entry.transaction().hash(),
                                    out_point,
                                );
                                effects.extend(self.rejected_effects(
                                    entry,
                                    Reject::Resolve(ckb_types::core::error::OutPointError::Dead(
                                        out_point.clone(),
                                    )),
                                ));
                            }
                        }
                        if let Err(error) = self.publish_reserved_effects(cascade_permit, effects) {
                            panic!(
                                "reserved reorg cascade journal failed inside pool transaction: {error:?}"
                            );
                        }
                    }
                    self.after_process(tx, TxSource::Local, &Err(reject)).await;
                }
            }
        }
    }

    pub(crate) async fn clear_pool(&mut self, new_snapshot: Arc<Snapshot>) {
        let terminal_permit = match self
            .reserve_effects(Self::pipeline_terminal_effect_bytes(
                self.pipeline.runtime.max_entries(),
            ))
            .await
        {
            Ok(permit) => permit,
            Err(error) => {
                error!("clear pool effect reservation failed: {:?}", error);
                return;
            }
        };
        // Invalidate popped/active work before waiting for recovery or the
        // pool write lock. Every later pipeline boundary and the final commit
        // reject the old generation, while a submit that already linearized
        // under the pool lock is removed by the clear below.
        self.advance_pipeline_epoch();
        // Same lock as the reorg recovery: an in-flight reorg must finish
        // re-adding its detached transactions before the pool is cleared,
        // otherwise the freshly cleared pool would be repopulated by the
        // recovery and `clear_pool` would return with a non-empty pool.
        let _recovery_guard = self.recovery_lock.lock().await;
        let terminal = {
            let mut tx_pool = self.pool.tx_pool.write().await;
            tx_pool.clear(Arc::clone(&new_snapshot));
            self.pipeline.runtime.mutate(|coordinator| {
                let result = coordinator.clear();
                if let Ok(records) = &result {
                    self.journal_pipeline_terminal_records(terminal_permit, records);
                }
                result
            })
        };
        match terminal {
            Ok(_records) => {}
            Err(error) => error!("failed to clear pipeline coordinator: {:?}", error),
        }
        // reset block_assembler
        if self
            .relay
            .block_assembler_sender
            .send(BlockAssemblerMessage::Reset(new_snapshot))
            .await
            .is_err()
        {
            error!("block_assembler receiver dropped");
        }
    }

    pub(crate) async fn save_pool(&self) {
        // Wait for any in-flight reorg recovery to finish so the persisted
        // file always represents a complete recovery point: a snapshot
        // taken mid-recovery would silently lose the detached transactions
        // that have not been re-added yet. Lock order: recovery_lock before
        // tx_pool, and nothing holding tx_pool ever acquires recovery_lock.
        let _recovery_guard = self.recovery_lock.lock().await;
        let mut tx_pool = self.pool.tx_pool.write().await;
        if let Err(err) = tx_pool.save_into_file() {
            error!("failed to save pool, error: {:?}", err)
        } else {
            info!("TxPool saved successfully")
        }
    }

    pub(crate) async fn update_ibd_state(&self, in_ibd: bool) {
        self.aux.fee_estimator.update_ibd_state(in_ibd);
    }

    pub(crate) async fn estimate_fee_rate(
        &self,
        estimate_mode: EstimateMode,
        enable_fallback: bool,
    ) -> Result<FeeRate, AnyError> {
        let all_entry_info = self
            .read_tx_pool(|tx_pool| tx_pool.get_all_entry_info())
            .await;
        match self
            .aux
            .fee_estimator
            .estimate_fee_rate(estimate_mode, all_entry_info)
        {
            Ok(fee_rate) => Ok(fee_rate),
            Err(err) => {
                if enable_fallback {
                    let target_blocks =
                        FeeEstimator::target_blocks_for_estimate_mode(estimate_mode);
                    self.pool
                        .tx_pool
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
    pub(crate) status: Status,
    /// Transaction fee.
    pub(crate) fee: Capacity,
    /// Transaction size in bytes as serialized in a block.
    pub(crate) tx_size: usize,
}

type ResolveResult = Result<(Arc<ResolvedTransaction>, Status), Reject>;

/// Assemble a [`PreCheckedTx`] from its parts; shared by the fast
/// (chain-only) and locked (pool-overlay) pre-check paths.
fn make_pre_checked_tx(
    pre_resolve_tip: Byte32,
    rtx: Arc<ResolvedTransaction>,
    status: Status,
    fee: Capacity,
    tx_size: usize,
) -> PreCheckedTx {
    PreCheckedTx {
        pre_resolve_tip,
        rtx,
        status,
        fee,
        tx_size,
    }
}

fn get_tx_status(snapshot: &Snapshot, short_id: &ProposalShortId) -> Status {
    if snapshot.proposals().contains_proposed(short_id) {
        Status::Proposed
    } else if snapshot.proposals().contains_gap(short_id) {
        Status::Gap
    } else {
        Status::Pending
    }
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
