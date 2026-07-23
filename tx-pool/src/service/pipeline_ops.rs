//! Cross-structure pipeline operations.
//!
//! The pipeline's structure list — pre-check queue, ordered-resolve queue,
//! verify queue, RBF-held registrations, orphan pool, and the main pool —
//! is enumerated in exactly one place: this module. Every cross-structure
//! operation (duplicate/visibility checks, removal, lookup, clearing)
//! lives here instead of being hand-enumerated at each call site, so
//! adding a structure (e.g. a new queue or a new held set) touches one
//! file instead of ten.
//!
//! Lock acquisition inside a helper follows the documented hierarchy
//! (`ordered_resolve_queue → rbf_candidates → verify_queue → orphan →
//! tx_pool`), and guards are always acquired and released sequentially —
//! never nested — so these helpers cannot deadlock.

use crate::component::pipeline_coordinator::{
    CoordinatorError, CoordinatorSource, RawWorkLease, VerifyWorkLease,
};
use crate::component::pipeline_runtime::PipelineRawTx;
use crate::service::{PipelineTxLocation, RemoveTxOutcome, TxPoolService};
use ckb_store::ChainStore;
use ckb_types::core::TransactionView;
use ckb_types::packed::{Byte32, ProposalShortId};
use std::collections::HashSet;

/// Result of registering a resolver/verification miss under the one
/// TxPool -> coordinator ownership boundary.
pub(crate) enum ParentWaitOutcome {
    /// At least one parent is still unavailable and the transaction now owns
    /// a coordinator wait registration.
    Parked {
        parents: HashSet<Byte32>,
        source: CoordinatorSource,
    },
    /// Every reported parent became available during the handoff window, so
    /// the raw transaction was queued for a fresh resolution snapshot.
    Requeued,
    /// A trusted Local/Proposal owner referenced a parent that is neither
    /// accepted nor currently coordinator-owned. It must fail terminally;
    /// unlike a remote owner it has no parent-request/expiry protocol.
    Unavailable,
}

fn retain_unavailable_parents(pool: &crate::pool::TxPool, parents: &mut HashSet<Byte32>) {
    let snapshot = pool.cloned_snapshot();
    parents.retain(|parent| {
        if snapshot.transaction_exists(parent) {
            return false;
        }
        let id = ProposalShortId::from_tx_hash(parent);
        !pool
            .get_tx_from_pool(&id)
            .is_some_and(|tx| tx.hash() == *parent)
    });
}

impl TxPoolService {
    pub(crate) fn current_pipeline_epoch(&self) -> Result<u64, crate::error::Reject> {
        self.pipeline.epoch.current().ok_or_else(|| {
            crate::error::Reject::Internal("tx-pool pipeline epoch exhausted".to_string())
        })
    }

    pub(crate) fn is_pipeline_epoch_current(&self, epoch: u64) -> bool {
        self.pipeline.epoch.is_current(epoch)
    }

    /// Invalidate every pipeline job admitted before this call.
    pub(crate) fn advance_pipeline_epoch(&self) {
        if self.pipeline.epoch.advance().is_none() {
            ckb_logger::error!(
                "tx-pool pipeline epoch exhausted; future pipeline work is fail-closed"
            );
        }
    }

    /// Recheck a raw resolver miss and install its wait/retry state while a
    /// parent cannot move from coordinator ownership into TxPool membership.
    /// This closes the lost-wakeup window between observing `Unknown` and
    /// registering `WaitingParents`.
    pub(crate) async fn settle_raw_parent_wait(
        &self,
        lease: &RawWorkLease<PipelineRawTx>,
        mut parents: HashSet<Byte32>,
        permit: crate::service::effects::EffectPermit,
    ) -> Result<ParentWaitOutcome, CoordinatorError> {
        let pool = self.pool.tx_pool.read().await;
        retain_unavailable_parents(&pool, &mut parents);
        let mut permit = Some(permit);
        let outcome = self.pipeline.runtime.mutate(|coordinator| {
            let source = coordinator
                .view(&lease.hash)
                .map(|view| view.source)
                .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
            if parents.is_empty() {
                coordinator.requeue_raw(lease)?;
                return Ok(ParentWaitOutcome::Requeued);
            }
            if !matches!(source, CoordinatorSource::Remote(_))
                && parents
                    .iter()
                    .any(|parent| !coordinator.contains_hash(parent))
            {
                return Ok(ParentWaitOutcome::Unavailable);
            }
            coordinator.wait_for_parents(lease, parents.clone())?;
            if let CoordinatorSource::Remote(peer) = source {
                let effect = crate::service::effects::TxPoolEffect::Relay(
                    crate::service::TxVerificationResult::UnknownParents {
                        peer,
                        parents: parents.clone(),
                    },
                );
                if let Err(error) = self.publish_reserved_effects(
                    permit
                        .take()
                        .expect("parent-wait effect permit is consumed at most once"),
                    vec![effect],
                ) {
                    panic!("reserved raw parent-wait journal failed: {error:?}");
                }
            }
            Ok(ParentWaitOutcome::Parked { parents, source })
        });
        drop(permit);
        outcome
    }

    /// Verification can discover that its resolution snapshot went stale.
    /// Demote and re-resolve under the same atomic parent boundary used by the
    /// raw resolver, rather than creating a second orphan owner.
    pub(crate) async fn settle_verify_parent_wait(
        &self,
        lease: &VerifyWorkLease<crate::resolved_tx::ResolvedTx>,
        mut parents: HashSet<Byte32>,
        permit: crate::service::effects::EffectPermit,
    ) -> Result<ParentWaitOutcome, CoordinatorError> {
        let pool = self.pool.tx_pool.read().await;
        retain_unavailable_parents(&pool, &mut parents);
        let mut permit = Some(permit);
        let outcome = self.pipeline.runtime.mutate(|coordinator| {
            let source = coordinator
                .view(&lease.hash)
                .map(|view| view.source)
                .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
            if !parents.is_empty()
                && !matches!(source, CoordinatorSource::Remote(_))
                && parents
                    .iter()
                    .any(|parent| !coordinator.contains_hash(parent))
            {
                return Ok(ParentWaitOutcome::Unavailable);
            }
            coordinator.verification_retry_resolution(lease, parents.clone())?;
            if parents.is_empty() {
                Ok(ParentWaitOutcome::Requeued)
            } else {
                if let CoordinatorSource::Remote(peer) = source {
                    let effect = crate::service::effects::TxPoolEffect::Relay(
                        crate::service::TxVerificationResult::UnknownParents {
                            peer,
                            parents: parents.clone(),
                        },
                    );
                    if let Err(error) = self.publish_reserved_effects(
                        permit
                            .take()
                            .expect("parent-wait effect permit is consumed at most once"),
                        vec![effect],
                    ) {
                        panic!("reserved verify parent-wait journal failed: {error:?}");
                    }
                }
                Ok(ParentWaitOutcome::Parked { parents, source })
            }
        });
        drop(permit);
        outcome
    }

    /// Remove a transaction by hash from every pipeline structure it may
    /// occupy (pre-check, ordered resolve, verify, RBF registrations,
    /// orphan and the main pool).
    pub(crate) async fn remove_tx(&self, tx_hash: Byte32) -> RemoveTxOutcome {
        let terminal_permit = match self
            .reserve_effects(Self::pipeline_terminal_effect_bytes(1))
            .await
        {
            Ok(permit) => permit,
            Err(error) => {
                ckb_logger::error!("remove effect reservation failed: {:?}", error);
                return RemoveTxOutcome::InProgress;
            }
        };
        let id = ProposalShortId::from_tx_hash(&tx_hash);
        let (record, removed_entries, conflict_removed) = {
            let mut tx_pool = self.pool.tx_pool.write().await;
            let pool_target = tx_pool
                .get_tx_from_pool(&id)
                .is_some_and(|tx| tx.hash() == tx_hash);
            let (record, removed_entries) = if pool_target {
                // Compute the exact accepted closure before any mutation.
                // Every hash can have already-resolved coordinator consumers,
                // not only the requested root.
                let mut removal_ids = tx_pool.pool_map.calc_descendants(&id);
                removal_ids.insert(id.clone());
                let removal_hashes: HashSet<_> = removal_ids
                    .iter()
                    .filter_map(|removed_id| {
                        tx_pool
                            .pool_map
                            .get_by_id(removed_id)
                            .map(|entry| entry.inner.transaction().hash())
                    })
                    .collect();
                let removal_statuses: HashSet<_> = removal_ids
                    .iter()
                    .filter_map(|removed_id| {
                        tx_pool
                            .pool_map
                            .get_by_id(removed_id)
                            .map(|entry| entry.status)
                    })
                    .collect();

                // Pre-pool consumers are demoted in one undo-protected
                // coordinator transition while the pool membership write lock
                // prevents a handoff. Only then is the infallible physical
                // pool closure removed.
                if let Err(error) = self
                    .pipeline
                    .runtime
                    .mutate(|coordinator| coordinator.parents_unavailable(&removal_hashes))
                {
                    ckb_logger::error!(
                        "failed to prepare coordinator consumers before removing {}: {:?}",
                        tx_hash,
                        error
                    );
                    return RemoveTxOutcome::InProgress;
                }
                let removed = tx_pool.remove_tx(&id);
                for status in removal_statuses {
                    self.journal_block_assembler_update(status);
                }
                (None, removed)
            } else {
                let record = match self.pipeline.runtime.mutate(|coordinator| {
                    coordinator.force_terminalize(
                        &tx_hash,
                        crate::component::pipeline_coordinator::TerminalDisposition::Removed,
                    )
                }) {
                    Ok(record) => record,
                    Err(CoordinatorError::CommitInProgress(_)) => {
                        return RemoveTxOutcome::InProgress;
                    }
                    Err(error) => {
                        ckb_logger::error!(
                            "failed to remove {} from pipeline coordinator: {:?}",
                            tx_hash,
                            error
                        );
                        return RemoveTxOutcome::InProgress;
                    }
                };
                (record, Vec::new())
            };
            let conflict_removed = tx_pool.remove_conflict_hash(&tx_hash);
            if let Some(record) = &record {
                self.journal_pipeline_terminal_records(
                    terminal_permit,
                    std::slice::from_ref(record),
                );
            } else {
                drop(terminal_permit);
            }
            (record, removed_entries, conflict_removed)
        };
        let coordinator_removed = record.is_some();
        if coordinator_removed || conflict_removed || !removed_entries.is_empty() {
            RemoveTxOutcome::Removed
        } else {
            RemoveTxOutcome::NotFound
        }
    }

    /// Linearizably clear queued and active pipeline work without touching
    /// transactions that already committed to the main pool.
    pub(crate) async fn clear_pipeline(&self) {
        let terminal_permit = match self
            .reserve_effects(Self::pipeline_terminal_effect_bytes(
                self.pipeline.runtime.max_entries(),
            ))
            .await
        {
            Ok(permit) => permit,
            Err(error) => {
                ckb_logger::error!("clear pipeline effect reservation failed: {:?}", error);
                return;
            }
        };
        self.advance_pipeline_epoch();
        // A submit that acquired the pool write lock and validated its epoch
        // before the generation advance is allowed to finish. Waiting on the
        // same lock makes that ordering explicit before this method returns;
        // later submitters observe the stale generation and cannot commit.
        let terminal = {
            let _commit_barrier = self.pool.tx_pool.write().await;
            let result = self.pipeline.runtime.mutate(|coordinator| {
                let result = coordinator.clear();
                if let Ok(records) = &result {
                    self.journal_pipeline_terminal_records(terminal_permit, records);
                }
                result
            });
            result
        };
        match terminal {
            Ok(_terminal) => {}
            Err(error) => ckb_logger::error!("failed to clear pipeline coordinator: {:?}", error),
        }
    }

    pub(crate) fn find_tx_in_coordinator_hash(&self, hash: &Byte32) -> Option<PipelineTxLocation> {
        self.find_tx_in_coordinator_by(|coordinator| {
            coordinator.contains_hash(hash).then(|| hash.clone())
        })
    }

    fn find_tx_in_coordinator_by(
        &self,
        select: impl FnOnce(
            &crate::component::pipeline_runtime::ProductionCoordinator,
        ) -> Option<Byte32>,
    ) -> Option<PipelineTxLocation> {
        self.pipeline.runtime.read(|coordinator| {
            let hash = select(coordinator)?;
            let view = coordinator.view(&hash)?;
            let raw = coordinator.raw_by_hash(&hash)?;
            let unverified = coordinator.unverified_by_short_id(&view.short_id);
            let verified = coordinator.verified_by_short_id(&view.short_id);
            use crate::component::pipeline_coordinator::CoordinatorLocation;
            Some(match view.location {
                CoordinatorLocation::RawQueued(
                    crate::component::pipeline_coordinator::RawStage::PreCheck,
                )
                | CoordinatorLocation::RawActive(
                    crate::component::pipeline_coordinator::RawStage::PreCheck,
                ) => PipelineTxLocation::PreChecking { tx: raw.tx.clone() },
                CoordinatorLocation::WaitingParents { .. } => PipelineTxLocation::Orphan {
                    tx: raw.tx.clone(),
                    cycle: raw.declared_cycles.unwrap_or(0),
                },
                CoordinatorLocation::VerifyQueued | CoordinatorLocation::VerifyActive => {
                    if let Some(resolved) = unverified {
                        PipelineTxLocation::Verifying {
                            tx: raw.tx.clone(),
                            fee: resolved.fee,
                            status: resolved.status,
                        }
                    } else {
                        PipelineTxLocation::Ordered { tx: raw.tx.clone() }
                    }
                }
                CoordinatorLocation::ReadyToCommit
                | CoordinatorLocation::WaitingConflict { .. }
                | CoordinatorLocation::ConflictRecheck
                | CoordinatorLocation::Committing => {
                    if let Some(verified) = verified {
                        PipelineTxLocation::Verifying {
                            tx: raw.tx.clone(),
                            fee: verified.resolved.fee,
                            status: verified.resolved.status,
                        }
                    } else {
                        PipelineTxLocation::Ordered { tx: raw.tx.clone() }
                    }
                }
                CoordinatorLocation::RawQueued(_)
                | CoordinatorLocation::RawActive(_)
                | CoordinatorLocation::Invalidated { .. } => {
                    PipelineTxLocation::Ordered { tx: raw.tx.clone() }
                }
            })
        })
    }

    /// Filter proposals down to those that are **completely new** to this
    /// node: not in any pipeline queue (or active), not RBF-held, not in
    /// the orphan pool, and not in the main pool.
    ///
    /// These locations are exactly the same stages searched by
    /// [`Self::get_tx_for_compact_block`], so filtering them out here is safe:
    /// a proposal marked as "known" can always be retrieved later for compact
    /// block reconstruction.
    pub async fn exclude_existing_proposal(
        &self,
        mut proposals: Vec<ProposalShortId>,
    ) -> Vec<ProposalShortId> {
        {
            // Pool membership and coordinator ownership are one read
            // transaction. A commit holds the pool write guard while it
            // performs the coordinator handoff, so a proposal cannot be
            // invisible between the two authorities.
            let tx_pool = self.pool.tx_pool.read().await;
            self.pipeline.runtime.read(|coordinator| {
                proposals.retain(|id| {
                    coordinator.hash_by_short_id(id).is_none() && !tx_pool.contains_proposal_id(id)
                });
            });
        }
        proposals
    }

    /// Retrieves transactions required for compact block reconstruction.
    ///
    /// During compact block relay, a node may receive a block that contains transactions
    /// still being verified and not yet present in the main mempool. This method searches
    /// all locations where a transaction can reside when its short ID is known:
    ///
    /// 1. `ordered_resolve_queue` – transactions waiting for parent resolution
    /// 2. `pre_check_queue` – transactions awaiting pre-check by workers
    /// 3. `verify_queue` – transactions currently undergoing background validation
    /// 4. `rbf_candidates` – displaced transactions held by in-flight registrations
    /// 5. `orphan_pool` – orphan transactions waiting for missing parents
    /// 6. `pool_map` – the main mempool (already accepted transactions)
    ///
    /// # Returns
    /// A map containing only the transactions that were found, keyed by their short ID.
    /// Missing entries are simply omitted (caller should treat absence as "need to request")
    /// Returning a `HashMap` allows the caller (compact block reconstructor) to:
    /// - Immediately obtain all locally-available transactions in a single call
    /// - Quickly identify which short IDs are missing
    pub async fn get_tx_for_compact_block(
        &self,
        short_ids: HashSet<ProposalShortId>,
    ) -> std::collections::HashMap<ProposalShortId, TransactionView> {
        let mut txs = std::collections::HashMap::with_capacity(short_ids.len());
        {
            // Snapshot accepted and coordinator-owned data under the sole
            // cross-authority order. The commit writer cannot expose a gap
            // between consuming the coordinator entry and inserting pool
            // membership while this read guard is held.
            let tx_pool = self.pool.tx_pool.read().await;
            txs.extend(short_ids.iter().filter_map(|short_id| {
                self.pipeline
                    .runtime
                    .read(|coordinator| coordinator.raw_by_short_id(short_id))
                    .map(|raw| (short_id.to_owned(), raw.tx.clone()))
            }));
            txs.extend(short_ids.iter().filter_map(|short_id| {
                tx_pool
                    .get_tx_from_pool_or_store(short_id)
                    .map(|tx| (short_id.to_owned(), tx))
            }));
        }
        txs
    }
}
