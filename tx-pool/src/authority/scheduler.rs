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

/// ABA-safe identity of one lane's last successful checkout. The selected
/// entry version is already globally unique within an authority generation,
/// so fairness needs no separate global queue revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FairCursor {
    owner: WorkOwner,
    version: EntryVersion,
}

impl FairCursor {
    fn selected(ticket: &CheckoutTicket) -> Self {
        Self {
            owner: ticket.owner,
            version: ticket.version(),
        }
    }
}

/// ABA-safe queue-cut witness for one scheduler lane. Exhaustion is absorbing:
/// queue publication remains available, but no later multi-ticket wave can
/// claim a reusable cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueueRevision {
    Active(u64),
    Exhausted,
}

impl Default for QueueRevision {
    fn default() -> Self {
        Self::Active(0)
    }
}

impl QueueRevision {
    fn witness(self) -> Option<u64> {
        match self {
            Self::Active(revision) => Some(revision),
            Self::Exhausted => None,
        }
    }

    fn advance(&mut self) {
        *self = match *self {
            Self::Active(revision) => revision
                .checked_add(1)
                .map(Self::Active)
                .unwrap_or(Self::Exhausted),
            Self::Exhausted => Self::Exhausted,
        };
    }
}

pub(super) struct SchedulerDelta {
    before: Option<SchedulerSlot>,
    after: Option<SchedulerSlot>,
    owner_cursor: Option<(QueueLane, FairCursor)>,
}

#[derive(Default)]
pub(super) struct SchedulerBatchDelta {
    removed: Vec<SchedulerSlot>,
    added: Vec<SchedulerSlot>,
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
    expected: Option<FairCursor>,
    target: Option<FairCursor>,
    queue_revision: Option<u64>,
}

/// One monotonic publication fact shared by every physical row in a retained
/// ingress stage. Consumers may only observe it; the scheduler publication
/// cut owns the single false-to-true transition.
#[derive(Clone, Debug)]
struct SchedulerStageVisibility(Arc<StagedSchedulerVisibility>);

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SchedulerLaneStage {
    None,
    Queue,
    Fairness,
}

#[derive(Clone, Debug)]
struct StagedSchedulerMarker {
    visibility: SchedulerStageVisibility,
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

impl SchedulerStageVisibility {
    fn hidden() -> Self {
        Self(Arc::new(StagedSchedulerVisibility {
            visible: AtomicBool::new(false),
        }))
    }

    fn is_visible(&self) -> bool {
        self.0.visible.load(AtomicOrdering::Acquire)
    }

    fn same_stage(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    fn activate(&self) {
        self.0.visible.store(true, AtomicOrdering::Release);
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
    visibility: SchedulerStageVisibility,
    scheduler_wake_before: SchedulerWakeProjection,
    resolve_stage: SchedulerLaneStage,
    verify_stage: SchedulerLaneStage,
    #[cfg(feature = "profiling")]
    _gate_hold_span: Option<tracing::Span>,
    terminal: bool,
}

/// One exact Ready claim bound to the hidden Resolve row that will replace it.
/// Keeping both capabilities in one move-only value makes it impossible for
/// safe callers to validate one reservation and publish another one's stage.
#[must_use = "a staged Ready re-resolution must be activated or rolled back by Drop"]
pub(super) struct StagedReadyReresolution<'reservation, 'frontier> {
    reservation: &'reservation mut ReadySlotReservation,
    staged: StagedSchedulerBatch<'frontier>,
}

#[cfg(feature = "profiling")]
fn scheduler_fairness_stage_wait_span(fairness_stage: bool) -> Option<tracing::Span> {
    fairness_stage.then(|| {
        tracing::trace_span!(
            target: "ckb_tx_pool_profile",
            "tx_pool.scheduler.fairness_stage_wait"
        )
    })
}

#[cfg(feature = "profiling")]
fn scheduler_fairness_stage_hold_span(fairness_stage: bool) -> Option<tracing::Span> {
    fairness_stage.then(|| {
        tracing::trace_span!(
            target: "ckb_tx_pool_profile",
            "tx_pool.scheduler.fairness_stage_hold"
        )
    })
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
impl SchedulerDelta {
    fn changes_queue_lane(&self, lane: QueueLane) -> bool {
        self.before != self.after
            && self
                .before
                .iter()
                .chain(&self.after)
                .any(|slot| slot.queue_lane() == Some(lane))
    }

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
        let mut removed = Vec::with_capacity(usize::from(self.before.is_some()));
        let mut added = Vec::with_capacity(usize::from(self.after.is_some()));
        if let Some(before) = self.before {
            removed.push(before);
        }
        if let Some(after) = self.after {
            added.push(after);
        }
        Ok(SchedulerBatchDelta {
            removed,
            added,
            resolve_cursor: None,
            verify_cursor: None,
        })
    }
}

impl SchedulerSlot {
    fn queue_lane(&self) -> Option<QueueLane> {
        match self {
            Self::Queue { lane, .. } => Some(*lane),
            Self::Ready(_) => None,
        }
    }
}

impl SchedulerBatchDelta {
    pub(in crate::authority) fn is_empty(&self) -> bool {
        self.removed.is_empty()
            && self.added.is_empty()
            && self.resolve_cursor.is_none()
            && self.verify_cursor.is_none()
    }

    pub(in crate::authority) fn prestate_is_fresh(&self, frontier: &FairFrontier) -> bool {
        self.removed.iter().all(|slot| frontier.contains(slot))
            && self
                .added
                .iter()
                .all(|slot| self.removed.binary_search(slot).is_ok() || !frontier.contains(slot))
            && self.resolve_cursor.is_none_or(|change| {
                frontier.resolve.owner_cursor == change.expected
                    && frontier.resolve.queue_revision.witness() == change.queue_revision
            })
            && self.verify_cursor.is_none_or(|change| {
                frontier.verify.owner_cursor == change.expected
                    && frontier.verify.queue_revision.witness() == change.queue_revision
            })
    }

    fn changes_queue_lane(&self, lane: QueueLane) -> bool {
        self.removed
            .iter()
            .filter(|slot| self.added.binary_search(slot).is_err())
            .chain(
                self.added
                    .iter()
                    .filter(|slot| self.removed.binary_search(slot).is_err()),
            )
            .any(|slot| slot.queue_lane() == Some(lane))
    }

    fn lane_stage(&self, lane: QueueLane) -> SchedulerLaneStage {
        let changes_cursor = match lane {
            QueueLane::Resolve => self.resolve_cursor.is_some(),
            QueueLane::Verify => self.verify_cursor.is_some(),
        };
        if changes_cursor {
            SchedulerLaneStage::Fairness
        } else if self.changes_queue_lane(lane) {
            SchedulerLaneStage::Queue
        } else {
            SchedulerLaneStage::None
        }
    }

    /// A pure Ready acceptance removes committed Ready slots and publishes no
    /// new queue node or fairness cursor. That shape is allocation-free after
    /// the owner-version OCC cut succeeds, which is required by shared Apply.
    pub(super) fn is_shared_acceptance_removal_only(&self) -> bool {
        self.added.is_empty() && self.resolve_cursor.is_none() && self.verify_cursor.is_none()
    }

    fn is_exact_ready_removal(&self, key: &ReadyKey) -> bool {
        self.is_shared_acceptance_removal_only()
            && self.removed.len() == 1
            && matches!(self.removed.first(), Some(SchedulerSlot::Ready(removed)) if removed == key)
    }

    /// The only Ready transition that must publish a new scheduler row.
    /// Acceptance and rejection remove the reserved Ready slot without a
    /// queue allocation; a rules change replaces it with the same raw hash at
    /// a fresh version in Resolve.
    fn is_exact_ready_reresolution(&self, key: &ReadyKey) -> bool {
        self.resolve_cursor.is_none()
            && self.verify_cursor.is_none()
            && self.removed.len() == 1
            && self.added.len() == 1
            && matches!(self.removed.first(), Some(SchedulerSlot::Ready(removed)) if removed == key)
            && matches!(
                self.added.first(),
                Some(SchedulerSlot::Queue {
                    lane: QueueLane::Resolve,
                    key: QueueKey::Resolve(added),
                    ..
                }) if &added.hash == key.hash() && added.version != key.version()
            )
    }
}

impl FairFrontier {
    fn lane(&self, lane: QueueLane) -> &FairLane {
        match lane {
            QueueLane::Resolve => &self.resolve,
            QueueLane::Verify => &self.verify,
        }
    }

    fn lane_mut(&mut self, lane: QueueLane) -> &mut FairLane {
        match lane {
            QueueLane::Resolve => &mut self.resolve,
            QueueLane::Verify => &mut self.verify,
        }
    }

    /// Queue stages are concurrent readers of one lane's ordering cut; a
    /// multi-ticket fairness stage is its writer. The two lanes never wait on
    /// each other, and Ready-only stages bypass this gate.
    fn acquire_lane_stage(
        scheduler: &mut MutexGuard<'_, Self>,
        lane: QueueLane,
        stage: SchedulerLaneStage,
    ) -> Result<(), SchedulerError> {
        match stage {
            SchedulerLaneStage::None => Ok(()),
            SchedulerLaneStage::Queue => loop {
                let current = scheduler.lane(lane);
                if !current.fairness_stage_active && current.fairness_stage_waiters == 0 {
                    let current = scheduler.lane_mut(lane);
                    current.hidden_queue_stages = current
                        .hidden_queue_stages
                        .checked_add(1)
                        .ok_or(SchedulerError::Arithmetic)?;
                    return Ok(());
                }
                let changed = Arc::clone(&current.stage_gate_changed);
                changed.wait(scheduler);
            },
            SchedulerLaneStage::Fairness => {
                let current = scheduler.lane_mut(lane);
                current.fairness_stage_waiters = current
                    .fairness_stage_waiters
                    .checked_add(1)
                    .ok_or(SchedulerError::Arithmetic)?;
                loop {
                    let current = scheduler.lane(lane);
                    if !current.fairness_stage_active && current.hidden_queue_stages == 0 {
                        let current = scheduler.lane_mut(lane);
                        current.fairness_stage_waiters = current
                            .fairness_stage_waiters
                            .checked_sub(1)
                            .ok_or(SchedulerError::Projection)?;
                        current.fairness_stage_active = true;
                        return Ok(());
                    }
                    let changed = Arc::clone(&current.stage_gate_changed);
                    changed.wait(scheduler);
                }
            }
        }
    }

    fn release_lane_stage(&mut self, lane: QueueLane, stage: SchedulerLaneStage) {
        let current = self.lane_mut(lane);
        match stage {
            SchedulerLaneStage::None => return,
            SchedulerLaneStage::Queue => {
                let Some(remaining) = current.hidden_queue_stages.checked_sub(1) else {
                    debug_assert!(false, "a live queue stage owns one lane reservation");
                    return;
                };
                current.hidden_queue_stages = remaining;
            }
            SchedulerLaneStage::Fairness => {
                debug_assert!(current.fairness_stage_active);
                current.fairness_stage_active = false;
            }
        }
        current.stage_gate_changed.notify_all();
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
        let mut slots = Vec::with_capacity(count);
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
        let mut slots = Vec::with_capacity(hashes.len());
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
        let mut reservations = Vec::with_capacity(count);
        let mut claims = Vec::with_capacity(count);
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

    fn matches_reresolution_locked(
        &self,
        frontier: &Arc<Mutex<FairFrontier>>,
        scheduler: &FairFrontier,
        delta: &SchedulerBatchDelta,
    ) -> bool {
        let Some(slot) = self.slot.as_ref() else {
            return false;
        };
        Arc::ptr_eq(&self.frontier, frontier)
            && scheduler.slot_claim_is_current(slot, &self.claim)
            && self.claim.state() == READY_SLOT_FRESH
            && delta.is_exact_ready_reresolution(slot)
    }

    /// The staged scheduler row already owns every B-tree and revision
    /// premise. The final priority race is therefore the same lock-free claim
    /// transition used by acceptance; no second scheduler read is needed
    /// while owner shards are locked.
    pub(in crate::authority) fn begin_reresolution_commit(
        &self,
        frontier: &Arc<Mutex<FairFrontier>>,
    ) -> bool {
        self.slot.is_some()
            && Arc::ptr_eq(&self.frontier, frontier)
            && self.claim.try_begin_commit()
    }

    fn commit_reresolution(&mut self) {
        let _ = self.slot.take();
        self.claim.commit();
    }

    pub(in crate::authority) fn activate(
        &mut self,
        _frontier: &Arc<Mutex<FairFrontier>>,
        delta: SchedulerBatchDelta,
    ) {
        let _ = delta;
        let _ = self.slot.take();
        self.claim.commit();
    }

    pub(in crate::authority) fn scheduler_wake_before(
        &self,
    ) -> Result<SchedulerWakeProjection, SchedulerError> {
        self.claim
            .scheduler_wake_before()
            .ok_or(SchedulerError::Projection)
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

impl<'frontier> StagedSchedulerBatch<'frontier> {
    pub(super) fn stage_primary_replacements(
        frontier: &'frontier Arc<Mutex<FairFrontier>>,
        delta: SchedulerBatchDelta,
    ) -> Result<Self, SchedulerError> {
        Self::stage(frontier, delta, |_, _| true)
    }

    fn stage(
        frontier: &'frontier Arc<Mutex<FairFrontier>>,
        delta: SchedulerBatchDelta,
        validate: impl FnOnce(&FairFrontier, &SchedulerBatchDelta) -> bool,
    ) -> Result<Self, SchedulerError> {
        let resolve_stage = delta.lane_stage(QueueLane::Resolve);
        let verify_stage = delta.lane_stage(QueueLane::Verify);
        #[cfg(feature = "profiling")]
        let fairness_stage = matches!(resolve_stage, SchedulerLaneStage::Fairness)
            || matches!(verify_stage, SchedulerLaneStage::Fairness);
        let visibility = SchedulerStageVisibility::hidden();
        #[cfg(feature = "profiling")]
        let gate_wait_span = scheduler_fairness_stage_wait_span(fairness_stage);
        let mut scheduler = frontier.lock();
        FairFrontier::acquire_lane_stage(&mut scheduler, QueueLane::Resolve, resolve_stage)?;
        if let Err(error) =
            FairFrontier::acquire_lane_stage(&mut scheduler, QueueLane::Verify, verify_stage)
        {
            scheduler.release_lane_stage(QueueLane::Resolve, resolve_stage);
            return Err(error);
        }
        #[cfg(feature = "profiling")]
        drop(gate_wait_span);
        #[cfg(feature = "profiling")]
        let gate_hold_span = scheduler_fairness_stage_hold_span(fairness_stage);
        let scheduler_wake_before = scheduler.wake_projection();
        if !validate(&scheduler, &delta)
            || !delta.prestate_is_fresh(&scheduler)
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
            scheduler.release_lane_stage(QueueLane::Verify, verify_stage);
            scheduler.release_lane_stage(QueueLane::Resolve, resolve_stage);
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
            resolve_stage,
            verify_stage,
            #[cfg(feature = "profiling")]
            _gate_hold_span: gate_hold_span,
            terminal: false,
        })
    }

    /// Stable ownership established at staging. Unlike a batch Ready prefix,
    /// these premises cannot change while this stage owns its marker set and
    /// queue/fairness gate. Publication may therefore assert them after owner
    /// mutation without rechecking the time-sensitive Ready ordering proof.
    fn stage_ownership_is_intact_locked(&self, scheduler: &FairFrontier) -> bool {
        let lane_is_held = |lane, stage| match stage {
            SchedulerLaneStage::None => true,
            SchedulerLaneStage::Queue => scheduler.lane(lane).hidden_queue_stages != 0,
            SchedulerLaneStage::Fairness => scheduler.lane(lane).fairness_stage_active,
        };
        let gate_is_held = lane_is_held(QueueLane::Resolve, self.resolve_stage)
            && lane_is_held(QueueLane::Verify, self.verify_stage);
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
                .resolve_cursor
                .is_none_or(|change| scheduler.resolve.owner_cursor == change.expected)
            && self
                .delta
                .verify_cursor
                .is_none_or(|change| scheduler.verify.owner_cursor == change.expected)
    }

    fn release_stage_gate_locked(&mut self, scheduler: &mut FairFrontier) {
        scheduler.release_lane_stage(
            QueueLane::Verify,
            std::mem::replace(&mut self.verify_stage, SchedulerLaneStage::None),
        );
        scheduler.release_lane_stage(
            QueueLane::Resolve,
            std::mem::replace(&mut self.resolve_stage, SchedulerLaneStage::None),
        );
    }

    fn publish_locked<'published>(
        &'published mut self,
        scheduler: &'published mut FairFrontier,
        owner_cut: ShardedOwnerWriteCut<'_>,
    ) -> PublishedSchedulerBatch<'published, 'frontier> {
        debug_assert!(self.stage_ownership_is_intact_locked(scheduler));
        if let Some(change) = self.delta.resolve_cursor {
            scheduler.resolve.owner_cursor = change.target;
        }
        if let Some(change) = self.delta.verify_cursor {
            scheduler.verify.owner_cursor = change.target;
        }
        if self.delta.changes_queue_lane(QueueLane::Resolve) {
            scheduler.resolve.queue_revision.advance();
        }
        if self.delta.changes_queue_lane(QueueLane::Verify) {
            scheduler.verify.queue_revision.advance();
        }
        self.release_stage_gate_locked(scheduler);
        // The staged rows become externally visible in this one scheduler
        // lock cut. Only same-lane queue and fairness changes conflict.
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

    fn activate_inner(mut self, owner_cut: ShardedOwnerWriteCut<'_>) {
        let frontier = self.frontier;
        let mut scheduler = frontier.lock();
        let retired_delta = self.publish_locked(&mut scheduler, owner_cut).finalize();
        drop(scheduler);
        drop(retired_delta);
    }

    pub(super) fn activate(self, _token: &ApplyToken, owner_cut: ShardedOwnerWriteCut<'_>) {
        self.activate_inner(owner_cut)
    }

    fn activate_ready_reresolution(
        mut self,
        reservation: &mut ReadySlotReservation,
        owner_cut: ShardedOwnerWriteCut<'_>,
    ) {
        let frontier = self.frontier;
        let mut scheduler = frontier.lock();
        reservation.commit_reresolution();
        let retired_delta = self.publish_locked(&mut scheduler, owner_cut).finalize();
        drop(scheduler);
        drop(retired_delta);
    }

    #[cfg(test)]
    fn activate_for_foundation(self, owner_cut: ShardedOwnerWriteCut<'_>) {
        self.activate_inner(owner_cut)
    }

    pub(super) fn wake_projection_before(&self) -> Option<SchedulerWakeProjection> {
        Some(self.scheduler_wake_before)
    }
}

impl<'reservation, 'frontier> StagedReadyReresolution<'reservation, 'frontier> {
    /// Preallocate and hide the Resolve row while atomically checking that it
    /// is the exact replacement for this reservation. The retained mutable
    /// borrow prevents the stage from later being paired with another claim.
    pub(super) fn stage(
        frontier: &'frontier Arc<Mutex<FairFrontier>>,
        delta: SchedulerBatchDelta,
        reservation: &'reservation mut ReadySlotReservation,
    ) -> Result<Self, SchedulerError> {
        let staged = StagedSchedulerBatch::stage(frontier, delta, |scheduler, delta| {
            reservation.matches_reresolution_locked(frontier, scheduler, delta)
        })?;
        Ok(Self {
            reservation,
            staged,
        })
    }

    pub(super) fn begin_commit(&self, frontier: &Arc<Mutex<FairFrontier>>) -> bool {
        self.reservation.begin_reresolution_commit(frontier)
    }

    pub(super) fn activate(self, _token: &ApplyToken, owner_cut: ShardedOwnerWriteCut<'_>) {
        self.staged
            .activate_ready_reresolution(self.reservation, owner_cut)
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
    owner_cursor: Option<FairCursor>,
    queue_revision: QueueRevision,
    hidden_queue_stages: usize,
    fairness_stage_active: bool,
    fairness_stage_waiters: usize,
    stage_gate_changed: Arc<Condvar>,
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
            self.next_owner(
                lane,
                capability,
                self.owner_cursor.map(|cursor| cursor.owner),
                staged,
            )?
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
    verify_order: VerifyOrder,
}

/// Bounded virtual checkout cut. Selected versions are globally unique within
/// one authority generation, so this overlay can remove up to one worker wave
/// without cloning the scheduler or publishing a second queue authority.
pub(super) struct SchedulerWaveCursor {
    selected_versions: Vec<EntryVersion>,
    resolve_cursor: SchedulerCursorChange,
    verify_cursor: SchedulerCursorChange,
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
            QueueLane::Resolve => self.resolve_cursor.target,
            QueueLane::Verify => self.verify_cursor.target,
        }
        .map(|cursor| cursor.owner)
    }

    pub(super) fn select(&mut self, ticket: &CheckoutTicket) -> Result<(), SchedulerError> {
        let queue_revision = match ticket.lane {
            QueueLane::Resolve => self.resolve_cursor.queue_revision,
            QueueLane::Verify => self.verify_cursor.queue_revision,
        };
        if queue_revision.is_none() {
            return Err(SchedulerError::Arithmetic);
        }
        match self.selected_versions.binary_search(&ticket.version()) {
            Ok(_) => return Err(SchedulerError::Projection),
            Err(position) => self.selected_versions.insert(position, ticket.version()),
        }
        let cursor = FairCursor::selected(ticket);
        match ticket.lane {
            QueueLane::Resolve => self.resolve_cursor.target = Some(cursor),
            QueueLane::Verify => self.verify_cursor.target = Some(cursor),
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
                let cursor = FairCursor::selected(&ticket);
                let selected = SchedulerSlot::Queue {
                    lane: ticket.lane,
                    owner: ticket.owner,
                    key: ticket.key,
                };
                if before.as_ref() != Some(&selected) || after.is_some() {
                    return Err(SchedulerError::Projection);
                }
                Some((ticket.lane, cursor))
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

    /// Compile the exact cursor-free set transition without reading the live
    /// frontier. Shared Apply stages this delta under the scheduler mutex and
    /// validates every removed/added slot before it can reach the owner cut.
    pub(super) fn compile_batch<'entry>(
        &self,
        changes: impl IntoIterator<Item = (Option<&'entry OwnedTx>, Option<&'entry OwnedTx>)>,
    ) -> Result<SchedulerBatchDelta, SchedulerError> {
        let mut input = changes.into_iter();
        let mut removed = Vec::with_capacity(input.size_hint().1.unwrap_or(0));
        let mut added = Vec::with_capacity(removed.capacity());
        for (before, after) in input.by_ref() {
            if let Some(before) = before.map(|owner| self.slot(owner)).transpose()?.flatten() {
                removed.push(before);
            }
            if let Some(after) = after.map(|owner| self.slot(owner)).transpose()?.flatten() {
                added.push(after);
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
        Ok(SchedulerBatchDelta {
            removed,
            added,
            resolve_cursor: None,
            verify_cursor: None,
        })
    }

    fn plan_batch_with_missing_before<'entry>(
        &self,
        changes: impl IntoIterator<Item = (Option<&'entry OwnedTx>, Option<&'entry OwnedTx>)>,
        missing_before: SchedulerError,
    ) -> Result<SchedulerBatchDelta, SchedulerError> {
        let delta = self.compile_batch(changes)?;
        if delta.removed.iter().any(|before| !self.contains(before)) {
            return Err(missing_before);
        }
        if delta
            .added
            .iter()
            .any(|slot| self.contains(slot) && delta.removed.binary_search(slot).is_err())
        {
            return Err(SchedulerError::Projection);
        }
        Ok(delta)
    }

    /// Compile final owner projections together with the fairness cursors
    /// produced by a sealed virtual worker wave. The wave is stack-owned Plan
    /// evidence; only this consumption point can publish its cursor advance.
    pub(super) fn plan_exchange_batch<'entry>(
        &self,
        changes: impl IntoIterator<Item = (Option<&'entry OwnedTx>, Option<&'entry OwnedTx>)>,
        cursor: SchedulerWaveCursor,
    ) -> Result<SchedulerBatchDelta, SchedulerError> {
        let mut delta = self.plan_batch_with_missing_before(changes, SchedulerError::Stale)?;
        if cursor.resolve_cursor.expected != cursor.resolve_cursor.target {
            if cursor.resolve_cursor.expected != self.resolve.owner_cursor
                || cursor.resolve_cursor.queue_revision != self.resolve.queue_revision.witness()
            {
                return Err(SchedulerError::Stale);
            }
            delta.resolve_cursor = Some(cursor.resolve_cursor);
        }
        if cursor.verify_cursor.expected != cursor.verify_cursor.target {
            if cursor.verify_cursor.expected != self.verify.owner_cursor
                || cursor.verify_cursor.queue_revision != self.verify.queue_revision.witness()
            {
                return Err(SchedulerError::Stale);
            }
            delta.verify_cursor = Some(cursor.verify_cursor);
        }
        Ok(delta)
    }

    pub(super) fn checkout_wave(
        &self,
        selection_bound: usize,
    ) -> Result<SchedulerWaveCursor, SchedulerError> {
        let selected_versions = Vec::with_capacity(selection_bound);
        Ok(SchedulerWaveCursor {
            selected_versions,
            resolve_cursor: SchedulerCursorChange {
                expected: self.resolve.owner_cursor,
                target: self.resolve.owner_cursor,
                queue_revision: self.resolve.queue_revision.witness(),
            },
            verify_cursor: SchedulerCursorChange {
                expected: self.verify.owner_cursor,
                target: self.verify.owner_cursor,
                queue_revision: self.verify.queue_revision.witness(),
            },
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
        let mut ready = Vec::with_capacity(count);
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
        let resolve_changed = delta.changes_queue_lane(QueueLane::Resolve);
        let verify_changed = delta.changes_queue_lane(QueueLane::Verify);
        if let Some(before) = delta.before {
            self.remove_physical(before);
        }
        if let Some(after) = delta.after {
            self.insert_physical(after);
        }
        if let Some((lane, owner)) = delta.owner_cursor {
            match lane {
                QueueLane::Resolve => self.resolve.owner_cursor = Some(owner),
                QueueLane::Verify => self.verify.owner_cursor = Some(owner),
            }
        }
        if resolve_changed {
            self.resolve.queue_revision.advance();
        }
        if verify_changed {
            self.verify.queue_revision.advance();
        }
        let _ = self.reap_slot_claims();
        self.refresh_slot_claims();
    }

    pub(super) fn apply_batch(&mut self, delta: SchedulerBatchDelta) {
        let resolve_changed = delta.changes_queue_lane(QueueLane::Resolve);
        let verify_changed = delta.changes_queue_lane(QueueLane::Verify);
        // A batch is a set transition, independent of the caller's change
        // order. Remove the complete old projection before publishing any new
        // slot so an exchange can never be lost to BTreeSet insertion order.
        for slot in delta.removed {
            self.remove_physical(slot);
        }
        for slot in delta.added {
            self.insert_physical(slot);
        }
        if let Some(cursor) = delta.resolve_cursor {
            self.resolve.owner_cursor = cursor.target;
        }
        if let Some(cursor) = delta.verify_cursor {
            self.verify.owner_cursor = cursor.target;
        }
        if resolve_changed {
            self.resolve.queue_revision.advance();
        }
        if verify_changed {
            self.verify.queue_revision.advance();
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
mod scheduler_stage_tests {
    use super::*;
    use ckb_types::packed::Byte32;

    #[cfg(feature = "profiling")]
    #[test]
    fn staged_scheduler_batch_remains_send_with_lifetime_profiling_enabled() {
        fn assert_send<T: Send>() {}
        assert_send::<StagedSchedulerBatch<'static>>();
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

    fn verify_slot_with_version(owner: WorkOwner, nonce: u8, version: u128) -> SchedulerSlot {
        SchedulerSlot::Queue {
            lane: QueueLane::Verify,
            owner,
            key: QueueKey::Verify(VerifyKey {
                source: SourcePriority::Remote,
                order: VerifyOrder::Arrival,
                fee: 1,
                serialized_bytes: 1,
                arrival: Arrival(0),
                hash: RawTxHash(Byte32::new([nonce; 32])),
                version: EntryVersion(version),
                class: VerifyCycleClass::Small,
            }),
        }
    }

    fn insertion(slot: SchedulerSlot) -> SchedulerBatchDelta {
        SchedulerBatchDelta {
            removed: Vec::new(),
            added: vec![slot],
            resolve_cursor: None,
            verify_cursor: None,
        }
    }

    fn resolve_fairness() -> SchedulerBatchDelta {
        SchedulerBatchDelta {
            removed: Vec::new(),
            added: Vec::new(),
            resolve_cursor: Some(SchedulerCursorChange {
                expected: None,
                target: Some(FairCursor {
                    owner: WorkOwner::Trusted,
                    version: EntryVersion(1),
                }),
                queue_revision: Some(0),
            }),
            verify_cursor: None,
        }
    }

    fn ready_slot(nonce: u8, version: u128) -> SchedulerSlot {
        SchedulerSlot::Ready(ReadyKey {
            source: SourcePriority::Remote,
            fee: 1,
            serialized_bytes: 1,
            arrival: Arrival(version),
            hash: RawTxHash(Byte32::new([nonce; 32])),
            version: EntryVersion(version),
        })
    }

    #[test]
    fn ready_changes_do_not_stale_a_compute_queue_wave() {
        let frontier = Arc::new(Mutex::new(FairFrontier::new(VerifyOrder::Arrival)));
        let wave = frontier
            .lock()
            .checkout_wave(0)
            .expect("the initial empty compute wave is bounded");
        frontier.lock().apply(SchedulerDelta {
            before: None,
            after: Some(ready_slot(96, 96)),
            owner_cursor: None,
        });

        assert!(
            frontier
                .lock()
                .plan_exchange_batch(std::iter::empty(), wave)
                .is_ok(),
            "Ready-only changes cannot invalidate compute checkout"
        );
    }

    #[test]
    fn stamped_fairness_cursor_rejects_owner_aba() {
        let mut frontier = FairFrontier::new(VerifyOrder::Arrival);
        let initial = FairCursor {
            owner: WorkOwner::Remote(PeerIndex::from(90usize)),
            version: EntryVersion(1),
        };
        frontier.resolve.owner_cursor = Some(initial);
        let mut wave = frontier
            .checkout_wave(1)
            .expect("the initial fairness cut is bounded");
        let SchedulerSlot::Queue { lane, owner, key } = resolve_slot_with_version(
            WorkOwner::Remote(PeerIndex::from(91usize)),
            SourcePriority::Remote,
            91,
            2,
        ) else {
            unreachable!("the fixture constructs one Resolve slot")
        };
        wave.select(&CheckoutTicket { lane, owner, key })
            .expect("the wave selects one distinct version");

        frontier.resolve.owner_cursor = Some(FairCursor {
            owner: initial.owner,
            version: EntryVersion(3),
        });
        assert!(matches!(
            frontier.plan_exchange_batch(std::iter::empty(), wave),
            Err(SchedulerError::Stale)
        ));
    }

    #[test]
    fn multi_ticket_wave_rejects_a_mixed_lane_cut() {
        let frontier = Arc::new(Mutex::new(FairFrontier::new(VerifyOrder::Arrival)));
        let owner_a = WorkOwner::Remote(PeerIndex::from(1usize));
        let owner_b = WorkOwner::Remote(PeerIndex::from(2usize));
        let owner_c = WorkOwner::Remote(PeerIndex::from(3usize));
        let owner_d = WorkOwner::Remote(PeerIndex::from(4usize));
        {
            let mut scheduler = frontier.lock();
            for slot in [
                resolve_slot_with_version(owner_a, SourcePriority::Remote, 11, 11),
                resolve_slot_with_version(owner_c, SourcePriority::Remote, 13, 13),
                resolve_slot_with_version(owner_d, SourcePriority::Remote, 14, 14),
            ] {
                scheduler.apply(SchedulerDelta {
                    before: None,
                    after: Some(slot),
                    owner_cursor: None,
                });
            }
            scheduler.resolve.owner_cursor = Some(FairCursor {
                owner: owner_a,
                version: EntryVersion(11),
            });
        }

        let mut wave = SchedulerExchangeWave::after(Arc::clone(&frontier), std::iter::empty(), 4)
            .expect("the initial lane cut is available");
        let first = wave
            .next(crate::authority::state::WorkPermit::ResolveOnly)
            .expect("owner C follows the old cursor");
        assert_eq!(first.owner(), owner_c);
        wave.select(&first).expect("the first ticket is unique");

        frontier.lock().apply(SchedulerDelta {
            before: None,
            after: Some(resolve_slot_with_version(
                owner_b,
                SourcePriority::Remote,
                12,
                12,
            )),
            owner_cursor: None,
        });
        for expected in [owner_d, owner_a, owner_b] {
            let ticket = wave
                .next(crate::authority::state::WorkPermit::ResolveOnly)
                .expect("the live walk still has one owner");
            assert_eq!(ticket.owner(), expected);
            wave.select(&ticket).expect("each ticket version is unique");
        }

        assert!(matches!(
            frontier
                .lock()
                .plan_exchange_batch(std::iter::empty(), wave.into_cursor()),
            Err(SchedulerError::Stale)
        ));
    }

    #[test]
    fn other_lane_queue_stage_does_not_wait_for_active_fairness_stage() {
        const WAIT: std::time::Duration = std::time::Duration::from_secs(2);
        let frontier = Arc::new(Mutex::new(FairFrontier::new(VerifyOrder::Arrival)));
        let fairness =
            StagedSchedulerBatch::stage_primary_replacements(&frontier, resolve_fairness())
                .expect("one fairness cursor stage is reserved");
        let queued = verify_slot_with_version(WorkOwner::Remote(PeerIndex::from(95usize)), 95, 95);
        std::thread::scope(|scope| {
            let (staged_tx, staged_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let queue_frontier = Arc::clone(&frontier);
            scope.spawn(move || {
                let stage = StagedSchedulerBatch::stage_primary_replacements(
                    &queue_frontier,
                    insertion(queued),
                )
                .expect("a Verify queue change commutes with the active Resolve fairness stage");
                let _ = staged_tx.send(());
                let _ = release_rx.recv();
                drop(stage);
            });
            staged_rx
                .recv_timeout(WAIT)
                .expect("ordinary queue staging never waits across fairness owner Apply");
            assert!(frontier.lock().resolve.fairness_stage_active);
            release_tx.send(()).expect("release the queue stage");
        });
        drop(fairness);
        let scheduler = frontier.lock();
        assert!(!scheduler.resolve.fairness_stage_active);
        assert!(!scheduler.verify.fairness_stage_active);
        assert_eq!(scheduler.resolve.hidden_queue_stages, 0);
        assert_eq!(scheduler.verify.hidden_queue_stages, 0);
        assert!(scheduler.staged_visibility.is_empty());
    }

    #[test]
    fn same_lane_fairness_waits_for_an_earlier_queue_stage() {
        const WAIT: std::time::Duration = std::time::Duration::from_secs(2);
        let frontier = Arc::new(Mutex::new(FairFrontier::new(VerifyOrder::Arrival)));
        let queued = StagedSchedulerBatch::stage_primary_replacements(
            &frontier,
            insertion(resolve_slot_with_version(
                WorkOwner::Remote(PeerIndex::from(92usize)),
                SourcePriority::Remote,
                92,
                92,
            )),
        )
        .expect("the first queue stage owns one lane reservation");
        std::thread::scope(|scope| {
            let (fair_tx, fair_rx) = std::sync::mpsc::channel();
            let (release_fair_tx, release_fair_rx) = std::sync::mpsc::channel();
            let fair_frontier = Arc::clone(&frontier);
            scope.spawn(move || {
                let stage = StagedSchedulerBatch::stage_primary_replacements(
                    &fair_frontier,
                    resolve_fairness(),
                )
                .expect("fairness acquires after the earlier queue stage");
                let _ = fair_tx.send(());
                let _ = release_fair_rx.recv();
                drop(stage);
            });
            let deadline = std::time::Instant::now() + WAIT;
            while frontier.lock().resolve.fairness_stage_waiters == 0 {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the fairness stage publishes its wait intent"
                );
                std::thread::yield_now();
            }

            drop(queued);
            fair_rx
                .recv_timeout(WAIT)
                .expect("fairness progresses after the earlier queue stage");
            release_fair_tx.send(()).expect("release fairness");
        });
    }

    #[test]
    fn disjoint_queue_stages_activate_in_reverse_order_without_staling_wave() {
        let frontier = Arc::new(Mutex::new(FairFrontier::new(VerifyOrder::Arrival)));
        let first_slot = resolve_slot_with_version(
            WorkOwner::Remote(PeerIndex::from(93usize)),
            SourcePriority::Remote,
            93,
            93,
        );
        let second_slot = resolve_slot_with_version(
            WorkOwner::Remote(PeerIndex::from(94usize)),
            SourcePriority::Remote,
            94,
            94,
        );
        let old_wave = frontier
            .lock()
            .checkout_wave(0)
            .expect("the empty pre-publication wave is bounded");
        let first = StagedSchedulerBatch::stage_primary_replacements(
            &frontier,
            insertion(first_slot.clone()),
        )
        .expect("the first disjoint queue stage is hidden");
        let second = StagedSchedulerBatch::stage_primary_replacements(
            &frontier,
            insertion(second_slot.clone()),
        )
        .expect("the second disjoint queue stage is independently hidden");
        let entries = crate::authority::shard::ShardedOwnerMap::new(
            crate::authority::shard::AuthorityShardRouter::new(),
        );
        second.activate_for_foundation(
            entries.write_cut(crate::authority::shard::ShardWriteSupport::default()),
        );
        assert!(
            frontier
                .lock()
                .plan_exchange_batch(std::iter::empty(), old_wave)
                .is_ok()
        );
        first.activate_for_foundation(
            entries.write_cut(crate::authority::shard::ShardWriteSupport::default()),
        );
        let scheduler = frontier.lock();
        assert!(scheduler.contains(&first_slot));
        assert!(scheduler.contains(&second_slot));
        assert!(!scheduler.resolve.fairness_stage_active);
        assert!(!scheduler.verify.fairness_stage_active);
        assert_eq!(scheduler.resolve.hidden_queue_stages, 0);
        assert_eq!(scheduler.verify.hidden_queue_stages, 0);
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
            resolve_cursor: None,
            verify_cursor: None,
        }
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
        let mut slot = capture_slot(&frontier);

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
