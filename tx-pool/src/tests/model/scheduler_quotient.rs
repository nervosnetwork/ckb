//! Ephemeral scheduler quotient for multi-owner retained-compute waves.
//!
//! Runnable entries are derived from one `Omega` cut. The quotient owns only
//! the two temporal owner cursors that cannot be reconstructed from the owner
//! set. Worker roles are transient plan input and never become owner state.

use super::{
    permit::{RetainedWorkerGrant, RetainedWorkerGrantBatch, RetainedWorkerRole},
    state::{
        Arrival, EntryVersion, Omega, OwnerLocation, PeerId, PoolGeneration, ProposalBase,
        RemoteDeadline, RemoteResidency, RetainedOwner, RetainedPhase, Source, TxId,
        VerifyCapability, VerifyCycleClass, WorkPermit, WorkStage,
    },
};
use std::{cmp::Ordering, collections::BTreeSet};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SchedulerVerifyOrder {
    #[default]
    Arrival,
    FeeRate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum SchedulerOwner {
    Remote(PeerId),
    Trusted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SchedulerLane {
    Resolve,
    Verify,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SchedulerEntry {
    transaction: TxId,
    version: EntryVersion,
    arrival: Arrival,
    source: Source,
    owner: SchedulerOwner,
    stage: SchedulerEntryStage,
    fee: u64,
    bytes: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SchedulerEntryStage {
    Resolve,
    Verify(VerifyCycleClass),
}

impl SchedulerEntry {
    fn from_owner(transaction: TxId, owner: &super::state::Owner) -> Option<Self> {
        let OwnerLocation::Retained(RetainedOwner {
            source,
            phase: RetainedPhase::Queued(stage),
        }) = &owner.location
        else {
            return None;
        };
        Some(Self {
            transaction,
            version: owner.version,
            arrival: owner.arrival,
            source: *source,
            owner: scheduler_owner(*source),
            stage: match stage {
                WorkStage::Resolve => SchedulerEntryStage::Resolve,
                WorkStage::Verify(evidence) => SchedulerEntryStage::Verify(evidence.verify_class),
            },
            fee: owner.transaction.cost.fee(),
            bytes: owner.transaction.cost.serialized_bytes(),
        })
    }

    fn verify_class(&self) -> Option<VerifyCycleClass> {
        match self.stage {
            SchedulerEntryStage::Resolve => None,
            SchedulerEntryStage::Verify(class) => Some(class),
        }
    }
}

/// Exact role-compatible selection emitted by the quotient. Its fields are
/// private so an assigned checkout cannot be fabricated outside this module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SchedulerAssignment {
    transaction: TxId,
    permit: WorkPermit,
}

impl SchedulerAssignment {
    pub(super) const fn transaction(&self) -> TxId {
        self.transaction
    }

    pub(super) const fn permit(&self) -> WorkPermit {
        self.permit
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SchedulerQuotient {
    verify_order: SchedulerVerifyOrder,
    resolve_cursor: Option<SchedulerOwner>,
    verify_cursor: Option<SchedulerOwner>,
}

impl Default for SchedulerQuotient {
    fn default() -> Self {
        Self::new(SchedulerVerifyOrder::Arrival)
    }
}

impl SchedulerQuotient {
    pub(super) const fn new(verify_order: SchedulerVerifyOrder) -> Self {
        Self {
            verify_order,
            resolve_cursor: None,
            verify_cursor: None,
        }
    }

    pub(super) const fn cursors(&self) -> (Option<SchedulerOwner>, Option<SchedulerOwner>) {
        (self.resolve_cursor, self.verify_cursor)
    }

    pub(super) fn plan_wave(
        &self,
        omega: &Omega,
        grants: RetainedWorkerGrantBatch,
    ) -> SchedulerWavePlan {
        let mut after = self.clone();
        let mut entries = omega
            .authority
            .owners
            .iter()
            .filter_map(|(transaction, owner)| SchedulerEntry::from_owner(*transaction, owner))
            .collect::<Vec<_>>();
        let mut assignments = Vec::new();
        let mut idle = Vec::new();
        for grant in grants.into_grants() {
            let slot = grant.slot();
            let Some((entry, permit)) = after.select_for_slot(&mut entries, slot.role()) else {
                idle.push(grant);
                continue;
            };
            assignments.push((
                grant,
                SchedulerAssignment {
                    transaction: entry.transaction,
                    permit,
                },
            ));
        }
        SchedulerWavePlan {
            cursor: SchedulerCursorPlan {
                expected: self.clone(),
                after,
            },
            assignments,
            idle,
        }
    }

    fn select_for_slot(
        &mut self,
        entries: &mut Vec<SchedulerEntry>,
        role: RetainedWorkerRole,
    ) -> Option<(SchedulerEntry, WorkPermit)> {
        let selected = match role {
            RetainedWorkerRole::OrderedResolve => {
                self.select(entries, SchedulerLane::Resolve, VerifyCapability::Any)
            }
            RetainedWorkerRole::Verifier(capability) => self
                .select(entries, SchedulerLane::Verify, capability)
                .or_else(|| self.select(entries, SchedulerLane::Resolve, capability)),
        };
        selected.map(|(entry, lane)| {
            let permit = match lane {
                SchedulerLane::Resolve => role.resolve_permit(),
                SchedulerLane::Verify => role
                    .verify_permit()
                    .unwrap_or_else(|| role.resolve_permit()),
            };
            (entry, permit)
        })
    }

    fn select(
        &mut self,
        entries: &mut Vec<SchedulerEntry>,
        lane: SchedulerLane,
        capability: VerifyCapability,
    ) -> Option<(SchedulerEntry, SchedulerLane)> {
        let owners = entries
            .iter()
            .filter(|entry| eligible(entry, lane, capability))
            .map(|entry| entry.owner)
            .collect::<BTreeSet<_>>();
        let cursor = match lane {
            SchedulerLane::Resolve => self.resolve_cursor,
            SchedulerLane::Verify => self.verify_cursor,
        };
        let owner = next_owner(&owners, cursor)?;
        let selected = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.owner == owner && eligible(entry, lane, capability))
            .min_by(|(_, left), (_, right)| match lane {
                SchedulerLane::Resolve => resolve_order(left, right),
                SchedulerLane::Verify => verify_order(self.verify_order, right, left),
            })
            .map(|(index, _)| index)?;
        let entry = entries.remove(selected);
        match lane {
            SchedulerLane::Resolve => self.resolve_cursor = Some(owner),
            SchedulerLane::Verify => self.verify_cursor = Some(owner),
        }
        Some((entry, lane))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SchedulerCursorPlan {
    expected: SchedulerQuotient,
    after: SchedulerQuotient,
}

impl SchedulerCursorPlan {
    pub(super) fn is_current(&self, current: &SchedulerQuotient) -> bool {
        &self.expected == current
    }

    pub(super) fn apply(self, current: &mut SchedulerQuotient) -> bool {
        if !self.is_current(current) {
            return false;
        }
        *current = self.after;
        true
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct SchedulerWavePlan {
    cursor: SchedulerCursorPlan,
    assignments: Vec<(RetainedWorkerGrant, SchedulerAssignment)>,
    idle: Vec<RetainedWorkerGrant>,
}

impl SchedulerWavePlan {
    pub(super) fn assignments(&self) -> &[(RetainedWorkerGrant, SchedulerAssignment)] {
        &self.assignments
    }

    pub(super) fn into_parts(
        self,
    ) -> (
        SchedulerCursorPlan,
        Vec<(RetainedWorkerGrant, SchedulerAssignment)>,
        Vec<RetainedWorkerGrant>,
    ) {
        (self.cursor, self.assignments, self.idle)
    }
}

fn scheduler_owner(source: Source) -> SchedulerOwner {
    match source {
        Source::Remote(residency) => SchedulerOwner::Remote(residency.peer),
        Source::Proposal { .. } | Source::Recovery(_) => SchedulerOwner::Trusted,
    }
}

const fn source_rank(source: Source) -> u8 {
    match source {
        Source::Remote(_) => 0,
        Source::Proposal { .. } => 1,
        Source::Recovery(_) => 2,
    }
}

fn eligible(entry: &SchedulerEntry, lane: SchedulerLane, capability: VerifyCapability) -> bool {
    match (lane, entry.stage) {
        (SchedulerLane::Resolve, SchedulerEntryStage::Resolve) => true,
        (SchedulerLane::Verify, SchedulerEntryStage::Verify(class)) => capability.permits(class),
        _ => false,
    }
}

fn next_owner(
    owners: &BTreeSet<SchedulerOwner>,
    cursor: Option<SchedulerOwner>,
) -> Option<SchedulerOwner> {
    if cursor.is_none() && owners.contains(&SchedulerOwner::Trusted) {
        return Some(SchedulerOwner::Trusted);
    }
    cursor
        .and_then(|cursor| {
            owners
                .range((
                    std::ops::Bound::Excluded(cursor),
                    std::ops::Bound::Unbounded,
                ))
                .next()
                .copied()
        })
        .or_else(|| owners.first().copied())
}

fn resolve_order(left: &SchedulerEntry, right: &SchedulerEntry) -> Ordering {
    source_rank(right.source)
        .cmp(&source_rank(left.source))
        .then_with(|| left.arrival.cmp(&right.arrival))
        .then_with(|| left.transaction.cmp(&right.transaction))
        .then_with(|| left.version.cmp(&right.version))
}

fn verify_order(
    order: SchedulerVerifyOrder,
    left: &SchedulerEntry,
    right: &SchedulerEntry,
) -> Ordering {
    let configured = source_rank(left.source)
        .cmp(&source_rank(right.source))
        .then_with(|| match order {
            SchedulerVerifyOrder::Arrival => Ordering::Equal,
            SchedulerVerifyOrder::FeeRate => {
                let left_rate = u128::from(left.fee) * u128::from(right.bytes);
                let right_rate = u128::from(right.fee) * u128::from(left.bytes);
                left_rate
                    .cmp(&right_rate)
                    .then_with(|| left.fee.cmp(&right.fee))
            }
        });
    configured
        .then_with(|| right.arrival.cmp(&left.arrival))
        .then_with(|| right.transaction.cmp(&left.transaction))
        .then_with(|| left.version.cmp(&right.version))
        .then_with(|| left.verify_class().cmp(&right.verify_class()))
}

// Only this finite input/observation algebra crosses into production
// refinement tests. Production reconstructs the same cut from real authority
// owners and calls its own scheduler; it cannot construct or inspect model
// `Omega`, entries, grants or cursor plans.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SchedulerRefinementVerifyOrder {
    Arrival,
    FeeRate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SchedulerRefinementSource {
    Remote(u8),
    Proposal,
    Recovery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SchedulerRefinementStage {
    Resolve,
    Verify(SchedulerRefinementVerifyClass),
    /// Ready remains part of the derived scheduler set, but it is not
    /// eligible for a retained-compute worker assignment.
    Ready,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SchedulerRefinementVerifyClass {
    Small,
    Large,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SchedulerRefinementCapability {
    SmallOnly,
    Any,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SchedulerRefinementEntry {
    pub(crate) transaction: u8,
    pub(crate) version: u16,
    pub(crate) arrival: u16,
    pub(crate) source: SchedulerRefinementSource,
    pub(crate) stage: SchedulerRefinementStage,
    pub(crate) fee: u64,
    pub(crate) bytes: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SchedulerRefinementWorkerRole {
    OrderedResolve,
    VerifySmall,
    VerifyAny,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SchedulerRefinementWorker {
    pub(crate) slot: u8,
    pub(crate) role: SchedulerRefinementWorkerRole,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SchedulerRefinementOwner {
    Remote(u8),
    Trusted,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SchedulerRefinementCursors {
    pub(crate) resolve: Option<SchedulerRefinementOwner>,
    pub(crate) verify: Option<SchedulerRefinementOwner>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SchedulerRefinementPermit {
    ResolveOnly,
    ResolveThenVerify(SchedulerRefinementCapability),
    VerifyOnly(SchedulerRefinementCapability),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SchedulerRefinementAssignment {
    pub(crate) slot: u8,
    pub(crate) transaction: u8,
    pub(crate) owner: SchedulerRefinementOwner,
    pub(crate) permit: SchedulerRefinementPermit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SchedulerRefinementObservation {
    pub(crate) assignments: Vec<SchedulerRefinementAssignment>,
    pub(crate) idle_slots: Vec<u8>,
    pub(crate) cursors: SchedulerRefinementCursors,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SchedulerRefinementError {
    DuplicateTransaction(u8),
    DuplicateWorker(u8),
    MultipleOrderedResolvers,
    ZeroSerializedBytes(u8),
}

pub(crate) fn scheduler_wave_observation(
    input: &[SchedulerRefinementEntry],
    workers: &[SchedulerRefinementWorker],
    cursors: SchedulerRefinementCursors,
    order: SchedulerRefinementVerifyOrder,
) -> Result<SchedulerRefinementObservation, SchedulerRefinementError> {
    let mut transactions = BTreeSet::new();
    let mut entries = Vec::with_capacity(input.len());
    for entry in input {
        if !transactions.insert(entry.transaction) {
            return Err(SchedulerRefinementError::DuplicateTransaction(
                entry.transaction,
            ));
        }
        if entry.bytes == 0 {
            return Err(SchedulerRefinementError::ZeroSerializedBytes(
                entry.transaction,
            ));
        }
        let source = refinement_source(entry.source);
        let stage = match entry.stage {
            SchedulerRefinementStage::Resolve => SchedulerEntryStage::Resolve,
            SchedulerRefinementStage::Verify(class) => {
                SchedulerEntryStage::Verify(model_verify_class(class))
            }
            SchedulerRefinementStage::Ready => continue,
        };
        entries.push(SchedulerEntry {
            transaction: TxId(entry.transaction),
            version: EntryVersion(entry.version),
            arrival: Arrival(entry.arrival),
            source,
            owner: scheduler_owner(source),
            stage,
            fee: entry.fee,
            bytes: entry.bytes,
        });
    }

    let mut slots = workers.to_vec();
    slots.sort_unstable_by_key(|worker| {
        let rank = match worker.role {
            SchedulerRefinementWorkerRole::OrderedResolve => 0,
            SchedulerRefinementWorkerRole::VerifySmall => 1,
            SchedulerRefinementWorkerRole::VerifyAny => 2,
        };
        (rank, worker.slot)
    });
    let mut worker_ids = BTreeSet::new();
    if let Some(duplicate) = slots
        .iter()
        .map(|worker| worker.slot)
        .find(|slot| !worker_ids.insert(*slot))
    {
        return Err(SchedulerRefinementError::DuplicateWorker(duplicate));
    }
    if slots
        .iter()
        .filter(|worker| worker.role == SchedulerRefinementWorkerRole::OrderedResolve)
        .count()
        > 1
    {
        return Err(SchedulerRefinementError::MultipleOrderedResolvers);
    }

    let mut quotient = SchedulerQuotient {
        verify_order: match order {
            SchedulerRefinementVerifyOrder::Arrival => SchedulerVerifyOrder::Arrival,
            SchedulerRefinementVerifyOrder::FeeRate => SchedulerVerifyOrder::FeeRate,
        },
        resolve_cursor: cursors.resolve.map(model_owner),
        verify_cursor: cursors.verify.map(model_owner),
    };
    let mut assignments = Vec::new();
    let mut idle_slots = Vec::new();
    for worker in slots {
        let role = match worker.role {
            SchedulerRefinementWorkerRole::OrderedResolve => RetainedWorkerRole::OrderedResolve,
            SchedulerRefinementWorkerRole::VerifySmall => {
                RetainedWorkerRole::Verifier(VerifyCapability::SmallCycleOnly)
            }
            SchedulerRefinementWorkerRole::VerifyAny => {
                RetainedWorkerRole::Verifier(VerifyCapability::Any)
            }
        };
        let Some((entry, permit)) = quotient.select_for_slot(&mut entries, role) else {
            idle_slots.push(worker.slot);
            continue;
        };
        assignments.push(SchedulerRefinementAssignment {
            slot: worker.slot,
            transaction: entry.transaction.0,
            owner: refinement_owner(entry.owner),
            permit: refinement_permit(permit),
        });
    }
    Ok(SchedulerRefinementObservation {
        assignments,
        idle_slots,
        cursors: SchedulerRefinementCursors {
            resolve: quotient.resolve_cursor.map(refinement_owner),
            verify: quotient.verify_cursor.map(refinement_owner),
        },
    })
}

const fn refinement_source(source: SchedulerRefinementSource) -> Source {
    match source {
        SchedulerRefinementSource::Remote(peer) => Source::Remote(RemoteResidency {
            peer: PeerId(peer),
            expires_at: RemoteDeadline(u64::MAX),
        }),
        SchedulerRefinementSource::Proposal => Source::Proposal {
            base: ProposalBase::Trusted,
        },
        SchedulerRefinementSource::Recovery => Source::Recovery(PoolGeneration(0)),
    }
}

const fn model_owner(owner: SchedulerRefinementOwner) -> SchedulerOwner {
    match owner {
        SchedulerRefinementOwner::Remote(peer) => SchedulerOwner::Remote(PeerId(peer)),
        SchedulerRefinementOwner::Trusted => SchedulerOwner::Trusted,
    }
}

const fn refinement_owner(owner: SchedulerOwner) -> SchedulerRefinementOwner {
    match owner {
        SchedulerOwner::Remote(peer) => SchedulerRefinementOwner::Remote(peer.0),
        SchedulerOwner::Trusted => SchedulerRefinementOwner::Trusted,
    }
}

const fn refinement_permit(permit: WorkPermit) -> SchedulerRefinementPermit {
    match permit {
        WorkPermit::ResolveOnly => SchedulerRefinementPermit::ResolveOnly,
        WorkPermit::ResolveThenVerify(capability) => {
            SchedulerRefinementPermit::ResolveThenVerify(refinement_capability(capability))
        }
        WorkPermit::VerifyOnly(capability) => {
            SchedulerRefinementPermit::VerifyOnly(refinement_capability(capability))
        }
    }
}

const fn model_verify_class(class: SchedulerRefinementVerifyClass) -> VerifyCycleClass {
    match class {
        SchedulerRefinementVerifyClass::Small => VerifyCycleClass::Small,
        SchedulerRefinementVerifyClass::Large => VerifyCycleClass::Large,
    }
}

const fn refinement_capability(capability: VerifyCapability) -> SchedulerRefinementCapability {
    match capability {
        VerifyCapability::SmallCycleOnly => SchedulerRefinementCapability::SmallOnly,
        VerifyCapability::Any => SchedulerRefinementCapability::Any,
    }
}
