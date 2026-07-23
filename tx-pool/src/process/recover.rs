//! Bounded re-entry of pool-conflict recoveries into the authoritative
//! coordinator.
//!
//! Missing-parent and in-flight conflict waiting are coordinator states. This
//! module only handles transactions recovered from the accepted pool's
//! historical conflict cache after their inputs become available again.

use crate::error::Reject;
use crate::service::TxVerificationResult;
use crate::service::effects::{EffectBatch, TxPoolEffect};
use crate::tx_source::TxSource;
use ckb_logger::warn;

pub(crate) fn journal_recovery_terminal_records(
    relay: &crate::service::RelayState,
    permit: crate::service::effects::EffectPermit,
    records: &[crate::component::pipeline_coordinator::TerminalRecord<
        crate::component::pipeline_runtime::PipelineRawTx,
    >],
) {
    let mut effects = Vec::new();
    for record in records {
        let source = record
            .raw
            .authoritative_source(record.source)
            .unwrap_or(TxSource::Local);
        if source.peer().is_some() {
            effects.push(TxPoolEffect::Relay(TxVerificationResult::Reject {
                tx_hash: record.hash.clone(),
            }));
        }
    }
    let result = match EffectBatch::new(effects) {
        Some(batch) => relay.effects.commit(permit, batch),
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
    /// ownership. A capacity rejection keeps the cache entry scheduled for a
    /// later timer tick; every other admission outcome consumes the historical
    /// candidate instead of spinning forever.
    pub(crate) async fn recover_conflict_cache_slice(
        &self,
        limit: usize,
    ) -> ConflictRecoveryProgress {
        let initial = self
            .pool
            .tx_pool
            .read()
            .await
            .conflict_recovery_len()
            .min(limit);
        if initial == 0 {
            return ConflictRecoveryProgress {
                saturated: false,
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
                Err(error) => {
                    warn!("conflict recovery effect reservation failed: {error:?}");
                    capacity_blocked = true;
                    break;
                }
            };
            let mut tx_pool = self.pool.tx_pool.write().await;
            if !self.is_pipeline_epoch_current(epoch) {
                // `clear_pipeline` advances the epoch before waiting for this
                // lock and clears every recovery ticket in its pool/coordinator
                // transaction. Do not pop or admit old-generation work while
                // that linearization barrier is waiting.
                drop(permit);
                break;
            }
            let Some(candidate) = tx_pool.pop_conflict_recovery() else {
                drop(permit);
                break;
            };
            let tx_hash = candidate.tx.hash();
            if tx_pool
                .pool_map
                .find_conflict_outpoint(&candidate.tx)
                .is_some()
            {
                // A new accepted blocker arrived after scheduling. Its later
                // removal will mark this candidate again from the same input
                // indexes; do not poll a known-blocked entry.
                drop(permit);
                continue;
            }

            let admitted = self.pipeline.runtime.admit_transaction_journaled(
                candidate.tx,
                candidate.source,
                epoch,
                crate::component::pipeline_coordinator::RawStage::Resolve,
                |records| journal_recovery_terminal_records(&self.relay, permit, records),
            );
            match admitted {
                Ok((_added, _terminal)) => {
                    tx_pool.remove_conflict_hash(&tx_hash);
                }
                Err(error) => {
                    let reject = crate::component::pipeline_runtime::coordinator_reject(error);
                    if matches!(reject, Reject::Full(_)) {
                        tx_pool.reschedule_conflict_recovery(&tx_hash);
                        capacity_blocked = true;
                    } else {
                        warn!(
                            "dropping conflict-cache candidate {} after permanent coordinator admission failure: {:?}",
                            tx_hash, reject
                        );
                        tx_pool.remove_conflict_hash(&tx_hash);
                    }
                }
            }
        }

        let remaining = self.pool.tx_pool.read().await.conflict_recovery_len();
        ConflictRecoveryProgress {
            saturated: !capacity_blocked && initial == limit && remaining != 0,
            capacity_blocked,
        }
    }
}
