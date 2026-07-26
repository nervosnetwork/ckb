//! Cross-structure pipeline operations.
//!
//! The accepted pool and the pre-pool coordinator are the only executable
//! state owners. Operations that cross that boundary take `TxPool` before
//! the coordinator and complete membership/effect bookkeeping before the
//! pool guard is released. Lookup, removal, parent waiting, and clearing live
//! here so call sites cannot invent a partial structure list or lock order.

use crate::component::pool_map::PoolMutationFault;
use crate::component::pre_pool::PrePoolKernel;
use crate::component::pre_pool::{
    DependencyKey, PrePoolError, PrePoolSource, ResolveLease, TerminalRecord, VerifyLease,
};
use crate::error::Reject;
use crate::service::effects::{EffectBatch, EffectClass, EffectJournalError, TxPoolEffect};
use crate::service::{PipelineTxLocation, RemoveTxOutcome, TxPoolService};
use ckb_store::ChainStore;
use ckb_types::core::TransactionView;
use ckb_types::packed::{Byte32, ProposalShortId};
use ckb_types::prelude::Unpack;
use std::collections::{BTreeSet, HashSet};

#[derive(Debug)]
enum AdministrativeRemovalError {
    Accepted(PoolMutationFault),
    PrePool(PrePoolError),
}

impl From<PoolMutationFault> for AdministrativeRemovalError {
    fn from(error: PoolMutationFault) -> Self {
        Self::Accepted(error)
    }
}

impl From<PrePoolError> for AdministrativeRemovalError {
    fn from(error: PrePoolError) -> Self {
        Self::PrePool(error)
    }
}

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
        discovered_dependencies: BTreeSet<DependencyKey>,
        mut transition: impl FnMut(
            &mut PrePoolKernel,
            BTreeSet<DependencyKey>,
        ) -> Result<(), PrePoolError>,
    ) -> Option<ParentWaitOutcome> {
        loop {
            let pool = self.pool.tx_pool.read().await;
            let mut effect_bytes = 0;
            let outcome: Result<
                Result<Result<ParentWaitOutcome, PrePoolError>, EffectJournalError>,
                PrePoolError,
            > = self.pipeline.kernel.mutate_authoritative(|coordinator| {
                // Resolution reports one new miss at a time. Rebuild the full
                // level from the current primary while both authorities are
                // held, then filter accepted availability from that exact
                // snapshot. A Full retry therefore cannot forget a parent.
                let mut unavailable = coordinator
                    .cell_dependency_frontier(hash, discovered_dependencies.iter().cloned())
                    .ok_or_else(|| PrePoolError::Missing(hash.clone()))?;
                retain_unavailable_dependencies(&pool, &mut unavailable);
                let parents = unavailable
                    .iter()
                    .map(DependencyKey::parent_hash)
                    .collect::<HashSet<_>>();
                let source = coordinator
                    .source_by_hash(hash)
                    .ok_or_else(|| PrePoolError::Missing(hash.clone()))?;
                if !parents.is_empty()
                    && !matches!(source, PrePoolSource::Remote(_))
                    && parents
                        .iter()
                        .any(|parent| !coordinator.contains_hash(parent))
                {
                    return Ok(Ok(Ok(ParentWaitOutcome::Unavailable)));
                }
                let batch = if let PrePoolSource::Remote(remote) = source
                    && !unavailable.is_empty()
                {
                    EffectBatch::new(vec![TxPoolEffect::Relay(
                        crate::service::TxVerificationResult::UnknownParents {
                            peer: remote.peer,
                            parents: parents.clone(),
                        },
                    )])
                } else {
                    None
                };
                let class = if matches!(source, PrePoolSource::Remote(_)) {
                    EffectClass::Remote
                } else {
                    EffectClass::Trusted
                };
                effect_bytes = Self::unknown_parents_effect_bytes(unavailable.len());
                Ok(self.relay.effects.try_apply(batch, class, || {
                    transition(coordinator, unavailable.clone())?;
                    Ok(if unavailable.is_empty() {
                        ParentWaitOutcome::Requeued
                    } else {
                        ParentWaitOutcome::Parked
                    })
                }))
            });
            drop(pool);
            match outcome {
                Ok(Ok(Ok(outcome))) => return Some(outcome),
                Ok(Ok(Err(error))) | Err(error) if error.is_stale_lease() => return None,
                Ok(Ok(Err(error))) | Err(error) => {
                    if error.is_transaction_rejection() {
                        return Some(ParentWaitOutcome::Rejected(
                            crate::component::pre_pool::pre_pool_reject(error),
                        ));
                    }
                    self.pipeline
                        .kernel
                        .report_fault("parent-wait settlement invariant failed", &error);
                    return None;
                }
                Ok(Err(EffectJournalError::Full)) => {
                    if self
                        .relay
                        .effects
                        .wait_capacity(effect_bytes, EffectClass::Remote)
                        .await
                        .is_err()
                    {
                        return Some(ParentWaitOutcome::Rejected(Reject::Full(
                            "effect journal unavailable".to_owned(),
                        )));
                    }
                }
                Ok(Err(error)) => {
                    ckb_logger::error!(
                        "parent-wait settlement effect journal unavailable: {error:?}"
                    );
                    return Some(ParentWaitOutcome::Rejected(Reject::Full(
                        "effect journal unavailable".to_owned(),
                    )));
                }
            }
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
        mutation_context: &'static str,
        mut terminalize: impl FnMut(&mut PrePoolKernel, bool) -> Result<TerminalRecord, PrePoolError>
        + Send,
    ) {
        loop {
            let Some(preview) = self
                .pipeline
                .kernel
                .read(|kernel| kernel.terminal_record(subject))
            else {
                return;
            };
            let (preview_batch, _) = self.pipeline_outcome_effects(&preview, reject.as_ref());
            let class = if matches!(preview.source, PrePoolSource::Remote(_)) {
                EffectClass::Remote
            } else {
                EffectClass::Trusted
            };
            if let Some(batch) = &preview_batch
                && self
                    .relay
                    .effects
                    .wait_capacity(batch.charge_bytes(), class)
                    .await
                    .is_err()
            {
                return;
            }

            let tx_pool = if reject.is_some() {
                Some(self.pool.tx_pool.write().await)
            } else {
                None
            };
            let retain_conflict = reject.as_ref().is_some_and(|reject| {
                matches!(
                    reject,
                    Reject::RBFRejected(..)
                        | Reject::Resolve(ckb_types::core::error::OutPointError::Dead(_))
                ) && tx_pool.as_ref().is_some_and(|pool| {
                    self.pipeline.kernel.read(|kernel| {
                        kernel.raw_by_hash(subject).is_some_and(|raw| {
                            pool.pool_map.find_conflict_outpoint(&raw.tx).is_some()
                        })
                    })
                })
            });
            let result = self.pipeline.kernel.mutate_authoritative(|coordinator| {
                let Some(record) = coordinator.terminal_record(subject) else {
                    return Ok(Err(PrePoolError::Missing(subject.clone())));
                };
                let (batch, peer_ban) = self.pipeline_outcome_effects(&record, reject.as_ref());
                let class = if matches!(record.source, PrePoolSource::Remote(_)) {
                    EffectClass::Remote
                } else {
                    EffectClass::Trusted
                };
                self.relay.effects.try_apply(batch, class, || {
                    let terminal = terminalize(coordinator, retain_conflict)?;
                    if let Some((peer, duration)) = peer_ban {
                        self.record_peer_ban(peer, duration);
                    }
                    Ok((terminal, peer_ban.map(|(peer, _)| peer)))
                })
            });
            drop(tx_pool);
            match result {
                Ok(Ok((_, banned_peer))) => {
                    if let Some(peer) = banned_peer {
                        self.remove_banned_peer_entries(peer).await;
                    }
                    return;
                }
                Ok(Err(error)) if error.is_stale_lease() => return,
                Ok(Err(error)) => {
                    self.pipeline.kernel.report_fault(mutation_context, &error);
                    return;
                }
                Err(EffectJournalError::Full) => continue,
                Err(error) => {
                    ckb_logger::error!("{mutation_context}: effect journal unavailable: {error:?}");
                    return;
                }
            }
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
    ) -> Option<ParentWaitOutcome> {
        self.settle_parent_wait(&lease.hash, dependencies, |coordinator, dependencies| {
            if dependencies.is_empty() {
                coordinator.requeue_resolve(lease).map(|_| ())
            } else {
                coordinator.wait_resolve(lease, dependencies).map(|_| ())
            }
        })
        .await
    }

    /// Verification can discover that its resolution snapshot went stale.
    /// Demote and re-resolve under the same atomic parent boundary used by the
    /// raw resolver, rather than creating a second orphan owner.
    pub(crate) async fn settle_verify_parent_wait(
        &self,
        lease: &VerifyLease,
        dependencies: BTreeSet<DependencyKey>,
    ) -> Option<ParentWaitOutcome> {
        self.settle_parent_wait(&lease.hash, dependencies, |coordinator, dependencies| {
            coordinator
                .verification_retry_resolution(lease, dependencies)
                .map(|_| ())
        })
        .await
    }

    /// Remove a transaction by hash from both ownership authorities: every
    /// pre-pool location and the accepted pool.
    pub(crate) async fn remove_tx(&self, tx_hash: Byte32) -> RemoveTxOutcome {
        let preview = self
            .pipeline
            .kernel
            .read(|kernel| kernel.terminal_record(&tx_hash));
        if let Some(batch) = preview
            .as_ref()
            .and_then(|record| self.pipeline_terminal_effects(std::slice::from_ref(record)))
            && self
                .relay
                .effects
                .wait_capacity(batch.charge_bytes(), EffectClass::Trusted)
                .await
                .is_err()
        {
            return RemoveTxOutcome::InProgress;
        }
        let id = ProposalShortId::from_tx_hash(&tx_hash);
        let mutation = {
            let mut tx_pool = self.pool.tx_pool.write().await;
            (|| -> Result<_, AdministrativeRemovalError> {
                let pool_target = tx_pool.get_tx_from_pool_by_hash(&tx_hash).is_some();
                let (record, removed_entries) = if pool_target {
                    let snapshot = tx_pool.cloned_snapshot();
                    let roots = HashSet::from([id.clone()]);
                    let removal = match tx_pool
                        .pool_map
                        .conflict_closure(&roots, tx_pool.pool_map.len())
                    {
                        crate::component::pool_map::ConflictClosure::Complete {
                            removal, ..
                        } => removal,
                        crate::component::pool_map::ConflictClosure::Exceeded { .. } => {
                            return Err(PoolMutationFault::ProjectionMismatch(
                                "administrative descendant closure exceeds membership",
                            )
                            .into());
                        }
                    };
                    let prepared = tx_pool.pool_map.prepare_removals(&removal)?.ok_or(
                        PoolMutationFault::MissingEntry("administrative removal root"),
                    )?;

                    let removal_hashes = prepared
                        .entries()
                        .map(|entry| entry.transaction().hash())
                        .filter(|hash| !snapshot.transaction_exists(hash))
                        .collect();
                    let removal_statuses = prepared
                        .records()
                        .map(|(status, _)| status)
                        .collect::<HashSet<_>>();
                    let released_inputs = prepared
                        .entries()
                        .filter(|entry| !snapshot.transaction_exists(&entry.transaction().hash()))
                        .flat_map(|entry| entry.transaction().input_pts_iter())
                        .collect::<Vec<_>>();
                    let available_dependencies =
                        crate::component::pre_pool::available_cell_keys(released_inputs)
                            .filter(|key| match key {
                                DependencyKey::Cell(out_point) => {
                                    prepared.contains_output_after_apply(out_point)
                                        || snapshot.get_cell(out_point).is_some()
                                }
                                DependencyKey::Header(hash) => snapshot.is_main_chain(hash),
                            })
                            .collect::<BTreeSet<_>>();

                    // Both authorities are now exclusively borrowed by
                    // validated capabilities. Neither Apply can fail, and no
                    // observer can acquire either authority between them.
                    let removed = self.pipeline.kernel.mutate_authoritative(|kernel| {
                        let prepared_kernel = kernel.prepare_dependency_reconciliation(
                            &removal_hashes,
                            available_dependencies,
                        )?;
                        let removed = prepared.apply();
                        prepared_kernel.apply();
                        Ok::<_, PrePoolError>(removed)
                    })?;
                    for status in removal_statuses {
                        self.journal_block_assembler_update(status);
                    }
                    (None, removed)
                } else {
                    let record = match self.pipeline.kernel.mutate_authoritative(|coordinator| {
                        let preview = coordinator.terminal_record(&tx_hash);
                        let batch = preview.as_ref().and_then(|record| {
                            self.pipeline_terminal_effects(std::slice::from_ref(record))
                        });
                        self.relay
                            .effects
                            .try_apply(batch, EffectClass::Trusted, || {
                                coordinator.force_terminalize(&tx_hash)
                            })
                    }) {
                        Ok(Ok(record)) => record,
                        Ok(Err(error)) => return Err(error.into()),
                        Err(error) => {
                            ckb_logger::error!(
                                "administrative removal journal unavailable: {error:?}"
                            );
                            return Ok(None);
                        }
                    };
                    (record, Vec::new())
                };
                let conflict_removed = false;
                Ok(Some((record, removed_entries, conflict_removed)))
            })()
        };
        let mutation = match mutation {
            Ok(mutation) => mutation,
            Err(AdministrativeRemovalError::Accepted(error)) => {
                self.pipeline.kernel.report_fault(
                    "administrative accepted-pool removal planning failed",
                    &error,
                );
                return RemoveTxOutcome::InProgress;
            }
            Err(AdministrativeRemovalError::PrePool(error)) => {
                self.pipeline.kernel.report_fault(
                    "administrative pre-pool reconciliation planning failed",
                    &error,
                );
                return RemoveTxOutcome::InProgress;
            }
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
        self.advance_pipeline_epoch();
        // This write guard is the clear/commit ordering barrier. The kernel
        // swap is O(1); its retired population is destroyed after the guard.
        let tx_pool = self.pool.tx_pool.write().await;
        let transition = self.pipeline.kernel.mutate_authoritative(|kernel| {
            self.relay
                .effects
                .apply_generation_reset(|| kernel.replace_empty_generation())
        });
        drop(tx_pool);
        match transition {
            Ok(retired) => drop(retired),
            Err(error) => self
                .pipeline
                .kernel
                .report_fault("clear-pipeline generation reset journal failed", &error),
        }
    }

    pub(crate) fn find_tx_in_coordinator_hash(&self, hash: &Byte32) -> Option<PipelineTxLocation> {
        self.pipeline
            .kernel
            .read(|coordinator| coordinator.tx_location_by_hash(hash))
    }

    /// Filter proposals down to those that are **completely new** to this
    /// node: owned by neither the pre-pool nor the accepted pool.
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
