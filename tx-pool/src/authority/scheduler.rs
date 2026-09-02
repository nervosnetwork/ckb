use super::state::{
    Arrival, EntryVersion, OwnedTx, PreAcceptedEntry, PreAcceptedPhase, PreAcceptedSource,
    RawTxHash, VerifyCapability, VerifyCycleClass,
};
use super::{plan::ApplyToken, shard::ShardedOwnerWriteCut};
use crate::{constants::MAX_READY_BATCH, util::fee_rate_cross_product};
use ckb_network::PeerIndex;
use ckb_util::parking_lot::{Condvar, Mutex, MutexGuard};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    ops::Bound::{Excluded, Unbounded},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU8, Ordering as AtomicOrdering},
    },
};

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum WorkOwner {
    Remote(PeerIndex),
    Trusted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourcePriority {
    Remote,
    Proposal,
    Recovery,
}

impl SourcePriority {
    fn rank(self) -> u8 {
        match self {
            Self::Remote => 0,
            Self::Proposal => 1,
            Self::Recovery => 2,
        }
    }
}

impl Ord for SourcePriority {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rank().cmp(&other.rank())
    }
}

impl PartialOrd for SourcePriority {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum VerifyOrder {
    #[default]
    Arrival,
    FeeRate,
}

impl From<PreAcceptedSource> for SourcePriority {
    fn from(source: PreAcceptedSource) -> Self {
        match source {
            PreAcceptedSource::Remote(_) => Self::Remote,
            PreAcceptedSource::Proposal { .. } => Self::Proposal,
            PreAcceptedSource::Recovery(_) => Self::Recovery,
        }
    }
}

impl WorkOwner {
    fn from_source(source: PreAcceptedSource) -> Self {
        match source {
            PreAcceptedSource::Remote(remote) => Self::Remote(remote.residency.peer),
            PreAcceptedSource::Proposal { .. } | PreAcceptedSource::Recovery(_) => Self::Trusted,
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum QueueLane {
    Resolve,
    Verify,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueuePopulation {
    All,
    SmallOnly,
}

impl QueueLane {
    pub(super) fn for_permit(permit: super::state::WorkPermit) -> Self {
        match permit {
            super::state::WorkPermit::ResolveOnly
            | super::state::WorkPermit::ResolveThenVerify(_) => Self::Resolve,
            super::state::WorkPermit::VerifyOnly(_) => Self::Verify,
        }
    }

    fn capability(permit: super::state::WorkPermit) -> VerifyCapability {
        match permit {
            super::state::WorkPermit::ResolveOnly
            | super::state::WorkPermit::ResolveThenVerify(VerifyCapability::Any) => {
                VerifyCapability::Any
            }
            super::state::WorkPermit::ResolveThenVerify(VerifyCapability::SmallCycleOnly) => {
                VerifyCapability::SmallCycleOnly
            }
            super::state::WorkPermit::VerifyOnly(capability) => capability,
        }
    }

    fn population(self, capability: VerifyCapability) -> QueuePopulation {
        match (self, capability) {
            (Self::Resolve, VerifyCapability::Any | VerifyCapability::SmallCycleOnly)
            | (Self::Verify, VerifyCapability::Any) => QueuePopulation::All,
            (Self::Verify, VerifyCapability::SmallCycleOnly) => QueuePopulation::SmallOnly,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ResolveKey {
    source: SourcePriority,
    arrival: Arrival,
    hash: RawTxHash,
    version: EntryVersion,
}

impl Ord for ResolveKey {
    fn cmp(&self, other: &Self) -> Ordering {
        // Resolve selects the smallest key: higher-trust work comes first,
        // followed by earlier arrival and then the deterministic full hash.
        other
            .source
            .cmp(&self.source)
            .then_with(|| self.arrival.cmp(&other.arrival))
            .then_with(|| self.hash.cmp(&other.hash))
            .then_with(|| self.version.cmp(&other.version))
    }
}

impl PartialOrd for ResolveKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct VerifyKey {
    source: SourcePriority,
    order: VerifyOrder,
    fee: u64,
    serialized_bytes: u64,
    arrival: Arrival,
    hash: RawTxHash,
    version: EntryVersion,
    class: VerifyCycleClass,
}

impl Ord for VerifyKey {
    fn cmp(&self, other: &Self) -> Ordering {
        let left_rate = fee_rate_cross_product(self.fee, other.serialized_bytes);
        let right_rate = fee_rate_cross_product(other.fee, self.serialized_bytes);
        let source_and_order = self
            .source
            .cmp(&other.source)
            .then_with(|| self.order.cmp(&other.order));
        let configured_order = match self.order {
            VerifyOrder::Arrival => source_and_order,
            VerifyOrder::FeeRate => source_and_order
                .then_with(|| left_rate.cmp(&right_rate))
                .then_with(|| self.fee.cmp(&other.fee)),
        };
        configured_order
            .then_with(|| other.arrival.cmp(&self.arrival))
            .then_with(|| other.hash.cmp(&self.hash))
            .then_with(|| self.version.cmp(&other.version))
            // `class` selects a physically distinct small/large index. Keep
            // it in the total order as well as `Eq`; BTree keys must never
            // compare equal while naming different projection slots.
            .then_with(|| self.class.cmp(&other.class))
    }
}

impl PartialOrd for VerifyKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum QueueKey {
    Resolve(ResolveKey),
    Verify(VerifyKey),
}

impl QueueKey {
    fn hash(&self) -> &RawTxHash {
        match self {
            Self::Resolve(key) => &key.hash,
            Self::Verify(key) => &key.hash,
        }
    }

    fn version(&self) -> EntryVersion {
        match self {
            Self::Resolve(key) => key.version,
            Self::Verify(key) => key.version,
        }
    }

    fn class(&self) -> VerifyCycleClass {
        match self {
            Self::Resolve(_) => VerifyCycleClass::Small,
            Self::Verify(key) => key.class,
        }
    }
}

impl Ord for QueueKey {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Resolve(left), Self::Resolve(right)) => left.cmp(right),
            (Self::Verify(left), Self::Verify(right)) => left.cmp(right),
            (Self::Resolve(_), Self::Verify(_)) => Ordering::Less,
            (Self::Verify(_), Self::Resolve(_)) => Ordering::Greater,
        }
    }
}

impl PartialOrd for QueueKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ReadyKey {
    source: SourcePriority,
    fee: u64,
    serialized_bytes: u64,
    arrival: Arrival,
    hash: RawTxHash,
    version: EntryVersion,
}

const READY_SLOT_FRESH: u8 = 0;
const READY_SLOT_INVALID: u8 = 1;
const READY_SLOT_COMMITTING: u8 = 2;
const READY_SLOT_COMMITTED: u8 = 3;
const READY_SLOT_RETIRED: u8 = 4;
const READY_SLOT_POISONED: u8 = 5;

/// One exact worker-owned Ready priority decision. Capture remains the sole
/// global economic-order cut. After a reservation is split into independent
/// jobs, a stronger scheduler mutation races `Fresh -> Invalid` against the
/// job's `Fresh -> Committing`; that atomic winner is the serial order.
#[derive(Debug)]
struct ReadySlotClaim {
    state: AtomicU8,
    scheduler_wake_before: OnceLock<SchedulerWakeProjection>,
}

impl ReadySlotClaim {
    fn fresh() -> Self {
        Self {
            state: AtomicU8::new(READY_SLOT_FRESH),
            scheduler_wake_before: OnceLock::new(),
        }
    }

    fn initialize_scheduler_wake_before(&self, projection: SchedulerWakeProjection) -> bool {
        self.scheduler_wake_before.set(projection).is_ok()
    }

    fn scheduler_wake_before(&self) -> Option<SchedulerWakeProjection> {
        self.scheduler_wake_before.get().copied()
    }

    fn state(&self) -> u8 {
        self.state.load(AtomicOrdering::Acquire)
    }

    fn try_begin_commit(&self) -> bool {
        self.state
            .compare_exchange(
                READY_SLOT_FRESH,
                READY_SLOT_COMMITTING,
                AtomicOrdering::AcqRel,
                AtomicOrdering::Acquire,
            )
            .is_ok()
    }

    fn refresh(&self, fresh: bool) {
        let target = if fresh {
            READY_SLOT_FRESH
        } else {
            READY_SLOT_INVALID
        };
        let mut state = self.state();
        while matches!(state, READY_SLOT_FRESH | READY_SLOT_INVALID) && state != target {
            match self.state.compare_exchange(
                state,
                target,
                AtomicOrdering::AcqRel,
                AtomicOrdering::Acquire,
            ) {
                Ok(_) => return,
                Err(current) => state = current,
            }
        }
    }

    fn retire(&self) {
        let mut state = self.state();
        loop {
            let target = match state {
                READY_SLOT_FRESH | READY_SLOT_INVALID => READY_SLOT_RETIRED,
                READY_SLOT_COMMITTING => READY_SLOT_POISONED,
                READY_SLOT_COMMITTED | READY_SLOT_RETIRED | READY_SLOT_POISONED => return,
                _ => READY_SLOT_POISONED,
            };
            match self.state.compare_exchange(
                state,
                target,
                AtomicOrdering::AcqRel,
                AtomicOrdering::Acquire,
            ) {
                Ok(_) => return,
                Err(current) => state = current,
            }
        }
    }

    fn poison_if_committing(&self) -> bool {
        self.state
            .compare_exchange(
                READY_SLOT_COMMITTING,
                READY_SLOT_POISONED,
                AtomicOrdering::AcqRel,
                AtomicOrdering::Acquire,
            )
            .is_ok()
    }

    fn commit(&self) {
        // No scheduler mutation can retire a Committing exact key without
        // first winning the same owner shard OCC. After owner mutation the
        // private Apply tail is infallible, so this transition cannot fail.
        if self
            .state
            .compare_exchange(
                READY_SLOT_COMMITTING,
                READY_SLOT_COMMITTED,
                AtomicOrdering::AcqRel,
                AtomicOrdering::Acquire,
            )
            .is_err()
        {
            self.state
                .store(READY_SLOT_POISONED, AtomicOrdering::Release);
        }
    }
}

#[derive(Debug)]
enum ReadyReservationEntry {
    Captured,
    Claimed(Arc<ReadySlotClaim>),
}

impl ReadyReservationEntry {
    fn claim(&self) -> Option<&Arc<ReadySlotClaim>> {
        match self {
            Self::Captured => None,
            Self::Claimed(claim) => Some(claim),
        }
    }
}

impl ReadyKey {
    pub(super) fn from_ready(entry: &PreAcceptedEntry) -> Result<Self, SchedulerError> {
        let PreAcceptedPhase::Ready(verified) = &entry.phase else {
            return Err(SchedulerError::Projection);
        };
        let serialized_bytes = u64::try_from(verified.metrics().cost.serialized_bytes)
            .map_err(|_| SchedulerError::Arithmetic)?;
        if serialized_bytes == 0 {
            return Err(SchedulerError::Projection);
        }
        Ok(Self {
            source: entry.source.into(),
            fee: verified.metrics().fee.as_u64(),
            serialized_bytes,
            arrival: entry.record.arrival,
            hash: entry.record.identity.raw.clone(),
            version: entry.record.version,
        })
    }

    pub(super) fn hash(&self) -> &RawTxHash {
        &self.hash
    }

    pub(super) fn version(&self) -> EntryVersion {
        self.version
    }
}

impl Ord for ReadyKey {
    fn cmp(&self, other: &Self) -> Ordering {
        // Ready admission deliberately uses strict economic/source priority,
        // not the per-owner round-robin policy of Resolve and Verify. The
        // descending consumer therefore selects Recovery, then Proposal, then
        // Remote; within a source it selects fee rate, absolute fee and the
        // earlier arrival before deterministic identity/version ties. There
        // is no aging state. Remote residency expiry bounds hostile retention;
        // trusted work has no per-entry service-latency guarantee.
        let left_rate = fee_rate_cross_product(self.fee, other.serialized_bytes);
        let right_rate = fee_rate_cross_product(other.fee, self.serialized_bytes);
        self.source
            .cmp(&other.source)
            .then_with(|| left_rate.cmp(&right_rate))
            .then_with(|| self.fee.cmp(&other.fee))
            .then_with(|| other.arrival.cmp(&self.arrival))
            .then_with(|| other.hash.cmp(&self.hash))
            .then_with(|| self.version.cmp(&other.version))
    }
}

impl PartialOrd for ReadyKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SchedulerSlot {
    Queue {
        lane: QueueLane,
        owner: WorkOwner,
        key: QueueKey,
    },
    Ready(ReadyKey),
}

#[derive(Debug)]
pub(super) enum SchedulerError {
    Stale,
    Projection,
    Arithmetic,
    Allocation,
}

/// Move-only proof that a specific queue slot was selected from this
/// frontier. Dropping a ticket is mutation-free; only consuming it in the
/// matching scheduler delta can advance the fairness cursor.
pub(super) struct CheckoutTicket {
    lane: QueueLane,
    owner: WorkOwner,
    key: QueueKey,
}

impl CheckoutTicket {
    pub(super) fn owner(&self) -> WorkOwner {
        self.owner
    }

    pub(super) fn hash(&self) -> &RawTxHash {
        self.key.hash()
    }

    pub(super) fn version(&self) -> EntryVersion {
        self.key.version()
    }
}

pub(super) struct SchedulerDelta {
    before: Option<SchedulerSlot>,
    after: Option<SchedulerSlot>,
    owner_cursor: Option<(QueueLane, WorkOwner)>,
}

#[derive(Default)]
pub(super) struct SchedulerBatchDelta {
    removed: Vec<SchedulerSlot>,
    added: Vec<SchedulerSlot>,
    compute_queue_revision: Option<ComputeQueueRevisionChange>,
    resolve_cursor: Option<SchedulerCursorChange>,
    verify_cursor: Option<SchedulerCursorChange>,
}

/// Move-only checkout of one exact Ready prefix. Slots remain in the sole
/// scheduler projection so owner/scheduler correspondence is unchanged, but
/// subsequent Ready selection skips the reserved versions. Dropping returns
/// the exact prefix; successful shared Apply consumes it after owner commit.
#[must_use = "a Ready reservation must commit or return its exact slots"]
pub(in crate::authority) struct ReadyReservation {
    frontier: Arc<Mutex<FairFrontier>>,
    slots: Vec<ReadyKey>,
}

/// One move-owned slot split from a bounded Ready prefix without allocating a
/// per-slot vector. A worker either consumes this exact reservation in the
/// matching owner Apply or Drop returns it to the sole scheduler projection.
#[must_use = "a Ready slot reservation must commit or return its exact slot"]
pub(in crate::authority) struct ReadySlotReservation {
    frontier: Arc<Mutex<FairFrontier>>,
    slot: Option<ReadyKey>,
    claim: Arc<ReadySlotClaim>,
}

pub(in crate::authority) enum ReadyApplyReservation {
    Batch(ReadyReservation),
    Slot(ReadySlotReservation),
}

impl std::fmt::Debug for ReadyReservation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReadyReservation")
            .field("slots", &self.slots)
            .finish_non_exhaustive()
    }
}

impl PartialEq for ReadyReservation {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.frontier, &other.frontier) && self.slots == other.slots
    }
}

impl Eq for ReadyReservation {}

#[derive(Clone, Copy)]
struct SchedulerCursorChange {
    expected: Option<WorkOwner>,
    target: Option<WorkOwner>,
}

/// ABA-safe identity of the last externally visible compute-queue mutation.
/// `EntryVersion` is globally unique within one authority generation, so an
/// insertion and removal of the same slot remain distinct without a counter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComputeQueueRevision {
    Initial,
    QueueInsert(EntryVersion),
    QueueRemove(EntryVersion),
    // Source-only demotion can move one visible queue slot without replacing
    // its owner identity. This same-size token keeps that remove+insert cut
    // distinct from both the original insertion and a later removal.
    QueueReplace(EntryVersion),
}

#[derive(Clone, Copy)]
struct ComputeQueueRevisionChange {
    expected: ComputeQueueRevision,
    target: ComputeQueueRevision,
}

/// One monotonic publication fact shared by every physical row in a retained
/// ingress stage. Consumers may only observe it; the scheduler publication
/// cut owns the single false-to-true transition.
#[derive(Clone, Debug)]
pub(in crate::authority) struct StagedIngressVisibility(Arc<StagedSchedulerVisibility>);

/// Move-only publication authority for a dependency stage that is not paired
/// with a staged scheduler batch. Observers receive only the cloneable
/// `StagedIngressVisibility`; they cannot manufacture this capability.
pub(in crate::authority) struct DependencyIngressPublication(StagedIngressVisibility);

/// Linear receipt that the shared stage's one visibility bit was published.
/// A dependency stage must bind this exact receipt before terminal cleanup.
pub(in crate::authority) struct PublishedIngressVisibility(StagedIngressVisibility);

#[derive(Debug)]
struct StagedSchedulerVisibility {
    visible: AtomicBool,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum StagedSchedulerSlotKey {
    Queue(QueueKey),
    Ready(ReadyKey),
}

impl From<&SchedulerSlot> for StagedSchedulerSlotKey {
    fn from(slot: &SchedulerSlot) -> Self {
        match slot {
            SchedulerSlot::Queue { key, .. } => Self::Queue(key.clone()),
            SchedulerSlot::Ready(key) => Self::Ready(key.clone()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StagedSchedulerRole {
    Added,
    Removed,
}

#[derive(Clone, Debug)]
struct StagedSchedulerMarker {
    visibility: StagedIngressVisibility,
    role: StagedSchedulerRole,
}

impl StagedSchedulerMarker {
    fn logical_is_visible(&self) -> bool {
        match self.role {
            StagedSchedulerRole::Added => self.visibility.is_visible(),
            StagedSchedulerRole::Removed => !self.visibility.is_visible(),
        }
    }
}

impl StagedIngressVisibility {
    pub(in crate::authority) fn hidden() -> Self {
        Self(Arc::new(StagedSchedulerVisibility {
            visible: AtomicBool::new(false),
        }))
    }

    pub(in crate::authority) fn is_visible(&self) -> bool {
        self.0.visible.load(AtomicOrdering::Acquire)
    }

    pub(in crate::authority) fn same_stage(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub(in crate::authority) fn hidden_with_dependency_publication()
    -> (Self, DependencyIngressPublication) {
        let visibility = Self::hidden();
        let publication = DependencyIngressPublication(visibility.clone());
        (visibility, publication)
    }

    fn activate(&self) {
        self.0.visible.store(true, AtomicOrdering::Release);
    }

    #[cfg(test)]
    pub(in crate::authority) fn activate_for_dependency_foundation(&self) {
        self.activate();
    }
}

impl DependencyIngressPublication {
    pub(in crate::authority) fn publish(self) -> PublishedIngressVisibility {
        self.0.activate();
        PublishedIngressVisibility(self.0)
    }
}

impl PublishedIngressVisibility {
    pub(in crate::authority) fn same_stage(&self, visibility: &StagedIngressVisibility) -> bool {
        self.0.same_stage(visibility) && self.0.is_visible()
    }
}

/// A cursor-free scheduler insertion whose B-tree storage is allocated before
/// owner mutation but remains invisible to checkout and wake reads until the
/// matching shard Apply activates it. Dropping an unactivated capability
/// removes the staged slots from the one scheduler authority.
#[must_use = "a staged scheduler batch must be activated or rolled back by Drop"]
pub(super) struct StagedSchedulerBatch<'frontier> {
    frontier: &'frontier Mutex<FairFrontier>,
    delta: SchedulerBatchDelta,
    visibility: StagedIngressVisibility,
    scheduler_wake_before: SchedulerWakeProjection,
    compute_queue_revision_target: Option<ComputeQueueRevision>,
    gate: Option<SchedulerStageClass>,
    #[cfg(feature = "profiling")]
    _gate_hold_span: Option<tracing::Span>,
    ready_reservation: Option<ReadyReservation>,
    terminal: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SchedulerStageClass {
    ReadyOnly,
    CursorFreeQueue,
    Fairness,
}

#[cfg(feature = "profiling")]
fn scheduler_stage_wait_span(class: SchedulerStageClass) -> Option<tracing::Span> {
    match class {
        SchedulerStageClass::ReadyOnly => None,
        SchedulerStageClass::CursorFreeQueue => Some(tracing::trace_span!(
            target: "ckb_tx_pool_profile",
            "tx_pool.scheduler.queue_stage_wait"
        )),
        SchedulerStageClass::Fairness => Some(tracing::trace_span!(
            target: "ckb_tx_pool_profile",
            "tx_pool.scheduler.fairness_stage_wait"
        )),
    }
}

#[cfg(feature = "profiling")]
fn scheduler_stage_hold_span(class: SchedulerStageClass) -> Option<tracing::Span> {
    match class {
        SchedulerStageClass::ReadyOnly => None,
        SchedulerStageClass::CursorFreeQueue => Some(tracing::trace_span!(
            target: "ckb_tx_pool_profile",
            "tx_pool.scheduler.queue_stage_hold"
        )),
        SchedulerStageClass::Fairness => Some(tracing::trace_span!(
            target: "ckb_tx_pool_profile",
            "tx_pool.scheduler.fairness_stage_hold"
        )),
    }
}

/// Directional owner of a scheduler batch whose logical publication already
/// committed with the matching owner cut. The borrowed scheduler guard keeps
/// the Published-but-not-finalized state unobservable to every scheduler
/// reader while the physical B-tree cleanup runs after the owner cut.
#[must_use = "a published scheduler batch must be finalized before releasing its scheduler guard"]
struct PublishedSchedulerBatch<'published, 'frontier> {
    scheduler: &'published mut FairFrontier,
    stage: &'published mut StagedSchedulerBatch<'frontier>,
}

#[cfg(test)]
impl SchedulerSlot {
    fn extend_shard_support(&self, support: &mut super::shard_support::AuthorityShardSupport) {
        match self {
            Self::Queue { owner, .. } => match owner {
                WorkOwner::Remote(peer) => support.insert(b"scheduler/owner", &(0u8, peer)),
                WorkOwner::Trusted => support.insert(b"scheduler/owner", &(1u8, 0u8)),
            },
            Self::Ready(key) => support.insert(b"scheduler/ready", key.hash()),
        }
    }
}

impl SchedulerDelta {
    pub(super) const fn shared_absent_accepted() -> Self {
        Self {
            before: None,
            after: None,
            owner_cursor: None,
        }
    }

    /// Lift one cursor-free canonical replacement into the existing shared
    /// set-transition carrier. Storage is reserved before any owner cut; an
    /// exact settlement never owns a checkout cursor, so accepting one here
    /// would silently duplicate fairness authority.
    pub(super) fn into_shared_batch(self) -> Result<SchedulerBatchDelta, SchedulerError> {
        if self.owner_cursor.is_some() {
            return Err(SchedulerError::Projection);
        }
        let mut removed = Vec::new();
        let mut added = Vec::new();
        if self.before.is_some() {
            removed
                .try_reserve_exact(1)
                .map_err(|_| SchedulerError::Allocation)?;
        }
        if self.after.is_some() {
            added
                .try_reserve_exact(1)
                .map_err(|_| SchedulerError::Allocation)?;
        }
        if let Some(before) = self.before {
            removed.push(before);
        }
        if let Some(after) = self.after {
            added.push(after);
        }
        Ok(SchedulerBatchDelta {
            removed,
            added,
            compute_queue_revision: None,
            resolve_cursor: None,
            verify_cursor: None,
        })
    }
}

#[cfg(test)]
impl SchedulerDelta {
    pub(in crate::authority) fn extend_shard_support(
        &self,
        support: &mut super::shard_support::AuthorityShardSupport,
        exclusive: &mut super::shard_support::ExclusiveSupport,
    ) {
        if let Some(before) = &self.before {
            before.extend_shard_support(support);
        }
        if let Some(after) = &self.after {
            after.extend_shard_support(support);
        }
        exclusive.scheduler_cursor |= self.owner_cursor.is_some();
    }
}

impl SchedulerBatchDelta {
    pub(in crate::authority) fn is_empty(&self) -> bool {
        self.removed.is_empty()
            && self.added.is_empty()
            && self.compute_queue_revision.is_none()
            && self.resolve_cursor.is_none()
            && self.verify_cursor.is_none()
    }

    pub(in crate::authority) fn prestate_is_fresh(&self, frontier: &FairFrontier) -> bool {
        self.removed.iter().all(|slot| frontier.contains(slot))
            && self
                .added
                .iter()
                .all(|slot| self.removed.binary_search(slot).is_ok() || !frontier.contains(slot))
            && self
                .resolve_cursor
                .is_none_or(|change| frontier.resolve.owner_cursor == change.expected)
            && self
                .verify_cursor
                .is_none_or(|change| frontier.verify.owner_cursor == change.expected)
            && self
                .compute_queue_revision
                .is_none_or(|change| frontier.compute_queue_revision == change.expected)
    }

    /// A pure Ready acceptance removes committed Ready slots and publishes no
    /// new queue node or fairness cursor. That shape is allocation-free after
    /// the owner-version OCC cut succeeds, which is required by shared Apply.
    pub(super) fn is_shared_acceptance_removal_only(&self) -> bool {
        self.added.is_empty()
            && self.compute_queue_revision.is_none()
            && self.resolve_cursor.is_none()
            && self.verify_cursor.is_none()
    }

    #[cfg(test)]
    pub(super) fn is_shared_primary_insertion_only(&self) -> bool {
        self.removed.is_empty()
            && self.compute_queue_revision.is_none()
            && self.resolve_cursor.is_none()
            && self.verify_cursor.is_none()
    }

    fn compute_queue_revision_after(
        &self,
        mut revision: ComputeQueueRevision,
    ) -> ComputeQueueRevision {
        // A slot present on both sides is not externally removed or inserted:
        // the complete set transition remains hidden by the scheduler mutex.
        for slot in &self.removed {
            if self.added.binary_search(slot).is_err()
                && let Some(version) = slot.compute_queue_version()
            {
                revision = ComputeQueueRevision::QueueRemove(version);
            }
        }
        for slot in &self.added {
            if self.removed.binary_search(slot).is_err()
                && let Some(version) = slot.compute_queue_version()
            {
                revision = if self
                    .removed
                    .iter()
                    .any(|removed| removed.compute_queue_version() == Some(version))
                {
                    ComputeQueueRevision::QueueReplace(version)
                } else {
                    ComputeQueueRevision::QueueInsert(version)
                };
            }
        }
        revision
    }

    /// Precompute the only queue-revision write owned by this cursor-free set
    /// transition. The resulting token is independent of the current global
    /// revision, so the final publication cut performs one assignment instead
    /// of scanning the batch while owner shards are locked.
    fn compute_queue_revision_target(&self) -> Option<ComputeQueueRevision> {
        let mut target = None;
        for slot in &self.removed {
            if self.added.binary_search(slot).is_err()
                && let Some(version) = slot.compute_queue_version()
            {
                target = Some(ComputeQueueRevision::QueueRemove(version));
            }
        }
        for slot in &self.added {
            if self.removed.binary_search(slot).is_err()
                && let Some(version) = slot.compute_queue_version()
            {
                target = Some(
                    if self
                        .removed
                        .iter()
                        .any(|removed| removed.compute_queue_version() == Some(version))
                    {
                        ComputeQueueRevision::QueueReplace(version)
                    } else {
                        ComputeQueueRevision::QueueInsert(version)
                    },
                );
            }
        }
        target
    }

    fn is_exact_ready_removal(&self, key: &ReadyKey) -> bool {
        self.is_shared_acceptance_removal_only()
            && self.removed.len() == 1
            && matches!(self.removed.first(), Some(SchedulerSlot::Ready(removed)) if removed == key)
    }
}

impl FairFrontier {
    /// Writer-preferred reader/writer gate for the globally coupled compute
    /// frontier. Cursor-free queue stages are concurrent readers; a fairness
    /// cursor stage is the writer. Waiting happens before owner locks, and a
    /// queued writer prevents an unbounded stream of new readers from starving
    /// checkout progress. Ready-only stages bypass the gate entirely.
    fn acquire_hidden_stage(
        scheduler: &mut MutexGuard<'_, Self>,
        class: SchedulerStageClass,
    ) -> Result<(), SchedulerError> {
        match class {
            SchedulerStageClass::ReadyOnly => Ok(()),
            SchedulerStageClass::CursorFreeQueue => {
                let changed = Arc::clone(&scheduler.stage_gate_changed);
                while scheduler.fairness_stage_active || scheduler.fairness_stage_waiters != 0 {
                    changed.wait(scheduler);
                }
                scheduler.hidden_cursor_free_queue_stages = scheduler
                    .hidden_cursor_free_queue_stages
                    .checked_add(1)
                    .ok_or(SchedulerError::Arithmetic)?;
                Ok(())
            }
            SchedulerStageClass::Fairness => {
                scheduler.fairness_stage_waiters = scheduler
                    .fairness_stage_waiters
                    .checked_add(1)
                    .ok_or(SchedulerError::Arithmetic)?;
                let changed = Arc::clone(&scheduler.stage_gate_changed);
                while scheduler.fairness_stage_active
                    || scheduler.hidden_cursor_free_queue_stages != 0
                {
                    changed.wait(scheduler);
                }
                scheduler.fairness_stage_waiters = scheduler
                    .fairness_stage_waiters
                    .checked_sub(1)
                    .ok_or(SchedulerError::Projection)?;
                scheduler.fairness_stage_active = true;
                Ok(())
            }
        }
    }

    fn release_hidden_stage(&mut self, class: SchedulerStageClass) {
        match class {
            SchedulerStageClass::ReadyOnly => {}
            SchedulerStageClass::CursorFreeQueue => {
                if let Some(remaining) = self.hidden_cursor_free_queue_stages.checked_sub(1) {
                    self.hidden_cursor_free_queue_stages = remaining;
                } else {
                    debug_assert!(false, "a live queue stage owns one reservation");
                }
            }
            SchedulerStageClass::Fairness => {
                debug_assert!(self.fairness_stage_active);
                self.fairness_stage_active = false;
            }
        }
        if !matches!(class, SchedulerStageClass::ReadyOnly) {
            self.stage_gate_changed.notify_all();
        }
    }

    #[cfg(test)]
    fn staged_queue_marker_for(&self, key: &QueueKey) -> Option<&StagedSchedulerMarker> {
        self.staged_visibility
            .get(&StagedSchedulerSlotKey::Queue(key.clone()))
    }

    fn staged_ready_marker_for(&self, key: &ReadyKey) -> Option<&StagedSchedulerMarker> {
        self.staged_visibility
            .get(&StagedSchedulerSlotKey::Ready(key.clone()))
    }

    fn slot_claim_is_current(&self, key: &ReadyKey, claim: &Arc<ReadySlotClaim>) -> bool {
        self.ready_reserved
            .get(key)
            .and_then(ReadyReservationEntry::claim)
            .is_some_and(|current| Arc::ptr_eq(current, claim))
    }

    fn ready_slot_claim_count(&self) -> usize {
        self.ready_reserved
            .values()
            .filter(|entry| entry.claim().is_some())
            .count()
    }

    fn logical_ready_contains(&self, key: &ReadyKey) -> bool {
        self.ready.contains(key)
            && self
                .staged_ready_marker_for(key)
                .is_none_or(StagedSchedulerMarker::logical_is_visible)
            && !self
                .ready_reserved
                .get(key)
                .and_then(ReadyReservationEntry::claim)
                .is_some_and(|claim| {
                    matches!(
                        claim.state(),
                        READY_SLOT_COMMITTING
                            | READY_SLOT_COMMITTED
                            | READY_SLOT_RETIRED
                            | READY_SLOT_POISONED
                    )
                })
    }

    fn refresh_slot_claims(&self) {
        if self.ready_slot_claim_count() == 0 {
            return;
        }
        let strongest_unreserved = self.ready.iter().rev().find(|key| {
            self.logical_ready_contains(key) && !self.ready_reserved.contains_key(*key)
        });
        for (key, claim) in self
            .ready_reserved
            .iter()
            .filter_map(|(key, entry)| entry.claim().map(|claim| (key, claim)))
        {
            let fresh = self.logical_ready_contains(key)
                && strongest_unreserved.is_none_or(|blocker| blocker <= key);
            claim.refresh(fresh);
        }
    }

    fn reap_slot_claims(&mut self) -> Result<(), SchedulerError> {
        if self.ready_slot_claim_count() == 0 {
            return Ok(());
        }
        let mut poisoned = false;
        let ready = &mut self.ready;
        self.ready_reserved.retain(|key, entry| {
            let Some(claim) = entry.claim() else {
                return true;
            };
            match claim.state() {
                READY_SLOT_COMMITTED | READY_SLOT_RETIRED => {
                    ready.remove(key);
                    false
                }
                READY_SLOT_POISONED => {
                    poisoned = true;
                    true
                }
                READY_SLOT_FRESH | READY_SLOT_INVALID | READY_SLOT_COMMITTING => true,
                _ => {
                    poisoned = true;
                    true
                }
            }
        });
        if poisoned {
            Err(SchedulerError::Projection)
        } else {
            Ok(())
        }
    }

    fn return_slot_claim(&mut self, key: &ReadyKey, claim: &Arc<ReadySlotClaim>) {
        if !self.slot_claim_is_current(key, claim) {
            return;
        }
        match claim.state() {
            READY_SLOT_FRESH | READY_SLOT_INVALID => {
                // Keep this stronger key reserved until every weaker live
                // slot has lost the claim race. A weaker CAS that wins first
                // is serializable before this cancellation.
                for weaker in self
                    .ready_reserved
                    .range(..key)
                    .filter_map(|(_, entry)| entry.claim())
                {
                    weaker.refresh(false);
                }
                claim.refresh(false);
                self.ready_reserved.remove(key);
            }
            READY_SLOT_RETIRED => {
                self.ready_reserved.remove(key);
                self.refresh_slot_claims();
            }
            READY_SLOT_COMMITTING => {
                let _ = claim.poison_if_committing();
            }
            READY_SLOT_COMMITTED | READY_SLOT_POISONED => {}
            _ => claim.retire(),
        }
    }

    fn return_batch_slots(&mut self, slots: &[ReadyKey]) {
        for key in slots {
            if self.ready.contains(key) && self.ready_reserved.contains_key(key) {
                for claim in self
                    .ready_reserved
                    .range(..key)
                    .filter_map(|(_, entry)| entry.claim())
                {
                    claim.refresh(false);
                }
            }
        }
        for key in slots {
            self.ready_reserved.remove(key);
        }
    }
}

impl ReadyReservation {
    pub(in crate::authority) fn capture(
        frontier: &Arc<Mutex<FairFrontier>>,
    ) -> Result<Option<Self>, SchedulerError> {
        let mut scheduler = frontier.lock();
        scheduler.reap_slot_claims()?;
        let remaining = MAX_READY_BATCH
            .checked_sub(scheduler.ready_reserved.len())
            .ok_or(SchedulerError::Projection)?;
        let count = scheduler
            .ready
            .iter()
            .rev()
            .filter(|key| {
                scheduler.logical_ready_contains(key)
                    && !scheduler.ready_reserved.contains_key(*key)
            })
            .take(remaining)
            .count();
        if count == 0 {
            return Ok(None);
        }
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(count)
            .map_err(|_| SchedulerError::Allocation)?;
        slots.extend(
            scheduler
                .ready
                .iter()
                .rev()
                .filter(|key| {
                    scheduler.logical_ready_contains(key)
                        && !scheduler.ready_reserved.contains_key(*key)
                })
                .take(remaining)
                .cloned(),
        );
        for key in &slots {
            if scheduler
                .ready_reserved
                .insert(key.clone(), ReadyReservationEntry::Captured)
                .is_some()
            {
                for inserted in &slots {
                    if inserted == key {
                        break;
                    }
                    scheduler.ready_reserved.remove(inserted);
                }
                return Err(SchedulerError::Projection);
            }
        }
        drop(scheduler);
        Ok(Some(Self {
            frontier: Arc::clone(frontier),
            slots,
        }))
    }

    #[cfg(test)]
    pub(in crate::authority) fn capture_exact_for_foundation(
        frontier: &Arc<Mutex<FairFrontier>>,
        hashes: &[RawTxHash],
    ) -> Result<Self, SchedulerError> {
        if hashes.is_empty() {
            return Err(SchedulerError::Projection);
        }
        let mut scheduler = frontier.lock();
        scheduler.reap_slot_claims()?;
        let remaining = MAX_READY_BATCH
            .checked_sub(scheduler.ready_reserved.len())
            .ok_or(SchedulerError::Projection)?;
        if hashes.len() > remaining {
            return Err(SchedulerError::Projection);
        }
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(hashes.len())
            .map_err(|_| SchedulerError::Allocation)?;
        for hash in hashes {
            let key = scheduler
                .ready
                .iter()
                .find(|key| key.hash() == hash && scheduler.logical_ready_contains(key))
                .ok_or(SchedulerError::Projection)?;
            if scheduler.ready_reserved.contains_key(key)
                || slots.iter().any(|selected| selected == key)
            {
                return Err(SchedulerError::Projection);
            }
            slots.push(key.clone());
        }
        for key in &slots {
            if scheduler
                .ready_reserved
                .insert(key.clone(), ReadyReservationEntry::Captured)
                .is_some()
            {
                return Err(SchedulerError::Projection);
            }
        }
        drop(scheduler);
        Ok(Self {
            frontier: Arc::clone(frontier),
            slots,
        })
    }

    pub(in crate::authority) fn candidates(
        &self,
    ) -> impl ExactSizeIterator<Item = (&RawTxHash, EntryVersion)> {
        self.slots.iter().map(|key| (key.hash(), key.version()))
    }

    pub(in crate::authority) fn try_split_prefix(
        mut self,
        count: usize,
    ) -> Result<(Vec<ReadySlotReservation>, Option<Self>), Self> {
        if count == 0 || self.slots.len() < count {
            return Err(self);
        }
        let mut reservations = Vec::new();
        if reservations.try_reserve_exact(count).is_err() {
            return Err(self);
        }
        let mut claims = Vec::new();
        if claims.try_reserve_exact(count).is_err() {
            return Err(self);
        }
        claims.extend((0..count).map(|_| Arc::new(ReadySlotClaim::fresh())));
        let installed = {
            let mut scheduler = self.frontier.lock();
            let mut invalid = scheduler.reap_slot_claims().is_err()
                || scheduler
                    .ready_slot_claim_count()
                    .checked_add(count)
                    .is_none_or(|claims| claims > MAX_READY_BATCH)
                || self.slots.iter().take(count).any(|key| {
                    !matches!(
                        scheduler.ready_reserved.get(key),
                        Some(ReadyReservationEntry::Captured)
                    )
                });
            let scheduler_wake_before = (!invalid).then(|| scheduler.wake_projection());
            if !invalid {
                invalid = scheduler_wake_before.is_none_or(|projection| {
                    claims
                        .iter()
                        .any(|claim| !claim.initialize_scheduler_wake_before(projection))
                });
            }
            if !invalid {
                let mut installed = 0usize;
                for (key, claim) in self.slots.iter().take(count).zip(&claims) {
                    match scheduler.ready_reserved.get_mut(key) {
                        Some(entry @ ReadyReservationEntry::Captured) => {
                            let Some(next_installed) = installed.checked_add(1) else {
                                break;
                            };
                            *entry = ReadyReservationEntry::Claimed(Arc::clone(claim));
                            installed = next_installed;
                        }
                        Some(ReadyReservationEntry::Claimed(_)) | None => break,
                    }
                }
                if installed != count {
                    for (key, claim) in self.slots.iter().take(installed).zip(&claims) {
                        if scheduler
                            .ready_reserved
                            .get(key)
                            .and_then(ReadyReservationEntry::claim)
                            .is_some_and(|current| Arc::ptr_eq(current, claim))
                            && let Some(entry) = scheduler.ready_reserved.get_mut(key)
                        {
                            *entry = ReadyReservationEntry::Captured;
                        }
                    }
                    invalid = true;
                }
                if !invalid {
                    scheduler.refresh_slot_claims();
                }
            }
            !invalid
        };
        if !installed {
            return Err(self);
        }
        reservations.extend(self.slots.drain(..count).zip(claims).map(|(slot, claim)| {
            ReadySlotReservation {
                frontier: Arc::clone(&self.frontier),
                slot: Some(slot),
                claim,
            }
        }));
        let remainder = (!self.slots.is_empty()).then_some(self);
        Ok((reservations, remainder))
    }

    pub(in crate::authority) fn current_prefix_len<'candidate>(
        &self,
        frontier: &Arc<Mutex<FairFrontier>>,
        captured: impl IntoIterator<Item = (&'candidate RawTxHash, EntryVersion)>,
    ) -> usize {
        if !Arc::ptr_eq(&self.frontier, frontier) {
            return 0;
        }
        let scheduler = frontier.lock();
        scheduler
            .ready
            .iter()
            .rev()
            .filter(|key| {
                scheduler.logical_ready_contains(key)
                    && (!scheduler.ready_reserved.contains_key(*key) || self.slots.contains(*key))
            })
            .zip(captured)
            .take_while(|(current, captured)| {
                current.hash() == captured.0
                    && current.version() == captured.1
                    && scheduler.ready_reserved.contains_key(*current)
            })
            .count()
    }

    fn prestate_is_fresh_locked(
        &self,
        scheduler: &FairFrontier,
        delta: &SchedulerBatchDelta,
    ) -> bool {
        delta.prestate_is_fresh(scheduler) && self.selection_is_fresh_locked(scheduler, delta)
    }

    /// Revalidate only the captured Ready-prefix premise after the matching
    /// scheduler stage owns its hidden rows. The complete set prestate is a
    /// pre-stage condition: repeating it here would reject the stage's own
    /// physically inserted but still-invisible queue additions.
    fn selection_is_fresh_locked(
        &self,
        scheduler: &FairFrontier,
        delta: &SchedulerBatchDelta,
    ) -> bool {
        if !self
            .slots
            .iter()
            .all(|key| scheduler.ready_reserved.contains_key(key))
        {
            return false;
        }
        let mut last_consumed = None;
        for slot in &delta.removed {
            let SchedulerSlot::Ready(key) = slot else {
                return false;
            };
            let Some(index) = self.slots.iter().position(|reserved| reserved == key) else {
                return false;
            };
            last_consumed = Some(last_consumed.map_or(index, |last: usize| last.max(index)));
        }
        let Some(last_consumed) = last_consumed else {
            return false;
        };
        scheduler
            .ready
            .iter()
            .rev()
            .filter(|key| {
                scheduler.logical_ready_contains(key)
                    && (!scheduler.ready_reserved.contains_key(*key) || self.slots.contains(*key))
            })
            .take(last_consumed.saturating_add(1))
            .eq(self.slots.iter().take(last_consumed.saturating_add(1)))
    }
}

impl ReadySlotReservation {
    pub(in crate::authority) fn prestate_is_fresh(
        &self,
        frontier: &Arc<Mutex<FairFrontier>>,
        delta: &SchedulerBatchDelta,
    ) -> bool {
        let Some(slot) = self.slot.as_ref() else {
            return false;
        };
        if !Arc::ptr_eq(&self.frontier, frontier) || !delta.is_shared_acceptance_removal_only() {
            return false;
        }
        delta.is_exact_ready_removal(slot) && self.claim.try_begin_commit()
    }

    pub(in crate::authority) fn activate(
        mut self,
        _frontier: &Arc<Mutex<FairFrontier>>,
        delta: SchedulerBatchDelta,
    ) {
        let _ = delta;
        let _ = self.slot.take();
        self.claim.commit();
    }
}

impl ReadyApplyReservation {
    pub(in crate::authority) fn scheduler_wake_before(
        &self,
    ) -> Result<Option<SchedulerWakeProjection>, SchedulerError> {
        match self {
            Self::Batch(_) => Ok(None),
            Self::Slot(reservation) => reservation
                .claim
                .scheduler_wake_before()
                .map(Some)
                .ok_or(SchedulerError::Projection),
        }
    }
}

impl Drop for ReadyReservation {
    fn drop(&mut self) {
        if self.slots.is_empty() {
            return;
        }
        let mut scheduler = self.frontier.lock();
        scheduler.return_batch_slots(&self.slots);
    }
}

impl Drop for ReadySlotReservation {
    fn drop(&mut self) {
        let Some(slot) = self.slot.take() else {
            return;
        };
        self.frontier.lock().return_slot_claim(&slot, &self.claim);
    }
}

impl SchedulerSlot {
    fn compute_queue_version(&self) -> Option<EntryVersion> {
        match self {
            Self::Queue { key, .. } => Some(key.version()),
            Self::Ready(_) => None,
        }
    }
}

impl<'frontier> StagedSchedulerBatch<'frontier> {
    #[cfg(test)]
    pub(super) fn stage_primary_insertions(
        frontier: &'frontier Arc<Mutex<FairFrontier>>,
        delta: SchedulerBatchDelta,
    ) -> Result<Self, SchedulerError> {
        if !delta.is_shared_primary_insertion_only() {
            return Err(SchedulerError::Projection);
        }
        Self::stage_primary_replacements(frontier, delta)
    }

    pub(super) fn stage_primary_replacements(
        frontier: &'frontier Arc<Mutex<FairFrontier>>,
        delta: SchedulerBatchDelta,
    ) -> Result<Self, SchedulerError> {
        Self::stage_with_ready_reservation(frontier, delta, None)
    }

    pub(super) fn stage_reserved_ready_batch(
        frontier: &'frontier Arc<Mutex<FairFrontier>>,
        delta: SchedulerBatchDelta,
        reservation: ReadyReservation,
    ) -> Result<Self, SchedulerError> {
        if !Arc::ptr_eq(&reservation.frontier, frontier) {
            return Err(SchedulerError::Projection);
        }
        Self::stage_with_ready_reservation(frontier, delta, Some(reservation))
    }

    fn stage_with_ready_reservation(
        frontier: &'frontier Arc<Mutex<FairFrontier>>,
        delta: SchedulerBatchDelta,
        ready_reservation: Option<ReadyReservation>,
    ) -> Result<Self, SchedulerError> {
        let cursor_free_revision_target = delta.compute_queue_revision_target();
        let class = if delta.compute_queue_revision.is_some()
            || delta.resolve_cursor.is_some()
            || delta.verify_cursor.is_some()
        {
            SchedulerStageClass::Fairness
        } else if cursor_free_revision_target.is_some() {
            SchedulerStageClass::CursorFreeQueue
        } else {
            SchedulerStageClass::ReadyOnly
        };
        let compute_queue_revision_target = delta
            .compute_queue_revision
            .map(|change| change.target)
            .or(cursor_free_revision_target);
        let visibility = StagedIngressVisibility::hidden();
        #[cfg(feature = "profiling")]
        let gate_wait_span = scheduler_stage_wait_span(class);
        let mut scheduler = frontier.lock();
        let gate_result = FairFrontier::acquire_hidden_stage(&mut scheduler, class);
        #[cfg(feature = "profiling")]
        drop(gate_wait_span);
        gate_result?;
        #[cfg(feature = "profiling")]
        let gate_hold_span = scheduler_stage_hold_span(class);
        let scheduler_wake_before = scheduler.wake_projection();
        if ready_reservation
            .as_ref()
            .is_some_and(|reservation| !reservation.prestate_is_fresh_locked(&scheduler, &delta))
            || !delta.prestate_is_fresh(&scheduler)
            || delta.removed.iter().any(|slot| !scheduler.contains(slot))
            || delta.removed.iter().any(|slot| {
                if delta.added.binary_search(slot).is_ok() {
                    return false;
                }
                scheduler
                    .staged_visibility
                    .contains_key(&StagedSchedulerSlotKey::from(slot))
            })
            || delta.added.iter().any(|slot| {
                delta.removed.binary_search(slot).is_err() && scheduler.contains_physical(slot)
            })
            || delta.added.iter().any(|slot| {
                if delta.removed.binary_search(slot).is_ok() {
                    return false;
                }
                scheduler
                    .staged_visibility
                    .contains_key(&StagedSchedulerSlotKey::from(slot))
            })
        {
            scheduler.release_hidden_stage(class);
            drop(scheduler);
            #[cfg(feature = "profiling")]
            drop(gate_hold_span);
            return Err(SchedulerError::Stale);
        }
        for slot in delta
            .removed
            .iter()
            .filter(|slot| delta.added.binary_search(slot).is_err())
        {
            scheduler.staged_visibility.insert(
                StagedSchedulerSlotKey::from(slot),
                StagedSchedulerMarker {
                    visibility: visibility.clone(),
                    role: StagedSchedulerRole::Removed,
                },
            );
        }
        for slot in delta
            .added
            .iter()
            .filter(|slot| delta.removed.binary_search(slot).is_err())
            .cloned()
        {
            let key = StagedSchedulerSlotKey::from(&slot);
            scheduler.insert_physical(slot);
            scheduler.staged_visibility.insert(
                key,
                StagedSchedulerMarker {
                    visibility: visibility.clone(),
                    role: StagedSchedulerRole::Added,
                },
            );
        }
        drop(scheduler);
        Ok(Self {
            frontier: Arc::as_ref(frontier),
            delta,
            visibility,
            scheduler_wake_before,
            compute_queue_revision_target,
            gate: Some(class),
            #[cfg(feature = "profiling")]
            _gate_hold_span: gate_hold_span,
            ready_reservation,
            terminal: false,
        })
    }

    pub(super) fn prestate_is_fresh(&self) -> bool {
        // Stage markers, the generation read guard, the exact owner cut and
        // the writer-preferred queue/fairness gate make every no-reservation
        // scheduler premise stable until this stage publishes. Batch Ready is
        // different: its strongest-prefix premise spans unrelated Ready rows
        // and therefore needs one final scheduler selection cut.
        if self.ready_reservation.is_none() {
            return true;
        }
        let scheduler = self.frontier.lock();
        self.prestate_is_fresh_locked(&scheduler)
    }

    fn prestate_is_fresh_locked(&self, scheduler: &FairFrontier) -> bool {
        self.stage_ownership_is_intact_locked(scheduler)
            && self.ready_reservation.as_ref().is_none_or(|reservation| {
                reservation.selection_is_fresh_locked(scheduler, &self.delta)
            })
    }

    /// Stable ownership established at staging. Unlike a batch Ready prefix,
    /// these premises cannot change while this stage owns its marker set and
    /// queue/fairness gate. Publication may therefore assert them after owner
    /// mutation without rechecking the time-sensitive Ready ordering proof.
    fn stage_ownership_is_intact_locked(&self, scheduler: &FairFrontier) -> bool {
        let gate_is_held = match self.gate {
            Some(SchedulerStageClass::ReadyOnly) => true,
            Some(SchedulerStageClass::CursorFreeQueue) => {
                scheduler.hidden_cursor_free_queue_stages != 0
            }
            Some(SchedulerStageClass::Fairness) => scheduler.fairness_stage_active,
            None => false,
        };
        gate_is_held
            && self
                .delta
                .removed
                .iter()
                .filter(|slot| self.delta.added.binary_search(slot).is_err())
                .all(|slot| {
                    scheduler.contains_physical(slot)
                        && scheduler
                            .staged_visibility
                            .get(&StagedSchedulerSlotKey::from(slot))
                            .is_some_and(|marker| {
                                marker.role == StagedSchedulerRole::Removed
                                    && marker.visibility.same_stage(&self.visibility)
                                    && marker.logical_is_visible()
                            })
                })
            && self
                .delta
                .added
                .iter()
                .filter(|slot| self.delta.removed.binary_search(slot).is_err())
                .all(|slot| {
                    scheduler.contains_physical(slot)
                        && scheduler
                            .staged_visibility
                            .get(&StagedSchedulerSlotKey::from(slot))
                            .is_some_and(|marker| {
                                marker.role == StagedSchedulerRole::Added
                                    && marker.visibility.same_stage(&self.visibility)
                                    && !marker.logical_is_visible()
                            })
                })
            && self
                .delta
                .removed
                .iter()
                .filter(|slot| self.delta.added.binary_search(slot).is_ok())
                .all(|slot| scheduler.contains(slot))
            && self
                .delta
                .compute_queue_revision
                .is_none_or(|change| scheduler.compute_queue_revision == change.expected)
            && self
                .delta
                .resolve_cursor
                .is_none_or(|change| scheduler.resolve.owner_cursor == change.expected)
            && self
                .delta
                .verify_cursor
                .is_none_or(|change| scheduler.verify.owner_cursor == change.expected)
    }

    fn release_stage_gate_locked(&mut self, scheduler: &mut FairFrontier) {
        if let Some(class) = self.gate.take() {
            scheduler.release_hidden_stage(class);
        }
    }

    fn publish_locked<'published>(
        &'published mut self,
        scheduler: &'published mut FairFrontier,
        owner_cut: ShardedOwnerWriteCut<'_>,
    ) -> PublishedSchedulerBatch<'published, 'frontier> {
        debug_assert!(self.stage_ownership_is_intact_locked(scheduler));
        if let Some(target) = self.compute_queue_revision_target {
            scheduler.compute_queue_revision = target;
        }
        if let Some(change) = self.delta.resolve_cursor {
            scheduler.resolve.owner_cursor = change.target;
        }
        if let Some(change) = self.delta.verify_cursor {
            scheduler.verify.owner_cursor = change.target;
        }
        self.release_stage_gate_locked(scheduler);
        // The revision and staged rows become externally visible in this one
        // scheduler lock cut. No checkout can observe a new queue slot paired
        // with the revision that preceded it.
        self.visibility.activate();
        // Hidden Ready additions are deliberately ignored by selection. At
        // publication the claim refresh CAS is the priority linearization:
        // an already-Committing weaker claim orders before this stage, while
        // every still-Fresh weaker claim becomes invalid before owner release.
        scheduler.refresh_slot_claims();
        // The scheduler guard remains held by PublishedSchedulerBatch, but the
        // complete owner cut ends before B-tree cleanup or scheduler payload
        // destruction. Ready claim refresh belongs to the publication cut and
        // therefore completes before owner release.
        drop(owner_cut);
        PublishedSchedulerBatch {
            scheduler,
            stage: self,
        }
    }

    fn activate_inner(mut self, owner_cut: ShardedOwnerWriteCut<'_>) -> PublishedIngressVisibility {
        let visibility = self.visibility.clone();
        let frontier = self.frontier;
        let mut scheduler = frontier.lock();
        let retired_delta = self.publish_locked(&mut scheduler, owner_cut).finalize();
        drop(scheduler);
        drop(retired_delta);
        PublishedIngressVisibility(visibility)
    }

    pub(super) fn activate(
        self,
        _token: &ApplyToken,
        owner_cut: ShardedOwnerWriteCut<'_>,
    ) -> PublishedIngressVisibility {
        self.activate_inner(owner_cut)
    }

    #[cfg(test)]
    fn activate_for_foundation(
        self,
        owner_cut: ShardedOwnerWriteCut<'_>,
    ) -> PublishedIngressVisibility {
        self.activate_inner(owner_cut)
    }

    pub(super) fn visibility(&self) -> StagedIngressVisibility {
        self.visibility.clone()
    }

    pub(super) fn wake_projection_before(&self) -> Option<SchedulerWakeProjection> {
        Some(self.scheduler_wake_before)
    }
}

impl PublishedSchedulerBatch<'_, '_> {
    fn finalize_inner(&mut self) -> SchedulerBatchDelta {
        if self.stage.terminal {
            return SchedulerBatchDelta::default();
        }
        let delta = std::mem::take(&mut self.stage.delta);
        for slot in delta
            .removed
            .iter()
            .filter(|slot| delta.added.binary_search(slot).is_err())
        {
            self.scheduler.remove_physical_ref(slot);
        }
        for slot in delta
            .removed
            .iter()
            .filter(|slot| delta.added.binary_search(slot).is_err())
            .chain(
                delta
                    .added
                    .iter()
                    .filter(|slot| delta.removed.binary_search(slot).is_err()),
            )
        {
            let key = StagedSchedulerSlotKey::from(slot);
            if self
                .scheduler
                .staged_visibility
                .get(&key)
                .is_some_and(|marker| marker.visibility.same_stage(&self.stage.visibility))
            {
                self.scheduler.staged_visibility.remove(&key);
            }
        }
        self.scheduler.refresh_slot_claims();
        self.stage.compute_queue_revision_target = None;
        self.stage.terminal = true;
        delta
    }

    fn finalize(mut self) -> SchedulerBatchDelta {
        self.finalize_inner()
    }
}

impl Drop for PublishedSchedulerBatch<'_, '_> {
    fn drop(&mut self) {
        // Exceptional unwinding still follows the committed direction. The
        // ordinary path calls finalize explicitly so scheduler payloads can be
        // destroyed only after the scheduler mutex is released.
        let _ = self.finalize_inner();
    }
}

impl Drop for StagedSchedulerBatch<'_> {
    fn drop(&mut self) {
        if self.terminal || self.delta.is_empty() {
            return;
        }
        let mut scheduler = self.frontier.lock();
        self.release_stage_gate_locked(&mut scheduler);
        for slot in self
            .delta
            .removed
            .iter()
            .filter(|slot| self.delta.added.binary_search(slot).is_err())
            .chain(
                self.delta
                    .added
                    .iter()
                    .filter(|slot| self.delta.removed.binary_search(slot).is_err()),
            )
        {
            let key = StagedSchedulerSlotKey::from(slot);
            if scheduler
                .staged_visibility
                .get(&key)
                .is_some_and(|marker| marker.visibility.same_stage(&self.visibility))
            {
                scheduler.staged_visibility.remove(&key);
            }
        }
        if !self.visibility.is_visible() {
            for slot in self
                .delta
                .added
                .iter()
                .filter(|slot| self.delta.removed.binary_search(slot).is_err())
            {
                scheduler.remove_physical_ref(slot);
            }
        }
        self.terminal = true;
    }
}

/// Allocation-free runnable heads derived from the committed scheduler.
///
/// `EntryVersion` is globally unique within one authority generation. A
/// changed non-empty value therefore proves that a capability class has a new
/// head worth probing without copying a transaction identity or maintaining a
/// second ready flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SchedulerWakeProjection {
    pub(super) resolve: Option<EntryVersion>,
    pub(super) verify_small: Option<EntryVersion>,
    pub(super) verify_any: Option<EntryVersion>,
    pub(super) ready: Option<EntryVersion>,
}

#[derive(Debug, Default)]
struct OwnerQueue {
    small: BTreeSet<QueueKey>,
    large: BTreeSet<QueueKey>,
}

fn queue_key_is_visible(
    staged: &BTreeMap<StagedSchedulerSlotKey, StagedSchedulerMarker>,
    key: &QueueKey,
) -> bool {
    staged
        .get(&StagedSchedulerSlotKey::Queue(key.clone()))
        .is_none_or(StagedSchedulerMarker::logical_is_visible)
}

impl OwnerQueue {
    fn entries(&self, class: VerifyCycleClass) -> &BTreeSet<QueueKey> {
        match class {
            VerifyCycleClass::Small => &self.small,
            VerifyCycleClass::Large => &self.large,
        }
    }

    fn entries_mut(&mut self, class: VerifyCycleClass) -> &mut BTreeSet<QueueKey> {
        match class {
            VerifyCycleClass::Small => &mut self.small,
            VerifyCycleClass::Large => &mut self.large,
        }
    }

    fn contains(&self, key: &QueueKey) -> bool {
        self.entries(key.class()).contains(key)
    }

    fn insert(&mut self, key: QueueKey) {
        self.entries_mut(key.class()).insert(key);
    }

    fn remove(&mut self, key: &QueueKey) {
        self.entries_mut(key.class()).remove(key);
    }

    fn head(
        &self,
        lane: QueueLane,
        capability: VerifyCapability,
        staged: &BTreeMap<StagedSchedulerSlotKey, StagedSchedulerMarker>,
    ) -> Option<&QueueKey> {
        fn first_visible<'entries>(
            entries: &'entries BTreeSet<QueueKey>,
            staged: &BTreeMap<StagedSchedulerSlotKey, StagedSchedulerMarker>,
        ) -> Option<&'entries QueueKey> {
            entries.iter().find(|key| queue_key_is_visible(staged, key))
        }
        fn last_visible<'entries>(
            entries: &'entries BTreeSet<QueueKey>,
            staged: &BTreeMap<StagedSchedulerSlotKey, StagedSchedulerMarker>,
        ) -> Option<&'entries QueueKey> {
            entries
                .iter()
                .rev()
                .find(|key| queue_key_is_visible(staged, key))
        }
        match lane {
            QueueLane::Resolve => first_visible(&self.small, staged),
            QueueLane::Verify => match capability {
                VerifyCapability::SmallCycleOnly => last_visible(&self.small, staged),
                VerifyCapability::Any => {
                    match (
                        last_visible(&self.small, staged),
                        last_visible(&self.large, staged),
                    ) {
                        (Some(small), Some(large)) => Some(std::cmp::max(small, large)),
                        (Some(small), None) => Some(small),
                        (None, Some(large)) => Some(large),
                        (None, None) => None,
                    }
                }
            },
        }
    }

    fn head_excluding(
        &self,
        lane: QueueLane,
        capability: VerifyCapability,
        excluded_versions: &[EntryVersion],
        staged: &BTreeMap<StagedSchedulerSlotKey, StagedSchedulerMarker>,
    ) -> Option<&QueueKey> {
        fn first_available<'entries>(
            entries: &'entries BTreeSet<QueueKey>,
            excluded_versions: &[EntryVersion],
            staged: &BTreeMap<StagedSchedulerSlotKey, StagedSchedulerMarker>,
        ) -> Option<&'entries QueueKey> {
            entries.iter().find(|key| {
                queue_key_is_visible(staged, key)
                    && excluded_versions.binary_search(&key.version()).is_err()
            })
        }
        fn last_available<'entries>(
            entries: &'entries BTreeSet<QueueKey>,
            excluded_versions: &[EntryVersion],
            staged: &BTreeMap<StagedSchedulerSlotKey, StagedSchedulerMarker>,
        ) -> Option<&'entries QueueKey> {
            entries.iter().rev().find(|key| {
                queue_key_is_visible(staged, key)
                    && excluded_versions.binary_search(&key.version()).is_err()
            })
        }
        match lane {
            QueueLane::Resolve => first_available(&self.small, excluded_versions, staged),
            QueueLane::Verify => match capability {
                VerifyCapability::SmallCycleOnly => {
                    last_available(&self.small, excluded_versions, staged)
                }
                VerifyCapability::Any => {
                    match (
                        last_available(&self.small, excluded_versions, staged),
                        last_available(&self.large, excluded_versions, staged),
                    ) {
                        (Some(small), Some(large)) => Some(std::cmp::max(small, large)),
                        (Some(small), None) => Some(small),
                        (None, Some(large)) => Some(large),
                        (None, None) => None,
                    }
                }
            },
        }
    }

    fn is_empty(&self) -> bool {
        self.small.is_empty() && self.large.is_empty()
    }
}

#[derive(Debug, Default)]
struct FairLane {
    by_owner: BTreeMap<WorkOwner, OwnerQueue>,
    small_owners: BTreeSet<WorkOwner>,
    owner_cursor: Option<WorkOwner>,
}

#[derive(Debug, Default)]
struct SchedulerWaveOverlay {
    resolve: FairLane,
    verify: FairLane,
}

impl FairLane {
    fn contains(&self, owner: WorkOwner, key: &QueueKey) -> bool {
        self.by_owner
            .get(&owner)
            .is_some_and(|entries| entries.contains(key))
    }

    fn insert(&mut self, owner: WorkOwner, key: QueueKey) {
        let class = key.class();
        self.by_owner.entry(owner).or_default().insert(key);
        match class {
            VerifyCycleClass::Small => {
                self.small_owners.insert(owner);
            }
            VerifyCycleClass::Large => {}
        }
    }

    fn remove(&mut self, owner: WorkOwner, key: &QueueKey) {
        let remove_owner = self.by_owner.get_mut(&owner).is_some_and(|entries| {
            entries.remove(key);
            if entries.small.is_empty() {
                self.small_owners.remove(&owner);
            }
            entries.is_empty()
        });
        if remove_owner {
            self.by_owner.remove(&owner);
            self.small_owners.remove(&owner);
        }
    }

    fn owner_is_eligible(
        &self,
        lane: QueueLane,
        capability: VerifyCapability,
        owner: WorkOwner,
        staged: &BTreeMap<StagedSchedulerSlotKey, StagedSchedulerMarker>,
    ) -> bool {
        match lane.population(capability) {
            QueuePopulation::All => self.by_owner.get(&owner),
            QueuePopulation::SmallOnly if self.small_owners.contains(&owner) => {
                self.by_owner.get(&owner)
            }
            QueuePopulation::SmallOnly => None,
        }
        .and_then(|queue| queue.head(lane, capability, staged))
        .is_some()
    }

    fn raw_next_owner(
        &self,
        lane: QueueLane,
        capability: VerifyCapability,
        cursor: Option<WorkOwner>,
    ) -> Option<WorkOwner> {
        match lane.population(capability) {
            QueuePopulation::All => {
                let next = cursor.and_then(|cursor| {
                    self.by_owner
                        .range((Excluded(cursor), Unbounded))
                        .next()
                        .map(|(owner, _)| *owner)
                });
                next.or_else(|| self.by_owner.first_key_value().map(|(owner, _)| *owner))
            }
            QueuePopulation::SmallOnly => {
                let next = cursor.and_then(|cursor| {
                    self.small_owners
                        .range((Excluded(cursor), Unbounded))
                        .next()
                        .copied()
                });
                next.or_else(|| self.small_owners.first().copied())
            }
        }
    }

    fn next_owner(
        &self,
        lane: QueueLane,
        capability: VerifyCapability,
        cursor: Option<WorkOwner>,
        staged: &BTreeMap<StagedSchedulerSlotKey, StagedSchedulerMarker>,
    ) -> Option<WorkOwner> {
        let mut cursor = cursor;
        for _ in 0..self.owner_count(lane, capability) {
            let owner = self.raw_next_owner(lane, capability, cursor)?;
            if self.owner_is_eligible(lane, capability, owner, staged) {
                return Some(owner);
            }
            cursor = Some(owner);
        }
        None
    }

    fn next(
        &self,
        lane: QueueLane,
        capability: VerifyCapability,
        staged: &BTreeMap<StagedSchedulerSlotKey, StagedSchedulerMarker>,
    ) -> Option<(WorkOwner, &QueueKey)> {
        // The first cut may prefer Trusted. Every committed checkout then
        // advances one shared owner ring, so newly queued Remote or Trusted
        // work receives service within one bounded owner traversal while a
        // sole owner can still borrow every global slot.
        let owner = if self.owner_cursor.is_none()
            && self.owner_is_eligible(lane, capability, WorkOwner::Trusted, staged)
        {
            WorkOwner::Trusted
        } else {
            self.next_owner(lane, capability, self.owner_cursor, staged)?
        };
        let key = self.by_owner.get(&owner)?.head(lane, capability, staged)?;
        Some((owner, key))
    }

    fn overlay_owner_is_eligible(
        &self,
        overlay: &Self,
        lane: QueueLane,
        capability: VerifyCapability,
        owner: WorkOwner,
        staged: &BTreeMap<StagedSchedulerSlotKey, StagedSchedulerMarker>,
    ) -> bool {
        self.owner_is_eligible(lane, capability, owner, staged)
            || overlay.owner_is_eligible(lane, capability, owner, staged)
    }

    fn overlay_next_owner(
        &self,
        overlay: &Self,
        lane: QueueLane,
        capability: VerifyCapability,
        cursor: Option<WorkOwner>,
        staged: &BTreeMap<StagedSchedulerSlotKey, StagedSchedulerMarker>,
    ) -> Option<WorkOwner> {
        let choose = |left: Option<WorkOwner>, right: Option<WorkOwner>| match (left, right) {
            (Some(left), Some(right)) => Some(std::cmp::min(left, right)),
            (Some(owner), None) | (None, Some(owner)) => Some(owner),
            (None, None) => None,
        };
        let raw_next = |cursor: Option<WorkOwner>| {
            let next = match lane.population(capability) {
                QueuePopulation::All => choose(
                    cursor.and_then(|cursor| {
                        self.by_owner
                            .range((Excluded(cursor), Unbounded))
                            .next()
                            .map(|(owner, _)| *owner)
                    }),
                    cursor.and_then(|cursor| {
                        overlay
                            .by_owner
                            .range((Excluded(cursor), Unbounded))
                            .next()
                            .map(|(owner, _)| *owner)
                    }),
                ),
                QueuePopulation::SmallOnly => choose(
                    cursor.and_then(|cursor| {
                        self.small_owners
                            .range((Excluded(cursor), Unbounded))
                            .next()
                            .copied()
                    }),
                    cursor.and_then(|cursor| {
                        overlay
                            .small_owners
                            .range((Excluded(cursor), Unbounded))
                            .next()
                            .copied()
                    }),
                ),
            };
            next.or_else(|| match lane.population(capability) {
                QueuePopulation::All => choose(
                    self.by_owner.first_key_value().map(|(owner, _)| *owner),
                    overlay.by_owner.first_key_value().map(|(owner, _)| *owner),
                ),
                QueuePopulation::SmallOnly => choose(
                    self.small_owners.first().copied(),
                    overlay.small_owners.first().copied(),
                ),
            })
        };
        let mut cursor = cursor;
        let bound = self
            .owner_count(lane, capability)
            .saturating_add(overlay.owner_count(lane, capability));
        for _ in 0..bound {
            let owner = raw_next(cursor)?;
            if self.overlay_owner_is_eligible(overlay, lane, capability, owner, staged) {
                return Some(owner);
            }
            cursor = Some(owner);
        }
        None
    }

    fn overlay_head_excluding<'lane>(
        &'lane self,
        overlay: &'lane Self,
        lane: QueueLane,
        capability: VerifyCapability,
        owner: WorkOwner,
        excluded_versions: &[EntryVersion],
        staged: &BTreeMap<StagedSchedulerSlotKey, StagedSchedulerMarker>,
    ) -> Option<&'lane QueueKey> {
        let current = self
            .by_owner
            .get(&owner)
            .and_then(|queue| queue.head_excluding(lane, capability, excluded_versions, staged));
        let added = overlay
            .by_owner
            .get(&owner)
            .and_then(|queue| queue.head_excluding(lane, capability, excluded_versions, staged));
        match (lane, current, added) {
            (_, Some(current), None) => Some(current),
            (_, None, Some(added)) => Some(added),
            (QueueLane::Resolve, Some(current), Some(added)) => Some(std::cmp::min(current, added)),
            (QueueLane::Verify, Some(current), Some(added)) => Some(std::cmp::max(current, added)),
            (_, None, None) => None,
        }
    }

    fn next_excluding_with_overlay<'lane>(
        &'lane self,
        overlay: &'lane Self,
        lane: QueueLane,
        capability: VerifyCapability,
        cursor: Option<WorkOwner>,
        excluded_versions: &[EntryVersion],
        staged: &BTreeMap<StagedSchedulerSlotKey, StagedSchedulerMarker>,
    ) -> Option<(WorkOwner, &'lane QueueKey)> {
        if cursor.is_none()
            && self.overlay_owner_is_eligible(overlay, lane, capability, WorkOwner::Trusted, staged)
            && let Some(key) = self.overlay_head_excluding(
                overlay,
                lane,
                capability,
                WorkOwner::Trusted,
                excluded_versions,
                staged,
            )
        {
            return Some((WorkOwner::Trusted, key));
        }

        let owner_count = self
            .owner_count(lane, capability)
            .checked_add(overlay.owner_count(lane, capability))?;
        let mut cursor = cursor;
        for _ in 0..owner_count {
            let owner = self.overlay_next_owner(overlay, lane, capability, cursor, staged)?;
            if let Some(key) = self.overlay_head_excluding(
                overlay,
                lane,
                capability,
                owner,
                excluded_versions,
                staged,
            ) {
                return Some((owner, key));
            }
            cursor = Some(owner);
        }
        None
    }

    fn owner_count(&self, lane: QueueLane, capability: VerifyCapability) -> usize {
        match lane.population(capability) {
            QueuePopulation::All => self.by_owner.len(),
            QueuePopulation::SmallOnly => self.small_owners.len(),
        }
    }
}

#[derive(Debug)]
pub(super) struct FairFrontier {
    resolve: FairLane,
    verify: FairLane,
    ready: BTreeSet<ReadyKey>,
    ready_reserved: BTreeMap<ReadyKey, ReadyReservationEntry>,
    staged_visibility: BTreeMap<StagedSchedulerSlotKey, StagedSchedulerMarker>,
    hidden_cursor_free_queue_stages: usize,
    fairness_stage_active: bool,
    fairness_stage_waiters: usize,
    stage_gate_changed: Arc<Condvar>,
    compute_queue_revision: ComputeQueueRevision,
    verify_order: VerifyOrder,
}

/// Bounded virtual checkout cut. Selected versions are globally unique within
/// one authority generation, so this overlay can remove up to one worker wave
/// without cloning the scheduler or publishing a second queue authority.
pub(super) struct SchedulerWaveCursor {
    selected_versions: Vec<EntryVersion>,
    resolve_cursor: Option<WorkOwner>,
    verify_cursor: Option<WorkOwner>,
    compute_queue_revision: ComputeQueueRevision,
}

/// Mutable Plan-only view of the committed scheduler plus a bounded set of
/// owner-local settlement additions. Candidate probes do not advance
/// fairness; only consuming a selected ticket does. This lets the exchange
/// apply resource and dependency eligibility in canonical checkout order
/// without cloning the committed frontier.
pub(super) struct SchedulerExchangeWave {
    frontier: Arc<Mutex<FairFrontier>>,
    overlay: SchedulerWaveOverlay,
    cursor: SchedulerWaveCursor,
}

#[cfg(test)]
impl FairFrontier {
    pub(in crate::authority) fn invalidate_compute_exchange_cursor_for_foundation(
        &mut self,
        version: EntryVersion,
    ) {
        self.compute_queue_revision =
            if self.compute_queue_revision == ComputeQueueRevision::QueueInsert(version) {
                ComputeQueueRevision::QueueRemove(version)
            } else {
                ComputeQueueRevision::QueueInsert(version)
            };
    }
}

impl SchedulerExchangeWave {
    pub(super) fn after<'entry>(
        frontier: Arc<Mutex<FairFrontier>>,
        settled: impl IntoIterator<Item = &'entry OwnedTx>,
        selection_bound: usize,
    ) -> Result<Self, SchedulerError> {
        let mut overlay = SchedulerWaveOverlay::default();
        let cursor = {
            let committed = frontier.lock();
            for owner in settled {
                match committed.slot(owner)? {
                    Some(SchedulerSlot::Queue { lane, owner, key }) => {
                        let frontier = match lane {
                            QueueLane::Resolve => &mut overlay.resolve,
                            QueueLane::Verify => &mut overlay.verify,
                        };
                        if frontier.contains(owner, &key) {
                            return Err(SchedulerError::Projection);
                        }
                        frontier.insert(owner, key);
                    }
                    Some(SchedulerSlot::Ready(_)) | None => {}
                }
            }
            committed.checkout_wave(selection_bound)?
        };
        Ok(Self {
            frontier,
            overlay,
            cursor,
        })
    }

    pub(super) fn next(&self, permit: super::state::WorkPermit) -> Option<CheckoutTicket> {
        self.frontier
            .lock()
            .next_queued_in_wave_with_overlay(&self.cursor, permit, &self.overlay)
    }

    pub(super) fn next_after(
        &self,
        permit: super::state::WorkPermit,
        owner: WorkOwner,
    ) -> Option<CheckoutTicket> {
        self.frontier.lock().next_queued_after_in_wave_with_overlay(
            &self.cursor,
            permit,
            owner,
            &self.overlay,
        )
    }

    pub(super) fn owner_count(
        &self,
        permit: super::state::WorkPermit,
    ) -> Result<usize, SchedulerError> {
        let lane = QueueLane::for_permit(permit);
        let capability = QueueLane::capability(permit);
        let frontier = self.frontier.lock();
        let (frontier, overlay) = match lane {
            QueueLane::Resolve => (&frontier.resolve, &self.overlay.resolve),
            QueueLane::Verify => (&frontier.verify, &self.overlay.verify),
        };
        frontier
            .owner_count(lane, capability)
            .checked_add(overlay.owner_count(lane, capability))
            .ok_or(SchedulerError::Arithmetic)
    }

    pub(super) fn select(&mut self, ticket: &CheckoutTicket) -> Result<(), SchedulerError> {
        self.cursor.select(ticket)
    }

    pub(super) fn into_cursor(self) -> SchedulerWaveCursor {
        self.cursor
    }
}

impl SchedulerWaveCursor {
    fn lane_cursor(&self, lane: QueueLane) -> Option<WorkOwner> {
        match lane {
            QueueLane::Resolve => self.resolve_cursor,
            QueueLane::Verify => self.verify_cursor,
        }
    }

    pub(super) fn select(&mut self, ticket: &CheckoutTicket) -> Result<(), SchedulerError> {
        match self.selected_versions.binary_search(&ticket.version()) {
            Ok(_) => return Err(SchedulerError::Projection),
            Err(position) => self.selected_versions.insert(position, ticket.version()),
        }
        match ticket.lane {
            QueueLane::Resolve => self.resolve_cursor = Some(ticket.owner),
            QueueLane::Verify => self.verify_cursor = Some(ticket.owner),
        }
        Ok(())
    }
}

impl FairFrontier {
    pub(super) fn new(verify_order: VerifyOrder) -> Self {
        Self {
            resolve: FairLane::default(),
            verify: FairLane::default(),
            ready: BTreeSet::new(),
            ready_reserved: BTreeMap::new(),
            staged_visibility: BTreeMap::new(),
            hidden_cursor_free_queue_stages: 0,
            fairness_stage_active: false,
            fairness_stage_waiters: 0,
            stage_gate_changed: Arc::new(Condvar::new()),
            compute_queue_revision: ComputeQueueRevision::Initial,
            verify_order,
        }
    }

    pub(super) fn verify_order(&self) -> VerifyOrder {
        self.verify_order
    }

    fn slot(&self, owner: &OwnedTx) -> Result<Option<SchedulerSlot>, SchedulerError> {
        let OwnedTx::PreAccepted(entry) = owner else {
            return Ok(None);
        };
        let record = &entry.record;
        let owner = WorkOwner::from_source(entry.source);
        let slot = match &entry.phase {
            PreAcceptedPhase::Queued(super::state::QueuedWork::Resolve) => SchedulerSlot::Queue {
                lane: QueueLane::Resolve,
                owner,
                key: QueueKey::Resolve(ResolveKey {
                    source: entry.source.into(),
                    arrival: record.arrival,
                    hash: record.identity.raw.clone(),
                    version: record.version,
                }),
            },
            PreAcceptedPhase::Queued(super::state::QueuedWork::Verify(resolved)) => {
                let serialized_bytes = u64::try_from(resolved.payload().serialized_bytes())
                    .map_err(|_| SchedulerError::Arithmetic)?;
                if serialized_bytes == 0 {
                    return Err(SchedulerError::Projection);
                }
                SchedulerSlot::Queue {
                    lane: QueueLane::Verify,
                    owner,
                    key: QueueKey::Verify(VerifyKey {
                        source: entry.source.into(),
                        order: self.verify_order,
                        fee: resolved.payload().fee().as_u64(),
                        serialized_bytes,
                        arrival: record.arrival,
                        hash: record.identity.raw.clone(),
                        version: record.version,
                        class: resolved.verify_class(),
                    }),
                }
            }
            PreAcceptedPhase::Ready(_) => SchedulerSlot::Ready(ReadyKey::from_ready(entry)?),
            PreAcceptedPhase::Computing(_) | PreAcceptedPhase::Waiting(_) => return Ok(None),
        };
        Ok(Some(slot))
    }

    pub(super) fn plan_replace(
        &self,
        before: Option<&OwnedTx>,
        after: Option<&OwnedTx>,
        checkout: Option<CheckoutTicket>,
    ) -> Result<SchedulerDelta, SchedulerError> {
        let before = before.map(|owner| self.slot(owner)).transpose()?.flatten();
        let after = after.map(|owner| self.slot(owner)).transpose()?.flatten();
        if before.as_ref().is_some_and(|slot| !self.contains(slot)) {
            return Err(SchedulerError::Projection);
        }
        if after
            .as_ref()
            .is_some_and(|slot| Some(slot) != before.as_ref() && self.contains(slot))
        {
            return Err(SchedulerError::Projection);
        }
        let owner_cursor = match checkout {
            Some(ticket) => {
                let selected = SchedulerSlot::Queue {
                    lane: ticket.lane,
                    owner: ticket.owner,
                    key: ticket.key,
                };
                if before.as_ref() != Some(&selected) || after.is_some() {
                    return Err(SchedulerError::Projection);
                }
                Some((ticket.lane, ticket.owner))
            }
            None => None,
        };
        Ok(SchedulerDelta {
            before,
            after,
            owner_cursor,
        })
    }

    pub(super) fn plan_batch<'entry>(
        &self,
        changes: impl IntoIterator<Item = (Option<&'entry OwnedTx>, Option<&'entry OwnedTx>)>,
    ) -> Result<SchedulerBatchDelta, SchedulerError> {
        self.plan_batch_with_missing_before(changes, SchedulerError::Projection)
    }

    fn plan_batch_with_missing_before<'entry>(
        &self,
        changes: impl IntoIterator<Item = (Option<&'entry OwnedTx>, Option<&'entry OwnedTx>)>,
        missing_before: SchedulerError,
    ) -> Result<SchedulerBatchDelta, SchedulerError> {
        let mut input = changes.into_iter();
        let mut changes = Vec::new();
        if let Some(capacity) = input.size_hint().1 {
            changes
                .try_reserve_exact(capacity)
                .map_err(|_| SchedulerError::Allocation)?;
        }
        for (before, after) in input.by_ref() {
            if changes.len() == changes.capacity() {
                changes
                    .try_reserve(1)
                    .map_err(|_| SchedulerError::Allocation)?;
            }
            changes.push(SchedulerDelta {
                before: before.map(|owner| self.slot(owner)).transpose()?.flatten(),
                after: after.map(|owner| self.slot(owner)).transpose()?.flatten(),
                owner_cursor: None,
            });
        }

        let mut removed = Vec::new();
        let mut added = Vec::new();
        removed
            .try_reserve_exact(changes.len())
            .map_err(|_| SchedulerError::Allocation)?;
        added
            .try_reserve_exact(changes.len())
            .map_err(|_| SchedulerError::Allocation)?;
        for change in &changes {
            match &change.before {
                Some(before) if !self.contains(before) => return Err(missing_before),
                Some(before) => removed.push(before.clone()),
                _ => {}
            }
            if let Some(after) = &change.after {
                added.push(after.clone());
            }
        }
        removed.sort_unstable();
        added.sort_unstable();
        if removed
            .array_windows::<2>()
            .any(|[left, right]| left == right)
            || added
                .array_windows::<2>()
                .any(|[left, right]| left == right)
        {
            return Err(SchedulerError::Projection);
        }
        if added
            .iter()
            .any(|slot| self.contains(slot) && removed.binary_search(slot).is_err())
        {
            return Err(SchedulerError::Projection);
        }
        Ok(SchedulerBatchDelta {
            removed,
            added,
            compute_queue_revision: None,
            resolve_cursor: None,
            verify_cursor: None,
        })
    }

    /// Compile final owner projections together with the fairness cursors
    /// produced by a sealed virtual worker wave. The wave is stack-owned Plan
    /// evidence; only this consumption point can publish its cursor advance.
    pub(super) fn plan_exchange_batch<'entry>(
        &self,
        changes: impl IntoIterator<Item = (Option<&'entry OwnedTx>, Option<&'entry OwnedTx>)>,
        cursor: SchedulerWaveCursor,
    ) -> Result<SchedulerBatchDelta, SchedulerError> {
        if cursor.compute_queue_revision != self.compute_queue_revision {
            return Err(SchedulerError::Stale);
        }
        let mut delta = self.plan_batch_with_missing_before(changes, SchedulerError::Stale)?;
        let target = delta.compute_queue_revision_after(cursor.compute_queue_revision);
        delta.compute_queue_revision = Some(ComputeQueueRevisionChange {
            expected: cursor.compute_queue_revision,
            target,
        });
        delta.resolve_cursor =
            (cursor.resolve_cursor != self.resolve.owner_cursor).then_some(SchedulerCursorChange {
                expected: self.resolve.owner_cursor,
                target: cursor.resolve_cursor,
            });
        delta.verify_cursor =
            (cursor.verify_cursor != self.verify.owner_cursor).then_some(SchedulerCursorChange {
                expected: self.verify.owner_cursor,
                target: cursor.verify_cursor,
            });
        Ok(delta)
    }

    pub(super) fn checkout_wave(
        &self,
        selection_bound: usize,
    ) -> Result<SchedulerWaveCursor, SchedulerError> {
        let mut selected_versions = Vec::new();
        selected_versions
            .try_reserve(selection_bound)
            .map_err(|_| SchedulerError::Allocation)?;
        Ok(SchedulerWaveCursor {
            selected_versions,
            resolve_cursor: self.resolve.owner_cursor,
            verify_cursor: self.verify.owner_cursor,
            compute_queue_revision: self.compute_queue_revision,
        })
    }

    fn next_queued_in_wave_with_overlay(
        &self,
        wave: &SchedulerWaveCursor,
        permit: super::state::WorkPermit,
        overlay: &SchedulerWaveOverlay,
    ) -> Option<CheckoutTicket> {
        let lane = QueueLane::for_permit(permit);
        let capability = QueueLane::capability(permit);
        let (frontier, added) = match lane {
            QueueLane::Resolve => (&self.resolve, &overlay.resolve),
            QueueLane::Verify => (&self.verify, &overlay.verify),
        };
        frontier
            .next_excluding_with_overlay(
                added,
                lane,
                capability,
                wave.lane_cursor(lane),
                &wave.selected_versions,
                &self.staged_visibility,
            )
            .map(|(owner, key)| CheckoutTicket {
                lane,
                owner,
                key: key.clone(),
            })
    }

    fn next_queued_after_in_wave_with_overlay(
        &self,
        wave: &SchedulerWaveCursor,
        permit: super::state::WorkPermit,
        owner: WorkOwner,
        overlay: &SchedulerWaveOverlay,
    ) -> Option<CheckoutTicket> {
        let lane = QueueLane::for_permit(permit);
        let capability = QueueLane::capability(permit);
        let (frontier, added) = match lane {
            QueueLane::Resolve => (&self.resolve, &overlay.resolve),
            QueueLane::Verify => (&self.verify, &overlay.verify),
        };
        frontier
            .next_excluding_with_overlay(
                added,
                lane,
                capability,
                Some(owner),
                &wave.selected_versions,
                &self.staged_visibility,
            )
            .map(|(owner, key)| CheckoutTicket {
                lane,
                owner,
                key: key.clone(),
            })
    }

    pub(super) fn wake_projection(&self) -> SchedulerWakeProjection {
        SchedulerWakeProjection {
            resolve: self
                .resolve
                .next(
                    QueueLane::Resolve,
                    VerifyCapability::Any,
                    &self.staged_visibility,
                )
                .map(|(_, key)| key.version()),
            verify_small: self
                .verify
                .next(
                    QueueLane::Verify,
                    VerifyCapability::SmallCycleOnly,
                    &self.staged_visibility,
                )
                .map(|(_, key)| key.version()),
            verify_any: self
                .verify
                .next(
                    QueueLane::Verify,
                    VerifyCapability::Any,
                    &self.staged_visibility,
                )
                .map(|(_, key)| key.version()),
            ready: self
                .ready
                .iter()
                .rev()
                .find(|key| {
                    self.logical_ready_contains(key) && !self.ready_reserved.contains_key(*key)
                })
                .map(ReadyKey::version),
        }
    }

    #[cfg(test)]
    pub(super) fn ready(&self) -> Result<Vec<(RawTxHash, EntryVersion)>, SchedulerError> {
        let count = self
            .ready
            .iter()
            .rev()
            .filter(|key| self.logical_ready_contains(key))
            .take(MAX_READY_BATCH)
            .count();
        let mut ready = Vec::new();
        ready
            .try_reserve_exact(count)
            .map_err(|_| SchedulerError::Allocation)?;
        ready.extend(
            self.ready
                .iter()
                .rev()
                .filter(|key| self.logical_ready_contains(key))
                .take(MAX_READY_BATCH)
                .map(|key| (key.hash().clone(), key.version())),
        );
        Ok(ready)
    }

    pub(super) fn apply(&mut self, delta: SchedulerDelta) {
        let compute_queue_revision = if delta.before == delta.after {
            self.compute_queue_revision
        } else {
            let mut revision = self.compute_queue_revision;
            let before = delta
                .before
                .as_ref()
                .and_then(SchedulerSlot::compute_queue_version);
            let after = delta
                .after
                .as_ref()
                .and_then(SchedulerSlot::compute_queue_version);
            match (before, after) {
                (Some(before), Some(after)) if before == after => {
                    revision = ComputeQueueRevision::QueueReplace(after);
                }
                (before, after) => {
                    if let Some(version) = before {
                        revision = ComputeQueueRevision::QueueRemove(version);
                    }
                    if let Some(version) = after {
                        revision = ComputeQueueRevision::QueueInsert(version);
                    }
                }
            }
            revision
        };
        if let Some(before) = delta.before {
            self.remove_physical(before);
        }
        if let Some(after) = delta.after {
            self.insert_physical(after);
        }
        self.compute_queue_revision = compute_queue_revision;
        if let Some((lane, owner)) = delta.owner_cursor {
            match lane {
                QueueLane::Resolve => self.resolve.owner_cursor = Some(owner),
                QueueLane::Verify => self.verify.owner_cursor = Some(owner),
            }
        }
        let _ = self.reap_slot_claims();
        self.refresh_slot_claims();
    }

    pub(super) fn apply_batch(&mut self, delta: SchedulerBatchDelta) {
        let compute_queue_revision = delta.compute_queue_revision.map_or_else(
            || delta.compute_queue_revision_after(self.compute_queue_revision),
            |change| change.target,
        );
        // A batch is a set transition, independent of the caller's change
        // order. Remove the complete old projection before publishing any new
        // slot so an exchange can never be lost to BTreeSet insertion order.
        for slot in delta.removed {
            self.remove_physical(slot);
        }
        for slot in delta.added {
            self.insert_physical(slot);
        }
        self.compute_queue_revision = compute_queue_revision;
        if let Some(cursor) = delta.resolve_cursor {
            self.resolve.owner_cursor = cursor.target;
        }
        if let Some(cursor) = delta.verify_cursor {
            self.verify.owner_cursor = cursor.target;
        }
        let _ = self.reap_slot_claims();
        self.refresh_slot_claims();
    }

    fn contains(&self, slot: &SchedulerSlot) -> bool {
        match slot {
            SchedulerSlot::Queue { lane, owner, key } => match lane {
                QueueLane::Resolve => self.resolve.contains(*owner, key),
                QueueLane::Verify => self.verify.contains(*owner, key),
            },
            SchedulerSlot::Ready(key) => self.logical_ready_contains(key),
        }
    }

    fn contains_physical(&self, slot: &SchedulerSlot) -> bool {
        match slot {
            SchedulerSlot::Queue { lane, owner, key } => match lane {
                QueueLane::Resolve => self.resolve.contains(*owner, key),
                QueueLane::Verify => self.verify.contains(*owner, key),
            },
            SchedulerSlot::Ready(key) => self.ready.contains(key),
        }
    }

    fn insert_physical(&mut self, slot: SchedulerSlot) {
        match slot {
            SchedulerSlot::Queue { lane, owner, key } => match lane {
                QueueLane::Resolve => self.resolve.insert(owner, key),
                QueueLane::Verify => self.verify.insert(owner, key),
            },
            SchedulerSlot::Ready(key) => {
                self.ready.insert(key);
            }
        }
    }

    fn remove_physical(&mut self, slot: SchedulerSlot) {
        self.remove_physical_ref(&slot);
    }

    fn remove_physical_ref(&mut self, slot: &SchedulerSlot) {
        match slot {
            SchedulerSlot::Queue { lane, owner, key } => match lane {
                QueueLane::Resolve => self.resolve.remove(*owner, key),
                QueueLane::Verify => self.verify.remove(*owner, key),
            },
            SchedulerSlot::Ready(key) => {
                match self.ready_reserved.get(key) {
                    Some(ReadyReservationEntry::Claimed(claim)) => claim.retire(),
                    Some(ReadyReservationEntry::Captured) => {
                        self.ready_reserved.remove(key);
                    }
                    None => {}
                }
                self.ready.remove(key);
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/support/scheduler.rs"]
pub(in crate::authority) mod test_support;

impl Ord for SchedulerSlot {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (
                Self::Queue {
                    lane: left_lane,
                    owner: left_owner,
                    key: left_key,
                },
                Self::Queue {
                    lane: right_lane,
                    owner: right_owner,
                    key: right_key,
                },
            ) => left_lane
                .cmp(right_lane)
                .then_with(|| left_owner.cmp(right_owner))
                .then_with(|| left_key.cmp(right_key)),
            (Self::Ready(left), Self::Ready(right)) => left.cmp(right),
            (Self::Queue { .. }, Self::Ready(_)) => Ordering::Less,
            (Self::Ready(_), Self::Queue { .. }) => Ordering::Greater,
        }
    }
}

impl PartialOrd for SchedulerSlot {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod compute_queue_revision_tests {
    use super::*;
    use ckb_types::packed::Byte32;

    #[cfg(feature = "profiling")]
    #[test]
    fn staged_scheduler_batch_remains_send_with_lifetime_profiling_enabled() {
        fn assert_send<T: Send>() {}
        assert_send::<StagedSchedulerBatch<'static>>();
    }

    fn resolve_slot(owner: WorkOwner, source: SourcePriority) -> SchedulerSlot {
        resolve_slot_with_version(owner, source, 91, 91)
    }

    fn resolve_slot_with_version(
        owner: WorkOwner,
        source: SourcePriority,
        nonce: u8,
        version: u128,
    ) -> SchedulerSlot {
        SchedulerSlot::Queue {
            lane: QueueLane::Resolve,
            owner,
            key: QueueKey::Resolve(ResolveKey {
                source,
                arrival: Arrival(0),
                hash: RawTxHash(Byte32::new([nonce; 32])),
                version: EntryVersion(version),
            }),
        }
    }

    fn insertion(slot: SchedulerSlot) -> SchedulerBatchDelta {
        SchedulerBatchDelta {
            removed: Vec::new(),
            added: vec![slot],
            compute_queue_revision: None,
            resolve_cursor: None,
            verify_cursor: None,
        }
    }

    #[test]
    fn same_version_queue_move_has_distinct_insert_replace_remove_revisions() {
        let mut frontier = FairFrontier::new(VerifyOrder::Arrival);
        let trusted = resolve_slot(WorkOwner::Trusted, SourcePriority::Proposal);
        let remote = resolve_slot(
            WorkOwner::Remote(PeerIndex::from(91usize)),
            SourcePriority::Remote,
        );
        frontier.apply(SchedulerDelta {
            before: None,
            after: Some(trusted.clone()),
            owner_cursor: None,
        });
        assert_eq!(
            frontier.compute_queue_revision,
            ComputeQueueRevision::QueueInsert(EntryVersion(91))
        );
        frontier.apply(SchedulerDelta {
            before: Some(trusted),
            after: Some(remote.clone()),
            owner_cursor: None,
        });
        assert_eq!(
            frontier.compute_queue_revision,
            ComputeQueueRevision::QueueReplace(EntryVersion(91))
        );
        frontier.apply(SchedulerDelta {
            before: Some(remote),
            after: None,
            owner_cursor: None,
        });
        assert_eq!(
            frontier.compute_queue_revision,
            ComputeQueueRevision::QueueRemove(EntryVersion(91))
        );
    }

    #[test]
    fn waiting_fairness_stage_blocks_later_queue_stages_until_prior_readers_drain() {
        const WAIT: std::time::Duration = std::time::Duration::from_secs(2);
        let frontier = Arc::new(Mutex::new(FairFrontier::new(VerifyOrder::Arrival)));
        let queued = resolve_slot_with_version(
            WorkOwner::Remote(PeerIndex::from(92usize)),
            SourcePriority::Remote,
            92,
            92,
        );
        let queued = StagedSchedulerBatch::stage_primary_replacements(&frontier, insertion(queued))
            .expect("the cursor-free queue insertion owns one hidden stage");
        let fairness = || SchedulerBatchDelta {
            removed: Vec::new(),
            added: Vec::new(),
            compute_queue_revision: Some(ComputeQueueRevisionChange {
                expected: ComputeQueueRevision::Initial,
                target: ComputeQueueRevision::Initial,
            }),
            resolve_cursor: Some(SchedulerCursorChange {
                expected: None,
                target: Some(WorkOwner::Trusted),
            }),
            verify_cursor: None,
        };
        let later = resolve_slot_with_version(
            WorkOwner::Remote(PeerIndex::from(95usize)),
            SourcePriority::Remote,
            95,
            95,
        );
        std::thread::scope(|scope| {
            let (fairness_acquired_tx, fairness_acquired_rx) = std::sync::mpsc::channel();
            let (release_fairness_tx, release_fairness_rx) = std::sync::mpsc::channel();
            let fairness_frontier = Arc::clone(&frontier);
            scope.spawn(move || {
                let stage = StagedSchedulerBatch::stage_primary_replacements(
                    &fairness_frontier,
                    fairness(),
                )
                .expect("the queued fairness writer acquires after prior readers drain");
                let _ = fairness_acquired_tx.send(());
                let _ = release_fairness_rx.recv();
                drop(stage);
            });
            let deadline = std::time::Instant::now() + WAIT;
            loop {
                if frontier.lock().fairness_stage_waiters == 1 {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "the fairness writer publishes its wait intent"
                );
                std::thread::yield_now();
            }

            let (later_acquired_tx, later_acquired_rx) = std::sync::mpsc::channel();
            let (release_later_tx, release_later_rx) = std::sync::mpsc::channel();
            let later_frontier = Arc::clone(&frontier);
            scope.spawn(move || {
                let stage = StagedSchedulerBatch::stage_primary_replacements(
                    &later_frontier,
                    insertion(later),
                )
                .expect("the later reader acquires only after the fairness writer");
                let _ = later_acquired_tx.send(());
                let _ = release_later_rx.recv();
                drop(stage);
            });
            assert!(fairness_acquired_rx.try_recv().is_err());
            assert!(later_acquired_rx.try_recv().is_err());

            drop(queued);
            fairness_acquired_rx
                .recv_timeout(WAIT)
                .expect("the waiting fairness writer wins after prior readers drain");
            assert!(
                later_acquired_rx.try_recv().is_err(),
                "a queued fairness writer prevents later reader barging"
            );
            release_fairness_tx
                .send(())
                .expect("release the fairness stage");
            later_acquired_rx
                .recv_timeout(WAIT)
                .expect("the later queue stage proceeds after fairness terminates");
            release_later_tx.send(()).expect("release the later stage");
        });
        let scheduler = frontier.lock();
        assert_eq!(scheduler.hidden_cursor_free_queue_stages, 0);
        assert!(!scheduler.fairness_stage_active);
        assert_eq!(scheduler.fairness_stage_waiters, 0);
        assert!(scheduler.staged_visibility.is_empty());
    }

    #[test]
    fn disjoint_queue_stages_publish_revision_in_actual_commit_order() {
        let frontier = Arc::new(Mutex::new(FairFrontier::new(VerifyOrder::Arrival)));
        let first = resolve_slot_with_version(
            WorkOwner::Remote(PeerIndex::from(93usize)),
            SourcePriority::Remote,
            93,
            93,
        );
        let second = resolve_slot_with_version(
            WorkOwner::Remote(PeerIndex::from(94usize)),
            SourcePriority::Remote,
            94,
            94,
        );
        let old_wave = frontier
            .lock()
            .checkout_wave(0)
            .expect("the empty pre-publication wave is bounded");
        let first = StagedSchedulerBatch::stage_primary_replacements(&frontier, insertion(first))
            .expect("the first disjoint queue stage is hidden");
        let second = StagedSchedulerBatch::stage_primary_replacements(&frontier, insertion(second))
            .expect("the second disjoint queue stage is independently hidden");
        let entries = crate::authority::shard::ShardedOwnerMap::new(
            crate::authority::shard::AuthorityShardRouter::new(),
        );
        second.activate_for_foundation(
            entries.write_cut(crate::authority::shard::ShardWriteSupport::default()),
        );
        assert!(matches!(
            frontier
                .lock()
                .plan_exchange_batch(std::iter::empty(), old_wave),
            Err(SchedulerError::Stale)
        ));
        assert_eq!(
            frontier.lock().compute_queue_revision,
            ComputeQueueRevision::QueueInsert(EntryVersion(94))
        );
        first.activate_for_foundation(
            entries.write_cut(crate::authority::shard::ShardWriteSupport::default()),
        );
        let scheduler = frontier.lock();
        assert_eq!(
            scheduler.compute_queue_revision,
            ComputeQueueRevision::QueueInsert(EntryVersion(93))
        );
        assert_eq!(scheduler.hidden_cursor_free_queue_stages, 0);
        assert!(!scheduler.fairness_stage_active);
        assert!(scheduler.staged_visibility.is_empty());
    }
}

#[cfg(test)]
mod ready_slot_claim_tests {
    use super::*;
    use ckb_types::packed::Byte32;

    fn key(fee: u64, nonce: u8, version: u128) -> ReadyKey {
        ReadyKey {
            source: SourcePriority::Remote,
            fee,
            serialized_bytes: 1,
            arrival: Arrival(version),
            hash: RawTxHash(Byte32::new([nonce; 32])),
            version: EntryVersion(version),
        }
    }

    fn apply_insert(frontier: &Arc<Mutex<FairFrontier>>, key: &ReadyKey) {
        frontier.lock().apply(SchedulerDelta {
            before: None,
            after: Some(SchedulerSlot::Ready(key.clone())),
            owner_cursor: None,
        });
    }

    fn apply_remove(frontier: &Arc<Mutex<FairFrontier>>, key: &ReadyKey) {
        frontier.lock().apply(SchedulerDelta {
            before: Some(SchedulerSlot::Ready(key.clone())),
            after: None,
            owner_cursor: None,
        });
    }

    fn exact_removal(key: &ReadyKey) -> SchedulerBatchDelta {
        SchedulerBatchDelta {
            removed: vec![SchedulerSlot::Ready(key.clone())],
            added: Vec::new(),
            compute_queue_revision: None,
            resolve_cursor: None,
            verify_cursor: None,
        }
    }

    fn ready_insertion(key: &ReadyKey) -> SchedulerBatchDelta {
        SchedulerBatchDelta {
            removed: Vec::new(),
            added: vec![SchedulerSlot::Ready(key.clone())],
            compute_queue_revision: None,
            resolve_cursor: None,
            verify_cursor: None,
        }
    }

    fn ready_to_resolve(
        key: &ReadyKey,
        version: EntryVersion,
    ) -> (SchedulerSlot, SchedulerBatchDelta) {
        let resolve = SchedulerSlot::Queue {
            lane: QueueLane::Resolve,
            owner: WorkOwner::Remote(PeerIndex::from(1usize)),
            key: QueueKey::Resolve(ResolveKey {
                source: key.source,
                arrival: key.arrival,
                hash: key.hash.clone(),
                version,
            }),
        };
        (
            resolve.clone(),
            SchedulerBatchDelta {
                removed: vec![SchedulerSlot::Ready(key.clone())],
                added: vec![resolve],
                compute_queue_revision: None,
                resolve_cursor: None,
                verify_cursor: None,
            },
        )
    }

    fn capture_slot(frontier: &Arc<Mutex<FairFrontier>>) -> ReadySlotReservation {
        let reservation = ReadyReservation::capture(frontier)
            .expect("the scheduler capture is coherent")
            .expect("one Ready key is available");
        let (mut slots, remainder) = reservation
            .try_split_prefix(1)
            .unwrap_or_else(|_| panic!("one Ready key splits into a worker slot"));
        assert!(remainder.is_none());
        slots.pop().expect("one worker slot exists")
    }

    #[test]
    fn stronger_blocker_removal_refreshes_an_invalid_slot_to_fresh() {
        let frontier = Arc::new(Mutex::new(FairFrontier::new(VerifyOrder::Arrival)));
        let weak = key(1, 1, 1);
        let strong = key(2, 2, 2);
        apply_insert(&frontier, &weak);
        let slot = capture_slot(&frontier);

        apply_insert(&frontier, &strong);
        assert_eq!(slot.claim.state(), READY_SLOT_INVALID);
        apply_remove(&frontier, &strong);
        assert_eq!(slot.claim.state(), READY_SLOT_FRESH);

        let delta = exact_removal(&weak);
        assert!(slot.prestate_is_fresh(&frontier, &delta));
        slot.activate(&frontier, delta);
        assert!(
            ReadyReservation::capture(&frontier)
                .expect("committed slot reaping stays coherent")
                .is_none()
        );
        assert_eq!(
            frontier.lock().ready_physical_counts_for_foundation(),
            (0, 0, 0)
        );
    }

    #[test]
    fn hidden_stronger_stage_neither_suppresses_nor_invalidates_weaker_work() {
        let frontier = Arc::new(Mutex::new(FairFrontier::new(VerifyOrder::Arrival)));
        let weak = key(1, 11, 11);
        let strong = key(2, 12, 12);
        apply_insert(&frontier, &weak);

        let staged =
            StagedSchedulerBatch::stage_primary_insertions(&frontier, ready_insertion(&strong))
                .expect("the stronger Ready row stages without becoming visible");
        assert_eq!(
            frontier.lock().wake_projection().ready,
            Some(weak.version())
        );
        let reservation = ReadyReservation::capture(&frontier)
            .expect("a hidden stronger row is skipped without suppressing committed work")
            .expect("the weaker committed Ready row remains selectable");
        assert_eq!(
            reservation.candidates().next(),
            Some((weak.hash(), weak.version()))
        );
        drop(reservation);

        drop(staged);
        assert_eq!(
            frontier.lock().wake_projection().ready,
            Some(weak.version())
        );
        let slot = capture_slot(&frontier);
        let delta = exact_removal(&weak);
        assert!(slot.prestate_is_fresh(&frontier, &delta));
        slot.activate(&frontier, delta);
        assert!(
            ReadyReservation::capture(&frontier)
                .expect("the committed weaker slot reaps coherently")
                .is_none()
        );
        assert_eq!(
            frontier.lock().ready_physical_counts_for_foundation(),
            (0, 0, 0)
        );
    }

    #[test]
    fn committing_weaker_claim_linearizes_before_a_later_hidden_stage() {
        let frontier = Arc::new(Mutex::new(FairFrontier::new(VerifyOrder::Arrival)));
        let weak = key(1, 13, 13);
        let strong = key(2, 14, 14);
        apply_insert(&frontier, &weak);
        let slot = capture_slot(&frontier);
        let delta = exact_removal(&weak);
        assert!(slot.prestate_is_fresh(&frontier, &delta));

        let staged =
            StagedSchedulerBatch::stage_primary_insertions(&frontier, ready_insertion(&strong))
                .expect("the later stronger Ready row stages");
        assert_eq!(slot.claim.state(), READY_SLOT_COMMITTING);
        slot.activate(&frontier, delta);
        drop(staged);
        assert!(
            ReadyReservation::capture(&frontier)
                .expect("the earlier committed slot reaps coherently")
                .is_none()
        );
        assert_eq!(
            frontier.lock().ready_physical_counts_for_foundation(),
            (0, 0, 0)
        );
    }

    #[test]
    fn batch_ready_prefix_is_not_rechecked_after_its_owner_linearization() {
        let frontier = Arc::new(Mutex::new(FairFrontier::new(VerifyOrder::Arrival)));
        let weak = key(1, 15, 15);
        let strong = key(2, 16, 16);
        apply_insert(&frontier, &weak);
        let reservation = ReadyReservation::capture(&frontier)
            .expect("the initial Ready capture is coherent")
            .expect("the weaker row is available");
        let weak_stage = StagedSchedulerBatch::stage_reserved_ready_batch(
            &frontier,
            exact_removal(&weak),
            reservation,
        )
        .expect("the weaker batch stages its exact Ready removal");

        // This is the batch Ready linearization point. A disjoint stronger
        // owner may commit after this read but before scheduler publication;
        // rechecking the prefix in that Apply tail would panic after mutation.
        assert!(weak_stage.prestate_is_fresh());
        let strong_stage =
            StagedSchedulerBatch::stage_primary_insertions(&frontier, ready_insertion(&strong))
                .expect("the later disjoint stronger owner stages independently");
        let entries = crate::authority::shard::ShardedOwnerMap::new(
            crate::authority::shard::AuthorityShardRouter::new(),
        );
        strong_stage.activate_for_foundation(
            entries.write_cut(crate::authority::shard::ShardWriteSupport::default()),
        );
        weak_stage.activate_for_foundation(
            entries.write_cut(crate::authority::shard::ShardWriteSupport::default()),
        );

        let next = ReadyReservation::capture(&frontier)
            .expect("the two ordered publications leave one coherent Ready row")
            .expect("the later stronger row remains available");
        assert_eq!(
            next.candidates().next(),
            Some((strong.hash(), strong.version()))
        );
    }

    #[test]
    fn reserved_ready_reresolution_rechecks_selection_not_its_hidden_addition() {
        let frontier = Arc::new(Mutex::new(FairFrontier::new(VerifyOrder::Arrival)));
        let ready = key(1, 17, 17);
        apply_insert(&frontier, &ready);
        let reservation = ReadyReservation::capture(&frontier)
            .expect("the Ready capture is coherent")
            .expect("the Ready row is available");
        let (resolve, delta) = ready_to_resolve(&ready, EntryVersion(18));
        let staged =
            StagedSchedulerBatch::stage_reserved_ready_batch(&frontier, delta, reservation)
                .expect("Ready-to-Resolve stages under the captured prefix");

        assert!(staged.prestate_is_fresh());
        {
            let scheduler = frontier.lock();
            assert!(scheduler.logical_ready_contains(&ready));
            assert!(
                scheduler
                    .staged_visibility
                    .get(&StagedSchedulerSlotKey::from(&resolve))
                    .is_some_and(|marker| !marker.logical_is_visible())
            );
        }

        let entries = crate::authority::shard::ShardedOwnerMap::new(
            crate::authority::shard::AuthorityShardRouter::new(),
        );
        staged.activate_for_foundation(
            entries.write_cut(crate::authority::shard::ShardWriteSupport::default()),
        );
        let scheduler = frontier.lock();
        assert!(!scheduler.contains(&SchedulerSlot::Ready(ready)));
        assert!(scheduler.contains(&resolve));
        assert!(scheduler.staged_visibility.is_empty());
    }

    #[test]
    fn claimed_slot_linearizes_before_a_later_stronger_insertion() {
        let frontier = Arc::new(Mutex::new(FairFrontier::new(VerifyOrder::Arrival)));
        let weak = key(1, 3, 3);
        let strong = key(2, 4, 4);
        apply_insert(&frontier, &weak);
        let slot = capture_slot(&frontier);
        let delta = exact_removal(&weak);
        assert!(slot.prestate_is_fresh(&frontier, &delta));
        assert_eq!(slot.claim.state(), READY_SLOT_COMMITTING);

        apply_insert(&frontier, &strong);
        assert_eq!(slot.claim.state(), READY_SLOT_COMMITTING);
        slot.activate(&frontier, delta);

        let next = ReadyReservation::capture(&frontier)
            .expect("the later stronger key remains coherent")
            .expect("the later stronger key is available");
        assert_eq!(
            next.candidates().next(),
            Some((strong.hash(), strong.version()))
        );
    }

    #[test]
    fn administrative_removal_retires_a_reserved_slot_without_resurrection() {
        let frontier = Arc::new(Mutex::new(FairFrontier::new(VerifyOrder::Arrival)));
        let removed = key(1, 5, 5);
        apply_insert(&frontier, &removed);
        let slot = capture_slot(&frontier);

        apply_remove(&frontier, &removed);
        assert_eq!(slot.claim.state(), READY_SLOT_RETIRED);
        assert!(!slot.prestate_is_fresh(&frontier, &exact_removal(&removed)));
        drop(slot);
        assert!(
            ReadyReservation::capture(&frontier)
                .expect("retired slot reaping stays coherent")
                .is_none()
        );
        assert_eq!(
            frontier.lock().ready_physical_counts_for_foundation(),
            (0, 0, 0)
        );
    }

    #[test]
    fn impossible_removal_after_claim_poison_fails_closed() {
        let frontier = Arc::new(Mutex::new(FairFrontier::new(VerifyOrder::Arrival)));
        let removed = key(1, 6, 6);
        apply_insert(&frontier, &removed);
        let slot = capture_slot(&frontier);
        let delta = exact_removal(&removed);
        assert!(slot.prestate_is_fresh(&frontier, &delta));

        apply_remove(&frontier, &removed);
        assert_eq!(slot.claim.state(), READY_SLOT_POISONED);
        drop(slot);
        assert!(matches!(
            ReadyReservation::capture(&frontier),
            Err(SchedulerError::Projection)
        ));
    }

    #[test]
    fn returning_a_stronger_slot_invalidates_every_weaker_live_claim() {
        let frontier = Arc::new(Mutex::new(FairFrontier::new(VerifyOrder::Arrival)));
        let weak = key(1, 7, 7);
        let strong = key(2, 8, 8);
        apply_insert(&frontier, &weak);
        apply_insert(&frontier, &strong);
        let reservation = ReadyReservation::capture(&frontier)
            .expect("the two-key scheduler capture is coherent")
            .expect("two Ready keys are available");
        let (mut slots, remainder) = reservation
            .try_split_prefix(2)
            .unwrap_or_else(|_| panic!("the complete prefix splits into two slots"));
        assert!(remainder.is_none());
        let weaker = slots.pop().expect("the weaker slot exists");
        let stronger = slots.pop().expect("the stronger slot exists");
        assert_eq!(weaker.claim.state(), READY_SLOT_FRESH);

        drop(stronger);
        assert_eq!(weaker.claim.state(), READY_SLOT_INVALID);
        drop(weaker);
    }

    #[test]
    fn retired_old_version_cannot_touch_a_readmitted_same_hash() {
        let frontier = Arc::new(Mutex::new(FairFrontier::new(VerifyOrder::Arrival)));
        let old = key(1, 9, 9);
        let mut new = key(1, 9, 10);
        new.arrival = old.arrival;
        apply_insert(&frontier, &old);
        let old_slot = capture_slot(&frontier);

        apply_remove(&frontier, &old);
        apply_insert(&frontier, &new);
        drop(old_slot);

        let current = ReadyReservation::capture(&frontier)
            .expect("same-hash readmission remains coherent")
            .expect("the new version is independently available");
        assert_eq!(
            current.candidates().next(),
            Some((new.hash(), new.version()))
        );
    }

    #[test]
    fn slot_claims_and_batch_reservations_share_one_hard_wave_bound() {
        let frontier = Arc::new(Mutex::new(FairFrontier::new(VerifyOrder::Arrival)));
        for offset in 0..(MAX_READY_BATCH * 2) {
            let key = key(
                u64::try_from(offset + 1).expect("the fixed fixture fee fits u64"),
                u8::try_from(offset).expect("the fixed fixture nonce fits u8"),
                u128::try_from(offset + 1).expect("the fixed fixture version fits u128"),
            );
            apply_insert(&frontier, &key);
        }
        let reservation = ReadyReservation::capture(&frontier)
            .expect("the first bounded capture is coherent")
            .expect("the first wave exists");
        let (slots, remainder) = reservation
            .try_split_prefix(MAX_READY_BATCH)
            .unwrap_or_else(|_| panic!("the first complete wave splits"));
        assert!(remainder.is_none());
        assert_eq!(
            frontier.lock().ready_physical_counts_for_foundation(),
            (MAX_READY_BATCH * 2, MAX_READY_BATCH, MAX_READY_BATCH)
        );
        assert!(
            ReadyReservation::capture(&frontier)
                .expect("a full outstanding wave is not a projection fault")
                .is_none(),
            "a second capture cannot grow either reservation structure beyond the hard wave bound"
        );

        drop(slots);
        assert_eq!(
            frontier.lock().ready_physical_counts_for_foundation(),
            (MAX_READY_BATCH * 2, 0, 0)
        );
        let next = ReadyReservation::capture(&frontier)
            .expect("capacity returns after every slot is terminal")
            .expect("the next bounded wave is now available");
        drop(next);
        assert_eq!(
            frontier.lock().ready_physical_counts_for_foundation(),
            (MAX_READY_BATCH * 2, 0, 0)
        );
    }
}
