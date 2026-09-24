//! Two factual queues with separate locks. Pop releases its lane before acquiring owner guards.
use super::{
    budget::{ActivePermit, Budget},
    model::{Entry, Error, FullReason, Phase},
};
use ckb_app_config::VerifyOrdering;
use ckb_network::PeerIndex;
use ckb_types::packed::Byte32;
use ckb_util::parking_lot::Mutex;
use std::{
    cmp::Ordering,
    collections::BTreeMap,
    ops::Bound::{Excluded, Unbounded},
    sync::{Arc, Weak},
};

/// The queue projection of a runnable owner phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkStage {
    Resolve,
    Verify,
}

impl WorkStage {
    pub(super) fn for_phase(phase: &Phase) -> Option<Self> {
        match phase {
            Phase::Resolve => Some(Self::Resolve),
            Phase::Verify(_) => Some(Self::Verify),
            Phase::Waiting(_) | Phase::Accepted(_) | Phase::Replaced(_) => None,
        }
    }
}

/// A worker's selection range. `Any` compares both queues by their normal key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkSelection {
    SmallOnly,
    Any,
}

#[derive(Clone, Copy)]
enum SizeClass {
    Small,
    Large,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum WorkOwner {
    Trusted,
    Peer(PeerIndex),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    source_priority: u8,
    fee_and_size: Option<(u64, u64)>,
    arrival: u64,
    hash: Byte32,
}
impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        self.source_priority
            .cmp(&other.source_priority)
            .then_with(|| match (self.fee_and_size, other.fee_and_size) {
                (Some((a, size_a)), Some((b, size_b))) => {
                    crate::util::fee_rate_cross_product(b, size_a)
                        .cmp(&crate::util::fee_rate_cross_product(a, size_b))
                        .then_with(|| b.cmp(&a))
                }
                (a, b) => a.cmp(&b),
            })
            .then_with(|| self.arrival.cmp(&other.arrival))
            .then_with(|| self.hash.cmp(&other.hash))
    }
}
impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Default)]
struct OwnerQueue {
    small: BTreeMap<Key, Weak<Entry>>,
    large: BTreeMap<Key, Weak<Entry>>,
}
impl OwnerQueue {
    fn entries_mut(&mut self, size: SizeClass) -> &mut BTreeMap<Key, Weak<Entry>> {
        match size {
            SizeClass::Small => &mut self.small,
            SizeClass::Large => &mut self.large,
        }
    }
}

struct Placement {
    stage: WorkStage,
    size: SizeClass,
    owner: WorkOwner,
    key: Key,
}

#[derive(Default)]
struct Lane {
    owners: BTreeMap<WorkOwner, OwnerQueue>,
    cursor: Option<WorkOwner>,
    // Exact entries across small/large maps, including uncollected stale Weak values.
    len: usize,
}

impl Lane {
    /// Remove a key and settle its owner row and the lane count together.
    /// Owner updates name the exact entry; Pop already selected under this lock.
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "Each successful removal owns one entry already included in the lane count."
    )]
    fn remove(
        &mut self,
        owner: WorkOwner,
        size: SizeClass,
        key: &Key,
        expected: Option<&Arc<Entry>>,
    ) {
        let Some(queue) = self.owners.get_mut(&owner) else {
            return;
        };
        let entries = queue.entries_mut(size);
        let matches = expected.is_none_or(|entry| {
            entries
                .get(key)
                .is_some_and(|old| old.ptr_eq(&Arc::downgrade(entry)))
        });
        if matches && entries.remove(key).is_some() {
            self.len -= 1;
        }
        if queue.small.is_empty() && queue.large.is_empty() {
            self.owners.remove(&owner);
        }
    }
}

pub(super) struct Queues {
    resolve: Mutex<Lane>,
    verify: Mutex<Lane>,
    order: VerifyOrdering,
    large_threshold: u64,
}

impl Queues {
    pub(super) fn new(order: VerifyOrdering, large_threshold: u64) -> Self {
        Self {
            resolve: Mutex::new(Lane::default()),
            verify: Mutex::new(Lane::default()),
            order,
            large_threshold,
        }
    }

    /// Remaining work in both queues; selected jobs have already been removed.
    /// Complete summary capture holds owner guards while taking these lanes.
    pub(super) fn queued_len(&self) -> usize {
        let resolve = self.resolve.lock();
        let verify = self.verify.lock();
        [resolve.len, verify.len].into_iter().sum()
    }

    pub(super) fn take(&self) -> Self {
        // Clear owns the lifecycle and all owner guards. Like summary capture,
        // it takes resolve before verify; Pop holds no owner guard.
        let mut resolve = self.resolve.lock();
        let mut verify = self.verify.lock();
        Self {
            resolve: Mutex::new(std::mem::take(&mut *resolve)),
            verify: Mutex::new(std::mem::take(&mut *verify)),
            order: self.order,
            large_threshold: self.large_threshold,
        }
    }

    fn lane(&self, stage: WorkStage) -> &Mutex<Lane> {
        match stage {
            WorkStage::Resolve => &self.resolve,
            WorkStage::Verify => &self.verify,
        }
    }

    fn placement(&self, entry: &Entry) -> Option<Placement> {
        let stage = WorkStage::for_phase(&entry.phase)?;
        let fee_and_size = match (&entry.phase, self.order) {
            (Phase::Verify(resolved), VerifyOrdering::FeeRate) => Some((
                resolved.fee.as_u64(),
                u64::try_from(entry.transaction.data().serialized_size_in_block()).ok()?,
            )),
            _ => None,
        };
        let size = if entry
            .source
            .declared_cycles()
            .is_some_and(|cycles| cycles > self.large_threshold)
        {
            SizeClass::Large
        } else {
            SizeClass::Small
        };
        let owner = entry
            .source
            .compute_peer()
            .map_or(WorkOwner::Trusted, WorkOwner::Peer);
        Some(Placement {
            stage,
            size,
            owner,
            key: Key {
                source_priority: entry.source.priority(),
                fee_and_size,
                arrival: entry.arrival,
                hash: entry.hash(),
            },
        })
    }

    #[expect(
        clippy::arithmetic_side_effects,
        reason = "The count grows only for a newly allocated map entry; live entries cannot exceed usize."
    )]
    pub(super) fn insert(&self, entry: &Arc<Entry>) {
        let Some(Placement {
            stage,
            size,
            owner,
            key,
        }) = self.placement(entry)
        else {
            return;
        };
        let mut lane = self.lane(stage).lock();
        let queue = lane.owners.entry(owner).or_default();
        let entries = queue.entries_mut(size);
        if entries.insert(key, Arc::downgrade(entry)).is_none() {
            lane.len += 1;
        }
    }

    pub(super) fn remove(&self, entry: &Arc<Entry>) {
        let Some(Placement {
            stage,
            size,
            owner,
            key,
        }) = self.placement(entry)
        else {
            return;
        };
        self.lane(stage)
            .lock()
            .remove(owner, size, &key, Some(entry));
    }

    /// Active memory/peer capacity is reserved before removing the exact item.
    /// Neither this queue nor the budget mutex ever waits for an owner lock.
    pub(super) fn pop(
        &self,
        stage: WorkStage,
        selection: WorkSelection,
        budget: &Arc<Budget>,
    ) -> Result<Option<(Arc<Entry>, ActivePermit)>, Error> {
        let mut lane = self.lane(stage).lock();
        let mut cursor = lane.cursor;
        let bound = lane.owners.len();
        for _ in 0..bound {
            let next = lane
                .owners
                .range((cursor.map_or(Unbounded, Excluded), Unbounded))
                .next()
                .or_else(|| lane.owners.first_key_value())
                .map(|(owner, _)| *owner);
            let Some(owner) = next else {
                return Ok(None);
            };
            cursor = Some(owner);
            let selected = lane.owners.get(&owner).and_then(|queue| {
                let small = queue
                    .small
                    .first_key_value()
                    .map(|(key, entry)| (SizeClass::Small, key, entry));
                let large = match selection {
                    WorkSelection::SmallOnly => None,
                    WorkSelection::Any => queue.large.first_key_value(),
                }
                .map(|(key, entry)| (SizeClass::Large, key, entry));
                small
                    .into_iter()
                    .chain(large)
                    .min_by(|(_, a, _), (_, b, _)| a.cmp(b))
                    .map(|(size, key, entry)| (size, key.clone(), entry.upgrade()))
            });
            let Some((size, key, entry)) = selected else {
                continue;
            };
            let selected = match entry {
                Some(entry) => match budget.active(entry.source) {
                    Ok(reservation) => Some((entry, reservation)),
                    // Total capacity blocks every peer and both work stages.
                    Err(error @ Error::Full(FullReason::Active)) => return Err(error),
                    Err(Error::Full(_)) => continue,
                    Err(error) => return Err(error),
                },
                None => None,
            };
            lane.remove(owner, size, &key, None);
            if let Some(selected) = selected {
                lane.cursor = Some(owner);
                return Ok(Some(selected));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
impl Queues {
    /// Preserve stale Weak entries in the observation so tests can detect them.
    pub(super) fn queued_owners(&self) -> [Vec<Weak<Entry>>; 2] {
        [&self.resolve, &self.verify].map(|lane| {
            let lane = lane.lock();
            assert!(
                lane.owners
                    .values()
                    .all(|owner| !owner.small.is_empty() || !owner.large.is_empty())
            );
            lane.owners
                .values()
                .flat_map(|owner| owner.small.values().chain(owner.large.values()))
                .cloned()
                .collect()
        })
    }
}

#[cfg(test)]
#[path = "tests/queue.rs"]
mod tests;
