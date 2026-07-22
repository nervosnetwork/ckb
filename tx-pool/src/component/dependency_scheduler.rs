//! ID-only dependency scheduling for the target tx-pool coordinator.
//!
//! Payloads stay in `LifecycleStore`; this component owns only dependency
//! edges and versioned readiness tickets.  It is introduced as an executable
//! model before production integration so queue-full and reorg wake-up
//! semantics can be proved without changing the current hot path.
#![allow(dead_code)]

use crate::component::lifecycle_store::PipelineStage;
use ckb_types::packed::Byte32;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DependencyLimits {
    pub(crate) max_entries: usize,
    pub(crate) max_edges: usize,
    pub(crate) max_edges_per_entry: usize,
}

impl DependencyLimits {
    pub(crate) const fn new(
        max_entries: usize,
        max_edges: usize,
        max_edges_per_entry: usize,
    ) -> Self {
        Self {
            max_entries,
            max_edges,
            max_edges_per_entry,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DependencyTicket {
    pub(crate) hash: Byte32,
    pub(crate) generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DependencyState {
    Waiting { missing: HashSet<Byte32> },
    Ready,
    Dispatched,
    CapacityBlocked(PipelineStage),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DependencyView {
    pub(crate) dependencies: HashSet<Byte32>,
    pub(crate) state: DependencyState,
    pub(crate) generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DependencyFailure {
    pub(crate) hash: Byte32,
    pub(crate) failed_parent: Byte32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DependencyError {
    MissingNotDependency {
        child: Byte32,
        parent: Byte32,
    },
    SelfDependency(Byte32),
    EntryLimitExceeded,
    EdgeLimitExceeded,
    PerEntryEdgeLimitExceeded,
    GenerationExhausted,
    Missing(Byte32),
    StaleTicket {
        hash: Byte32,
        expected: u64,
        actual: u64,
    },
    StateMismatch {
        hash: Byte32,
        expected: &'static str,
        actual: DependencyState,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DependencyAuditError {
    EdgeCount { expected: usize, actual: usize },
    ParentIndex,
    ReadyIndex,
    BlockedIndex,
    InvalidState(Byte32),
}

#[derive(Debug)]
struct DependencyRecord {
    dependencies: HashSet<Byte32>,
    missing: HashSet<Byte32>,
    state: RecordState,
    generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordState {
    Waiting,
    Ready,
    Dispatched,
    CapacityBlocked(PipelineStage),
}

impl DependencyRecord {
    fn ticket(&self, hash: &Byte32) -> DependencyTicket {
        DependencyTicket {
            hash: hash.clone(),
            generation: self.generation,
        }
    }

    fn view(&self) -> DependencyView {
        let state = match self.state {
            RecordState::Waiting => DependencyState::Waiting {
                missing: self.missing.clone(),
            },
            RecordState::Ready => DependencyState::Ready,
            RecordState::Dispatched => DependencyState::Dispatched,
            RecordState::CapacityBlocked(stage) => DependencyState::CapacityBlocked(stage),
        };
        DependencyView {
            dependencies: self.dependencies.clone(),
            state,
            generation: self.generation,
        }
    }
}

#[derive(Debug)]
pub(crate) struct DependencyScheduler {
    records: HashMap<Byte32, DependencyRecord>,
    /// All dependency edges, not only currently missing parents. Keeping live
    /// edges lets a reorg invalidate a ready or dispatched child immediately.
    by_parent: HashMap<Byte32, HashSet<Byte32>>,
    ready: VecDeque<DependencyTicket>,
    ready_set: HashSet<DependencyTicket>,
    capacity_waiters: HashMap<PipelineStage, VecDeque<DependencyTicket>>,
    blocked_set: HashSet<DependencyTicket>,
    edge_count: usize,
    next_generation: u64,
    limits: DependencyLimits,
}

impl DependencyScheduler {
    pub(crate) fn new(limits: DependencyLimits) -> Self {
        Self {
            records: HashMap::new(),
            by_parent: HashMap::new(),
            ready: VecDeque::new(),
            ready_set: HashSet::new(),
            capacity_waiters: HashMap::new(),
            blocked_set: HashSet::new(),
            edge_count: 0,
            next_generation: 1,
            limits,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }

    pub(crate) fn edge_count(&self) -> usize {
        self.edge_count
    }

    pub(crate) fn view(&self, hash: &Byte32) -> Option<DependencyView> {
        self.records.get(hash).map(DependencyRecord::view)
    }

    /// Insert or atomically reclassify one child. `missing` must be a subset
    /// of `dependencies`; the caller computes it from the authoritative
    /// snapshot/lifecycle view.
    pub(crate) fn park(
        &mut self,
        child: Byte32,
        dependencies: HashSet<Byte32>,
        missing: HashSet<Byte32>,
    ) -> Result<DependencyTicket, DependencyError> {
        if dependencies.contains(&child) {
            return Err(DependencyError::SelfDependency(child));
        }
        if let Some(parent) = missing.difference(&dependencies).next() {
            return Err(DependencyError::MissingNotDependency {
                child,
                parent: parent.clone(),
            });
        }
        if dependencies.len() > self.limits.max_edges_per_entry {
            return Err(DependencyError::PerEntryEdgeLimitExceeded);
        }

        let old_edges = self
            .records
            .get(&child)
            .map_or(0, |record| record.dependencies.len());
        let next_entries = self
            .records
            .len()
            .checked_add(usize::from(!self.records.contains_key(&child)))
            .ok_or(DependencyError::EntryLimitExceeded)?;
        if next_entries > self.limits.max_entries {
            return Err(DependencyError::EntryLimitExceeded);
        }
        let next_edges = self
            .edge_count
            .checked_sub(old_edges)
            .and_then(|count| count.checked_add(dependencies.len()))
            .ok_or(DependencyError::EdgeLimitExceeded)?;
        if next_edges > self.limits.max_edges {
            return Err(DependencyError::EdgeLimitExceeded);
        }
        let generation = self.allocate_generation()?;

        if self.records.contains_key(&child) {
            self.remove_present(&child);
        }
        for parent in &dependencies {
            self.by_parent
                .entry(parent.clone())
                .or_default()
                .insert(child.clone());
        }
        let state = if missing.is_empty() {
            RecordState::Ready
        } else {
            RecordState::Waiting
        };
        let record = DependencyRecord {
            dependencies,
            missing,
            state,
            generation,
        };
        let ticket = record.ticket(&child);
        self.edge_count = next_edges;
        self.records.insert(child, record);
        if state == RecordState::Ready {
            self.enqueue_ready(ticket.clone());
        }
        Ok(ticket)
    }

    /// Mark one parent available. Every child whose final missing dependency
    /// is satisfied becomes ready exactly once.
    pub(crate) fn parent_available(
        &mut self,
        parent: &Byte32,
    ) -> Result<Vec<DependencyTicket>, DependencyError> {
        let children = self.by_parent.get(parent).cloned().unwrap_or_default();
        let affected: Vec<_> = children
            .into_iter()
            .filter(|child| {
                self.records.get(child).is_some_and(|record| {
                    record.missing.contains(parent) && record.state == RecordState::Waiting
                })
            })
            .collect();
        let ready_count = affected
            .iter()
            .filter(|child| {
                self.records
                    .get(*child)
                    .is_some_and(|record| record.missing.len() == 1)
            })
            .count();
        let mut generations = self.allocate_generations(ready_count)?;
        let mut ready = Vec::new();
        for child in affected {
            let record = self.records.get_mut(&child).expect("waiting child exists");
            record.missing.remove(parent);
            if record.missing.is_empty() {
                record.state = RecordState::Ready;
                record.generation = generations
                    .next()
                    .expect("generation reserved per ready child");
                let ticket = record.ticket(&child);
                self.enqueue_ready(ticket.clone());
                ready.push(ticket);
            }
        }
        self.compact_physical_queues();
        Ok(ready)
    }

    /// Reorg/removal invalidation. A child that was ready, dispatched, or
    /// capacity-blocked gets a fresh generation and returns to parent wait;
    /// all outstanding tickets immediately become stale.
    pub(crate) fn parent_unavailable(
        &mut self,
        parent: &Byte32,
    ) -> Result<Vec<Byte32>, DependencyError> {
        let children = self.by_parent.get(parent).cloned().unwrap_or_default();
        let affected: Vec<_> = children
            .into_iter()
            .filter(|child| {
                self.records
                    .get(child)
                    .is_some_and(|record| !record.missing.contains(parent))
            })
            .collect();
        let generations = self.allocate_generations(affected.len())?;

        for (child, generation) in affected.iter().zip(generations) {
            let record = self.records.get_mut(child).expect("indexed child exists");
            let old_ticket = record.ticket(child);
            self.ready_set.remove(&old_ticket);
            self.blocked_set.remove(&old_ticket);
            record.missing.insert(parent.clone());
            record.state = RecordState::Waiting;
            record.generation = generation;
        }
        self.compact_physical_queues();
        Ok(affected)
    }

    /// Definitive parent failure cascades through all tracked descendants,
    /// including children that had already become ready or were waiting for
    /// downstream queue capacity.
    pub(crate) fn parent_failed(&mut self, parent: &Byte32) -> Vec<DependencyFailure> {
        let mut work = VecDeque::from([parent.clone()]);
        let mut visited = HashSet::new();
        let mut failed = Vec::new();

        while let Some(failed_parent) = work.pop_front() {
            if !visited.insert(failed_parent.clone()) {
                continue;
            }
            let children = self
                .by_parent
                .get(&failed_parent)
                .cloned()
                .unwrap_or_default();
            for child in children {
                if !self.records.contains_key(&child) {
                    continue;
                }
                self.remove_present(&child);
                failed.push(DependencyFailure {
                    hash: child.clone(),
                    failed_parent: failed_parent.clone(),
                });
                work.push_back(child);
            }
        }
        failed
    }

    /// Dispatch the next ready ID. The record remains authoritative and moves
    /// to `Dispatched`, so a lost worker result is visible and recoverable.
    pub(crate) fn pop_ready(&mut self) -> Result<Option<DependencyTicket>, DependencyError> {
        while let Some(ticket) = self.ready.pop_front() {
            if !self.ready_set.remove(&ticket) {
                continue;
            }
            let Some(record) = self.records.get(&ticket.hash) else {
                continue;
            };
            if record.generation != ticket.generation || record.state != RecordState::Ready {
                continue;
            }
            let generation = self.allocate_generation()?;
            let record = self
                .records
                .get_mut(&ticket.hash)
                .expect("validated ready record");
            record.state = RecordState::Dispatched;
            record.generation = generation;
            let dispatched = record.ticket(&ticket.hash);
            self.compact_physical_queues();
            return Ok(Some(dispatched));
        }
        self.compact_physical_queues();
        Ok(None)
    }

    /// Return a failed/panicked dispatch to the ready queue.
    pub(crate) fn return_ready(
        &mut self,
        ticket: &DependencyTicket,
    ) -> Result<DependencyTicket, DependencyError> {
        self.validate_ticket_state(ticket, RecordState::Dispatched, "dispatched")?;
        let generation = self.allocate_generation()?;
        let record = self
            .records
            .get_mut(&ticket.hash)
            .expect("validated record");
        record.state = RecordState::Ready;
        record.generation = generation;
        let ready = record.ticket(&ticket.hash);
        self.enqueue_ready(ready.clone());
        self.compact_physical_queues();
        Ok(ready)
    }

    /// Preserve a dispatched child when its next bounded queue is full. It is
    /// woken by `capacity_available`, never solely by polling or expiry.
    pub(crate) fn block_on_capacity(
        &mut self,
        ticket: &DependencyTicket,
        stage: PipelineStage,
    ) -> Result<DependencyTicket, DependencyError> {
        self.validate_ticket_state(ticket, RecordState::Dispatched, "dispatched")?;
        let generation = self.allocate_generation()?;
        let record = self
            .records
            .get_mut(&ticket.hash)
            .expect("validated record");
        record.state = RecordState::CapacityBlocked(stage);
        record.generation = generation;
        let blocked = record.ticket(&ticket.hash);
        self.blocked_set.insert(blocked.clone());
        self.capacity_waiters
            .entry(stage)
            .or_default()
            .push_back(blocked.clone());
        self.compact_physical_queues();
        Ok(blocked)
    }

    /// Move up to `limit` live waiters for one downstream stage back to the
    /// ready queue. Stale capacity queue slots are discarded lazily.
    pub(crate) fn capacity_available(
        &mut self,
        stage: PipelineStage,
        limit: usize,
    ) -> Result<Vec<DependencyTicket>, DependencyError> {
        let candidates: Vec<_> = self
            .capacity_waiters
            .get(&stage)
            .into_iter()
            .flat_map(|queue| queue.iter())
            .filter(|ticket| {
                self.blocked_set.contains(*ticket)
                    && self.records.get(&ticket.hash).is_some_and(|record| {
                        record.generation == ticket.generation
                            && record.state == RecordState::CapacityBlocked(stage)
                    })
            })
            .take(limit)
            .cloned()
            .collect();
        let generations = self.allocate_generations(candidates.len())?;
        let mut woken = Vec::with_capacity(candidates.len());
        for (ticket, generation) in candidates.into_iter().zip(generations) {
            self.blocked_set.remove(&ticket);
            let record = self
                .records
                .get_mut(&ticket.hash)
                .expect("validated blocked record");
            record.state = RecordState::Ready;
            record.generation = generation;
            let ready = record.ticket(&ticket.hash);
            self.enqueue_ready(ready.clone());
            woken.push(ready);
        }
        self.compact_physical_queues();
        Ok(woken)
    }

    /// Finish a successfully dispatched dependency record. The lifecycle
    /// coordinator has already moved its payload to the next authoritative
    /// location before calling this method.
    pub(crate) fn complete(&mut self, ticket: &DependencyTicket) -> Result<(), DependencyError> {
        self.validate_ticket_state(ticket, RecordState::Dispatched, "dispatched")?;
        self.remove_present(&ticket.hash);
        Ok(())
    }

    pub(crate) fn remove(&mut self, child: &Byte32) -> bool {
        if !self.records.contains_key(child) {
            return false;
        }
        self.remove_present(child);
        true
    }

    pub(crate) fn clear(&mut self) {
        self.records.clear();
        self.by_parent.clear();
        self.ready.clear();
        self.ready_set.clear();
        self.capacity_waiters.clear();
        self.blocked_set.clear();
        self.edge_count = 0;
    }

    pub(crate) fn audit(&self) -> Result<(), DependencyAuditError> {
        let mut expected_edges = 0usize;
        let mut expected_by_parent: HashMap<Byte32, HashSet<Byte32>> = HashMap::new();
        let mut expected_ready = HashSet::new();
        let mut expected_blocked = HashSet::new();

        for (child, record) in &self.records {
            expected_edges = expected_edges.saturating_add(record.dependencies.len());
            if !record.missing.is_subset(&record.dependencies) {
                return Err(DependencyAuditError::InvalidState(child.clone()));
            }
            for parent in &record.dependencies {
                expected_by_parent
                    .entry(parent.clone())
                    .or_default()
                    .insert(child.clone());
            }
            let ticket = record.ticket(child);
            match record.state {
                RecordState::Waiting if !record.missing.is_empty() => {}
                RecordState::Ready if record.missing.is_empty() => {
                    expected_ready.insert(ticket);
                }
                RecordState::Dispatched if record.missing.is_empty() => {}
                RecordState::CapacityBlocked(_) if record.missing.is_empty() => {
                    expected_blocked.insert(ticket);
                }
                RecordState::Waiting
                | RecordState::Ready
                | RecordState::Dispatched
                | RecordState::CapacityBlocked(_) => {
                    return Err(DependencyAuditError::InvalidState(child.clone()));
                }
            }
        }

        if expected_edges != self.edge_count {
            return Err(DependencyAuditError::EdgeCount {
                expected: expected_edges,
                actual: self.edge_count,
            });
        }
        if expected_by_parent != self.by_parent {
            return Err(DependencyAuditError::ParentIndex);
        }
        if expected_ready != self.ready_set {
            return Err(DependencyAuditError::ReadyIndex);
        }
        if expected_blocked != self.blocked_set {
            return Err(DependencyAuditError::BlockedIndex);
        }
        Ok(())
    }

    fn enqueue_ready(&mut self, ticket: DependencyTicket) {
        if self.ready_set.insert(ticket.clone()) {
            self.ready.push_back(ticket);
        }
    }

    /// Lazy queue slots are intentionally decoupled from the authoritative
    /// sets, but attacker-driven requeue/remove churn must not make their
    /// physical allocation grow without bound. Compact once stale slack is
    /// materially larger than live state; the additive floor keeps the hot
    /// path amortized O(1) for small queues.
    fn compact_physical_queues(&mut self) {
        const STALE_SLACK: usize = 64;
        if self.ready.len()
            > self
                .ready_set
                .len()
                .saturating_mul(2)
                .saturating_add(STALE_SLACK)
        {
            self.ready.retain(|ticket| self.ready_set.contains(ticket));
        }
        let stages: Vec<_> = self.capacity_waiters.keys().copied().collect();
        for stage in stages {
            let live = self
                .blocked_set
                .iter()
                .filter(|ticket| {
                    self.records.get(&ticket.hash).is_some_and(|record| {
                        record.generation == ticket.generation
                            && record.state == RecordState::CapacityBlocked(stage)
                    })
                })
                .count();
            let should_compact = self.capacity_waiters.get(&stage).is_some_and(|queue| {
                queue.len() > live.saturating_mul(2).saturating_add(STALE_SLACK)
            });
            if should_compact {
                if let Some(queue) = self.capacity_waiters.get_mut(&stage) {
                    queue.retain(|ticket| {
                        self.blocked_set.contains(ticket)
                            && self.records.get(&ticket.hash).is_some_and(|record| {
                                record.generation == ticket.generation
                                    && record.state == RecordState::CapacityBlocked(stage)
                            })
                    });
                }
            }
            if self
                .capacity_waiters
                .get(&stage)
                .is_some_and(VecDeque::is_empty)
            {
                self.capacity_waiters.remove(&stage);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn physical_queue_slots_for_test(&self) -> usize {
        self.ready.len()
            + self
                .capacity_waiters
                .values()
                .map(VecDeque::len)
                .sum::<usize>()
    }

    fn validate_ticket_state(
        &self,
        ticket: &DependencyTicket,
        expected_state: RecordState,
        expected_name: &'static str,
    ) -> Result<(), DependencyError> {
        let record = self
            .records
            .get(&ticket.hash)
            .ok_or_else(|| DependencyError::Missing(ticket.hash.clone()))?;
        if record.generation != ticket.generation {
            return Err(DependencyError::StaleTicket {
                hash: ticket.hash.clone(),
                expected: ticket.generation,
                actual: record.generation,
            });
        }
        if record.state != expected_state {
            return Err(DependencyError::StateMismatch {
                hash: ticket.hash.clone(),
                expected: expected_name,
                actual: record.view().state,
            });
        }
        Ok(())
    }

    fn allocate_generation(&mut self) -> Result<u64, DependencyError> {
        let generation = self.next_generation;
        self.next_generation = self
            .next_generation
            .checked_add(1)
            .ok_or(DependencyError::GenerationExhausted)?;
        Ok(generation)
    }

    fn allocate_generations(
        &mut self,
        count: usize,
    ) -> Result<std::ops::Range<u64>, DependencyError> {
        let count = u64::try_from(count).map_err(|_| DependencyError::GenerationExhausted)?;
        let start = self.next_generation;
        let end = start
            .checked_add(count)
            .ok_or(DependencyError::GenerationExhausted)?;
        self.next_generation = end;
        Ok(start..end)
    }

    fn remove_present(&mut self, child: &Byte32) {
        let record = self
            .records
            .remove(child)
            .expect("dependency record present");
        let ticket = record.ticket(child);
        self.ready_set.remove(&ticket);
        self.blocked_set.remove(&ticket);
        self.edge_count = self
            .edge_count
            .checked_sub(record.dependencies.len())
            .expect("authoritative dependency edge accounting");
        for parent in record.dependencies {
            if let Some(children) = self.by_parent.get_mut(&parent) {
                children.remove(child);
                if children.is_empty() {
                    self.by_parent.remove(&parent);
                }
            }
        }
        self.compact_physical_queues();
    }
}
