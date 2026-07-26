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
use crate::error::Reject;
use crate::service::TxPoolService;
use crate::service::effects::{
    EffectClass, EffectJournalError, TxPoolEffect, bounded_commit_ban_reason,
};
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
    ) -> Result<SubmitEntryResult, Reject> {
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
        let tx_hash = entry.transaction().hash();

        let effect_bound = self.max_submit_effect_bytes();
        let effect_class = if matches!(source, crate::tx_source::TxSource::Remote { .. }) {
            EffectClass::Remote
        } else {
            EffectClass::Trusted
        };
        // Synchronous local/reorg submissions do not participate in a second
        // speculative RBF owner. All remote/proposal pipeline competition is
        // verified-only in the coordinator, while the authoritative complete
        // replacement closure is recalculated here under the pool write lock.
        let outcome = loop {
            if !self.is_pipeline_epoch_current(epoch) {
                return Ok(SubmitEntryResult::Cleared);
            }
            if let Err(error) = self
                .relay
                .effects
                .wait_capacity(effect_bound, effect_class)
                .await
            {
                return Err(Reject::Full(format!(
                    "tx-pool effect journal unavailable: {error:?}"
                )));
            }
            let mut tx_pool = self.pool.tx_pool.write().await;
            if !self.is_pipeline_epoch_current(epoch) {
                return Ok(SubmitEntryResult::Cleared);
            }
            let applied = {
                let snapshot = tx_pool.cloned_snapshot();
                self.pipeline.kernel.mutate_authoritative(|kernel| {
                    self.relay
                        .effects
                        .try_apply_bounded(effect_bound, effect_class, || {
                            let mut committed_ingress_peer = original_peer;
                            let (mut outcome, record) = self.try_submit_entry_with_handoff(
                                &mut tx_pool,
                                snapshot,
                                pre_resolve_tip.clone(),
                                entry.clone(),
                                |tx_pool, plan| {
                                    let unavailable = self.planned_unavailable_parent_hashes(
                                        plan,
                                        tx_pool.snapshot(),
                                    );
                                    let record = kernel
                                        .external_commit_with_unavailable_parents(
                                            &tx_hash,
                                            &unavailable,
                                        )
                                        .unwrap_or_else(|error| {
                                            panic!("planned direct handoff failed: {error:?}")
                                        });
                                    self.settle_kernel_for_pool_plan(kernel, tx_pool, &entry, plan);
                                    record
                                },
                            );
                            if let Some(Some(record)) = record {
                                committed_ingress_peer =
                                    record.raw.ingress_peer().or(committed_ingress_peer);
                            }
                            if matches!(
                                outcome.result.as_ref(),
                                Err(Reject::RBFRejected(..)
                                    | Reject::Resolve(
                                        ckb_types::core::error::OutPointError::Dead(_)
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
                                self.retain_optional_conflict(
                                    kernel,
                                    raw,
                                    owner,
                                    keys,
                                    expires_at,
                                    "direct-submit conflict retention failed",
                                );
                            }
                            let extra_effects = outcome
                                .result
                                .is_ok()
                                .then(|| {
                                    TxPoolEffect::Relay(crate::service::TxVerificationResult::Ok {
                                        original_peer: committed_ingress_peer,
                                        tx_hash: tx_hash.clone(),
                                    })
                                })
                                .into_iter()
                                .collect();
                            for status in outcome.block_assembler_statuses() {
                                self.journal_block_assembler_update(status);
                            }
                            let batch = self.prepare_submit_effects(&mut outcome, extra_effects);
                            (outcome, batch)
                        })
                })
            };
            match applied {
                Ok(outcome) => break outcome,
                Err(EffectJournalError::Full) => continue,
                Err(error) => {
                    return Err(Reject::Full(format!(
                        "tx-pool effect journal unavailable: {error:?}"
                    )));
                }
            }
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
        resolved: crate::resolved_tx::ResolvedTx,
        snapshot: Arc<Snapshot>,
        command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    ) -> Result<crate::component::pre_pool::PipelineVerifiedTx, Reject> {
        let declared_cycles = resolved.source.cycles();
        let verify_cache = self.fetch_tx_verify_cache(&resolved.tx).await;
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
                resolved.tx.hash()
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
    /// enqueue a verify-cache update and record metrics. Coordinator ownership
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
    ) -> Result<VerifySubmitOutcome, Reject> {
        let declared_cycles = resolved.source.cycles();
        // Verification uses the snapshot captured at resolve time. If the chain
        // tip has advanced since then (detected via pre_resolve_tip != tip_hash),
        // plan_pool_mutation re-runs check_rtx + time_relative_verify against
        // the current snapshot to catch any state-dependent invalidation.
        let tx_hash = resolved.tx.hash();
        let verify_cache_key = TxVerificationCacheKey::from_transaction(&resolved.tx);
        let instant = Instant::now();
        let is_sync_process = command_rx.is_none();

        let verify_cache = self.fetch_tx_verify_cache(&resolved.tx).await;
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
                resolved.tx.hash()
            );
            if !self.is_pipeline_epoch_current(resolved.epoch) {
                return Ok(VerifySubmitOutcome::Cleared);
            }
            return Err(Reject::DeclaredWrongCycles(declared, verified.cycles));
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

    /// Drain a bounded ordered slice of verified pre-pool work. The
    /// serial guard is acquired before any lease is checked out, so two
    /// verify workers cannot invert commit order while awaiting `TxPool`.
    pub(crate) async fn drive_pipeline_commits(&self) {
        const MAX_COMMITS_PER_DRIVE: usize = 64;
        let _driver = self.pipeline.kernel.lock_commit_driver().await;
        for _ in 0..MAX_COMMITS_PER_DRIVE {
            let bound = self.max_submit_effect_bytes();
            if self
                .relay
                .effects
                .wait_capacity(bound, EffectClass::Remote)
                .await
                .is_err()
            {
                break;
            }
            if !self.commit_next_pipeline_entry(bound).await {
                break;
            }
        }
    }

    /// Select and commit one Ready candidate under the same TxPool write
    /// guard. The pool guard is the authoritative membership sequencer, so a
    /// Local/clear/reorg handoff cannot consume the Ready owner between ticket
    /// validation and pool insertion. The kernel mutex is held only for the
    /// constant-time ticket transition; no transient commit location exists.
    async fn commit_next_pipeline_entry(&self, effect_bound: usize) -> bool {
        let transaction = {
            let mut tx_pool = self.pool.tx_pool.write().await;
            let prepared = (|| {
                // Checkout must be inside the TxPool write boundary. Every
                // operation allowed to consume coordinator ownership from
                // outside this driver takes the same guard first.
                let lease = self
                    .pipeline
                    .kernel
                    .mutate_required("pipeline commit checkout failed", |coordinator| {
                        coordinator.begin_next_commit()
                    })?;
                let verified = Arc::clone(&lease.payload);
                let entry = TxEntry::new_with_resident_size(
                    Arc::clone(&verified.candidate.rtx),
                    verified.completed.cycles,
                    verified.candidate.fee,
                    verified.candidate.tx_size,
                    verified.candidate.resident_size,
                );
                let entry_id = entry.proposal_short_id();
                let snapshot = tx_pool.cloned_snapshot();
                Some(self.pipeline.kernel.mutate_authoritative(|kernel| {
                    self.relay
                        .effects
                        .try_apply_bounded(effect_bound, EffectClass::Remote, || {
                            let (mut outcome, settlement) = self.try_submit_entry_with_handoff(
                                &mut tx_pool,
                                snapshot,
                                verified.candidate.pre_resolve_tip.clone(),
                                entry.clone(),
                                |tx_pool, plan| {
                                    let unavailable = self.planned_unavailable_parent_hashes(
                                        plan,
                                        tx_pool.snapshot(),
                                    );
                                    let handoff = kernel
                                        .commit_any_handoff_with_unavailable_parents(
                                            &lease,
                                            &unavailable,
                                        )
                                        .unwrap_or_else(|error| {
                                            panic!("planned pipeline handoff failed: {error:?}")
                                        });
                                    self.settle_kernel_for_pool_plan(kernel, tx_pool, &entry, plan);
                                    handoff
                                },
                            );
                            let failed_terminal = if outcome.result.is_err() && settlement.is_none()
                            {
                                let retain_conflict = matches!(
                                    outcome.result.as_ref(),
                                    Err(Reject::RBFRejected(..)
                                        | Reject::Resolve(
                                            ckb_types::core::error::OutPointError::Dead(_)
                                        ))
                                ) && tx_pool
                                    .pool_map
                                    .find_conflict_outpoint(entry.transaction())
                                    .is_some();
                                Some(if retain_conflict {
                                    kernel
                                        .park_failed_commit(&lease)
                                        .expect("validated Ready lease must park")
                                } else {
                                    kernel
                                        .fail_commit(&lease)
                                        .expect("validated Ready lease must terminalize")
                                })
                            } else {
                                None
                            };
                            let mut failed_banned_peer = None;
                            let mut extra_effects = Vec::new();
                            if let Some(handoff) = &settlement {
                                extra_effects.push(TxPoolEffect::Relay(
                                    crate::service::TxVerificationResult::Ok {
                                        original_peer: handoff.winner.raw.ingress_peer(),
                                        tx_hash: verified.candidate.tx.hash(),
                                    },
                                ));
                                let reject = Reject::RBFRejected(
                                    Self::SUPERSEDED_BY_HIGHER_FEE_CANDIDATE.to_string(),
                                );
                                for record in &handoff.superseded {
                                    record.raw.authoritative_source(record.source);
                                    if let Some(effect) =
                                        self.recent_reject_effect(record.hash.clone(), &reject)
                                    {
                                        extra_effects.push(effect);
                                    }
                                    if record.raw.ingress_peer().is_some()
                                        && reject.is_allowed_relay()
                                    {
                                        extra_effects.push(TxPoolEffect::Relay(
                                            crate::service::TxVerificationResult::Reject {
                                                tx_hash: record.hash.clone(),
                                            },
                                        ));
                                    }
                                }
                            } else if let Some(record) = &failed_terminal
                                && let Some(reject) = outcome.result.as_ref().err()
                            {
                                if let Some(effect) =
                                    self.recent_reject_effect(record.hash.clone(), reject)
                                {
                                    extra_effects.push(effect);
                                }
                                if reject.is_malformed_tx()
                                    && let Some(peer) = record.raw.blame_peer()
                                {
                                    let duration = std::time::Duration::from_secs(
                                        crate::constants::MALFORMED_TX_BAN_SECONDS,
                                    );
                                    self.record_peer_ban(peer, duration);
                                    extra_effects.push(TxPoolEffect::BanPeer {
                                        peer,
                                        duration,
                                        reason: bounded_commit_ban_reason(reject),
                                    });
                                    failed_banned_peer = Some(peer);
                                }
                                if record.raw.ingress_peer().is_some()
                                    && reject.is_allowed_relay()
                                    && !matches!(reject, Reject::Duplicated(_))
                                {
                                    extra_effects.push(TxPoolEffect::Relay(
                                        crate::service::TxVerificationResult::Reject {
                                            tx_hash: record.hash.clone(),
                                        },
                                    ));
                                }
                            }
                            for status in outcome.block_assembler_statuses() {
                                self.journal_block_assembler_update(status);
                            }
                            let batch = self.prepare_submit_effects(&mut outcome, extra_effects);
                            (
                                (outcome, settlement, failed_banned_peer, verified, entry_id),
                                batch,
                            )
                        })
                }))
            })();
            match prepared {
                None => None,
                Some(Ok(value)) => Some(value),
                Some(Err(EffectJournalError::Full)) => return true,
                Some(Err(error)) => {
                    ckb_logger::error!("pipeline commit effect journal unavailable: {error:?}");
                    return true;
                }
            }
        };
        let Some((outcome, settlement, failed_banned_peer, verified, entry_id)) = transaction
        else {
            return false;
        };

        let dispatch_result = outcome.result;
        if let Some(peer) = failed_banned_peer {
            self.remove_banned_peer_entries(peer).await;
        }
        match (dispatch_result, settlement) {
            (Ok(()), Some(settlement)) => {
                self.post_submit_side_effects(
                    verified.completed,
                    verified.verify_cache_hit,
                    &verified.candidate.tx.hash(),
                    TxVerificationCacheKey::from_transaction(&verified.candidate.tx),
                    false,
                    verified.started_at,
                )
                .await;
                drop(settlement);
            }
            (Err(_reject), _) => {}
            (Ok(()), None) => unreachable!(
                "successful pipeline submit {entry_id} escaped without coordinator settlement"
            ),
        }
        true
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
