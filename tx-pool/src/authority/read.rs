//! Coherent query and persistence projections over one authority read cut.
//!
//! The borrowed view never exposes the primary owner representation. Large
//! persistence work first captures immutable payload handles and relation
//! facts, then performs deterministic topological ordering after the authority
//! guard has been released.

use super::template::{AuthorityTemplateReadReceipt, TemplateReadError};
use super::{
    effect::ParentTransactionRequest,
    indexes::{AuthorityIndexes, DueRemote, IndexError},
    plan::{
        AcceptedOrderKey, AncestorAggregate, DescendantAggregate, MembershipConfig,
        MembershipProjection,
    },
    resources::AcceptedResources,
    shard::{ShardedOwnerMap, ShardedOwnerReadCut, ShardedOwnerReadGuard},
    source::AuthoritySourceVersions,
    state::{
        AcceptedAtMillis, AcceptedStatus, ApplySequence, Arrival, ChainViewId, DependencyKey,
        DependencySetError, KnownDependencies, ObservedDependencies, OwnedTx, PreAcceptedPhase,
        PreAcceptedSource, ProposalId, QueuedWork, RawTxHash, WorkPermit,
    },
    validation::proposal_status,
};
use ckb_network::PeerIndex;
use ckb_snapshot::Snapshot;
use ckb_types::core::{Capacity, TransactionView};
use ckb_types::packed::OutPoint;
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
    num::NonZeroUsize,
    sync::Arc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RelayParentRebuildCut(ApplySequence);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RelayParentRebuildCursor {
    cut: RelayParentRebuildCut,
    remote: DueRemote,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RelayParentRebuildError {
    StaleCut,
    Allocation,
    Projection,
}

struct MissingParentCapture {
    peer: PeerIndex,
    observed: ObservedDependencies,
}

pub(super) struct RelayParentRebuildScratch {
    remote: Vec<DueRemote>,
    missing: Vec<MissingParentCapture>,
}

impl RelayParentRebuildScratch {
    pub(super) fn try_new(limit: NonZeroUsize) -> Result<Self, RelayParentRebuildError> {
        let mut remote = Vec::new();
        let mut missing = Vec::new();
        remote
            .try_reserve(limit.get())
            .map_err(|_| RelayParentRebuildError::Allocation)?;
        missing
            .try_reserve(limit.get())
            .map_err(|_| RelayParentRebuildError::Allocation)?;
        Ok(Self { remote, missing })
    }
}

#[must_use = "captured relay evidence must be compiled after releasing the authority guard"]
pub(super) struct PreparedRelayParentRebuildPage {
    cut: RelayParentRebuildCut,
    remote: Vec<DueRemote>,
    missing: Vec<MissingParentCapture>,
    next: Option<RelayParentRebuildCursor>,
}

impl PreparedRelayParentRebuildPage {
    pub(super) fn finish(self) -> Result<RelayParentRebuildPage, RelayParentRebuildError> {
        let Self {
            cut,
            remote: _remote,
            missing,
            next,
        } = self;
        let mut requests = Vec::new();
        requests
            .try_reserve(missing.len())
            .map_err(|_| RelayParentRebuildError::Allocation)?;
        for capture in missing {
            let parents = capture.observed.parent_transactions()?;
            if let Some(request) = ParentTransactionRequest::new(capture.peer, parents) {
                requests.push(request);
            }
        }
        Ok(RelayParentRebuildPage {
            cut,
            requests,
            next,
        })
    }
}

#[must_use = "a relay rebuild page must be consumed or its continuation retained"]
pub(super) struct RelayParentRebuildPage {
    cut: RelayParentRebuildCut,
    requests: Vec<ParentTransactionRequest>,
    next: Option<RelayParentRebuildCursor>,
}

impl RelayParentRebuildPage {
    pub(super) fn into_parts(
        self,
    ) -> (
        RelayParentRebuildCut,
        Vec<ParentTransactionRequest>,
        Option<RelayParentRebuildCursor>,
    ) {
        (self.cut, self.requests, self.next)
    }
}

impl From<IndexError> for RelayParentRebuildError {
    fn from(error: IndexError) -> Self {
        match error {
            IndexError::Allocation => Self::Allocation,
            IndexError::ProposalCollision | IndexError::Projection | IndexError::Arithmetic => {
                Self::Projection
            }
        }
    }
}

impl From<DependencySetError> for RelayParentRebuildError {
    fn from(error: DependencySetError) -> Self {
        match error {
            DependencySetError::Allocation => Self::Allocation,
            DependencySetError::Empty
            | DependencySetError::TooMany
            | DependencySetError::Arithmetic => Self::Projection,
        }
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

pub(super) struct AuthorityReadEntry<'authority> {
    owner: ShardedOwnerReadGuard<'authority>,
}

impl<'authority> AuthorityReadEntry<'authority> {
    fn new(owner: ShardedOwnerReadGuard<'authority>) -> Self {
        Self { owner }
    }

    pub(super) fn transaction(&self) -> &Arc<TransactionView> {
        &self.owner.record().tx
    }

    pub(super) fn state(&self) -> AuthorityReadState {
        match &*self.owner {
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
    pub(super) fn rpc_status(&self, snapshot: &Snapshot) -> Option<AuthorityRpcStatus> {
        match &*self.owner {
            OwnedTx::Accepted(entry) => Some(rpc_status_for_accepted(entry.status())),
            OwnedTx::PreAccepted(entry) => Some(match &entry.phase {
                PreAcceptedPhase::Queued(QueuedWork::Verify(_))
                | PreAcceptedPhase::Computing(super::state::ActiveWork {
                    permit: WorkPermit::VerifyOnly(_),
                    ..
                })
                | PreAcceptedPhase::Ready(_) => rpc_status_for_accepted(proposal_status(
                    snapshot,
                    &entry.record.identity.proposal.0,
                )),
                // ResolveThenVerify intentionally keeps one checked-out work
                // capability and performs no intermediate authority
                // transition. It therefore remains conservatively Pending
                // while either stage may own that capability; adding shared
                // stage state only for RPC would weaken the one-owner and
                // continuation performance model.
                PreAcceptedPhase::Queued(QueuedWork::Resolve)
                | PreAcceptedPhase::Computing(_)
                | PreAcceptedPhase::Waiting(_) => AuthorityRpcStatus::Pending,
            }),
            OwnedTx::ReplacementHistory(_) => None,
        }
    }

    pub(super) fn fee(&self) -> Option<Capacity> {
        match &*self.owner {
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
        match &*self.owner {
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

    pub(super) fn transaction_status_cycles(&self) -> Option<u64> {
        match &*self.owner {
            OwnedTx::Accepted(entry) => Some(entry.proof.metrics().cost.cycles),
            OwnedTx::PreAccepted(entry) => match &entry.phase {
                PreAcceptedPhase::Ready(verified) => Some(verified.metrics().cost.cycles),
                PreAcceptedPhase::Waiting(_) => {
                    entry.source.payload_policy().declared_cycles().or(Some(0))
                }
                PreAcceptedPhase::Queued(_) | PreAcceptedPhase::Computing(_) => None,
            },
            OwnedTx::ReplacementHistory(_) => None,
        }
    }

    pub(super) fn accepted_at(&self) -> Option<AcceptedAtMillis> {
        match &*self.owner {
            OwnedTx::Accepted(entry) => Some(entry.accepted_at),
            OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_) => None,
        }
    }
}

fn rpc_status_for_accepted(status: AcceptedStatus) -> AuthorityRpcStatus {
    match status {
        AcceptedStatus::Proposed => AuthorityRpcStatus::Proposed,
        AcceptedStatus::Pending | AcceptedStatus::Gap => AuthorityRpcStatus::Pending,
    }
}

pub(super) struct AcceptedReadEntry<'authority> {
    entry: &'authority super::state::AcceptedEntry,
    ancestor: AncestorAggregate,
    descendant: DescendantAggregate,
    order: AcceptedOrderKey,
}

impl<'authority> AcceptedReadEntry<'authority> {
    pub(super) fn entry(&self) -> &super::state::AcceptedEntry {
        self.entry
    }

    pub(super) fn ancestor(&self) -> AncestorAggregate {
        self.ancestor
    }

    pub(super) fn descendant(&self) -> DescendantAggregate {
        self.descendant
    }

    pub(super) fn order(&self) -> &AcceptedOrderKey {
        &self.order
    }
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
    pub(super) verify_queued: usize,
    pub(super) accepted_pending: usize,
    pub(super) accepted_gap: usize,
    pub(super) accepted_proposed: usize,
    pub(super) accepted_resources: AcceptedResources,
    pub(super) latest_accepted_at: Option<AcceptedAtMillis>,
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
    chain_view: ChainViewId,
    entries: &'authority ShardedOwnerMap,
    indexes: &'authority AuthorityIndexes,
    membership: &'authority MembershipProjection,
    membership_config: MembershipConfig,
    relay_parent_source: ApplySequence,
    source_versions: &'authority AuthoritySourceVersions,
}

pub(super) struct AuthorityFullReadCut<'authority> {
    owners: ShardedOwnerReadCut<'authority>,
    accepted_resources: AcceptedResources,
}

impl<'authority> AuthorityReadView<'authority> {
    pub(super) fn new(
        chain_view: ChainViewId,
        entries: &'authority ShardedOwnerMap,
        indexes: &'authority AuthorityIndexes,
        membership: &'authority MembershipProjection,
        membership_config: MembershipConfig,
        relay_parent_source: ApplySequence,
        source_versions: &'authority AuthoritySourceVersions,
    ) -> Self {
        Self {
            chain_view,
            entries,
            indexes,
            membership,
            membership_config,
            relay_parent_source,
            source_versions,
        }
    }

    /// Capture one bounded scan page of the current Remote missing-parent
    /// level. The cursor is valid only for the exact relay-parent source cut
    /// that produced it. A relevant owner transition forces the derived
    /// relayer projection to restart; unrelated effect and owner transitions
    /// cannot starve a bounded rebuild.
    ///
    /// Scratch capacity is acquired before the caller takes the authority
    /// read guard. This method therefore performs no fallible allocation while
    /// holding the guard; parent-list allocation happens in `finish` after the
    /// guard is released.
    pub(super) fn capture_relay_parent_rebuild(
        &self,
        cursor: Option<RelayParentRebuildCursor>,
        limit: NonZeroUsize,
        mut scratch: RelayParentRebuildScratch,
    ) -> Result<PreparedRelayParentRebuildPage, RelayParentRebuildError> {
        let cut = RelayParentRebuildCut(self.relay_parent_source);
        let after = match cursor {
            Some(cursor) if cursor.cut != cut => return Err(RelayParentRebuildError::StaleCut),
            Some(cursor) => Some(cursor.remote),
            None => None,
        };
        let has_more =
            self.indexes
                .remote_page_into(after.as_ref(), limit.get(), &mut scratch.remote)?;

        for due in &scratch.remote {
            let owner = self
                .entries
                .get(&due.hash)
                .ok_or(RelayParentRebuildError::Projection)?;
            let OwnedTx::PreAccepted(entry) = &*owner else {
                return Err(RelayParentRebuildError::Projection);
            };
            if entry.source.active_remote_deadline() != Some(due.expires_at) {
                return Err(RelayParentRebuildError::Projection);
            }
            let PreAcceptedPhase::Waiting(observed) = &entry.phase else {
                continue;
            };
            let peer = entry
                .source
                .ingress_peer()
                .ok_or(RelayParentRebuildError::Projection)?;
            scratch.missing.push(MissingParentCapture {
                peer,
                observed: observed.clone(),
            });
        }

        let next = if has_more {
            let remote = scratch
                .remote
                .last()
                .cloned()
                .ok_or(RelayParentRebuildError::Projection)?;
            Some(RelayParentRebuildCursor { cut, remote })
        } else {
            None
        };
        Ok(PreparedRelayParentRebuildPage {
            cut,
            remote: scratch.remote,
            missing: scratch.missing,
            next,
        })
    }

    pub(super) fn relay_parent_rebuild_cut_is_current(&self, cut: &RelayParentRebuildCut) -> bool {
        cut.0 == self.relay_parent_source
    }

    pub(super) fn entry_by_raw(&self, hash: &RawTxHash) -> Option<AuthorityReadEntry<'authority>> {
        self.entries.get(hash).map(AuthorityReadEntry::new)
    }

    pub(super) fn accepted_spends(&self, out_point: &OutPoint) -> bool {
        self.membership.spender(out_point).is_some()
    }

    pub(super) fn minimum_replacement_rate(&self) -> Option<ckb_types::core::FeeRate> {
        self.membership_config.minimum_replacement_rate()
    }

    pub(super) fn owner_count(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub(super) fn replacement_history(&self) -> Result<Vec<RawTxHash>, AuthorityReadError> {
        let owners = self.entries.read_all();
        let mut history = Vec::new();
        history
            .try_reserve(owners.len())
            .map_err(|_| AuthorityReadError::Allocation)?;
        history.extend(
            owners
                .iter()
                .filter(|(_, owner)| matches!(owner, OwnedTx::ReplacementHistory(_)))
                .map(|(hash, _)| hash.clone()),
        );
        Ok(history)
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
            .get(&hash)
            .ok_or(AuthorityReadError::Projection)?;
        if &owner.record().identity.proposal != proposal {
            return Err(AuthorityReadError::Projection);
        }
        Ok(Some(AuthorityReadEntry::new(owner)))
    }

    pub(super) fn compact_transactions(
        &self,
        proposals: &[ckb_types::packed::ProposalShortId],
    ) -> Result<Vec<(ProposalId, Arc<TransactionView>)>, AuthorityReadError> {
        let mut transactions = Vec::new();
        transactions
            .try_reserve(proposals.len())
            .map_err(|_| AuthorityReadError::Allocation)?;
        for proposal in proposals {
            let proposal = ProposalId(proposal.clone());
            if let Some(entry) = self.entry_by_proposal(&proposal)? {
                transactions.push((proposal, Arc::clone(entry.transaction())));
            }
        }
        Ok(transactions)
    }

    pub(super) fn summary(&self) -> Result<AuthorityReadSummary, AuthorityReadError> {
        let owners = self.entries.read_all();
        let mut summary = AuthorityReadSummary {
            owners: owners.len(),
            ..AuthorityReadSummary::default()
        };
        for owner in owners.values() {
            match owner {
                OwnedTx::Accepted(entry) => {
                    match entry.status() {
                        AcceptedStatus::Pending => increment(&mut summary.accepted_pending)?,
                        AcceptedStatus::Gap => increment(&mut summary.accepted_gap)?,
                        AcceptedStatus::Proposed => increment(&mut summary.accepted_proposed)?,
                    }
                    summary.latest_accepted_at = Some(
                        summary
                            .latest_accepted_at
                            .map_or(entry.accepted_at, |latest| latest.max(entry.accepted_at)),
                    );
                }
                OwnedTx::PreAccepted(entry) => {
                    increment(&mut summary.preaccepted)?;
                    match &entry.phase {
                        PreAcceptedPhase::Queued(work) => {
                            increment(&mut summary.queued)?;
                            if matches!(work, QueuedWork::Verify(_)) {
                                increment(&mut summary.verify_queued)?;
                            }
                        }
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
        let counts = owners
            .status_counts()
            .ok_or(AuthorityReadError::Arithmetic)?;
        if summary.accepted_pending != counts.pending
            || summary.accepted_gap != counts.gap
            || summary.accepted_proposed != counts.proposed
        {
            return Err(AuthorityReadError::Projection);
        }
        summary.accepted_resources = owners
            .accepted_resources()
            .ok_or(AuthorityReadError::Arithmetic)?;
        let accepted_count = counts
            .pending
            .checked_add(counts.gap)
            .and_then(|count| count.checked_add(counts.proposed))
            .ok_or(AuthorityReadError::Arithmetic)?;
        if accepted_count != summary.accepted_resources.entries {
            return Err(AuthorityReadError::Projection);
        }
        Ok(summary)
    }

    pub(super) fn full_read_cut(&self) -> Result<AuthorityFullReadCut<'_>, AuthorityReadError> {
        let owners = self.entries.read_all();
        let accepted_resources = owners
            .accepted_resources()
            .ok_or(AuthorityReadError::Arithmetic)?;
        Ok(AuthorityFullReadCut {
            owners,
            accepted_resources,
        })
    }

    pub(super) fn capture_persistence(&self) -> Result<PersistenceReadReceipt, AuthorityReadError> {
        let mut selected = Vec::new();
        let owners = self.entries.read_all();
        selected
            .try_reserve(owners.len())
            .map_err(|_| AuthorityReadError::Allocation)?;
        for (hash, owner) in &owners {
            if &owner.record().identity.raw != hash {
                return Err(AuthorityReadError::Projection);
            }
            let relation = match owner {
                OwnedTx::Accepted(_) => {
                    let parents = owners
                        .membership_parents(hash)
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
        Ok(PersistenceReadReceipt { selected })
    }

    /// Capture accepted payloads, relations and their exact template source
    /// versions from this same borrowed authority cut. Sorting and template
    /// construction happen only after the caller releases the read guard.
    pub(super) fn capture_template(
        &self,
    ) -> Result<AuthorityTemplateReadReceipt, TemplateReadError> {
        let owners = self.entries.read_all();
        let sources = owners.template_sources(self.source_versions.template());
        AuthorityTemplateReadReceipt::capture(
            self.chain_view.clone(),
            sources,
            &owners,
            owners.status_counts(),
        )
    }
}

impl AuthorityFullReadCut<'_> {
    pub(super) fn owner_count(&self) -> usize {
        self.owners.len()
    }

    pub(super) fn accepted_status_counts(&self) -> Result<(usize, usize), AuthorityReadError> {
        let counts = self
            .owners
            .status_counts()
            .ok_or(AuthorityReadError::Arithmetic)?;
        let pending = counts
            .pending
            .checked_add(counts.gap)
            .ok_or(AuthorityReadError::Arithmetic)?;
        let total = pending
            .checked_add(counts.proposed)
            .ok_or(AuthorityReadError::Arithmetic)?;
        if total != self.accepted_resources.entries {
            return Err(AuthorityReadError::Projection);
        }
        Ok((pending, counts.proposed))
    }

    pub(super) fn accepted_order(&self) -> Vec<AcceptedOrderKey> {
        self.owners.accepted_order()
    }

    pub(super) fn accepted_entry_by_raw(
        &self,
        hash: &RawTxHash,
    ) -> Result<Option<AcceptedReadEntry<'_>>, AuthorityReadError> {
        let Some(OwnedTx::Accepted(entry)) = self.owners.get(hash) else {
            return Ok(None);
        };
        let ancestor = self
            .owners
            .membership_ancestor(hash)
            .ok_or(AuthorityReadError::Projection)?;
        let descendant = self
            .owners
            .membership_descendant(hash)
            .ok_or(AuthorityReadError::Projection)?;
        let order = AcceptedOrderKey::new(entry, ancestor);
        if !self.owners.contains_accepted_order(&order) {
            return Err(AuthorityReadError::Projection);
        }
        Ok(Some(AcceptedReadEntry {
            entry,
            ancestor,
            descendant,
            order,
        }))
    }

    pub(super) fn accepted_entry_for_order(
        &self,
        order: &AcceptedOrderKey,
    ) -> Result<AcceptedReadEntry<'_>, AuthorityReadError> {
        let Some(OwnedTx::Accepted(entry)) = self.owners.get(order.hash()) else {
            return Err(AuthorityReadError::Projection);
        };
        let ancestor = self
            .owners
            .membership_ancestor(order.hash())
            .ok_or(AuthorityReadError::Projection)?;
        let descendant = self
            .owners
            .membership_descendant(order.hash())
            .ok_or(AuthorityReadError::Projection)?;
        let current_order = AcceptedOrderKey::new(entry, ancestor);
        if &current_order != order {
            return Err(AuthorityReadError::Projection);
        }
        Ok(AcceptedReadEntry {
            entry,
            ancestor,
            descendant,
            order: current_order,
        })
    }

    pub(super) fn replacement_history(&self) -> Result<Vec<RawTxHash>, AuthorityReadError> {
        let mut history = Vec::new();
        history
            .try_reserve(self.owners.len())
            .map_err(|_| AuthorityReadError::Allocation)?;
        history.extend(
            self.owners
                .iter()
                .filter(|(_, owner)| matches!(owner, OwnedTx::ReplacementHistory(_)))
                .map(|(hash, _)| hash.clone()),
        );
        Ok(history)
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
    selected: Vec<PersistenceRow>,
}

impl PersistenceReadReceipt {
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
        Ok(ParentFirstPersistence { accepted, recovery })
    }
}

#[derive(Debug)]
pub(super) struct ParentFirstPersistence {
    accepted: Vec<Arc<TransactionView>>,
    recovery: Vec<Arc<TransactionView>>,
}

impl ParentFirstPersistence {
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

#[cfg(test)]
#[path = "tests/support/read.rs"]
mod test_support;
