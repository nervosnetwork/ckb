//! The sole owner of every transaction that has not entered [`crate::pool::TxPool`].
//!
//! The primary map owns payloads. Scheduling, waiting, conflict and deadline
//! structures contain identities only and are updated in the same short mutex
//! section. Workers borrow immutable `Arc`s with one process-global revision;
//! queues and notifications never own transaction data.

mod commit;
mod lifecycle;
mod queue;
mod recovery;
mod runtime;
mod stored_entry;
mod wait;

pub(crate) use commit::{ConflictRetention, ExternalCommitPlan, FailedCommitPlan, ReadyCommitPlan};

#[cfg(test)]
#[path = "../tests/pre_pool_test_support.rs"]
mod test_support;

pub(crate) use runtime::{
    PipelineAdmissionSource, PipelineRawTx, PipelineVerifiedTx, PrePool, historical_deadline,
    historical_source, pre_pool_reject,
};

use self::queue::{FairQueue, WorkKey, WorkOwner};
use self::stored_entry::StoredEntry;
use crate::resolved_tx::ResolvedTx;
use ckb_network::PeerIndex;
use ckb_types::prelude::Entity;
use ckb_types::{
    core::{Capacity, Cycle, TransactionView},
    packed::{Byte32, OutPoint, ProposalShortId},
};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;

/// Process-global, non-reused identity for one exact primary state and its
/// derived projections. Every transition that changes the indexed state takes
/// a fresh revision, so stale leases and retired index keys cannot alias a
/// later state of the same full transaction hash.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct EntryRevision(u128);

impl EntryRevision {
    const FIRST: Self = Self(1);

    fn take(cursor: &mut Self) -> Result<Self, PrePoolError> {
        let current = *cursor;
        cursor.0 = cursor
            .0
            .checked_add(1)
            .ok_or(PrePoolError::CounterExhausted)?;
        Ok(current)
    }
}

/// Stable admission order. This is deliberately a different domain from an
/// ownership revision even though both compile to one `u128`.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
struct Arrival(u128);

impl Arrival {
    const FIRST: Self = Self(0);

    fn take(cursor: &mut Self) -> Result<Self, PrePoolError> {
        let current = *cursor;
        cursor.0 = cursor
            .0
            .checked_add(1)
            .ok_or(PrePoolError::CounterExhausted)?;
        Ok(current)
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RemoteSource {
    pub(crate) peer: PeerIndex,
    pub(crate) declared_cycles: Cycle,
}

impl RemoteSource {
    pub(crate) const fn new(peer: PeerIndex, declared_cycles: Cycle) -> Self {
        Self {
            peer,
            declared_cycles,
        }
    }

    pub(crate) const fn tx_source(self) -> crate::tx_source::TxSource {
        crate::tx_source::TxSource::Remote {
            cycles: self.declared_cycles,
            peer: self.peer,
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PrePoolSource {
    Remote(RemoteSource),
    Proposal,
    /// Trusted transaction retained by an authoritative detached-chain
    /// transition. This is deliberately not `TxSource::Local`: local RPC
    /// submissions remain direct and never acquire pre-pool ownership.
    Recovery,
}

impl PrePoolSource {
    fn peer(self) -> Option<PeerIndex> {
        match self {
            Self::Remote(remote) => Some(remote.peer),
            Self::Proposal | Self::Recovery => None,
        }
    }

    fn priority(self) -> u8 {
        match self {
            Self::Remote(_) => 0,
            Self::Proposal => 1,
            Self::Recovery => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) enum ResolveLane {
    Ingress,
    Ordered,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) enum WorkLane {
    Ingress,
    Resolve,
    Verify,
    Commit,
}

/// Exhaustive storage for one value per work lane.
///
/// A raw array turns the enum discriminant into an unchecked runtime index.
/// Named fields plus exhaustive matches make both lane coverage and lookup a
/// compile-time property; adding a lane cannot silently alias or overrun a
/// slot.
#[derive(Debug)]
struct WorkLaneSlots<T> {
    ingress: T,
    resolve: T,
    verify: T,
    commit: T,
}

impl<T> WorkLaneSlots<T> {
    fn from_fn(mut make: impl FnMut(WorkLane) -> T) -> Self {
        Self {
            ingress: make(WorkLane::Ingress),
            resolve: make(WorkLane::Resolve),
            verify: make(WorkLane::Verify),
            commit: make(WorkLane::Commit),
        }
    }

    fn get(&self, lane: WorkLane) -> &T {
        match lane {
            WorkLane::Ingress => &self.ingress,
            WorkLane::Resolve => &self.resolve,
            WorkLane::Verify => &self.verify,
            WorkLane::Commit => &self.commit,
        }
    }

    fn get_mut(&mut self, lane: WorkLane) -> &mut T {
        match lane {
            WorkLane::Ingress => &mut self.ingress,
            WorkLane::Resolve => &mut self.resolve,
            WorkLane::Verify => &mut self.verify,
            WorkLane::Commit => &mut self.commit,
        }
    }

    fn map<U>(&self, mut transform: impl FnMut(&T) -> U) -> WorkLaneSlots<U> {
        WorkLaneSlots {
            ingress: transform(&self.ingress),
            resolve: transform(&self.resolve),
            verify: transform(&self.verify),
            commit: transform(&self.commit),
        }
    }

    fn into_entries(self) -> impl Iterator<Item = (WorkLane, T)> {
        [
            (WorkLane::Ingress, self.ingress),
            (WorkLane::Resolve, self.resolve),
            (WorkLane::Verify, self.verify),
            (WorkLane::Commit, self.commit),
        ]
        .into_iter()
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) enum WorkCapability {
    Any,
    SmallCycleOnly,
}

#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
pub(crate) struct VerifySchedule {
    pub(crate) fee_rate_per_kb: u64,
    pub(crate) is_large_cycle: bool,
}

impl VerifySchedule {
    pub(crate) const fn new(fee_rate_per_kb: u64, is_large_cycle: bool) -> Self {
        Self {
            fee_rate_per_kb,
            is_large_cycle,
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DependencyKey {
    Cell(OutPoint),
    Header(Byte32),
}

impl DependencyKey {
    pub(crate) fn parent_hash(&self) -> Byte32 {
        match self {
            Self::Cell(out_point) => out_point.tx_hash(),
            Self::Header(hash) => hash.clone(),
        }
    }

    fn into_compact(self) -> Self {
        match self {
            Self::Cell(out_point) => Self::Cell(crate::util::compact_packed(&out_point)),
            Self::Header(hash) => Self::Header(crate::util::compact_packed(&hash)),
        }
    }
}

/// Unique parent transaction hashes projected from the already ordered
/// dependency set without allocating a second set.
///
/// Cell dependencies are ordered by `(tx_hash, index)`, so equal cell parents
/// are adjacent. Header dependencies form the second enum range; a logarithmic
/// lookup suppresses the uncommon hash that is present in both ranges.
struct ParentHashes<'a> {
    dependencies: &'a BTreeSet<DependencyKey>,
    iter: std::collections::btree_set::Iter<'a, DependencyKey>,
    last_cell: Option<Byte32>,
}

impl<'a> ParentHashes<'a> {
    fn new(dependencies: &'a BTreeSet<DependencyKey>) -> Self {
        Self {
            dependencies,
            iter: dependencies.iter(),
            last_cell: None,
        }
    }

    fn has_cell_parent(&self, hash: &Byte32) -> bool {
        let lower = DependencyKey::Cell(OutPoint::new(hash.clone(), 0));
        self.dependencies.range(lower..).next().is_some_and(
            |key| matches!(key, DependencyKey::Cell(out_point) if out_point.tx_hash() == *hash),
        )
    }
}

impl Iterator for ParentHashes<'_> {
    type Item = Byte32;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.iter.next()? {
                DependencyKey::Cell(out_point) => {
                    let parent = out_point.tx_hash();
                    if self.last_cell.as_ref() == Some(&parent) {
                        continue;
                    }
                    self.last_cell = Some(parent.clone());
                    return Some(parent);
                }
                DependencyKey::Header(parent) => {
                    if !self.has_cell_parent(parent) {
                        return Some(parent.clone());
                    }
                }
            }
        }
    }
}

fn cell_dependency_keys(
    tx: &ckb_types::core::TransactionView,
    expanded: impl IntoIterator<Item = OutPoint>,
) -> BTreeSet<DependencyKey> {
    tx.input_pts_iter()
        .chain(tx.cell_deps().into_iter().map(|dep| dep.out_point()))
        .chain(expanded)
        .map(|out_point| DependencyKey::Cell(crate::util::compact_packed(&out_point)))
        .collect()
}

pub(crate) fn conflict_dependency_keys(
    tx: &ckb_types::core::TransactionView,
    expanded: impl IntoIterator<Item = OutPoint>,
) -> BTreeSet<DependencyKey> {
    cell_dependency_keys(tx, expanded)
        .into_iter()
        .chain(
            tx.header_deps()
                .into_iter()
                .map(|hash| DependencyKey::Header(crate::util::compact_packed(&hash))),
        )
        .collect()
}

pub(crate) fn available_cell_keys(
    outpoints: impl IntoIterator<Item = OutPoint>,
) -> impl Iterator<Item = DependencyKey> {
    outpoints
        .into_iter()
        .map(|out_point| DependencyKey::Cell(crate::util::compact_packed(&out_point)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WaitReason {
    Missing,
    Conflict,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrePoolLocation {
    ResolveQueued,
    ResolveLeased,
    Wait(WaitReason),
    VerifyQueued,
    VerifyLeased,
    Ready,
}

/// Stable RPC/query projection derived from one primary-entry lookup.
pub(crate) enum PipelineTxLocation {
    Ordered {
        tx: TransactionView,
    },
    Verifying {
        tx: TransactionView,
        fee: Capacity,
        status: crate::component::pool_map::Status,
    },
    Orphan {
        tx: TransactionView,
        cycle: Cycle,
    },
    ConflictHistory,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Residency {
    pub(crate) entries: usize,
    pub(crate) bytes: usize,
}

/// Fully checked counter values for one entry replacement. Apply installs
/// these values verbatim, so budget validation and mutation cannot drift.
struct UsagePlan {
    total: Residency,
    remote: Residency,
    conflict: Residency,
    peer_updates: [Option<(PeerIndex, Residency)>; 2],
}

impl Residency {
    pub(crate) const fn new(entries: usize, bytes: usize) -> Self {
        Self { entries, bytes }
    }

    fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_add(other.entries)?,
            bytes: self.bytes.checked_add(other.bytes)?,
        })
    }

    fn checked_sub(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_sub(other.entries)?,
            bytes: self.bytes.checked_sub(other.bytes)?,
        })
    }

    fn fits(self, limit: Self) -> bool {
        self.entries <= limit.entries && self.bytes <= limit.bytes
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PrePoolLimits {
    pub(crate) total: Residency,
    pub(crate) remote: Residency,
    pub(crate) per_peer: Residency,
    pub(crate) conflict_history: Residency,
    pub(crate) max_dependencies_per_entry: usize,
    pub(crate) max_dependents_per_parent: usize,
    pub(crate) max_inputs_per_ready: usize,
    pub(crate) max_candidates_per_input: usize,
    pub(crate) max_active_work: usize,
    pub(crate) max_active_work_per_peer: usize,
    pub(crate) entry_overhead: usize,
    pub(crate) dependency_overhead: usize,
    pub(crate) verify_fee_rate_ordering: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PrePoolError {
    Public(PrePoolPublicError),
    Duplicate(Byte32),
    Stale(PrePoolStale),
    Fault(PrePoolFault),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PrePoolPublicError {
    Rejected(PrePoolRejection),
    Backpressure(PrePoolBackpressure),
}

/// Closed error domain for a fresh ingress admission.
///
/// Admission owns no lease, so a stale lease/location result or a duplicate
/// discovered after the same-authority primary check is an internal
/// contradiction rather than a transaction outcome. Keeping those cases out
/// of the public variants makes the ingress adapter exhaustively separate
/// peer policy from generation failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PrePoolAdmissionError {
    Public(PrePoolPublicError),
    Fault(PrePoolFault),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PrePoolRejection {
    SelfDependency(Byte32),
    ZeroTransactionSize(Byte32),
    ResidencyChargeOverflow,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PrePoolBackpressure {
    ShortIdCollision {
        short_id: ProposalShortId,
        existing_hash: Byte32,
    },
    DependencyLimitExceeded,
    ParentFanoutLimitExceeded(Byte32),
    ConflictInputLimitExceeded,
    ConflictCandidateLimitExceeded(OutPoint),
    CommitConflictCohortLimitExceeded,
    TotalBudgetExceeded,
    RemoteBudgetExceeded,
    PeerBudgetExceeded(PeerIndex),
    ConflictHistoryBudgetExceeded,
    ActiveWorkLimitExceeded,
    PeerActiveWorkLimitExceeded(PeerIndex),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PrePoolStale {
    Missing(Byte32),
    RevisionMismatch {
        hash: Byte32,
        expected: EntryRevision,
        actual: EntryRevision,
    },
    LocationMismatch {
        hash: Byte32,
        expected: PrePoolLocation,
        actual: PrePoolLocation,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PrePoolFault {
    CounterExhausted,
    PrimaryKeyMismatch { expected: Byte32, actual: Byte32 },
    ProjectionInconsistent(&'static str),
    InvalidConfiguration(&'static str),
    UnexpectedTransitionOutcome(PrePoolPublicError),
    UnexpectedTransitionStale(PrePoolStale),
    UnexpectedTransitionDuplicate(Byte32),
}

impl From<PrePoolError> for PrePoolAdmissionError {
    fn from(error: PrePoolError) -> Self {
        match error {
            PrePoolError::Public(error) => Self::Public(error),
            PrePoolError::Duplicate(hash) => {
                Self::Fault(PrePoolFault::UnexpectedTransitionDuplicate(hash))
            }
            PrePoolError::Stale(reason) => {
                Self::Fault(PrePoolFault::UnexpectedTransitionStale(reason))
            }
            PrePoolError::Fault(fault) => Self::Fault(fault),
        }
    }
}

#[allow(non_snake_case, non_upper_case_globals)]
impl PrePoolError {
    const fn Rejected(reason: PrePoolRejection) -> Self {
        Self::Public(PrePoolPublicError::Rejected(reason))
    }

    const fn Backpressure(reason: PrePoolBackpressure) -> Self {
        Self::Public(PrePoolPublicError::Backpressure(reason))
    }

    fn DuplicateHash(hash: Byte32) -> Self {
        Self::Duplicate(hash)
    }

    fn ShortIdCollision(short_id: ProposalShortId, existing_hash: Byte32) -> Self {
        Self::Backpressure(PrePoolBackpressure::ShortIdCollision {
            short_id,
            existing_hash,
        })
    }

    fn SelfDependency(hash: Byte32) -> Self {
        Self::Rejected(PrePoolRejection::SelfDependency(hash))
    }

    const DependencyLimitExceeded: Self =
        Self::Backpressure(PrePoolBackpressure::DependencyLimitExceeded);

    fn ParentFanoutLimitExceeded(hash: Byte32) -> Self {
        Self::Backpressure(PrePoolBackpressure::ParentFanoutLimitExceeded(hash))
    }

    const ConflictInputLimitExceeded: Self =
        Self::Backpressure(PrePoolBackpressure::ConflictInputLimitExceeded);

    fn ConflictCandidateLimitExceeded(input: OutPoint) -> Self {
        Self::Backpressure(PrePoolBackpressure::ConflictCandidateLimitExceeded(input))
    }

    const CommitConflictCohortLimitExceeded: Self =
        Self::Backpressure(PrePoolBackpressure::CommitConflictCohortLimitExceeded);

    fn ZeroTransactionSize(hash: Byte32) -> Self {
        Self::Rejected(PrePoolRejection::ZeroTransactionSize(hash))
    }

    pub(crate) const ResidencyChargeOverflow: Self =
        Self::Rejected(PrePoolRejection::ResidencyChargeOverflow);
    const TotalBudgetExceeded: Self = Self::Backpressure(PrePoolBackpressure::TotalBudgetExceeded);
    const RemoteBudgetExceeded: Self =
        Self::Backpressure(PrePoolBackpressure::RemoteBudgetExceeded);

    fn PeerBudgetExceeded(peer: PeerIndex) -> Self {
        Self::Backpressure(PrePoolBackpressure::PeerBudgetExceeded(peer))
    }

    const ConflictHistoryBudgetExceeded: Self =
        Self::Backpressure(PrePoolBackpressure::ConflictHistoryBudgetExceeded);
    const ActiveWorkLimitExceeded: Self =
        Self::Backpressure(PrePoolBackpressure::ActiveWorkLimitExceeded);

    fn PeerActiveWorkLimitExceeded(peer: PeerIndex) -> Self {
        Self::Backpressure(PrePoolBackpressure::PeerActiveWorkLimitExceeded(peer))
    }

    pub(crate) fn Missing(hash: Byte32) -> Self {
        Self::Stale(PrePoolStale::Missing(hash))
    }

    fn revision_mismatch(hash: Byte32, expected: EntryRevision, actual: EntryRevision) -> Self {
        Self::Stale(PrePoolStale::RevisionMismatch {
            hash,
            expected,
            actual,
        })
    }

    fn location_mismatch(hash: Byte32, expected: PrePoolLocation, actual: PrePoolLocation) -> Self {
        Self::Stale(PrePoolStale::LocationMismatch {
            hash,
            expected,
            actual,
        })
    }

    const CounterExhausted: Self = Self::Fault(PrePoolFault::CounterExhausted);

    fn primary_key_mismatch(expected: Byte32, actual: Byte32) -> Self {
        Self::Fault(PrePoolFault::PrimaryKeyMismatch { expected, actual })
    }

    pub(crate) fn ProjectionInconsistent(message: &'static str) -> Self {
        Self::Fault(PrePoolFault::ProjectionInconsistent(message))
    }

    fn InvalidConfiguration(message: &'static str) -> Self {
        Self::Fault(PrePoolFault::InvalidConfiguration(message))
    }

    pub(crate) fn is_capacity_rejection(&self) -> bool {
        matches!(self, Self::Public(PrePoolPublicError::Backpressure(_)))
    }

    /// An optional conflict-history projection may be omitted when its own
    /// transaction shape, bounded capacity or duplicate identity cannot be
    /// represented. None of these outcomes may veto the authoritative winner
    /// whose commit caused the history record to be considered.
    pub(crate) fn is_optional_retention_rejection(&self) -> bool {
        matches!(self, Self::Public(_) | Self::Duplicate(_))
    }

    pub(crate) fn is_retryable_capacity_rejection(&self) -> bool {
        matches!(
            self,
            Self::Public(PrePoolPublicError::Backpressure(
                PrePoolBackpressure::ShortIdCollision { .. }
                    | PrePoolBackpressure::TotalBudgetExceeded
                    | PrePoolBackpressure::RemoteBudgetExceeded
                    | PrePoolBackpressure::PeerBudgetExceeded(_)
                    | PrePoolBackpressure::ConflictHistoryBudgetExceeded
                    | PrePoolBackpressure::ActiveWorkLimitExceeded
                    | PrePoolBackpressure::PeerActiveWorkLimitExceeded(_)
            ))
        )
    }

    pub(crate) fn is_stale_lease(&self) -> bool {
        matches!(self, Self::Stale(_))
    }

    /// Convert an error at a boundary that has statically ruled out every
    /// ordinary transaction outcome and stale lease. Callers must make that
    /// proof at the transition site; the returned type cannot be routed back
    /// into peer policy or RPC rejection handling.
    pub(crate) fn into_unexpected_fault(self) -> PrePoolFault {
        match self {
            Self::Public(error) => PrePoolFault::UnexpectedTransitionOutcome(error),
            Self::Duplicate(hash) => PrePoolFault::UnexpectedTransitionDuplicate(hash),
            Self::Stale(stale) => PrePoolFault::UnexpectedTransitionStale(stale),
            Self::Fault(fault) => fault,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ResolveLease {
    pub(crate) hash: Byte32,
    pub(crate) lane: ResolveLane,
    revision: EntryRevision,
    pub(crate) payload: Arc<PipelineRawTx>,
}

#[derive(Clone, Debug)]
pub(crate) struct VerifyLease {
    pub(crate) hash: Byte32,
    revision: EntryRevision,
    /// Sealed at checkout so a completion cannot widen the worker's queue view.
    capability: WorkCapability,
    pub(crate) payload: Arc<ResolvedTx>,
}

/// Proof that stage completion applied before the optional same-lane checkout.
/// A checkout error therefore cannot be mistaken for a completion failure and
/// must never roll back or settle the now-stale completed lease.
pub(crate) struct AppliedContinuation<T>(Result<Option<T>, PrePoolError>);

impl<T> AppliedContinuation<T> {
    fn from_checkout(result: Result<Option<T>, PrePoolError>) -> Self {
        Self(result)
    }

    fn yielded() -> Self {
        Self(Ok(None))
    }

    pub(crate) fn into_checkout(self) -> Result<Option<T>, PrePoolError> {
        self.0
    }
}

struct ReadyCommitCandidate {
    rank: ReadyKey,
    payload: Arc<PipelineVerifiedTx>,
    /// Immutable remote origin captured with the exact Ready revision. A
    /// source promotion may change scheduling priority, but a ban fence that
    /// linearized before final Plan must still revoke this owner.
    ingress_peer: Option<PeerIndex>,
}

/// Exclusive capability for planning the currently selected Ready owner.
///
/// The borrowed kernel is intentionally private. While this value exists,
/// callers can inspect the immutable candidate and build exactly one of the
/// Ready handoff plans, but cannot mutate or select from the authority through
/// another path. A returned plan reborrows the session until Apply or drop.
pub(crate) struct ReadyCommitSession<'authority> {
    authority: &'authority mut PrePoolKernel,
    candidate: ReadyCommitCandidate,
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalRecord {
    pub(crate) hash: Byte32,
    pub(crate) raw: Arc<PipelineRawTx>,
    pub(crate) source: PrePoolSource,
}

#[derive(Clone, Debug)]
pub(crate) struct CommitSettlement {
    pub(crate) winner: TerminalRecord,
    /// Conflict owners superseded by the winner. They normally remain as
    /// bounded conflict history, but are terminalized when that optional
    /// partition is full. These records describe the public outcome; they do
    /// not transfer payload ownership.
    pub(crate) superseded: Vec<TerminalRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReadyKey {
    source_class: u8,
    fee: u64,
    tx_size: usize,
    arrival: Arrival,
    hash: Byte32,
    revision: EntryRevision,
}

impl Ord for ReadyKey {
    fn cmp(&self, other: &Self) -> Ordering {
        let other_size = u64::try_from(other.tx_size).unwrap_or(u64::MAX);
        let self_size = u64::try_from(self.tx_size).unwrap_or(u64::MAX);
        // Both operands are at most u64::MAX, so their product is exact in u128.
        // A u64-by-u64 product is exact in u128. `saturating_mul` keeps that
        // proof explicit to the production arithmetic lint and remains safe
        // if either operand type is widened later.
        let left_rate = u128::from(self.fee).saturating_mul(u128::from(other_size));
        let right_rate = u128::from(other.fee).saturating_mul(u128::from(self_size));
        self.source_class
            .cmp(&other.source_class)
            .then_with(|| left_rate.cmp(&right_rate))
            .then_with(|| self.fee.cmp(&other.fee))
            // `ready.last()` selects the maximum: reverse comparisons make
            // earlier arrivals and smaller hashes the stronger deterministic
            // tie-breakers. Process-global revisions make the order total.
            .then_with(|| other.arrival.cmp(&self.arrival))
            .then_with(|| other.hash.as_slice().cmp(self.hash.as_slice()))
            .then_with(|| self.revision.cmp(&other.revision))
    }
}

impl PartialOrd for ReadyKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug)]
struct ObservedDependencies(BTreeMap<DependencyKey, u128>);

impl ObservedDependencies {
    fn new(values: BTreeMap<DependencyKey, u128>) -> Result<Self, PrePoolError> {
        if values.is_empty() {
            Err(PrePoolError::ProjectionInconsistent(
                "wait state has no observed dependencies",
            ))
        } else {
            Ok(Self(values))
        }
    }
}

impl std::ops::Deref for ObservedDependencies {
    type Target = BTreeMap<DependencyKey, u128>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Clone, Debug)]
struct ReadyInputs(BTreeSet<OutPoint>);

impl ReadyInputs {
    fn new(inputs: BTreeSet<OutPoint>, max: usize) -> Result<Self, PrePoolError> {
        if inputs.is_empty() || inputs.len() > max {
            Err(PrePoolError::ConflictInputLimitExceeded)
        } else {
            Ok(Self(inputs))
        }
    }
}

impl std::ops::Deref for ReadyInputs {
    type Target = BTreeSet<OutPoint>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> IntoIterator for &'a ReadyInputs {
    type Item = &'a OutPoint;
    type IntoIter = std::collections::btree_set::Iter<'a, OutPoint>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[derive(Clone, Debug)]
struct WaitState {
    reason: WaitReason,
    observed: ObservedDependencies,
}

#[derive(Clone, Debug)]
enum EntryState {
    ResolveQueued {
        lane: ResolveLane,
    },
    ResolveLeased,
    Wait(WaitState),
    VerifyQueued {
        payload: Arc<ResolvedTx>,
        schedule: VerifySchedule,
    },
    VerifyLeased {
        payload: Arc<ResolvedTx>,
    },
    Ready {
        payload: Arc<PipelineVerifiedTx>,
        inputs: ReadyInputs,
    },
}

/// Whether a failed/conflicting executable owner remains as bounded conflict
/// history or becomes a definitive dependency loss.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConflictDisposition {
    Retain,
    Terminalize,
}

impl ConflictDisposition {
    fn retains(self) -> bool {
        matches!(self, Self::Retain)
    }
}

impl EntryState {
    fn location(&self) -> PrePoolLocation {
        match self {
            Self::ResolveQueued { .. } => PrePoolLocation::ResolveQueued,
            Self::ResolveLeased => PrePoolLocation::ResolveLeased,
            Self::Wait(wait) => PrePoolLocation::Wait(wait.reason),
            Self::VerifyQueued { .. } => PrePoolLocation::VerifyQueued,
            Self::VerifyLeased { .. } => PrePoolLocation::VerifyLeased,
            Self::Ready { .. } => PrePoolLocation::Ready,
        }
    }
}

#[derive(Clone, Debug)]
pub(in crate::component::pre_pool) struct Entry {
    raw: Arc<PipelineRawTx>,
    source: PrePoolSource,
    state: EntryState,
    revision: EntryRevision,
    arrival: Arrival,
    expires_at: Option<u64>,
    payload_charge_bytes: usize,
    dependencies: BTreeSet<DependencyKey>,
}

impl Entry {
    fn parent_hashes(&self) -> ParentHashes<'_> {
        ParentHashes::new(&self.dependencies)
    }

    fn short_id(&self) -> ProposalShortId {
        self.raw.tx.proposal_short_id()
    }

    fn queued_work(
        &self,
        hash: &Byte32,
        verify_fee_rate_ordering: bool,
    ) -> Option<(WorkLane, WorkKey)> {
        let (lane, schedule) = match &self.state {
            EntryState::VerifyQueued { schedule, .. } => (WorkLane::Verify, *schedule),
            EntryState::ResolveQueued { lane } => (
                match lane {
                    ResolveLane::Ingress => WorkLane::Ingress,
                    ResolveLane::Ordered => WorkLane::Resolve,
                },
                VerifySchedule::default(),
            ),
            _ => return None,
        };
        Some((
            lane,
            WorkKey {
                hash: hash.clone(),
                revision: self.revision,
                source: self.source,
                arrival: self.arrival,
                schedule,
                fee_ordered: lane == WorkLane::Verify && verify_fee_rate_ordering,
            },
        ))
    }

    fn ready_key(&self, hash: &Byte32) -> Option<ReadyKey> {
        let EntryState::Ready { payload, .. } = &self.state else {
            return None;
        };
        Some(self.ready_key_for(hash, payload))
    }

    fn ready_key_for(&self, hash: &Byte32, payload: &PipelineVerifiedTx) -> ReadyKey {
        ReadyKey {
            source_class: self.source.priority(),
            fee: payload.candidate.fee.as_u64(),
            tx_size: payload.candidate.tx_size,
            arrival: self.arrival,
            hash: hash.clone(),
            revision: self.revision,
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
struct WaitEdge {
    hash: Byte32,
    revision: EntryRevision,
}

#[derive(Clone, Debug)]
struct DirtyDependency {
    target_epoch: u128,
    cursor: Option<WaitEdge>,
    pending_epoch: Option<u128>,
}

/// Exact dependency-level publications compiled before a primary mutation.
/// Applying this value is total: every epoch increment and projected waiter
/// predicate has already been checked against the same exclusive authority.
#[derive(Default)]
struct DependencyChangePlan(Vec<(DependencyKey, u128)>);

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
struct DeadlineKey {
    expires_at: u64,
    hash: Byte32,
    revision: EntryRevision,
}

/// Swappable entry/index generation. Process-global clocks, limits and the
/// runtime shell deliberately live outside this value, so a reset cannot
/// reuse an ABA token or replace scheduler/effect authority.
#[derive(Debug)]
pub(crate) struct PrePoolGeneration {
    entries: HashMap<Byte32, StoredEntry>,
    by_short_id: HashMap<ProposalShortId, Byte32>,
    /// Immutable ingress attribution used only for peer revocation. Remote
    /// residency/accounting follows the mutable authoritative `source`
    /// instead, so trusted promotion cannot erase the origin needed by a
    /// later ban or accidentally keep the relayer's known filter pinned.
    by_ingress_peer: HashMap<PeerIndex, BTreeSet<Byte32>>,
    by_parent: HashMap<Byte32, BTreeSet<Byte32>>,
    waiters: HashMap<DependencyKey, BTreeSet<WaitEdge>>,
    availability_epoch: HashMap<DependencyKey, u128>,
    dirty: BTreeMap<DependencyKey, DirtyDependency>,
    dirty_order: VecDeque<DependencyKey>,
    queues: WorkLaneSlots<FairQueue>,
    ready: BTreeSet<ReadyKey>,
    ready_by_input: HashMap<OutPoint, BTreeSet<ReadyKey>>,
    deadlines: BTreeSet<DeadlineKey>,
    total_usage: Residency,
    remote_usage: Residency,
    conflict_usage: Residency,
    peer_usage: HashMap<PeerIndex, Residency>,
    active_work: usize,
    active_by_owner: HashMap<WorkOwner, usize>,
}

impl PrePoolGeneration {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            by_short_id: HashMap::new(),
            by_ingress_peer: HashMap::new(),
            by_parent: HashMap::new(),
            waiters: HashMap::new(),
            availability_epoch: HashMap::new(),
            dirty: BTreeMap::new(),
            dirty_order: VecDeque::new(),
            queues: WorkLaneSlots::from_fn(FairQueue::new),
            ready: BTreeSet::new(),
            ready_by_input: HashMap::new(),
            deadlines: BTreeSet::new(),
            total_usage: Residency::default(),
            remote_usage: Residency::default(),
            conflict_usage: Residency::default(),
            peer_usage: HashMap::new(),
            active_work: 0,
            active_by_owner: HashMap::new(),
        }
    }
}

/// Concrete primary owner and exact derived projections. No method awaits or
/// acquires `TxPool`; callers establish the universal `TxPool -> kernel`
/// order around cross-authority commands.
#[derive(Debug)]
pub(crate) struct PrePoolKernel {
    generation: PrePoolGeneration,
    limits: PrePoolLimits,
    next_revision: EntryRevision,
    next_arrival: Arrival,
}

impl std::ops::Deref for PrePoolKernel {
    type Target = PrePoolGeneration;

    fn deref(&self) -> &Self::Target {
        &self.generation
    }
}

impl std::ops::DerefMut for PrePoolKernel {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.generation
    }
}

impl PrePoolKernel {
    pub(crate) fn new(limits: PrePoolLimits) -> Self {
        Self {
            generation: PrePoolGeneration::new(),
            limits,
            next_revision: EntryRevision::FIRST,
            next_arrival: Arrival::FIRST,
        }
    }

    fn lane_for_resolve(lane: ResolveLane) -> WorkLane {
        match lane {
            ResolveLane::Ingress => WorkLane::Ingress,
            ResolveLane::Ordered => WorkLane::Resolve,
        }
    }

    fn is_conflict(entry: &Entry) -> bool {
        matches!(
            entry.state,
            EntryState::Wait(WaitState {
                reason: WaitReason::Conflict,
                ..
            })
        )
    }

    fn plan_usage_delta(
        &self,
        old: Option<&StoredEntry>,
        new: Option<&StoredEntry>,
    ) -> Result<UsagePlan, PrePoolError> {
        let old_charge = old.map_or(Residency::default(), |entry| {
            Residency::new(1, entry.charge_bytes())
        });
        let new_charge = new.map_or(Residency::default(), |entry| {
            Residency::new(1, entry.charge_bytes())
        });
        let total = self
            .total_usage
            .checked_sub(old_charge)
            .and_then(|usage| usage.checked_add(new_charge))
            .ok_or(PrePoolError::ProjectionInconsistent(
                "total usage does not match primary ownership",
            ))?;
        if !total.fits(self.limits.total) {
            return Err(PrePoolError::TotalBudgetExceeded);
        }

        let old_peer = old.and_then(|entry| entry.source.peer());
        let new_peer = new.and_then(|entry| entry.source.peer());
        let mut remote = self.remote_usage;
        if old_peer.is_some() {
            remote = remote
                .checked_sub(old_charge)
                .ok_or(PrePoolError::ProjectionInconsistent(
                    "remote usage does not match primary ownership",
                ))?;
        }
        if new_peer.is_some() {
            remote = remote
                .checked_add(new_charge)
                .ok_or(PrePoolError::RemoteBudgetExceeded)?;
        }
        if !remote.fits(self.limits.remote) {
            return Err(PrePoolError::RemoteBudgetExceeded);
        }

        let mut conflict = self.conflict_usage;
        if old.is_some_and(|entry| Self::is_conflict(entry)) {
            conflict =
                conflict
                    .checked_sub(old_charge)
                    .ok_or(PrePoolError::ProjectionInconsistent(
                        "conflict usage does not match primary ownership",
                    ))?;
        }
        if new.is_some_and(|entry| Self::is_conflict(entry)) {
            conflict = conflict
                .checked_add(new_charge)
                .ok_or(PrePoolError::ConflictHistoryBudgetExceeded)?;
        }
        if !conflict.fits(self.limits.conflict_history) {
            return Err(PrePoolError::ConflictHistoryBudgetExceeded);
        }

        let project_peer = |peer| {
            let mut usage = self.peer_usage.get(&peer).copied().unwrap_or_default();
            if old_peer == Some(peer) {
                usage =
                    usage
                        .checked_sub(old_charge)
                        .ok_or(PrePoolError::ProjectionInconsistent(
                            "peer usage does not match primary ownership",
                        ))?;
            }
            if new_peer == Some(peer) {
                usage = usage
                    .checked_add(new_charge)
                    .ok_or(PrePoolError::PeerBudgetExceeded(peer))?;
            }
            if !usage.fits(self.limits.per_peer) {
                return Err(PrePoolError::PeerBudgetExceeded(peer));
            }
            Ok(usage)
        };
        let old_update = old_peer
            .map(|peer| project_peer(peer).map(|usage| (peer, usage)))
            .transpose()?;
        let new_update = new_peer
            .filter(|peer| Some(*peer) != old_peer)
            .map(|peer| project_peer(peer).map(|usage| (peer, usage)))
            .transpose()?;
        Ok(UsagePlan {
            total,
            remote,
            conflict,
            peer_updates: [old_update, new_update],
        })
    }

    fn apply_usage_plan(&mut self, plan: UsagePlan) {
        self.total_usage = plan.total;
        self.remote_usage = plan.remote;
        self.conflict_usage = plan.conflict;
        for (peer, usage) in plan.peer_updates.into_iter().flatten() {
            if usage == Residency::default() {
                self.peer_usage.remove(&peer);
            } else {
                self.peer_usage.insert(peer, usage);
            }
        }
    }

    pub(crate) fn ingress_peer_hashes(&self, peer: PeerIndex, max: usize) -> Vec<Byte32> {
        self.by_ingress_peer
            .get(&peer)
            .into_iter()
            .flatten()
            .take(max)
            .cloned()
            .collect()
    }

    pub(crate) fn source_by_hash(&self, hash: &Byte32) -> Option<PrePoolSource> {
        self.entries.get(hash).map(|entry| entry.source)
    }

    pub(crate) fn contains_hash(&self, hash: &Byte32) -> bool {
        self.entries.contains_key(hash)
    }

    /// Reuse the primary entry's canonical cell graph when a worker reports
    /// the provider's first missing edge. This avoids rebuilding direct
    /// dependencies from the raw transaction and retains expanded dep-group
    /// members discovered by an earlier resolution generation.
    pub(crate) fn cell_dependency_frontier(
        &self,
        hash: &Byte32,
        discovered: impl IntoIterator<Item = DependencyKey>,
    ) -> Option<BTreeSet<DependencyKey>> {
        let entry = self.entries.get(hash)?;
        Some(
            entry
                .dependencies
                .iter()
                .filter(|key| matches!(key, DependencyKey::Cell(_)))
                .cloned()
                .chain(discovered.into_iter().map(DependencyKey::into_compact))
                .collect(),
        )
    }

    pub(crate) fn raw_by_hash(&self, hash: &Byte32) -> Option<Arc<PipelineRawTx>> {
        self.entries.get(hash).map(|entry| Arc::clone(&entry.raw))
    }

    pub(crate) fn raw_by_short_id(&self, short_id: &ProposalShortId) -> Option<Arc<PipelineRawTx>> {
        self.by_short_id
            .get(short_id)
            .and_then(|hash| self.raw_by_hash(hash))
    }

    pub(crate) fn tx_location_by_hash(&self, hash: &Byte32) -> Option<PipelineTxLocation> {
        let entry = self.entries.get(hash)?;
        let tx = entry.raw.tx.clone();
        Some(match &entry.state {
            EntryState::Wait(WaitState {
                reason: WaitReason::Missing,
                ..
            }) => PipelineTxLocation::Orphan {
                tx,
                cycle: entry.raw.declared_cycles.unwrap_or(0),
            },
            EntryState::VerifyQueued { payload, .. } | EntryState::VerifyLeased { payload } => {
                PipelineTxLocation::Verifying {
                    tx,
                    fee: payload.fee,
                    status: payload.status,
                }
            }
            EntryState::Ready { payload, .. } => PipelineTxLocation::Verifying {
                tx,
                fee: payload.candidate.fee,
                status: payload.candidate.status,
            },
            EntryState::Wait(WaitState {
                reason: WaitReason::Conflict,
                ..
            }) => PipelineTxLocation::ConflictHistory,
            EntryState::ResolveQueued { .. } | EntryState::ResolveLeased => {
                PipelineTxLocation::Ordered { tx }
            }
        })
    }

    pub(crate) fn hash_by_short_id(&self, short_id: &ProposalShortId) -> Option<&Byte32> {
        self.by_short_id.get(short_id)
    }

    pub(crate) fn queue_len(&self, lane: WorkLane) -> usize {
        match lane {
            WorkLane::Commit => self.ready.len(),
            _ => self.queues.get(lane).len(),
        }
    }

    pub(crate) fn waiting_parent_len(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| {
                matches!(
                    entry.state,
                    EntryState::Wait(WaitState {
                        reason: WaitReason::Missing,
                        ..
                    })
                )
            })
            .count()
    }

    pub(crate) fn conflict_hashes(&self) -> Vec<Byte32> {
        self.entries
            .iter()
            .filter(|(_, entry)| {
                matches!(
                    entry.state,
                    EntryState::Wait(WaitState {
                        reason: WaitReason::Conflict,
                        ..
                    })
                )
            })
            .map(|(hash, _)| hash.clone())
            .collect()
    }
}
