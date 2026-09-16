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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum WorkOwner {
    Trusted,
    Peer(PeerIndex),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    source: u8,
    fee: Option<(u64, u64)>,
    arrival: u64,
    hash: Byte32,
}
impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        self.source
            .cmp(&other.source)
            .then_with(|| match (self.fee, other.fee) {
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
#[derive(Default)]
struct Lane {
    owners: BTreeMap<WorkOwner, OwnerQueue>,
    cursor: Option<WorkOwner>,
    // Exact entries across small/large maps, including uncollected stale Weak values.
    len: usize,
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
    fn key(&self, entry: &Entry) -> Option<(WorkStage, bool, WorkOwner, Key)> {
        let (stage, fee) = match &entry.phase {
            Phase::Resolve => (WorkStage::Resolve, None),
            Phase::Verify(resolved) => (
                WorkStage::Verify,
                match self.order {
                    VerifyOrdering::ArrivalTime => None,
                    VerifyOrdering::FeeRate => Some((
                        resolved.fee.as_u64(),
                        u64::try_from(entry.transaction.data().serialized_size_in_block()).ok()?,
                    )),
                },
            ),
            _ => return None,
        };
        let large = entry
            .source
            .declared_cycles()
            .is_some_and(|cycles| cycles > self.large_threshold);
        let owner = entry
            .source
            .compute_peer()
            .map_or(WorkOwner::Trusted, WorkOwner::Peer);
        Some((
            stage,
            large,
            owner,
            Key {
                source: entry.source.priority(),
                fee,
                arrival: entry.arrival,
                hash: entry.hash(),
            },
        ))
    }
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "The count grows only for a newly allocated map entry; live entries cannot exceed usize."
    )]
    pub(super) fn insert(&self, entry: &Arc<Entry>) {
        let Some((stage, large, owner, key)) = self.key(entry) else {
            return;
        };
        let mut lane = self.lane(stage).lock();
        let queue = lane.owners.entry(owner).or_default();
        let entries = if large {
            &mut queue.large
        } else {
            &mut queue.small
        };
        if entries.insert(key, Arc::downgrade(entry)).is_none() {
            lane.len += 1;
        }
    }
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "Each successful removal owns one entry already included in the lane count."
    )]
    pub(super) fn remove(&self, entry: &Arc<Entry>) {
        let Some((stage, large, owner, key)) = self.key(entry) else {
            return;
        };
        let mut lane = self.lane(stage).lock();
        if let Some(queue) = lane.owners.get_mut(&owner) {
            let entries = if large {
                &mut queue.large
            } else {
                &mut queue.small
            };
            let removed = entries
                .get(&key)
                .is_some_and(|old| old.ptr_eq(&Arc::downgrade(entry)))
                && entries.remove(&key).is_some();
            let empty = queue.small.is_empty() && queue.large.is_empty();
            if removed {
                lane.len -= 1;
            }
            if empty {
                lane.owners.remove(&owner);
            }
        }
    }
    /// Active memory/peer capacity is reserved before removing the exact item.
    /// Neither this queue nor the budget mutex ever waits for an owner lock.
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "Each successful pop removes one entry already included in the lane count."
    )]
    pub(super) fn pop(
        &self,
        stage: WorkStage,
        small_only: bool,
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
                    .map(|(key, entry)| (false, key, entry));
                let large = (!small_only)
                    .then(|| queue.large.first_key_value())
                    .flatten()
                    .map(|(key, entry)| (true, key, entry));
                small
                    .into_iter()
                    .chain(large)
                    .min_by(|a, b| a.1.cmp(b.1))
                    .map(|(large, key, entry)| (large, key.clone(), entry.upgrade()))
            });
            let Some((large, key, entry)) = selected else {
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
            if let Some(queue) = lane.owners.get_mut(&owner) {
                let removed = if large {
                    queue.large.remove(&key)
                } else {
                    queue.small.remove(&key)
                };
                let empty = queue.small.is_empty() && queue.large.is_empty();
                if removed.is_some() {
                    lane.len -= 1;
                }
                if empty {
                    lane.owners.remove(&owner);
                }
            }
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
