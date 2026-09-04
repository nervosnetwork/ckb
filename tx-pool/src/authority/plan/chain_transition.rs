use super::apply_seal::{ScratchAuthority, ScratchAuthoritySeed};
use super::*;
use crate::authority::chain::{
    ChainCommittedOwner, ChainConflictOwner, ChainProposalSubject, ChainRecoveryOwner,
    ChainRecoveryReceipt, ChainRecoveryWork, ChainRemoval, ChainStatusSubject,
    ChainTransitionFactsView, ChainTransitionReceipt, ChainValidationWork,
    ExpectedPreAcceptedOwner,
};
use crate::authority::shard::ShardedOwnerMap;
use crate::authority::state::{DependencyOrigin, DependencySetError, RemoteBase};
use ckb_types::{core::TransactionView, packed::OutPoint};
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap, HashSet, VecDeque},
};

#[derive(Clone, Debug, PartialEq, Eq)]
enum CausalDisposition {
    Recovery,
    ChainConflictRemoval { out_point: OutPoint },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PreacceptedDisposition {
    Requeue,
    ChainConflictRemoval { out_point: OutPoint },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PreacceptedCausalAction {
    PreserveOwner,
    Requeue,
    Terminalize { out_point: OutPoint },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreacceptedCapability {
    Inactive,
    ActiveCompute,
}

impl PreacceptedCapability {
    fn from_phase(phase: &PreAcceptedPhase) -> Self {
        match phase {
            PreAcceptedPhase::Computing(_) => Self::ActiveCompute,
            PreAcceptedPhase::Queued(_)
            | PreAcceptedPhase::Waiting(_)
            | PreAcceptedPhase::Ready(_) => Self::Inactive,
        }
    }
}

impl CausalDisposition {
    /// Join the causal lattice explicitly. Enum declaration order is not a
    /// correctness policy: adding a cause must extend this closed matrix. If
    /// several attached inputs reach the same causal closure, canonical
    /// outpoint ordering makes the externally reported conflict deterministic.
    fn join(&self, incoming: &Self) -> Self {
        match self {
            Self::Recovery => match incoming {
                Self::Recovery => Self::Recovery,
                Self::ChainConflictRemoval { out_point } => Self::ChainConflictRemoval {
                    out_point: out_point.clone(),
                },
            },
            Self::ChainConflictRemoval { out_point } => match incoming {
                Self::Recovery => Self::ChainConflictRemoval {
                    out_point: out_point.clone(),
                },
                Self::ChainConflictRemoval {
                    out_point: incoming,
                } => Self::ChainConflictRemoval {
                    out_point: canonical_conflict_out_point(out_point, incoming),
                },
            },
        }
    }

    /// Closed `cause x phase` policy for a PreAccepted consumer. Accepted
    /// consumers always propagate the disposition; PreAccepted has no
    /// proposal status, and an active compute capability remains uniquely
    /// settleable across chain/dependency cuts.
    fn preaccepted_action(&self, phase: &PreAcceptedPhase) -> PreacceptedCausalAction {
        match (self, PreacceptedCapability::from_phase(phase)) {
            (
                Self::Recovery | Self::ChainConflictRemoval { .. },
                PreacceptedCapability::ActiveCompute,
            ) => PreacceptedCausalAction::PreserveOwner,
            (Self::Recovery, PreacceptedCapability::Inactive) => PreacceptedCausalAction::Requeue,
            (Self::ChainConflictRemoval { out_point }, PreacceptedCapability::Inactive) => {
                PreacceptedCausalAction::Terminalize {
                    out_point: out_point.clone(),
                }
            }
        }
    }
}

impl PreacceptedDisposition {
    fn join(&self, incoming: &Self) -> Self {
        match self {
            Self::Requeue => match incoming {
                Self::Requeue => Self::Requeue,
                Self::ChainConflictRemoval { out_point } => Self::ChainConflictRemoval {
                    out_point: out_point.clone(),
                },
            },
            Self::ChainConflictRemoval { out_point } => match incoming {
                Self::Requeue => Self::ChainConflictRemoval {
                    out_point: out_point.clone(),
                },
                Self::ChainConflictRemoval {
                    out_point: incoming,
                } => Self::ChainConflictRemoval {
                    out_point: canonical_conflict_out_point(out_point, incoming),
                },
            },
        }
    }
}

fn canonical_conflict_out_point(left: &OutPoint, right: &OutPoint) -> OutPoint {
    if left <= right {
        left.clone()
    } else {
        right.clone()
    }
}

struct PreparedOwnerChange {
    key: RawTxHash,
    before: Option<OwnedTx>,
    after: Option<OwnedTx>,
}

struct RecoveryCandidate<T> {
    recovery: T,
    dependencies: KnownDependencies,
}

/// Provenance-preserving payload for the one fresh-generation fallback.
/// Trusted detached/Accepted input is intentionally reconstructed as Recovery;
/// an already preaccepted owner retains its external Remote/Proposal source.
#[derive(Clone, Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "the bounded recovery vector intentionally owns preaccepted entries inline; boxing every recovered owner would add attacker-shaped allocations to generation fallback"
)]
pub(in crate::authority) enum ChainGenerationRecovery {
    Trusted(TransactionView),
    RequeueExisting(PreAcceptedEntry),
}

enum SelectedChainRecovery {
    Trusted { admission: ChargedAdmission },
    RequeueExisting { hash: RawTxHash },
}

impl SelectedChainRecovery {
    fn key(&self) -> &RawTxHash {
        match self {
            Self::Trusted { admission } => &admission.admission().identity.raw,
            Self::RequeueExisting { hash } => hash,
        }
    }
}

/// One bounded compiler owns the complete derived causal budget. Direct
/// attached/detached transactions are the canonical chain-transition input:
/// processing them is input-linear and cannot be truncated without changing
/// the chain cut. They therefore contribute to the explicit
/// `O(attached + detached)` term, but never consume the independent
/// affected-owner bound used for the derived closure.
struct CausalCompiler<'facts> {
    attached: &'facts HashSet<RawTxHash>,
    detached: &'facts HashSet<RawTxHash>,
    accepted: HashMap<RawTxHash, CausalDisposition>,
    frontier: VecDeque<RawTxHash>,
    preaccepted: HashMap<RawTxHash, PreacceptedDisposition>,
    max: usize,
}

impl<'facts> CausalCompiler<'facts> {
    fn new(
        attached: &'facts HashSet<RawTxHash>,
        detached: &'facts HashSet<RawTxHash>,
        max: usize,
    ) -> Self {
        Self {
            attached,
            detached,
            accepted: HashMap::with_capacity(max),
            frontier: VecDeque::with_capacity(max),
            preaccepted: HashMap::with_capacity(max),
            max,
        }
    }

    fn is_direct_fact(&self, hash: &RawTxHash) -> bool {
        self.attached.contains(hash) || self.detached.contains(hash)
    }

    fn reserve_new_owner(&self) -> Result<(), PlanError> {
        let used = self
            .accepted
            .len()
            .checked_add(self.preaccepted.len())
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        if used >= self.max {
            return Err(PlanError::Backpressure(Backpressure::GenerationReplacement));
        }
        Ok(())
    }

    fn enqueue(&mut self, hash: RawTxHash) {
        self.frontier.push_back(hash);
    }

    fn seed_accepted(
        &mut self,
        hash: RawTxHash,
        disposition: CausalDisposition,
    ) -> Result<(), PlanError> {
        if self.is_direct_fact(&hash) {
            return Ok(());
        }
        if let Some(current) = self.accepted.get_mut(&hash) {
            let joined = current.join(&disposition);
            if *current != joined {
                self.frontier.reserve(1);
                *current = joined;
                self.enqueue(hash);
            }
            return Ok(());
        }
        self.reserve_new_owner()?;
        self.frontier.reserve(1);
        self.accepted.insert(hash.clone(), disposition);
        self.enqueue(hash);
        Ok(())
    }

    fn seed_preaccepted(
        &mut self,
        hash: RawTxHash,
        disposition: PreacceptedDisposition,
    ) -> Result<(), PlanError> {
        if self.is_direct_fact(&hash) {
            return Ok(());
        }
        if let Some(current) = self.preaccepted.get_mut(&hash) {
            *current = current.join(&disposition);
            return Ok(());
        }
        self.reserve_new_owner()?;
        self.preaccepted.insert(hash, disposition);
        Ok(())
    }

    fn finish(
        self,
    ) -> (
        HashMap<RawTxHash, CausalDisposition>,
        HashMap<RawTxHash, PreacceptedDisposition>,
    ) {
        (self.accepted, self.preaccepted)
    }
}

impl TxPoolAuthority {
    /// Production chain commands retain their canonical block facts while
    /// this compiler allocates the affected owner slice. Returning an error
    /// therefore leaves the exact move-only command available for retry.
    pub(in crate::authority) fn chain_validation_work_from_view(
        &self,
        facts: ChainTransitionFactsView<'_>,
    ) -> Result<ChainValidationWork, PlanError> {
        if facts.new_view.revision() != next_chain_revision(self.chain_revision())? {
            return Err(PlanError::Stale(StalePlan::ChainRevision));
        }

        let max_affected = self.membership_config.max_component();
        let mut attached_hashes = HashSet::new();
        attached_hashes.reserve(facts.attached.len());
        attached_hashes.extend(
            facts
                .attached
                .iter()
                .map(|transaction| RawTxHash(transaction.hash())),
        );
        let mut detached_hashes = HashSet::new();
        detached_hashes.reserve(facts.detached.len());
        detached_hashes.extend(
            facts
                .detached
                .iter()
                .map(|transaction| RawTxHash(transaction.hash())),
        );

        let mut causal = CausalCompiler::new(&attached_hashes, &detached_hashes, max_affected);

        // Attached inputs kill both accepted spenders and accepted cell-dep
        // readers. The attached transaction itself is a committed removal,
        // not its own conflict root.
        for transaction in facts.attached {
            let attached_hash = RawTxHash(transaction.hash());
            for input in transaction.input_pts_iter() {
                // Molecule iterators may share the attached transaction's
                // backing bytes. The committed reason retains only this
                // compact cell identity, never the whole block transaction.
                let conflict_out_point = crate::util::compact_packed(&input);
                if let Some(spender) = self.membership.spender(&input)
                    && spender != attached_hash
                {
                    causal.seed_accepted(
                        spender.clone(),
                        CausalDisposition::ChainConflictRemoval {
                            out_point: conflict_out_point.clone(),
                        },
                    )?;
                }
                self.seed_consumers(
                    &DependencyKey::Cell(input),
                    CausalDisposition::ChainConflictRemoval {
                        out_point: conflict_out_point,
                    },
                    &mut causal,
                )?;
            }
        }

        // A detached producer/header changes the role of each dependent. An
        // accepted dependent and its causal descendants return through normal
        // Recovery; preaccepted work is requeued under the new view.
        for transaction in facts.detached {
            self.seed_transaction_consumers(transaction, CausalDisposition::Recovery, &mut causal)?;
        }
        for header in facts.detached_headers {
            self.seed_consumers(
                &DependencyKey::Header(header.clone()),
                CausalDisposition::Recovery,
                &mut causal,
            )?;
        }
        // Same raw producer on both forks preserves content identity but not
        // inclusion height/epoch. Consumers must rebuild location/time proof
        // even though detached-payload recovery is correctly suppressed.
        for transaction in facts.attached {
            if facts
                .relocated
                .binary_search(&RawTxHash(transaction.hash()))
                .is_ok()
            {
                self.seed_transaction_consumers(
                    transaction,
                    CausalDisposition::Recovery,
                    &mut causal,
                )?;
            }
        }

        // A genuine detach can move tip height/epoch/median-time independently
        // of explicit producer loss. Only validation-proven contextual owners
        // enter this derived index; stable Accepted membership remains O(1).
        match facts.accepted_validity {
            crate::authority::chain::AcceptedValidityTransition::Preserved => {}
            crate::authority::chain::AcceptedValidityTransition::ContextChanged => {
                let context_sensitive = self.indexes.context_sensitive_accepted();
                for hash in context_sensitive.iter() {
                    if !matches!(
                        self.entries.get(hash).as_deref(),
                        Some(OwnedTx::Accepted(_))
                    ) {
                        return Err(PlanError::Fault(AuthorityFault::IndexProjection));
                    }
                    causal.seed_accepted(hash.clone(), CausalDisposition::Recovery)?;
                }
            }
            crate::authority::chain::AcceptedValidityTransition::RulesChanged => {
                let owners = self.entries.read_all();
                for (hash, owner) in &owners {
                    if matches!(owner, OwnedTx::Accepted(_)) {
                        causal.seed_accepted(hash.clone(), CausalDisposition::Recovery)?;
                    }
                }
            }
        }

        // One priority-tagged traversal computes all dependency consequences.
        // The owner-derived frontier covers both input children and cell-dep
        // readers, while also requeueing affected PreAccepted consumers.
        // Higher-priority conflict can upgrade a previously visited recovery
        // subtree and is then propagated exactly once at the stronger level.
        while let Some(hash) = causal.frontier.pop_front() {
            let disposition = causal
                .accepted
                .get(&hash)
                .cloned()
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            let owner = self
                .entries
                .get(&hash)
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            let OwnedTx::Accepted(entry) = &*owner else {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            };
            self.seed_transaction_consumers(&entry.record.tx, disposition, &mut causal)?;
        }
        let (dispositions, preaccepted_dispositions) = causal.finish();

        let mut removals = Vec::with_capacity(
            facts
                .attached
                .len()
                .checked_add(dispositions.len())
                .and_then(|count| count.checked_add(preaccepted_dispositions.len()))
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?,
        );
        for hash in &attached_hashes {
            if let Some(owner) = self.entries.get(hash) {
                removals.push(ChainRemoval::Committed {
                    hash: hash.clone(),
                    expected: chain_committed_owner(&owner),
                });
            }
        }
        for (hash, disposition) in &dispositions {
            let removal = match disposition {
                CausalDisposition::Recovery => {
                    let owner = self.entries.get(hash);
                    let Some(OwnedTx::Accepted(entry)) = owner.as_deref() else {
                        return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                    };
                    ChainRemoval::Recovery {
                        hash: hash.clone(),
                        expected: entry.record.version,
                    }
                }
                CausalDisposition::ChainConflictRemoval { out_point } => {
                    let Some(owner) = self.entries.get(hash) else {
                        return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                    };
                    ChainRemoval::ChainConflict {
                        hash: hash.clone(),
                        expected: chain_conflict_owner(&owner)?,
                        out_point: out_point.clone(),
                    }
                }
            };
            removals.push(removal);
        }
        for (hash, disposition) in &preaccepted_dispositions {
            match disposition {
                PreacceptedDisposition::Requeue => {}
                PreacceptedDisposition::ChainConflictRemoval { out_point } => {
                    let owner = self.entries.get(hash);
                    let Some(OwnedTx::PreAccepted(entry)) = owner.as_deref() else {
                        return Err(PlanError::Fault(AuthorityFault::DependencyProjection));
                    };
                    removals.push(ChainRemoval::ChainConflict {
                        hash: hash.clone(),
                        expected: ChainConflictOwner::PreAccepted(expected_preaccepted(entry)),
                        out_point: out_point.clone(),
                    });
                }
            }
        }
        removals.sort_unstable_by(|left, right| left.hash().cmp(right.hash()));
        removals.dedup_by(|left, right| left.hash() == right.hash());

        // Status is meaningful only for owners whose projected final state is
        // still Accepted. Besides terminal removals, every direct detached
        // transaction is recovered into PreAccepted ownership; compiling a
        // proposal-window update for the same raw hash would create two owner
        // changes from one chain fact.
        let mut non_status_hashes = HashSet::new();
        non_status_hashes.reserve(
            removals
                .len()
                .checked_add(detached_hashes.len())
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?,
        );
        non_status_hashes.extend(removals.iter().map(|removal| removal.hash().clone()));
        non_status_hashes.extend(detached_hashes.iter().cloned());
        let mut proposal_candidates = Vec::with_capacity(facts.changed_proposals.len());
        proposal_candidates.extend(facts.changed_proposals.iter().cloned());

        let mut status_subjects = HashMap::<RawTxHash, ChainStatusSubject>::new();
        status_subjects.reserve(proposal_candidates.len());
        let mut proposal_subjects = Vec::with_capacity(facts.changed_proposals.len());
        for proposal in &proposal_candidates {
            let Some(hash) = self.indexes.proposal_owner(proposal) else {
                continue;
            };
            if non_status_hashes.contains(&hash) {
                continue;
            }
            let owner = self.entries.get(&hash);
            match owner.as_deref() {
                Some(OwnedTx::Accepted(entry)) => {
                    status_subjects.insert(
                        hash.clone(),
                        ChainStatusSubject {
                            hash: hash.clone(),
                            expected: entry.record.version,
                            proposal: proposal.clone(),
                            before: entry.status(),
                        },
                    );
                }
                Some(OwnedTx::PreAccepted(entry)) => match entry.source {
                    PreAcceptedSource::Proposal { base, .. } => {
                        proposal_subjects.push(ChainProposalSubject {
                            hash: hash.clone(),
                            expected: expected_preaccepted(entry),
                            proposal: proposal.clone(),
                            base,
                        });
                    }
                    PreAcceptedSource::Remote(_) | PreAcceptedSource::Recovery(_) => {}
                },
                Some(OwnedTx::ReplacementHistory(_)) => {}
                None => return Err(PlanError::Fault(AuthorityFault::IndexProjection)),
            }
        }
        let mut ordered_status_subjects = Vec::with_capacity(status_subjects.len());
        ordered_status_subjects.extend(status_subjects.into_values());
        let mut status_subjects = ordered_status_subjects;
        status_subjects.sort_unstable_by(|left, right| left.hash.cmp(&right.hash));
        proposal_subjects.sort_unstable_by(|left, right| left.hash.cmp(&right.hash));

        let (available, lost) = self.chain_dependency_events(
            facts.attached,
            facts.detached,
            &removals,
            facts.attached_headers,
            facts.detached_headers,
        )?;
        let recovery_transactions = self.prepare_recovery_transactions(
            facts.detached,
            &dispositions,
            &preaccepted_dispositions,
        )?;
        let mut recoveries = Vec::with_capacity(recovery_transactions.len());
        for transaction in recovery_transactions {
            let hash = RawTxHash(transaction.hash());
            let requeue_existing = match preaccepted_dispositions.get(&hash) {
                Some(PreacceptedDisposition::Requeue) => !detached_hashes.contains(&hash),
                Some(PreacceptedDisposition::ChainConflictRemoval { .. }) | None => false,
            };
            if requeue_existing {
                let owner = self.entries.get(&hash);
                let Some(OwnedTx::PreAccepted(entry)) = owner.as_deref() else {
                    return Err(PlanError::Fault(AuthorityFault::DependencyProjection));
                };
                recoveries.push(ChainRecoveryWork::RequeueExisting {
                    hash,
                    expected: expected_preaccepted(entry),
                });
            } else {
                recoveries.push(ChainRecoveryWork::Trusted {
                    expected: chain_recovery_owner(self.entries.get(&hash).as_deref()),
                    transaction,
                });
            }
        }

        let mut committed = Vec::with_capacity(facts.attached.len());
        committed.extend(
            facts
                .attached
                .iter()
                .map(|transaction| RawTxHash(transaction.hash())),
        );

        Ok(ChainValidationWork {
            generation: self.generation,
            old_view: self.chain_view.clone(),
            new_view: facts.new_view,
            committed,
            removals,
            recoveries,
            status_subjects,
            proposal_subjects,
            chain_available: available,
            chain_lost: lost,
        })
    }

    fn seed_transaction_consumers(
        &self,
        transaction: &TransactionView,
        disposition: CausalDisposition,
        causal: &mut CausalCompiler<'_>,
    ) -> Result<(), PlanError> {
        for output in transaction.output_pts() {
            self.seed_consumers(&DependencyKey::Cell(output), disposition.clone(), causal)?;
        }
        Ok(())
    }

    fn seed_consumers(
        &self,
        key: &DependencyKey,
        disposition: CausalDisposition,
        causal: &mut CausalCompiler<'_>,
    ) -> Result<(), PlanError> {
        let Some(consumers) = self.dependencies.consumers_for(key)? else {
            return Ok(());
        };
        for hash in consumers {
            if causal.is_direct_fact(&hash) {
                continue;
            }
            let owner = self.entries.get(&hash);
            match owner.as_deref() {
                Some(OwnedTx::Accepted(_)) => {
                    causal.seed_accepted(hash.clone(), disposition.clone())?;
                }
                Some(OwnedTx::PreAccepted(entry)) => {
                    match disposition.preaccepted_action(&entry.phase) {
                        PreacceptedCausalAction::PreserveOwner => {}
                        PreacceptedCausalAction::Requeue => causal
                            .seed_preaccepted(hash.clone(), PreacceptedDisposition::Requeue)?,
                        PreacceptedCausalAction::Terminalize { out_point } => causal
                            .seed_preaccepted(
                                hash.clone(),
                                PreacceptedDisposition::ChainConflictRemoval { out_point },
                            )?,
                    }
                }
                Some(OwnedTx::ReplacementHistory(_)) => {
                    // A definitive chain loss keeps replacement history
                    // parked. Only a later availability event may promote it
                    // back to executable recovery work.
                }
                None => return Err(PlanError::Fault(AuthorityFault::DependencyProjection)),
            }
        }
        Ok(())
    }

    fn prepare_recovery_transactions(
        &self,
        detached: &[TransactionView],
        dispositions: &HashMap<RawTxHash, CausalDisposition>,
        preaccepted: &HashMap<RawTxHash, PreacceptedDisposition>,
    ) -> Result<Vec<TransactionView>, PlanError> {
        let capacity = detached
            .len()
            .checked_add(dispositions.len())
            .and_then(|count| count.checked_add(preaccepted.len()))
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        let mut by_hash = HashMap::new();
        by_hash.reserve(capacity);
        // Detached block payload is authoritative for its witness variant.
        for transaction in detached {
            let transaction = transaction.clone();
            let dependencies = declared_dependencies(&transaction)?;
            by_hash.insert(
                RawTxHash(transaction.hash()),
                RecoveryCandidate {
                    recovery: transaction,
                    dependencies,
                },
            );
        }
        for (hash, disposition) in dispositions {
            match disposition {
                CausalDisposition::Recovery => {}
                CausalDisposition::ChainConflictRemoval { .. } => {
                    continue;
                }
            }
            if by_hash.contains_key(hash) {
                continue;
            }
            let entry = self
                .entries
                .get(hash)
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            by_hash.insert(
                hash.clone(),
                RecoveryCandidate {
                    recovery: entry.record().tx.as_ref().clone(),
                    dependencies: entry.dependencies().clone(),
                },
            );
        }
        for (hash, disposition) in preaccepted {
            match disposition {
                PreacceptedDisposition::Requeue => {}
                PreacceptedDisposition::ChainConflictRemoval { .. } => continue,
            }
            if by_hash.contains_key(hash) {
                continue;
            }
            let entry = self
                .entries
                .get(hash)
                .ok_or(PlanError::Fault(AuthorityFault::DependencyProjection))?;
            by_hash.insert(
                hash.clone(),
                RecoveryCandidate {
                    recovery: entry.record().tx.as_ref().clone(),
                    dependencies: entry.dependencies().clone(),
                },
            );
        }
        topological_recoveries(by_hash)
    }

    fn chain_dependency_events(
        &self,
        attached: &[TransactionView],
        detached: &[TransactionView],
        removals: &[ChainRemoval],
        attached_headers: &[ckb_types::packed::Byte32],
        detached_headers: &[ckb_types::packed::Byte32],
    ) -> Result<(Vec<DependencyKey>, Vec<DependencyKey>), PlanError> {
        let mut available = Vec::new();
        let mut lost = Vec::new();
        for transaction in attached {
            append_transaction_output_keys(&mut available, transaction);
            append_keys(
                &mut lost,
                transaction.input_pts_iter().map(DependencyKey::Cell),
            );
        }
        // Detached block facts change chain availability whether or not the
        // same raw transaction already has a pool owner. They are deliberately
        // distinct from preaccepted dependents that are merely requeued.
        for transaction in detached {
            append_transaction_output_keys(&mut lost, transaction);
            append_keys(
                &mut available,
                transaction.input_pts_iter().map(DependencyKey::Cell),
            );
        }
        for removal in removals {
            match removal {
                ChainRemoval::Committed { .. } => continue,
                ChainRemoval::ChainConflict { .. }
                | ChainRemoval::Recovery { .. }
                | ChainRemoval::ProposalWindowExpired { .. } => {}
            }
            let owner = self
                .entries
                .get(removal.hash())
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            // Accepted owns the pool overlay, so its removal both releases
            // inputs and removes produced outputs. A PreAccepted producer
            // never made its inputs unavailable, but its definitive removal
            // still invalidates every child waiting or computing against its
            // outputs. Publishing that loss here is what makes a preserved
            // active child's old DependencyCut stale without revoking the
            // child's unique compute capability.
            match &*owner {
                OwnedTx::PreAccepted(_) => {
                    append_transaction_output_keys(&mut lost, &owner.record().tx);
                }
                OwnedTx::Accepted(_) => {
                    append_transaction_output_keys(&mut lost, &owner.record().tx);
                    append_keys(
                        &mut available,
                        owner.record().tx.input_pts_iter().map(DependencyKey::Cell),
                    );
                }
                OwnedTx::ReplacementHistory(_) => {}
            }
        }
        for header in attached_headers {
            available.push(DependencyKey::Header(header.clone()));
        }
        for header in detached_headers {
            lost.push(DependencyKey::Header(header.clone()));
        }
        available.sort_unstable();
        available.dedup();
        lost.sort_unstable();
        lost.dedup();
        // Final dead/spent state wins when a key appears in both histories
        // (for example an output created and spent within the attached set).
        available.retain(|key| lost.binary_search(key).is_err());
        Ok((available, lost))
    }

    pub(in crate::authority) fn plan_chain_transition(
        &mut self,
        receipt: ChainTransitionReceipt,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.effects.lock().ensure_open()?;
        if receipt.generation != self.generation {
            return Err(PlanError::Stale(StalePlan::Generation));
        }
        if receipt.old_view != self.chain_view
            || receipt.new_view.revision() != next_chain_revision(self.chain_revision())?
        {
            return Err(PlanError::Stale(StalePlan::ChainRevision));
        }
        validate_chain_receipt_owners(&self.entries, &receipt)?;

        // A preaccepted owner may arrive for an attached raw hash while the
        // lock-outside receipt is being validated. Committed raw hashes need
        // no snapshot proof to remove, so absorb that race at final Plan
        // instead of versioning every absent transaction in a large block.
        let mut removals = receipt.removals;
        removals.reserve(receipt.committed.len());
        let mut removal_hashes = HashSet::new();
        removal_hashes.reserve(
            removals
                .len()
                .checked_add(receipt.committed.len())
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?,
        );
        removal_hashes.extend(removals.iter().map(|removal| removal.hash().clone()));
        for hash in &receipt.committed {
            let Some(owner) = self.entries.get(hash) else {
                continue;
            };
            if removal_hashes.insert(hash.clone()) {
                removals.push(ChainRemoval::Committed {
                    hash: hash.clone(),
                    expected: chain_committed_owner(&owner),
                });
            }
        }
        removals.sort_unstable_by(|left, right| left.hash().cmp(right.hash()));

        let recoveries = self.select_chain_recoveries(receipt.recoveries)?;

        let mut clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let change_capacity = removals
            .len()
            .checked_add(recoveries.len())
            .and_then(|count| count.checked_add(receipt.proposal_demotions.len()))
            .and_then(|count| count.checked_add(receipt.statuses.len()))
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        let mut changes = Vec::with_capacity(change_capacity);

        let mut recovery_hashes = HashSet::new();
        recovery_hashes.reserve(recoveries.len());
        for recovery in &recoveries {
            if !recovery_hashes.insert(recovery.key().clone()) {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            }
        }
        for removal in &removals {
            if recovery_hashes.contains(removal.hash()) {
                continue;
            }
            let before = self
                .entries
                .get(removal.hash())
                .as_deref()
                .cloned()
                .ok_or(PlanError::Stale(StalePlan::Missing))?;
            changes.push(PreparedOwnerChange {
                key: removal.hash().clone(),
                before: Some(before),
                after: None,
            });
        }
        for recovery in recoveries {
            let (version, arrival) = clocks.insertion()?;
            let key = recovery.key().clone();
            let before = self.entries.get(&key).as_deref().cloned();
            let after = match recovery {
                SelectedChainRecovery::Trusted { admission } => {
                    queued_recovery_owner(admission, version, arrival)
                }
                SelectedChainRecovery::RequeueExisting { .. } => {
                    let Some(OwnedTx::PreAccepted(entry)) = before.clone() else {
                        return Err(PlanError::Stale(StalePlan::Phase));
                    };
                    requeued_existing_owner(entry, version, arrival)?
                }
            };
            changes.push(PreparedOwnerChange {
                key,
                before,
                after: Some(after),
            });
        }

        for demotion in receipt.proposal_demotions {
            let key = demotion.hash;
            let before = self
                .entries
                .get(&key)
                .as_deref()
                .cloned()
                .ok_or(PlanError::Stale(StalePlan::Missing))?;
            let OwnedTx::PreAccepted(mut after) = before.clone() else {
                return Err(PlanError::Stale(StalePlan::Phase));
            };
            let PreAcceptedSource::Proposal {
                base: ProposalBase::Remote(residency),
                ..
            } = after.source
            else {
                return Err(PlanError::Stale(StalePlan::Phase));
            };
            // A source-only demotion preserves EntryVersion and any unique
            // compute capability. The owner-typed demotion carries the exact
            // source fact that makes this safe despite the unchanged token.
            after.source = PreAcceptedSource::Remote(RemoteBase {
                residency,
                payload_policy: PayloadPolicy::Trusted,
            });
            changes.push(PreparedOwnerChange {
                key,
                before: Some(before),
                after: Some(OwnedTx::PreAccepted(after)),
            });
        }

        let mut status_after = HashMap::new();
        status_after.reserve(receipt.statuses.len());
        for change in receipt.statuses {
            let hash = change.hash;
            let proposal = change.after;
            let status = proposal.status();
            let before = self
                .entries
                .get(&hash)
                .as_deref()
                .cloned()
                .ok_or(PlanError::Stale(StalePlan::Missing))?;
            let OwnedTx::Accepted(mut after) = before.clone() else {
                return Err(PlanError::Stale(StalePlan::Phase));
            };
            if after.status() == status {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            }
            let version = clocks.replacement()?;
            after.record.version = version;
            after.proposal = proposal;
            status_after.insert(hash.clone(), after.clone());
            changes.push(PreparedOwnerChange {
                key: hash,
                before: Some(before),
                after: Some(OwnedTx::Accepted(after)),
            });
        }
        changes.sort_unstable_by(|left, right| left.key.cmp(&right.key));
        if changes
            .array_windows::<2>()
            .any(|[left, right]| left.key == right.key)
        {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        }
        let mut accepted_removals = Vec::with_capacity(changes.len());
        for change in &changes {
            match (&change.before, &change.after) {
                (
                    Some(OwnedTx::Accepted(_)),
                    Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None,
                ) => {
                    accepted_removals.push(change.key.clone());
                }
                (Some(OwnedTx::Accepted(_)), Some(OwnedTx::Accepted(_)))
                | (
                    Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None,
                    Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None,
                )
                | (
                    Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None,
                    Some(OwnedTx::Accepted(_)),
                ) => {}
            }
        }
        let accepted_removals = AcceptedRemovalSet::try_from_vec(accepted_removals)?;
        let membership = self.prepare_chain_projection(&accepted_removals, &status_after)?;

        let mut resource_changes = Vec::with_capacity(changes.len());
        resource_changes.extend(changes.iter().map(|change| {
            (
                change.key.clone(),
                change.before.as_ref().map(OwnedTx::charge_record),
                change.after.as_ref().map(OwnedTx::charge_record),
            )
        }));
        let resources = self
            .resources_for_plan()
            .plan_batch(resource_changes)
            .map_err(chain_resource_error)?;
        let scheduler = self.scheduler.lock().plan_batch(
            changes
                .iter()
                .map(|change| (change.before.as_ref(), change.after.as_ref())),
        )?;
        let mut available = receipt.chain_available;
        let mut lost = receipt.chain_lost;
        for removal in &removals {
            match removal {
                ChainRemoval::ProposalWindowExpired { .. } => {
                    let owner = self
                        .entries
                        .get(removal.hash())
                        .ok_or(PlanError::Stale(StalePlan::Missing))?;
                    append_transaction_output_keys(&mut lost, &owner.record().tx);
                }
                ChainRemoval::Committed { .. }
                | ChainRemoval::ChainConflict { .. }
                | ChainRemoval::Recovery { .. } => {}
            }
        }
        lost.sort_unstable();
        lost.dedup();
        available.retain(|key| lost.binary_search(key).is_err());
        // The receipt carries immutable chain-layer facts, while the
        // dependency frontier publishes final combined availability. A cell
        // created on chain is still unavailable when the projected Accepted
        // membership retains a spender (notably an RBF winner whose parent is
        // committed later). Reading through the already-compiled membership
        // delta keeps both decisions on the same Apply cut without a second
        // projection, lock, or repair pass.
        available.retain(|key| match key {
            DependencyKey::Cell(input) => {
                membership.spender_after(&self.membership, input).is_none()
            }
            DependencyKey::Header(_) => true,
        });
        let control = self
            .dependencies
            .plan_events(available, lost, DependencyCut(sequence))?
            .unwrap_or_default();
        let dependency = self
            .dependencies
            .plan_primary_replacements(
                changes
                    .iter()
                    .map(|change| (change.before.as_ref(), change.after.as_ref())),
            )?
            .with_control(control.into(), &self.dependencies)?;
        let sources = self.source_versions.plan_chain_replacements(
            changes
                .iter()
                .map(|change| (change.before.as_ref(), change.after.as_ref())),
            sequence,
        );
        let template_sources = self.plan_owner_sources(
            changes
                .iter()
                .map(|change| (&change.key, change.before.as_ref(), change.after.as_ref())),
        )?;
        let indexes = self
            .indexes_for_plan()
            .plan_replacements(
                changes
                    .iter()
                    .map(|change| (&change.key, change.before.as_ref(), change.after.as_ref())),
            )
            .map_err(chain_index_error)?;
        let owners = DerivedOwnerDelta {
            indexes,
            sources,
            template_sources,
        };

        let mut effects = Vec::with_capacity(
            removals
                .len()
                .checked_add(status_after.len())
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?,
        );
        for removal in &removals {
            let owner = self
                .entries
                .get(removal.hash())
                .ok_or(PlanError::Stale(StalePlan::Missing))?;
            match removal {
                ChainRemoval::Committed { .. } => {
                    // Accepted membership already settled the relayer at
                    // admission, while replacement history is deliberately
                    // invisible. Only an in-flight Remote owner needs a
                    // successful verification result when the chain wins the
                    // race; this preserves known/original-peer semantics
                    // without inventing an Accepted callback.
                    if let OwnedTx::PreAccepted(_) = &*owner
                        && let Some(ingress_peer) = owner.ingress_peer()
                    {
                        effects.push(CommittedEffect::ChainCommitted {
                            tx_hash: owner.record().identity.raw.clone(),
                            ingress_peer,
                        });
                    }
                }
                ChainRemoval::ChainConflict { out_point, .. } => {
                    let conflict_owner = match &*owner {
                        OwnedTx::PreAccepted(entry) => CommittedConflictOwner::PreAccepted {
                            tx: Arc::clone(&entry.record.tx),
                            audience: RejectionAudience::from_source(entry.source),
                        },
                        OwnedTx::Accepted(entry) => {
                            CommittedConflictOwner::Accepted(self.committed_entry_before(entry)?)
                        }
                        OwnedTx::ReplacementHistory(_) => {
                            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                        }
                    };
                    effects.push(CommittedEffect::Rejected(
                        CommittedRejection::ChainConflict {
                            owner: conflict_owner,
                            out_point: out_point.clone(),
                        },
                    ));
                }
                ChainRemoval::Recovery { .. } | ChainRemoval::ProposalWindowExpired { .. } => {}
            }
        }
        let mut status_effect_keys = Vec::with_capacity(status_after.len());
        status_effect_keys.extend(status_after.keys().cloned());
        status_effect_keys.sort_unstable();
        for hash in status_effect_keys {
            let entry = status_after
                .get(&hash)
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            effects.push(CommittedEffect::Accepted(
                CommittedAcceptance::ChainStatusChange {
                    entry: self.committed_entry_after(entry, &membership)?,
                    status: entry.status(),
                },
            ));
        }
        let effect = if effects.is_empty() {
            EffectDelta::default()
        } else {
            self.effects_for_plan()
                .plan_chain_rebuildable(effects, sequence)?
        };

        // Keep the potentially large primary-map reservation behind all
        // semantic/resource checks. An over-bound recovery must not grow the
        // live authority merely because its rejected Plan carried many
        // detached transactions.
        self.reserve_primary_owner_insertions(
            changes
                .iter()
                .filter(|change| change.before.is_none() && change.after.is_some())
                .map(|change| &change.key),
        );
        let retired = retired_buffer(changes.len());
        let mut updates = Vec::with_capacity(changes.len());
        updates.extend(changes.into_iter().map(|change| ChainOwnerUpdate {
            key: change.key,
            after: change.after,
        }));
        PreparedApply::prepare(
            self,
            DependencyAuthorityDelta::Chain(ChainDelta {
                view: receipt.new_view,
                updates,
                owners,
                resources,
                membership,
                scheduler,
                dependency,
                effect,
                retired,
            }),
        )
    }

    fn select_chain_recoveries(
        &self,
        recoveries: Vec<ChainRecoveryReceipt>,
    ) -> Result<Vec<SelectedChainRecovery>, PlanError> {
        let mut all = HashSet::new();
        all.reserve(recoveries.len());
        for recovery in &recoveries {
            if !all.insert(recovery.key().clone()) {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            }
        }
        let mut seen = HashSet::new();
        seen.reserve(recoveries.len());
        let mut excluded = HashSet::new();
        excluded.reserve(recoveries.len());
        let mut selected = Vec::with_capacity(recoveries.len());

        for recovery in recoveries {
            let key = recovery.key().clone();
            let dependencies = match &recovery {
                ChainRecoveryReceipt::Trusted { admission, .. } => admission.dependencies.clone(),
                ChainRecoveryReceipt::RequeueExisting { hash, .. } => self
                    .entries
                    .get(hash)
                    .as_deref()
                    .map(OwnedTx::dependencies)
                    .cloned()
                    .ok_or(PlanError::Stale(StalePlan::Missing))?,
            };
            for dependency in dependencies.keys() {
                let DependencyOrigin::Transaction(parent) = dependency.origin() else {
                    continue;
                };
                if all.contains(&parent) && !seen.contains(&parent) {
                    return Err(PlanError::Fault(AuthorityFault::DependencyProjection));
                }
            }
            let parent_excluded = recovery_parent_is_excluded(&dependencies, &excluded);

            match recovery {
                ChainRecoveryReceipt::Trusted { admission, .. } => {
                    if parent_excluded {
                        excluded.insert(key.clone());
                    } else if let Some(admission) =
                        charge_chain_recovery(&self.resources, admission)?
                    {
                        selected.push(SelectedChainRecovery::Trusted { admission });
                    } else {
                        excluded.insert(key.clone());
                    }
                }
                ChainRecoveryReceipt::RequeueExisting { hash, .. } => {
                    // Existing preaccepted owners must still discard stale
                    // chain evidence. Their next Resolve decides whether the
                    // unavailable parent is waitable under current source
                    // policy; recovery exclusion applies only to new trusted
                    // ownership reconstructed from detached chain facts.
                    selected.push(SelectedChainRecovery::RequeueExisting { hash });
                }
            }
            seen.insert(key);
        }
        Ok(selected)
    }

    /// Capture the parent-first recovery payload before consuming a detailed
    /// receipt. The runtime retains this bounded value only across the final
    /// detailed Plan attempt, so an unrepresentable transition can converge
    /// through the one fresh-generation fallback without reconstructing
    /// semantic causes after the receipt has been consumed.
    pub(in crate::authority) fn chain_generation_recoveries(
        &self,
        receipt: &ChainTransitionReceipt,
    ) -> Result<Vec<ChainGenerationRecovery>, PlanError> {
        let mut recoveries = Vec::with_capacity(receipt.recoveries.len());
        for recovery in &receipt.recoveries {
            let recovery = match recovery {
                ChainRecoveryReceipt::Trusted { admission, .. } => {
                    ChainGenerationRecovery::Trusted(admission.tx.as_ref().clone())
                }
                ChainRecoveryReceipt::RequeueExisting { hash, expected } => {
                    let owner = self.entries.get(hash);
                    if !expected_preaccepted_matches(*expected, owner.as_deref()) {
                        return Err(PlanError::Stale(StalePlan::Version));
                    }
                    let Some(OwnedTx::PreAccepted(entry)) = owner.as_deref() else {
                        return Err(PlanError::Stale(StalePlan::Phase));
                    };
                    ChainGenerationRecovery::RequeueExisting(entry.clone())
                }
            };
            recoveries.push(recovery);
        }
        Ok(recoveries)
    }

    /// Replace an over-bound chain transition with a deterministic
    /// closure-safe parent-first recovery subset in a fresh generation. The scratch
    /// authority reuses the ordinary admission and every derived projection
    /// compiler; the live authority changes only through the final O(1)
    /// generation swap and emits one rebuildable GenerationReset effect.
    pub(in crate::authority) fn plan_chain_generation_replacement(
        &mut self,
        new_view: ChainViewId,
        transactions: Vec<TransactionView>,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let mut recoveries = Vec::with_capacity(transactions.len());
        recoveries.extend(
            transactions
                .into_iter()
                .map(ChainGenerationRecovery::Trusted),
        );
        self.plan_chain_generation_replacement_preserving_sources(new_view, recoveries)
    }

    pub(in crate::authority) fn plan_chain_generation_replacement_preserving_sources(
        &mut self,
        new_view: ChainViewId,
        recoveries: Vec<ChainGenerationRecovery>,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.effects.lock().ensure_open()?;
        if new_view.revision() != next_chain_revision(self.chain_revision())? {
            return Err(PlanError::Stale(StalePlan::ChainRevision));
        }

        let generation = next_generation(self.generation)?;
        let ordered = canonical_generation_recoveries(recoveries)?;
        let scratch_effects =
            EffectLog::new(self.effects.lock().limits()).map_err(|error| match error {
                EffectConfigError::Allocation => PlanError::Fault(AuthorityFault::EffectProjection),
                EffectConfigError::EmptyRemoteRegion
                | EffectConfigError::EmptyBatchBound
                | EffectConfigError::Arithmetic
                | EffectConfigError::IndivisibleBatch => {
                    PlanError::Fault(AuthorityFault::EffectProjection)
                }
            })?;
        let mut scratch = ScratchAuthority::assemble(
            self.resources.limits(),
            self.scheduler.lock().verify_order(),
            scratch_effects,
            self.membership_config,
            ScratchAuthoritySeed::new(
                new_view.clone(),
                generation,
                self.clocks.snapshot(),
                self.entries.router(),
            ),
        );

        let mut excluded = HashSet::new();
        excluded.reserve(ordered.len());
        for recovery in ordered {
            let (admission, expected_charge) = match recovery {
                ChainGenerationRecovery::Trusted(transaction) => (
                    ValidatedAdmission::recovery(transaction, generation).map_err(|error| {
                        match error {
                            super::super::state::RecoveryAdmissionError::ResourceUnavailable => {
                                PlanError::Fault(AuthorityFault::ResourceProjection)
                            }
                            super::super::state::RecoveryAdmissionError::InvalidTransaction => {
                                PlanError::Fault(AuthorityFault::ResourceProjection)
                            }
                        }
                    })?,
                    None,
                ),
                ChainGenerationRecovery::RequeueExisting(entry) => {
                    let (admission, original_charge) =
                        ValidatedAdmission::generation_requeue(entry, generation);
                    (admission, Some(original_charge))
                }
            };
            let key = admission.identity.raw.clone();
            if recovery_parent_is_excluded(&admission.dependencies, &excluded) {
                excluded.insert(key);
                continue;
            }
            let Some(admission) = charge_chain_recovery(scratch.resources(), admission)? else {
                excluded.insert(key);
                continue;
            };
            if expected_charge.is_some_and(|expected| admission.charge() != expected) {
                return Err(PlanError::Fault(AuthorityFault::ResourceProjection));
            }
            match scratch.apply_charged_admission(admission) {
                Ok(()) => {}
                Err(PlanError::Backpressure(
                    Backpressure::TotalResources
                    | Backpressure::ComputeResources
                    | Backpressure::ProposalCollision,
                )) => {
                    excluded.insert(key);
                }
                Err(PlanError::Duplicate | PlanError::PayloadVariant) => {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
                Err(error) => return Err(error),
            }
        }

        let scratch_owner_progress = scratch.clocks().owner_progress();
        let fresh = scratch.into_fresh_generation();
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        // Scratch admission sequences are compiler-local: queued Resolve
        // owners and their primary projections retain no dependency/effect
        // cut from those intermediate Applies. The generation swap publishes
        // every external source at `sequence`, so the live clock advances
        // exactly once while versions and arrivals retain their monotonic
        // values from the compiled prefix.
        clocks.adopt_owner_progress(scratch_owner_progress)?;
        let sources = self.source_versions.plan_generation_replacement(sequence);
        let effect = self.effects.lock().plan_generation_reset(sequence)?;
        let compute_slot_released = self.resources.read(&self.entries).preaccepted().active_work
            > fresh.preaccepted_active_work();
        Ok(PreparedApply::plain(
            self,
            PlainAuthorityDelta::ClearPool(Box::new(ClearPoolDelta {
                generation,
                chain_view: new_view,
                fresh,
                sources,
                effect,
                compute_slot_released,
            })),
        ))
    }
}

fn canonical_generation_recoveries(
    recoveries: Vec<ChainGenerationRecovery>,
) -> Result<Vec<ChainGenerationRecovery>, PlanError> {
    let mut by_hash = HashMap::new();
    by_hash.reserve(recoveries.len());
    for recovery in recoveries {
        let (key, dependencies) = match &recovery {
            ChainGenerationRecovery::Trusted(transaction) => (
                RawTxHash(transaction.hash()),
                declared_dependencies(transaction)?,
            ),
            ChainGenerationRecovery::RequeueExisting(entry) => (
                entry.record.identity.raw.clone(),
                entry.basis.dependencies().clone(),
            ),
        };
        let replaced = by_hash.insert(
            key,
            RecoveryCandidate {
                recovery,
                dependencies,
            },
        );
        if replaced.is_some() {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        }
    }
    topological_recoveries(by_hash)
}

fn chain_resource_error(error: ResourceError) -> PlanError {
    match error {
        ResourceError::PreAcceptedLimit => {
            PlanError::Backpressure(Backpressure::GenerationReplacement)
        }
        ResourceError::Arithmetic
        | ResourceError::ExistingChargeMismatch
        | ResourceError::AttributionMismatch
        | ResourceError::DuplicateChange
        | ResourceError::RemoteLimit
        | ResourceError::PeerLimit(_)
        | ResourceError::ReplacementHistoryLimit
        | ResourceError::AcceptedLimit
        | ResourceError::ComputeEnvelope
        | ResourceError::CapacityBankFault => PlanError::Fault(AuthorityFault::ResourceProjection),
    }
}

fn charge_chain_recovery(
    resources: &ResourceLedger,
    admission: ValidatedAdmission,
) -> Result<Option<ChargedAdmission>, PlanError> {
    match resources.charge_admission(admission) {
        Ok(admission) => Ok(Some(admission)),
        Err(ResourceError::Arithmetic | ResourceError::ComputeEnvelope) => Ok(None),
        Err(
            ResourceError::PreAcceptedLimit
            | ResourceError::RemoteLimit
            | ResourceError::PeerLimit(_)
            | ResourceError::ReplacementHistoryLimit
            | ResourceError::AcceptedLimit
            | ResourceError::ExistingChargeMismatch
            | ResourceError::AttributionMismatch
            | ResourceError::DuplicateChange
            | ResourceError::CapacityBankFault,
        ) => Err(PlanError::Fault(AuthorityFault::ResourceProjection)),
    }
}

fn recovery_parent_is_excluded(
    dependencies: &KnownDependencies,
    excluded: &HashSet<RawTxHash>,
) -> bool {
    dependencies.keys().iter().any(|dependency| {
        matches!(
            dependency.origin(),
            DependencyOrigin::Transaction(parent) if excluded.contains(&parent)
        )
    })
}

fn chain_index_error(error: IndexError) -> PlanError {
    match error {
        IndexError::Stale => PlanError::Stale(StalePlan::Version),
        IndexError::ProposalCollision => {
            PlanError::Backpressure(Backpressure::GenerationReplacement)
        }
        IndexError::Allocation => PlanError::Fault(AuthorityFault::IndexProjection),
        IndexError::Arithmetic => PlanError::Fault(AuthorityFault::CounterExhausted),
        IndexError::Projection => PlanError::Fault(AuthorityFault::IndexProjection),
    }
}

fn expected_preaccepted(entry: &PreAcceptedEntry) -> ExpectedPreAcceptedOwner {
    ExpectedPreAcceptedOwner {
        version: entry.record.version,
        source: entry.source,
    }
}

fn chain_recovery_owner(owner: Option<&OwnedTx>) -> ChainRecoveryOwner {
    match owner {
        Some(OwnedTx::PreAccepted(entry)) => {
            ChainRecoveryOwner::PreAccepted(expected_preaccepted(entry))
        }
        Some(OwnedTx::Accepted(entry)) => ChainRecoveryOwner::Accepted(entry.record.version),
        Some(OwnedTx::ReplacementHistory(entry)) => {
            ChainRecoveryOwner::ReplacementHistory(entry.record().version)
        }
        None => ChainRecoveryOwner::Vacant,
    }
}

fn chain_committed_owner(owner: &OwnedTx) -> ChainCommittedOwner {
    match owner {
        OwnedTx::PreAccepted(entry) => {
            ChainCommittedOwner::PreAccepted(expected_preaccepted(entry))
        }
        OwnedTx::Accepted(entry) => ChainCommittedOwner::Accepted(entry.record.version),
        OwnedTx::ReplacementHistory(entry) => {
            ChainCommittedOwner::ReplacementHistory(entry.record().version)
        }
    }
}

fn chain_conflict_owner(owner: &OwnedTx) -> Result<ChainConflictOwner, PlanError> {
    match owner {
        OwnedTx::PreAccepted(entry) => {
            Ok(ChainConflictOwner::PreAccepted(expected_preaccepted(entry)))
        }
        OwnedTx::Accepted(entry) => Ok(ChainConflictOwner::Accepted(entry.record.version)),
        OwnedTx::ReplacementHistory(_) => {
            Err(PlanError::Fault(AuthorityFault::MembershipProjection))
        }
    }
}

fn expected_preaccepted_matches(
    expected: ExpectedPreAcceptedOwner,
    owner: Option<&OwnedTx>,
) -> bool {
    matches!(
        owner,
        Some(OwnedTx::PreAccepted(entry))
            if entry.record.version == expected.version && entry.source == expected.source
    )
}

fn recovery_owner_matches(expected: ChainRecoveryOwner, owner: Option<&OwnedTx>) -> bool {
    match (expected, owner) {
        (ChainRecoveryOwner::Vacant, None) => true,
        (ChainRecoveryOwner::PreAccepted(expected), owner) => {
            expected_preaccepted_matches(expected, owner)
        }
        (ChainRecoveryOwner::Accepted(expected), Some(OwnedTx::Accepted(entry))) => {
            entry.record.version == expected
        }
        (
            ChainRecoveryOwner::ReplacementHistory(expected),
            Some(OwnedTx::ReplacementHistory(entry)),
        ) => entry.record().version == expected,
        (ChainRecoveryOwner::Vacant, Some(_))
        | (ChainRecoveryOwner::Accepted(_), Some(_) | None)
        | (ChainRecoveryOwner::ReplacementHistory(_), Some(_) | None) => false,
    }
}

fn committed_owner_matches(expected: ChainCommittedOwner, owner: Option<&OwnedTx>) -> bool {
    match (expected, owner) {
        (ChainCommittedOwner::PreAccepted(expected), owner) => {
            expected_preaccepted_matches(expected, owner)
        }
        (ChainCommittedOwner::Accepted(expected), Some(OwnedTx::Accepted(entry))) => {
            entry.record.version == expected
        }
        (
            ChainCommittedOwner::ReplacementHistory(expected),
            Some(OwnedTx::ReplacementHistory(entry)),
        ) => entry.record().version == expected,
        (ChainCommittedOwner::Accepted(_), Some(_) | None)
        | (ChainCommittedOwner::ReplacementHistory(_), Some(_) | None) => false,
    }
}

fn conflict_owner_matches(expected: ChainConflictOwner, owner: Option<&OwnedTx>) -> bool {
    match (expected, owner) {
        (ChainConflictOwner::PreAccepted(expected), owner) => {
            expected_preaccepted_matches(expected, owner)
        }
        (ChainConflictOwner::Accepted(expected), Some(OwnedTx::Accepted(entry))) => {
            entry.record.version == expected
        }
        (ChainConflictOwner::Accepted(_), Some(_) | None) => false,
    }
}

fn validate_chain_receipt_owners(
    entries: &ShardedOwnerMap,
    receipt: &ChainTransitionReceipt,
) -> Result<(), PlanError> {
    for removal in &receipt.removals {
        let owner = entries.get(removal.hash());
        let owner = owner.as_deref();
        let matches = match removal {
            ChainRemoval::Committed { expected, .. } => committed_owner_matches(*expected, owner),
            ChainRemoval::Recovery { expected, .. } => {
                matches!(owner, Some(OwnedTx::Accepted(entry)) if entry.record.version == *expected)
            }
            ChainRemoval::ChainConflict { expected, .. } => {
                conflict_owner_matches(*expected, owner)
            }
            ChainRemoval::ProposalWindowExpired { expected, .. } => {
                expected_preaccepted_matches(*expected, owner)
            }
        };
        if !matches {
            return Err(PlanError::Stale(StalePlan::Version));
        }
    }
    for recovery in &receipt.recoveries {
        let matches = match recovery {
            ChainRecoveryReceipt::Trusted {
                admission,
                expected,
            } => {
                let owner = entries.get(&admission.identity.raw);
                recovery_owner_matches(*expected, owner.as_deref())
            }
            ChainRecoveryReceipt::RequeueExisting { hash, expected } => {
                let owner = entries.get(hash);
                expected_preaccepted_matches(*expected, owner.as_deref())
            }
        };
        if !matches {
            return Err(PlanError::Stale(StalePlan::Version));
        }
    }
    for status in &receipt.statuses {
        if !matches!(
            entries.get(&status.hash).as_deref(),
            Some(OwnedTx::Accepted(entry)) if entry.record.version == status.expected
        ) {
            return Err(PlanError::Stale(StalePlan::Version));
        }
    }
    for demotion in &receipt.proposal_demotions {
        let owner = entries.get(&demotion.hash);
        if !expected_preaccepted_matches(demotion.expected, owner.as_deref()) {
            return Err(PlanError::Stale(StalePlan::Version));
        }
    }
    Ok(())
}

fn append_keys(target: &mut Vec<DependencyKey>, keys: impl IntoIterator<Item = DependencyKey>) {
    let keys = keys.into_iter();
    target.reserve(keys.size_hint().0);
    target.extend(keys);
}

fn append_transaction_output_keys(target: &mut Vec<DependencyKey>, transaction: &TransactionView) {
    append_keys(
        target,
        transaction
            .output_pts()
            .into_iter()
            .map(DependencyKey::Cell),
    );
}

fn queued_recovery_owner(
    admission: ChargedAdmission,
    version: EntryVersion,
    arrival: Arrival,
) -> OwnedTx {
    let (admission, charge) = admission.into_parts();
    let record = TxRecord {
        tx: admission.tx,
        identity: admission.identity,
        version,
        arrival,
    };
    OwnedTx::PreAccepted(PreAcceptedEntry {
        record,
        source: admission.source,
        basis: AdmissionBasis::new(
            admission.dependencies,
            admission.payload_bytes,
            admission.encoded_edges,
            charge,
        ),
        phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
        charge,
    })
}

fn requeued_existing_owner(
    mut entry: PreAcceptedEntry,
    version: EntryVersion,
    arrival: Arrival,
) -> Result<OwnedTx, PlanError> {
    entry.record.version = version;
    entry.record.arrival = arrival;
    entry.phase = PreAcceptedPhase::Queued(QueuedWork::Resolve);
    entry.charge = entry.original_charge();
    Ok(OwnedTx::PreAccepted(entry))
}

fn declared_dependencies(transaction: &TransactionView) -> Result<KnownDependencies, PlanError> {
    KnownDependencies::from_transaction(transaction).map_err(|error| match error {
        DependencySetError::Empty
        | DependencySetError::TooMany
        | DependencySetError::Arithmetic => PlanError::Fault(AuthorityFault::DependencyProjection),
    })
}

fn topological_recoveries<T>(
    mut transactions: HashMap<RawTxHash, RecoveryCandidate<T>>,
) -> Result<Vec<T>, PlanError> {
    let count = transactions.len();
    let mut indegree = HashMap::<RawTxHash, usize>::new();
    let mut outdegree = HashMap::<RawTxHash, usize>::new();
    indegree.reserve(count);
    outdegree.reserve(count);
    indegree.extend(transactions.keys().cloned().map(|hash| (hash, 0)));
    for (child, candidate) in &transactions {
        for dependency in candidate.dependencies.keys() {
            let DependencyOrigin::Transaction(parent) = dependency.origin() else {
                continue;
            };
            if !transactions.contains_key(&parent) {
                continue;
            }
            let degree = indegree
                .get_mut(child)
                .ok_or(PlanError::Fault(AuthorityFault::DependencyProjection))?;
            *degree = degree
                .checked_add(1)
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
            let children = outdegree.entry(parent).or_default();
            *children = children
                .checked_add(1)
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        }
    }
    let mut children = HashMap::<RawTxHash, Vec<RawTxHash>>::new();
    children.reserve(outdegree.len());
    for (parent, capacity) in outdegree {
        let row = Vec::with_capacity(capacity);
        children.insert(parent, row);
    }
    for (child, candidate) in &transactions {
        for dependency in candidate.dependencies.keys() {
            let DependencyOrigin::Transaction(parent) = dependency.origin() else {
                continue;
            };
            if let Some(row) = children.get_mut(&parent) {
                row.push(child.clone());
            }
        }
    }
    let mut ready = BinaryHeap::new();
    ready.reserve(count);
    ready.extend(
        indegree
            .iter()
            .filter(|(_, degree)| **degree == 0)
            .map(|(hash, _)| Reverse(hash.clone())),
    );
    let mut ordered = Vec::with_capacity(count);
    while let Some(Reverse(hash)) = ready.pop() {
        let candidate = transactions
            .remove(&hash)
            .ok_or(PlanError::Fault(AuthorityFault::DependencyProjection))?;
        ordered.push(candidate.recovery);
        if let Some(row) = children.get(&hash) {
            for child in row {
                let degree = indegree
                    .get_mut(child)
                    .and_then(|degree| degree.checked_sub(1).map(|next| (degree, next)))
                    .ok_or(PlanError::Fault(AuthorityFault::DependencyProjection))?;
                *degree.0 = degree.1;
                if degree.1 == 0 {
                    ready.push(Reverse(child.clone()));
                }
            }
        }
    }
    if let Some((hash, _)) = transactions.into_iter().next() {
        return Err(PlanError::Membership(MembershipReject::CausalCycle(hash)));
    }
    Ok(ordered)
}
