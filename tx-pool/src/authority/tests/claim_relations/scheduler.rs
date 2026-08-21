//! Finite scheduler relation for production worker-wave property tests.

use std::{cmp::Ordering, collections::BTreeSet};

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
    Ready,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SchedulerRefinementVerifyClass {
    Small,
    Large,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SchedulerRefinementCapability {
    SmallOnly,
    Any,
}

impl SchedulerRefinementCapability {
    const fn permits(self, class: SchedulerRefinementVerifyClass) -> bool {
        matches!(self, Self::Any) || matches!(class, SchedulerRefinementVerifyClass::Small)
    }
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

#[derive(Clone, Copy)]
enum Lane {
    Resolve,
    Verify,
}

pub(crate) fn scheduler_wave_observation(
    input: &[SchedulerRefinementEntry],
    workers: &[SchedulerRefinementWorker],
    mut cursors: SchedulerRefinementCursors,
    order: SchedulerRefinementVerifyOrder,
) -> Result<SchedulerRefinementObservation, SchedulerRefinementError> {
    let mut transactions = BTreeSet::new();
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
    }
    let mut remaining = input
        .iter()
        .copied()
        .filter(|entry| !matches!(entry.stage, SchedulerRefinementStage::Ready))
        .collect::<Vec<_>>();
    let mut slots = workers.to_vec();
    slots.sort_unstable_by_key(|worker| (worker_rank(worker.role), worker.slot));
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
        .filter(|worker| matches!(worker.role, SchedulerRefinementWorkerRole::OrderedResolve))
        .count()
        > 1
    {
        return Err(SchedulerRefinementError::MultipleOrderedResolvers);
    }

    let mut assignments = Vec::new();
    let mut idle_slots = Vec::new();
    for worker in slots {
        let capability = match worker.role {
            SchedulerRefinementWorkerRole::VerifySmall => SchedulerRefinementCapability::SmallOnly,
            SchedulerRefinementWorkerRole::OrderedResolve
            | SchedulerRefinementWorkerRole::VerifyAny => SchedulerRefinementCapability::Any,
        };
        let selected = match worker.role {
            SchedulerRefinementWorkerRole::OrderedResolve => select(
                &mut remaining,
                Lane::Resolve,
                capability,
                order,
                &mut cursors,
            ),
            SchedulerRefinementWorkerRole::VerifySmall
            | SchedulerRefinementWorkerRole::VerifyAny => select(
                &mut remaining,
                Lane::Verify,
                capability,
                order,
                &mut cursors,
            )
            .or_else(|| {
                select(
                    &mut remaining,
                    Lane::Resolve,
                    capability,
                    order,
                    &mut cursors,
                )
            }),
        };
        let Some((entry, lane)) = selected else {
            idle_slots.push(worker.slot);
            continue;
        };
        let permit = match (worker.role, lane) {
            (SchedulerRefinementWorkerRole::OrderedResolve, Lane::Resolve) => {
                SchedulerRefinementPermit::ResolveOnly
            }
            (SchedulerRefinementWorkerRole::VerifySmall, Lane::Resolve) => {
                SchedulerRefinementPermit::ResolveThenVerify(
                    SchedulerRefinementCapability::SmallOnly,
                )
            }
            (SchedulerRefinementWorkerRole::VerifyAny, Lane::Resolve) => {
                SchedulerRefinementPermit::ResolveThenVerify(SchedulerRefinementCapability::Any)
            }
            (SchedulerRefinementWorkerRole::VerifySmall, Lane::Verify) => {
                SchedulerRefinementPermit::VerifyOnly(SchedulerRefinementCapability::SmallOnly)
            }
            (SchedulerRefinementWorkerRole::VerifyAny, Lane::Verify) => {
                SchedulerRefinementPermit::VerifyOnly(SchedulerRefinementCapability::Any)
            }
            (SchedulerRefinementWorkerRole::OrderedResolve, Lane::Verify) => unreachable!(),
        };
        assignments.push(SchedulerRefinementAssignment {
            slot: worker.slot,
            transaction: entry.transaction,
            owner: owner(entry.source),
            permit,
        });
    }
    Ok(SchedulerRefinementObservation {
        assignments,
        idle_slots,
        cursors,
    })
}

fn select(
    entries: &mut Vec<SchedulerRefinementEntry>,
    lane: Lane,
    capability: SchedulerRefinementCapability,
    order: SchedulerRefinementVerifyOrder,
    cursors: &mut SchedulerRefinementCursors,
) -> Option<(SchedulerRefinementEntry, Lane)> {
    let owners = entries
        .iter()
        .filter(|entry| eligible(entry, lane, capability))
        .map(|entry| owner(entry.source))
        .collect::<BTreeSet<_>>();
    let cursor = match lane {
        Lane::Resolve => cursors.resolve,
        Lane::Verify => cursors.verify,
    };
    let selected_owner = next_owner(&owners, cursor)?;
    let index = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            owner(entry.source) == selected_owner && eligible(entry, lane, capability)
        })
        .min_by(|(_, left), (_, right)| match lane {
            Lane::Resolve => resolve_order(left, right),
            Lane::Verify => verify_order(order, right, left),
        })
        .map(|(index, _)| index)?;
    let entry = entries.remove(index);
    match lane {
        Lane::Resolve => cursors.resolve = Some(selected_owner),
        Lane::Verify => cursors.verify = Some(selected_owner),
    }
    Some((entry, lane))
}

fn eligible(
    entry: &SchedulerRefinementEntry,
    lane: Lane,
    capability: SchedulerRefinementCapability,
) -> bool {
    match (lane, entry.stage) {
        (Lane::Resolve, SchedulerRefinementStage::Resolve) => true,
        (Lane::Verify, SchedulerRefinementStage::Verify(class)) => capability.permits(class),
        _ => false,
    }
}

fn next_owner(
    owners: &BTreeSet<SchedulerRefinementOwner>,
    cursor: Option<SchedulerRefinementOwner>,
) -> Option<SchedulerRefinementOwner> {
    if cursor.is_none() && owners.contains(&SchedulerRefinementOwner::Trusted) {
        return Some(SchedulerRefinementOwner::Trusted);
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

const fn worker_rank(role: SchedulerRefinementWorkerRole) -> u8 {
    match role {
        SchedulerRefinementWorkerRole::OrderedResolve => 0,
        SchedulerRefinementWorkerRole::VerifySmall => 1,
        SchedulerRefinementWorkerRole::VerifyAny => 2,
    }
}

const fn source_rank(source: SchedulerRefinementSource) -> u8 {
    match source {
        SchedulerRefinementSource::Remote(_) => 0,
        SchedulerRefinementSource::Proposal => 1,
        SchedulerRefinementSource::Recovery => 2,
    }
}

const fn owner(source: SchedulerRefinementSource) -> SchedulerRefinementOwner {
    match source {
        SchedulerRefinementSource::Remote(peer) => SchedulerRefinementOwner::Remote(peer),
        SchedulerRefinementSource::Proposal | SchedulerRefinementSource::Recovery => {
            SchedulerRefinementOwner::Trusted
        }
    }
}

fn resolve_order(left: &SchedulerRefinementEntry, right: &SchedulerRefinementEntry) -> Ordering {
    source_rank(right.source)
        .cmp(&source_rank(left.source))
        .then_with(|| left.arrival.cmp(&right.arrival))
        .then_with(|| left.transaction.cmp(&right.transaction))
        .then_with(|| left.version.cmp(&right.version))
}

fn verify_order(
    order: SchedulerRefinementVerifyOrder,
    left: &SchedulerRefinementEntry,
    right: &SchedulerRefinementEntry,
) -> Ordering {
    source_rank(left.source)
        .cmp(&source_rank(right.source))
        .then_with(|| match order {
            SchedulerRefinementVerifyOrder::Arrival => Ordering::Equal,
            SchedulerRefinementVerifyOrder::FeeRate => {
                let left_rate = u128::from(left.fee) * u128::from(right.bytes);
                let right_rate = u128::from(right.fee) * u128::from(left.bytes);
                left_rate
                    .cmp(&right_rate)
                    .then_with(|| left.fee.cmp(&right.fee))
            }
        })
        .then_with(|| right.arrival.cmp(&left.arrival))
        .then_with(|| right.transaction.cmp(&left.transaction))
        .then_with(|| left.version.cmp(&right.version))
        .then_with(|| verify_class(left).cmp(&verify_class(right)))
}

fn verify_class(entry: &SchedulerRefinementEntry) -> Option<SchedulerRefinementVerifyClass> {
    match entry.stage {
        SchedulerRefinementStage::Verify(class) => Some(class),
        SchedulerRefinementStage::Resolve | SchedulerRefinementStage::Ready => None,
    }
}
