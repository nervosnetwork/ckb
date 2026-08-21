use super::state::{
    Arrival, EntryVersion, OwnedTx, PreAcceptedEntry, PreAcceptedPhase, PreAcceptedSource,
    RawTxHash, VerifyCapability, VerifyCycleClass,
};
use crate::{constants::MAX_READY_BATCH, util::fee_rate_cross_product};
use ckb_network::PeerIndex;
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    ops::Bound::{Excluded, Unbounded},
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

pub(super) struct SchedulerBatchDelta {
    removed: Vec<SchedulerSlot>,
    added: Vec<SchedulerSlot>,
    resolve_cursor: Option<WorkOwner>,
    verify_cursor: Option<WorkOwner>,
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

    fn head(&self, lane: QueueLane, capability: VerifyCapability) -> Option<&QueueKey> {
        match lane {
            QueueLane::Resolve => self.small.first(),
            QueueLane::Verify => match capability {
                VerifyCapability::SmallCycleOnly => self.small.last(),
                VerifyCapability::Any => match (self.small.last(), self.large.last()) {
                    (Some(small), Some(large)) => Some(std::cmp::max(small, large)),
                    (Some(small), None) => Some(small),
                    (None, Some(large)) => Some(large),
                    (None, None) => None,
                },
            },
        }
    }

    fn head_excluding(
        &self,
        lane: QueueLane,
        capability: VerifyCapability,
        excluded_versions: &[EntryVersion],
    ) -> Option<&QueueKey> {
        fn first_available<'entries>(
            entries: &'entries BTreeSet<QueueKey>,
            excluded_versions: &[EntryVersion],
        ) -> Option<&'entries QueueKey> {
            entries
                .iter()
                .find(|key| excluded_versions.binary_search(&key.version()).is_err())
        }
        fn last_available<'entries>(
            entries: &'entries BTreeSet<QueueKey>,
            excluded_versions: &[EntryVersion],
        ) -> Option<&'entries QueueKey> {
            entries
                .iter()
                .rev()
                .find(|key| excluded_versions.binary_search(&key.version()).is_err())
        }
        match lane {
            QueueLane::Resolve => first_available(&self.small, excluded_versions),
            QueueLane::Verify => match capability {
                VerifyCapability::SmallCycleOnly => last_available(&self.small, excluded_versions),
                VerifyCapability::Any => {
                    match (
                        last_available(&self.small, excluded_versions),
                        last_available(&self.large, excluded_versions),
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
    ) -> bool {
        match lane.population(capability) {
            QueuePopulation::All => self.by_owner.contains_key(&owner),
            QueuePopulation::SmallOnly => self.small_owners.contains(&owner),
        }
    }

    fn next_owner(
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

    fn next(
        &self,
        lane: QueueLane,
        capability: VerifyCapability,
    ) -> Option<(WorkOwner, &QueueKey)> {
        // The first cut may prefer Trusted. Every committed checkout then
        // advances one shared owner ring, so newly queued Remote or Trusted
        // work receives service within one bounded owner traversal while a
        // sole owner can still borrow every global slot.
        let owner = if self.owner_cursor.is_none()
            && self.owner_is_eligible(lane, capability, WorkOwner::Trusted)
        {
            WorkOwner::Trusted
        } else {
            self.next_owner(lane, capability, self.owner_cursor)?
        };
        let key = self.by_owner.get(&owner)?.head(lane, capability)?;
        Some((owner, key))
    }

    fn overlay_owner_is_eligible(
        &self,
        overlay: &Self,
        lane: QueueLane,
        capability: VerifyCapability,
        owner: WorkOwner,
    ) -> bool {
        self.owner_is_eligible(lane, capability, owner)
            || overlay.owner_is_eligible(lane, capability, owner)
    }

    fn overlay_next_owner(
        &self,
        overlay: &Self,
        lane: QueueLane,
        capability: VerifyCapability,
        cursor: Option<WorkOwner>,
    ) -> Option<WorkOwner> {
        let choose = |left: Option<WorkOwner>, right: Option<WorkOwner>| match (left, right) {
            (Some(left), Some(right)) => Some(std::cmp::min(left, right)),
            (Some(owner), None) | (None, Some(owner)) => Some(owner),
            (None, None) => None,
        };
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
    }

    fn overlay_head_excluding<'lane>(
        &'lane self,
        overlay: &'lane Self,
        lane: QueueLane,
        capability: VerifyCapability,
        owner: WorkOwner,
        excluded_versions: &[EntryVersion],
    ) -> Option<&'lane QueueKey> {
        let current = self
            .by_owner
            .get(&owner)
            .and_then(|queue| queue.head_excluding(lane, capability, excluded_versions));
        let added = overlay
            .by_owner
            .get(&owner)
            .and_then(|queue| queue.head_excluding(lane, capability, excluded_versions));
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
    ) -> Option<(WorkOwner, &'lane QueueKey)> {
        if cursor.is_none()
            && self.overlay_owner_is_eligible(overlay, lane, capability, WorkOwner::Trusted)
            && let Some(key) = self.overlay_head_excluding(
                overlay,
                lane,
                capability,
                WorkOwner::Trusted,
                excluded_versions,
            )
        {
            return Some((WorkOwner::Trusted, key));
        }

        let owner_count = self
            .owner_count(lane, capability)
            .checked_add(overlay.owner_count(lane, capability))?;
        let mut cursor = cursor;
        for _ in 0..owner_count {
            let owner = self.overlay_next_owner(overlay, lane, capability, cursor)?;
            if let Some(key) =
                self.overlay_head_excluding(overlay, lane, capability, owner, excluded_versions)
            {
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
    verify_order: VerifyOrder,
}

/// Bounded virtual checkout cut. Selected versions are globally unique within
/// one authority generation, so this overlay can remove up to one worker wave
/// without cloning the scheduler or publishing a second queue authority.
pub(super) struct SchedulerWaveCursor {
    selected_versions: Vec<EntryVersion>,
    resolve_cursor: Option<WorkOwner>,
    verify_cursor: Option<WorkOwner>,
}

/// Mutable Plan-only view of the committed scheduler plus a bounded set of
/// owner-local settlement additions. Candidate probes do not advance
/// fairness; only consuming a selected ticket does. This lets the exchange
/// apply resource and dependency eligibility in canonical checkout order
/// without cloning the committed frontier.
pub(super) struct SchedulerExchangeWave<'frontier> {
    frontier: &'frontier FairFrontier,
    overlay: SchedulerWaveOverlay,
    cursor: SchedulerWaveCursor,
}

impl SchedulerExchangeWave<'_> {
    pub(super) fn next(&self, permit: super::state::WorkPermit) -> Option<CheckoutTicket> {
        self.frontier
            .next_queued_in_wave_with_overlay(&self.cursor, permit, &self.overlay)
    }

    pub(super) fn next_after(
        &self,
        permit: super::state::WorkPermit,
        owner: WorkOwner,
    ) -> Option<CheckoutTicket> {
        self.frontier.next_queued_after_in_wave_with_overlay(
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
        let (frontier, overlay) = match lane {
            QueueLane::Resolve => (&self.frontier.resolve, &self.overlay.resolve),
            QueueLane::Verify => (&self.frontier.verify, &self.overlay.verify),
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
                Some(before) if !self.contains(before) => return Err(SchedulerError::Projection),
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
            resolve_cursor: self.resolve.owner_cursor,
            verify_cursor: self.verify.owner_cursor,
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
        let mut delta = self.plan_batch(changes)?;
        delta.resolve_cursor = cursor.resolve_cursor;
        delta.verify_cursor = cursor.verify_cursor;
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
        })
    }

    pub(super) fn exchange_wave_after<'entry>(
        &self,
        settled: impl IntoIterator<Item = &'entry OwnedTx>,
        selection_bound: usize,
    ) -> Result<SchedulerExchangeWave<'_>, SchedulerError> {
        let mut overlay = SchedulerWaveOverlay::default();
        for owner in settled {
            match self.slot(owner)? {
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
        Ok(SchedulerExchangeWave {
            frontier: self,
            overlay,
            cursor: self.checkout_wave(selection_bound)?,
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
                .next(QueueLane::Resolve, VerifyCapability::Any)
                .map(|(_, key)| key.version()),
            verify_small: self
                .verify
                .next(QueueLane::Verify, VerifyCapability::SmallCycleOnly)
                .map(|(_, key)| key.version()),
            verify_any: self
                .verify
                .next(QueueLane::Verify, VerifyCapability::Any)
                .map(|(_, key)| key.version()),
            ready: self.ready.last().map(ReadyKey::version),
        }
    }

    pub(super) fn ready(&self) -> Result<Vec<(RawTxHash, EntryVersion)>, SchedulerError> {
        let count = self.ready.len().min(MAX_READY_BATCH);
        let mut ready = Vec::new();
        ready
            .try_reserve_exact(count)
            .map_err(|_| SchedulerError::Allocation)?;
        ready.extend(
            self.ready
                .iter()
                .rev()
                .take(MAX_READY_BATCH)
                .map(|key| (key.hash().clone(), key.version())),
        );
        Ok(ready)
    }

    /// Length of the longest captured prefix that is still the exact current
    /// strongest-first Ready prefix. This is one linear pass over the bounded
    /// scheduler frontier and allocates no second projection.
    pub(super) fn ready_common_prefix_len<'a>(
        &self,
        captured: impl IntoIterator<Item = (&'a RawTxHash, EntryVersion)>,
    ) -> usize {
        self.ready
            .iter()
            .rev()
            .take(MAX_READY_BATCH)
            .zip(captured)
            .take_while(|(current, captured)| {
                current.hash() == captured.0 && current.version() == captured.1
            })
            .count()
    }

    pub(super) fn apply(&mut self, delta: SchedulerDelta) {
        if let Some(before) = delta.before {
            self.remove(before);
        }
        if let Some(after) = delta.after {
            self.insert(after);
        }
        if let Some((lane, owner)) = delta.owner_cursor {
            match lane {
                QueueLane::Resolve => self.resolve.owner_cursor = Some(owner),
                QueueLane::Verify => self.verify.owner_cursor = Some(owner),
            }
        }
    }

    pub(super) fn apply_batch(&mut self, delta: SchedulerBatchDelta) {
        // A batch is a set transition, independent of the caller's change
        // order. Remove the complete old projection before publishing any new
        // slot so an exchange can never be lost to BTreeSet insertion order.
        for slot in delta.removed {
            self.remove(slot);
        }
        for slot in delta.added {
            self.insert(slot);
        }
        self.resolve.owner_cursor = delta.resolve_cursor;
        self.verify.owner_cursor = delta.verify_cursor;
    }

    fn contains(&self, slot: &SchedulerSlot) -> bool {
        match slot {
            SchedulerSlot::Queue { lane, owner, key } => match lane {
                QueueLane::Resolve => self.resolve.contains(*owner, key),
                QueueLane::Verify => self.verify.contains(*owner, key),
            },
            SchedulerSlot::Ready(key) => self.ready.contains(key),
        }
    }

    fn insert(&mut self, slot: SchedulerSlot) {
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

    fn remove(&mut self, slot: SchedulerSlot) {
        match slot {
            SchedulerSlot::Queue { lane, owner, key } => match lane {
                QueueLane::Resolve => self.resolve.remove(owner, &key),
                QueueLane::Verify => self.verify.remove(owner, &key),
            },
            SchedulerSlot::Ready(key) => {
                self.ready.remove(&key);
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
