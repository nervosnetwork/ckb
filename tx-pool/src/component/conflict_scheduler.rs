//! ID-only conflict ordering for the target tx-pool coordinator.
//!
//! This scheduler never accepts a transaction into the pool. It orders only
//! candidates that have already passed an authoritative replacement-fee gate;
//! the commit sequencer remains the sole source of final acceptance/rejection.
#![allow(dead_code)]

use ckb_types::packed::{Byte32, OutPoint};
use ckb_types::prelude::Entity;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConflictLimits {
    pub(crate) max_candidates: usize,
    pub(crate) max_edges: usize,
    pub(crate) max_edges_per_candidate: usize,
}

impl ConflictLimits {
    pub(crate) const fn new(
        max_candidates: usize,
        max_edges: usize,
        max_edges_per_candidate: usize,
    ) -> Self {
        Self {
            max_candidates,
            max_edges,
            max_edges_per_candidate,
        }
    }
}

/// The authoritative fee requirements calculated against the current pool
/// conflict closure. `required_replacement_fee` is candidate-specific; the
/// rate floor closes the held-candidate under-fee construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReplacementFeeGate {
    pub(crate) required_replacement_fee: u64,
    pub(crate) min_fee_rate_per_kb: u64,
}

impl ReplacementFeeGate {
    pub(crate) const fn new(required_replacement_fee: u64, min_fee_rate_per_kb: u64) -> Self {
        Self {
            required_replacement_fee,
            min_fee_rate_per_kb,
        }
    }

    pub(crate) fn validate(
        self,
        hash: Byte32,
        inputs: HashSet<OutPoint>,
        fee: u64,
        tx_size: usize,
    ) -> Result<EligibleCandidate, ConflictError> {
        if inputs.is_empty() {
            return Err(ConflictError::NoConflictInputs(hash));
        }
        if tx_size == 0 {
            return Err(ConflictError::ZeroSize(hash));
        }
        if fee < self.required_replacement_fee {
            return Err(ConflictError::UnderReplacementFee {
                hash,
                required: self.required_replacement_fee,
                actual: fee,
            });
        }
        let actual = u128::from(fee) * 1_000;
        let required = u128::from(self.min_fee_rate_per_kb)
            * u128::try_from(tx_size).map_err(|_| ConflictError::FeeRateOverflow)?;
        if actual < required {
            return Err(ConflictError::UnderFeeRate {
                hash,
                required_per_kb: self.min_fee_rate_per_kb,
            });
        }
        Ok(EligibleCandidate {
            hash,
            inputs,
            fee,
            tx_size,
        })
    }
}

/// Constructible only through `ReplacementFeeGate::validate`.
#[derive(Debug, Clone)]
pub(crate) struct EligibleCandidate {
    hash: Byte32,
    inputs: HashSet<OutPoint>,
    fee: u64,
    tx_size: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ConflictTicket {
    pub(crate) hash: Byte32,
    pub(crate) generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ConflictCommitTicket {
    pub(crate) hash: Byte32,
    pub(crate) generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConflictState {
    Active,
    Waiting { blockers: HashSet<Byte32> },
    Committing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConflictView {
    pub(crate) inputs: HashSet<OutPoint>,
    pub(crate) fee: u64,
    pub(crate) tx_size: usize,
    pub(crate) state: ConflictState,
    pub(crate) generation: u64,
}

#[derive(Debug, Default)]
pub(crate) struct ConflictChanges {
    pub(crate) activated: Vec<ConflictTicket>,
    pub(crate) preempted: Vec<Byte32>,
}

#[derive(Debug)]
pub(crate) struct ConflictCommitOutcome {
    pub(crate) winner: Byte32,
    pub(crate) rejected: Vec<Byte32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConflictError {
    NoConflictInputs(Byte32),
    ZeroSize(Byte32),
    UnderReplacementFee {
        hash: Byte32,
        required: u64,
        actual: u64,
    },
    UnderFeeRate {
        hash: Byte32,
        required_per_kb: u64,
    },
    FeeRateOverflow,
    Duplicate(Byte32),
    CandidateLimitExceeded,
    EdgeLimitExceeded,
    PerCandidateEdgeLimitExceeded,
    GenerationExhausted,
    ArrivalSequenceExhausted,
    Missing(Byte32),
    StaleTicket {
        hash: Byte32,
        expected: u64,
        actual: u64,
    },
    StateMismatch {
        hash: Byte32,
        expected: &'static str,
        actual: ConflictState,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConflictAuditError {
    EdgeCount { expected: usize, actual: usize },
    CandidateIndex,
    ActiveIndex,
    WaiterIndex,
    InvalidState(Byte32),
}

#[derive(Debug)]
struct CandidateRecord {
    inputs: HashSet<OutPoint>,
    fee: u64,
    tx_size: usize,
    arrival: u64,
    state: RecordState,
    generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RecordState {
    Active,
    Waiting { blockers: HashSet<Byte32> },
    Committing,
}

impl CandidateRecord {
    fn ticket(&self, hash: &Byte32) -> ConflictTicket {
        ConflictTicket {
            hash: hash.clone(),
            generation: self.generation,
        }
    }

    fn view(&self) -> ConflictView {
        let state = match &self.state {
            RecordState::Active => ConflictState::Active,
            RecordState::Waiting { blockers } => ConflictState::Waiting {
                blockers: blockers.clone(),
            },
            RecordState::Committing => ConflictState::Committing,
        };
        ConflictView {
            inputs: self.inputs.clone(),
            fee: self.fee,
            tx_size: self.tx_size,
            state,
            generation: self.generation,
        }
    }
}

#[derive(Debug)]
pub(crate) struct ConflictScheduler {
    records: HashMap<Byte32, CandidateRecord>,
    /// All candidate edges. Values contain IDs only.
    by_input: HashMap<OutPoint, HashSet<Byte32>>,
    /// At most one active/committing owner per input.
    active_by_input: HashMap<OutPoint, Byte32>,
    /// Reverse blocker index for incremental rebalancing.
    waiters_by_blocker: HashMap<Byte32, HashSet<Byte32>>,
    edge_count: usize,
    next_generation: u64,
    next_arrival: u64,
    limits: ConflictLimits,
}

impl ConflictScheduler {
    pub(crate) fn new(limits: ConflictLimits) -> Self {
        Self {
            records: HashMap::new(),
            by_input: HashMap::new(),
            active_by_input: HashMap::new(),
            waiters_by_blocker: HashMap::new(),
            edge_count: 0,
            next_generation: 1,
            next_arrival: 0,
            limits,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }

    pub(crate) fn edge_count(&self) -> usize {
        self.edge_count
    }

    pub(crate) fn view(&self, hash: &Byte32) -> Option<ConflictView> {
        self.records.get(hash).map(CandidateRecord::view)
    }

    pub(crate) fn active_owner(&self, input: &OutPoint) -> Option<&Byte32> {
        self.active_by_input.get(input)
    }

    /// Register an already fee-gated candidate and schedule it atomically
    /// across every conflicting input.
    pub(crate) fn register(
        &mut self,
        candidate: EligibleCandidate,
    ) -> Result<ConflictChanges, ConflictError> {
        if self.records.contains_key(&candidate.hash) {
            return Err(ConflictError::Duplicate(candidate.hash));
        }
        if candidate.inputs.len() > self.limits.max_edges_per_candidate {
            return Err(ConflictError::PerCandidateEdgeLimitExceeded);
        }
        let next_len = self
            .records
            .len()
            .checked_add(1)
            .ok_or(ConflictError::CandidateLimitExceeded)?;
        if next_len > self.limits.max_candidates {
            return Err(ConflictError::CandidateLimitExceeded);
        }
        let next_edges = self
            .edge_count
            .checked_add(candidate.inputs.len())
            .ok_or(ConflictError::EdgeLimitExceeded)?;
        if next_edges > self.limits.max_edges {
            return Err(ConflictError::EdgeLimitExceeded);
        }
        let generation = self.allocate_generation()?;
        let arrival = self.next_arrival;
        self.next_arrival = self
            .next_arrival
            .checked_add(1)
            .ok_or(ConflictError::ArrivalSequenceExhausted)?;

        let hash = candidate.hash;
        for input in &candidate.inputs {
            self.by_input
                .entry(input.clone())
                .or_default()
                .insert(hash.clone());
        }
        self.edge_count = next_edges;
        self.records.insert(
            hash.clone(),
            CandidateRecord {
                inputs: candidate.inputs,
                fee: candidate.fee,
                tx_size: candidate.tx_size,
                arrival,
                state: RecordState::Waiting {
                    blockers: HashSet::new(),
                },
                generation,
            },
        );
        self.rebalance(HashSet::from([hash]))
    }

    /// Freeze an active candidate for the authoritative pool commit. A
    /// committing owner cannot be preempted by a later arrival.
    pub(crate) fn begin_commit(
        &mut self,
        ticket: &ConflictTicket,
    ) -> Result<ConflictCommitTicket, ConflictError> {
        self.validate_ticket(ticket, &RecordState::Active, "active")?;
        let generation = self.allocate_generation()?;
        let record = self
            .records
            .get_mut(&ticket.hash)
            .expect("validated candidate");
        record.state = RecordState::Committing;
        record.generation = generation;
        Ok(ConflictCommitTicket {
            hash: ticket.hash.clone(),
            generation,
        })
    }

    /// Failed verification/commit removes the speculative candidate and
    /// deterministically rebalances its waiters. No final rejection is emitted
    /// for the restored candidates.
    pub(crate) fn abort_active(
        &mut self,
        ticket: &ConflictTicket,
    ) -> Result<ConflictChanges, ConflictError> {
        self.validate_ticket(ticket, &RecordState::Active, "active")?;
        let affected = self
            .waiters_by_blocker
            .remove(&ticket.hash)
            .unwrap_or_default();
        self.remove_present(&ticket.hash);
        self.rebalance(affected)
    }

    pub(crate) fn abort_commit(
        &mut self,
        ticket: &ConflictCommitTicket,
    ) -> Result<ConflictChanges, ConflictError> {
        self.validate_commit_ticket(ticket)?;
        let affected = self
            .waiters_by_blocker
            .remove(&ticket.hash)
            .unwrap_or_default();
        self.remove_present(&ticket.hash);
        self.rebalance(affected)
    }

    /// Complete an authoritative pool commit. Only now do direct conflicting
    /// candidates become truly rejected; the scheduler itself never made the
    /// acceptance decision.
    pub(crate) fn commit_succeeded(
        &mut self,
        ticket: &ConflictCommitTicket,
    ) -> Result<ConflictCommitOutcome, ConflictError> {
        self.validate_commit_ticket(ticket)?;
        let inputs = self
            .records
            .get(&ticket.hash)
            .expect("validated candidate")
            .inputs
            .clone();
        let mut rejected = HashSet::new();
        for input in &inputs {
            if let Some(candidates) = self.by_input.get(input) {
                rejected.extend(
                    candidates
                        .iter()
                        .filter(|hash| *hash != &ticket.hash)
                        .cloned(),
                );
            }
        }
        // Remove wait links first so every terminal removal is index-clean.
        for hash in &rejected {
            self.remove_waiter_links(hash);
        }
        for hash in &rejected {
            if self.records.contains_key(hash) {
                self.remove_present(hash);
            }
        }
        self.waiters_by_blocker.remove(&ticket.hash);
        self.remove_present(&ticket.hash);
        let mut rejected: Vec<_> = rejected.into_iter().collect();
        rejected.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        Ok(ConflictCommitOutcome {
            winner: ticket.hash.clone(),
            rejected,
        })
    }

    /// Administrative removal. Active/committing removal rebalances waiters;
    /// waiting removal only drops its ID-only indexes.
    pub(crate) fn remove(&mut self, hash: &Byte32) -> Result<ConflictChanges, ConflictError> {
        let state = self
            .records
            .get(hash)
            .ok_or_else(|| ConflictError::Missing(hash.clone()))?
            .state
            .clone();
        let affected = match state {
            RecordState::Active | RecordState::Committing => {
                self.waiters_by_blocker.remove(hash).unwrap_or_default()
            }
            RecordState::Waiting { .. } => HashSet::new(),
        };
        self.remove_waiter_links(hash);
        self.remove_present(hash);
        self.rebalance(affected)
    }

    pub(crate) fn clear(&mut self) {
        self.records.clear();
        self.by_input.clear();
        self.active_by_input.clear();
        self.waiters_by_blocker.clear();
        self.edge_count = 0;
    }

    pub(crate) fn audit(&self) -> Result<(), ConflictAuditError> {
        let mut expected_edges = 0usize;
        let mut expected_by_input: HashMap<OutPoint, HashSet<Byte32>> = HashMap::new();
        let mut expected_active: HashMap<OutPoint, Byte32> = HashMap::new();
        let mut expected_waiters: HashMap<Byte32, HashSet<Byte32>> = HashMap::new();

        for (hash, record) in &self.records {
            expected_edges = expected_edges.saturating_add(record.inputs.len());
            for input in &record.inputs {
                expected_by_input
                    .entry(input.clone())
                    .or_default()
                    .insert(hash.clone());
            }
            match &record.state {
                RecordState::Active | RecordState::Committing => {
                    for input in &record.inputs {
                        if expected_active
                            .insert(input.clone(), hash.clone())
                            .is_some()
                        {
                            return Err(ConflictAuditError::InvalidState(hash.clone()));
                        }
                    }
                }
                RecordState::Waiting { blockers } => {
                    if blockers.is_empty() {
                        return Err(ConflictAuditError::InvalidState(hash.clone()));
                    }
                    for blocker in blockers {
                        let Some(blocker_record) = self.records.get(blocker) else {
                            return Err(ConflictAuditError::InvalidState(hash.clone()));
                        };
                        if !matches!(
                            blocker_record.state,
                            RecordState::Active | RecordState::Committing
                        ) || record.inputs.is_disjoint(&blocker_record.inputs)
                        {
                            return Err(ConflictAuditError::InvalidState(hash.clone()));
                        }
                        expected_waiters
                            .entry(blocker.clone())
                            .or_default()
                            .insert(hash.clone());
                    }
                }
            }
        }

        if expected_edges != self.edge_count {
            return Err(ConflictAuditError::EdgeCount {
                expected: expected_edges,
                actual: self.edge_count,
            });
        }
        if expected_by_input != self.by_input {
            return Err(ConflictAuditError::CandidateIndex);
        }
        if expected_active != self.active_by_input {
            return Err(ConflictAuditError::ActiveIndex);
        }
        if expected_waiters != self.waiters_by_blocker {
            return Err(ConflictAuditError::WaiterIndex);
        }
        Ok(())
    }

    fn rebalance(
        &mut self,
        mut pending: HashSet<Byte32>,
    ) -> Result<ConflictChanges, ConflictError> {
        let mut changes = ConflictChanges::default();
        while !pending.is_empty() {
            let mut round: Vec<_> = pending.drain().collect();
            round.sort_by(|left, right| self.compare_hashes(right, left));
            for hash in round {
                if !matches!(
                    self.records.get(&hash).map(|record| &record.state),
                    Some(RecordState::Waiting { .. })
                ) {
                    continue;
                }
                self.remove_waiter_links(&hash);
                let blockers = self.active_blockers(&hash);
                if blockers.is_empty() {
                    changes.activated.push(self.activate(&hash)?);
                    continue;
                }
                let can_preempt = blockers.iter().all(|blocker| {
                    matches!(
                        self.records.get(blocker).map(|record| &record.state),
                        Some(RecordState::Active)
                    ) && self.compare_hashes(&hash, blocker) == Ordering::Greater
                });
                if !can_preempt {
                    self.set_waiting(&hash, blockers);
                    continue;
                }

                for blocker in blockers {
                    let inherited = self.waiters_by_blocker.remove(&blocker).unwrap_or_default();
                    pending.extend(inherited);
                    self.release_claims(&blocker);
                    let generation = self.allocate_generation()?;
                    let record = self.records.get_mut(&blocker).expect("active blocker");
                    record.state = RecordState::Waiting {
                        blockers: HashSet::new(),
                    };
                    record.generation = generation;
                    changes.preempted.push(blocker.clone());
                    pending.insert(blocker);
                }
                changes.activated.push(self.activate(&hash)?);
            }
        }
        Ok(changes)
    }

    fn activate(&mut self, hash: &Byte32) -> Result<ConflictTicket, ConflictError> {
        let inputs = self
            .records
            .get(hash)
            .expect("candidate exists")
            .inputs
            .clone();
        debug_assert!(
            inputs
                .iter()
                .all(|input| !self.active_by_input.contains_key(input))
        );
        let generation = self.allocate_generation()?;
        let record = self.records.get_mut(hash).expect("candidate exists");
        record.state = RecordState::Active;
        record.generation = generation;
        for input in inputs {
            self.active_by_input.insert(input, hash.clone());
        }
        Ok(record.ticket(hash))
    }

    fn set_waiting(&mut self, hash: &Byte32, blockers: HashSet<Byte32>) {
        debug_assert!(!blockers.is_empty());
        for blocker in &blockers {
            self.waiters_by_blocker
                .entry(blocker.clone())
                .or_default()
                .insert(hash.clone());
        }
        self.records.get_mut(hash).expect("candidate exists").state =
            RecordState::Waiting { blockers };
    }

    fn active_blockers(&self, hash: &Byte32) -> HashSet<Byte32> {
        self.records
            .get(hash)
            .expect("candidate exists")
            .inputs
            .iter()
            .filter_map(|input| self.active_by_input.get(input).cloned())
            .filter(|blocker| blocker != hash)
            .collect()
    }

    fn compare_hashes(&self, left: &Byte32, right: &Byte32) -> Ordering {
        let left_record = self.records.get(left).expect("left candidate exists");
        let right_record = self.records.get(right).expect("right candidate exists");
        let left_rate = u128::from(left_record.fee)
            * u128::try_from(right_record.tx_size).expect("usize fits u128");
        let right_rate = u128::from(right_record.fee)
            * u128::try_from(left_record.tx_size).expect("usize fits u128");
        left_rate
            .cmp(&right_rate)
            .then_with(|| left_record.fee.cmp(&right_record.fee))
            // Earlier arrival wins exact fee ties.
            .then_with(|| right_record.arrival.cmp(&left_record.arrival))
            // Full hash supplies a deterministic final order.
            .then_with(|| right.as_slice().cmp(left.as_slice()))
    }

    fn remove_waiter_links(&mut self, hash: &Byte32) {
        let blockers = match self.records.get(hash).map(|record| &record.state) {
            Some(RecordState::Waiting { blockers }) => blockers.clone(),
            _ => return,
        };
        for blocker in blockers {
            if let Some(waiters) = self.waiters_by_blocker.get_mut(&blocker) {
                waiters.remove(hash);
                if waiters.is_empty() {
                    self.waiters_by_blocker.remove(&blocker);
                }
            }
        }
    }

    fn release_claims(&mut self, hash: &Byte32) {
        let inputs = self
            .records
            .get(hash)
            .expect("candidate exists")
            .inputs
            .clone();
        for input in inputs {
            if self.active_by_input.get(&input) == Some(hash) {
                self.active_by_input.remove(&input);
            }
        }
    }

    fn remove_present(&mut self, hash: &Byte32) {
        self.remove_waiter_links(hash);
        self.release_claims(hash);
        let record = self.records.remove(hash).expect("candidate present");
        self.edge_count = self
            .edge_count
            .checked_sub(record.inputs.len())
            .expect("authoritative conflict edge accounting");
        for input in record.inputs {
            if let Some(candidates) = self.by_input.get_mut(&input) {
                candidates.remove(hash);
                if candidates.is_empty() {
                    self.by_input.remove(&input);
                }
            }
        }
    }

    fn validate_ticket(
        &self,
        ticket: &ConflictTicket,
        expected_state: &RecordState,
        expected_name: &'static str,
    ) -> Result<(), ConflictError> {
        let record = self
            .records
            .get(&ticket.hash)
            .ok_or_else(|| ConflictError::Missing(ticket.hash.clone()))?;
        if record.generation != ticket.generation {
            return Err(ConflictError::StaleTicket {
                hash: ticket.hash.clone(),
                expected: ticket.generation,
                actual: record.generation,
            });
        }
        if &record.state != expected_state {
            return Err(ConflictError::StateMismatch {
                hash: ticket.hash.clone(),
                expected: expected_name,
                actual: record.view().state,
            });
        }
        Ok(())
    }

    fn validate_commit_ticket(&self, ticket: &ConflictCommitTicket) -> Result<(), ConflictError> {
        let record = self
            .records
            .get(&ticket.hash)
            .ok_or_else(|| ConflictError::Missing(ticket.hash.clone()))?;
        if record.generation != ticket.generation {
            return Err(ConflictError::StaleTicket {
                hash: ticket.hash.clone(),
                expected: ticket.generation,
                actual: record.generation,
            });
        }
        if record.state != RecordState::Committing {
            return Err(ConflictError::StateMismatch {
                hash: ticket.hash.clone(),
                expected: "committing",
                actual: record.view().state,
            });
        }
        Ok(())
    }

    fn allocate_generation(&mut self) -> Result<u64, ConflictError> {
        let generation = self.next_generation;
        self.next_generation = self
            .next_generation
            .checked_add(1)
            .ok_or(ConflictError::GenerationExhausted)?;
        Ok(generation)
    }
}
