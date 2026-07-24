//! Bounded re-entry of pool-conflict recoveries into the authoritative
//! coordinator.
//!
//! Missing-parent and in-flight conflict waiting are coordinator states. This
//! module only handles transactions recovered from the accepted pool's
//! historical conflict cache after their inputs become available again.

use crate::service::TxVerificationResult;
use crate::service::effects::{EffectBatch, TxPoolEffect};
use ckb_logger::warn;

pub(crate) fn journal_recovery_terminal_records(
    permit: crate::service::effects::EffectPermit,
    records: &[crate::component::pipeline_coordinator::TerminalRecord<
        crate::component::pipeline_runtime::PipelineRawTx,
    >],
) {
    let mut effects = Vec::new();
    for record in records {
        if record.raw.ingress_peer().is_some() {
            effects.push(TxPoolEffect::Relay(TxVerificationResult::Reject {
                tx_hash: record.hash.clone(),
            }));
        }
    }
    let result = match EffectBatch::new(effects) {
        Some(batch) => permit.commit(batch),
        None => {
            drop(permit);
            Ok(())
        }
    };
    if let Err(error) = result {
        panic!("reserved recovery terminal journal failed: {error:?}");
    }
}

pub(crate) struct ConflictRecoveryProgress {
    pub(crate) saturated: bool,
    pub(crate) capacity_blocked: bool,
}

impl crate::service::TxPoolService {
    /// Transfer a bounded slice from the historical ConflictCache into the
    /// sole executable coordinator owner. `TxPool → coordinator` is held for
    /// the complete handoff, so combined readers cannot observe dual or zero
    /// ownership. An explicit capacity rejection keeps the cache entry
    /// scheduled for a later timer tick. Every other rejection is impossible
    /// for a previously verified historical candidate and therefore fails the
    /// authoritative service instead of silently consuming or spinning it.
    pub(crate) async fn recover_conflict_cache_slice(
        &self,
        limit: usize,
    ) -> ConflictRecoveryProgress {
        enum RecoveryStep {
            Continue,
            Stop,
            CapacityBlocked,
        }

        // Discovery has its own explicit probe budget. Input-release paths
        // only enqueue outpoints, so neither reorg nor submit/remove holds the
        // TxPool write lock while scanning a 10k-candidate fan-out.
        let (discovery, initial) = {
            let mut tx_pool = self.pool.tx_pool.write().await;
            self.pipeline.runtime.guard_authoritative_mutation(
                "conflict-cache discovery mutation panicked",
                || {
                    let discovery = tx_pool.discover_conflicted_txs(limit);
                    let initial = tx_pool.conflict_recovery_len().min(limit);
                    (discovery, initial)
                },
            )
        };
        if discovery.examined != 0 {
            ckb_logger::debug!(
                "conflict-cache discovery examined {}, scheduled {}, pending {}",
                discovery.examined,
                discovery.scheduled,
                discovery.pending
            );
        }
        if initial == 0 {
            return ConflictRecoveryProgress {
                saturated: discovery.pending,
                capacity_blocked: false,
            };
        }
        let epoch = match self.current_pipeline_epoch() {
            Ok(epoch) => epoch,
            Err(error) => {
                warn!("conflict recovery stopped by exhausted pipeline epoch: {error}");
                return ConflictRecoveryProgress {
                    saturated: false,
                    capacity_blocked: true,
                };
            }
        };
        let mut capacity_blocked = false;

        for _ in 0..initial {
            let permit = match self
                .reserve_effects(Self::pipeline_terminal_effect_bytes(
                    crate::constants::MAX_RBF_REPLACEMENT_CANDIDATES.saturating_add(1),
                ))
                .await
            {
                Ok(permit) => permit,
                Err(error) => self
                    .pipeline
                    .runtime
                    .fail_stop("conflict recovery effect reservation failed", &error),
            };
            let step = {
                let mut tx_pool = self.pool.tx_pool.write().await;
                self.pipeline.runtime.guard_authoritative_mutation(
                    "conflict-cache ownership handoff panicked",
                    || {
                        if !self.is_pipeline_epoch_current(epoch) {
                            // `clear_pipeline` advances the epoch before
                            // waiting for this lock and clears every recovery
                            // ticket in its pool/coordinator transaction.
                            drop(permit);
                            return RecoveryStep::Stop;
                        }
                        let Some(candidate) = tx_pool.pop_conflict_recovery() else {
                            drop(permit);
                            return RecoveryStep::Stop;
                        };
                        let tx_hash = candidate.tx.hash();
                        let short_id = candidate.tx.proposal_short_id();
                        if let Some(existing) = tx_pool.pool_map.get_by_id(&short_id) {
                            if existing.inner.transaction().hash() == tx_hash {
                                // Defensive stale-history cleanup: accepted
                                // membership is already the executable owner.
                                tx_pool.remove_conflict_hash(&tx_hash);
                                drop(permit);
                                return RecoveryStep::Continue;
                            }
                            // PoolMap is intentionally indexed by proposal ID
                            // because block proposals cannot disambiguate a
                            // collision. Preserve the full-hash historical
                            // owner until the colliding accepted entry leaves.
                            tx_pool.reschedule_conflict_recovery(&tx_hash);
                            drop(permit);
                            return RecoveryStep::CapacityBlocked;
                        }
                        if tx_pool
                            .pool_map
                            .find_conflict_outpoint(&candidate.tx)
                            .is_some()
                        {
                            // A new accepted blocker arrived after scheduling.
                            // Its later removal marks this candidate again.
                            drop(permit);
                            return RecoveryStep::Continue;
                        }

                        let admitted = self.pipeline.runtime.admit_transaction_journaled(
                            candidate.tx,
                            candidate.source,
                            epoch,
                            crate::component::pipeline_coordinator::RawStage::Resolve,
                            |records| journal_recovery_terminal_records(permit, records),
                        );
                        match admitted {
                            Ok((_added, _terminal)) => {
                                tx_pool.remove_conflict_hash(&tx_hash);
                                RecoveryStep::Continue
                            }
                            Err(error) => {
                                if error.is_retryable_capacity_rejection() {
                                    tx_pool.reschedule_conflict_recovery(&tx_hash);
                                    RecoveryStep::CapacityBlocked
                                } else if error.is_transaction_rejection() {
                                    // ConflictCache is bounded historical
                                    // ownership, not an executable queue. A
                                    // fixed policy failure (for example a
                                    // per-payload structural limit) cannot be
                                    // repaired by another timer tick, so
                                    // consume this generation explicitly
                                    // instead of creating permanent
                                    // maintenance work. The transaction
                                    // already has its historical pool reject;
                                    // no relayer request is outstanding here.
                                    tx_pool.remove_conflict_hash(&tx_hash);
                                    warn!(
                                        "dropping conflict-cache candidate {tx_hash} after permanent coordinator policy rejection: {error:?}"
                                    );
                                    RecoveryStep::Continue
                                } else {
                                    self.pipeline.runtime.fail_stop(
                                        "verified conflict-cache candidate hit an impossible coordinator admission failure",
                                        &(tx_hash, error),
                                    )
                                }
                            }
                        }
                    },
                )
            };
            match step {
                RecoveryStep::Continue => {}
                RecoveryStep::Stop => break,
                RecoveryStep::CapacityBlocked => capacity_blocked = true,
            }
        }

        let (remaining, discovery_remaining) = {
            let tx_pool = self.pool.tx_pool.read().await;
            (
                tx_pool.conflict_recovery_len(),
                tx_pool.conflict_discovery_len() != 0,
            )
        };
        ConflictRecoveryProgress {
            saturated: !capacity_blocked
                && (discovery_remaining || (initial == limit && remaining != 0)),
            capacity_blocked,
        }
    }
}
