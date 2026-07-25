//! Cross-structure pipeline operations.
//!
//! The accepted pool and the pre-pool coordinator are the only executable
//! state owners. Operations that cross that boundary take `TxPool` before
//! the coordinator and complete membership/effect bookkeeping before the
//! pool guard is released. Lookup, removal, parent waiting, and clearing live
//! here so call sites cannot invent a partial structure list or lock order.

use crate::component::pre_pool::PrePoolKernel;
use crate::component::pre_pool::{
    DependencyKey, PrePoolError, PrePoolSource, ResolveLease, TerminalRecord, VerifyLease,
};
use crate::error::Reject;
use crate::service::{PipelineTxLocation, RemoveTxOutcome, TxPoolService};
use ckb_store::ChainStore;
use ckb_types::core::TransactionView;
use ckb_types::packed::{Byte32, ProposalShortId};
use ckb_types::prelude::Unpack;
use std::collections::{BTreeSet, HashSet};

/// Result of registering a resolver/verification miss under the one
/// TxPool -> coordinator ownership boundary.
pub(crate) enum ParentWaitOutcome {
    /// At least one parent is still unavailable and the transaction now owns
    /// a coordinator wait registration.
    Parked,
    /// Every reported parent became available during the handoff window, so
    /// the raw transaction was queued for a fresh resolution snapshot.
    Requeued,
    /// A trusted Local/Proposal owner referenced a parent that is neither
    /// accepted nor currently coordinator-owned. It must fail terminally;
    /// unlike a remote owner it has no parent-request/expiry protocol.
    Unavailable,
    /// The newly discovered dependency set violates a transaction policy or
    /// bounded coordinator capacity rule. The current lease remains owned and
    /// must be terminalized normally.
    Rejected(crate::error::Reject),
}

pub(crate) fn dependency_is_available(pool: &crate::pool::TxPool, key: &DependencyKey) -> bool {
    match key {
        DependencyKey::Cell(out_point) => {
            let parent = out_point.tx_hash();
            pool.get_tx_from_pool_by_hash(&parent).is_some_and(|tx| {
                let index: u32 = out_point.index().unpack();
                (index as usize) < tx.outputs().len()
            }) || pool.snapshot().get_cell(out_point).is_some()
        }
        DependencyKey::Header(hash) => pool.snapshot().is_main_chain(hash),
    }
}

/// Convert an outpoint delta into exact dependency levels that are available
/// in the post-mutation pool/snapshot overlay. Physical removal/attachment is
/// not itself availability: an input consumed by the newly committed branch
/// is still dead and must not spuriously wake historical conflict owners.
pub(crate) fn available_cell_dependencies(
    pool: &crate::pool::TxPool,
    outpoints: impl IntoIterator<Item = ckb_types::packed::OutPoint>,
) -> BTreeSet<DependencyKey> {
    crate::component::pre_pool::available_cell_keys(outpoints)
        .filter(|key| dependency_is_available(pool, key))
        .collect()
}

fn retain_unavailable_dependencies(
    pool: &crate::pool::TxPool,
    dependencies: &mut BTreeSet<DependencyKey>,
) {
    dependencies.retain(|key| !dependency_is_available(pool, key));
}

impl TxPoolService {
    /// Snapshot accepted entries and conflict-history owners under the global
    /// TxPool -> pre-pool read order. RPC and fee estimation therefore see
    /// the same conflict projection without giving `TxPool` a second cache.
    pub(crate) async fn all_entry_info(&self) -> ckb_types::core::tx_pool::TxPoolEntryInfo {
        let tx_pool = self.pool.tx_pool.read().await;
        let mut info = tx_pool.get_all_entry_info();
        info.conflicted = self.pipeline.kernel.read(PrePoolKernel::conflict_hashes);
        info
    }

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

    async fn settle_parent_wait(
        &self,
        hash: &Byte32,
        mut dependencies: BTreeSet<DependencyKey>,
        permit: crate::service::effects::EffectPermit,
        transition: impl FnOnce(&mut PrePoolKernel, BTreeSet<DependencyKey>) -> Result<(), PrePoolError>,
        journal_context: &'static str,
        error_context: &'static str,
    ) -> Option<ParentWaitOutcome> {
        let pool = self.pool.tx_pool.read().await;
        retain_unavailable_dependencies(&pool, &mut dependencies);
        let parents = dependencies
            .iter()
            .map(DependencyKey::parent_hash)
            .collect::<HashSet<_>>();
        let mut permit = Some(permit);
        let outcome: Result<ParentWaitOutcome, PrePoolError> =
            self.pipeline.kernel.mutate(|coordinator| {
                let source = coordinator
                    .source_by_hash(hash)
                    .ok_or_else(|| PrePoolError::Missing(hash.clone()))?;
                if !parents.is_empty()
                    && !matches!(source, PrePoolSource::Remote(_))
                    && parents
                        .iter()
                        .any(|parent| !coordinator.contains_hash(parent))
                {
                    return Ok(ParentWaitOutcome::Unavailable);
                }
                transition(coordinator, dependencies.clone())?;
                if dependencies.is_empty() {
                    return Ok(ParentWaitOutcome::Requeued);
                }
                if let PrePoolSource::Remote(peer) = source {
                    let effect = crate::service::effects::TxPoolEffect::Relay(
                        crate::service::TxVerificationResult::UnknownParents {
                            peer,
                            parents: parents.clone(),
                        },
                    );
                    self.publish_required_reserved_effects(
                        permit
                            .take()
                            .expect("parent-wait effect permit is consumed at most once"),
                        vec![effect],
                        journal_context,
                    );
                }
                Ok(ParentWaitOutcome::Parked)
            });
        drop(permit);
        match outcome {
            Ok(outcome) => Some(outcome),
            Err(error) if error.is_stale_lease() => None,
            Err(error) => Some(ParentWaitOutcome::Rejected(
                self.pipeline.kernel.reject_or_fail(error_context, error),
            )),
        }
    }

    /// Settle one active worker lease at the common cross-authority terminal
    /// boundary. A public rejection may need the accepted pool for conflict
    /// history; internal/cancellation exits deliberately avoid that lock.
    /// Effect publication and peer revocation stay inside this one protocol so
    /// raw and verify workers cannot diverge in terminal ownership handling.
    pub(crate) async fn settle_pipeline_terminal(
        &self,
        subject: &Byte32,
        reject: Option<Reject>,
        reservation_context: &'static str,
        mutation_context: &'static str,
        terminalize: impl FnOnce(&mut PrePoolKernel, bool) -> Result<TerminalRecord, PrePoolError>
        + Send,
    ) {
        let permit = self
            .reserve_required_effects(
                Self::pipeline_outcome_effect_bytes(reject.as_ref()),
                reservation_context,
            )
            .await;
        let tx_pool = if reject.is_some() {
            Some(self.pool.tx_pool.write().await)
        } else {
            None
        };
        let mut banned_peer = None;
        let retain_conflict = reject.as_ref().is_some_and(|reject| {
            matches!(
                reject,
                Reject::RBFRejected(..)
                    | Reject::Resolve(ckb_types::core::error::OutPointError::Dead(_))
            ) && tx_pool.as_ref().is_some_and(|pool| {
                // The raw payload remains kernel-owned until the transition
                // below, so this check and the transition share the universal
                // TxPool -> kernel order.
                self.pipeline.kernel.read(|kernel| {
                    kernel
                        .raw_by_hash(subject)
                        .is_some_and(|raw| pool.pool_map.find_conflict_outpoint(&raw.tx).is_some())
                })
            })
        });
        let terminal = self
            .pipeline
            .kernel
            .mutate_lease(mutation_context, |coordinator| {
                let result = terminalize(coordinator, retain_conflict);
                if let Ok(record) = &result {
                    banned_peer = self.journal_pipeline_outcome(permit, record, reject.as_ref());
                }
                result
            });
        drop(tx_pool);
        if terminal.is_some()
            && let Some(peer) = banned_peer
        {
            self.remove_banned_peer_entries(peer).await;
        }
    }

    /// Recheck a raw resolver miss and install its wait/retry state while a
    /// parent cannot move from coordinator ownership into TxPool membership.
    /// This closes the lost-wakeup window between observing `Unknown` and
    /// registering `WaitingParents`.
    pub(crate) async fn settle_raw_parent_wait(
        &self,
        lease: &ResolveLease,
        dependencies: BTreeSet<DependencyKey>,
        permit: crate::service::effects::EffectPermit,
    ) -> Option<ParentWaitOutcome> {
        self.settle_parent_wait(
            &lease.hash,
            dependencies,
            permit,
            |coordinator, dependencies| {
                if dependencies.is_empty() {
                    coordinator.requeue_resolve(lease).map(|_| ())
                } else {
                    coordinator.wait_resolve(lease, dependencies).map(|_| ())
                }
            },
            "reserved raw parent-wait journal failed",
            "current raw lease could not extend parent-wait dependencies",
        )
        .await
    }

    /// Verification can discover that its resolution snapshot went stale.
    /// Demote and re-resolve under the same atomic parent boundary used by the
    /// raw resolver, rather than creating a second orphan owner.
    pub(crate) async fn settle_verify_parent_wait(
        &self,
        lease: &VerifyLease,
        dependencies: BTreeSet<DependencyKey>,
        permit: crate::service::effects::EffectPermit,
    ) -> Option<ParentWaitOutcome> {
        self.settle_parent_wait(
            &lease.hash,
            dependencies,
            permit,
            |coordinator, dependencies| {
                coordinator
                    .verification_retry_resolution(lease, dependencies)
                    .map(|_| ())
            },
            "reserved verify parent-wait journal failed",
            "current verify lease could not extend parent-wait dependencies",
        )
        .await
    }

    /// Remove a transaction by hash from every pipeline structure it may
    /// occupy (pre-check, ordered resolve, verify, RBF registrations,
    /// orphan and the main pool).
    pub(crate) async fn remove_tx(&self, tx_hash: Byte32) -> RemoveTxOutcome {
        let terminal_permit = self
            .reserve_required_effects(
                Self::pipeline_terminal_effect_bytes(1),
                "remove effect reservation failed",
            )
            .await;
        let id = ProposalShortId::from_tx_hash(&tx_hash);
        let mutation = {
            let mut tx_pool = self.pool.tx_pool.write().await;
            self.pipeline.kernel.guard_authoritative_mutation(
                "administrative removal mutation panicked",
                || {
                    let pool_target = tx_pool.get_tx_from_pool_by_hash(&tx_hash).is_some();
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
                            .filter(|hash| !tx_pool.snapshot().transaction_exists(hash))
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

                        // Pre-pool consumers are demoted in one kernel transition
                        // while the pool membership write lock prevents a handoff.
                        // Only then is the physical pool closure removed.
                        self.pipeline.kernel.mutate_required(
                            "administrative removal could not demote coordinator consumers",
                            |coordinator| coordinator.parents_unavailable(&removal_hashes),
                        );
                        let removed = tx_pool.remove_tx(&id);
                        let released_inputs =
                            tx_pool.released_inputs_from_removed_entries(&removed);
                        let available_dependencies =
                            available_cell_dependencies(&tx_pool, released_inputs);
                        self.pipeline.kernel.mutate_required(
                            "administrative removal availability update failed",
                            |kernel| kernel.note_available(available_dependencies),
                        );
                        for status in removal_statuses {
                            self.journal_block_assembler_update(status);
                        }
                        (None, removed)
                    } else {
                        let record = match self.pipeline.kernel.mutate(|coordinator| {
                            coordinator.force_terminalize(
                                &tx_hash,
                                crate::component::pre_pool::TerminalDisposition::Removed,
                            )
                        }) {
                            Ok(record) => record,
                            Err(error) => self.pipeline.kernel.fail_stop(
                                "administrative pipeline owner could not terminalize",
                                &error,
                            ),
                        };
                        (record, Vec::new())
                    };
                    let conflict_removed = false;
                    if let Some(record) = &record {
                        self.journal_pipeline_terminal_records(
                            terminal_permit,
                            std::slice::from_ref(record),
                        );
                    } else {
                        drop(terminal_permit);
                    }
                    Some((record, removed_entries, conflict_removed))
                },
            )
        };
        let Some((record, removed_entries, conflict_removed)) = mutation else {
            return RemoveTxOutcome::InProgress;
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
        let terminal_permit = self
            .reserve_required_effects(
                Self::pipeline_terminal_effect_bytes(self.pipeline.kernel.max_entries()),
                "clear pipeline effect reservation failed",
            )
            .await;
        self.advance_pipeline_epoch();
        // A submit that acquired the pool write lock and validated its epoch
        // before the generation advance is allowed to finish. Waiting on the
        // same lock makes that ordering explicit before this method returns;
        // later submitters observe the stale generation and cannot commit.
        {
            let _tx_pool = self.pool.tx_pool.write().await;
            self.pipeline.kernel.guard_authoritative_mutation(
                "clear-pipeline authoritative mutation panicked",
                || {
                    self.pipeline.kernel.mutate_required(
                        "clear pipeline could not clear coordinator",
                        |coordinator| {
                            let result = coordinator.clear();
                            if let Ok(records) = &result {
                                self.journal_pipeline_terminal_records(terminal_permit, records);
                            }
                            result
                        },
                    );
                },
            );
        }
    }

    pub(crate) fn find_tx_in_coordinator_hash(&self, hash: &Byte32) -> Option<PipelineTxLocation> {
        self.find_tx_in_coordinator_by(|coordinator| {
            coordinator.contains_hash(hash).then(|| hash.clone())
        })
    }

    fn find_tx_in_coordinator_by(
        &self,
        select: impl FnOnce(&crate::component::pre_pool::PrePoolKernel) -> Option<Byte32>,
    ) -> Option<PipelineTxLocation> {
        self.pipeline.kernel.read(|coordinator| {
            let hash = select(coordinator)?;
            let view = coordinator.view(&hash)?;
            let raw = coordinator.raw_by_hash(&hash)?;
            let unverified = coordinator.unverified_by_hash(&hash);
            let verified = coordinator.verified_by_hash(&hash);
            use crate::component::pre_pool::PrePoolLocation;
            Some(match view.location {
                PrePoolLocation::Wait(crate::component::pre_pool::WaitReason::Missing) => {
                    PipelineTxLocation::Orphan {
                        tx: raw.tx.clone(),
                        cycle: raw.declared_cycles.unwrap_or(0),
                    }
                }
                PrePoolLocation::VerifyQueued | PrePoolLocation::VerifyLeased => {
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
                PrePoolLocation::Ready => {
                    if let Some(verified) = verified {
                        PipelineTxLocation::Verifying {
                            tx: raw.tx.clone(),
                            fee: verified.candidate.fee,
                            status: verified.candidate.status,
                        }
                    } else {
                        PipelineTxLocation::Ordered { tx: raw.tx.clone() }
                    }
                }
                PrePoolLocation::RecoveryRetained
                | PrePoolLocation::ResolveQueued
                | PrePoolLocation::ResolveLeased => {
                    PipelineTxLocation::Ordered { tx: raw.tx.clone() }
                }
                PrePoolLocation::Wait(crate::component::pre_pool::WaitReason::Conflict) => {
                    PipelineTxLocation::ConflictHistory
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
            self.pipeline.kernel.read(|coordinator| {
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
    /// both executable owners when its short ID is known: the single
    /// pre-pool coordinator and the accepted `pool_map`.
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
        let (snapshot, committed) = {
            // Snapshot accepted and coordinator-owned data under the sole
            // cross-authority order. The commit writer cannot expose a gap
            // between consuming the coordinator entry and inserting pool
            // membership while this read guard is held.
            let tx_pool = self.pool.tx_pool.read().await;
            // Take the coordinator mutex once for the complete bounded compact
            // block request. Taking it once per short id lets a large valid
            // block amplify lock traffic and delays every pipeline worker.
            self.pipeline.kernel.read(|coordinator| {
                txs.extend(short_ids.iter().filter_map(|short_id| {
                    coordinator
                        .raw_by_short_id(short_id)
                        .map(|raw| (short_id.to_owned(), raw.tx.clone()))
                }));
            });
            txs.extend(short_ids.iter().filter_map(|short_id| {
                tx_pool
                    .get_tx_from_pool(short_id)
                    .cloned()
                    .map(|tx| (short_id.to_owned(), tx))
            }));
            let committed = short_ids
                .iter()
                .filter(|short_id| !txs.contains_key(*short_id))
                .filter_map(|short_id| {
                    tx_pool
                        .committed_txs_hash_cache
                        .peek(short_id)
                        .cloned()
                        .map(|hash| (short_id.clone(), hash))
                })
                .collect::<Vec<_>>();
            (tx_pool.cloned_snapshot(), committed)
        };
        // The recent-commit fallback can touch storage. Do not hold either
        // lifecycle authority or block an async executor worker while loading
        // those immutable snapshot transactions.
        txs.extend(crate::util::block_offload(move || {
            committed
                .into_iter()
                .filter_map(|(short_id, hash)| {
                    snapshot
                        .get_transaction(&hash)
                        .map(|(tx, _)| (short_id, tx))
                })
                .collect::<Vec<_>>()
        }));
        txs
    }
}
