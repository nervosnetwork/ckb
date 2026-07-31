//! Coherent query and persistence projections over one authority read cut.
//!
//! The borrowed view never exposes the primary owner representation. Large
//! persistence work first captures immutable payload handles and relation
//! facts, then performs deterministic topological ordering after the authority
//! guard has been released.

use super::template::{AuthorityTemplateReadReceipt, TemplateReadError};
use super::{
    indexes::AuthorityIndexes,
    plan::MembershipProjection,
    source::PoolTemplateVersions,
    state::{
        AcceptedAtMillis, AcceptedStatus, ApplySequence, Arrival, ChainViewId, DependencyKey,
        EntryVersion, KnownDependencies, OwnedTx, PoolGeneration, PreAcceptedPhase,
        PreAcceptedSource, ProposalId, QueuedWork, RawTxHash, TxIdentity,
    },
};
use ckb_types::core::{Capacity, TransactionView};
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
    sync::Arc,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AuthorityReadCut {
    generation: PoolGeneration,
    chain_view: ChainViewId,
    next_apply_sequence: ApplySequence,
}

impl AuthorityReadCut {
    pub(super) fn generation(&self) -> PoolGeneration {
        self.generation
    }

    pub(super) fn chain_view(&self) -> &ChainViewId {
        &self.chain_view
    }

    pub(super) fn next_apply_sequence(&self) -> ApplySequence {
        self.next_apply_sequence
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PreAcceptedReadPhase {
    ResolveQueued,
    VerifyQueued,
    Computing,
    WaitingMissing,
    Ready,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AuthorityReadState {
    PreAccepted(PreAcceptedReadPhase),
    Accepted(AcceptedStatus),
    ReplacementHistory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AuthorityRpcStatus {
    Pending,
    Proposed,
}

#[derive(Clone, Copy)]
pub(super) struct AuthorityReadEntry<'authority> {
    owner: &'authority OwnedTx,
}

impl<'authority> AuthorityReadEntry<'authority> {
    fn new(owner: &'authority OwnedTx) -> Self {
        Self { owner }
    }

    pub(super) fn transaction(&self) -> &'authority Arc<TransactionView> {
        &self.owner.record().tx
    }

    pub(super) fn identity(&self) -> &'authority TxIdentity {
        &self.owner.record().identity
    }

    pub(super) fn version(&self) -> EntryVersion {
        self.owner.record().version
    }

    pub(super) fn arrival(&self) -> Arrival {
        self.owner.record().arrival
    }

    pub(super) fn state(&self) -> AuthorityReadState {
        match self.owner {
            OwnedTx::Accepted(entry) => AuthorityReadState::Accepted(entry.status()),
            OwnedTx::PreAccepted(entry) => {
                let phase = match &entry.phase {
                    PreAcceptedPhase::Queued(QueuedWork::Resolve) => {
                        PreAcceptedReadPhase::ResolveQueued
                    }
                    PreAcceptedPhase::Queued(QueuedWork::Verify(_)) => {
                        PreAcceptedReadPhase::VerifyQueued
                    }
                    PreAcceptedPhase::Computing(_) => PreAcceptedReadPhase::Computing,
                    PreAcceptedPhase::Waiting(_) => PreAcceptedReadPhase::WaitingMissing,
                    PreAcceptedPhase::Ready(_) => PreAcceptedReadPhase::Ready,
                };
                AuthorityReadState::PreAccepted(phase)
            }
            OwnedTx::ReplacementHistory(_) => AuthorityReadState::ReplacementHistory,
        }
    }

    /// Project only owners that are part of the public live-pool surface.
    ///
    /// Replacement history is private recovery state. Returning `None` is a
    /// load-bearing boundary: the production query adapter must continue to
    /// the existing recent-reject lookup instead of inventing a live-pool RPC
    /// status for a retained victim.
    pub(super) fn rpc_status(&self) -> Option<AuthorityRpcStatus> {
        match self.state() {
            AuthorityReadState::Accepted(AcceptedStatus::Proposed) => {
                Some(AuthorityRpcStatus::Proposed)
            }
            AuthorityReadState::ReplacementHistory => None,
            AuthorityReadState::PreAccepted(
                PreAcceptedReadPhase::ResolveQueued
                | PreAcceptedReadPhase::VerifyQueued
                | PreAcceptedReadPhase::Computing
                | PreAcceptedReadPhase::WaitingMissing
                | PreAcceptedReadPhase::Ready,
            )
            | AuthorityReadState::Accepted(AcceptedStatus::Pending | AcceptedStatus::Gap) => {
                Some(AuthorityRpcStatus::Pending)
            }
        }
    }

    pub(super) fn fee(&self) -> Option<Capacity> {
        match self.owner {
            OwnedTx::Accepted(entry) => Some(entry.proof.metrics().fee),
            OwnedTx::PreAccepted(entry) => match &entry.phase {
                PreAcceptedPhase::Queued(QueuedWork::Verify(resolved)) => {
                    Some(resolved.payload().fee())
                }
                PreAcceptedPhase::Ready(verified) => Some(verified.metrics().fee),
                PreAcceptedPhase::Queued(QueuedWork::Resolve)
                | PreAcceptedPhase::Computing(_)
                | PreAcceptedPhase::Waiting(_) => None,
            },
            OwnedTx::ReplacementHistory(_) => None,
        }
    }

    pub(super) fn cycles(&self) -> Option<u64> {
        match self.owner {
            OwnedTx::Accepted(entry) => Some(entry.proof.metrics().cost.cycles),
            OwnedTx::PreAccepted(entry) => match &entry.phase {
                PreAcceptedPhase::Ready(verified) => Some(verified.metrics().cost.cycles),
                PreAcceptedPhase::Queued(_)
                | PreAcceptedPhase::Computing(_)
                | PreAcceptedPhase::Waiting(_) => None,
            },
            OwnedTx::ReplacementHistory(_) => None,
        }
    }

    pub(super) fn accepted_at(&self) -> Option<AcceptedAtMillis> {
        match self.owner {
            OwnedTx::Accepted(entry) => Some(entry.accepted_at),
            OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_) => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct AuthorityPoolIds {
    pub(super) pending: Vec<RawTxHash>,
    pub(super) proposed: Vec<RawTxHash>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct AuthorityReadSummary {
    pub(super) owners: usize,
    pub(super) preaccepted: usize,
    pub(super) queued: usize,
    pub(super) computing: usize,
    pub(super) waiting_missing: usize,
    pub(super) replacement_history: usize,
    pub(super) ready: usize,
    pub(super) accepted_pending: usize,
    pub(super) accepted_gap: usize,
    pub(super) accepted_proposed: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AuthorityReadError {
    Allocation,
    Arithmetic,
    Projection,
    AcceptedCycle,
    RecoveryCycle,
}

pub(super) struct AuthorityReadView<'authority> {
    cut: AuthorityReadCut,
    entries: &'authority HashMap<RawTxHash, OwnedTx>,
    indexes: &'authority AuthorityIndexes,
    membership: &'authority MembershipProjection,
    template_sources: PoolTemplateVersions,
}

impl<'authority> AuthorityReadView<'authority> {
    pub(super) fn new(
        generation: PoolGeneration,
        chain_view: ChainViewId,
        next_apply_sequence: ApplySequence,
        entries: &'authority HashMap<RawTxHash, OwnedTx>,
        indexes: &'authority AuthorityIndexes,
        membership: &'authority MembershipProjection,
        template_sources: PoolTemplateVersions,
    ) -> Self {
        Self {
            cut: AuthorityReadCut {
                generation,
                chain_view,
                next_apply_sequence,
            },
            entries,
            indexes,
            membership,
            template_sources,
        }
    }

    pub(super) fn cut(&self) -> &AuthorityReadCut {
        &self.cut
    }

    pub(super) fn entry_by_raw(&self, hash: &RawTxHash) -> Option<AuthorityReadEntry<'authority>> {
        self.entries.get(hash).map(AuthorityReadEntry::new)
    }

    pub(super) fn entry_by_proposal(
        &self,
        proposal: &ProposalId,
    ) -> Result<Option<AuthorityReadEntry<'authority>>, AuthorityReadError> {
        let Some(hash) = self.indexes.proposal_owner(proposal) else {
            return Ok(None);
        };
        let owner = self
            .entries
            .get(hash)
            .ok_or(AuthorityReadError::Projection)?;
        if &owner.record().identity.proposal != proposal {
            return Err(AuthorityReadError::Projection);
        }
        Ok(Some(AuthorityReadEntry::new(owner)))
    }

    pub(super) fn entries(&self) -> impl Iterator<Item = AuthorityReadEntry<'authority>> + '_ {
        self.entries.values().map(AuthorityReadEntry::new)
    }

    pub(super) fn compact_transactions(
        &self,
        proposals: &[ProposalId],
    ) -> Result<Vec<(ProposalId, Arc<TransactionView>)>, AuthorityReadError> {
        let mut transactions = Vec::new();
        transactions
            .try_reserve(proposals.len())
            .map_err(|_| AuthorityReadError::Allocation)?;
        for proposal in proposals {
            if let Some(entry) = self.entry_by_proposal(proposal)? {
                transactions.push((proposal.clone(), Arc::clone(entry.transaction())));
            }
        }
        Ok(transactions)
    }

    pub(super) fn pool_ids(&self) -> Result<AuthorityPoolIds, AuthorityReadError> {
        let counts = self.membership.counts();
        let pending_capacity = counts
            .pending
            .checked_add(counts.gap)
            .ok_or(AuthorityReadError::Arithmetic)?;
        let mut pending = Vec::new();
        let mut proposed = Vec::new();
        pending
            .try_reserve(pending_capacity)
            .map_err(|_| AuthorityReadError::Allocation)?;
        proposed
            .try_reserve(counts.proposed)
            .map_err(|_| AuthorityReadError::Allocation)?;
        for (hash, owner) in self.entries {
            let OwnedTx::Accepted(entry) = owner else {
                continue;
            };
            match entry.status() {
                AcceptedStatus::Pending | AcceptedStatus::Gap => pending.push(hash.clone()),
                AcceptedStatus::Proposed => proposed.push(hash.clone()),
            }
        }
        if pending.len() != pending_capacity || proposed.len() != counts.proposed {
            return Err(AuthorityReadError::Projection);
        }
        pending.sort_unstable();
        proposed.sort_unstable();
        Ok(AuthorityPoolIds { pending, proposed })
    }

    pub(super) fn replacement_history_hashes(&self) -> Result<Vec<RawTxHash>, AuthorityReadError> {
        let mut history = Vec::new();
        history
            .try_reserve(self.entries.len())
            .map_err(|_| AuthorityReadError::Allocation)?;
        for (hash, owner) in self.entries {
            if matches!(owner, OwnedTx::ReplacementHistory(_)) {
                history.push(hash.clone());
            }
        }
        history.sort_unstable();
        Ok(history)
    }

    pub(super) fn summary(&self) -> Result<AuthorityReadSummary, AuthorityReadError> {
        let mut summary = AuthorityReadSummary {
            owners: self.entries.len(),
            ..AuthorityReadSummary::default()
        };
        for owner in self.entries.values() {
            match owner {
                OwnedTx::Accepted(entry) => match entry.status() {
                    AcceptedStatus::Pending => increment(&mut summary.accepted_pending)?,
                    AcceptedStatus::Gap => increment(&mut summary.accepted_gap)?,
                    AcceptedStatus::Proposed => increment(&mut summary.accepted_proposed)?,
                },
                OwnedTx::PreAccepted(entry) => {
                    increment(&mut summary.preaccepted)?;
                    match &entry.phase {
                        PreAcceptedPhase::Queued(_) => increment(&mut summary.queued)?,
                        PreAcceptedPhase::Computing(_) => increment(&mut summary.computing)?,
                        PreAcceptedPhase::Waiting(_) => {
                            increment(&mut summary.waiting_missing)?;
                        }
                        PreAcceptedPhase::Ready(_) => increment(&mut summary.ready)?,
                    }
                }
                OwnedTx::ReplacementHistory(_) => {
                    increment(&mut summary.replacement_history)?;
                }
            }
        }
        let counts = self.membership.counts();
        if summary.accepted_pending != counts.pending
            || summary.accepted_gap != counts.gap
            || summary.accepted_proposed != counts.proposed
        {
            return Err(AuthorityReadError::Projection);
        }
        Ok(summary)
    }

    pub(super) fn capture_persistence(&self) -> Result<PersistenceReadReceipt, AuthorityReadError> {
        let mut selected = Vec::new();
        selected
            .try_reserve(self.entries.len())
            .map_err(|_| AuthorityReadError::Allocation)?;
        for (hash, owner) in self.entries {
            if &owner.record().identity.raw != hash {
                return Err(AuthorityReadError::Projection);
            }
            let relation = match owner {
                OwnedTx::Accepted(_) => {
                    let parents = self
                        .membership
                        .parents(hash)
                        .ok_or(AuthorityReadError::Projection)?;
                    let mut copied = Vec::new();
                    copied
                        .try_reserve(parents.len())
                        .map_err(|_| AuthorityReadError::Allocation)?;
                    copied.extend(parents.iter().cloned());
                    PersistenceParents::Accepted(copied)
                }
                OwnedTx::PreAccepted(entry)
                    if matches!(entry.source, PreAcceptedSource::Recovery(_)) =>
                {
                    // Restart completeness follows the durable Recovery
                    // source, not its transient compute/wait phase.
                    PersistenceParents::Recovery(entry.dependencies().clone())
                }
                OwnedTx::PreAccepted(_) => continue,
                OwnedTx::ReplacementHistory(_) => continue,
            };
            selected.push(PersistenceRow {
                hash: hash.clone(),
                arrival: owner.record().arrival,
                transaction: Arc::clone(&owner.record().tx),
                relation,
            });
        }
        Ok(PersistenceReadReceipt {
            cut: self.cut.clone(),
            selected,
        })
    }

    /// Capture accepted payloads, relations and their exact template source
    /// versions from this same borrowed authority cut. Sorting and template
    /// construction happen only after the caller releases the read guard.
    pub(super) fn capture_template(
        &self,
    ) -> Result<AuthorityTemplateReadReceipt, TemplateReadError> {
        AuthorityTemplateReadReceipt::capture(
            self.cut.clone(),
            self.template_sources,
            self.entries,
            self.membership,
        )
    }
}

fn increment(value: &mut usize) -> Result<(), AuthorityReadError> {
    *value = value.checked_add(1).ok_or(AuthorityReadError::Arithmetic)?;
    Ok(())
}

#[derive(Debug)]
enum PersistenceParents {
    Accepted(Vec<RawTxHash>),
    Recovery(KnownDependencies),
}

#[derive(Debug)]
struct PersistenceRow {
    hash: RawTxHash,
    arrival: Arrival,
    transaction: Arc<TransactionView>,
    relation: PersistenceParents,
}

#[derive(Debug)]
pub(super) struct PersistenceReadReceipt {
    cut: AuthorityReadCut,
    selected: Vec<PersistenceRow>,
}

impl PersistenceReadReceipt {
    pub(super) fn cut(&self) -> &AuthorityReadCut {
        &self.cut
    }

    pub(super) fn selected_len(&self) -> usize {
        self.selected.len()
    }

    pub(super) fn into_parent_first(self) -> Result<ParentFirstPersistence, AuthorityReadError> {
        let accepted_count = self
            .selected
            .iter()
            .filter(|row| matches!(row.relation, PersistenceParents::Accepted(_)))
            .count();
        let recovery_count = self
            .selected
            .len()
            .checked_sub(accepted_count)
            .ok_or(AuthorityReadError::Arithmetic)?;
        let mut accepted = Vec::new();
        let mut recovery = Vec::new();
        accepted
            .try_reserve(accepted_count)
            .map_err(|_| AuthorityReadError::Allocation)?;
        recovery
            .try_reserve(recovery_count)
            .map_err(|_| AuthorityReadError::Allocation)?;
        for row in self.selected {
            match row.relation {
                PersistenceParents::Accepted(_) => accepted.push(row),
                PersistenceParents::Recovery(_) => recovery.push(row),
            }
        }

        let accepted_order = parent_first_indices(&accepted, PersistencePartition::Accepted)?;
        let recovery_order = parent_first_indices(&recovery, PersistencePartition::Recovery)?;
        let accepted = ordered_transactions(&accepted, &accepted_order)?;
        let recovery = ordered_transactions(&recovery, &recovery_order)?;
        Ok(ParentFirstPersistence {
            cut: self.cut,
            accepted,
            recovery,
        })
    }
}

#[derive(Debug)]
pub(super) struct ParentFirstPersistence {
    cut: AuthorityReadCut,
    accepted: Vec<Arc<TransactionView>>,
    recovery: Vec<Arc<TransactionView>>,
}

impl ParentFirstPersistence {
    pub(super) fn cut(&self) -> &AuthorityReadCut {
        &self.cut
    }

    pub(super) fn accepted(&self) -> &[Arc<TransactionView>] {
        &self.accepted
    }

    pub(super) fn recovery(&self) -> &[Arc<TransactionView>] {
        &self.recovery
    }

    /// Persistence exports transaction bytes only. Proof, charge, source,
    /// status and dependency-cut facts cannot cross the restart boundary.
    pub(super) fn into_transactions(
        self,
    ) -> (Vec<Arc<TransactionView>>, Vec<Arc<TransactionView>>) {
        (self.accepted, self.recovery)
    }
}

#[derive(Clone, Copy)]
enum PersistencePartition {
    Accepted,
    Recovery,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PersistenceReadyKey {
    arrival: Arrival,
    hash: RawTxHash,
    index: usize,
}

fn parent_first_indices(
    rows: &[PersistenceRow],
    partition: PersistencePartition,
) -> Result<Vec<usize>, AuthorityReadError> {
    let mut by_hash = HashMap::new();
    by_hash
        .try_reserve(rows.len())
        .map_err(|_| AuthorityReadError::Allocation)?;
    for (index, row) in rows.iter().enumerate() {
        if by_hash.insert(row.hash.clone(), index).is_some() {
            return Err(AuthorityReadError::Projection);
        }
    }

    let mut indegree = Vec::new();
    indegree
        .try_reserve(rows.len())
        .map_err(|_| AuthorityReadError::Allocation)?;
    indegree.resize(rows.len(), 0usize);
    let mut children = Vec::new();
    children
        .try_reserve(rows.len())
        .map_err(|_| AuthorityReadError::Allocation)?;
    children.resize_with(rows.len(), Vec::new);

    for (child, row) in rows.iter().enumerate() {
        let mut parents = relation_parent_indices(row, partition, &by_hash)?;
        parents.sort_unstable();
        parents.dedup();
        for parent in parents {
            let degree = indegree
                .get_mut(child)
                .ok_or(AuthorityReadError::Projection)?;
            *degree = degree
                .checked_add(1)
                .ok_or(AuthorityReadError::Arithmetic)?;
            let outgoing = children
                .get_mut(parent)
                .ok_or(AuthorityReadError::Projection)?;
            outgoing
                .try_reserve(1)
                .map_err(|_| AuthorityReadError::Allocation)?;
            outgoing.push(child);
        }
    }

    let mut ready = BinaryHeap::new();
    ready
        .try_reserve(rows.len())
        .map_err(|_| AuthorityReadError::Allocation)?;
    for (index, degree) in indegree.iter().copied().enumerate() {
        if degree == 0 {
            let row = rows.get(index).ok_or(AuthorityReadError::Projection)?;
            ready.push(Reverse(PersistenceReadyKey {
                arrival: row.arrival,
                hash: row.hash.clone(),
                index,
            }));
        }
    }

    let mut order = Vec::new();
    order
        .try_reserve(rows.len())
        .map_err(|_| AuthorityReadError::Allocation)?;
    while let Some(Reverse(next)) = ready.pop() {
        order.push(next.index);
        let outgoing = children
            .get(next.index)
            .ok_or(AuthorityReadError::Projection)?;
        for child in outgoing {
            let degree = indegree
                .get_mut(*child)
                .ok_or(AuthorityReadError::Projection)?;
            *degree = degree
                .checked_sub(1)
                .ok_or(AuthorityReadError::Projection)?;
            if *degree == 0 {
                let row = rows.get(*child).ok_or(AuthorityReadError::Projection)?;
                ready.push(Reverse(PersistenceReadyKey {
                    arrival: row.arrival,
                    hash: row.hash.clone(),
                    index: *child,
                }));
            }
        }
    }
    if order.len() != rows.len() {
        return Err(match partition {
            PersistencePartition::Accepted => AuthorityReadError::AcceptedCycle,
            PersistencePartition::Recovery => AuthorityReadError::RecoveryCycle,
        });
    }
    Ok(order)
}

fn relation_parent_indices(
    row: &PersistenceRow,
    partition: PersistencePartition,
    by_hash: &HashMap<RawTxHash, usize>,
) -> Result<Vec<usize>, AuthorityReadError> {
    match (&row.relation, partition) {
        (PersistenceParents::Accepted(parents), PersistencePartition::Accepted) => {
            let mut indices = Vec::new();
            indices
                .try_reserve(parents.len())
                .map_err(|_| AuthorityReadError::Allocation)?;
            for parent in parents {
                indices.push(*by_hash.get(parent).ok_or(AuthorityReadError::Projection)?);
            }
            Ok(indices)
        }
        (PersistenceParents::Recovery(dependencies), PersistencePartition::Recovery) => {
            let mut indices = Vec::new();
            indices
                .try_reserve(dependencies.len())
                .map_err(|_| AuthorityReadError::Allocation)?;
            for dependency in dependencies.keys() {
                if let DependencyKey::Cell(out_point) = dependency
                    && let Some(parent) = by_hash.get(&RawTxHash(out_point.tx_hash()))
                {
                    indices.push(*parent);
                }
            }
            Ok(indices)
        }
        (PersistenceParents::Accepted(_), PersistencePartition::Recovery)
        | (PersistenceParents::Recovery(_), PersistencePartition::Accepted) => {
            Err(AuthorityReadError::Projection)
        }
    }
}

fn ordered_transactions(
    rows: &[PersistenceRow],
    order: &[usize],
) -> Result<Vec<Arc<TransactionView>>, AuthorityReadError> {
    let mut transactions = Vec::new();
    transactions
        .try_reserve(order.len())
        .map_err(|_| AuthorityReadError::Allocation)?;
    for index in order {
        let row = rows.get(*index).ok_or(AuthorityReadError::Projection)?;
        transactions.push(Arc::clone(&row.transaction));
    }
    Ok(transactions)
}
