use super::state::{
    AdmissionClass, Arrival, EntryVersion, OwnedTx, PreAcceptedEntry, PreAcceptedPhase, RawTxHash,
    VerifyCapability, VerifyCycleClass,
};
use ckb_network::PeerIndex;
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    ops::Bound::{Excluded, Unbounded},
};

pub(super) const MAX_READY_BATCH: usize = 8;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum WorkOwner {
    Remote(PeerIndex),
    Trusted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum SourcePriority {
    Remote,
    Proposal,
    Recovery,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum VerifyOrder {
    #[default]
    Arrival,
    FeeRate,
}

impl From<AdmissionClass> for SourcePriority {
    fn from(class: AdmissionClass) -> Self {
        match class {
            AdmissionClass::Remote(_) => Self::Remote,
            AdmissionClass::Proposal(_) => Self::Proposal,
            AdmissionClass::Recovery(_) => Self::Recovery,
        }
    }
}

impl WorkOwner {
    fn from_class(class: AdmissionClass) -> Self {
        match class {
            AdmissionClass::Remote(lease) => Self::Remote(lease.peer),
            AdmissionClass::Proposal(_) | AdmissionClass::Recovery(_) => Self::Trusted,
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum QueueLane {
    Resolve,
    Verify,
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
        let left_rate = u128::from(self.fee) * u128::from(other.serialized_bytes);
        let right_rate = u128::from(other.fee) * u128::from(self.serialized_bytes);
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
    pub(super) fn from_computed(entry: &PreAcceptedEntry) -> Result<Self, SchedulerError> {
        let PreAcceptedPhase::Computed(super::state::ComputedOutcome::Verified(verified)) =
            &entry.phase
        else {
            return Err(SchedulerError::Projection);
        };
        let serialized_bytes = u64::try_from(verified.metrics().cost.serialized_bytes)
            .map_err(|_| SchedulerError::Arithmetic)?;
        if serialized_bytes == 0 {
            return Err(SchedulerError::Projection);
        }
        Ok(Self {
            source: entry.record.class.into(),
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
        let left_rate = u128::from(self.fee) * u128::from(other.serialized_bytes);
        let right_rate = u128::from(other.fee) * u128::from(self.serialized_bytes);
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
    remote_cursor: Option<(QueueLane, PeerIndex)>,
}

pub(super) struct SchedulerBatchDelta {
    removed: BTreeSet<SchedulerSlot>,
    added: BTreeSet<SchedulerSlot>,
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

    fn is_empty(&self) -> bool {
        self.small.is_empty() && self.large.is_empty()
    }
}

#[derive(Debug, Default)]
struct FairLane {
    by_owner: BTreeMap<WorkOwner, OwnerQueue>,
    small_owners: BTreeSet<WorkOwner>,
    remote_cursor: Option<PeerIndex>,
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
        if class == VerifyCycleClass::Small {
            self.small_owners.insert(owner);
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

    fn trusted_head(&self, lane: QueueLane, capability: VerifyCapability) -> Option<&QueueKey> {
        if lane == QueueLane::Verify
            && capability == VerifyCapability::SmallCycleOnly
            && !self.small_owners.contains(&WorkOwner::Trusted)
        {
            return None;
        }
        self.by_owner
            .get(&WorkOwner::Trusted)?
            .head(lane, capability)
    }

    fn next_remote_owner(
        &self,
        lane: QueueLane,
        capability: VerifyCapability,
        cursor: Option<PeerIndex>,
    ) -> Option<WorkOwner> {
        let lower = match cursor {
            Some(peer) => Excluded(WorkOwner::Remote(peer)),
            None => Unbounded,
        };
        if lane == QueueLane::Verify && capability == VerifyCapability::SmallCycleOnly {
            self.small_owners
                .range((lower, Excluded(WorkOwner::Trusted)))
                .next()
                .copied()
                .or_else(|| {
                    self.small_owners
                        .range((Unbounded, Excluded(WorkOwner::Trusted)))
                        .next()
                        .copied()
                })
        } else {
            self.by_owner
                .range((lower, Excluded(WorkOwner::Trusted)))
                .next()
                .map(|(owner, _)| *owner)
                .or_else(|| {
                    self.by_owner
                        .range((Unbounded, Excluded(WorkOwner::Trusted)))
                        .next()
                        .map(|(owner, _)| *owner)
                })
        }
    }

    fn next_after(
        &self,
        lane: QueueLane,
        capability: VerifyCapability,
        cursor: Option<WorkOwner>,
    ) -> Option<(WorkOwner, &QueueKey)> {
        // `next` considers Trusted exactly once. Enumeration after that first
        // candidate walks one complete Remote ring without revisiting Trusted
        // or an already examined peer. If Trusted was first, start the ring at
        // the persistent fairness cursor rather than at the smallest peer.
        let remote_cursor = match cursor {
            Some(WorkOwner::Remote(peer)) => Some(peer),
            Some(WorkOwner::Trusted) | None => self.remote_cursor,
        };
        let owner = self.next_remote_owner(lane, capability, remote_cursor)?;
        let key = self.by_owner.get(&owner)?.head(lane, capability)?;
        Some((owner, key))
    }

    fn next(
        &self,
        lane: QueueLane,
        capability: VerifyCapability,
    ) -> Option<(WorkOwner, &QueueKey)> {
        self.trusted_head(lane, capability)
            .map(|key| (WorkOwner::Trusted, key))
            .or_else(|| {
                let owner = self.next_remote_owner(lane, capability, self.remote_cursor)?;
                let key = self.by_owner.get(&owner)?.head(lane, capability)?;
                Some((owner, key))
            })
    }

    fn owner_count(&self, lane: QueueLane, capability: VerifyCapability) -> usize {
        if lane == QueueLane::Verify && capability == VerifyCapability::SmallCycleOnly {
            self.small_owners.len()
        } else {
            self.by_owner.len()
        }
    }

    fn secondary_index_consistent(&self) -> bool {
        self.small_owners
            == self
                .by_owner
                .iter()
                .filter_map(|(owner, queue)| (!queue.small.is_empty()).then_some(*owner))
                .collect()
    }
}

#[derive(Debug)]
pub(super) struct FairFrontier {
    resolve: FairLane,
    verify: FairLane,
    ready: BTreeSet<ReadyKey>,
    verify_order: VerifyOrder,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SchedulerSnapshot {
    verify_order: VerifyOrder,
    slots: BTreeSet<SchedulerSlot>,
    resolve_remote_cursor: Option<PeerIndex>,
    verify_remote_cursor: Option<PeerIndex>,
    resolve_small_owners: BTreeSet<WorkOwner>,
    verify_small_owners: BTreeSet<WorkOwner>,
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
        let owner = WorkOwner::from_class(record.class);
        let slot = match &entry.phase {
            PreAcceptedPhase::Queued(super::state::QueuedWork::Resolve) => SchedulerSlot::Queue {
                lane: QueueLane::Resolve,
                owner,
                key: QueueKey::Resolve(ResolveKey {
                    source: record.class.into(),
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
                        source: record.class.into(),
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
            PreAcceptedPhase::Computed(super::state::ComputedOutcome::Verified(_)) => {
                SchedulerSlot::Ready(ReadyKey::from_computed(entry)?)
            }
            PreAcceptedPhase::Computing(_)
            | PreAcceptedPhase::Waiting(_)
            | PreAcceptedPhase::Computed(_) => return Ok(None),
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
        let remote_cursor = match checkout {
            Some(ticket) => {
                let selected = SchedulerSlot::Queue {
                    lane: ticket.lane,
                    owner: ticket.owner,
                    key: ticket.key,
                };
                if before.as_ref() != Some(&selected) || after.is_some() {
                    return Err(SchedulerError::Projection);
                }
                match ticket.owner {
                    WorkOwner::Remote(peer) => Some((ticket.lane, peer)),
                    WorkOwner::Trusted => None,
                }
            }
            None => None,
        };
        Ok(SchedulerDelta {
            before,
            after,
            remote_cursor,
        })
    }

    pub(super) fn plan_batch<'entry>(
        &self,
        changes: impl IntoIterator<Item = (Option<&'entry OwnedTx>, Option<&'entry OwnedTx>)>,
    ) -> Result<SchedulerBatchDelta, SchedulerError> {
        let changes = changes
            .into_iter()
            .map(|(before, after)| {
                Ok(SchedulerDelta {
                    before: before.map(|owner| self.slot(owner)).transpose()?.flatten(),
                    after: after.map(|owner| self.slot(owner)).transpose()?.flatten(),
                    remote_cursor: None,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut removed = BTreeSet::new();
        let mut added = BTreeSet::new();
        for change in &changes {
            match &change.before {
                Some(before) if !self.contains(before) || !removed.insert(before.clone()) => {
                    return Err(SchedulerError::Projection);
                }
                _ => {}
            }
            match &change.after {
                Some(after) if !added.insert(after.clone()) => {
                    return Err(SchedulerError::Projection);
                }
                _ => {}
            }
        }
        if added
            .iter()
            .any(|slot| self.contains(slot) && !removed.contains(slot))
        {
            return Err(SchedulerError::Projection);
        }
        Ok(SchedulerBatchDelta { removed, added })
    }

    pub(super) fn next_queued(&self, permit: super::state::WorkPermit) -> Option<CheckoutTicket> {
        let lane = QueueLane::for_permit(permit);
        let capability = QueueLane::capability(permit);
        let frontier = match lane {
            QueueLane::Resolve => &self.resolve,
            QueueLane::Verify => &self.verify,
        };
        frontier
            .next(lane, capability)
            .map(|(owner, key)| CheckoutTicket {
                lane,
                owner,
                key: key.clone(),
            })
    }

    pub(super) fn next_queued_after(
        &self,
        permit: super::state::WorkPermit,
        cursor: Option<WorkOwner>,
    ) -> Option<CheckoutTicket> {
        let lane = QueueLane::for_permit(permit);
        let capability = QueueLane::capability(permit);
        let frontier = match lane {
            QueueLane::Resolve => &self.resolve,
            QueueLane::Verify => &self.verify,
        };
        frontier
            .next_after(lane, capability, cursor)
            .map(|(owner, key)| CheckoutTicket {
                lane,
                owner,
                key: key.clone(),
            })
    }

    pub(super) fn owner_count(&self, permit: super::state::WorkPermit) -> usize {
        let lane = QueueLane::for_permit(permit);
        let capability = QueueLane::capability(permit);
        match lane {
            QueueLane::Resolve => self.resolve.owner_count(lane, capability),
            QueueLane::Verify => self.verify.owner_count(lane, capability),
        }
    }

    pub(super) fn ready(&self) -> Vec<(RawTxHash, EntryVersion)> {
        self.ready
            .iter()
            .rev()
            .take(MAX_READY_BATCH)
            .map(|key| (key.hash().clone(), key.version()))
            .collect()
    }

    pub(super) fn snapshot(&self) -> SchedulerSnapshot {
        SchedulerSnapshot {
            verify_order: self.verify_order,
            slots: self.slots(),
            resolve_remote_cursor: self.resolve.remote_cursor,
            verify_remote_cursor: self.verify.remote_cursor,
            resolve_small_owners: self.resolve.small_owners.clone(),
            verify_small_owners: self.verify.small_owners.clone(),
        }
    }

    pub(super) fn apply(&mut self, delta: SchedulerDelta) {
        if let Some(before) = delta.before {
            self.remove(before);
        }
        if let Some(after) = delta.after {
            self.insert(after);
        }
        if let Some((lane, peer)) = delta.remote_cursor {
            match lane {
                QueueLane::Resolve => self.resolve.remote_cursor = Some(peer),
                QueueLane::Verify => self.verify.remote_cursor = Some(peer),
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

    #[cfg(test)]
    pub(super) fn semantically_matches(
        &self,
        entries: &std::collections::HashMap<RawTxHash, OwnedTx>,
    ) -> bool {
        let Ok(expected) = entries
            .values()
            .map(|owner| self.slot(owner))
            .collect::<Result<Vec<_>, _>>()
        else {
            return false;
        };
        let expected = expected.into_iter().flatten().collect::<BTreeSet<_>>();
        self.resolve.secondary_index_consistent()
            && self.verify.secondary_index_consistent()
            && self.slots() == expected
    }

    fn slots(&self) -> BTreeSet<SchedulerSlot> {
        let mut actual = BTreeSet::new();
        for (owner, entries) in &self.resolve.by_owner {
            actual.extend(
                entries
                    .small
                    .iter()
                    .chain(&entries.large)
                    .cloned()
                    .map(|key| SchedulerSlot::Queue {
                        lane: QueueLane::Resolve,
                        owner: *owner,
                        key,
                    }),
            );
        }
        for (owner, entries) in &self.verify.by_owner {
            actual.extend(
                entries
                    .small
                    .iter()
                    .chain(&entries.large)
                    .cloned()
                    .map(|key| SchedulerSlot::Queue {
                        lane: QueueLane::Verify,
                        owner: *owner,
                        key,
                    }),
            );
        }
        actual.extend(self.ready.iter().cloned().map(SchedulerSlot::Ready));
        actual
    }
}

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
