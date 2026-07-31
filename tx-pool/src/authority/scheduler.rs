use super::state::{
    Arrival, EntryVersion, OwnedTx, PreAcceptedEntry, PreAcceptedPhase, PreAcceptedSource,
    RawTxHash, VerifyCapability, VerifyCycleClass,
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
        // Both operands originate as `u64`, so the mathematical product fits
        // in `u128`; saturating multiplication makes that proof explicit to
        // the production arithmetic lint without changing the ordering.
        let left_rate = u128::from(self.fee).saturating_mul(u128::from(other.serialized_bytes));
        let right_rate = u128::from(other.fee).saturating_mul(u128::from(self.serialized_bytes));
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
        let left_rate = u128::from(self.fee).saturating_mul(u128::from(other.serialized_bytes));
        let right_rate = u128::from(other.fee).saturating_mul(u128::from(self.serialized_bytes));
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
    owner_cursor: Option<(QueueLane, WorkOwner)>,
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
    owner_cursor: Option<WorkOwner>,
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

    fn next_after(
        &self,
        lane: QueueLane,
        capability: VerifyCapability,
        cursor: Option<WorkOwner>,
    ) -> Option<(WorkOwner, &QueueKey)> {
        let owner = self.next_owner(lane, capability, cursor)?;
        let key = self.by_owner.get(&owner)?.head(lane, capability)?;
        Some((owner, key))
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

    fn owner_count(&self, lane: QueueLane, capability: VerifyCapability) -> usize {
        match lane.population(capability) {
            QueuePopulation::All => self.by_owner.len(),
            QueuePopulation::SmallOnly => self.small_owners.len(),
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
    resolve_owner_cursor: Option<WorkOwner>,
    verify_owner_cursor: Option<WorkOwner>,
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
        let changes = changes
            .into_iter()
            .map(|(before, after)| {
                Ok(SchedulerDelta {
                    before: before.map(|owner| self.slot(owner)).transpose()?.flatten(),
                    after: after.map(|owner| self.slot(owner)).transpose()?.flatten(),
                    owner_cursor: None,
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
            resolve_owner_cursor: self.resolve.owner_cursor,
            verify_owner_cursor: self.verify.owner_cursor,
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
