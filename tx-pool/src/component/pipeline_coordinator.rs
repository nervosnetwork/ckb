//! Single-authority model for the target tx-pool pipeline.
//!
//! This module is intentionally isolated from the production hot path while
//! the legacy queue/wait/conflict owners are replaced. Unlike the earlier
//! split prototypes, lifecycle state, payload phase, worker leases,
//! dependency edges, queue tickets and residency accounting live in one
//! authoritative store and use one incarnation/revision.
#![allow(dead_code)]

use ckb_network::PeerIndex;
use ckb_types::packed::{Byte32, OutPoint, ProposalShortId};
use ckb_types::prelude::Entity;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RawStage {
    PreCheck,
    Resolve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum QueueKind {
    PreCheck,
    Resolve,
    Verify,
    Commit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CoordinatorLocation {
    RawQueued(RawStage),
    RawActive(RawStage),
    WaitingParents { missing: HashSet<Byte32> },
    VerifyQueued,
    VerifyActive,
    ReadyToCommit,
    WaitingPoolInputs { inputs: HashSet<OutPoint> },
    WaitingConflict { blockers: HashSet<Byte32> },
    ConflictRecheck,
    Committing,
    Invalidated { cause: Byte32 },
}

impl CoordinatorLocation {
    fn queue_kind(&self) -> Option<QueueKind> {
        match self {
            Self::RawQueued(RawStage::PreCheck) => Some(QueueKind::PreCheck),
            Self::RawQueued(RawStage::Resolve) => Some(QueueKind::Resolve),
            Self::VerifyQueued => Some(QueueKind::Verify),
            Self::ReadyToCommit => Some(QueueKind::Commit),
            Self::RawActive(_)
            | Self::WaitingParents { .. }
            | Self::VerifyActive
            | Self::WaitingPoolInputs { .. }
            | Self::WaitingConflict { .. }
            | Self::ConflictRecheck
            | Self::Committing
            | Self::Invalidated { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PayloadPhase {
    Raw,
    Unverified,
    Verified,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CoordinatorResidency {
    pub(crate) entries: usize,
    pub(crate) bytes: usize,
}

impl CoordinatorResidency {
    pub(crate) const fn new(entries: usize, bytes: usize) -> Self {
        Self { entries, bytes }
    }

    fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_add(other.entries)?,
            bytes: self.bytes.checked_add(other.bytes)?,
        })
    }

    fn checked_sub(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_sub(other.entries)?,
            bytes: self.bytes.checked_sub(other.bytes)?,
        })
    }

    fn fits(self, limit: Self) -> bool {
        self.entries <= limit.entries && self.bytes <= limit.bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CoordinatorLimits {
    pub(crate) global: CoordinatorResidency,
    pub(crate) per_peer: Option<CoordinatorResidency>,
    pub(crate) max_dependencies_per_entry: usize,
    pub(crate) max_dependents_per_parent: usize,
    pub(crate) max_conflict_inputs_per_entry: usize,
    pub(crate) max_candidates_per_input: usize,
    pub(crate) max_conflict_edges: usize,
}

impl CoordinatorLimits {
    pub(crate) const fn new(
        global: CoordinatorResidency,
        per_peer: Option<CoordinatorResidency>,
        max_dependencies_per_entry: usize,
        max_dependents_per_parent: usize,
    ) -> Self {
        Self {
            global,
            per_peer,
            max_dependencies_per_entry,
            max_dependents_per_parent,
            max_conflict_inputs_per_entry: max_dependencies_per_entry,
            max_candidates_per_input: max_dependents_per_parent,
            max_conflict_edges: global.entries.saturating_mul(max_dependencies_per_entry),
        }
    }

    pub(crate) const fn with_conflict_limits(
        mut self,
        max_conflict_inputs_per_entry: usize,
        max_candidates_per_input: usize,
        max_conflict_edges: usize,
    ) -> Self {
        self.max_conflict_inputs_per_entry = max_conflict_inputs_per_entry;
        self.max_candidates_per_input = max_candidates_per_input;
        self.max_conflict_edges = max_conflict_edges;
        self
    }
}

/// Conflict metadata becomes constructible pipeline state only when supplied
/// together with a successful verify lease completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedCandidate {
    inputs: HashSet<OutPoint>,
    fee: u64,
    tx_size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CoordinatorFeeGate {
    required_replacement_fee: u64,
    min_fee_rate_per_kb: u64,
}

impl CoordinatorFeeGate {
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
    ) -> Result<VerifiedCandidate, CoordinatorError> {
        if inputs.is_empty() {
            return Err(CoordinatorError::NoConflictInputs(hash));
        }
        if tx_size == 0 {
            return Err(CoordinatorError::ZeroTransactionSize(hash));
        }
        if fee < self.required_replacement_fee {
            return Err(CoordinatorError::UnderReplacementFee {
                hash,
                required: self.required_replacement_fee,
                actual: fee,
            });
        }
        let actual = u128::from(fee) * 1_000;
        let required = u128::from(self.min_fee_rate_per_kb)
            .checked_mul(u128::try_from(tx_size).map_err(|_| CoordinatorError::FeeRateOverflow)?)
            .ok_or(CoordinatorError::FeeRateOverflow)?;
        if actual < required {
            return Err(CoordinatorError::UnderFeeRate {
                hash,
                required_per_kb: self.min_fee_rate_per_kb,
            });
        }
        Ok(VerifiedCandidate {
            inputs,
            fee,
            tx_size,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CoordinatorVersion {
    pub(crate) incarnation: u64,
    pub(crate) revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CoordinatorTicket {
    pub(crate) hash: Byte32,
    pub(crate) version: CoordinatorVersion,
}

#[derive(Debug, Clone)]
pub(crate) struct RawWorkLease<R> {
    pub(crate) hash: Byte32,
    pub(crate) stage: RawStage,
    pub(crate) version: CoordinatorVersion,
    pub(crate) payload: Arc<R>,
}

#[derive(Debug, Clone)]
pub(crate) struct VerifyWorkLease<U> {
    pub(crate) hash: Byte32,
    pub(crate) version: CoordinatorVersion,
    pub(crate) payload: Arc<U>,
}

#[derive(Debug, Clone)]
pub(crate) struct CommitLease<V> {
    pub(crate) hash: Byte32,
    pub(crate) version: CoordinatorVersion,
    pub(crate) payload: Arc<V>,
}

#[derive(Debug)]
pub(crate) struct CommitHandoff<R, V> {
    pub(crate) hash: Byte32,
    pub(crate) short_id: ProposalShortId,
    pub(crate) raw: Arc<R>,
    pub(crate) verified: Arc<V>,
    pub(crate) peer: Option<PeerIndex>,
}

#[derive(Debug)]
pub(crate) struct ConflictCommitHandoff<R, U, V> {
    pub(crate) winner: CommitHandoff<R, V>,
    pub(crate) rejected: Vec<TerminalRecord<R, U, V>>,
}

/// Administrative/negative terminal outcomes deliberately exclude commit.
/// A committed payload can leave only through `commit_handoff` with a valid
/// `CommitLease` created from `ReadyToCommit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalDisposition {
    Rejected,
    Removed,
    Cleared,
    Internal,
}

#[derive(Debug)]
pub(crate) struct TerminalRecord<R, U, V> {
    pub(crate) hash: Byte32,
    pub(crate) short_id: ProposalShortId,
    pub(crate) raw: Arc<R>,
    pub(crate) later_phase: Option<TerminalPhase<U, V>>,
    pub(crate) peer: Option<PeerIndex>,
    pub(crate) disposition: TerminalDisposition,
}

#[derive(Debug)]
pub(crate) enum TerminalPhase<U, V> {
    Unverified(Arc<U>),
    Verified(Arc<V>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CoordinatorView {
    pub(crate) short_id: ProposalShortId,
    pub(crate) phase: PayloadPhase,
    pub(crate) location: CoordinatorLocation,
    pub(crate) peer: Option<PeerIndex>,
    pub(crate) charge_bytes: usize,
    pub(crate) dependencies: HashSet<Byte32>,
    pub(crate) version: CoordinatorVersion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CoordinatorError {
    DuplicateHash(Byte32),
    ShortIdCollision {
        short_id: ProposalShortId,
        existing_hash: Byte32,
    },
    SelfDependency(Byte32),
    DependencyLimitExceeded,
    ParentFanoutLimitExceeded(Byte32),
    NoConflictInputs(Byte32),
    ZeroTransactionSize(Byte32),
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
    ConflictInputLimitExceeded,
    ConflictCandidateLimitExceeded(OutPoint),
    ConflictEdgeLimitExceeded,
    ArrivalSequenceExhausted,
    MissingParentNotDependency {
        child: Byte32,
        parent: Byte32,
    },
    GlobalBudgetExceeded,
    PeerBudgetExceeded(PeerIndex),
    IncarnationExhausted,
    RevisionExhausted(Byte32),
    Missing(Byte32),
    IncarnationMismatch {
        expected: u64,
        actual: u64,
    },
    RevisionMismatch {
        expected: u64,
        actual: u64,
    },
    LocationMismatch {
        expected: CoordinatorLocation,
        actual: CoordinatorLocation,
    },
    PhaseMismatch {
        expected: PayloadPhase,
        actual: PayloadPhase,
    },
    QueueInvariant(QueueKind),
    QueueReservationFailed,
    ConflictInvariant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CoordinatorAuditError {
    GlobalUsage,
    PeerUsage,
    ShortIdIndex,
    PeerIndex,
    ParentIndex,
    QueueLogicalIndex,
    QueuePhysicalIndex,
    ConflictEdgeCount,
    ConflictCandidateIndex,
    ConflictActiveIndex,
    ConflictWaiterIndex,
    ConflictMaintenanceIndex,
    InvalidPhaseLocation(Byte32),
    BudgetExceeded,
}

#[derive(Debug)]
enum ResidentPhase<U, V> {
    Raw,
    Unverified(Arc<U>),
    Verified(Arc<V>),
}

impl<U, V> ResidentPhase<U, V> {
    fn kind(&self) -> PayloadPhase {
        match self {
            Self::Raw => PayloadPhase::Raw,
            Self::Unverified(_) => PayloadPhase::Unverified,
            Self::Verified(_) => PayloadPhase::Verified,
        }
    }
}

#[derive(Debug)]
struct CoordinatorEntry<R, U, V> {
    short_id: ProposalShortId,
    raw: Arc<R>,
    phase: ResidentPhase<U, V>,
    location: CoordinatorLocation,
    peer: Option<PeerIndex>,
    raw_charge_bytes: usize,
    charge_bytes: usize,
    dependencies: HashSet<Byte32>,
    candidate: Option<CandidateMeta>,
    incarnation: u64,
    revision: u64,
}

#[derive(Debug, Clone)]
struct CandidateMeta {
    inputs: HashSet<OutPoint>,
    fee: u64,
    tx_size: usize,
    arrival: u64,
}

impl<R, U, V> CoordinatorEntry<R, U, V> {
    fn version(&self) -> CoordinatorVersion {
        CoordinatorVersion {
            incarnation: self.incarnation,
            revision: self.revision,
        }
    }

    fn ticket(&self, hash: &Byte32) -> CoordinatorTicket {
        CoordinatorTicket {
            hash: hash.clone(),
            version: self.version(),
        }
    }

    fn view(&self) -> CoordinatorView {
        CoordinatorView {
            short_id: self.short_id.clone(),
            phase: self.phase.kind(),
            location: self.location.clone(),
            peer: self.peer,
            charge_bytes: self.charge_bytes,
            dependencies: self.dependencies.clone(),
            version: self.version(),
        }
    }
}

#[derive(Debug, Default)]
struct TicketQueue {
    physical: VecDeque<CoordinatorTicket>,
    live: HashSet<CoordinatorTicket>,
}

impl TicketQueue {
    fn reserve_live(&mut self) -> Result<(), CoordinatorError> {
        self.physical
            .try_reserve(1)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.live
            .try_reserve(1)
            .map_err(|_| CoordinatorError::QueueReservationFailed)
    }

    fn push_reserved(
        &mut self,
        kind: QueueKind,
        ticket: CoordinatorTicket,
    ) -> Result<(), CoordinatorError> {
        if !self.live.insert(ticket.clone()) {
            return Err(CoordinatorError::QueueInvariant(kind));
        }
        self.physical.push_back(ticket);
        Ok(())
    }

    fn remove_live(&mut self, ticket: &CoordinatorTicket) {
        self.live.remove(ticket);
    }

    fn compact(&mut self) {
        const STALE_SLACK: usize = 64;
        if self.physical.len()
            > self
                .live
                .len()
                .saturating_mul(2)
                .saturating_add(STALE_SLACK)
        {
            self.physical.retain(|ticket| self.live.contains(ticket));
        }
    }
}

#[derive(Debug)]
pub(crate) struct PipelineCoordinator<R, U, V> {
    entries: HashMap<Byte32, CoordinatorEntry<R, U, V>>,
    by_short_id: HashMap<ProposalShortId, Byte32>,
    by_peer: HashMap<PeerIndex, HashSet<Byte32>>,
    by_parent: HashMap<Byte32, HashSet<Byte32>>,
    candidates_by_input: HashMap<OutPoint, HashSet<Byte32>>,
    active_by_input: HashMap<OutPoint, Byte32>,
    waiters_by_blocker: HashMap<Byte32, HashSet<Byte32>>,
    conflict_rechecks: VecDeque<Byte32>,
    conflict_recheck_set: HashSet<Byte32>,
    conflict_edge_count: usize,
    queues: HashMap<QueueKind, TicketQueue>,
    global_usage: CoordinatorResidency,
    peer_usage: HashMap<PeerIndex, CoordinatorResidency>,
    limits: CoordinatorLimits,
    next_incarnation: u64,
    next_arrival: u64,
}

impl<R, U, V> PipelineCoordinator<R, U, V> {
    pub(crate) fn new(limits: CoordinatorLimits) -> Self {
        Self {
            entries: HashMap::new(),
            by_short_id: HashMap::new(),
            by_peer: HashMap::new(),
            by_parent: HashMap::new(),
            candidates_by_input: HashMap::new(),
            active_by_input: HashMap::new(),
            waiters_by_blocker: HashMap::new(),
            conflict_rechecks: VecDeque::new(),
            conflict_recheck_set: HashSet::new(),
            conflict_edge_count: 0,
            queues: HashMap::from([
                (QueueKind::PreCheck, TicketQueue::default()),
                (QueueKind::Resolve, TicketQueue::default()),
                (QueueKind::Verify, TicketQueue::default()),
                (QueueKind::Commit, TicketQueue::default()),
            ]),
            global_usage: CoordinatorResidency::default(),
            peer_usage: HashMap::new(),
            limits,
            next_incarnation: 1,
            next_arrival: 0,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn usage(&self) -> CoordinatorResidency {
        self.global_usage
    }

    pub(crate) fn peer_usage(&self, peer: PeerIndex) -> CoordinatorResidency {
        self.peer_usage.get(&peer).copied().unwrap_or_default()
    }

    pub(crate) fn view(&self, hash: &Byte32) -> Option<CoordinatorView> {
        self.entries.get(hash).map(CoordinatorEntry::view)
    }

    pub(crate) fn hash_by_short_id(&self, short_id: &ProposalShortId) -> Option<&Byte32> {
        self.by_short_id.get(short_id)
    }

    pub(crate) fn queue_len(&self, kind: QueueKind) -> usize {
        self.queues.get(&kind).map_or(0, |queue| queue.live.len())
    }

    pub(crate) fn conflict_recheck_len(&self) -> usize {
        self.conflict_recheck_set.len()
    }

    pub(crate) fn active_conflict_owner(&self, input: &OutPoint) -> Option<&Byte32> {
        self.active_by_input.get(input)
    }

    pub(crate) fn conflict_edge_count(&self) -> usize {
        self.conflict_edge_count
    }

    pub(crate) fn admit_raw(
        &mut self,
        hash: Byte32,
        short_id: ProposalShortId,
        raw: R,
        initial_stage: RawStage,
        peer: Option<PeerIndex>,
        charge_bytes: usize,
        dependencies: HashSet<Byte32>,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        if self.entries.contains_key(&hash) {
            return Err(CoordinatorError::DuplicateHash(hash));
        }
        if let Some(existing_hash) = self.by_short_id.get(&short_id) {
            return Err(CoordinatorError::ShortIdCollision {
                short_id,
                existing_hash: existing_hash.clone(),
            });
        }
        if dependencies.contains(&hash) {
            return Err(CoordinatorError::SelfDependency(hash));
        }
        if dependencies.len() > self.limits.max_dependencies_per_entry {
            return Err(CoordinatorError::DependencyLimitExceeded);
        }
        for parent in &dependencies {
            if self
                .by_parent
                .get(parent)
                .map_or(0, HashSet::len)
                .saturating_add(1)
                > self.limits.max_dependents_per_parent
            {
                return Err(CoordinatorError::ParentFanoutLimitExceeded(parent.clone()));
            }
        }
        let charge = CoordinatorResidency::new(1, charge_bytes);
        self.check_add_budget(peer, charge)?;
        let incarnation = self.next_incarnation;
        let next_incarnation = incarnation
            .checked_add(1)
            .ok_or(CoordinatorError::IncarnationExhausted)?;
        let location = CoordinatorLocation::RawQueued(initial_stage);
        let queue_kind = match initial_stage {
            RawStage::PreCheck => QueueKind::PreCheck,
            RawStage::Resolve => QueueKind::Resolve,
        };
        self.queue_mut(queue_kind)?.reserve_live()?;

        let entry = CoordinatorEntry {
            short_id: short_id.clone(),
            raw: Arc::new(raw),
            phase: ResidentPhase::Raw,
            location,
            peer,
            raw_charge_bytes: charge_bytes,
            charge_bytes,
            dependencies: dependencies.clone(),
            candidate: None,
            incarnation,
            revision: 0,
        };
        let ticket = entry.ticket(&hash);
        self.next_incarnation = next_incarnation;
        self.global_usage = self
            .global_usage
            .checked_add(charge)
            .ok_or(CoordinatorError::GlobalBudgetExceeded)?;
        if let Some(peer) = peer {
            let usage = self.peer_usage.entry(peer).or_default();
            *usage = usage
                .checked_add(charge)
                .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
            self.by_peer.entry(peer).or_default().insert(hash.clone());
        }
        for parent in dependencies {
            self.by_parent
                .entry(parent)
                .or_default()
                .insert(hash.clone());
        }
        self.by_short_id.insert(short_id, hash.clone());
        self.entries.insert(hash, entry);
        self.queue_mut(queue_kind)?
            .push_reserved(queue_kind, ticket)?;
        Ok(CoordinatorVersion {
            incarnation,
            revision: 0,
        })
    }

    pub(crate) fn checkout_raw(
        &mut self,
        stage: RawStage,
    ) -> Result<Option<RawWorkLease<R>>, CoordinatorError> {
        let kind = match stage {
            RawStage::PreCheck => QueueKind::PreCheck,
            RawStage::Resolve => QueueKind::Resolve,
        };
        let Some(ticket) = self.peek_live_ticket(kind)? else {
            return Ok(None);
        };
        let expected = CoordinatorLocation::RawQueued(stage);
        self.validate_version_location_phase(
            &ticket.hash,
            ticket.version,
            &expected,
            PayloadPhase::Raw,
        )?;
        self.ensure_revision_capacity(&ticket.hash)?;
        self.consume_front_ticket(kind, &ticket)?;
        let entry = self
            .entries
            .get_mut(&ticket.hash)
            .ok_or_else(|| CoordinatorError::Missing(ticket.hash.clone()))?;
        entry.location = CoordinatorLocation::RawActive(stage);
        entry.revision += 1;
        Ok(Some(RawWorkLease {
            hash: ticket.hash,
            stage,
            version: entry.version(),
            payload: Arc::clone(&entry.raw),
        }))
    }

    pub(crate) fn complete_raw(
        &mut self,
        lease: &RawWorkLease<R>,
        unverified: U,
        charge_bytes: usize,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        let expected = CoordinatorLocation::RawActive(lease.stage);
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &expected,
            PayloadPhase::Raw,
        )?;
        self.ensure_revision_capacity(&lease.hash)?;
        self.check_recharge(&lease.hash, charge_bytes)?;
        self.queue_mut(QueueKind::Verify)?.reserve_live()?;

        self.apply_recharge(&lease.hash, charge_bytes)?;
        let entry = self
            .entries
            .get_mut(&lease.hash)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        entry.phase = ResidentPhase::Unverified(Arc::new(unverified));
        entry.location = CoordinatorLocation::VerifyQueued;
        entry.revision += 1;
        let version = entry.version();
        let ticket = entry.ticket(&lease.hash);
        self.queue_mut(QueueKind::Verify)?
            .push_reserved(QueueKind::Verify, ticket)?;
        Ok(version)
    }

    pub(crate) fn wait_for_parents(
        &mut self,
        lease: &RawWorkLease<R>,
        missing: HashSet<Byte32>,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        let expected = CoordinatorLocation::RawActive(lease.stage);
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &expected,
            PayloadPhase::Raw,
        )?;
        if let Some(parent) = missing.iter().find(|parent| {
            !self
                .entries
                .get(&lease.hash)
                .is_some_and(|entry| entry.dependencies.contains(*parent))
        }) {
            return Err(CoordinatorError::MissingParentNotDependency {
                child: lease.hash.clone(),
                parent: parent.clone(),
            });
        }
        if missing.is_empty() {
            return self.requeue_raw(lease);
        }
        self.ensure_revision_capacity(&lease.hash)?;
        let entry = self
            .entries
            .get_mut(&lease.hash)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        entry.location = CoordinatorLocation::WaitingParents { missing };
        entry.revision += 1;
        Ok(entry.version())
    }

    pub(crate) fn requeue_raw(
        &mut self,
        lease: &RawWorkLease<R>,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        let expected = CoordinatorLocation::RawActive(lease.stage);
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &expected,
            PayloadPhase::Raw,
        )?;
        self.ensure_revision_capacity(&lease.hash)?;
        let kind = match lease.stage {
            RawStage::PreCheck => QueueKind::PreCheck,
            RawStage::Resolve => QueueKind::Resolve,
        };
        self.queue_mut(kind)?.reserve_live()?;
        let entry = self
            .entries
            .get_mut(&lease.hash)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        entry.location = CoordinatorLocation::RawQueued(lease.stage);
        entry.revision += 1;
        let version = entry.version();
        let ticket = entry.ticket(&lease.hash);
        self.queue_mut(kind)?.push_reserved(kind, ticket)?;
        Ok(version)
    }

    pub(crate) fn parent_available(
        &mut self,
        parent: &Byte32,
    ) -> Result<Vec<CoordinatorTicket>, CoordinatorError> {
        let children = self.by_parent.get(parent).cloned().unwrap_or_default();
        let mut affected = Vec::new();
        let mut ready_count = 0usize;
        for child in children {
            let Some(entry) = self.entries.get(&child) else {
                continue;
            };
            let CoordinatorLocation::WaitingParents { missing } = &entry.location else {
                continue;
            };
            if !missing.contains(parent) {
                continue;
            }
            self.ensure_revision_capacity(&child)?;
            if missing.len() == 1 {
                ready_count = ready_count.saturating_add(1);
            }
            affected.push(child);
        }
        self.queue_mut(QueueKind::Resolve)?
            .physical
            .try_reserve(ready_count)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.queue_mut(QueueKind::Resolve)?
            .live
            .try_reserve(ready_count)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;

        let mut ready = Vec::with_capacity(ready_count);
        for child in affected {
            let entry = self
                .entries
                .get_mut(&child)
                .ok_or_else(|| CoordinatorError::Missing(child.clone()))?;
            let CoordinatorLocation::WaitingParents { missing } = &mut entry.location else {
                return Err(CoordinatorError::LocationMismatch {
                    expected: CoordinatorLocation::WaitingParents {
                        missing: HashSet::from([parent.clone()]),
                    },
                    actual: entry.location.clone(),
                });
            };
            missing.remove(parent);
            entry.revision += 1;
            if missing.is_empty() {
                entry.location = CoordinatorLocation::RawQueued(RawStage::Resolve);
                let ticket = entry.ticket(&child);
                self.queue_mut(QueueKind::Resolve)?
                    .push_reserved(QueueKind::Resolve, ticket.clone())?;
                ready.push(ticket);
            }
        }
        Ok(ready)
    }

    pub(crate) fn parent_unavailable(
        &mut self,
        parent: &Byte32,
    ) -> Result<Vec<Byte32>, CoordinatorError> {
        let children = self.by_parent.get(parent).cloned().unwrap_or_default();
        let mut affected = Vec::new();
        for child in children {
            let Some(entry) = self.entries.get(&child) else {
                continue;
            };
            if matches!(
                &entry.location,
                CoordinatorLocation::WaitingParents { missing } if missing.contains(parent)
            ) {
                continue;
            }
            self.ensure_revision_capacity(&child)?;
            self.preflight_remove_conflict_indexes(&child)?;
            affected.push(child);
        }

        for child in &affected {
            self.remove_current_queue_ticket(child)?;
            self.remove_conflict_indexes(child)?;
            let raw_charge = self
                .entries
                .get(child)
                .ok_or_else(|| CoordinatorError::Missing(child.clone()))?
                .raw_charge_bytes;
            self.apply_recharge(child, raw_charge)?;
            let entry = self
                .entries
                .get_mut(child)
                .ok_or_else(|| CoordinatorError::Missing(child.clone()))?;
            let mut missing = match &entry.location {
                CoordinatorLocation::WaitingParents { missing } => missing.clone(),
                _ => HashSet::new(),
            };
            missing.insert(parent.clone());
            entry.phase = ResidentPhase::Raw;
            entry.candidate = None;
            entry.location = CoordinatorLocation::WaitingParents { missing };
            entry.revision += 1;
        }
        Ok(affected)
    }

    pub(crate) fn checkout_verify(
        &mut self,
    ) -> Result<Option<VerifyWorkLease<U>>, CoordinatorError> {
        let Some(ticket) = self.peek_live_ticket(QueueKind::Verify)? else {
            return Ok(None);
        };
        self.validate_version_location_phase(
            &ticket.hash,
            ticket.version,
            &CoordinatorLocation::VerifyQueued,
            PayloadPhase::Unverified,
        )?;
        self.ensure_revision_capacity(&ticket.hash)?;
        self.consume_front_ticket(QueueKind::Verify, &ticket)?;
        let entry = self
            .entries
            .get_mut(&ticket.hash)
            .ok_or_else(|| CoordinatorError::Missing(ticket.hash.clone()))?;
        entry.location = CoordinatorLocation::VerifyActive;
        entry.revision += 1;
        let ResidentPhase::Unverified(payload) = &entry.phase else {
            return Err(CoordinatorError::PhaseMismatch {
                expected: PayloadPhase::Unverified,
                actual: entry.phase.kind(),
            });
        };
        Ok(Some(VerifyWorkLease {
            hash: ticket.hash,
            version: entry.version(),
            payload: Arc::clone(payload),
        }))
    }

    pub(crate) fn complete_verification(
        &mut self,
        lease: &VerifyWorkLease<U>,
        verified: V,
        charge_bytes: usize,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::VerifyActive,
            PayloadPhase::Unverified,
        )?;
        self.ensure_revision_capacity(&lease.hash)?;
        self.check_recharge(&lease.hash, charge_bytes)?;
        self.queue_mut(QueueKind::Commit)?.reserve_live()?;
        self.apply_recharge(&lease.hash, charge_bytes)?;
        let entry = self
            .entries
            .get_mut(&lease.hash)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        entry.phase = ResidentPhase::Verified(Arc::new(verified));
        entry.location = CoordinatorLocation::ReadyToCommit;
        entry.revision += 1;
        let version = entry.version();
        let ticket = entry.ticket(&lease.hash);
        self.queue_mut(QueueKind::Commit)?
            .push_reserved(QueueKind::Commit, ticket)?;
        Ok(version)
    }

    pub(crate) fn complete_verification_candidate(
        &mut self,
        lease: &VerifyWorkLease<U>,
        verified: V,
        charge_bytes: usize,
        candidate: VerifiedCandidate,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::VerifyActive,
            PayloadPhase::Unverified,
        )?;
        self.ensure_revision_capacity(&lease.hash)?;
        self.check_recharge(&lease.hash, charge_bytes)?;
        if candidate.inputs.len() > self.limits.max_conflict_inputs_per_entry {
            return Err(CoordinatorError::ConflictInputLimitExceeded);
        }
        let next_edges = self
            .conflict_edge_count
            .checked_add(candidate.inputs.len())
            .ok_or(CoordinatorError::ConflictEdgeLimitExceeded)?;
        if next_edges > self.limits.max_conflict_edges {
            return Err(CoordinatorError::ConflictEdgeLimitExceeded);
        }
        for input in &candidate.inputs {
            if self
                .candidates_by_input
                .get(input)
                .map_or(0, HashSet::len)
                .saturating_add(1)
                > self.limits.max_candidates_per_input
            {
                return Err(CoordinatorError::ConflictCandidateLimitExceeded(
                    input.clone(),
                ));
            }
        }
        let arrival = self.next_arrival;
        let next_arrival = arrival
            .checked_add(1)
            .ok_or(CoordinatorError::ArrivalSequenceExhausted)?;
        let meta = CandidateMeta {
            inputs: candidate.inputs,
            fee: candidate.fee,
            tx_size: candidate.tx_size,
            arrival,
        };
        let blockers = self.active_blockers_for_inputs(&lease.hash, &meta.inputs);
        let can_preempt = !blockers.is_empty()
            && blockers.iter().all(|blocker| {
                self.entries.get(blocker).is_some_and(|entry| {
                    entry.location == CoordinatorLocation::ReadyToCommit
                        && entry.candidate.as_ref().is_some_and(|blocker_meta| {
                            Self::compare_candidates(&lease.hash, &meta, blocker, blocker_meta)
                                == Ordering::Greater
                        })
                })
            });

        let mut invalidated_waiters = HashSet::new();
        if can_preempt {
            for blocker in &blockers {
                self.ensure_revision_capacity(blocker)?;
                if let Some(waiters) = self.waiters_by_blocker.get(blocker) {
                    invalidated_waiters.extend(waiters.iter().cloned());
                }
            }
            for waiter in &invalidated_waiters {
                self.ensure_revision_capacity(waiter)?;
            }
            if blockers.len() > self.limits.max_candidates_per_input {
                return Err(CoordinatorError::ConflictCandidateLimitExceeded(
                    meta.inputs
                        .iter()
                        .next()
                        .cloned()
                        .ok_or(CoordinatorError::ConflictInvariant)?,
                ));
            }
        } else {
            for blocker in &blockers {
                if self
                    .waiters_by_blocker
                    .get(blocker)
                    .map_or(0, HashSet::len)
                    .saturating_add(1)
                    > self.limits.max_candidates_per_input
                {
                    return Err(CoordinatorError::ConflictCandidateLimitExceeded(
                        meta.inputs
                            .iter()
                            .next()
                            .cloned()
                            .ok_or(CoordinatorError::ConflictInvariant)?,
                    ));
                }
            }
        }

        if blockers.is_empty() || can_preempt {
            self.queue_mut(QueueKind::Commit)?.reserve_live()?;
        }
        self.conflict_rechecks
            .try_reserve(invalidated_waiters.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.conflict_recheck_set
            .try_reserve(invalidated_waiters.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;

        self.apply_recharge(&lease.hash, charge_bytes)?;
        self.next_arrival = next_arrival;
        self.conflict_edge_count = next_edges;
        for input in &meta.inputs {
            self.candidates_by_input
                .entry(input.clone())
                .or_default()
                .insert(lease.hash.clone());
        }

        if can_preempt {
            for blocker in &blockers {
                self.invalidate_conflict_waiters(blocker)?;
                self.remove_current_queue_ticket(blocker)?;
                self.release_conflict_claims(blocker)?;
                let entry = self
                    .entries
                    .get_mut(blocker)
                    .ok_or_else(|| CoordinatorError::Missing(blocker.clone()))?;
                entry.location = CoordinatorLocation::WaitingConflict {
                    blockers: HashSet::from([lease.hash.clone()]),
                };
                entry.revision += 1;
                self.waiters_by_blocker
                    .entry(lease.hash.clone())
                    .or_default()
                    .insert(blocker.clone());
            }
        }

        let entry = self
            .entries
            .get_mut(&lease.hash)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        entry.phase = ResidentPhase::Verified(Arc::new(verified));
        entry.candidate = Some(meta);
        entry.revision += 1;
        if blockers.is_empty() || can_preempt {
            entry.location = CoordinatorLocation::ReadyToCommit;
            let version = entry.version();
            let ticket = entry.ticket(&lease.hash);
            self.claim_conflict_inputs(&lease.hash)?;
            self.queue_mut(QueueKind::Commit)?
                .push_reserved(QueueKind::Commit, ticket)?;
            Ok(version)
        } else {
            entry.location = CoordinatorLocation::WaitingConflict {
                blockers: blockers.clone(),
            };
            let version = entry.version();
            for blocker in blockers {
                self.waiters_by_blocker
                    .entry(blocker)
                    .or_default()
                    .insert(lease.hash.clone());
            }
            Ok(version)
        }
    }

    pub(crate) fn begin_next_commit(&mut self) -> Result<Option<CommitLease<V>>, CoordinatorError> {
        let Some(ticket) = self.peek_live_ticket(QueueKind::Commit)? else {
            return Ok(None);
        };
        self.validate_version_location_phase(
            &ticket.hash,
            ticket.version,
            &CoordinatorLocation::ReadyToCommit,
            PayloadPhase::Verified,
        )?;
        self.ensure_revision_capacity(&ticket.hash)?;
        self.consume_front_ticket(QueueKind::Commit, &ticket)?;
        let entry = self
            .entries
            .get_mut(&ticket.hash)
            .ok_or_else(|| CoordinatorError::Missing(ticket.hash.clone()))?;
        entry.location = CoordinatorLocation::Committing;
        entry.revision += 1;
        let ResidentPhase::Verified(payload) = &entry.phase else {
            return Err(CoordinatorError::PhaseMismatch {
                expected: PayloadPhase::Verified,
                actual: entry.phase.kind(),
            });
        };
        Ok(Some(CommitLease {
            hash: ticket.hash,
            version: entry.version(),
            payload: Arc::clone(payload),
        }))
    }

    pub(crate) fn drain_conflict_rechecks(
        &mut self,
        limit: usize,
    ) -> Result<Vec<CoordinatorTicket>, CoordinatorError> {
        let mut activated = Vec::new();
        let mut processed = 0usize;
        while processed < limit {
            let Some(hash) = self.conflict_rechecks.front().cloned() else {
                break;
            };
            if !self.conflict_recheck_set.contains(&hash) {
                self.conflict_rechecks.pop_front();
                continue;
            }
            let ticket = self.recheck_conflict_candidate(&hash)?;
            self.conflict_rechecks.pop_front();
            self.conflict_recheck_set.remove(&hash);
            if let Some(ticket) = ticket {
                activated.push(ticket);
            }
            processed += 1;
        }
        Ok(activated)
    }

    pub(crate) fn abort_commit(
        &mut self,
        lease: &CommitLease<V>,
    ) -> Result<CoordinatorVersion, CoordinatorError> {
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::Committing,
            PayloadPhase::Verified,
        )?;
        self.ensure_revision_capacity(&lease.hash)?;
        self.queue_mut(QueueKind::Commit)?.reserve_live()?;
        let entry = self
            .entries
            .get_mut(&lease.hash)
            .ok_or_else(|| CoordinatorError::Missing(lease.hash.clone()))?;
        entry.location = CoordinatorLocation::ReadyToCommit;
        entry.revision += 1;
        let version = entry.version();
        let ticket = entry.ticket(&lease.hash);
        self.queue_mut(QueueKind::Commit)?
            .push_reserved(QueueKind::Commit, ticket)?;
        Ok(version)
    }

    pub(crate) fn commit_handoff(
        &mut self,
        lease: &CommitLease<V>,
    ) -> Result<CommitHandoff<R, V>, CoordinatorError> {
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::Committing,
            PayloadPhase::Verified,
        )?;
        if self
            .entries
            .get(&lease.hash)
            .is_some_and(|entry| entry.candidate.is_some())
        {
            return Err(CoordinatorError::ConflictInvariant);
        }
        let entry = self.remove_present(&lease.hash)?;
        let ResidentPhase::Verified(verified) = entry.phase else {
            return Err(CoordinatorError::PhaseMismatch {
                expected: PayloadPhase::Verified,
                actual: entry.phase.kind(),
            });
        };
        Ok(CommitHandoff {
            hash: lease.hash.clone(),
            short_id: entry.short_id,
            raw: entry.raw,
            verified,
            peer: entry.peer,
        })
    }

    pub(crate) fn commit_candidate_handoff(
        &mut self,
        lease: &CommitLease<V>,
    ) -> Result<ConflictCommitHandoff<R, U, V>, CoordinatorError> {
        self.validate_version_location_phase(
            &lease.hash,
            lease.version,
            &CoordinatorLocation::Committing,
            PayloadPhase::Verified,
        )?;
        let winner_inputs = self
            .entries
            .get(&lease.hash)
            .and_then(|entry| entry.candidate.as_ref())
            .map(|candidate| candidate.inputs.clone())
            .ok_or(CoordinatorError::ConflictInvariant)?;
        let mut rejected = HashSet::new();
        for input in &winner_inputs {
            if let Some(candidates) = self.candidates_by_input.get(input) {
                rejected.extend(
                    candidates
                        .iter()
                        .filter(|hash| *hash != &lease.hash)
                        .cloned(),
                );
            }
        }
        for hash in &rejected {
            let entry = self
                .entries
                .get(hash)
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            let candidate = entry
                .candidate
                .as_ref()
                .ok_or(CoordinatorError::ConflictInvariant)?;
            if candidate.inputs.is_disjoint(&winner_inputs)
                || matches!(
                    entry.location,
                    CoordinatorLocation::ReadyToCommit | CoordinatorLocation::Committing
                )
            {
                return Err(CoordinatorError::ConflictInvariant);
            }
            self.preflight_remove_conflict_indexes(hash)?;
        }

        let mut rejected: Vec<_> = rejected.into_iter().collect();
        rejected.sort_by(|left, right| left.as_slice().cmp(right.as_slice()));
        let mut terminal = Vec::with_capacity(rejected.len());
        for hash in rejected {
            let entry = self.remove_present(&hash)?;
            terminal.push(Self::terminal_record(
                hash,
                entry,
                TerminalDisposition::Rejected,
            ));
        }
        let entry = self.remove_present(&lease.hash)?;
        let ResidentPhase::Verified(verified) = entry.phase else {
            return Err(CoordinatorError::PhaseMismatch {
                expected: PayloadPhase::Verified,
                actual: entry.phase.kind(),
            });
        };
        Ok(ConflictCommitHandoff {
            winner: CommitHandoff {
                hash: lease.hash.clone(),
                short_id: entry.short_id,
                raw: entry.raw,
                verified,
                peer: entry.peer,
            },
            rejected: terminal,
        })
    }

    pub(crate) fn force_terminalize(
        &mut self,
        hash: &Byte32,
        disposition: TerminalDisposition,
    ) -> Result<Option<TerminalRecord<R, U, V>>, CoordinatorError> {
        if !self.entries.contains_key(hash) {
            return Ok(None);
        }
        let entry = self.remove_present(hash)?;
        Ok(Some(Self::terminal_record(
            hash.clone(),
            entry,
            disposition,
        )))
    }

    pub(crate) fn clear(&mut self) -> Result<Vec<TerminalRecord<R, U, V>>, CoordinatorError> {
        // Clear is one ownership transaction, not N conflict removals. It must
        // not wake/revise records that are themselves being cleared, and stale
        // worker leases become harmless because re-admission receives a new
        // incarnation.
        let entries = std::mem::take(&mut self.entries);
        let mut terminal = Vec::with_capacity(entries.len());
        for (hash, entry) in entries {
            terminal.push(Self::terminal_record(
                hash,
                entry,
                TerminalDisposition::Cleared,
            ));
        }
        self.by_short_id.clear();
        self.by_peer.clear();
        self.by_parent.clear();
        self.candidates_by_input.clear();
        self.active_by_input.clear();
        self.waiters_by_blocker.clear();
        self.conflict_rechecks.clear();
        self.conflict_recheck_set.clear();
        self.conflict_edge_count = 0;
        for queue in self.queues.values_mut() {
            queue.physical.clear();
            queue.live.clear();
        }
        self.global_usage = CoordinatorResidency::default();
        self.peer_usage.clear();
        Ok(terminal)
    }

    pub(crate) fn audit(&self) -> Result<(), CoordinatorAuditError> {
        let mut global_usage = CoordinatorResidency::default();
        let mut peer_usage: HashMap<PeerIndex, CoordinatorResidency> = HashMap::new();
        let mut by_short_id = HashMap::new();
        let mut by_peer: HashMap<PeerIndex, HashSet<Byte32>> = HashMap::new();
        let mut by_parent: HashMap<Byte32, HashSet<Byte32>> = HashMap::new();
        let mut expected_live: HashMap<QueueKind, HashSet<CoordinatorTicket>> = HashMap::new();
        let mut conflict_edges = 0usize;
        let mut candidates_by_input: HashMap<OutPoint, HashSet<Byte32>> = HashMap::new();
        let mut active_by_input: HashMap<OutPoint, Byte32> = HashMap::new();
        let mut waiters_by_blocker: HashMap<Byte32, HashSet<Byte32>> = HashMap::new();
        let mut conflict_rechecks = HashSet::new();

        for (hash, entry) in &self.entries {
            if !Self::phase_location_valid(entry.phase.kind(), &entry.location) {
                return Err(CoordinatorAuditError::InvalidPhaseLocation(hash.clone()));
            }
            let charge = CoordinatorResidency::new(1, entry.charge_bytes);
            global_usage = global_usage
                .checked_add(charge)
                .ok_or(CoordinatorAuditError::GlobalUsage)?;
            if by_short_id
                .insert(entry.short_id.clone(), hash.clone())
                .is_some()
            {
                return Err(CoordinatorAuditError::ShortIdIndex);
            }
            if let Some(peer) = entry.peer {
                let usage = peer_usage.entry(peer).or_default();
                *usage = usage
                    .checked_add(charge)
                    .ok_or(CoordinatorAuditError::PeerUsage)?;
                by_peer.entry(peer).or_default().insert(hash.clone());
            }
            for parent in &entry.dependencies {
                by_parent
                    .entry(parent.clone())
                    .or_default()
                    .insert(hash.clone());
            }
            if let Some(kind) = entry.location.queue_kind() {
                expected_live
                    .entry(kind)
                    .or_default()
                    .insert(entry.ticket(hash));
            }
            if let Some(candidate) = &entry.candidate {
                if entry.phase.kind() != PayloadPhase::Verified {
                    return Err(CoordinatorAuditError::InvalidPhaseLocation(hash.clone()));
                }
                conflict_edges = conflict_edges
                    .checked_add(candidate.inputs.len())
                    .ok_or(CoordinatorAuditError::ConflictEdgeCount)?;
                for input in &candidate.inputs {
                    candidates_by_input
                        .entry(input.clone())
                        .or_default()
                        .insert(hash.clone());
                }
                match &entry.location {
                    CoordinatorLocation::ReadyToCommit | CoordinatorLocation::Committing => {
                        for input in &candidate.inputs {
                            if active_by_input
                                .insert(input.clone(), hash.clone())
                                .is_some()
                            {
                                return Err(CoordinatorAuditError::ConflictActiveIndex);
                            }
                        }
                    }
                    CoordinatorLocation::WaitingConflict { blockers } => {
                        if blockers.is_empty() {
                            return Err(CoordinatorAuditError::ConflictWaiterIndex);
                        }
                        for blocker in blockers {
                            let Some(blocker_entry) = self.entries.get(blocker) else {
                                return Err(CoordinatorAuditError::ConflictWaiterIndex);
                            };
                            if !matches!(
                                blocker_entry.location,
                                CoordinatorLocation::ReadyToCommit
                                    | CoordinatorLocation::Committing
                            ) || blocker_entry.candidate.as_ref().is_none_or(
                                |blocker_candidate| {
                                    candidate.inputs.is_disjoint(&blocker_candidate.inputs)
                                },
                            ) {
                                return Err(CoordinatorAuditError::ConflictWaiterIndex);
                            }
                            waiters_by_blocker
                                .entry(blocker.clone())
                                .or_default()
                                .insert(hash.clone());
                        }
                    }
                    CoordinatorLocation::ConflictRecheck => {
                        conflict_rechecks.insert(hash.clone());
                    }
                    _ => return Err(CoordinatorAuditError::InvalidPhaseLocation(hash.clone())),
                }
            } else if matches!(
                entry.location,
                CoordinatorLocation::WaitingConflict { .. } | CoordinatorLocation::ConflictRecheck
            ) {
                return Err(CoordinatorAuditError::ConflictCandidateIndex);
            }
        }

        if global_usage != self.global_usage {
            return Err(CoordinatorAuditError::GlobalUsage);
        }
        if !global_usage.fits(self.limits.global)
            || peer_usage
                .values()
                .any(|usage| self.limits.per_peer.is_some_and(|limit| !usage.fits(limit)))
        {
            return Err(CoordinatorAuditError::BudgetExceeded);
        }
        if peer_usage != self.peer_usage {
            return Err(CoordinatorAuditError::PeerUsage);
        }
        if by_short_id != self.by_short_id {
            return Err(CoordinatorAuditError::ShortIdIndex);
        }
        if by_peer != self.by_peer {
            return Err(CoordinatorAuditError::PeerIndex);
        }
        if by_parent != self.by_parent {
            return Err(CoordinatorAuditError::ParentIndex);
        }
        if conflict_edges != self.conflict_edge_count {
            return Err(CoordinatorAuditError::ConflictEdgeCount);
        }
        if candidates_by_input != self.candidates_by_input {
            return Err(CoordinatorAuditError::ConflictCandidateIndex);
        }
        if active_by_input != self.active_by_input {
            return Err(CoordinatorAuditError::ConflictActiveIndex);
        }
        if waiters_by_blocker != self.waiters_by_blocker {
            return Err(CoordinatorAuditError::ConflictWaiterIndex);
        }
        if conflict_rechecks != self.conflict_recheck_set {
            return Err(CoordinatorAuditError::ConflictMaintenanceIndex);
        }
        let mut physical_rechecks = HashMap::new();
        for hash in &self.conflict_rechecks {
            if self.conflict_recheck_set.contains(hash) {
                *physical_rechecks.entry(hash).or_insert(0usize) += 1;
            }
        }
        if self
            .conflict_recheck_set
            .iter()
            .any(|hash| physical_rechecks.get(hash) != Some(&1))
        {
            return Err(CoordinatorAuditError::ConflictMaintenanceIndex);
        }

        for kind in [
            QueueKind::PreCheck,
            QueueKind::Resolve,
            QueueKind::Verify,
            QueueKind::Commit,
        ] {
            let empty = HashSet::new();
            let expected = expected_live.get(&kind).unwrap_or(&empty);
            let Some(queue) = self.queues.get(&kind) else {
                return Err(CoordinatorAuditError::QueueLogicalIndex);
            };
            if &queue.live != expected {
                return Err(CoordinatorAuditError::QueueLogicalIndex);
            }
            let mut physical_live_counts: HashMap<&CoordinatorTicket, usize> = HashMap::new();
            for ticket in &queue.physical {
                if queue.live.contains(ticket) {
                    *physical_live_counts.entry(ticket).or_default() += 1;
                }
            }
            if expected
                .iter()
                .any(|ticket| physical_live_counts.get(ticket) != Some(&1))
            {
                return Err(CoordinatorAuditError::QueuePhysicalIndex);
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn set_revision_for_test(
        &mut self,
        hash: &Byte32,
        revision: u64,
    ) -> Result<(), CoordinatorError> {
        let entry = self
            .entries
            .get_mut(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        let old_ticket = entry.ticket(hash);
        entry.revision = revision;
        if let Some(kind) = entry.location.queue_kind() {
            let new_ticket = entry.ticket(hash);
            let queue = self.queue_mut(kind)?;
            queue.remove_live(&old_ticket);
            queue.reserve_live()?;
            queue.push_reserved(kind, new_ticket)?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn physical_queue_slots_for_test(&self, kind: QueueKind) -> usize {
        self.queues
            .get(&kind)
            .map_or(0, |queue| queue.physical.len())
    }

    fn queue_mut(&mut self, kind: QueueKind) -> Result<&mut TicketQueue, CoordinatorError> {
        self.queues
            .get_mut(&kind)
            .ok_or(CoordinatorError::QueueInvariant(kind))
    }

    fn peek_live_ticket(
        &mut self,
        kind: QueueKind,
    ) -> Result<Option<CoordinatorTicket>, CoordinatorError> {
        let queue = self.queue_mut(kind)?;
        loop {
            let Some(ticket) = queue.physical.front().cloned() else {
                return Ok(None);
            };
            if queue.live.contains(&ticket) {
                return Ok(Some(ticket));
            }
            queue.physical.pop_front();
        }
    }

    fn consume_front_ticket(
        &mut self,
        kind: QueueKind,
        ticket: &CoordinatorTicket,
    ) -> Result<(), CoordinatorError> {
        let queue = self.queue_mut(kind)?;
        if queue.physical.front() != Some(ticket) || !queue.live.remove(ticket) {
            return Err(CoordinatorError::QueueInvariant(kind));
        }
        queue.physical.pop_front();
        queue.compact();
        Ok(())
    }

    fn remove_current_queue_ticket(&mut self, hash: &Byte32) -> Result<(), CoordinatorError> {
        let Some(entry) = self.entries.get(hash) else {
            return Err(CoordinatorError::Missing(hash.clone()));
        };
        let Some(kind) = entry.location.queue_kind() else {
            return Ok(());
        };
        let ticket = entry.ticket(hash);
        let queue = self.queue_mut(kind)?;
        queue.remove_live(&ticket);
        queue.compact();
        Ok(())
    }

    fn validate_version_location_phase(
        &self,
        hash: &Byte32,
        version: CoordinatorVersion,
        expected_location: &CoordinatorLocation,
        expected_phase: PayloadPhase,
    ) -> Result<(), CoordinatorError> {
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        if entry.incarnation != version.incarnation {
            return Err(CoordinatorError::IncarnationMismatch {
                expected: version.incarnation,
                actual: entry.incarnation,
            });
        }
        if entry.revision != version.revision {
            return Err(CoordinatorError::RevisionMismatch {
                expected: version.revision,
                actual: entry.revision,
            });
        }
        if entry.location != *expected_location {
            return Err(CoordinatorError::LocationMismatch {
                expected: expected_location.clone(),
                actual: entry.location.clone(),
            });
        }
        let actual_phase = entry.phase.kind();
        if actual_phase != expected_phase {
            return Err(CoordinatorError::PhaseMismatch {
                expected: expected_phase,
                actual: actual_phase,
            });
        }
        Ok(())
    }

    fn ensure_revision_capacity(&self, hash: &Byte32) -> Result<(), CoordinatorError> {
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        if entry.revision == u64::MAX {
            return Err(CoordinatorError::RevisionExhausted(hash.clone()));
        }
        Ok(())
    }

    fn check_add_budget(
        &self,
        peer: Option<PeerIndex>,
        charge: CoordinatorResidency,
    ) -> Result<(), CoordinatorError> {
        let next_global = self
            .global_usage
            .checked_add(charge)
            .ok_or(CoordinatorError::GlobalBudgetExceeded)?;
        if !next_global.fits(self.limits.global) {
            return Err(CoordinatorError::GlobalBudgetExceeded);
        }
        if let (Some(peer), Some(limit)) = (peer, self.limits.per_peer) {
            let next_peer = self
                .peer_usage(peer)
                .checked_add(charge)
                .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
            if !next_peer.fits(limit) {
                return Err(CoordinatorError::PeerBudgetExceeded(peer));
            }
        }
        Ok(())
    }

    fn check_recharge(&self, hash: &Byte32, new_bytes: usize) -> Result<(), CoordinatorError> {
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        let old = CoordinatorResidency::new(1, entry.charge_bytes);
        let new = CoordinatorResidency::new(1, new_bytes);
        let next_global = self
            .global_usage
            .checked_sub(old)
            .and_then(|usage| usage.checked_add(new))
            .ok_or(CoordinatorError::GlobalBudgetExceeded)?;
        if !next_global.fits(self.limits.global) {
            return Err(CoordinatorError::GlobalBudgetExceeded);
        }
        if let (Some(peer), Some(limit)) = (entry.peer, self.limits.per_peer) {
            let next_peer = self
                .peer_usage(peer)
                .checked_sub(old)
                .and_then(|usage| usage.checked_add(new))
                .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
            if !next_peer.fits(limit) {
                return Err(CoordinatorError::PeerBudgetExceeded(peer));
            }
        }
        Ok(())
    }

    fn apply_recharge(&mut self, hash: &Byte32, new_bytes: usize) -> Result<(), CoordinatorError> {
        let (peer, old_bytes) = self
            .entries
            .get(hash)
            .map(|entry| (entry.peer, entry.charge_bytes))
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        let old = CoordinatorResidency::new(1, old_bytes);
        let new = CoordinatorResidency::new(1, new_bytes);
        self.global_usage = self
            .global_usage
            .checked_sub(old)
            .and_then(|usage| usage.checked_add(new))
            .ok_or(CoordinatorError::GlobalBudgetExceeded)?;
        if let Some(peer) = peer {
            let usage = self
                .peer_usage
                .get_mut(&peer)
                .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
            *usage = usage
                .checked_sub(old)
                .and_then(|usage| usage.checked_add(new))
                .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
        }
        self.entries
            .get_mut(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?
            .charge_bytes = new_bytes;
        Ok(())
    }

    fn active_blockers_for_inputs(
        &self,
        hash: &Byte32,
        inputs: &HashSet<OutPoint>,
    ) -> HashSet<Byte32> {
        inputs
            .iter()
            .filter_map(|input| self.active_by_input.get(input).cloned())
            .filter(|blocker| blocker != hash)
            .collect()
    }

    fn compare_candidates(
        left_hash: &Byte32,
        left: &CandidateMeta,
        right_hash: &Byte32,
        right: &CandidateMeta,
    ) -> Ordering {
        let left_rate = u128::from(left.fee) * right.tx_size as u128;
        let right_rate = u128::from(right.fee) * left.tx_size as u128;
        left_rate
            .cmp(&right_rate)
            .then_with(|| left.fee.cmp(&right.fee))
            .then_with(|| right.arrival.cmp(&left.arrival))
            .then_with(|| right_hash.as_slice().cmp(left_hash.as_slice()))
    }

    fn claim_conflict_inputs(&mut self, hash: &Byte32) -> Result<(), CoordinatorError> {
        let inputs = self
            .entries
            .get(hash)
            .and_then(|entry| entry.candidate.as_ref())
            .map(|candidate| candidate.inputs.clone())
            .ok_or(CoordinatorError::ConflictInvariant)?;
        if inputs
            .iter()
            .any(|input| self.active_by_input.contains_key(input))
        {
            return Err(CoordinatorError::ConflictInvariant);
        }
        for input in inputs {
            self.active_by_input.insert(input, hash.clone());
        }
        Ok(())
    }

    fn release_conflict_claims(&mut self, hash: &Byte32) -> Result<(), CoordinatorError> {
        let inputs = self
            .entries
            .get(hash)
            .and_then(|entry| entry.candidate.as_ref())
            .map(|candidate| candidate.inputs.clone())
            .ok_or(CoordinatorError::ConflictInvariant)?;
        for input in inputs {
            if self.active_by_input.get(&input) == Some(hash) {
                self.active_by_input.remove(&input);
            }
        }
        Ok(())
    }

    fn release_conflict_claims_if_present(&mut self, hash: &Byte32) {
        let Some(inputs) = self
            .entries
            .get(hash)
            .and_then(|entry| entry.candidate.as_ref())
            .map(|candidate| candidate.inputs.clone())
        else {
            return;
        };
        for input in inputs {
            if self.active_by_input.get(&input) == Some(hash) {
                self.active_by_input.remove(&input);
            }
        }
    }

    fn remove_conflict_waiter_links(&mut self, hash: &Byte32) -> Result<(), CoordinatorError> {
        let blockers = match self.entries.get(hash).map(|entry| &entry.location) {
            Some(CoordinatorLocation::WaitingConflict { blockers }) => blockers.clone(),
            Some(_) => return Ok(()),
            None => return Err(CoordinatorError::Missing(hash.clone())),
        };
        for blocker in blockers {
            if let Some(waiters) = self.waiters_by_blocker.get_mut(&blocker) {
                waiters.remove(hash);
                if waiters.is_empty() {
                    self.waiters_by_blocker.remove(&blocker);
                }
            }
        }
        Ok(())
    }

    fn invalidate_conflict_waiters(&mut self, blocker: &Byte32) -> Result<(), CoordinatorError> {
        let waiters = self
            .waiters_by_blocker
            .get(blocker)
            .cloned()
            .unwrap_or_default();
        for waiter in &waiters {
            let entry = self
                .entries
                .get(waiter)
                .ok_or_else(|| CoordinatorError::Missing(waiter.clone()))?;
            if !matches!(
                &entry.location,
                CoordinatorLocation::WaitingConflict { blockers }
                    if blockers.contains(blocker)
            ) {
                return Err(CoordinatorError::ConflictInvariant);
            }
            self.ensure_revision_capacity(waiter)?;
        }
        self.conflict_rechecks
            .try_reserve(waiters.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.conflict_recheck_set
            .try_reserve(waiters.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.waiters_by_blocker.remove(blocker);
        for waiter in waiters {
            self.remove_conflict_waiter_links(&waiter)?;
            let entry = self
                .entries
                .get_mut(&waiter)
                .ok_or_else(|| CoordinatorError::Missing(waiter.clone()))?;
            entry.location = CoordinatorLocation::ConflictRecheck;
            entry.revision += 1;
            if self.conflict_recheck_set.insert(waiter.clone()) {
                self.conflict_rechecks.push_back(waiter);
            }
        }
        Ok(())
    }

    fn remove_conflict_indexes(&mut self, hash: &Byte32) -> Result<(), CoordinatorError> {
        self.invalidate_conflict_waiters(hash)?;
        self.remove_conflict_waiter_links(hash)?;
        self.release_conflict_claims_if_present(hash);
        self.conflict_recheck_set.remove(hash);
        let Some(candidate) = self
            .entries
            .get(hash)
            .and_then(|entry| entry.candidate.as_ref())
            .cloned()
        else {
            return Ok(());
        };
        self.conflict_edge_count = self
            .conflict_edge_count
            .checked_sub(candidate.inputs.len())
            .ok_or(CoordinatorError::ConflictInvariant)?;
        for input in candidate.inputs {
            if let Some(candidates) = self.candidates_by_input.get_mut(&input) {
                candidates.remove(hash);
                if candidates.is_empty() {
                    self.candidates_by_input.remove(&input);
                }
            }
        }
        Ok(())
    }

    fn preflight_remove_conflict_indexes(&mut self, hash: &Byte32) -> Result<(), CoordinatorError> {
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        if let Some(candidate) = &entry.candidate
            && self.conflict_edge_count < candidate.inputs.len()
        {
            return Err(CoordinatorError::ConflictInvariant);
        }
        if let CoordinatorLocation::WaitingConflict { blockers } = &entry.location {
            for blocker in blockers {
                if !self
                    .waiters_by_blocker
                    .get(blocker)
                    .is_some_and(|waiters| waiters.contains(hash))
                {
                    return Err(CoordinatorError::ConflictInvariant);
                }
            }
        }
        let waiters = self
            .waiters_by_blocker
            .get(hash)
            .cloned()
            .unwrap_or_default();
        for waiter in &waiters {
            let waiter_entry = self
                .entries
                .get(waiter)
                .ok_or_else(|| CoordinatorError::Missing(waiter.clone()))?;
            if !matches!(
                &waiter_entry.location,
                CoordinatorLocation::WaitingConflict { blockers }
                    if blockers.contains(hash)
            ) {
                return Err(CoordinatorError::ConflictInvariant);
            }
            self.ensure_revision_capacity(waiter)?;
        }
        self.conflict_rechecks
            .try_reserve(waiters.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.conflict_recheck_set
            .try_reserve(waiters.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)
    }

    fn recheck_conflict_candidate(
        &mut self,
        hash: &Byte32,
    ) -> Result<Option<CoordinatorTicket>, CoordinatorError> {
        let entry = self
            .entries
            .get(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        if entry.location != CoordinatorLocation::ConflictRecheck {
            return Err(CoordinatorError::LocationMismatch {
                expected: CoordinatorLocation::ConflictRecheck,
                actual: entry.location.clone(),
            });
        }
        if entry.phase.kind() != PayloadPhase::Verified {
            return Err(CoordinatorError::PhaseMismatch {
                expected: PayloadPhase::Verified,
                actual: entry.phase.kind(),
            });
        }
        self.ensure_revision_capacity(hash)?;
        let candidate = entry
            .candidate
            .as_ref()
            .cloned()
            .ok_or(CoordinatorError::ConflictInvariant)?;
        let blockers = self.active_blockers_for_inputs(hash, &candidate.inputs);
        let can_preempt = !blockers.is_empty()
            && blockers.iter().all(|blocker| {
                self.entries.get(blocker).is_some_and(|blocker_entry| {
                    blocker_entry.location == CoordinatorLocation::ReadyToCommit
                        && blocker_entry
                            .candidate
                            .as_ref()
                            .is_some_and(|blocker_candidate| {
                                Self::compare_candidates(
                                    hash,
                                    &candidate,
                                    blocker,
                                    blocker_candidate,
                                ) == Ordering::Greater
                            })
                })
            });
        let mut inherited_waiters = HashSet::new();
        if can_preempt {
            for blocker in &blockers {
                self.ensure_revision_capacity(blocker)?;
                if let Some(waiters) = self.waiters_by_blocker.get(blocker) {
                    inherited_waiters.extend(waiters.iter().cloned());
                }
            }
            for waiter in &inherited_waiters {
                self.ensure_revision_capacity(waiter)?;
            }
        } else {
            for blocker in &blockers {
                if self
                    .waiters_by_blocker
                    .get(blocker)
                    .map_or(0, HashSet::len)
                    .saturating_add(1)
                    > self.limits.max_candidates_per_input
                {
                    return Err(CoordinatorError::ConflictInvariant);
                }
            }
        }
        if blockers.is_empty() || can_preempt {
            self.queue_mut(QueueKind::Commit)?.reserve_live()?;
        }
        self.conflict_rechecks
            .try_reserve(inherited_waiters.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.conflict_recheck_set
            .try_reserve(inherited_waiters.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;

        if can_preempt {
            for blocker in &blockers {
                self.invalidate_conflict_waiters(blocker)?;
                self.remove_current_queue_ticket(blocker)?;
                self.release_conflict_claims(blocker)?;
                let blocker_entry = self
                    .entries
                    .get_mut(blocker)
                    .ok_or_else(|| CoordinatorError::Missing(blocker.clone()))?;
                blocker_entry.location = CoordinatorLocation::WaitingConflict {
                    blockers: HashSet::from([hash.clone()]),
                };
                blocker_entry.revision += 1;
                self.waiters_by_blocker
                    .entry(hash.clone())
                    .or_default()
                    .insert(blocker.clone());
            }
        }

        if blockers.is_empty() || can_preempt {
            let (version, ticket) = {
                let entry = self
                    .entries
                    .get_mut(hash)
                    .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
                entry.location = CoordinatorLocation::ReadyToCommit;
                entry.revision += 1;
                (entry.version(), entry.ticket(hash))
            };
            self.claim_conflict_inputs(hash)?;
            self.queue_mut(QueueKind::Commit)?
                .push_reserved(QueueKind::Commit, ticket.clone())?;
            let _ = version;
            Ok(Some(ticket))
        } else {
            let entry = self
                .entries
                .get_mut(hash)
                .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
            entry.location = CoordinatorLocation::WaitingConflict {
                blockers: blockers.clone(),
            };
            entry.revision += 1;
            for blocker in blockers {
                self.waiters_by_blocker
                    .entry(blocker)
                    .or_default()
                    .insert(hash.clone());
            }
            Ok(None)
        }
    }

    fn remove_present(
        &mut self,
        hash: &Byte32,
    ) -> Result<CoordinatorEntry<R, U, V>, CoordinatorError> {
        self.preflight_remove_conflict_indexes(hash)?;
        self.remove_current_queue_ticket(hash)?;
        self.remove_conflict_indexes(hash)?;
        let entry = self
            .entries
            .remove(hash)
            .ok_or_else(|| CoordinatorError::Missing(hash.clone()))?;
        let charge = CoordinatorResidency::new(1, entry.charge_bytes);
        self.global_usage = self
            .global_usage
            .checked_sub(charge)
            .ok_or(CoordinatorError::GlobalBudgetExceeded)?;
        self.by_short_id.remove(&entry.short_id);
        if let Some(peer) = entry.peer {
            let remove_usage = {
                let usage = self
                    .peer_usage
                    .get_mut(&peer)
                    .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
                *usage = usage
                    .checked_sub(charge)
                    .ok_or(CoordinatorError::PeerBudgetExceeded(peer))?;
                *usage == CoordinatorResidency::default()
            };
            if remove_usage {
                self.peer_usage.remove(&peer);
            }
            if let Some(hashes) = self.by_peer.get_mut(&peer) {
                hashes.remove(hash);
                if hashes.is_empty() {
                    self.by_peer.remove(&peer);
                }
            }
        }
        for parent in &entry.dependencies {
            if let Some(children) = self.by_parent.get_mut(parent) {
                children.remove(hash);
                if children.is_empty() {
                    self.by_parent.remove(parent);
                }
            }
        }
        Ok(entry)
    }

    fn terminal_record(
        hash: Byte32,
        entry: CoordinatorEntry<R, U, V>,
        disposition: TerminalDisposition,
    ) -> TerminalRecord<R, U, V> {
        let later_phase = match entry.phase {
            ResidentPhase::Raw => None,
            ResidentPhase::Unverified(payload) => Some(TerminalPhase::Unverified(payload)),
            ResidentPhase::Verified(payload) => Some(TerminalPhase::Verified(payload)),
        };
        TerminalRecord {
            hash,
            short_id: entry.short_id,
            raw: entry.raw,
            later_phase,
            peer: entry.peer,
            disposition,
        }
    }

    fn phase_location_valid(phase: PayloadPhase, location: &CoordinatorLocation) -> bool {
        matches!(
            (phase, location),
            (
                PayloadPhase::Raw,
                CoordinatorLocation::RawQueued(_)
                    | CoordinatorLocation::RawActive(_)
                    | CoordinatorLocation::WaitingParents { .. }
                    | CoordinatorLocation::Invalidated { .. }
            ) | (
                PayloadPhase::Unverified,
                CoordinatorLocation::VerifyQueued
                    | CoordinatorLocation::VerifyActive
                    | CoordinatorLocation::Invalidated { .. }
            ) | (
                PayloadPhase::Verified,
                CoordinatorLocation::ReadyToCommit
                    | CoordinatorLocation::WaitingPoolInputs { .. }
                    | CoordinatorLocation::WaitingConflict { .. }
                    | CoordinatorLocation::ConflictRecheck
                    | CoordinatorLocation::Committing
                    | CoordinatorLocation::Invalidated { .. }
            )
        )
    }
}
