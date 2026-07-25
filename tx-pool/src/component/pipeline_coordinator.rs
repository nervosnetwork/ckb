//! Single authoritative owner for the production pre-pool pipeline.
//!
//! Lifecycle state, payload phase, worker leases, dependency/conflict edges,
//! queue tickets and residency accounting share one incarnation/revision and
//! one short-held transition lock.

use crate::constants::lazy_ticket_compaction_limit;
use ckb_network::PeerIndex;
use ckb_types::packed::{Byte32, OutPoint, ProposalShortId};
use ckb_types::prelude::Entity;
#[cfg(test)]
use std::cell::Cell;
use std::cmp::Ordering;
use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashMap, HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;

mod audit;
mod capacity;
mod commit;
mod indexes;
mod lifecycle;
mod maintenance;
mod scheduling;
mod types;
mod undo;
use self::indexes::*;
pub(crate) use types::*;
#[derive(Debug)]
pub(crate) struct PipelineCoordinator<R, U, V> {
    entries: HashMap<Byte32, CoordinatorEntry<R, U, V>>,
    by_short_id: HashMap<ProposalShortId, Byte32>,
    by_peer: HashMap<PeerIndex, HashSet<Byte32>>,
    by_parent: HashMap<Byte32, HashSet<Byte32>>,
    /// O(1) RPC/metrics projection of `RawLocation::WaitingParents`.
    /// Rebuilt with every undo recovery and independently checked by audit so
    /// an untrusted pool-info query never scans the whole coordinator while
    /// holding its mutation mutex.
    waiting_parent_count: usize,
    dependency_failures: VecDeque<Byte32>,
    dependency_failure_set: HashSet<Byte32>,
    conflicts: StagedConflictIndex,
    queues: [TicketQueue; QueueKind::ALL.len()],
    deadlines: BinaryHeap<Reverse<DeadlineTicket>>,
    live_deadlines: HashMap<Byte32, DeadlineTicket>,
    capacity_victim_index: BTreeSet<CapacityVictimKey>,
    candidate_victim_index: BTreeSet<CandidateRank>,
    /// A fallible multi-entry command owns exactly one undo boundary. Every
    /// entry write while it is active must belong to this snapshotted cohort.
    entry_transaction_active: bool,
    entry_transaction_membership: HashSet<Byte32>,
    #[cfg(test)]
    capacity_victim_probes: Cell<usize>,
    #[cfg(test)]
    candidate_victim_probes: Cell<usize>,
    global_usage: CoordinatorResidency,
    peer_usage: HashMap<PeerIndex, CoordinatorResidency>,
    active_work: usize,
    active_work_by_peer: HashMap<PeerIndex, usize>,
    limits: CoordinatorLimits,
    next_incarnation: u64,
    next_arrival: u64,
    next_maintenance_sequence: u64,
    next_queue_sequence: u64,
    #[cfg(test)]
    fault_after_apply_steps: Option<usize>,
    #[cfg(test)]
    apply_steps_seen: usize,
    /// One-shot recoverable error after a commit handoff has exercised its
    /// complete apply path. The enclosing undo scope must restore it before
    /// the production PoolMap cutover rolls back. Test-only: production has
    /// no alternate handoff state or failure channel.
    #[cfg(test)]
    fail_next_handoff_after_apply: Option<CoordinatorError>,
}

enum CapacitySubject {
    Absent(Byte32),
    Present(Byte32),
}

type EntrySnapshot<R, U, V> = (Byte32, Option<CoordinatorEntry<R, U, V>>);

impl<R, U, V> PipelineCoordinator<R, U, V> {
    pub(crate) fn new(limits: CoordinatorLimits) -> Self {
        let verify_ordering = match limits.verify_ordering {
            CoordinatorVerifyOrdering::ArrivalTime => QueueOrdering::Fifo,
            CoordinatorVerifyOrdering::FeeRate => QueueOrdering::FeeRate,
        };
        Self {
            entries: HashMap::new(),
            by_short_id: HashMap::new(),
            by_peer: HashMap::new(),
            by_parent: HashMap::new(),
            waiting_parent_count: 0,
            dependency_failures: VecDeque::new(),
            dependency_failure_set: HashSet::new(),
            conflicts: StagedConflictIndex::default(),
            queues: QueueKind::ALL.map(|kind| {
                TicketQueue::new(match kind {
                    QueueKind::PreCheck | QueueKind::Resolve => QueueOrdering::Fifo,
                    QueueKind::Verify => verify_ordering,
                    QueueKind::Commit => QueueOrdering::Candidate,
                })
            }),
            deadlines: BinaryHeap::new(),
            live_deadlines: HashMap::new(),
            capacity_victim_index: BTreeSet::new(),
            candidate_victim_index: BTreeSet::new(),
            entry_transaction_active: false,
            entry_transaction_membership: HashSet::new(),
            #[cfg(test)]
            capacity_victim_probes: Cell::new(0),
            #[cfg(test)]
            candidate_victim_probes: Cell::new(0),
            global_usage: CoordinatorResidency::default(),
            peer_usage: HashMap::new(),
            active_work: 0,
            active_work_by_peer: HashMap::new(),
            limits,
            next_incarnation: 1,
            next_arrival: 0,
            next_maintenance_sequence: 0,
            next_queue_sequence: 0,
            #[cfg(test)]
            fault_after_apply_steps: None,
            #[cfg(test)]
            apply_steps_seen: 0,
            #[cfg(test)]
            fail_next_handoff_after_apply: None,
        }
    }

    pub(crate) fn peer_usage(&self, peer: PeerIndex) -> CoordinatorResidency {
        self.peer_usage.get(&peer).copied().unwrap_or_default()
    }

    pub(crate) fn peer_hashes(&self, peer: PeerIndex, max: usize) -> Vec<Byte32> {
        self.by_peer
            .get(&peer)
            .into_iter()
            .flatten()
            .filter(|hash| {
                self.entries
                    .get(*hash)
                    .is_some_and(|entry| !entry.state.is_committing())
            })
            // Peer revocation has no ordering contract. Taking the bounded
            // slice directly avoids collecting and sorting the peer's entire
            // attacker-controlled residency on every maintenance turn.
            .take(max)
            .cloned()
            .collect()
    }

    pub(crate) fn view(&self, hash: &Byte32) -> Option<CoordinatorView> {
        self.entries.get(hash).map(CoordinatorEntry::view)
    }

    /// Lightweight source lookup for worker and mutation paths. Building a
    /// complete [`CoordinatorView`] clones dependency and missing-parent sets
    /// and is reserved for RPC/diagnostic snapshots.
    pub(crate) fn source_by_hash(&self, hash: &Byte32) -> Option<CoordinatorSource> {
        self.entries.get(hash).map(|entry| entry.source)
    }

    pub(crate) fn is_committing_hash(&self, hash: &Byte32) -> bool {
        self.entries
            .get(hash)
            .is_some_and(CoordinatorEntry::is_committing)
    }

    pub(crate) fn contains_hash(&self, hash: &Byte32) -> bool {
        self.entries.contains_key(hash)
    }

    pub(crate) fn raw_by_hash(&self, hash: &Byte32) -> Option<Arc<R>> {
        self.entries
            .get(hash)
            .map(|entry| Arc::clone(entry.state.raw()))
    }

    pub(crate) fn raw_by_short_id(&self, short_id: &ProposalShortId) -> Option<Arc<R>> {
        self.by_short_id
            .get(short_id)
            .and_then(|hash| self.raw_by_hash(hash))
    }

    pub(crate) fn unverified_by_hash(&self, hash: &Byte32) -> Option<&U> {
        match &self.entries.get(hash)?.state {
            EntryState::Unverified { payload, .. } => Some(payload.as_ref()),
            _ => None,
        }
    }

    pub(crate) fn verified_by_hash(&self, hash: &Byte32) -> Option<&V> {
        match &self.entries.get(hash)?.state {
            EntryState::CandidateVerified { payload, .. } => Some(payload.as_ref()),
            _ => None,
        }
    }

    pub(crate) fn hash_by_short_id(&self, short_id: &ProposalShortId) -> Option<&Byte32> {
        self.by_short_id.get(short_id)
    }

    pub(crate) fn queue_len(&self, kind: QueueKind) -> usize {
        self.queues[kind.index()].live.len()
    }

    pub(crate) fn waiting_parent_len(&self) -> usize {
        self.waiting_parent_count
    }

    fn enter_waiting_parent(&mut self) -> Result<(), CoordinatorError> {
        self.waiting_parent_count = self
            .waiting_parent_count
            .checked_add(1)
            .ok_or(CoordinatorError::ConflictInvariant)?;
        Ok(())
    }

    fn leave_waiting_parent(&mut self) -> Result<(), CoordinatorError> {
        self.waiting_parent_count = self
            .waiting_parent_count
            .checked_sub(1)
            .ok_or(CoordinatorError::ConflictInvariant)?;
        Ok(())
    }

    pub(crate) fn peer_active_work(&self, peer: PeerIndex) -> usize {
        self.active_work_by_peer.get(&peer).copied().unwrap_or(0)
    }
}

#[cfg(test)]
#[path = "tests/pipeline_coordinator_seam.rs"]
mod test_seam;
