mod membership;
mod settlement;

use super::resources::{
    ChargeRecord, ResourceBatchPlan, ResourceError, ResourceLedger, ResourceLimits, ResourcePlan,
    ResourceSnapshot, ResourceVector,
};
use super::state::{
    AcceptedEntry, AcceptedStatus, AdmissionClass, ApplySequence, Arrival, AuthorityClocks,
    ChainEpoch, EntryVersion, IngressAttribution, OwnedTx, PayloadBlame, PreAcceptedEntry,
    PreAcceptedPhase, ProposalId, QueuedWork, RawTxHash, TxIdentity, TxRecord, ValidatedAdmission,
};
use super::work::{CheckedOutWork, ComputeSettlement, LeaseToken, SettlementNext};
pub(in crate::authority) use membership::IndependentCoupling;
#[cfg(test)]
pub(in crate::authority) use membership::{
    DescendantAggregate, EvictionOrderKey, MembershipSnapshot,
};
use membership::{
    MembershipConfig, MembershipProjection, MembershipRemoval, PreparedMembership, ProjectionDelta,
};
pub(in crate::authority) use membership::{MembershipReject, RemovalCause, StatusCounts};
pub(in crate::authority) use settlement::{
    CandidateBatchError, IndependentCandidate, SettlementBatch, SettlementPlan,
};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum OwnerPhaseSnapshot {
    PreAccepted(PreAcceptedPhase),
    Accepted {
        status: AcceptedStatus,
        verified: super::state::VerifiedFacts,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OwnerSnapshot {
    identity: TxIdentity,
    ingress: IngressAttribution,
    blame: PayloadBlame,
    class: AdmissionClass,
    version: EntryVersion,
    arrival: Arrival,
    charge: ChargeRecord,
    phase: OwnerPhaseSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AuthoritySnapshot {
    chain_epoch: ChainEpoch,
    entries: HashMap<RawTxHash, OwnerSnapshot>,
    by_proposal: HashMap<ProposalId, RawTxHash>,
    resources: ResourceSnapshot,
    membership: MembershipSnapshot,
    clocks: AuthorityClocks,
}

#[derive(Debug)]
pub(super) struct TxPoolAuthority {
    chain_epoch: ChainEpoch,
    entries: HashMap<RawTxHash, OwnedTx>,
    by_proposal: HashMap<ProposalId, RawTxHash>,
    resources: ResourceLedger,
    membership: MembershipProjection,
    membership_config: MembershipConfig,
    clocks: AuthorityClocks,
}

impl TxPoolAuthority {
    pub(super) fn new(limits: ResourceLimits) -> Self {
        Self {
            chain_epoch: ChainEpoch(0),
            entries: HashMap::new(),
            by_proposal: HashMap::new(),
            resources: ResourceLedger::new(limits),
            membership: MembershipProjection::default(),
            membership_config: MembershipConfig::testing_default(),
            clocks: AuthorityClocks::first(),
        }
    }

    #[cfg(test)]
    pub(super) fn with_replacement(
        limits: ResourceLimits,
        minimum_rate: ckb_types::core::FeeRate,
    ) -> Self {
        let mut authority = Self::new(limits);
        authority.membership_config = MembershipConfig::testing_with_replacement(minimum_rate);
        authority
    }

    pub(super) fn entry(&self, hash: &RawTxHash) -> Option<&OwnedTx> {
        self.entries.get(hash)
    }

    pub(super) fn owner_count(&self) -> usize {
        self.entries.len()
    }

    pub(super) fn charged_count(&self) -> usize {
        self.resources.charge_count()
    }

    pub(super) fn resources(&self) -> &ResourceLedger {
        &self.resources
    }

    pub(super) fn membership_counts(&self) -> StatusCounts {
        self.membership.counts()
    }

    pub(super) fn accepted_spender(
        &self,
        input: &ckb_types::packed::OutPoint,
    ) -> Option<&RawTxHash> {
        self.membership.spender(input)
    }

    pub(super) fn accepted_parents(
        &self,
        hash: &RawTxHash,
    ) -> Option<&std::collections::HashSet<RawTxHash>> {
        self.membership.parents(hash)
    }

    pub(super) fn accepted_children(
        &self,
        hash: &RawTxHash,
    ) -> Option<&std::collections::HashSet<RawTxHash>> {
        self.membership.children(hash)
    }

    pub(super) fn chain_epoch(&self) -> ChainEpoch {
        self.chain_epoch
    }

    pub(super) fn clocks(&self) -> AuthorityClocks {
        self.clocks
    }

    #[cfg(test)]
    pub(super) fn entries_for_reference(&self) -> &HashMap<RawTxHash, OwnedTx> {
        &self.entries
    }

    #[cfg(test)]
    pub(super) fn membership_snapshot_for_reference(&self) -> MembershipSnapshot {
        self.membership.snapshot()
    }

    #[cfg(test)]
    pub(super) fn force_chain_epoch(&mut self, epoch: ChainEpoch) {
        self.chain_epoch = epoch;
    }

    #[cfg(test)]
    pub(super) fn force_next_sequence(&mut self, sequence: ApplySequence) {
        self.clocks.next_sequence = sequence;
    }

    pub(super) fn normalized_snapshot(&self) -> AuthoritySnapshot {
        let entries = self
            .entries
            .iter()
            .map(|(hash, owner)| {
                let record = owner.record();
                let phase = match owner {
                    OwnedTx::PreAccepted(entry) => {
                        OwnerPhaseSnapshot::PreAccepted(entry.phase.clone())
                    }
                    OwnedTx::Accepted(entry) => OwnerPhaseSnapshot::Accepted {
                        status: entry.status,
                        verified: entry.verified.clone(),
                    },
                };
                (
                    hash.clone(),
                    OwnerSnapshot {
                        identity: record.identity.clone(),
                        ingress: record.ingress,
                        blame: record.blame,
                        class: record.class,
                        version: record.version,
                        arrival: record.arrival,
                        charge: owner.charge_record(),
                        phase,
                    },
                )
            })
            .collect();
        AuthoritySnapshot {
            chain_epoch: self.chain_epoch,
            entries,
            by_proposal: self.by_proposal.clone(),
            resources: self.resources.snapshot(),
            membership: self.membership.snapshot(),
            clocks: self.clocks,
        }
    }

    pub(super) fn primary_projection_consistent(&self) -> bool {
        self.entries.len() == self.resources.charge_count()
            && self.entries.len() == self.by_proposal.len()
            && self.entries.iter().all(|(hash, owner)| {
                self.resources.charge(hash) == Some(owner.charge_record())
                    && self.by_proposal.get(&owner.record().identity.proposal) == Some(hash)
                    && &owner.record().identity.raw == hash
            })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Backpressure {
    ProposalCollision,
    TotalResources,
    RemoteResources,
    PeerResources,
    AcceptedResources,
    Allocation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum StalePlan {
    Missing,
    Version,
    Phase,
    ChainEpoch,
    Lease,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AuthorityFault {
    CounterExhausted,
    ResourceProjection,
    MembershipProjection,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PlanError {
    Duplicate,
    PayloadVariant,
    Membership(MembershipReject),
    Backpressure(Backpressure),
    Stale(StalePlan),
    Fault(AuthorityFault),
}

impl From<ResourceError> for PlanError {
    fn from(error: ResourceError) -> Self {
        match error {
            ResourceError::PreAcceptedLimit => Self::Backpressure(Backpressure::TotalResources),
            ResourceError::RemoteLimit => Self::Backpressure(Backpressure::RemoteResources),
            ResourceError::PeerLimit(_) => Self::Backpressure(Backpressure::PeerResources),
            ResourceError::AcceptedLimit => Self::Backpressure(Backpressure::AcceptedResources),
            ResourceError::Allocation => Self::Backpressure(Backpressure::Allocation),
            ResourceError::Arithmetic | ResourceError::ExistingChargeMismatch => {
                Self::Fault(AuthorityFault::ResourceProjection)
            }
            ResourceError::DuplicateChange => Self::Fault(AuthorityFault::ResourceProjection),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CommittedChange {
    pub(super) sequence: ApplySequence,
    pub(super) changed: RawTxHash,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum CommittedChanges {
    One(CommittedChange),
    IndependentRun(Vec<CommittedChange>),
}

#[derive(Debug)]
#[must_use = "a committed delta contains the only post-Apply work/effect handoff"]
pub(super) struct CommittedDelta {
    pub(super) changes: CommittedChanges,
    pub(super) work: Option<CheckedOutWork>,
    pub(super) removals: Vec<MembershipRemoval>,
    retired: Vec<OwnedTx>,
}

enum EntryRetirement {
    InlineDrop,
    Outside(Vec<OwnedTx>),
}

impl CommittedDelta {
    pub(in crate::authority) fn retired_len(&self) -> usize {
        self.retired.len()
    }
}

struct EntryDelta {
    key: RawTxHash,
    old_proposal: Option<ProposalId>,
    after: Option<OwnedTx>,
    retirement: EntryRetirement,
    resource: ResourcePlan,
    clocks: AuthorityClocks,
    sequence: ApplySequence,
}

struct MembershipDelta {
    changed_key: RawTxHash,
    changed_after: OwnedTx,
    removals: Vec<MembershipRemoval>,
    resource: ResourceBatchPlan,
    projection: ProjectionDelta,
    retired: Vec<OwnedTx>,
    clocks: AuthorityClocks,
    committed: CommittedChanges,
}

struct IndependentUpdate {
    key: RawTxHash,
    after: OwnedTx,
}

struct IndependentDelta {
    updates: Vec<IndependentUpdate>,
    resource: ResourceBatchPlan,
    projection: ProjectionDelta,
    clocks: AuthorityClocks,
    committed: Vec<CommittedChange>,
}

enum AuthorityDelta {
    Entry(EntryDelta),
    Membership(MembershipDelta),
    Independent(IndependentDelta),
}

#[must_use = "a prepared authority transition has no effect until explicitly applied"]
pub(super) struct PreparedApply<'authority> {
    authority: &'authority mut TxPoolAuthority,
    delta: AuthorityDelta,
    work: Option<CheckedOutWork>,
}

impl PreparedApply<'_> {
    pub(super) fn apply(self) -> CommittedDelta {
        let Self {
            authority,
            delta,
            work,
        } = self;
        match delta {
            AuthorityDelta::Entry(delta) => Self::apply_entry(authority, delta, work),
            AuthorityDelta::Membership(delta) => Self::apply_membership(authority, delta),
            AuthorityDelta::Independent(delta) => Self::apply_independent(authority, delta),
        }
    }

    fn apply_entry(
        authority: &mut TxPoolAuthority,
        delta: EntryDelta,
        work: Option<CheckedOutWork>,
    ) -> CommittedDelta {
        if let Some(proposal) = delta.old_proposal {
            authority.by_proposal.remove(&proposal);
        }
        let previous = match delta.after {
            Some(entry) => {
                let proposal = entry.record().identity.proposal.clone();
                authority.by_proposal.insert(proposal, delta.key.clone());
                authority.entries.insert(delta.key.clone(), entry)
            }
            None => authority.entries.remove(&delta.key),
        };
        let retired = match (delta.retirement, previous) {
            (EntryRetirement::Outside(mut retired), Some(owner)) => {
                retired.push(owner);
                retired
            }
            (EntryRetirement::Outside(retired), None) => retired,
            (EntryRetirement::InlineDrop, previous) => {
                drop(previous);
                Vec::new()
            }
        };
        authority.resources.apply(delta.resource);
        authority.clocks = delta.clocks;
        CommittedDelta {
            changes: CommittedChanges::One(CommittedChange {
                sequence: delta.sequence,
                changed: delta.key,
            }),
            work,
            removals: Vec::new(),
            retired,
        }
    }

    fn apply_membership(authority: &mut TxPoolAuthority, delta: MembershipDelta) -> CommittedDelta {
        let mut retired = delta.retired;
        for removal in &delta.removals {
            if let Some(owner) = authority.entries.remove(&removal.hash) {
                retired.push(owner);
            }
            authority.by_proposal.remove(&removal.proposal);
        }
        // The candidate's accepted owner shares its immutable transaction and
        // resolved facts with the pre-accepted predecessor. Only removed
        // victims can carry the last large owner, so their destruction is
        // handed out in `retired`; replacing this small shell cannot allocate.
        drop(
            authority
                .entries
                .insert(delta.changed_key, delta.changed_after),
        );
        authority.resources.apply_batch(delta.resource);
        authority.membership.apply(delta.projection);
        authority.clocks = delta.clocks;
        CommittedDelta {
            changes: delta.committed,
            work: None,
            removals: delta.removals,
            retired,
        }
    }

    fn apply_independent(
        authority: &mut TxPoolAuthority,
        delta: IndependentDelta,
    ) -> CommittedDelta {
        for update in delta.updates {
            // Independent acceptance also replaces a pre-accepted shell whose
            // immutable transaction and resolved facts are shared by `after`.
            drop(authority.entries.insert(update.key, update.after));
        }
        authority.resources.apply_batch(delta.resource);
        authority.membership.apply(delta.projection);
        authority.clocks = delta.clocks;
        CommittedDelta {
            changes: CommittedChanges::IndependentRun(delta.committed),
            work: None,
            removals: Vec::new(),
            retired: Vec::new(),
        }
    }
}

fn next_version(version: EntryVersion) -> Result<EntryVersion, PlanError> {
    version
        .0
        .checked_add(1)
        .map(EntryVersion)
        .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))
}

fn next_arrival(arrival: Arrival) -> Result<Arrival, PlanError> {
    arrival
        .0
        .checked_add(1)
        .map(Arrival)
        .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))
}

fn next_lease(
    lease: super::state::ComputeLeaseId,
) -> Result<super::state::ComputeLeaseId, PlanError> {
    lease
        .0
        .checked_add(1)
        .map(super::state::ComputeLeaseId)
        .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))
}

fn next_sequence(sequence: ApplySequence) -> Result<ApplySequence, PlanError> {
    sequence
        .0
        .checked_add(1)
        .map(ApplySequence)
        .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))
}

fn retired_buffer(capacity: usize) -> Result<Vec<OwnedTx>, PlanError> {
    let mut retired = Vec::new();
    retired
        .try_reserve(capacity)
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
    Ok(retired)
}

impl TxPoolAuthority {
    pub(super) fn plan_admission(
        &mut self,
        admission: ValidatedAdmission,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let key = admission.identity.raw.clone();
        if let Some(existing) = self.entries.get(&key).cloned() {
            return self.plan_existing_admission(key, existing, admission);
        }
        if self.by_proposal.contains_key(&admission.identity.proposal) {
            return Err(PlanError::Backpressure(Backpressure::ProposalCollision));
        }

        let version = self.clocks.next_version;
        let arrival = self.clocks.next_arrival;
        let sequence = self.clocks.next_sequence;
        let clocks = AuthorityClocks {
            next_version: next_version(version)?,
            next_arrival: next_arrival(arrival)?,
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
        let record = TxRecord {
            tx: admission.tx,
            identity: admission.identity,
            ingress: admission.ingress,
            blame: admission.blame,
            class: admission.class,
            version,
            arrival,
        };
        let after = OwnedTx::PreAccepted(PreAcceptedEntry {
            record,
            phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
            charge: admission.charge,
        });
        self.prepare_entry_delta(key, None, Some(after), clocks, sequence, None)
    }

    fn plan_existing_admission(
        &mut self,
        key: RawTxHash,
        existing: OwnedTx,
        admission: ValidatedAdmission,
    ) -> Result<PreparedApply<'_>, PlanError> {
        if existing.record().identity.witness != admission.identity.witness {
            return Err(PlanError::PayloadVariant);
        }
        let AdmissionClass::Proposal(proposal) = admission.class else {
            return Err(PlanError::Duplicate);
        };
        let OwnedTx::PreAccepted(entry) = &existing else {
            return Err(PlanError::Duplicate);
        };
        if entry.record.class == AdmissionClass::Proposal(proposal) {
            return Err(PlanError::Duplicate);
        }

        let sequence = self.clocks.next_sequence;
        let clocks = AuthorityClocks {
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
        let mut promoted = entry.clone();
        promoted.record.class = AdmissionClass::Proposal(proposal);
        self.prepare_entry_delta(
            key,
            Some(existing),
            Some(OwnedTx::PreAccepted(promoted)),
            clocks,
            sequence,
            None,
        )
    }

    pub(super) fn plan_accept_for_foundation(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        status: AcceptedStatus,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let existing = self
            .entries
            .get(key)
            .cloned()
            .ok_or(PlanError::Stale(StalePlan::Missing))?;
        if existing.record().version != expected {
            return Err(PlanError::Stale(StalePlan::Version));
        }
        let OwnedTx::PreAccepted(preaccepted) = &existing else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        let PreAcceptedPhase::Computed(super::state::ComputedOutcome::Verified(verified)) =
            &preaccepted.phase
        else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        if verified.chain_epoch != self.chain_epoch {
            return Err(PlanError::Stale(StalePlan::ChainEpoch));
        }
        let version = self.clocks.next_version;
        let sequence = self.clocks.next_sequence;
        let clocks = AuthorityClocks {
            next_version: next_version(version)?,
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
        let mut record = preaccepted.record.clone();
        record.version = version;
        let accepted = AcceptedEntry {
            record,
            status,
            verified: verified.clone(),
        };
        let PreparedMembership {
            removals,
            resource,
            projection,
        } = self.prepare_membership(key, preaccepted, &accepted)?;
        let retired = retired_buffer(removals.len())?;
        let after = OwnedTx::Accepted(accepted);
        Ok(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Membership(MembershipDelta {
                changed_key: key.clone(),
                changed_after: after,
                removals,
                resource,
                projection,
                retired,
                clocks,
                committed: CommittedChanges::One(CommittedChange {
                    sequence,
                    changed: key.clone(),
                }),
            }),
            work: None,
        })
    }

    pub(super) fn plan_status_for_foundation(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        status: AcceptedStatus,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let existing = self
            .entries
            .get(key)
            .cloned()
            .ok_or(PlanError::Stale(StalePlan::Missing))?;
        if existing.record().version != expected {
            return Err(PlanError::Stale(StalePlan::Version));
        }
        let OwnedTx::Accepted(before) = &existing else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        if before.status == status {
            return Err(PlanError::Duplicate);
        }

        let version = self.clocks.next_version;
        let sequence = self.clocks.next_sequence;
        let clocks = AuthorityClocks {
            next_version: next_version(version)?,
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
        let mut after = before.clone();
        after.record.version = version;
        after.status = status;
        let projection = self.prepare_status_change(key, before, &after)?;
        let after = OwnedTx::Accepted(after);
        let mut resource_changes = Vec::new();
        resource_changes
            .try_reserve(1)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        resource_changes.push((
            key.clone(),
            Some(existing.charge_record()),
            Some(after.charge_record()),
        ));
        let resource = self.resources.plan_batch(resource_changes)?;
        let retired = Vec::new();
        Ok(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Membership(MembershipDelta {
                changed_key: key.clone(),
                changed_after: after,
                removals: Vec::new(),
                resource,
                projection,
                retired,
                clocks,
                committed: CommittedChanges::One(CommittedChange {
                    sequence,
                    changed: key.clone(),
                }),
            }),
            work: None,
        })
    }

    pub(super) fn plan_terminalize_for_foundation(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let existing = self
            .entries
            .get(key)
            .ok_or(PlanError::Stale(StalePlan::Missing))?;
        if existing.record().version != expected {
            return Err(PlanError::Stale(StalePlan::Version));
        }
        if !matches!(existing, OwnedTx::PreAccepted(_)) {
            return Err(PlanError::Stale(StalePlan::Phase));
        }
        let sequence = self.clocks.next_sequence;
        let clocks = AuthorityClocks {
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
        self.prepare_entry_delta(
            key.clone(),
            Some(existing.clone()),
            None,
            clocks,
            sequence,
            None,
        )
    }

    pub(super) fn plan_checkout(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        permit: super::state::WorkPermit,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let existing = self
            .entries
            .get(key)
            .ok_or(PlanError::Stale(StalePlan::Missing))?;
        if existing.record().version != expected {
            return Err(PlanError::Stale(StalePlan::Version));
        }
        let OwnedTx::PreAccepted(preaccepted) = existing else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        let PreAcceptedPhase::Queued(queued) = &preaccepted.phase else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };

        let version = self.clocks.next_version;
        let lease = self.clocks.next_lease;
        let sequence = self.clocks.next_sequence;
        let token = LeaseToken {
            hash: key.clone(),
            version,
            lease,
            chain_epoch: self.chain_epoch,
            permit,
        };
        let work = CheckedOutWork::new(token, Arc::clone(&preaccepted.record.tx), queued.clone())
            .map_err(|_| PlanError::Stale(StalePlan::Phase))?;
        let mut charge = preaccepted.charge;
        charge.active_work = charge
            .active_work
            .checked_add(1)
            .ok_or(PlanError::Fault(AuthorityFault::ResourceProjection))?;
        let after = existing
            .with_foundation_phase(
                PreAcceptedPhase::Computing(super::state::ActiveWork { lease, permit }),
                version,
                charge,
            )
            .map_err(PlanError::Stale)?;
        let clocks = AuthorityClocks {
            next_version: next_version(version)?,
            next_lease: next_lease(lease)?,
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
        self.prepare_entry_delta(
            key.clone(),
            Some(existing.clone()),
            Some(after),
            clocks,
            sequence,
            Some(work),
        )
    }

    pub(super) fn plan_settlement(
        &mut self,
        settlement: ComputeSettlement,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let ComputeSettlement { token, next } = settlement;
        let existing = self
            .entries
            .get(&token.hash)
            .ok_or(PlanError::Stale(StalePlan::Missing))?;
        if existing.record().version != token.version {
            return Err(PlanError::Stale(StalePlan::Version));
        }
        if self.chain_epoch != token.chain_epoch {
            return Err(PlanError::Stale(StalePlan::ChainEpoch));
        }
        let OwnedTx::PreAccepted(preaccepted) = existing else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        let PreAcceptedPhase::Computing(active) = &preaccepted.phase else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        if active.lease != token.lease || active.permit != token.permit {
            return Err(PlanError::Stale(StalePlan::Lease));
        }
        let phase = match next {
            SettlementNext::QueuedVerify(resolved) if resolved.chain_epoch == token.chain_epoch => {
                PreAcceptedPhase::Queued(QueuedWork::Verify(resolved))
            }
            SettlementNext::QueuedVerify(_) => {
                return Err(PlanError::Stale(StalePlan::ChainEpoch));
            }
            SettlementNext::Waiting(wait) => PreAcceptedPhase::Waiting(wait),
            SettlementNext::Computed(super::state::ComputedOutcome::Verified(verified))
                if verified.chain_epoch != token.chain_epoch =>
            {
                return Err(PlanError::Stale(StalePlan::ChainEpoch));
            }
            SettlementNext::Computed(outcome) => PreAcceptedPhase::Computed(outcome),
        };
        let mut charge = preaccepted.charge;
        charge.active_work = charge
            .active_work
            .checked_sub(1)
            .ok_or(PlanError::Fault(AuthorityFault::ResourceProjection))?;
        let version = self.clocks.next_version;
        let sequence = self.clocks.next_sequence;
        let after = existing
            .with_foundation_phase(phase, version, charge)
            .map_err(PlanError::Stale)?;
        let clocks = AuthorityClocks {
            next_version: next_version(version)?,
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
        self.prepare_entry_delta(
            token.hash,
            Some(existing.clone()),
            Some(after),
            clocks,
            sequence,
            None,
        )
    }

    fn prepare_entry_delta(
        &mut self,
        key: RawTxHash,
        expected: Option<OwnedTx>,
        after: Option<OwnedTx>,
        clocks: AuthorityClocks,
        sequence: ApplySequence,
        work: Option<CheckedOutWork>,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let expected_charge = expected.as_ref().map(OwnedTx::charge_record);
        let after_charge = after.as_ref().map(OwnedTx::charge_record);
        let old_proposal = expected
            .as_ref()
            .map(|entry| entry.record().identity.proposal.clone());
        let retirement = if expected.is_some() && after.is_none() {
            EntryRetirement::Outside(retired_buffer(1)?)
        } else {
            EntryRetirement::InlineDrop
        };
        if after.is_some() && expected.is_none() {
            self.entries
                .try_reserve(1)
                .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
            self.by_proposal
                .try_reserve(1)
                .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        }
        let resource = self
            .resources
            .plan_replace(key.clone(), expected_charge, after_charge)?;
        Ok(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Entry(EntryDelta {
                key,
                old_proposal,
                after,
                retirement,
                resource,
                clocks,
                sequence,
            }),
            work,
        })
    }
}

impl OwnedTx {
    pub(super) fn with_foundation_phase(
        &self,
        phase: PreAcceptedPhase,
        version: EntryVersion,
        charge: ResourceVector,
    ) -> Result<Self, StalePlan> {
        let Self::PreAccepted(entry) = self else {
            return Err(StalePlan::Phase);
        };
        let mut record = entry.record.clone();
        record.version = version;
        Ok(Self::PreAccepted(PreAcceptedEntry {
            record,
            phase,
            charge,
        }))
    }
}
