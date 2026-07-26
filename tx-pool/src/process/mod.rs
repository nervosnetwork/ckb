use crate::component::pool_map::Status;
use crate::constants::{GAP_PROPOSAL_INDEX, PROPOSED_PROPOSAL_INDEX};
use crate::error::Reject;
use crate::pool::TxPool;
use crate::service::effects::{EffectBatch, TxPoolEffect};
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
use ckb_types::{
    core::{
        BlockView, Capacity, EstimateMode, FeeRate, HeaderView, TransactionView, UncleBlockView,
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
pub(crate) mod reorg;
pub(crate) mod submit;

/// A list for plug target for `plug_entry` method
pub enum PlugTarget {
    /// Pending pool
    Pending,
    /// Proposed pool
    Proposed,
}

/// Sealed accepted + pre-pool generations retired by one DefectDomain swap.
/// It has no lookup or re-entry API; dropping it is the only operation and
/// must happen after the caller releases every authority guard.
#[must_use = "retired generations must be dropped after authority guards are released"]
pub(crate) struct AuthoritativeDisposal<'a> {
    accepted: Option<crate::component::pool_map::PoolMap>,
    _kernel: crate::component::pre_pool::KernelDisposal<'a>,
}

enum ReorgMutation {
    Retained(crate::component::pre_pool::RecoveryBatch),
    Reset(Vec<TransactionView>),
}

impl Drop for AuthoritativeDisposal<'_> {
    fn drop(&mut self) {
        drop(self.accepted.take());
    }
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

    /// Contain an impossible Apply unwind below the service boundary. The
    /// accepted PoolMap and pre-pool entry/index generation are both
    /// ephemeral and are replaced while the caller still owns the TxPool
    /// write guard; no partially mutated authority becomes visible.
    pub(crate) fn recover_authoritative_defect(
        &self,
        tx_pool: &mut TxPool,
        target_snapshot: Arc<Snapshot>,
        context: &'static str,
        defect: String,
    ) -> AuthoritativeDisposal<'_> {
        self.recover_authoritative_defect_with_fingerprint(
            tx_pool,
            target_snapshot,
            context,
            defect,
            None,
        )
    }

    pub(crate) fn recover_authoritative_defect_with_fingerprint(
        &self,
        tx_pool: &mut TxPool,
        target_snapshot: Arc<Snapshot>,
        context: &'static str,
        defect: String,
        fingerprint: Option<Byte32>,
    ) -> AuthoritativeDisposal<'_> {
        let accepted = tx_pool.reset_generation(Arc::clone(&target_snapshot));
        let (kernel_generation, kernel) = self.pipeline.kernel.reset_after_defect(fingerprint);
        self.advance_pipeline_epoch();
        self.journal_block_assembler_reset(target_snapshot);
        if let Err(error) = self.relay.effects.install_generation_reset() {
            error!("authoritative generation reset journal is unavailable: {error:?}");
        }
        error!(
            "{context}: {defect}; replaced accepted/pre-pool generation {kernel_generation}; remote ingress cooling is active"
        );
        AuthoritativeDisposal {
            accepted: Some(accepted),
            _kernel: kernel,
        }
    }

    async fn recover_current_authoritative_defect(&self, context: &'static str, defect: String) {
        let mut tx_pool = self.pool.tx_pool.write().await;
        let current = tx_pool.cloned_snapshot();
        let disposal = self.recover_authoritative_defect(&mut tx_pool, current, context, defect);
        drop(tx_pool);
        drop(disposal);
    }

    fn wake_block_assembler(&self, message: BlockAssemblerMessage, saturated: &'static str) {
        if let Err(error) = self.relay.block_assembler_sender.try_send(message) {
            match error {
                mpsc::error::TrySendError::Full(_) => debug!("{saturated}"),
                mpsc::error::TrySendError::Closed(_) => {
                    error!("block_assembler receiver dropped")
                }
            }
        }
    }

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
        if let Some(message) = status_to_block_assembler_message(status) {
            self.journal_block_assembler_message(message);
        }
    }

    /// Record a level-triggered assembler delta before issuing a best-effort
    /// wake edge. This is shared by pool status and candidate-uncle changes;
    /// neither producer may await bounded channel capacity after mutation.
    pub(crate) fn journal_block_assembler_message(&self, message: BlockAssemblerMessage) {
        if !self.should_notify_block_assembler() {
            return;
        }
        self.relay.mark_block_assembler_dirty(&message);
        self.wake_block_assembler(
            message,
            "block_assembler channel full; dirty update retained",
        );
    }

    /// Preserve the original block-assembler priority model: `update_full`
    /// wins unconditionally, while proposal/transaction deltas remain
    /// optimistic. A successful full swap reissues both delta generations so
    /// work acknowledged just before the swap is reconciled once more.
    pub(crate) fn journal_block_assembler_full_reconcile(&self) {
        if !self.should_notify_block_assembler() {
            return;
        }
        self.relay.mark_block_assembler_full_reconcile();
        for message in [
            BlockAssemblerMessage::Pending,
            BlockAssemblerMessage::Proposed,
        ] {
            self.wake_block_assembler(
                message,
                "block_assembler channel full; post-full reconcile retained",
            );
        }
    }

    /// Journal a management reset at the pool-mutation linearization point.
    /// The bounded channel carries only a wake token; the latest snapshot is
    /// retained here and therefore survives channel saturation/cancellation.
    pub(crate) fn journal_block_assembler_reset(&self, snapshot: Arc<Snapshot>) {
        if !self.should_notify_block_assembler() {
            return;
        }
        self.relay.mark_block_assembler_reset(snapshot);
        self.wake_block_assembler(
            BlockAssemblerMessage::Reset,
            "block_assembler channel full; reset retained",
        );
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
        // Membership and pre-pool ownership are checked in the universal
        // TxPool -> coordinator order. This rejects replay of already accepted
        // transactions before it can consume bounded pipeline residency/CPU,
        // while remaining gap-free across a concurrent commit handoff.
        let duplicate = {
            let pool = self.pool.tx_pool.read().await;
            let accepted = pool.get_tx_from_pool_by_hash(&tx.hash()).is_some();
            accepted
                || self
                    .pipeline
                    .kernel
                    .read(|coordinator| coordinator.contains_hash(&tx.hash()))
        };
        if duplicate {
            return Err(dup());
        }

        Ok(())
    }

    pub(crate) async fn submit_remote_tx(
        &self,
        tx: TransactionView,
        source: TxSource,
    ) -> Result<bool, Reject> {
        // Preflight failures go through after_process too: the remote terminal
        // policy (malformed-peer ban, eligible relayer cleanup, and
        // recent-reject history) must apply here exactly like it does to
        // failures deeper in the pipeline. Malformed rejects are intentionally
        // not retryable relayer events, but they still ban and record.
        if let Err(reject) = self.check_tx_basic_validity(&tx).await {
            // `Duplicated` covers both an already-accepted pool entry and an
            // unverified coordinator owner. Only the former is a definitive
            // success; acknowledging the latter before verification would let
            // an invalid first witness make another peer accept the raw hash.
            let accepted_duplicate = if matches!(reject, Reject::Duplicated(_)) {
                self.pool
                    .tx_pool
                    .read()
                    .await
                    .get_tx_from_pool_by_hash(&tx.hash())
                    .is_some()
            } else {
                false
            };
            if accepted_duplicate {
                if let (Some(peer), Ok(epoch)) = (source.peer(), self.current_pipeline_epoch()) {
                    self.handle_verify_success(&tx, Some(peer), epoch).await;
                }
            } else {
                self.reject_with_after_process(tx, source, reject.clone())
                    .await;
            }
            return Err(reject);
        }
        self.classify_and_enqueue_tx_spawn(tx, source).await
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
        // If the same raw hash is already owned with another witness, the
        // admission adapter atomically installs this trusted payload, expires
        // the old worker lease and restarts normal bounded processing. Script
        // verification therefore never runs serially on this dispatcher.
        self.classify_and_enqueue_tx_spawn(tx, TxSource::Proposal)
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
    ) -> Result<(Vec<UncleBlockView>, Arc<Snapshot>), Reject> {
        let effect_bound = self.max_reorg_effect_bytes();
        let recovery_epoch = self.current_pipeline_epoch().map_err(|_| {
            Reject::Full("pipeline generation is unavailable during reorg".to_owned())
        })?;

        let mine_mode = self.block_assembler.is_some();
        let mut detached = LinkedHashSet::default();
        let mut attached = LinkedHashSet::default();

        let detached_headers: HashSet<Byte32> = detached_blocks
            .iter()
            .map(|blk| blk.header().hash())
            .collect();
        // Phase two needs only compact uncle candidates, never the full
        // transaction-bearing blocks. Bound the handoff through the same
        // candidate container used by the assembler so completing phase one
        // releases the original retained reorg message and all tx payloads.
        let candidate_uncles = if mine_mode {
            let mut candidates = crate::block_assembler::CandidateUncles::new();
            for block in &detached_blocks {
                candidates.insert(block.as_uncle());
            }
            candidates.into_values()
        } else {
            Vec::new()
        };
        let attached_headers: HashSet<Byte32> = attached_blocks
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
        Self::sort_txs_by_dependencies(&mut retain);
        let mut retained_hashes = HashSet::new();
        retain.retain(|tx| retained_hashes.insert(tx.hash()));

        let recovery = {
            // This closure is used to limit the lifetime of mutable tx_pool.
            let mut tx_pool = self.pool.tx_pool.write().await;
            // A later clear publishes its epoch barrier before waiting for
            // this guard. If it won while this reorg was queued, applying the
            // older chain cohort afterwards would resurrect cleared work.
            if !self.is_pipeline_epoch_current(recovery_epoch) {
                return Ok((Vec::new(), snapshot));
            }
            let transition = self.pipeline.kernel.guard_authoritative_mutation(
                "reorg authoritative pool mutation panicked",
                || {
                    self.pipeline.kernel.mutate(|kernel| {
                        self.relay.effects.try_apply_authoritative(
                            effect_bound,
                            |publish_detail| {
                                let applied = (|| -> Result<_, Reject> {
                                    // Decide the generation-reset fallback
                                    // before either authority mutates. The
                                    // attached branch can only remove entries
                                    // from this closure, so a bounded pre-plan
                                    // remains a safe over-approximation after
                                    // accepted-pool reconciliation.
                                    tx_pool.preflight_reorg_status_transitions()?;
                                    let pre_plan = reorg::plan_accepted_recovery(
                                        &tx_pool,
                                        &retain,
                                        crate::constants::MAX_POOL_MUTATION_CANDIDATES,
                                    );
                                    let accepted = match pre_plan {
                                        reorg::AcceptedRecoveryPlan::OverBound => {
                                            return Ok((
                                                ReorgMutation::Reset(retain.clone()),
                                                Vec::new(),
                                            ));
                                        }
                                        plan => plan.transactions_parent_first(),
                                    };

                                    let mut combined = retain.clone();
                                    combined.extend(accepted);
                                    Self::sort_txs_by_dependencies(&mut combined);
                                    let mut hashes = HashSet::new();
                                    combined.retain(|tx| hashes.insert(tx.hash()));
                                    let recovery = match kernel.retain_recovery_batch(
                                        combined.clone(),
                                        recovery_epoch,
                                    ) {
                                        Ok(batch) => batch,
                                        Err(error)
                                            if error.is_capacity_rejection()
                                                || error.is_transaction_rejection() =>
                                        {
                                            return Ok((ReorgMutation::Reset(combined), Vec::new()));
                                        }
                                        Err(error) => panic!(
                                            "reorg recovery retention violated kernel invariants: {error:?}"
                                        ),
                                    };

                                    let mut outcome = reorg::begin_tx_pool_reorg(
                                        &mut tx_pool,
                                        &attached,
                                        &detached_headers,
                                        detached_proposal_id.clone(),
                                        Arc::clone(&snapshot),
                                        mine_mode,
                                    )?;
                                    let accepted_plan = reorg::plan_accepted_recovery(
                                        &tx_pool,
                                        &retain,
                                        crate::constants::MAX_POOL_MUTATION_CANDIDATES,
                                    );
                                    let moved = reorg::apply_accepted_recovery(
                                        &mut tx_pool,
                                        accepted_plan,
                                    );
                                    outcome
                                        .recovery_removed
                                        .extend(moved.into_iter().map(|removed| removed.entry));
                                    reorg::finish_tx_pool_reorg(&mut tx_pool, &mut outcome);
                                    // Apply the complete membership delta under the same pool write
                                    // guard. Attached/on-chain parents remain available; every other
                                    // physical removal demotes its already-resolved coordinator
                                    // consumers. An impossible projection defect unwinds to the
                                    // generation-local DefectDomain before either guard opens.
                                    let mut committed: HashSet<_> =
                                        attached.iter().map(TransactionView::hash).collect();
                                    let mut unavailable = HashSet::new();
                                    for entry in outcome
                                        .reject_events
                                        .iter()
                                        .map(|(entry, _)| entry)
                                        .chain(outcome.silently_removed.iter())
                                        .chain(outcome.recovery_removed.iter())
                                    {
                                        let hash = entry.transaction().hash();
                                        if tx_pool.snapshot().transaction_exists(&hash) {
                                            committed.insert(hash);
                                        } else {
                                            unavailable.insert(hash);
                                        }
                                    }
                                    let mut available_outpoints = tx_pool
                                        .released_inputs_from_removed_entries(
                                            outcome
                                                .reject_events
                                                .iter()
                                                .map(|(entry, _)| entry)
                                                .chain(outcome.silently_removed.iter()),
                                        );
                                    // A historical candidate may have observed its conflicting
                                    // input release before another required parent was mined.
                                    // Attached outputs and headers are potential availability
                                    // edges, but only the post-reorg overlay decides their
                                    // level: an output created and consumed in the same
                                    // attached branch is not available and must not wake a
                                    // rejected conflict into a bogus second rejection.
                                    available_outpoints.extend(
                                        attached.iter().flat_map(TransactionView::output_pts),
                                    );
                                    let mut available_dependencies =
                                        crate::service::pipeline_ops::available_cell_dependencies(
                                            &tx_pool,
                                            available_outpoints,
                                        );
                                    available_dependencies.extend(
                                        attached_headers.iter().filter_map(|hash| {
                                            let key =
                                                crate::component::pre_pool::DependencyKey::Header(
                                                    crate::util::compact_packed(hash),
                                                );
                                            crate::service::pipeline_ops::dependency_is_available(
                                                &tx_pool, &key,
                                            )
                                            .then_some(key)
                                        }),
                                    );
                                    let externally_committed = kernel
                                        .external_commits_with_unavailable_parents(
                                            &committed,
                                            &unavailable,
                                        )
                                        .expect("planned reorg pre-pool membership update");
                                    kernel
                                        .note_available(available_dependencies)
                                        .expect("planned reorg availability update");
                                    let mut effects = Vec::new();
                                    if publish_detail {
                                        for (entry, reject) in &outcome.reject_events {
                                            effects.extend(self.rejected_effects(
                                                entry.clone(),
                                                reject.clone(),
                                            ));
                                        }
                                        for (entry, status) in &outcome.notify_events {
                                            if let Some(effect) =
                                                self.accepted_effect(entry.clone(), *status)
                                            {
                                                effects.push(effect);
                                            }
                                        }
                                        for record in externally_committed {
                                            if let Some(peer) = record.raw.ingress_peer() {
                                                effects.push(TxPoolEffect::Relay(
                                                    crate::service::TxVerificationResult::Ok {
                                                        original_peer: Some(peer),
                                                        tx_hash: record.raw.tx.hash(),
                                                    },
                                                ));
                                            }
                                        }
                                    }
                                    // Invalidate the old template at the same
                                    // linearization point as the new snapshot
                                    // and retained ownership. Full publication
                                    // remains the second retained reorg phase.
                                    self.journal_block_assembler_reset(Arc::clone(&snapshot));
                                    Ok((ReorgMutation::Retained(recovery), effects))
                                })();
                                match applied {
                                    Ok((mutation, effects)) => {
                                        (Ok(mutation), EffectBatch::new(effects))
                                    }
                                    Err(reject) => (Err(reject), None),
                                }
                            },
                        )
                    })
                },
            );
            match transition {
                Ok(Ok(Ok(ReorgMutation::Retained(recovery)))) => recovery,
                Ok(Ok(Ok(ReorgMutation::Reset(txs)))) => {
                    let reset = self.pipeline.kernel.reset_for_chain(|fresh| {
                        fresh.retain_recovery_prefix_after_clear(txs, recovery_epoch)
                    });
                    let (recovery, kernel) = match reset {
                        Ok(reset) => reset,
                        Err(error) => {
                            let disposal = self.recover_authoritative_defect(
                                &mut tx_pool,
                                Arc::clone(&snapshot),
                                "reorg generation preparation",
                                format!("{error:?}"),
                            );
                            drop(tx_pool);
                            drop(disposal);
                            return Ok((candidate_uncles, snapshot));
                        }
                    };
                    let accepted = tx_pool.reset_generation(Arc::clone(&snapshot));
                    self.journal_block_assembler_reset(Arc::clone(&snapshot));
                    if let Err(error) = self.relay.effects.install_generation_reset() {
                        error!("reorg generation reset journal is unavailable: {error:?}");
                    }
                    let disposal = AuthoritativeDisposal {
                        accepted: Some(accepted),
                        _kernel: kernel,
                    };
                    drop(tx_pool);
                    drop(disposal);
                    recovery
                }
                Ok(Ok(Err(reject))) => return Err(reject),
                Ok(Err(error)) => {
                    return Err(Reject::Full(format!(
                        "reorg effect journal unavailable: {error:?}"
                    )));
                }
                Err(defect) => {
                    let disposal = self.recover_authoritative_defect(
                        &mut tx_pool,
                        Arc::clone(&snapshot),
                        "reorg authoritative pool mutation",
                        defect,
                    );
                    drop(tx_pool);
                    drop(disposal);
                    return Ok((candidate_uncles, snapshot));
                }
            }
        };

        // Drain only kernel-owned payloads. The handler carries an identity
        // session, never a transaction owner; clear makes the lease stale and
        // explicit save can copy it at any await boundary.
        let mut chunk_rx = self.pipeline.chunk_rx.clone();
        loop {
            // Checkout is itself the completion predicate. Keeping the old
            // `pending` read separate created a legal clear-between-locks
            // race that looked like an impossible missing recovery head.
            let lease = self.pipeline.kernel.guard_authoritative_mutation(
                "recovery checkout panicked",
                || {
                    self.pipeline
                        .kernel
                        .checkout_recovery(recovery.session)
                        .unwrap_or_else(|error| panic!("recovery checkout failed: {error:?}"))
                },
            );
            let lease = match lease {
                Ok(Some(lease)) => lease,
                Ok(None) => break,
                Err(defect) => {
                    self.recover_current_authoritative_defect("recovery checkout", defect)
                        .await;
                    return Ok((candidate_uncles, snapshot));
                }
            };
            let tx = lease.payload.tx.clone();
            let outcome = self
                .process_tx_direct_outcome(tx.clone(), TxSource::Local, Some(&mut chunk_rx))
                .await;
            let reject = match outcome {
                Ok(crate::process::submit::VerifySubmitOutcome::Committed(_)) => continue,
                Err(Reject::Duplicated(_)) => {
                    let settled = self.pipeline.kernel.guard_authoritative_mutation(
                        "duplicate recovery lease terminalization panicked",
                        || {
                            self.pipeline.kernel.mutate_lease(
                                "duplicate recovery lease could not terminalize",
                                |kernel| kernel.terminalize_resolve(&lease),
                            )
                        },
                    );
                    if let Err(defect) = settled {
                        self.recover_current_authoritative_defect(
                            "duplicate recovery terminalization",
                            defect,
                        )
                        .await;
                        return Ok((candidate_uncles, snapshot));
                    }
                    continue;
                }
                Ok(crate::process::submit::VerifySubmitOutcome::Cleared) => {
                    debug!("reorg recovery tx {} invalidated by clear", tx.hash());
                    continue;
                }
                Err(reject) => reject,
            };
            let settled = self.pipeline.kernel.guard_authoritative_mutation(
                "failed recovery lease terminalization panicked",
                || {
                    self.pipeline
                        .kernel
                        .mutate_lease("failed recovery lease could not terminalize", |kernel| {
                            kernel.terminalize_resolve(&lease)
                        })
                },
            );
            if let Err(defect) = settled {
                self.recover_current_authoritative_defect(
                    "failed recovery terminalization",
                    defect,
                )
                .await;
                return Ok((candidate_uncles, snapshot));
            }
            debug!("reorg re-add failed: {}", reject);
            self.after_process(tx, TxSource::Local, &Err(reject)).await;
        }
        Ok((candidate_uncles, snapshot))
    }

    pub(crate) async fn clear_pool(&mut self, new_snapshot: Arc<Snapshot>) {
        self.advance_pipeline_epoch();
        let mut tx_pool = self.pool.tx_pool.write().await;
        let (_, kernel) = self
            .pipeline
            .kernel
            .reset_for_chain(|_| Ok(()))
            .expect("empty clear generation preparation is total");
        let accepted = tx_pool.reset_generation(Arc::clone(&new_snapshot));
        self.journal_block_assembler_reset(new_snapshot);
        if let Err(error) = self.relay.effects.install_generation_reset() {
            error!("clear-pool generation reset journal is unavailable: {error:?}");
        }
        let disposal = AuthoritativeDisposal {
            accepted: Some(accepted),
            _kernel: kernel,
        };
        drop(tx_pool);
        drop(disposal);
    }

    pub(crate) async fn save_pool(&self) {
        let _writer = self.persistence_lock.lock().await;
        let (base, accepted, recovery) = {
            // Universal cross-authority order. Copying compact transaction
            // views is bounded by the accepted + pre-pool residency limits;
            // no authority guard survives the blocking writer.
            let tx_pool = self.pool.tx_pool.read().await;
            let base = tx_pool.config.persisted_data.clone();
            let accepted = tx_pool.get_all_txs();
            let recovery = self.pipeline.kernel.recovery_snapshot();
            (base, accepted, recovery)
        };
        let snapshot = crate::persisted::PersistenceSnapshot { accepted, recovery };
        let result =
            tokio::task::spawn_blocking(move || crate::persisted::write_snapshot(&base, snapshot))
                .await;
        match result {
            Ok(Ok(())) => info!("TxPool saved successfully"),
            Ok(Err(err)) => error!("failed to save pool, error: {:?}", err),
            Err(err) => error!("tx-pool persistence writer failed to join: {:?}", err),
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
        let all_entry_info = self.all_entry_info().await;
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
    /// Conservative resolved payload residency, computed once at resolve.
    pub(crate) resident_size: usize,
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
    let rtx = crate::resolved_tx::compact_resolved_transaction_for_residency(rtx);
    let resident_size = crate::component::entry::resolved_transaction_charge_bytes(tx_size, &rtx);
    PreCheckedTx {
        pre_resolve_tip,
        rtx,
        status,
        fee,
        tx_size,
        resident_size,
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
