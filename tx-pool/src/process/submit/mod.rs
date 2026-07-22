//! Submission entry and verification orchestration.
//!
//! This module carries the pipeline's final stage entry points:
//! `verify_and_submit_tx` / `verify_and_submit_core` (script verification
//! and the transition into the write-locked commit), `submit_entry` (the
//! superseded gate and the commit dispatch), `post_submit_side_effects`,
//! and the `test_accept_tx` helpers. The write-lock commit transaction
//! family (RBF prepare / try / commit / aftermath) lives in
//! [`rbf_commit`].

pub(crate) mod rbf_commit;

use crate::component::entry::TxEntry;
use crate::component::pool_map::Status;
use crate::error::Reject;
use crate::service::TxPoolService;
use crate::util::verify_rtx;
use ckb_logger::{info, warn};
use ckb_script::ChunkCommand;
use ckb_types::core::TransactionView;
use ckb_types::packed::Byte32;
use ckb_verification::cache::{CacheEntry, Completed};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::watch;

use super::{PreCheckedTx, status_to_verify_env};

/// Result of [`TxPoolService::submit_entry`] for a transaction that passed
/// verification.
pub(crate) enum SubmitEntryResult {
    /// The transaction was committed to the pool.
    Committed,
    /// The transaction was superseded by a stronger in-flight RBF
    /// registration and is now *held* by it — its fate follows the
    /// winner's (finalize → rejected, abort → restored).
    Superseded,
}

/// Result of [`TxPoolService::verify_and_submit_core`].
pub(crate) enum VerifySubmitOutcome {
    /// Verified and committed to the pool.
    Committed(Completed),
    /// Verified, but superseded by a stronger in-flight RBF registration
    /// and held by it. Not a rejection: no `after_process` side effects
    /// (mirrors register-time displacement).
    Superseded,
}

impl TxPoolService {
    pub(crate) async fn fetch_tx_verify_cache(&self, tx: &TransactionView) -> Option<CacheEntry> {
        let guard = self.aux.txs_verify_cache.read().await;
        guard.peek(&tx.witness_hash()).cloned()
    }

    pub(crate) async fn submit_entry(
        &self,
        resolved: crate::resolved_tx::ResolvedTx,
        verified_cycles: ckb_types::core::Cycle,
    ) -> Result<SubmitEntryResult, Reject> {
        let status = resolved.status;
        let pre_resolve_tip = resolved.pre_resolve_tip.clone();
        let entry = TxEntry::new(
            Arc::clone(&resolved.rtx),
            verified_cycles,
            resolved.fee,
            resolved.tx_size,
        );
        let entry_id = entry.proposal_short_id();

        // Hold the RBF read guard from the conflict computation through the
        // submit: the conflict set and the superseded check are then read
        // against the same registration state, and no higher-fee-rate
        // candidate can register in between. The lock ordering here is rbf
        // (read) -> tx_pool (write), consistent with the intended pipeline
        // hierarchy.
        //
        // The gate stores size-based fee rates (see
        // `compute_size_based_fee_rate`), so the entry must be compared in
        // the same unit, not via `TxEntry::fee_rate` (which is
        // weight-based).
        let entry_fee_rate = self.compute_size_based_fee_rate(entry.fee, entry.size);
        let rbf_guard = self.pipeline.queues.rbf_candidates.read().await;
        let conflict_inputs = self.find_conflict_inputs(entry.transaction()).await;

        if !conflict_inputs.is_empty() {
            if rbf_guard.is_superseded(&entry_id, entry_fee_rate, &conflict_inputs) {
                drop(rbf_guard);
                // The superseding decision is speculative — the winner is
                // still unverified. Become held by the winner's registration
                // instead of being rejected: the winner's fate decides this
                // candidate's fate (finalize → really rejected; abort →
                // restored to the verify queue). This mirrors the
                // register-time displacement hold and closes the residual
                // censorship window for candidates that were already active
                // (mid-verification) when a stronger candidate appeared.
                self.hold_superseded_candidate(resolved, conflict_inputs)
                    .await;
                return Ok(SubmitEntryResult::Superseded);
            }

            let mut tx_pool = self.pool.tx_pool.write().await;
            let snapshot = tx_pool.cloned_snapshot();
            let outcome = self.try_submit_entry(
                &mut tx_pool,
                Arc::clone(&snapshot),
                pre_resolve_tip,
                entry,
                status,
                entry_id.clone(),
            );
            drop(tx_pool);
            drop(rbf_guard);

            self.dispatch_submit_aftermath(&entry_id, outcome).await?;
            Ok(SubmitEntryResult::Committed)
        } else {
            // Separate the successful result from the collected reject events and
            // recovered txs. Reject callbacks must be dispatched and displaced txs
            // must be recovered even if the closure returns an error after
            // `process_rbf` has already removed old transactions (e.g. the
            // replacement fails the pool ancestor/size limits). Without this, a
            // remote peer can evict in-pool txs via a crafted RBF replacement that
            // is itself rejected, leaving the node with neither transaction.
            let outcome = {
                let mut tx_pool = self.pool.tx_pool.write().await;
                let snapshot = tx_pool.cloned_snapshot();
                let outcome = self.try_submit_entry(
                    &mut tx_pool,
                    snapshot,
                    pre_resolve_tip,
                    entry,
                    status,
                    entry_id.clone(),
                );
                drop(tx_pool);
                outcome
            };
            drop(rbf_guard);

            self.dispatch_submit_aftermath(&entry_id, outcome).await?;
            Ok(SubmitEntryResult::Committed)
        }
    }
    pub(crate) async fn test_accept_tx(&self, tx: TransactionView) -> Result<Completed, Reject> {
        self.check_tx_basic_validity(&tx).await?;
        self.test_accept_tx_core(tx.clone()).await
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
    ) -> Result<VerifySubmitOutcome, Reject> {
        self.verify_and_submit_core(resolved, command_rx).await
    }
    /// Side effects run after a transaction has been successfully submitted to
    /// the pool: notify the block assembler, wake the ordered resolver, enqueue
    /// a verify cache update, and record metrics.
    pub(crate) async fn post_submit_side_effects(
        &self,
        status: Status,
        verified: Completed,
        verify_cache: Option<CacheEntry>,
        wtx_hash: &Byte32,
        is_sync_process: bool,
        instant: Instant,
    ) {
        self.notify_block_assembler(status).await;

        // A newly submitted transaction may resolve dependent transactions that
        // are waiting in the ordered resolve queue (e.g. children of a parent
        // that was just re-added after a reorg). Wake the ordered resolver so
        // those children can be retried promptly.
        self.wake_ordered_resolver_if_needed().await;

        if verify_cache.is_none() {
            self.defer_cache_update(wtx_hash, verified);
        }

        if let Some(metrics) = ckb_metrics::handle() {
            let elapsed = instant.elapsed().as_secs_f64();
            if is_sync_process {
                metrics.ckb_tx_pool_sync_process.observe(elapsed);
            } else {
                metrics.ckb_tx_pool_async_process.observe(elapsed);
            }
        }
    }
    /// Defer a verify-cache update to the background worker (rather than
    /// spawning a fire-and-forget task).
    pub(crate) fn defer_cache_update(&self, wtx_hash: &Byte32, verified: Completed) {
        if let Err(e) =
            self.pipeline
                .deferred_sender
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

    /// Shared core: verify a resolved transaction and submit it to the pool.
    ///
    /// Both `process_tx_direct` (reorg recovery / local RPC path) and
    /// `verify_and_submit_tx` (pipeline verify path) converge here after the
    /// resolve step.
    pub(crate) async fn verify_and_submit_core(
        &self,
        resolved: crate::resolved_tx::ResolvedTx,
        command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    ) -> Result<VerifySubmitOutcome, Reject> {
        let crate::resolved_tx::ResolvedTx {
            tx,
            rtx,
            status,
            fee,
            tx_size,
            pre_resolve_tip,
            snapshot,
            source,
            resident_permit,
        } = resolved;
        let declared_cycles = source.cycles();
        // Verification uses the snapshot captured at resolve time. If the chain
        // tip has advanced since then (detected via pre_resolve_tip != tip_hash),
        // prepare_rbf_replacement re-runs check_rtx + time_relative_verify against
        // the current snapshot to catch any state-dependent invalidation.
        let wtx_hash = tx.witness_hash();
        let instant = Instant::now();
        let is_sync_process = command_rx.is_none();

        let verify_cache = self.fetch_tx_verify_cache(&tx).await;
        let max_cycles = declared_cycles.unwrap_or_else(|| self.pool.consensus.max_block_cycles());
        let tip_header = snapshot.tip_header();
        let tx_env = Arc::new(status_to_verify_env(status, tip_header));

        let verified_ret = verify_rtx(
            Arc::clone(&snapshot),
            Arc::clone(&rtx),
            tx_env,
            &verify_cache,
            max_cycles,
            command_rx,
        )
        .await;

        let verified = match verified_ret {
            Ok(v) => v,
            Err(err) => {
                self.abort_rbf_candidate(&tx.proposal_short_id()).await;
                return Err(err);
            }
        };

        if let Some(declared) = declared_cycles
            && declared != verified.cycles
        {
            info!(
                "declared cycles not match verified cycles, declared: {}, verified: {}, tx_hash: {}",
                declared,
                verified.cycles,
                tx.hash()
            );
            self.abort_rbf_candidate(&tx.proposal_short_id()).await;
            return Err(Reject::DeclaredWrongCycles(declared, verified.cycles));
        }

        let entry_cycles = verified.cycles;
        let submit_result = self
            .submit_entry(
                crate::resolved_tx::ResolvedTx {
                    tx,
                    rtx,
                    status,
                    fee,
                    tx_size,
                    pre_resolve_tip,
                    snapshot,
                    source,
                    resident_permit,
                },
                entry_cycles,
            )
            .await?;

        match submit_result {
            SubmitEntryResult::Committed => {
                self.post_submit_side_effects(
                    status,
                    verified,
                    verify_cache,
                    &wtx_hash,
                    is_sync_process,
                    instant,
                )
                .await;
                Ok(VerifySubmitOutcome::Committed(verified))
            }
            // Held by the stronger in-flight registration: its fate follows
            // the winner's. Not a rejection — no after_process side effects.
            SubmitEntryResult::Superseded => {
                // Keep the verified cycles in the cache: when the winner
                // later aborts and this candidate is restored, it must not
                // pay for a full script re-verification.
                if verify_cache.is_none() {
                    self.defer_cache_update(&wtx_hash, verified);
                }
                Ok(VerifySubmitOutcome::Superseded)
            }
        }
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
        let max_cycles = self.pool.consensus.max_block_cycles();
        let tip_header = snapshot.tip_header();
        let tx_env = Arc::new(status_to_verify_env(status, tip_header));

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
}
