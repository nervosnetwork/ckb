//! Submission entry and verification orchestration.
//!
//! This module carries the pipeline's final stage entry points:
//! `verify_and_submit_core` (script verification and the transition into the
//! write-locked commit), `submit_entry` (the authoritative commit dispatch),
//! `post_submit_side_effects`, and the `test_accept_tx` helpers. The
//! write-lock Plan/handoff/Apply transaction family lives in
//! [`rbf_commit`].

pub(crate) mod rbf_commit;

use crate::component::entry::TxEntry;
use crate::component::pre_pool::{PipelineVerifiedTx, PrePoolError, PrePoolFault};
use crate::error::Reject;
use crate::service::TxPoolService;
use crate::service::effects::{EffectCapacityWaitError, EffectClass, EffectJournalError};
use crate::util::verify_rtx;
use ckb_logger::{info, warn};
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
use ckb_types::core::TransactionView;
use ckb_types::packed::Byte32;
use ckb_verification::cache::{CacheEntry, Completed, TxVerificationCacheKey};
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

pub(crate) enum SubmissionError {
    Rejected(Reject),
    Fault(PipelineCommitFault),
}

/// Result of one read-only Plan plus atomic Apply attempt for the current
/// highest-ranked Ready owner. No session or authority lock survives this
/// value: a backpressured driver waits and then replans from current state.
enum PipelineCommitStep {
    Progress,
    Idle,
    Backpressured { bytes: usize, class: EffectClass },
    Closed,
    Fault(PipelineCommitFault),
}

pub(crate) enum PipelineCommitFault {
    Epoch(crate::service::PipelineEpochExhausted),
    Kernel(PrePoolFault),
    Pool(crate::component::pool_map::PoolMutationFault),
    Effect(EffectJournalError),
    EffectBuild(crate::service::effects::EffectBuildError),
}

impl std::fmt::Debug for PipelineCommitFault {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Epoch(error) => formatter.debug_tuple("Epoch").field(error).finish(),
            Self::Kernel(error) => formatter.debug_tuple("Kernel").field(error).finish(),
            Self::Pool(error) => formatter.debug_tuple("Pool").field(error).finish(),
            Self::Effect(error) => formatter.debug_tuple("Effect").field(error).finish(),
            Self::EffectBuild(error) => formatter.debug_tuple("EffectBuild").field(error).finish(),
        }
    }
}

impl TxPoolService {
    pub(crate) async fn fetch_tx_verify_cache(&self, tx: &TransactionView) -> Option<CacheEntry> {
        let key = TxVerificationCacheKey::from_transaction(tx);
        let guard = self.aux.txs_verify_cache.read().await;
        guard.peek(&key).cloned()
    }

    pub(crate) async fn submit_entry(
        &self,
        candidate: crate::resolved_tx::PoolCandidate,
        verified_cycles: ckb_types::core::Cycle,
    ) -> Result<SubmitEntryResult, SubmissionError> {
        let epoch = candidate.epoch;
        let source = candidate.source;
        let original_peer = candidate.source.peer();
        let pre_resolve_tip = candidate.pre_resolve_tip.clone();
        let entry = TxEntry::new_with_resident_size(
            Arc::clone(&candidate.rtx),
            verified_cycles,
            candidate.fee,
            candidate.tx_size,
            candidate.resident_size,
        );
        // Synchronous local/reorg submissions do not participate in a second
        // speculative RBF owner. All remote/proposal pipeline competition is
        // verified-only in the kernel, while the authoritative complete
        // replacement closure is recalculated here under the pool write lock.
        loop {
            if !self.is_pipeline_epoch_current(epoch) {
                return Ok(SubmitEntryResult::Cleared);
            }
            let mut tx_pool = self.pool.tx_pool.write().await;
            if !self.is_pipeline_epoch_current(epoch) {
                return Ok(SubmitEntryResult::Cleared);
            }
            enum Attempt {
                Rejected(Reject),
                Applied(
                    Result<(), rbf_commit::AdmissionApplyError>,
                    usize,
                    EffectClass,
                ),
                Fault(PipelineCommitFault),
            }
            let attempt = {
                let snapshot = tx_pool.cloned_snapshot();
                self.pipeline.kernel.mutate_authoritative(|kernel| {
                    let plan = match self.plan_external_admission(
                        &mut tx_pool,
                        kernel,
                        snapshot,
                        pre_resolve_tip.clone(),
                        entry.clone(),
                        source,
                        original_peer,
                        epoch,
                    ) {
                        Ok(plan) => plan,
                        Err(rbf_commit::AdmissionPlanningError::Policy(reject)) => {
                            if matches!(
                                &reject,
                                Reject::RBFRejected(..)
                                    | Reject::Resolve(ckb_types::core::error::OutPointError::Dead(
                                        _
                                    ))
                            ) && tx_pool
                                .pool_map
                                .find_conflict_outpoint(entry.transaction())
                                .is_some()
                            {
                                let tx = entry.transaction().clone();
                                let keys = crate::component::pre_pool::conflict_dependency_keys(
                                    &tx,
                                    entry.related_dep_out_points().cloned(),
                                );
                                let raw = crate::component::pre_pool::PipelineRawTx::new(
                                    tx, source, epoch,
                                );
                                let owner = crate::component::pre_pool::historical_source(source);
                                let expires_at =
                                    crate::component::pre_pool::historical_deadline(owner);
                                if let Err(error) = self.retain_optional_conflict(
                                    kernel,
                                    raw,
                                    owner,
                                    keys,
                                    expires_at,
                                    "direct-submit conflict retention failed",
                                ) {
                                    return Attempt::Fault(PipelineCommitFault::Kernel(
                                        error.into_unexpected_fault(),
                                    ));
                                }
                            }
                            return Attempt::Rejected(reject);
                        }
                        Err(rbf_commit::AdmissionPlanningError::Kernel(error)) => {
                            return Attempt::Fault(PipelineCommitFault::Kernel(error));
                        }
                        Err(rbf_commit::AdmissionPlanningError::Pool(error)) => {
                            return Attempt::Fault(PipelineCommitFault::Pool(error));
                        }
                        Err(rbf_commit::AdmissionPlanningError::Effect(error)) => {
                            return Attempt::Fault(PipelineCommitFault::EffectBuild(error));
                        }
                    };
                    let effect_bytes = plan.effect_bytes();
                    let effect_class = plan.effect_class();
                    Attempt::Applied(self.apply_admission_plan(plan), effect_bytes, effect_class)
                })
            };
            match attempt {
                Attempt::Rejected(reject) => return Err(SubmissionError::Rejected(reject)),
                Attempt::Fault(error) => {
                    return Err(SubmissionError::Fault(error));
                }
                Attempt::Applied(Ok(()), _, _) => return Ok(SubmitEntryResult::Committed),
                Attempt::Applied(
                    Err(rbf_commit::AdmissionApplyError::Journal(EffectJournalError::Full)),
                    bytes,
                    class,
                ) => {
                    drop(tx_pool);
                    match self.relay.effects.wait_capacity(bytes, class).await {
                        Ok(()) => {}
                        Err(EffectCapacityWaitError::Closed) => {
                            return Ok(SubmitEntryResult::Cleared);
                        }
                        Err(error) => {
                            return Err(SubmissionError::Fault(PipelineCommitFault::Effect(
                                error.into(),
                            )));
                        }
                    }
                }
                Attempt::Applied(Err(error), _, _) => {
                    let fault = match error {
                        rbf_commit::AdmissionApplyError::Journal(error) => {
                            PipelineCommitFault::Effect(error)
                        }
                        rbf_commit::AdmissionApplyError::Pool(error) => {
                            PipelineCommitFault::Pool(error)
                        }
                    };
                    return Err(SubmissionError::Fault(fault));
                }
            }
        }
    }
    pub(crate) async fn test_accept_tx(&self, tx: TransactionView) -> Result<Completed, Reject> {
        self.check_tx_basic_validity(&tx).await?;
        self.test_accept_tx_core(tx.clone()).await
    }
    /// Run script verification for a kernel-owned resolved payload
    /// without changing lifecycle or pool membership. The caller must settle
    /// the versioned verify lease with `complete_verification*` or a terminal
    /// transition after this future returns.
    pub(crate) async fn verify_pipeline_resolved(
        &self,
        resolved: crate::resolved_tx::ResolvedTx,
        snapshot: Arc<Snapshot>,
        command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    ) -> Result<crate::component::pre_pool::PipelineVerifiedTx, Reject> {
        let declared_cycles = resolved.source.cycles();
        let verify_cache = self.fetch_tx_verify_cache(resolved.transaction()).await;
        let max_cycles = declared_cycles.unwrap_or_else(|| self.pool.consensus.max_block_cycles());
        let tip_header = snapshot.tip_header();
        let tx_env = Arc::new(status_to_verify_env(resolved.status, tip_header));
        let started_at = Instant::now();
        let verified = verify_rtx(
            snapshot,
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
                resolved.transaction().hash()
            );
            return Err(Reject::DeclaredWrongCycles(declared, verified.cycles));
        }

        let verify_cache_hit = verify_cache.is_some();
        Ok(crate::component::pre_pool::PipelineVerifiedTx {
            candidate: resolved.into_pool_candidate(),
            completed: verified,
            verify_cache_hit,
            started_at,
        })
    }
    /// Non-authoritative maintenance after a transaction has been submitted:
    /// enqueue a verify-cache update and record metrics. Kernel ownership
    /// and child wakeup already settle atomically inside the pool commit; a
    /// second best-effort wake here would obscure failures in that boundary.
    pub(crate) async fn post_submit_side_effects(
        &self,
        verified: Completed,
        verify_cache_hit: bool,
        _tx_hash: &Byte32,
        verify_cache_key: TxVerificationCacheKey,
        is_sync_process: bool,
        instant: Instant,
    ) {
        if !verify_cache_hit {
            self.defer_cache_update(verify_cache_key, verified);
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
    pub(crate) fn defer_cache_update(&self, key: TxVerificationCacheKey, verified: Completed) {
        let display_key = key.as_witness_hash();
        if let Err(e) = self
            .pipeline
            .verify_cache_sender
            .try_send(crate::service::VerifyCacheUpdate { key, verified })
        {
            warn!(
                "failed to enqueue verify cache update for {}: {}",
                display_key, e
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
        snapshot: Arc<Snapshot>,
        command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    ) -> Result<VerifySubmitOutcome, SubmissionError> {
        let declared_cycles = resolved.source.cycles();
        // Verification uses the snapshot captured at resolve time. If the chain
        // tip has advanced since then (detected via pre_resolve_tip != tip_hash),
        // plan_pool_mutation re-runs check_rtx + time_relative_verify against
        // the current snapshot to catch any state-dependent invalidation.
        let tx_hash = resolved.transaction().hash();
        let verify_cache_key = TxVerificationCacheKey::from_transaction(resolved.transaction());
        let instant = Instant::now();
        let is_sync_process = command_rx.is_none();

        let verify_cache = self.fetch_tx_verify_cache(resolved.transaction()).await;
        let max_cycles = declared_cycles.unwrap_or_else(|| self.pool.consensus.max_block_cycles());
        let tip_header = snapshot.tip_header();
        let tx_env = Arc::new(status_to_verify_env(resolved.status, tip_header));

        let verified_ret = verify_rtx(
            snapshot,
            Arc::clone(&resolved.rtx),
            tx_env,
            &verify_cache,
            max_cycles,
            command_rx,
        )
        .await;

        let verified = match verified_ret {
            Ok(v) => v,
            Err(err) => {
                if !self.is_pipeline_epoch_current(resolved.epoch) {
                    return Ok(VerifySubmitOutcome::Cleared);
                }
                return Err(SubmissionError::Rejected(err));
            }
        };

        if let Some(declared) = declared_cycles
            && declared != verified.cycles
        {
            info!(
                "declared cycles not match verified cycles, declared: {}, verified: {}, tx_hash: {}",
                declared,
                verified.cycles,
                resolved.transaction().hash()
            );
            if !self.is_pipeline_epoch_current(resolved.epoch) {
                return Ok(VerifySubmitOutcome::Cleared);
            }
            return Err(SubmissionError::Rejected(Reject::DeclaredWrongCycles(
                declared,
                verified.cycles,
            )));
        }

        let entry_cycles = verified.cycles;
        let submit_result = self
            .submit_entry(resolved.into_pool_candidate(), entry_cycles)
            .await?;

        match submit_result {
            SubmitEntryResult::Committed => {
                self.post_submit_side_effects(
                    verified,
                    verify_cache.is_some(),
                    &tx_hash,
                    verify_cache_key,
                    is_sync_process,
                    instant,
                )
                .await;
                Ok(VerifySubmitOutcome::Committed(verified))
            }
            SubmitEntryResult::Cleared => Ok(VerifySubmitOutcome::Cleared),
        }
    }

    /// Drain a bounded ordered slice of verified pre-pool work. The one commit
    /// worker opens each Ready commit session inside the same
    /// `TxPool -> PrePoolKernel` critical section that validates and applies
    /// it. Journal backpressure holds no state authority. Returns false only
    /// after journal shutdown or a structural fault.
    #[cfg_attr(
        feature = "profiling",
        tracing::instrument(
            name = "tx_pool.commit.drive",
            target = "ckb_tx_pool_profile",
            level = "trace",
            skip_all
        )
    )]
    pub(crate) async fn drive_pipeline_commits(&self) -> bool {
        const MAX_COMMITS_PER_DRIVE: usize = 64;
        for _ in 0..MAX_COMMITS_PER_DRIVE {
            match self.commit_next_pipeline_entry().await {
                PipelineCommitStep::Progress => {}
                PipelineCommitStep::Idle => return true,
                PipelineCommitStep::Closed => return false,
                PipelineCommitStep::Fault(error) => {
                    self.fail_tx_pool_generation(
                        "pipeline commit fault",
                        &crate::process::TxPoolGenerationFault::Commit(error),
                    );
                    return false;
                }
                PipelineCommitStep::Backpressured { bytes, class } => {
                    match self.relay.effects.wait_capacity(bytes, class).await {
                        Ok(()) => {}
                        Err(EffectCapacityWaitError::Closed) => return false,
                        Err(error) => {
                            self.fail_tx_pool_generation(
                                "a previously bounded pipeline effect batch became invalid",
                                &crate::process::TxPoolGenerationFault::Commit(
                                    PipelineCommitFault::Effect(error.into()),
                                ),
                            );
                            return false;
                        }
                    }
                }
            }
        }
        true
    }

    /// Select and commit one Ready candidate under the same TxPool write
    /// guard. The pool guard is the authoritative membership sequencer, so a
    /// Local/clear/reorg handoff cannot consume the Ready owner between
    /// selection and pool insertion. The non-copyable commit session keeps an
    /// exclusive kernel borrow through bounded Plan/Apply; no transient commit
    /// location or runtime stale-ticket branch exists.
    async fn commit_next_pipeline_entry(&self) -> PipelineCommitStep {
        let mut tx_pool = self.pool.tx_pool.write().await;
        let snapshot = tx_pool.cloned_snapshot();

        enum Applied {
            Committed(Result<(), rbf_commit::AdmissionApplyError>),
            Rejected(Result<Option<ckb_network::PeerIndex>, EffectJournalError>),
        }
        struct Attempt {
            applied: Applied,
            effect_bytes: usize,
            effect_class: EffectClass,
            verified: Arc<PipelineVerifiedTx>,
        }

        let attempt = self.pipeline.kernel.mutate_authoritative(|kernel| {
            // The session exclusively borrows the kernel from selection until
            // its accepted or rejected Plan is applied. No independent ticket
            // can escape this boundary and later be validated dynamically.
            let Some(mut session) = kernel
                .begin_next_commit()
                .map_err(PrePoolError::into_unexpected_fault)
                .map_err(PipelineCommitFault::Kernel)?
            else {
                return Ok(None);
            };
            let verified = Arc::clone(session.payload());
            let entry = TxEntry::new_with_resident_size(
                Arc::clone(&verified.candidate.rtx),
                verified.completed.cycles,
                verified.candidate.fee,
                verified.candidate.tx_size,
                verified.candidate.resident_size,
            );
            match self.plan_ready_admission(
                &mut tx_pool,
                &mut session,
                snapshot,
                verified.candidate.pre_resolve_tip.clone(),
                entry.clone(),
                verified.candidate.epoch,
            ) {
                Ok(plan) => {
                    let effect_bytes = plan.effect_bytes();
                    let effect_class = plan.effect_class();
                    Ok(Some(Attempt {
                        applied: Applied::Committed(self.apply_admission_plan(plan)),
                        effect_bytes,
                        effect_class,
                        verified,
                    }))
                }
                Err(error) => {
                    let reject = match error {
                        rbf_commit::ReadyAdmissionPlanningError::IngressRevoked => None,
                        rbf_commit::ReadyAdmissionPlanningError::Admission(
                            rbf_commit::AdmissionPlanningError::Policy(reject),
                        ) => Some(reject),
                        rbf_commit::ReadyAdmissionPlanningError::Admission(
                            rbf_commit::AdmissionPlanningError::Kernel(error),
                        ) => return Err(PipelineCommitFault::Kernel(error)),
                        rbf_commit::ReadyAdmissionPlanningError::Admission(
                            rbf_commit::AdmissionPlanningError::Pool(error),
                        ) => return Err(PipelineCommitFault::Pool(error)),
                        rbf_commit::ReadyAdmissionPlanningError::Admission(
                            rbf_commit::AdmissionPlanningError::Effect(error),
                        ) => return Err(PipelineCommitFault::EffectBuild(error)),
                    };
                    let cause = match reject.as_ref() {
                        Some(reject) => rbf_commit::UnacceptedReadyCause::Policy(reject),
                        None => rbf_commit::UnacceptedReadyCause::IngressRevoked,
                    };
                    let plan =
                        match self.plan_unaccepted_admission(&tx_pool, &mut session, &entry, cause)
                        {
                            Ok(plan) => plan,
                            Err(error) => return Err(PipelineCommitFault::Kernel(error)),
                        };
                    let effect_bytes = plan.effect_bytes();
                    let effect_class = plan.effect_class();
                    Ok(Some(Attempt {
                        applied: Applied::Rejected(self.apply_failed_admission(plan)),
                        effect_bytes,
                        effect_class,
                        verified,
                    }))
                }
            }
        });

        let attempt = match attempt {
            Ok(Some(attempt)) => attempt,
            Ok(None) => return PipelineCommitStep::Idle,
            Err(error) => return PipelineCommitStep::Fault(error),
        };
        let verified = attempt.verified;

        match attempt.applied {
            Applied::Committed(Ok(())) => {
                drop(tx_pool);
                self.post_submit_side_effects(
                    verified.completed,
                    verified.verify_cache_hit,
                    &verified.candidate.tx.hash(),
                    TxVerificationCacheKey::from_transaction(&verified.candidate.tx),
                    false,
                    verified.started_at,
                )
                .await;
                PipelineCommitStep::Progress
            }
            Applied::Rejected(Ok(banned_peer)) => {
                drop(tx_pool);
                if let Some(peer) = banned_peer {
                    self.remove_banned_peer_entries(peer).await;
                }
                PipelineCommitStep::Progress
            }
            Applied::Committed(Err(rbf_commit::AdmissionApplyError::Journal(
                EffectJournalError::Full,
            )))
            | Applied::Rejected(Err(EffectJournalError::Full)) => {
                drop(tx_pool);
                PipelineCommitStep::Backpressured {
                    bytes: attempt.effect_bytes,
                    class: attempt.effect_class,
                }
            }
            Applied::Committed(Err(rbf_commit::AdmissionApplyError::Journal(
                EffectJournalError::Closed,
            )))
            | Applied::Rejected(Err(EffectJournalError::Closed)) => PipelineCommitStep::Closed,
            Applied::Committed(Err(rbf_commit::AdmissionApplyError::Pool(error))) => {
                PipelineCommitStep::Fault(PipelineCommitFault::Pool(error))
            }
            Applied::Committed(Err(rbf_commit::AdmissionApplyError::Journal(error)))
            | Applied::Rejected(Err(error)) => {
                PipelineCommitStep::Fault(PipelineCommitFault::Effect(error))
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
