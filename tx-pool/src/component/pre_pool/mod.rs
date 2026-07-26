//! The sole owner of every transaction that has not entered [`crate::pool::TxPool`].
//!
//! The primary map owns payloads. Scheduling, waiting, conflict and deadline
//! structures contain identities only and are updated in the same short mutex
//! section. Workers borrow immutable `Arc`s with one process-global version;
//! queues and notifications never own transaction data.

mod commit;
mod lifecycle;
mod queue;
mod recovery;
mod runtime;
mod wait;

#[cfg(test)]
#[path = "../tests/pre_pool_seam.rs"]
mod test_seam;

#[cfg(test)]
pub(crate) use runtime::pre_pool_source;
pub(crate) use runtime::{
    KernelDisposal, PipelineRawTx, PipelineVerifiedTx, PrePool, historical_deadline,
    historical_source, pre_pool_reject,
};

use self::queue::{FairQueue, WorkKey, WorkOwner};
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

pub(crate) type EntryVersion = u128;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PrePoolSource {
    Remote(PeerIndex),
    Proposal,
    /// Trusted transaction retained by an authoritative detached-chain
    /// transition. This is deliberately not `TxSource::Local`: local RPC
    /// submissions remain direct and never acquire pre-pool ownership.
    Recovery,
}

impl PrePoolSource {
    fn peer(self) -> Option<PeerIndex> {
        match self {
            Self::Remote(peer) => Some(peer),
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

impl WorkLane {
    const ALL: [Self; 4] = [Self::Ingress, Self::Resolve, Self::Verify, Self::Commit];

    const fn index(self) -> usize {
        self as usize
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

pub(crate) fn conflict_dependency_keys(
    tx: &ckb_types::core::TransactionView,
    expanded: impl IntoIterator<Item = OutPoint>,
) -> BTreeSet<DependencyKey> {
    tx.input_pts_iter()
        .chain(tx.cell_deps().into_iter().map(|dep| dep.out_point()))
        .chain(expanded)
        .map(|out_point| DependencyKey::Cell(crate::util::compact_packed(&out_point)))
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
    DuplicateHash(Byte32),
    ShortIdCollision {
        short_id: ProposalShortId,
        existing_hash: Byte32,
    },
    SelfDependency(Byte32),
    DependencyLimitExceeded,
    ParentFanoutLimitExceeded(Byte32),
    ConflictInputLimitExceeded,
    ConflictCandidateLimitExceeded(OutPoint),
    ZeroTransactionSize(Byte32),
    UnderReplacementFee {
        hash: Byte32,
        required: u64,
        actual: u64,
    },
    UnderFeeRate {
        hash: Byte32,
        required_per_kb: u64,
    },
    FeeRateOverflow,
    ResidencyChargeOverflow,
    TotalBudgetExceeded,
    RemoteBudgetExceeded,
    PeerBudgetExceeded(PeerIndex),
    ConflictHistoryBudgetExceeded,
    ActiveWorkLimitExceeded,
    PeerActiveWorkLimitExceeded(PeerIndex),
    Missing(Byte32),
    Stale {
        hash: Byte32,
        expected: EntryVersion,
        actual: EntryVersion,
    },
    LocationMismatch {
        hash: Byte32,
        expected: PrePoolLocation,
        actual: PrePoolLocation,
    },
}

#[derive(Clone, Copy)]
enum ErrorClass {
    Transaction,
    Capacity,
    RetryableCapacity,
    Stale,
    Duplicate,
}

impl PrePoolError {
    fn class(&self) -> ErrorClass {
        match self {
            Self::SelfDependency(_)
            | Self::ZeroTransactionSize(_)
            | Self::UnderReplacementFee { .. }
            | Self::UnderFeeRate { .. }
            | Self::FeeRateOverflow
            | Self::ResidencyChargeOverflow => ErrorClass::Transaction,
            Self::DependencyLimitExceeded
            | Self::ParentFanoutLimitExceeded(_)
            | Self::ConflictInputLimitExceeded
            | Self::ConflictCandidateLimitExceeded(_) => ErrorClass::Capacity,
            Self::ShortIdCollision { .. }
            | Self::TotalBudgetExceeded
            | Self::RemoteBudgetExceeded
            | Self::PeerBudgetExceeded(_)
            | Self::ConflictHistoryBudgetExceeded
            | Self::ActiveWorkLimitExceeded
            | Self::PeerActiveWorkLimitExceeded(_) => ErrorClass::RetryableCapacity,
            Self::Missing(_) | Self::Stale { .. } | Self::LocationMismatch { .. } => {
                ErrorClass::Stale
            }
            Self::DuplicateHash(_) => ErrorClass::Duplicate,
        }
    }

    pub(crate) fn is_transaction_rejection(&self) -> bool {
        matches!(
            self.class(),
            ErrorClass::Transaction | ErrorClass::Capacity | ErrorClass::RetryableCapacity
        )
    }

    pub(crate) fn is_capacity_rejection(&self) -> bool {
        matches!(
            self.class(),
            ErrorClass::Capacity | ErrorClass::RetryableCapacity
        )
    }

    pub(crate) fn is_retryable_capacity_rejection(&self) -> bool {
        matches!(self.class(), ErrorClass::RetryableCapacity)
    }

    pub(crate) fn is_stale_lease(&self) -> bool {
        matches!(self.class(), ErrorClass::Stale)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct VerifiedCandidate {
    inputs: BTreeSet<OutPoint>,
    fee: u64,
    tx_size: usize,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FeeGate {
    required_replacement_fee: u64,
    min_fee_rate_per_kb: u64,
}

impl FeeGate {
    pub(crate) const fn new(required_replacement_fee: u64, min_fee_rate_per_kb: u64) -> Self {
        Self {
            required_replacement_fee,
            min_fee_rate_per_kb,
        }
    }

    pub(crate) fn validate(
        self,
        hash: Byte32,
        inputs: HashSet<OutPoint>,
        fee: u64,
        tx_size: usize,
    ) -> Result<VerifiedCandidate, PrePoolError> {
        if inputs.is_empty() || tx_size == 0 {
            return Err(PrePoolError::ZeroTransactionSize(hash));
        }
        if fee < self.required_replacement_fee {
            return Err(PrePoolError::UnderReplacementFee {
                hash,
                required: self.required_replacement_fee,
                actual: fee,
            });
        }
        let actual = u128::from(fee) * 1_000;
        let required = u128::from(self.min_fee_rate_per_kb)
            .checked_mul(u128::try_from(tx_size).map_err(|_| PrePoolError::FeeRateOverflow)?)
            .ok_or(PrePoolError::FeeRateOverflow)?;
        if actual < required {
            return Err(PrePoolError::UnderFeeRate {
                hash,
                required_per_kb: self.min_fee_rate_per_kb,
            });
        }
        Ok(VerifiedCandidate {
            inputs: inputs
                .into_iter()
                .map(|input| crate::util::compact_packed(&input))
                .collect(),
            fee,
            tx_size,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ResolveLease {
    pub(crate) hash: Byte32,
    pub(crate) lane: ResolveLane,
    pub(crate) version: EntryVersion,
    pub(crate) payload: Arc<PipelineRawTx>,
}

#[derive(Clone, Debug)]
pub(crate) struct VerifyLease {
    pub(crate) hash: Byte32,
    pub(crate) version: EntryVersion,
    pub(crate) payload: Arc<ResolvedTx>,
}

#[derive(Clone, Debug)]
pub(crate) struct CommitTicket {
    pub(crate) hash: Byte32,
    pub(crate) version: EntryVersion,
    pub(crate) rank: ReadyKey,
    pub(crate) payload: Arc<PipelineVerifiedTx>,
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
    arrival: u128,
    hash: Byte32,
    version: EntryVersion,
}

impl Ord for ReadyKey {
    fn cmp(&self, other: &Self) -> Ordering {
        let left_rate = u128::from(self.fee) * other.tx_size as u128;
        let right_rate = u128::from(other.fee) * self.tx_size as u128;
        self.source_class
            .cmp(&other.source_class)
            .then_with(|| left_rate.cmp(&right_rate))
            .then_with(|| self.fee.cmp(&other.fee))
            .then_with(|| other.arrival.cmp(&self.arrival))
            .then_with(|| other.hash.as_slice().cmp(self.hash.as_slice()))
            .then_with(|| self.version.cmp(&other.version))
            .then_with(|| self.tx_size.cmp(&other.tx_size))
    }
}

impl PartialOrd for ReadyKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug)]
struct WaitState {
    reason: WaitReason,
    observed: BTreeMap<DependencyKey, u128>,
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
        inputs: BTreeSet<OutPoint>,
        rank: ReadyKey,
    },
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
    short_id: ProposalShortId,
    raw: Arc<PipelineRawTx>,
    source: PrePoolSource,
    state: EntryState,
    version: EntryVersion,
    arrival: u128,
    expires_at: Option<u64>,
    payload_charge_bytes: usize,
    charge_bytes: usize,
    dependencies: BTreeSet<DependencyKey>,
}

impl Entry {
    fn work_key(&self, hash: &Byte32, verify_fee_rate_ordering: bool) -> Option<WorkKey> {
        let schedule = match &self.state {
            EntryState::VerifyQueued { schedule, .. } => *schedule,
            EntryState::ResolveQueued { .. } => VerifySchedule::default(),
            _ => return None,
        };
        Some(WorkKey {
            hash: hash.clone(),
            version: self.version,
            source: self.source,
            arrival: self.arrival,
            schedule,
            fee_ordered: matches!(&self.state, EntryState::VerifyQueued { .. })
                && verify_fee_rate_ordering,
        })
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
struct WaitEdge {
    hash: Byte32,
    version: EntryVersion,
}

#[derive(Clone, Debug)]
struct DirtyDependency {
    target_epoch: u128,
    cursor: Option<WaitEdge>,
    pending_epoch: Option<u128>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
struct DeadlineKey {
    expires_at: u64,
    hash: Byte32,
    version: EntryVersion,
}

/// Swappable entry/index generation. Process-global clocks, limits and the
/// runtime shell deliberately live outside this value, so a reset cannot
/// reuse an ABA token or replace scheduler/effect authority.
#[derive(Debug)]
pub(crate) struct PrePoolGeneration {
    entries: HashMap<Byte32, Entry>,
    by_short_id: HashMap<ProposalShortId, Byte32>,
    by_peer: HashMap<PeerIndex, BTreeSet<Byte32>>,
    by_parent: HashMap<Byte32, BTreeSet<Byte32>>,
    waiters: HashMap<DependencyKey, BTreeSet<WaitEdge>>,
    availability_epoch: HashMap<DependencyKey, u128>,
    dirty: BTreeMap<DependencyKey, DirtyDependency>,
    dirty_order: VecDeque<DependencyKey>,
    queues: [FairQueue; 4],
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
            by_peer: HashMap::new(),
            by_parent: HashMap::new(),
            waiters: HashMap::new(),
            availability_epoch: HashMap::new(),
            dirty: BTreeMap::new(),
            dirty_order: VecDeque::new(),
            queues: WorkLane::ALL.map(FairQueue::new),
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
    next_version: EntryVersion,
    next_arrival: u128,
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
            next_version: 1,
            next_arrival: 0,
        }
    }

    fn allocate_version(&mut self) -> EntryVersion {
        let version = self.next_version;
        self.next_version = self
            .next_version
            .checked_add(1)
            .expect("u128 entry version must not exhaust during process lifetime");
        version
    }

    fn allocate_arrival(&mut self) -> u128 {
        let arrival = self.next_arrival;
        self.next_arrival = self
            .next_arrival
            .checked_add(1)
            .expect("u128 arrival clock must not exhaust during process lifetime");
        arrival
    }

    fn lane_for_resolve(lane: ResolveLane) -> WorkLane {
        match lane {
            ResolveLane::Ingress => WorkLane::Ingress,
            ResolveLane::Ordered => WorkLane::Resolve,
        }
    }

    fn index_memberships(entry: &Entry) -> Result<usize, PrePoolError> {
        // Count a conservative bucket/member pair for every hash projection.
        // State-local ordered-set keys are counted separately. This is a
        // closed function of the primary entry, so moving between states
        // cannot silently add uncharged derived storage.
        let mut memberships = 2usize; // full-hash owner -> short-ID projection
        if entry.source.peer().is_some() {
            memberships = memberships
                .checked_add(2)
                .ok_or(PrePoolError::ResidencyChargeOverflow)?;
        }
        let parent_count = entry
            .dependencies
            .iter()
            .map(DependencyKey::parent_hash)
            .collect::<BTreeSet<_>>()
            .len();
        memberships = memberships
            .checked_add(entry.dependencies.len())
            .and_then(|value| value.checked_add(parent_count.checked_mul(2)?))
            .and_then(|value| value.checked_add(usize::from(entry.expires_at.is_some())))
            .ok_or(PrePoolError::ResidencyChargeOverflow)?;
        let current_state_memberships = match &entry.state {
            EntryState::ResolveLeased | EntryState::VerifyLeased { .. } => 0,
            // Work key, owner bucket and runnable head in the worst case.
            EntryState::ResolveQueued { .. } => 3,
            // Verify additionally projects the best small-cycle key so a
            // large-cycle owner head cannot hide eligible reserved work.
            EntryState::VerifyQueued { .. } => 4,
            // Exact waiter edge, dependency bucket, availability epoch and
            // dirty cursor/order reservation. The latter two are transient
            // but may appear without another entry transition.
            EntryState::Wait(wait) => wait
                .observed
                .len()
                .checked_mul(4)
                .map(|memberships| memberships.max(3))
                .ok_or(PrePoolError::ResidencyChargeOverflow)?,
            // One global Ready rank plus a bucket/rank pair per input.
            EntryState::Ready { inputs, .. } => inputs
                .len()
                .checked_mul(2)
                .and_then(|value| value.checked_add(1))
                .ok_or(PrePoolError::ResidencyChargeOverflow)?,
        };
        // Every live owner reserves the exact dependency-wait projection it
        // can reach from its currently retained payload. Parent invalidation
        // is therefore a non-growing transition even when every other byte of
        // the partition is occupied; it cannot turn a legal chain/pool event
        // into a capacity-triggered structural failure.
        let wait_reservation = Self::causal_keys(entry)
            .len()
            .checked_mul(4)
            .map(|memberships| memberships.max(3))
            .ok_or(PrePoolError::ResidencyChargeOverflow)?;
        let state_memberships = current_state_memberships.max(wait_reservation);
        memberships
            .checked_add(state_memberships)
            .ok_or(PrePoolError::ResidencyChargeOverflow)
    }

    fn entry_charge(&self, entry: &Entry) -> Result<usize, PrePoolError> {
        Self::index_memberships(entry)?
            .checked_mul(self.limits.dependency_overhead)
            .and_then(|metadata| metadata.checked_add(self.limits.entry_overhead))
            .and_then(|metadata| metadata.checked_add(entry.payload_charge_bytes))
            .ok_or(PrePoolError::ResidencyChargeOverflow)
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
        old: Option<&Entry>,
        new: Option<&Entry>,
    ) -> Result<UsagePlan, PrePoolError> {
        let old_charge = old.map_or(Residency::default(), |entry| {
            Residency::new(1, entry.charge_bytes)
        });
        let new_charge = new.map_or(Residency::default(), |entry| {
            Residency::new(1, entry.charge_bytes)
        });
        let total = self
            .total_usage
            .checked_sub(old_charge)
            .and_then(|usage| usage.checked_add(new_charge))
            .expect("total usage is derived from the primary map");
        if !total.fits(self.limits.total) {
            return Err(PrePoolError::TotalBudgetExceeded);
        }

        let old_peer = old.and_then(|entry| entry.source.peer());
        let new_peer = new.and_then(|entry| entry.source.peer());
        let mut remote = self.remote_usage;
        if old_peer.is_some() {
            remote = remote
                .checked_sub(old_charge)
                .expect("remote usage is derived from the primary map");
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
        if old.is_some_and(Self::is_conflict) {
            conflict = conflict
                .checked_sub(old_charge)
                .expect("conflict usage is derived from the primary map");
        }
        if new.is_some_and(Self::is_conflict) {
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
                usage = usage
                    .checked_sub(old_charge)
                    .expect("peer usage is derived from the primary map");
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

    pub(crate) fn peer_hashes(&self, peer: PeerIndex, max: usize) -> Vec<Byte32> {
        self.by_peer
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
            _ => self.queues[lane.index()].len(),
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
