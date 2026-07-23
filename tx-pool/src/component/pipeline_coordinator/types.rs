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
    pub(super) fn queue_kind(&self) -> Option<QueueKind> {
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

    pub(super) fn uses_active_slot(&self) -> bool {
        matches!(
            self,
            Self::RawActive(_) | Self::VerifyActive | Self::Committing
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PayloadPhase {
    Raw,
    Unverified,
    Verified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CoordinatorSource {
    Remote(PeerIndex),
    Local,
    Proposal,
}

impl CoordinatorSource {
    pub(super) fn peer(self) -> Option<PeerIndex> {
        match self {
            Self::Remote(peer) => Some(peer),
            Self::Local | Self::Proposal => None,
        }
    }

    pub(super) fn is_proposal(self) -> bool {
        self == Self::Proposal
    }

    pub(super) fn queue_owner(self) -> QueueOwner {
        match self {
            Self::Remote(peer) => QueueOwner::Remote(peer),
            Self::Local | Self::Proposal => QueueOwner::Trusted,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum QueueOwner {
    Trusted,
    Remote(PeerIndex),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrustedSource {
    Local,
    Proposal,
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

    pub(super) fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_add(other.entries)?,
            bytes: self.bytes.checked_add(other.bytes)?,
        })
    }

    pub(super) fn checked_sub(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_sub(other.entries)?,
            bytes: self.bytes.checked_sub(other.bytes)?,
        })
    }

    pub(super) fn fits(self, limit: Self) -> bool {
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
    pub(crate) max_pool_inputs_per_entry: usize,
    pub(crate) max_pool_waiters_per_input: usize,
    pub(crate) max_pool_input_edges: usize,
    pub(crate) metadata_cost: CoordinatorMetadataCost,
    pub(crate) max_active_work: usize,
    pub(crate) max_active_work_per_peer: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CoordinatorMetadataCost {
    pub(crate) entry_bytes: usize,
    pub(crate) dependency_edge_bytes: usize,
    pub(crate) lifecycle_ticket_bytes: usize,
    pub(crate) deadline_ticket_bytes: usize,
    pub(crate) conflict_edge_bytes: usize,
    pub(crate) pool_input_edge_bytes: usize,
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
            max_pool_inputs_per_entry: max_dependencies_per_entry,
            max_pool_waiters_per_input: max_dependents_per_parent,
            max_pool_input_edges: global.entries.saturating_mul(max_dependencies_per_entry),
            metadata_cost: CoordinatorMetadataCost {
                entry_bytes: 0,
                dependency_edge_bytes: 0,
                lifecycle_ticket_bytes: 0,
                deadline_ticket_bytes: 0,
                conflict_edge_bytes: 0,
                pool_input_edge_bytes: 0,
            },
            max_active_work: global.entries,
            max_active_work_per_peer: match per_peer {
                Some(limit) => limit.entries,
                None => global.entries,
            },
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

    pub(crate) const fn with_pool_input_limits(
        mut self,
        max_pool_inputs_per_entry: usize,
        max_pool_waiters_per_input: usize,
        max_pool_input_edges: usize,
    ) -> Self {
        self.max_pool_inputs_per_entry = max_pool_inputs_per_entry;
        self.max_pool_waiters_per_input = max_pool_waiters_per_input;
        self.max_pool_input_edges = max_pool_input_edges;
        self
    }

    pub(crate) const fn with_metadata_cost(
        mut self,
        metadata_cost: CoordinatorMetadataCost,
    ) -> Self {
        self.metadata_cost = metadata_cost;
        self
    }

    pub(crate) const fn with_active_limits(
        mut self,
        max_active_work: usize,
        max_active_work_per_peer: usize,
    ) -> Self {
        self.max_active_work = max_active_work;
        self.max_active_work_per_peer = max_active_work_per_peer;
        self
    }
}

/// Conflict metadata becomes constructible pipeline state only when supplied
/// together with a successful verify lease completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedCandidate {
    pub(super) inputs: HashSet<OutPoint>,
    pub(super) fee: u64,
    pub(super) tx_size: usize,
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
    pub(super) owner: QueueOwner,
    pub(super) priority: bool,
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
    pub(crate) source: CoordinatorSource,
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
    Expired,
    Internal,
}

#[derive(Debug)]
pub(crate) struct TerminalRecord<R, U, V> {
    pub(crate) hash: Byte32,
    pub(crate) short_id: ProposalShortId,
    pub(crate) raw: Arc<R>,
    pub(crate) later_phase: Option<TerminalPhase<U, V>>,
    pub(crate) peer: Option<PeerIndex>,
    pub(crate) source: CoordinatorSource,
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
    pub(crate) source: CoordinatorSource,
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
    PoolInputLimitExceeded,
    PoolInputWaiterLimitExceeded(OutPoint),
    PoolInputEdgeLimitExceeded,
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
    SourceDowngrade,
    CommitInProgress(Byte32),
    ResidencyChargeOverflow,
    ActiveWorkLimitExceeded,
    PeerActiveWorkLimitExceeded(PeerIndex),
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
    PoolInputIndex,
    PoolInputEdgeCount,
    DeadlineIndex,
    MetadataCharge,
    ActiveWork,
    InvalidPhaseLocation(Byte32),
    BudgetExceeded,
}

#[derive(Debug)]
pub(crate) enum ResidentPhase<U, V> {
    Raw,
    Unverified(Arc<U>),
    Verified(Arc<V>),
}

impl<U, V> ResidentPhase<U, V> {
    pub(super) fn kind(&self) -> PayloadPhase {
        match self {
            Self::Raw => PayloadPhase::Raw,
            Self::Unverified(_) => PayloadPhase::Unverified,
            Self::Verified(_) => PayloadPhase::Verified,
        }
    }
}

#[derive(Debug)]
pub(crate) struct CoordinatorEntry<R, U, V> {
    pub(super) short_id: ProposalShortId,
    pub(super) raw: Arc<R>,
    pub(super) phase: ResidentPhase<U, V>,
    pub(super) location: CoordinatorLocation,
    pub(super) source: CoordinatorSource,
    pub(super) expires_at: Option<u64>,
    pub(super) raw_charge_bytes: usize,
    pub(super) raw_payload_bytes: usize,
    pub(super) payload_bytes: usize,
    pub(super) base_metadata_bytes: usize,
    pub(super) metadata_bytes: usize,
    pub(super) charge_bytes: usize,
    pub(super) dependencies: HashSet<Byte32>,
    pub(super) candidate: Option<CandidateMeta>,
    pub(super) incarnation: u64,
    pub(super) revision: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct CandidateMeta {
    pub(super) inputs: HashSet<OutPoint>,
    pub(super) fee: u64,
    pub(super) tx_size: usize,
    pub(super) arrival: u64,
}

impl<R, U, V> CoordinatorEntry<R, U, V> {
    pub(super) fn version(&self) -> CoordinatorVersion {
        CoordinatorVersion {
            incarnation: self.incarnation,
            revision: self.revision,
        }
    }

    pub(super) fn ticket(&self, hash: &Byte32) -> CoordinatorTicket {
        CoordinatorTicket {
            hash: hash.clone(),
            version: self.version(),
            owner: self.source.queue_owner(),
            priority: self.source.is_proposal(),
        }
    }

    pub(super) fn view(&self) -> CoordinatorView {
        CoordinatorView {
            short_id: self.short_id.clone(),
            phase: self.phase.kind(),
            location: self.location.clone(),
            peer: self.source.peer(),
            source: self.source,
            charge_bytes: self.charge_bytes,
            dependencies: self.dependencies.clone(),
            version: self.version(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct TicketLane {
    pub(super) buckets: HashMap<QueueOwner, VecDeque<CoordinatorTicket>>,
    pub(super) rotation: VecDeque<QueueOwner>,
    rotating: HashSet<QueueOwner>,
    physical_len: usize,
}

impl TicketLane {
    fn reserve(&mut self, owner: QueueOwner, additional: usize) -> Result<(), CoordinatorError> {
        self.buckets
            .try_reserve(1)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.rotation
            .try_reserve(1)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.rotating
            .try_reserve(1)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.buckets
            .entry(owner)
            .or_default()
            .try_reserve(additional)
            .map_err(|_| CoordinatorError::QueueReservationFailed)
    }

    fn push_reserved(
        &mut self,
        kind: QueueKind,
        ticket: CoordinatorTicket,
    ) -> Result<(), CoordinatorError> {
        let bucket = self
            .buckets
            .get_mut(&ticket.owner)
            .ok_or(CoordinatorError::QueueInvariant(kind))?;
        if bucket.is_empty() && self.rotating.insert(ticket.owner) {
            self.rotation.push_back(ticket.owner);
        }
        bucket.push_back(ticket);
        self.physical_len = self
            .physical_len
            .checked_add(1)
            .ok_or(CoordinatorError::QueueInvariant(kind))?;
        Ok(())
    }

    fn peek_eligible<F>(
        &mut self,
        live: &HashSet<CoordinatorTicket>,
        mut eligible: F,
    ) -> Option<CoordinatorTicket>
    where
        F: FnMut(QueueOwner) -> bool,
    {
        let attempts = self.rotation.len();
        for _ in 0..attempts {
            let owner = self.rotation.pop_front()?;
            if !self.rotating.contains(&owner) {
                continue;
            }
            let Some(bucket) = self.buckets.get_mut(&owner) else {
                self.rotating.remove(&owner);
                continue;
            };
            while bucket.front().is_some_and(|ticket| !live.contains(ticket)) {
                bucket.pop_front();
                self.physical_len = self.physical_len.saturating_sub(1);
            }
            if bucket.is_empty() {
                self.buckets.remove(&owner);
                self.rotating.remove(&owner);
                continue;
            }
            self.rotation.push_back(owner);
            if eligible(owner) {
                return bucket.front().cloned();
            }
        }
        None
    }

    fn consume(
        &mut self,
        kind: QueueKind,
        ticket: &CoordinatorTicket,
    ) -> Result<(), CoordinatorError> {
        {
            let bucket = self
                .buckets
                .get_mut(&ticket.owner)
                .ok_or(CoordinatorError::QueueInvariant(kind))?;
            if bucket.front() != Some(ticket) {
                return Err(CoordinatorError::QueueInvariant(kind));
            }
            bucket.pop_front();
        }
        self.physical_len = self
            .physical_len
            .checked_sub(1)
            .ok_or(CoordinatorError::QueueInvariant(kind))?;
        Ok(())
    }

    fn compact(&mut self, live: &HashSet<CoordinatorTicket>) {
        for bucket in self.buckets.values_mut() {
            bucket.retain(|ticket| live.contains(ticket));
        }
        self.buckets.retain(|_, bucket| !bucket.is_empty());
        self.rotation.clear();
        self.rotating.clear();
        self.physical_len = 0;
        for (owner, bucket) in &self.buckets {
            self.rotation.push_back(*owner);
            self.rotating.insert(*owner);
            self.physical_len = self.physical_len.saturating_add(bucket.len());
        }
    }

    pub(super) fn physical_len(&self) -> usize {
        self.physical_len
    }

    pub(super) fn tickets(&self) -> impl Iterator<Item = &CoordinatorTicket> {
        self.buckets.values().flat_map(|bucket| bucket.iter())
    }

    fn structure_valid(&self, priority: bool) -> bool {
        let mut rotation_counts = HashMap::new();
        for owner in &self.rotation {
            *rotation_counts.entry(*owner).or_insert(0usize) += 1;
        }
        self.physical_len
            == self
                .buckets
                .values()
                .map(VecDeque::len)
                .fold(0usize, usize::saturating_add)
            && rotation_counts
                .keys()
                .all(|owner| self.rotating.contains(owner))
            && self.rotating.iter().all(|owner| {
                rotation_counts.get(owner) == Some(&1) && self.buckets.contains_key(owner)
            })
            && self.buckets.iter().all(|(owner, bucket)| {
                (bucket.is_empty() || self.rotating.contains(owner))
                    && bucket
                        .iter()
                        .all(|ticket| ticket.owner == *owner && ticket.priority == priority)
            })
    }
}

#[derive(Debug, Default)]
pub(crate) struct TicketQueue {
    pub(super) priority: TicketLane,
    pub(super) normal: TicketLane,
    pub(super) live: HashSet<CoordinatorTicket>,
}

impl TicketQueue {
    pub(super) fn reserve_live(
        &mut self,
        priority: bool,
        owner: QueueOwner,
    ) -> Result<(), CoordinatorError> {
        self.live
            .try_reserve(1)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.lane_mut(priority).reserve(owner, 1)
    }

    pub(super) fn reserve_many(
        &mut self,
        priority: bool,
        owners: impl IntoIterator<Item = QueueOwner>,
        count: usize,
    ) -> Result<(), CoordinatorError> {
        self.live
            .try_reserve(count)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        for owner in owners {
            self.lane_mut(priority).reserve(owner, 1)?;
        }
        Ok(())
    }

    pub(super) fn push_reserved(
        &mut self,
        kind: QueueKind,
        ticket: CoordinatorTicket,
        priority: bool,
    ) -> Result<(), CoordinatorError> {
        if ticket.priority != priority || !self.live.insert(ticket.clone()) {
            return Err(CoordinatorError::QueueInvariant(kind));
        }
        self.lane_mut(priority).push_reserved(kind, ticket)
    }

    pub(super) fn peek_eligible<F>(&mut self, eligible: F) -> Option<CoordinatorTicket>
    where
        F: Copy + Fn(QueueOwner) -> bool,
    {
        self.priority
            .peek_eligible(&self.live, eligible)
            .or_else(|| self.normal.peek_eligible(&self.live, eligible))
    }

    pub(super) fn consume(
        &mut self,
        kind: QueueKind,
        ticket: &CoordinatorTicket,
    ) -> Result<(), CoordinatorError> {
        if !self.live.remove(ticket) {
            return Err(CoordinatorError::QueueInvariant(kind));
        }
        let result = self.lane_mut(ticket.priority).consume(kind, ticket);
        if result.is_err() {
            self.live.insert(ticket.clone());
            return result;
        }
        self.compact();
        Ok(())
    }

    pub(super) fn remove_live(&mut self, ticket: &CoordinatorTicket) {
        self.live.remove(ticket);
        self.compact();
    }

    pub(super) fn compact(&mut self) {
        const STALE_SLACK: usize = 64;
        if self.physical_len()
            > self
                .live
                .len()
                .saturating_mul(2)
                .saturating_add(STALE_SLACK)
        {
            self.priority.compact(&self.live);
            self.normal.compact(&self.live);
        }
    }

    pub(super) fn physical_len(&self) -> usize {
        self.priority
            .physical_len()
            .saturating_add(self.normal.physical_len())
    }

    pub(super) fn tickets(&self) -> impl Iterator<Item = &CoordinatorTicket> {
        self.priority.tickets().chain(self.normal.tickets())
    }

    pub(super) fn structure_valid(&self) -> bool {
        self.priority.structure_valid(true) && self.normal.structure_valid(false)
    }

    fn lane_mut(&mut self, priority: bool) -> &mut TicketLane {
        if priority {
            &mut self.priority
        } else {
            &mut self.normal
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DeadlineTicket {
    pub(super) expires_at: u64,
    pub(super) hash: Byte32,
    pub(super) incarnation: u64,
}

impl Ord for DeadlineTicket {
    fn cmp(&self, other: &Self) -> Ordering {
        self.expires_at
            .cmp(&other.expires_at)
            .then_with(|| self.hash.as_slice().cmp(other.hash.as_slice()))
            .then_with(|| self.incarnation.cmp(&other.incarnation))
    }
}

impl PartialOrd for DeadlineTicket {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
