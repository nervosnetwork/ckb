use super::{
    Arrival, EntryRevision, PrePoolError, PrePoolSource, VerifySchedule, WorkCapability, WorkLane,
};
use ckb_network::PeerIndex;
use ckb_types::packed::Byte32;
use ckb_types::prelude::Entity;
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum WorkOwner {
    Remote(PeerIndex),
    Trusted,
}

impl From<PrePoolSource> for WorkOwner {
    fn from(source: PrePoolSource) -> Self {
        match source {
            PrePoolSource::Remote(remote) => Self::Remote(remote.peer),
            PrePoolSource::Proposal | PrePoolSource::Recovery => Self::Trusted,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct WorkKey {
    pub(super) hash: Byte32,
    pub(super) revision: EntryRevision,
    pub(super) source: PrePoolSource,
    pub(super) arrival: Arrival,
    pub(super) schedule: VerifySchedule,
    pub(super) fee_ordered: bool,
}

impl Ord for WorkKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.source
            .priority()
            .cmp(&other.source.priority())
            .then_with(|| self.fee_ordered.cmp(&other.fee_ordered))
            .then_with(|| {
                if self.fee_ordered {
                    let left = u128::from(self.schedule.fee_rate_per_kb);
                    let right = u128::from(other.schedule.fee_rate_per_kb);
                    left.cmp(&right)
                } else {
                    Ordering::Equal
                }
            })
            // Earlier arrival and then smaller full hash win. The queue takes
            // its greatest key, so both stable tie breakers are reversed.
            .then_with(|| other.arrival.cmp(&self.arrival))
            .then_with(|| other.hash.as_slice().cmp(self.hash.as_slice()))
            .then_with(|| self.revision.cmp(&other.revision))
            .then_with(|| self.source.cmp(&other.source))
            .then_with(|| self.schedule.is_large_cycle.cmp(&other.schedule.is_large_cycle))
            .then_with(|| self.schedule.fee_rate_per_kb.cmp(&other.schedule.fee_rate_per_kb))
    }
}

impl PartialOrd for WorkKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RunnableHead {
    // Smaller turns win; reversed because BTreeSet::last is selected.
    turn: std::cmp::Reverse<u128>,
    work: WorkKey,
    owner: WorkOwner,
}

impl Ord for RunnableHead {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.owner == WorkOwner::Trusted)
            .cmp(&(other.owner == WorkOwner::Trusted))
            .then_with(|| self.turn.cmp(&other.turn))
            .then_with(|| self.work.cmp(&other.work))
            .then_with(|| self.owner.cmp(&other.owner))
    }
}

impl PartialOrd for RunnableHead {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Default)]
struct OwnerQueue {
    work: BTreeSet<WorkKey>,
    turn: u128,
    runnable: bool,
}

/// Exact payload-free scheduler. Each owner contributes at most one head and
/// capped owners contribute none, so checkout work is bounded by owners rather
/// than by an attacker-controlled transaction prefix. There are no stale heap
/// tickets or compaction thresholds.
#[derive(Debug)]
pub(super) struct FairQueue {
    lane: WorkLane,
    owners: HashMap<WorkOwner, OwnerQueue>,
    heads: BTreeSet<RunnableHead>,
    small_cycle_heads: BTreeSet<RunnableHead>,
    len: usize,
    next_turn: u128,
}

impl FairQueue {
    pub(super) fn new(lane: WorkLane) -> Self {
        Self {
            lane,
            owners: HashMap::new(),
            heads: BTreeSet::new(),
            small_cycle_heads: BTreeSet::new(),
            len: 0,
            next_turn: 0,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.len
    }

    fn head_for(
        owner: WorkOwner,
        queue: &OwnerQueue,
        capability: WorkCapability,
    ) -> Option<RunnableHead> {
        if !queue.runnable {
            return None;
        }
        let work = match capability {
            WorkCapability::Any => queue.work.last(),
            WorkCapability::SmallCycleOnly => queue
                .work
                .iter()
                .rev()
                .find(|key| !key.schedule.is_large_cycle),
        }?;
        Some(RunnableHead {
            turn: std::cmp::Reverse(queue.turn),
            work: work.clone(),
            owner,
        })
    }

    fn remove_head(&mut self, owner: WorkOwner) {
        if let Some(queue) = self.owners.get(&owner) {
            if let Some(head) = Self::head_for(owner, queue, WorkCapability::Any) {
                self.heads.remove(&head);
            }
            if self.lane == WorkLane::Verify
                && let Some(head) = Self::head_for(owner, queue, WorkCapability::SmallCycleOnly)
            {
                self.small_cycle_heads.remove(&head);
            }
        }
    }

    fn insert_head(&mut self, owner: WorkOwner) {
        if let Some(queue) = self.owners.get(&owner) {
            if let Some(head) = Self::head_for(owner, queue, WorkCapability::Any) {
                self.heads.insert(head);
            }
            if self.lane == WorkLane::Verify
                && let Some(head) = Self::head_for(owner, queue, WorkCapability::SmallCycleOnly)
            {
                self.small_cycle_heads.insert(head);
            }
        }
    }

    pub(super) fn contains(&self, key: &WorkKey) -> bool {
        self.owners
            .get(&WorkOwner::from(key.source))
            .is_some_and(|queue| queue.work.contains(key))
    }

    pub(super) fn apply_insert(&mut self, key: WorkKey) {
        let owner = WorkOwner::from(key.source);
        self.remove_head(owner);
        self.owners
            .entry(owner)
            .or_insert_with(|| OwnerQueue {
                runnable: true,
                ..OwnerQueue::default()
            })
            .work
            .insert(key);
        self.insert_head(owner);
    }

    pub(super) fn apply_remove(&mut self, key: &WorkKey) {
        self.apply_remove_with_turn(key, None);
    }

    fn apply_remove_with_turn(&mut self, key: &WorkKey, turn: Option<u128>) {
        let owner = WorkOwner::from(key.source);
        self.remove_head(owner);
        let empty = self.owners.get_mut(&owner).is_none_or(|queue| {
            queue.work.remove(key);
            if let Some(turn) = turn {
                queue.turn = turn;
            }
            queue.work.is_empty()
        });
        if let Some(turn) = turn {
            self.next_turn = turn;
        }
        if empty {
            self.owners.remove(&owner);
        } else {
            self.insert_head(owner);
        }
    }

    pub(super) fn plan_checkout(
        &self,
        key: &WorkKey,
        capability: WorkCapability,
    ) -> Result<u128, PrePoolError> {
        let heads = match (self.lane, capability) {
            (WorkLane::Verify, WorkCapability::SmallCycleOnly) => &self.small_cycle_heads,
            _ => &self.heads,
        };
        let Some(head) = heads.last() else {
            return Err(PrePoolError::ProjectionInconsistent(
                "checkout queue has no runnable head",
            ));
        };
        if &head.work != key {
            return Err(PrePoolError::ProjectionInconsistent(
                "checkout ticket is not the selected runnable head",
            ));
        }
        let Some(owner) = self.owners.get(&head.owner) else {
            return Err(PrePoolError::ProjectionInconsistent(
                "runnable head has no owner queue",
            ));
        };
        if Self::head_for(head.owner, owner, capability).as_ref() != Some(head) {
            return Err(PrePoolError::ProjectionInconsistent(
                "runnable head does not match its owner queue",
            ));
        }
        self.next_turn
            .checked_add(1)
            .ok_or(PrePoolError::CounterExhausted)
    }

    pub(super) fn apply_checkout(&mut self, key: &WorkKey, next_turn: u128) {
        self.apply_remove_with_turn(key, Some(next_turn));
    }

    pub(super) fn set_len(&mut self, len: usize) {
        self.len = len;
    }

    pub(super) fn set_runnable(&mut self, owner: WorkOwner, runnable: bool) {
        self.remove_head(owner);
        if let Some(queue) = self.owners.get_mut(&owner) {
            queue.runnable = runnable;
        }
        self.insert_head(owner);
    }

    pub(super) fn peek(&self, capability: WorkCapability) -> Option<&WorkKey> {
        match (self.lane, capability) {
            (WorkLane::Verify, WorkCapability::SmallCycleOnly) => self.small_cycle_heads.last(),
            _ => self.heads.last(),
        }
        .map(|head| &head.work)
    }
}

#[cfg(test)]
#[path = "../tests/pre_pool_queue.rs"]
mod tests;
