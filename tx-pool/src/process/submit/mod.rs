//! Submission entry and verification orchestration.
//!
//! This module carries the pipeline's final stage entry points:
//! `verify_and_submit_core` (script verification and the transition into the
//! write-locked commit), `submit_entry` (the authoritative commit dispatch),
//! `post_submit_side_effects`, and the `test_accept_tx` helpers. The
//! write-lock commit transaction family (RBF prepare / try / commit) lives in
//! [`rbf_commit`].

pub(crate) mod rbf_commit;

use crate::component::entry::TxEntry;
use crate::error::Reject;
use crate::service::TxPoolService;
use crate::util::verify_rtx;
use ckb_logger::{info, warn};
use ckb_script::ChunkCommand;
use ckb_types::core::TransactionView;
use ckb_types::packed::Byte32;
use ckb_verification::cache::{CacheEntry, Completed};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::watch;

use super::{PreCheckedTx, status_to_verify_env};

/// Result of [`TxPoolService::submit_entry`] for a transaction that passed
/// verification.
pub(crate) enum SubmitEntryResult {
    /// The transaction was committed to the pool.
    Committed,
    /// Administrative clear invalidated the job before it could commit.
    Cleared,
}

/// Result of [`TxPoolService::verify_and_submit_core`].
pub(crate) enum VerifySubmitOutcome {
    /// Verified and committed to the pool.
    Committed(Completed),
    /// Administrative clear invalidated the job. This is not a transaction
    /// rejection and must not be recorded in recent-reject.
    Cleared,
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
        let epoch = resolved.epoch;
        let original_peer = resolved.source.peer();
        let pre_resolve_tip = resolved.pre_resolve_tip.clone();
        let entry = TxEntry::new(
            Arc::clone(&resolved.rtx),
            verified_cycles,
            resolved.fee,
            resolved.tx_size,
        );
        let entry_id = entry.proposal_short_id();
        let tx_hash = entry.transaction().hash();

        if !self.is_pipeline_epoch_current(epoch) {
            return Ok(SubmitEntryResult::Cleared);
        }
        let effect_permit = self
            .reserve_effects(self.max_submit_effect_bytes())
            .await
            .map_err(|error| {
                Reject::Internal(format!("effect outbox reservation failed: {error:?}"))
            })?;
        if !self.is_pipeline_epoch_current(epoch) {
            return Ok(SubmitEntryResult::Cleared);
        }
        // Synchronous local/reorg submissions do not participate in a second
        // speculative RBF owner. All remote/proposal pipeline competition is
        // verified-only in the coordinator, while the authoritative complete
        // replacement closure is recalculated here under the pool write lock.
        let outcome = {
            let mut tx_pool = self.pool.tx_pool.write().await;
            if !self.is_pipeline_epoch_current(epoch) {
                return Ok(SubmitEntryResult::Cleared);
            }
            let snapshot = tx_pool.cloned_snapshot();
            let mut coordinated = self.try_submit_entry_coordinated(
                &mut tx_pool,
                snapshot,
                pre_resolve_tip,
                entry.clone(),
                status,
                entry_id.clone(),
            );

            // Local and reorg recovery are deliberately synchronous and do
            // not lease work from the coordinator. Once their authoritative
            // pool insertion succeeds, invalidate any older remote/proposal
            // owner while the pool write guard is still held. All combined
            // readers take TxPool -> coordinator in the same order, so they
            // cannot observe a handoff gap or dual membership.
            //
            // `post_submit_side_effects` repeats this transition after the
            // lock is released. That call is an idempotent retry for an
            // internal coordinator failure, not the primary handoff.
            let mut committed_ingress_peer = original_peer;
            if coordinated.outcome.result.is_ok() {
                let removed_parents = coordinated.removed_parent_hashes();
                let finalized = catch_unwind(AssertUnwindSafe(|| {
                    self.pipeline.runtime.mutate(|coordinator| {
                        coordinator
                            .external_commit_with_unavailable_parents(&tx_hash, &removed_parents)
                    })
                }));
                let finalize_error = match finalized {
                    Ok(Ok(record)) => {
                        if let Some(record) = record {
                            committed_ingress_peer =
                                record.raw.ingress_peer().or(committed_ingress_peer);
                        }
                        None
                    }
                    Ok(Err(error)) => Some(Reject::Internal(format!(
                        "synchronous coordinator handoff failed: {error:?}"
                    ))),
                    Err(payload) => Some(Reject::Internal(format!(
                        "synchronous coordinator handoff panicked: {}",
                        crate::util::panic_payload_to_string(payload.as_ref())
                    ))),
                };
                if let Some(reject) = finalize_error
                    && let Err(rollback_error) = self.rollback_coordinated_submit(
                        &mut tx_pool,
                        &entry,
                        &mut coordinated,
                        reject,
                    )
                {
                    warn!(
                        "failed to roll back synchronous pool commit {}: {}",
                        tx_hash, rollback_error
                    );
                    coordinated.outcome.result = Err(rollback_error);
                }
            }
            let extra_effects = coordinated
                .outcome
                .result
                .is_ok()
                .then(|| {
                    crate::service::effects::TxPoolEffect::Relay(
                        crate::service::TxVerificationResult::Ok {
                            original_peer: committed_ingress_peer,
                            tx_hash: tx_hash.clone(),
                        },
                    )
                })
                .into_iter()
                .collect();
            for status in coordinated.block_assembler_statuses() {
                self.journal_block_assembler_update(status);
            }
            self.journal_submit_effects(&mut coordinated.outcome, effect_permit, extra_effects);
            coordinated.outcome
        };
        outcome.result?;
        Ok(SubmitEntryResult::Committed)
    }
    pub(crate) async fn test_accept_tx(&self, tx: TransactionView) -> Result<Completed, Reject> {
        self.check_tx_basic_validity(&tx).await?;
        self.test_accept_tx_core(tx.clone()).await
    }
    /// Run script verification for a coordinator-owned resolved payload
    /// without changing lifecycle or pool membership. The caller must settle
    /// the versioned verify lease with `complete_verification*` or a terminal
    /// transition after this future returns.
    pub(crate) async fn verify_pipeline_resolved(
        &self,
        mut resolved: crate::resolved_tx::ResolvedTx,
        command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    ) -> Result<crate::component::pipeline_runtime::PipelineVerifiedTx, Reject> {
        let declared_cycles = resolved.source.cycles();
        let verify_cache = match resolved.verified {
            Some(verified) => Some(verified),
            None => self.fetch_tx_verify_cache(&resolved.tx).await,
        };
        let max_cycles = declared_cycles.unwrap_or_else(|| self.pool.consensus.max_block_cycles());
        let tip_header = resolved.snapshot.tip_header();
        let tx_env = Arc::new(status_to_verify_env(resolved.status, tip_header));
        let started_at = Instant::now();
        let verified = verify_rtx(
            Arc::clone(&resolved.snapshot),
            Arc::clone(&resolved.rtx),
            tx_env,
            &verify_cache,
            max_cycles,
            command_rx,
        )
        .await?;

        if let Some(declared) = declared_cycles
            && declared != verified.cycles
        {
            info!(
                "declared cycles not match verified cycles, declared: {}, verified: {}, tx_hash: {}",
                declared,
                verified.cycles,
                resolved.tx.hash()
            );
            return Err(Reject::DeclaredWrongCycles(declared, verified.cycles));
        }

        let verify_cache_hit = verify_cache.is_some();
        resolved.verified = Some(verified);
        Ok(crate::component::pipeline_runtime::PipelineVerifiedTx {
            resolved,
            completed: verified,
            verify_cache_hit,
            started_at,
        })
    }
    /// Non-authoritative maintenance after a transaction has been submitted:
    /// retry the idempotent coordinator wake, enqueue a verify-cache update,
    /// and record metrics. The block-assembler delta is already journaled
    /// synchronously inside the pool commit transaction.
    pub(crate) async fn post_submit_side_effects(
        &self,
        verified: Completed,
        verify_cache_hit: bool,
        tx_hash: &Byte32,
        wtx_hash: &Byte32,
        is_sync_process: bool,
        instant: Instant,
    ) {
        if let Err(error) = self
            .pipeline
            .runtime
            .mutate(|coordinator| coordinator.external_commit(tx_hash))
        {
            warn!(
                "failed to wake coordinator children of committed {}: {:?}",
                tx_hash, error
            );
        }
        if !verify_cache_hit {
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
                .verify_cache_sender
                .try_send(crate::service::VerifyCacheUpdate {
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
            epoch,
            verified: carried_verified,
        } = resolved;
        let declared_cycles = source.cycles();
        // Verification uses the snapshot captured at resolve time. If the chain
        // tip has advanced since then (detected via pre_resolve_tip != tip_hash),
        // prepare_rbf_replacement re-runs check_rtx + time_relative_verify against
        // the current snapshot to catch any state-dependent invalidation.
        let tx_hash = tx.hash();
        let wtx_hash = tx.witness_hash();
        let instant = Instant::now();
        let is_sync_process = command_rx.is_none();

        let verify_cache = match carried_verified {
            Some(verified) => Some(verified),
            None => self.fetch_tx_verify_cache(&tx).await,
        };
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
                if !self.is_pipeline_epoch_current(epoch) {
                    return Ok(VerifySubmitOutcome::Cleared);
                }
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
            if !self.is_pipeline_epoch_current(epoch) {
                return Ok(VerifySubmitOutcome::Cleared);
            }
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
                    epoch,
                    verified: Some(verified),
                },
                entry_cycles,
            )
            .await?;

        match submit_result {
            SubmitEntryResult::Committed => {
                self.post_submit_side_effects(
                    verified,
                    verify_cache.is_some(),
                    &tx_hash,
                    &wtx_hash,
                    is_sync_process,
                    instant,
                )
                .await;
                Ok(VerifySubmitOutcome::Committed(verified))
            }
            SubmitEntryResult::Cleared => Ok(VerifySubmitOutcome::Cleared),
        }
    }

    /// Drain a bounded ordered slice of verified coordinator work. The
    /// serial guard is acquired before any lease is checked out, so two
    /// verify workers cannot invert commit order while awaiting `TxPool`.
    pub(crate) async fn drive_pipeline_commits(&self) {
        const MAX_COMMITS_PER_DRIVE: usize = 64;
        let _driver = self.pipeline.runtime.lock_commit_driver().await;
        for _ in 0..MAX_COMMITS_PER_DRIVE {
            let effect_permit = match self.reserve_effects(self.max_submit_effect_bytes()).await {
                Ok(permit) => permit,
                Err(error) => {
                    warn!("pipeline commit effect reservation failed: {:?}", error);
                    break;
                }
            };
            let lease = match self
                .pipeline
                .runtime
                .mutate(|coordinator| coordinator.begin_next_commit())
            {
                Ok(Some(lease)) => lease,
                Ok(None) => break,
                Err(error) => {
                    warn!("pipeline commit checkout failed: {:?}", error);
                    break;
                }
            };
            self.commit_pipeline_lease(lease, effect_permit).await;
        }
    }

    async fn commit_pipeline_lease(
        &self,
        lease: crate::component::pipeline_coordinator::CommitLease<
            crate::component::pipeline_runtime::PipelineVerifiedTx,
        >,
        effect_permit: crate::service::effects::EffectPermit,
    ) {
        use crate::component::pipeline_coordinator::TerminalDisposition;

        let verified = Arc::clone(&lease.payload);
        let entry = TxEntry::new(
            Arc::clone(&verified.resolved.rtx),
            verified.completed.cycles,
            verified.resolved.fee,
            verified.resolved.tx_size,
        );
        let entry_id = entry.proposal_short_id();
        let mut settlement = None;
        let mut failed_terminal = None;
        let mut internal_failure = false;
        let mut failed_banned_peer = None;
        let coordinated = {
            let mut tx_pool = self.pool.tx_pool.write().await;
            let snapshot = tx_pool.cloned_snapshot();
            let mut coordinated = self.try_submit_entry_coordinated(
                &mut tx_pool,
                snapshot,
                verified.resolved.pre_resolve_tip.clone(),
                entry.clone(),
                verified.resolved.status,
                entry_id.clone(),
            );

            if coordinated.outcome.result.is_ok() {
                let finalized = catch_unwind(AssertUnwindSafe(|| {
                    self.pipeline.runtime.mutate(|coordinator| {
                        coordinator.commit_any_handoff_with_unavailable_parents(
                            &lease,
                            &coordinated.removed_parent_hashes(),
                        )
                    })
                }));
                match finalized {
                    Ok(Ok(handoff)) => settlement = Some(handoff),
                    Ok(Err(error)) => {
                        internal_failure = true;
                        let reject = Reject::Internal(format!(
                            "coordinator commit finalization failed: {error:?}"
                        ));
                        if let Err(rollback_error) = self.rollback_coordinated_submit(
                            &mut tx_pool,
                            &entry,
                            &mut coordinated,
                            reject,
                        ) {
                            warn!(
                                "failed to roll back pool after coordinator finalization error: {}",
                                rollback_error
                            );
                            coordinated.outcome.result = Err(rollback_error);
                        }
                    }
                    Err(payload) => {
                        internal_failure = true;
                        let reject = Reject::Internal(format!(
                            "coordinator commit finalization panicked: {}",
                            crate::util::panic_payload_to_string(payload.as_ref())
                        ));
                        if let Err(rollback_error) = self.rollback_coordinated_submit(
                            &mut tx_pool,
                            &entry,
                            &mut coordinated,
                            reject,
                        ) {
                            warn!(
                                "failed to roll back pool after coordinator finalization panic: {}",
                                rollback_error
                            );
                            coordinated.outcome.result = Err(rollback_error);
                        }
                    }
                }
            }

            if coordinated.outcome.result.is_err() && settlement.is_none() {
                match self.pipeline.runtime.mutate(|coordinator| {
                    coordinator.fail_commit(&lease, TerminalDisposition::Rejected)
                }) {
                    Ok(record) => failed_terminal = Some(record),
                    Err(error) => warn!(
                        "failed to terminalize rejected coordinator commit {}: {:?}",
                        lease.hash, error
                    ),
                }
            }
            let mut extra_effects = Vec::new();
            if let Some(handoff) = &settlement {
                extra_effects.push(crate::service::effects::TxPoolEffect::Relay(
                    crate::service::TxVerificationResult::Ok {
                        original_peer: handoff.winner.raw.ingress_peer(),
                        tx_hash: verified.resolved.tx.hash(),
                    },
                ));

                let reject =
                    Reject::RBFRejected(Self::SUPERSEDED_BY_HIGHER_FEE_CANDIDATE.to_string());
                for record in &handoff.rejected {
                    let Ok(source) = record.raw.authoritative_source(record.source) else {
                        continue;
                    };
                    // The winner now owns the conflicting input. Retain the
                    // verified loser for bounded future recovery before the
                    // publication journal makes its terminal outcome visible.
                    tx_pool.record_conflict(record.raw.tx.clone(), source);
                    if reject.should_recorded() {
                        self.record_recent_reject(&record.hash, &reject);
                    }
                    if record.raw.ingress_peer().is_some() && reject.is_allowed_relay() {
                        extra_effects.push(crate::service::effects::TxPoolEffect::Relay(
                            crate::service::TxVerificationResult::Reject {
                                tx_hash: record.hash.clone(),
                            },
                        ));
                    }
                }
            } else if let Some(record) = &failed_terminal {
                match record.raw.authoritative_source(record.source) {
                    Ok(source) => {
                        if internal_failure {
                            if record.raw.ingress_peer().is_some() {
                                extra_effects.push(crate::service::effects::TxPoolEffect::Relay(
                                    crate::service::TxVerificationResult::Reject {
                                        tx_hash: record.hash.clone(),
                                    },
                                ));
                            }
                        } else if let Some(reject) = coordinated.outcome.result.as_ref().err() {
                            if matches!(
                                reject,
                                Reject::RBFRejected(..)
                                    | Reject::Resolve(ckb_types::core::error::OutPointError::Dead(
                                        _
                                    ))
                            ) && tx_pool
                                .pool_map
                                .find_conflict_outpoint(&record.raw.tx)
                                .is_some()
                            {
                                tx_pool.record_conflict(record.raw.tx.clone(), source);
                            }
                            if reject.should_recorded() {
                                self.record_recent_reject(&record.hash, reject);
                            }
                            if let Some(peer) = record.raw.ingress_peer() {
                                if reject.is_malformed_tx() {
                                    let duration = std::time::Duration::from_secs(
                                        crate::constants::MALFORMED_TX_BAN_SECONDS,
                                    );
                                    self.record_peer_ban(peer, duration);
                                    extra_effects.push(
                                        crate::service::effects::TxPoolEffect::BanPeer {
                                            peer,
                                            duration,
                                            reason: format!("reject {reject}"),
                                        },
                                    );
                                    failed_banned_peer = Some(peer);
                                }
                                if reject.is_allowed_relay()
                                    && !matches!(reject, Reject::Duplicated(_))
                                {
                                    extra_effects.push(
                                        crate::service::effects::TxPoolEffect::Relay(
                                            crate::service::TxVerificationResult::Reject {
                                                tx_hash: record.hash.clone(),
                                            },
                                        ),
                                    );
                                }
                            }
                        }
                    }
                    Err(error) => {
                        warn!(
                            "cannot attribute rejected coordinator commit {}: {}",
                            record.hash, error
                        );
                        extra_effects.push(crate::service::effects::TxPoolEffect::Relay(
                            crate::service::TxVerificationResult::Reject {
                                tx_hash: record.hash.clone(),
                            },
                        ));
                    }
                }
            }
            for status in coordinated.block_assembler_statuses() {
                self.journal_block_assembler_update(status);
            }
            self.journal_submit_effects(&mut coordinated.outcome, effect_permit, extra_effects);
            coordinated
        };

        let dispatch_result = coordinated.outcome.result;
        if let Some(peer) = failed_banned_peer {
            self.remove_banned_peer_entries(peer).await;
        }
        match (dispatch_result, settlement) {
            (Ok(()), Some(settlement)) => {
                self.post_submit_side_effects(
                    verified.completed,
                    verified.verify_cache_hit,
                    &verified.resolved.tx.hash(),
                    &verified.resolved.tx.witness_hash(),
                    false,
                    verified.started_at,
                )
                .await;
                let _ = settlement;
            }
            (Err(_reject), _) => {}
            (Ok(()), None) => {
                warn!(
                    "pipeline submit {} succeeded without a coordinator handoff",
                    entry_id
                );
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
