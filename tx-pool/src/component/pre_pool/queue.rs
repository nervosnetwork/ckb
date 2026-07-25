use super::{EntryVersion, PrePoolError, PrePoolSource, VerifySchedule, WorkCapability, WorkLane};
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
            PrePoolSource::Remote(peer) => Self::Remote(peer),
            PrePoolSource::Proposal => Self::Trusted,
        }
    }
}

#[derive(Clone, Debug, Eq)]
pub(super) struct WorkKey {
    pub(super) hash: Byte32,
    pub(super) version: EntryVersion,
    pub(super) source: PrePoolSource,
    pub(super) arrival: u128,
    pub(super) schedule: VerifySchedule,
    pub(super) fee_ordered: bool,
}

impl PartialEq for WorkKey {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash
            && self.version == other.version
            && self.source == other.source
            && self.arrival == other.arrival
            && self.schedule == other.schedule
            && self.fee_ordered == other.fee_ordered
    }
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
            .then_with(|| self.version.cmp(&other.version))
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
    trusted: bool,
    // Smaller turns win; reversed because BTreeSet::last is selected.
    turn: std::cmp::Reverse<u128>,
    work: WorkKey,
    owner: WorkOwner,
}

impl Ord for RunnableHead {
    fn cmp(&self, other: &Self) -> Ordering {
        self.trusted
            .cmp(&other.trusted)
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
    len: usize,
    next_turn: u128,
}

impl FairQueue {
    pub(super) fn new(lane: WorkLane) -> Self {
        Self {
            lane,
            owners: HashMap::new(),
            heads: BTreeSet::new(),
            len: 0,
            next_turn: 0,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.len
    }

    fn head_for(owner: WorkOwner, queue: &OwnerQueue) -> Option<RunnableHead> {
        if !queue.runnable {
            return None;
        }
        Some(RunnableHead {
            trusted: owner == WorkOwner::Trusted,
            turn: std::cmp::Reverse(queue.turn),
            work: queue.work.last()?.clone(),
            owner,
        })
    }

    fn remove_head(&mut self, owner: WorkOwner) {
        if let Some(queue) = self.owners.get(&owner)
            && let Some(head) = Self::head_for(owner, queue)
        {
            self.heads.remove(&head);
        }
    }

    fn insert_head(&mut self, owner: WorkOwner) {
        if let Some(queue) = self.owners.get(&owner)
            && let Some(head) = Self::head_for(owner, queue)
        {
            self.heads.insert(head);
        }
    }

    pub(super) fn insert(&mut self, key: WorkKey) -> Result<(), PrePoolError> {
        let next_len = self
            .len
            .checked_add(1)
            .ok_or(PrePoolError::VersionExhausted)?;
        let owner = WorkOwner::from(key.source);
        self.remove_head(owner);
        let queue = self.owners.entry(owner).or_insert_with(|| OwnerQueue {
            runnable: true,
            ..OwnerQueue::default()
        });
        if !queue.work.insert(key) {
            self.insert_head(owner);
            return Err(PrePoolError::Repair("duplicate exact work key"));
        }
        self.len = next_len;
        self.insert_head(owner);
        Ok(())
    }

    pub(super) fn remove(&mut self, key: &WorkKey) -> Result<(), PrePoolError> {
        let next_len = self
            .len
            .checked_sub(1)
            .ok_or(PrePoolError::Repair("work count underflow"))?;
        let owner = WorkOwner::from(key.source);
        self.remove_head(owner);
        let empty = {
            let queue = self
                .owners
                .get_mut(&owner)
                .ok_or(PrePoolError::Repair("work owner missing"))?;
            if !queue.work.remove(key) {
                self.insert_head(owner);
                return Err(PrePoolError::Repair("work key missing"));
            }
            queue.work.is_empty()
        };
        self.len = next_len;
        if empty {
            self.owners.remove(&owner);
        } else {
            self.insert_head(owner);
        }
        Ok(())
    }

    pub(super) fn set_runnable(&mut self, owner: WorkOwner, runnable: bool) {
        self.remove_head(owner);
        if let Some(queue) = self.owners.get_mut(&owner) {
            queue.runnable = runnable;
        }
        self.insert_head(owner);
    }

    pub(super) fn peek(&self, capability: WorkCapability) -> Option<&WorkKey> {
        self.heads.iter().rev().find_map(|head| {
            let eligible = match (self.lane, capability) {
                (WorkLane::Verify, WorkCapability::SmallCycleOnly) => {
                    !head.work.schedule.is_large_cycle
                }
                _ => true,
            };
            eligible.then_some(&head.work)
        })
    }

    pub(super) fn pop(
        &mut self,
        capability: WorkCapability,
    ) -> Result<Option<WorkKey>, PrePoolError> {
        let Some(head) = self
            .heads
            .iter()
            .rev()
            .find(|head| match (self.lane, capability) {
                (WorkLane::Verify, WorkCapability::SmallCycleOnly) => {
                    !head.work.schedule.is_large_cycle
                }
                _ => true,
            })
            .cloned()
        else {
            return Ok(None);
        };
        let next_turn = self
            .next_turn
            .checked_add(1)
            .ok_or(PrePoolError::VersionExhausted)?;
        let next_len = self
            .len
            .checked_sub(1)
            .ok_or(PrePoolError::Repair("work count underflow"))?;
        self.remove_head(head.owner);
        let empty = {
            let queue = self
                .owners
                .get_mut(&head.owner)
                .ok_or(PrePoolError::Repair("runnable owner missing"))?;
            if !queue.work.remove(&head.work) {
                return Err(PrePoolError::Repair("runnable head missing"));
            }
            queue.turn = next_turn;
            queue.work.is_empty()
        };
        self.next_turn = next_turn;
        self.len = next_len;
        if empty {
            self.owners.remove(&head.owner);
        } else {
            self.insert_head(head.owner);
        }
        Ok(Some(head.work))
    }

    #[cfg(test)]
    pub(super) fn audit(&self) -> Result<(), String> {
        let mut expected_heads = BTreeSet::new();
        let mut count = 0usize;
        for (owner, queue) in &self.owners {
            if queue.work.is_empty() {
                return Err("fair queue retains an empty owner".to_string());
            }
            if queue
                .work
                .iter()
                .any(|key| WorkOwner::from(key.source) != *owner)
            {
                return Err("fair queue owner contains a foreign work key".to_string());
            }
            count = count
                .checked_add(queue.work.len())
                .ok_or_else(|| "fair queue length overflow".to_string())?;
            if let Some(head) = Self::head_for(*owner, queue) {
                expected_heads.insert(head);
            }
        }
        if count != self.len {
            return Err("fair queue cached length drift".to_string());
        }
        if expected_heads != self.heads {
            return Err("fair queue runnable-head projection drift".to_string());
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn work_keys(&self) -> BTreeSet<WorkKey> {
        self.owners
            .values()
            .flat_map(|queue| queue.work.iter().cloned())
            .collect()
    }
}
