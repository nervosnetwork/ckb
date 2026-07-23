use ckb_network::PeerIndex;
use ckb_types::packed::{Byte32, OutPoint, ProposalShortId};
use ckb_types::prelude::Entity;
use std::cmp::Ordering;
use std::collections::{HashSet, VecDeque};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CoordinatorVerifyOrdering {
    ArrivalTime,
    FeeRate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkerCapability {
    Any,
    SmallCycleOnly,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) struct VerifySchedule {
    pub(crate) fee_rate_per_kb: u64,
    pub(crate) is_large_cycle: bool,
}

impl VerifySchedule {
    pub(crate) const fn new(fee_rate_per_kb: u64, is_large_cycle: bool) -> Self {
        Self {
            fee_rate_per_kb,
            is_large_cycle,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QueueOrdering {
    Fifo,
    FeeRate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CoordinatorLocation {
    RawQueued(RawStage),
    RawActive(RawStage),
    WaitingParents { missing: HashSet<Byte32> },
    VerifyQueued,
    VerifyActive,
    ReadyToCommit,
    WaitingConflict { blockers: HashSet<Byte32> },
    ConflictRecheck,
    Committing,
    Invalidated { cause: Byte32 },
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
    pub(crate) max_dependency_ancestors: usize,
    pub(crate) max_capacity_evictions_per_transition: usize,
    pub(crate) max_conflict_inputs_per_entry: usize,
    pub(crate) max_candidates_per_input: usize,
    pub(crate) max_conflict_edges: usize,
    pub(crate) metadata_cost: CoordinatorMetadataCost,
    pub(crate) max_active_work: usize,
    pub(crate) max_active_work_per_peer: usize,
    pub(crate) verify_ordering: CoordinatorVerifyOrdering,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CoordinatorReconciliationLimits {
    pub(crate) max_dependency_ancestors: usize,
    pub(crate) max_capacity_evictions_per_transition: usize,
}

impl CoordinatorReconciliationLimits {
    pub(crate) const fn new(
        max_dependency_ancestors: usize,
        max_capacity_evictions_per_transition: usize,
    ) -> Self {
        Self {
            max_dependency_ancestors,
            max_capacity_evictions_per_transition,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CoordinatorMetadataCost {
    pub(crate) entry_bytes: usize,
    pub(crate) dependency_edge_bytes: usize,
    pub(crate) lifecycle_ticket_bytes: usize,
    pub(crate) deadline_ticket_bytes: usize,
    pub(crate) conflict_edge_bytes: usize,
}

impl CoordinatorLimits {
    pub(crate) const fn new(
        global: CoordinatorResidency,
        per_peer: Option<CoordinatorResidency>,
        max_dependencies_per_entry: usize,
        max_dependents_per_parent: usize,
        reconciliation: CoordinatorReconciliationLimits,
    ) -> Self {
        Self {
            global,
            per_peer,
            max_dependencies_per_entry,
            max_dependents_per_parent,
            max_dependency_ancestors: reconciliation.max_dependency_ancestors,
            max_capacity_evictions_per_transition: reconciliation
                .max_capacity_evictions_per_transition,
            max_conflict_inputs_per_entry: max_dependencies_per_entry,
            max_candidates_per_input: max_dependents_per_parent,
            max_conflict_edges: global.entries.saturating_mul(max_dependencies_per_entry),
            metadata_cost: CoordinatorMetadataCost {
                entry_bytes: 0,
                dependency_edge_bytes: 0,
                lifecycle_ticket_bytes: 0,
                deadline_ticket_bytes: 0,
                conflict_edge_bytes: 0,
            },
            max_active_work: global.entries,
            max_active_work_per_peer: match per_peer {
                Some(limit) => limit.entries,
                None => global.entries,
            },
            verify_ordering: CoordinatorVerifyOrdering::ArrivalTime,
        }
    }

    pub(crate) const fn with_capacity_reconciliation_limits(
        mut self,
        max_dependency_ancestors: usize,
        max_capacity_evictions_per_transition: usize,
    ) -> Self {
        self.max_dependency_ancestors = max_dependency_ancestors;
        self.max_capacity_evictions_per_transition = max_capacity_evictions_per_transition;
        self
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

    pub(crate) const fn with_verify_ordering(
        mut self,
        verify_ordering: CoordinatorVerifyOrdering,
    ) -> Self {
        self.verify_ordering = verify_ordering;
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
    pub(super) queue_sequence: u64,
    pub(super) verify_schedule: VerifySchedule,
}

#[derive(Debug, Clone)]
pub(crate) struct RawWorkLease<R> {
    pub(crate) hash: Byte32,
    pub(crate) stage: RawStage,
    pub(crate) version: CoordinatorVersion,
    pub(crate) payload: Arc<R>,
    pub(crate) source: CoordinatorSource,
}

#[derive(Debug, Clone)]
pub(crate) struct VerifyWorkLease<U> {
    pub(crate) hash: Byte32,
    pub(crate) version: CoordinatorVersion,
    pub(crate) payload: Arc<U>,
    pub(crate) source: CoordinatorSource,
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
    pub(crate) ready_children: Vec<CoordinatorTicket>,
}

#[derive(Debug)]
pub(crate) struct ExternalCommitRecord<R> {
    pub(crate) hash: Byte32,
    pub(crate) short_id: ProposalShortId,
    pub(crate) raw: Arc<R>,
    pub(crate) source: CoordinatorSource,
    pub(crate) ready_children: Vec<CoordinatorTicket>,
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
    CapacityEvicted,
    Removed,
    Cleared,
    Expired,
    DependencyFailed,
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
    DependencyCycle(Byte32),
    DependencyLimitExceeded,
    DependencyAncestorLimitExceeded,
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
    CapacityEvictionLimitExceeded,
    ArrivalSequenceExhausted,
    QueueSequenceExhausted,
    MaintenanceSequenceExhausted,
    MissingParentNotDependency {
        child: Byte32,
        parent: Byte32,
    },
    GlobalBudgetExceeded,
    PeerBudgetExceeded(PeerIndex),
    IncarnationExhausted,
    RevisionExhausted(Byte32),
    DeadlineGenerationExhausted(Byte32),
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
    DependencyInvalidated {
        child: Byte32,
        parent: Byte32,
    },
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
    DeadlineIndex,
    StateInvariant(Byte32),
    MetadataCharge,
    ActiveWork,
    DependencyMaintenanceIndex,
    BudgetExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RawLocation {
    Queued(RawStage),
    Active(RawStage),
    WaitingParents { missing: HashSet<Byte32> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UnverifiedLocation {
    Queued,
    Active,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PlainVerifiedLocation {
    Ready,
    Committing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CandidateLocation {
    Ready,
    WaitingConflict { blockers: HashSet<Byte32> },
    Recheck { sequence: u64 },
    Committing,
}

#[derive(Debug)]
pub(super) enum InvalidatedPayload<U, V> {
    Raw,
    Unverified(Arc<U>),
    Verified(Arc<V>),
}

impl<U, V> Clone for InvalidatedPayload<U, V> {
    fn clone(&self) -> Self {
        match self {
            Self::Raw => Self::Raw,
            Self::Unverified(payload) => Self::Unverified(Arc::clone(payload)),
            Self::Verified(payload) => Self::Verified(Arc::clone(payload)),
        }
    }
}

#[derive(Debug)]
pub(super) enum EntryState<R, U, V> {
    Raw {
        raw: Arc<R>,
        location: RawLocation,
    },
    Unverified {
        raw: Arc<R>,
        payload: Arc<U>,
        location: UnverifiedLocation,
    },
    PlainVerified {
        raw: Arc<R>,
        payload: Arc<V>,
        location: PlainVerifiedLocation,
    },
    CandidateVerified {
        raw: Arc<R>,
        payload: Arc<V>,
        candidate: CandidateMeta,
        location: CandidateLocation,
    },
    Invalidated {
        raw: Arc<R>,
        payload: InvalidatedPayload<U, V>,
        cause: Byte32,
        sequence: u64,
    },
}

impl<R, U, V> Clone for EntryState<R, U, V> {
    fn clone(&self) -> Self {
        match self {
            Self::Raw { raw, location } => Self::Raw {
                raw: Arc::clone(raw),
                location: location.clone(),
            },
            Self::Unverified {
                raw,
                payload,
                location,
            } => Self::Unverified {
                raw: Arc::clone(raw),
                payload: Arc::clone(payload),
                location: *location,
            },
            Self::PlainVerified {
                raw,
                payload,
                location,
            } => Self::PlainVerified {
                raw: Arc::clone(raw),
                payload: Arc::clone(payload),
                location: location.clone(),
            },
            Self::CandidateVerified {
                raw,
                payload,
                candidate,
                location,
            } => Self::CandidateVerified {
                raw: Arc::clone(raw),
                payload: Arc::clone(payload),
                candidate: candidate.clone(),
                location: location.clone(),
            },
            Self::Invalidated {
                raw,
                payload,
                cause,
                sequence,
            } => Self::Invalidated {
                raw: Arc::clone(raw),
                payload: payload.clone(),
                cause: cause.clone(),
                sequence: *sequence,
            },
        }
    }
}

impl<R, U, V> EntryState<R, U, V> {
    pub(super) fn raw(&self) -> &Arc<R> {
        match self {
            Self::Raw { raw, .. }
            | Self::Unverified { raw, .. }
            | Self::PlainVerified { raw, .. }
            | Self::CandidateVerified { raw, .. }
            | Self::Invalidated { raw, .. } => raw,
        }
    }

    pub(super) fn phase_kind(&self) -> PayloadPhase {
        match self {
            Self::Raw { .. }
            | Self::Invalidated {
                payload: InvalidatedPayload::Raw,
                ..
            } => PayloadPhase::Raw,
            Self::Unverified { .. }
            | Self::Invalidated {
                payload: InvalidatedPayload::Unverified(_),
                ..
            } => PayloadPhase::Unverified,
            Self::PlainVerified { .. }
            | Self::CandidateVerified { .. }
            | Self::Invalidated {
                payload: InvalidatedPayload::Verified(_),
                ..
            } => PayloadPhase::Verified,
        }
    }

    pub(super) fn location(&self) -> CoordinatorLocation {
        match self {
            Self::Raw {
                location: RawLocation::Queued(stage),
                ..
            } => CoordinatorLocation::RawQueued(*stage),
            Self::Raw {
                location: RawLocation::Active(stage),
                ..
            } => CoordinatorLocation::RawActive(*stage),
            Self::Raw {
                location: RawLocation::WaitingParents { missing },
                ..
            } => CoordinatorLocation::WaitingParents {
                missing: missing.clone(),
            },
            Self::Unverified {
                location: UnverifiedLocation::Queued,
                ..
            } => CoordinatorLocation::VerifyQueued,
            Self::Unverified {
                location: UnverifiedLocation::Active,
                ..
            } => CoordinatorLocation::VerifyActive,
            Self::PlainVerified {
                location: PlainVerifiedLocation::Ready,
                ..
            }
            | Self::CandidateVerified {
                location: CandidateLocation::Ready,
                ..
            } => CoordinatorLocation::ReadyToCommit,
            Self::CandidateVerified {
                location: CandidateLocation::WaitingConflict { blockers },
                ..
            } => CoordinatorLocation::WaitingConflict {
                blockers: blockers.clone(),
            },
            Self::CandidateVerified {
                location: CandidateLocation::Recheck { .. },
                ..
            } => CoordinatorLocation::ConflictRecheck,
            Self::PlainVerified {
                location: PlainVerifiedLocation::Committing,
                ..
            }
            | Self::CandidateVerified {
                location: CandidateLocation::Committing,
                ..
            } => CoordinatorLocation::Committing,
            Self::Invalidated { cause, .. } => CoordinatorLocation::Invalidated {
                cause: cause.clone(),
            },
        }
    }

    pub(super) fn queue_kind(&self) -> Option<QueueKind> {
        match self {
            Self::Raw {
                location: RawLocation::Queued(RawStage::PreCheck),
                ..
            } => Some(QueueKind::PreCheck),
            Self::Raw {
                location: RawLocation::Queued(RawStage::Resolve),
                ..
            } => Some(QueueKind::Resolve),
            Self::Unverified {
                location: UnverifiedLocation::Queued,
                ..
            } => Some(QueueKind::Verify),
            Self::PlainVerified {
                location: PlainVerifiedLocation::Ready,
                ..
            }
            | Self::CandidateVerified {
                location: CandidateLocation::Ready,
                ..
            } => Some(QueueKind::Commit),
            _ => None,
        }
    }

    pub(super) fn uses_active_slot(&self) -> bool {
        matches!(
            self,
            Self::Raw {
                location: RawLocation::Active(_),
                ..
            } | Self::Unverified {
                location: UnverifiedLocation::Active,
                ..
            } | Self::PlainVerified {
                location: PlainVerifiedLocation::Committing,
                ..
            } | Self::CandidateVerified {
                location: CandidateLocation::Committing,
                ..
            }
        )
    }

    pub(super) fn is_committing(&self) -> bool {
        matches!(
            self,
            Self::PlainVerified {
                location: PlainVerifiedLocation::Committing,
                ..
            } | Self::CandidateVerified {
                location: CandidateLocation::Committing,
                ..
            }
        )
    }

    pub(super) fn candidate(&self) -> Option<&CandidateMeta> {
        match self {
            Self::CandidateVerified { candidate, .. } => Some(candidate),
            _ => None,
        }
    }

    pub(super) fn waiting_conflict_blockers(&self) -> Option<&HashSet<Byte32>> {
        match self {
            Self::CandidateVerified {
                location: CandidateLocation::WaitingConflict { blockers },
                ..
            } => Some(blockers),
            _ => None,
        }
    }

    pub(super) fn invalidated_cause(&self) -> Option<&Byte32> {
        match self {
            Self::Invalidated { cause, .. } => Some(cause),
            _ => None,
        }
    }

    pub(super) fn maintenance_sequence(&self) -> Option<u64> {
        match self {
            Self::CandidateVerified {
                location: CandidateLocation::Recheck { sequence },
                ..
            }
            | Self::Invalidated { sequence, .. } => Some(*sequence),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub(crate) struct CoordinatorEntry<R, U, V> {
    pub(super) short_id: ProposalShortId,
    pub(super) state: EntryState<R, U, V>,
    pub(super) source: CoordinatorSource,
    pub(super) expires_at: Option<u64>,
    pub(super) raw_charge_bytes: usize,
    /// Total resident payload bytes for the raw phase bundle.
    pub(super) raw_resident_payload_bytes: usize,
    /// Total resident payload bytes for the current typed phase bundle. When
    /// a later phase retains the raw transaction for demotion/terminal
    /// handoff, that retained ownership is included in this value.
    pub(super) resident_payload_bytes: usize,
    pub(super) base_metadata_bytes: usize,
    pub(super) metadata_bytes: usize,
    pub(super) charge_bytes: usize,
    pub(super) dependencies: HashSet<Byte32>,
    pub(super) incarnation: u64,
    pub(super) revision: u64,
    pub(super) deadline_generation: u64,
    pub(super) queue_sequence: u64,
    pub(super) verify_schedule: VerifySchedule,
}

impl<R, U, V> Clone for CoordinatorEntry<R, U, V> {
    fn clone(&self) -> Self {
        Self {
            short_id: self.short_id.clone(),
            state: self.state.clone(),
            source: self.source,
            expires_at: self.expires_at,
            raw_charge_bytes: self.raw_charge_bytes,
            raw_resident_payload_bytes: self.raw_resident_payload_bytes,
            resident_payload_bytes: self.resident_payload_bytes,
            base_metadata_bytes: self.base_metadata_bytes,
            metadata_bytes: self.metadata_bytes,
            charge_bytes: self.charge_bytes,
            dependencies: self.dependencies.clone(),
            incarnation: self.incarnation,
            revision: self.revision,
            deadline_generation: self.deadline_generation,
            queue_sequence: self.queue_sequence,
            verify_schedule: self.verify_schedule,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CandidateMeta {
    pub(super) inputs: HashSet<OutPoint>,
    pub(super) fee: u64,
    pub(super) tx_size: usize,
    pub(super) arrival: u64,
}

impl<R, U, V> CoordinatorEntry<R, U, V> {
    pub(super) fn state_shape_valid(&self, hash: &Byte32, limits: &CoordinatorLimits) -> bool {
        if self.dependencies.contains(hash)
            || self.dependencies.len() > limits.max_dependencies_per_entry
            || (!matches!(&self.state, EntryState::Unverified { .. })
                && self.verify_schedule != VerifySchedule::default())
        {
            return false;
        }
        match &self.state {
            EntryState::Raw {
                location: RawLocation::WaitingParents { missing },
                ..
            } => !missing.is_empty() && missing.is_subset(&self.dependencies),
            EntryState::CandidateVerified {
                candidate,
                location,
                ..
            } => {
                !candidate.inputs.is_empty()
                    && candidate.inputs.len() <= limits.max_conflict_inputs_per_entry
                    && candidate.tx_size != 0
                    && match location {
                        CandidateLocation::WaitingConflict { blockers } => {
                            !blockers.is_empty()
                                && blockers.len() <= limits.max_conflict_inputs_per_entry
                        }
                        CandidateLocation::Ready
                        | CandidateLocation::Recheck { .. }
                        | CandidateLocation::Committing => true,
                    }
            }
            EntryState::Raw { .. }
            | EntryState::Unverified { .. }
            | EntryState::PlainVerified { .. }
            | EntryState::Invalidated { .. } => true,
        }
    }

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
            queue_sequence: self.queue_sequence,
            verify_schedule: self.verify_schedule,
        }
    }

    pub(super) fn phase_kind(&self) -> PayloadPhase {
        self.state.phase_kind()
    }

    pub(super) fn location(&self) -> CoordinatorLocation {
        self.state.location()
    }

    pub(super) fn queue_kind(&self) -> Option<QueueKind> {
        self.state.queue_kind()
    }

    pub(super) fn uses_active_slot(&self) -> bool {
        self.state.uses_active_slot()
    }

    pub(super) fn is_committing(&self) -> bool {
        self.state.is_committing()
    }

    pub(super) fn candidate(&self) -> Option<&CandidateMeta> {
        self.state.candidate()
    }

    pub(super) fn waiting_conflict_blockers(&self) -> Option<&HashSet<Byte32>> {
        self.state.waiting_conflict_blockers()
    }

    pub(super) fn invalidated_cause(&self) -> Option<&Byte32> {
        self.state.invalidated_cause()
    }

    pub(super) fn maintenance_sequence(&self) -> Option<u64> {
        self.state.maintenance_sequence()
    }

    pub(super) fn view(&self) -> CoordinatorView {
        CoordinatorView {
            short_id: self.short_id.clone(),
            phase: self.phase_kind(),
            location: self.location(),
            peer: self.source.peer(),
            source: self.source,
            charge_bytes: self.charge_bytes,
            dependencies: self.dependencies.clone(),
            version: self.version(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct TicketQueue {
    ordering: QueueOrdering,
    physical: VecDeque<CoordinatorTicket>,
    pub(super) live: HashSet<CoordinatorTicket>,
}

impl TicketQueue {
    pub(super) fn new(ordering: QueueOrdering) -> Self {
        Self {
            ordering,
            physical: VecDeque::new(),
            live: HashSet::new(),
        }
    }

    pub(super) fn reserve_live(
        &mut self,
        _priority: bool,
        _owner: QueueOwner,
    ) -> Result<(), CoordinatorError> {
        self.live
            .try_reserve(1)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.physical
            .try_reserve(1)
            .map_err(|_| CoordinatorError::QueueReservationFailed)
    }

    pub(super) fn reserve_many(
        &mut self,
        _priority: bool,
        _owners: impl IntoIterator<Item = QueueOwner>,
        count: usize,
    ) -> Result<(), CoordinatorError> {
        self.live
            .try_reserve(count)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.physical
            .try_reserve(count)
            .map_err(|_| CoordinatorError::QueueReservationFailed)
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
        self.physical.push_back(ticket);
        Ok(())
    }

    pub(super) fn peek_eligible<F>(&self, mut eligible: F) -> Option<CoordinatorTicket>
    where
        F: FnMut(&CoordinatorTicket) -> bool,
    {
        self.live
            .iter()
            .filter(|ticket| eligible(ticket))
            .min_by(|left, right| self.compare(left, right))
            .cloned()
    }

    pub(super) fn consume(
        &mut self,
        kind: QueueKind,
        ticket: &CoordinatorTicket,
    ) -> Result<(), CoordinatorError> {
        if !self.live.remove(ticket) {
            return Err(CoordinatorError::QueueInvariant(kind));
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

    pub(super) fn physical_len(&self) -> usize {
        self.physical.len()
    }

    pub(super) fn tickets(&self) -> impl Iterator<Item = &CoordinatorTicket> {
        self.physical.iter()
    }

    pub(super) fn structure_valid(&self) -> bool {
        self.live.iter().all(|ticket| {
            self.physical
                .iter()
                .filter(|physical| *physical == ticket)
                .count()
                == 1
        })
    }

    pub(super) fn rebuild_live(
        &mut self,
        _kind: QueueKind,
        tickets: Vec<CoordinatorTicket>,
    ) -> Result<(), CoordinatorError> {
        let mut live = HashSet::new();
        live.try_reserve(tickets.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.physical
            .try_reserve(tickets.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        live.extend(tickets.iter().cloned());
        self.live = live;
        self.physical.retain(|ticket| self.live.contains(ticket));
        for ticket in tickets {
            if self.physical.iter().any(|physical| physical == &ticket) {
                continue;
            }
            self.physical.push_back(ticket);
        }
        Ok(())
    }

    fn compare(&self, left: &CoordinatorTicket, right: &CoordinatorTicket) -> Ordering {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| match self.ordering {
                QueueOrdering::Fifo => Ordering::Equal,
                QueueOrdering::FeeRate => right
                    .verify_schedule
                    .fee_rate_per_kb
                    .cmp(&left.verify_schedule.fee_rate_per_kb),
            })
            .then_with(|| left.queue_sequence.cmp(&right.queue_sequence))
            .then_with(|| left.hash.as_slice().cmp(right.hash.as_slice()))
            .then_with(|| left.version.incarnation.cmp(&right.version.incarnation))
            .then_with(|| left.version.revision.cmp(&right.version.revision))
    }

    pub(super) fn clear(&mut self) {
        self.physical.clear();
        self.live.clear();
    }

    pub(super) fn ordering(&self) -> QueueOrdering {
        self.ordering
    }

    pub(super) fn ticket_is_eligible(
        ticket: &CoordinatorTicket,
        capability: WorkerCapability,
    ) -> bool {
        match capability {
            WorkerCapability::Any => true,
            WorkerCapability::SmallCycleOnly => !ticket.verify_schedule.is_large_cycle,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DeadlineTicket {
    pub(super) expires_at: u64,
    pub(super) hash: Byte32,
    pub(super) incarnation: u64,
    pub(super) generation: u64,
}

impl Ord for DeadlineTicket {
    fn cmp(&self, other: &Self) -> Ordering {
        self.expires_at
            .cmp(&other.expires_at)
            .then_with(|| self.hash.as_slice().cmp(other.hash.as_slice()))
            .then_with(|| self.incarnation.cmp(&other.incarnation))
            .then_with(|| self.generation.cmp(&other.generation))
    }
}

impl PartialOrd for DeadlineTicket {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
