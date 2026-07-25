use crate::constants::lazy_ticket_compaction_limit;
use crate::tx_source::SourceTrust;
use ckb_network::PeerIndex;
use ckb_types::packed::{Byte32, OutPoint, ProposalShortId};
use ckb_types::prelude::Entity;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::Arc;

#[cfg(test)]
#[path = "../tests/pipeline_coordinator_types_seam.rs"]
mod test_seam;
#[cfg(test)]
pub(crate) use test_seam::CoordinatorAuditError;

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

impl QueueKind {
    pub(super) const ALL: [Self; 4] = [Self::PreCheck, Self::Resolve, Self::Verify, Self::Commit];

    pub(super) const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CoordinatorVerifyOrdering {
    ArrivalTime,
    FeeRate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum QueueOrdering {
    Fifo,
    FeeRate,
    Candidate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CoordinatorLocation {
    RawQueued(RawStage),
    RawActive(RawStage),
    WaitingParents {
        missing: HashSet<Byte32>,
    },
    VerifyQueued,
    VerifyActive,
    /// A fully verified candidate. Commit eligibility is derived from the
    /// staged-conflict relation and deliberately is not another lifecycle
    /// location.
    Verified,
    Committing,
    Invalidated {
        cause: Byte32,
    },
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

    pub(crate) fn trust(self) -> SourceTrust {
        match self {
            Self::Remote(_) => SourceTrust::Remote,
            Self::Local => SourceTrust::Local,
            Self::Proposal => SourceTrust::Proposal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
    pub(super) candidate_rank: Option<CandidateRank>,
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
pub(crate) struct CommitHandoff<R> {
    #[cfg(test)]
    pub(crate) hash: Byte32,
    pub(crate) raw: Arc<R>,
    #[cfg(test)]
    pub(crate) peer: Option<PeerIndex>,
    #[cfg(test)]
    pub(crate) ready_children: Vec<CoordinatorTicket>,
}

#[derive(Debug)]
pub(crate) struct ExternalCommitRecord<R> {
    /// Raw payload of the coordinator owner consumed by an external
    /// Local/chain commit. Besides ownership transfer, production uses this
    /// to preserve immutable ingress attribution at the success boundary.
    pub(crate) raw: Arc<R>,
    #[cfg(test)]
    pub(crate) hash: Byte32,
    #[cfg(test)]
    pub(crate) ready_children: Vec<CoordinatorTicket>,
}

#[derive(Debug)]
pub(crate) struct ConflictCommitHandoff<R> {
    pub(crate) winner: CommitHandoff<R>,
    pub(crate) rejected: Vec<TerminalRecord<R>>,
}

/// Administrative/negative terminal outcomes deliberately exclude commit.
/// A committed payload can leave only through `commit_handoff` with a valid
/// `CommitLease` created from an eligible `Verified` candidate.
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
pub(crate) struct TerminalRecord<R> {
    pub(crate) hash: Byte32,
    pub(crate) raw: Arc<R>,
    pub(crate) source: CoordinatorSource,
    #[cfg(test)]
    pub(crate) disposition: TerminalDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CoordinatorView {
    pub(crate) short_id: ProposalShortId,
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
    ConflictCohortLimitExceeded,
    ConflictEdgeLimitExceeded,
    CapacityEvictionLimitExceeded,
    ArrivalSequenceExhausted,
    QueueSequenceExhausted,
    MaintenanceSequenceExhausted,
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
    QueueInvariant(QueueKind),
    QueueReservationFailed,
    /// An entry mutation escaped the presence snapshot of an active undo
    /// transaction. Returning an error before the write makes rollback
    /// completeness a coordinator invariant instead of a caller convention.
    UndoCohortViolation {
        hash: Byte32,
        mutation_file: &'static str,
        mutation_line: u32,
        active_members: Vec<Byte32>,
    },
    /// Command composition attempted to establish a second rollback owner.
    /// Composite operations must use transaction-only apply primitives.
    NestedUndoTransaction,
    ConflictInvariant,
    SourceDowngrade,
    /// The coordinator's current source owner no longer agrees with the
    /// immutable ingress attribution retained by the raw payload. This is an
    /// internal ownership invariant, never a transaction-level rejection.
    SourceAttributionMismatch,
    CommitInProgress(Byte32),
    ResidencyChargeOverflow,
    ActiveWorkLimitExceeded,
    PeerActiveWorkLimitExceeded(PeerIndex),
    DependencyInvalidated {
        child: Byte32,
        parent: Byte32,
    },
}

enum CoordinatorRejectionClass {
    Policy,
    FixedCapacity,
    RetryableCapacity,
}

impl CoordinatorError {
    /// Single authoritative classification table. Adapter predicates below
    /// are projections of this value so a new internal invariant error cannot
    /// accidentally be downgraded by updating only one of several lists.
    fn rejection_class(&self) -> Option<CoordinatorRejectionClass> {
        use CoordinatorRejectionClass::{FixedCapacity, Policy, RetryableCapacity};

        Some(match self {
            Self::SelfDependency(_)
            | Self::DependencyCycle(_)
            | Self::NoConflictInputs(_)
            | Self::ZeroTransactionSize(_)
            | Self::UnderReplacementFee { .. }
            | Self::UnderFeeRate { .. }
            | Self::FeeRateOverflow => Policy,
            Self::DependencyLimitExceeded
            | Self::ConflictInputLimitExceeded
            | Self::ResidencyChargeOverflow => FixedCapacity,
            Self::ShortIdCollision { .. }
            | Self::DependencyAncestorLimitExceeded
            | Self::ParentFanoutLimitExceeded(_)
            | Self::ConflictCandidateLimitExceeded(_)
            | Self::ConflictCohortLimitExceeded
            | Self::ConflictEdgeLimitExceeded
            | Self::CapacityEvictionLimitExceeded
            | Self::GlobalBudgetExceeded
            | Self::PeerBudgetExceeded(_)
            | Self::QueueReservationFailed
            | Self::ActiveWorkLimitExceeded
            | Self::PeerActiveWorkLimitExceeded(_) => RetryableCapacity,
            _ => return None,
        })
    }

    /// Errors caused by bounded admission/completion policy. These leave the
    /// coordinator transaction unchanged and may safely become a transaction
    /// rejection. Every variant not listed here denotes an ownership/index/
    /// sequence invariant and must never be downgraded to a per-transaction
    /// outcome by a production adapter.
    pub(crate) fn is_transaction_rejection(&self) -> bool {
        self.rejection_class().is_some()
    }

    /// Capacity pressure is retryable and must not poison recent-reject state.
    pub(crate) fn is_capacity_rejection(&self) -> bool {
        use CoordinatorRejectionClass::{FixedCapacity, RetryableCapacity};
        matches!(
            self.rejection_class(),
            Some(FixedCapacity | RetryableCapacity)
        )
    }

    /// Capacity conditions that can become admissible after another owner
    /// leaves or a queue reservation becomes available. This is deliberately
    /// narrower than [`Self::is_capacity_rejection`]: fixed per-transaction
    /// limits (for example too many dependencies or conflict inputs) also map
    /// to public `Full`, but retrying the identical payload can never change
    /// their outcome.
    pub(crate) fn is_retryable_capacity_rejection(&self) -> bool {
        use CoordinatorRejectionClass::RetryableCapacity;
        matches!(self.rejection_class(), Some(RetryableCapacity))
    }

    /// A worker lease may legitimately lose ownership to clear, dependency
    /// invalidation or another administrative transition while the worker is
    /// outside the coordinator lock. Identity/version errors prove that the
    /// caller no longer owns the lifecycle transition and may stop quietly.
    ///
    /// Every other error from a lease-terminal or required maintenance path
    /// is an internal progress failure: treating it as best-effort can strand
    /// an active/committing owner indefinitely.
    pub(crate) fn is_stale_lease(&self) -> bool {
        matches!(
            self,
            Self::Missing(_)
                | Self::IncarnationMismatch { .. }
                | Self::RevisionMismatch { .. }
                | Self::DependencyInvalidated { .. }
        )
    }
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
pub(super) enum CandidateLocation {
    Verified,
    Committing,
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
        verify_schedule: VerifySchedule,
    },
    CandidateVerified {
        raw: Arc<R>,
        payload: Arc<V>,
        candidate: CandidateMeta,
        location: CandidateLocation,
    },
    Invalidated {
        raw: Arc<R>,
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
                verify_schedule,
            } => Self::Unverified {
                raw: Arc::clone(raw),
                payload: Arc::clone(payload),
                location: *location,
                verify_schedule: *verify_schedule,
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
                cause,
                sequence,
            } => Self::Invalidated {
                raw: Arc::clone(raw),
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
            | Self::CandidateVerified { raw, .. }
            | Self::Invalidated { raw, .. } => raw,
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
            Self::CandidateVerified {
                location: CandidateLocation::Verified,
                ..
            } => CoordinatorLocation::Verified,
            Self::CandidateVerified {
                location: CandidateLocation::Committing,
                ..
            } => CoordinatorLocation::Committing,
            Self::Invalidated { cause, .. } => CoordinatorLocation::Invalidated {
                cause: cause.clone(),
            },
        }
    }

    /// Compare a lease's expected location without materializing an owned
    /// diagnostic view. Waiting-parent and conflict-blocker sets are cloned
    /// only on the exceptional mismatch path.
    pub(super) fn location_matches(&self, expected: &CoordinatorLocation) -> bool {
        match (self, expected) {
            (
                Self::Raw {
                    location: RawLocation::Queued(actual),
                    ..
                },
                CoordinatorLocation::RawQueued(expected),
            )
            | (
                Self::Raw {
                    location: RawLocation::Active(actual),
                    ..
                },
                CoordinatorLocation::RawActive(expected),
            ) => actual == expected,
            (
                Self::Raw {
                    location: RawLocation::WaitingParents { missing: actual },
                    ..
                },
                CoordinatorLocation::WaitingParents { missing: expected },
            ) => actual == expected,
            (
                Self::Unverified {
                    location: UnverifiedLocation::Queued,
                    ..
                },
                CoordinatorLocation::VerifyQueued,
            )
            | (
                Self::Unverified {
                    location: UnverifiedLocation::Active,
                    ..
                },
                CoordinatorLocation::VerifyActive,
            )
            | (
                Self::CandidateVerified {
                    location: CandidateLocation::Verified,
                    ..
                },
                CoordinatorLocation::Verified,
            )
            | (
                Self::CandidateVerified {
                    location: CandidateLocation::Committing,
                    ..
                },
                CoordinatorLocation::Committing,
            ) => true,
            (
                Self::Invalidated { cause: actual, .. },
                CoordinatorLocation::Invalidated { cause: expected },
            ) => actual == expected,
            _ => false,
        }
    }

    pub(super) fn verify_schedule(&self) -> VerifySchedule {
        match self {
            Self::Unverified {
                verify_schedule, ..
            } => *verify_schedule,
            _ => VerifySchedule::default(),
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
            } | Self::CandidateVerified {
                location: CandidateLocation::Committing,
                ..
            }
        )
    }

    pub(super) fn is_committing(&self) -> bool {
        matches!(
            self,
            Self::CandidateVerified {
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

    pub(super) fn invalidated_cause(&self) -> Option<&Byte32> {
        match self {
            Self::Invalidated { cause, .. } => Some(cause),
            _ => None,
        }
    }

    pub(super) fn maintenance_sequence(&self) -> Option<u64> {
        match self {
            Self::Invalidated { sequence, .. } => Some(*sequence),
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

/// The complete, deterministic preference order for staged conflict
/// candidates. Greater ranks win. `Committing` is an absolute freeze above
/// every verified neighbour; the remaining fields are the admission policy
/// shared by conflict scheduling and capacity victim selection.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(super) struct CandidateRank {
    pub(super) committing: bool,
    pub(super) source_strength: SourceTrust,
    pub(super) fee: u64,
    pub(super) tx_size: usize,
    pub(super) arrival: u64,
    pub(super) hash: Byte32,
}

impl CandidateRank {
    pub(super) fn verified(
        hash: &Byte32,
        source: CoordinatorSource,
        candidate: &CandidateMeta,
    ) -> Self {
        Self {
            committing: false,
            source_strength: source.trust(),
            fee: candidate.fee,
            tx_size: candidate.tx_size,
            arrival: candidate.arrival,
            hash: hash.clone(),
        }
    }

    pub(super) fn from_entry(
        hash: &Byte32,
        source: CoordinatorSource,
        candidate: &CandidateMeta,
        location: &CandidateLocation,
    ) -> Self {
        let mut rank = Self::verified(hash, source, candidate);
        rank.committing = *location == CandidateLocation::Committing;
        rank
    }
}

impl Ord for CandidateRank {
    fn cmp(&self, other: &Self) -> Ordering {
        let self_rate = u128::from(self.fee) * other.tx_size as u128;
        let other_rate = u128::from(other.fee) * self.tx_size as u128;
        self.committing
            .cmp(&other.committing)
            .then_with(|| self.source_strength.cmp(&other.source_strength))
            .then_with(|| self_rate.cmp(&other_rate))
            .then_with(|| self.fee.cmp(&other.fee))
            // Earlier arrival and then the smaller stable identity win.
            .then_with(|| other.arrival.cmp(&self.arrival))
            .then_with(|| other.hash.as_slice().cmp(self.hash.as_slice()))
            // Preserve Ord/Eq even for synthetic zero-fee ranks whose other
            // fields are identical. Real lifecycle hashes are unique.
            .then_with(|| self.tx_size.cmp(&other.tx_size))
    }
}

impl PartialOrd for CandidateRank {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Derived relation state for one staged candidate. `degree` bounds direct
/// conflict fanout; `stronger_count == 0` is commit eligibility for a
/// `Verified` candidate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct CandidateRelation {
    pub(super) degree: usize,
    pub(super) stronger_count: usize,
}

/// Weakest-first key for global residency reconciliation. Invalidated work is
/// always reclaimable, then lower source trust, larger charge and later queue
/// arrival lose in that order. Committing entries are deliberately absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CapacityVictimKey {
    pub(super) valid: bool,
    pub(super) source_strength: SourceTrust,
    pub(super) charge_bytes: usize,
    pub(super) queue_sequence: u64,
    pub(super) hash: Byte32,
}

impl Ord for CapacityVictimKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.valid
            .cmp(&other.valid)
            .then_with(|| self.source_strength.cmp(&other.source_strength))
            .then_with(|| other.charge_bytes.cmp(&self.charge_bytes))
            .then_with(|| other.queue_sequence.cmp(&self.queue_sequence))
            .then_with(|| self.hash.as_slice().cmp(other.hash.as_slice()))
    }
}

impl PartialOrd for CapacityVictimKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<R, U, V> CoordinatorEntry<R, U, V> {
    pub(super) fn state_shape_valid(&self, hash: &Byte32, limits: &CoordinatorLimits) -> bool {
        if self.dependencies.contains(hash)
            || self.dependencies.len() > limits.max_dependencies_per_entry
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
                    && matches!(
                        location,
                        CandidateLocation::Verified | CandidateLocation::Committing
                    )
            }
            EntryState::Raw { .. }
            | EntryState::Unverified { .. }
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
        let candidate_rank = match &self.state {
            EntryState::CandidateVerified {
                candidate,
                location,
                ..
            } => Some(CandidateRank::from_entry(
                hash,
                self.source,
                candidate,
                location,
            )),
            _ => None,
        };
        CoordinatorTicket {
            hash: hash.clone(),
            version: self.version(),
            owner: self.source.queue_owner(),
            priority: self.source.is_proposal(),
            queue_sequence: self.queue_sequence,
            verify_schedule: self.state.verify_schedule(),
            candidate_rank,
        }
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

    pub(super) fn invalidated_cause(&self) -> Option<&Byte32> {
        self.state.invalidated_cause()
    }

    pub(super) fn maintenance_sequence(&self) -> Option<u64> {
        self.state.maintenance_sequence()
    }

    pub(super) fn view(&self) -> CoordinatorView {
        CoordinatorView {
            short_id: self.short_id.clone(),
            location: self.location(),
            peer: self.source.peer(),
            source: self.source,
            charge_bytes: self.charge_bytes,
            dependencies: self.dependencies.clone(),
            version: self.version(),
        }
    }
}

/// Heap key whose maximum is the queue's best ticket. The configured ordering
/// is captured in every key so `Ord` remains total and independent of the
/// containing queue.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RankedTicket {
    ordering: QueueOrdering,
    ticket: CoordinatorTicket,
}

impl Ord for RankedTicket {
    fn cmp(&self, other: &Self) -> Ordering {
        let policy = match (self.ordering, other.ordering) {
            (QueueOrdering::Candidate, QueueOrdering::Candidate) => {
                self.ticket.candidate_rank.cmp(&other.ticket.candidate_rank)
            }
            (QueueOrdering::FeeRate, QueueOrdering::FeeRate) => self
                .ticket
                .priority
                .cmp(&other.ticket.priority)
                .then_with(|| {
                    self.ticket
                        .verify_schedule
                        .fee_rate_per_kb
                        .cmp(&other.ticket.verify_schedule.fee_rate_per_kb)
                }),
            _ => self.ticket.priority.cmp(&other.ticket.priority),
        };
        policy
            // Earlier sequence/hash/version wins; reverse those comparisons
            // because BinaryHeap exposes the maximum key.
            .then_with(|| {
                other
                    .ticket
                    .queue_sequence
                    .cmp(&self.ticket.queue_sequence)
            })
            .then_with(|| {
                other
                    .ticket
                    .hash
                    .as_slice()
                    .cmp(self.ticket.hash.as_slice())
            })
            .then_with(|| {
                other
                    .ticket
                    .version
                    .incarnation
                    .cmp(&self.ticket.version.incarnation)
            })
            .then_with(|| {
                other
                    .ticket
                    .version
                    .revision
                    .cmp(&self.ticket.version.revision)
            })
            .then_with(|| self.ticket.owner.cmp(&other.ticket.owner))
            .then_with(|| {
                self.ticket
                    .verify_schedule
                    .fee_rate_per_kb
                    .cmp(&other.ticket.verify_schedule.fee_rate_per_kb)
            })
            .then_with(|| {
                self.ticket
                    .verify_schedule
                    .is_large_cycle
                    .cmp(&other.ticket.verify_schedule.is_large_cycle)
            })
            .then_with(|| self.ordering.cmp(&other.ordering))
    }
}

impl PartialOrd for RankedTicket {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Published owner-head identity. The ranked ticket alone is insufficient:
/// an owner's best ticket can change A -> B -> A while the first A is still
/// present as a stale global-heap entry. A monotonically increasing generation
/// makes that old publication distinguishable from the current one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RankedHead {
    ranked: RankedTicket,
    generation: u128,
}

impl Ord for RankedHead {
    fn cmp(&self, other: &Self) -> Ordering {
        self.ranked
            .cmp(&other.ranked)
            .then_with(|| self.generation.cmp(&other.generation))
    }
}

impl PartialOrd for RankedHead {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Default)]
struct OwnerTickets {
    small: BinaryHeap<RankedTicket>,
    large: BinaryHeap<RankedTicket>,
    small_live: usize,
    large_live: usize,
    reserved_small: usize,
    reserved_large: usize,
    head_generation: u128,
    published_any: Option<RankedHead>,
    published_small: Option<RankedHead>,
}

impl OwnerTickets {
    fn live_len(&self) -> usize {
        self.small_live
            .checked_add(self.large_live)
            .expect("ticket owner live counts are bounded by coordinator residency")
    }

    fn reserved_len(&self) -> usize {
        self.reserved_small
            .checked_add(self.reserved_large)
            .expect("ticket owner reservations are bounded by coordinator residency")
    }
}

/// Exact live membership plus a two-level priority index. Each source owner
/// contributes at most one current head to the global heap, so a peer at its
/// active-work cap costs one skipped head rather than a scan of every queued
/// transaction from that peer. Small-cycle workers use the parallel small
/// head without walking large-cycle work.
#[derive(Debug)]
pub(crate) struct TicketQueue {
    ordering: QueueOrdering,
    owners: HashMap<QueueOwner, OwnerTickets>,
    heads_any: BinaryHeap<RankedHead>,
    heads_small: BinaryHeap<RankedHead>,
    physical_len: usize,
    pub(super) live: HashSet<CoordinatorTicket>,
    #[cfg(test)]
    selection_probes: usize,
}

impl TicketQueue {
    pub(super) fn new(ordering: QueueOrdering) -> Self {
        Self {
            ordering,
            owners: HashMap::new(),
            heads_any: BinaryHeap::new(),
            heads_small: BinaryHeap::new(),
            physical_len: 0,
            live: HashSet::new(),
            #[cfg(test)]
            selection_probes: 0,
        }
    }

    fn ranked(&self, ticket: CoordinatorTicket) -> RankedTicket {
        RankedTicket {
            ordering: self.ordering,
            ticket,
        }
    }

    pub(super) fn reserve_live(
        &mut self,
        owner: QueueOwner,
        is_large_cycle: bool,
    ) -> Result<(), CoordinatorError> {
        self.live
            .try_reserve(1)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.owners
            .try_reserve(1)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.heads_any
            .try_reserve(1)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        if !is_large_cycle {
            self.heads_small
                .try_reserve(1)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        }
        let owner = self.owners.entry(owner).or_default();
        let (heap, reserved) = if is_large_cycle {
            (&mut owner.large, &mut owner.reserved_large)
        } else {
            (&mut owner.small, &mut owner.reserved_small)
        };
        heap.try_reserve(1)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        *reserved = reserved
            .checked_add(1)
            .ok_or(CoordinatorError::QueueReservationFailed)?;
        Ok(())
    }

    pub(super) fn reserve_many(
        &mut self,
        owners: Vec<QueueOwner>,
        is_large_cycle: bool,
    ) -> Result<(), CoordinatorError> {
        let count = owners.len();
        let mut owner_counts = HashMap::new();
        owner_counts
            .try_reserve(count)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        for owner in owners {
            let owner_count = owner_counts.entry(owner).or_insert(0usize);
            *owner_count = owner_count
                .checked_add(1)
                .ok_or(CoordinatorError::QueueReservationFailed)?;
        }
        self.live
            .try_reserve(count)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.owners
            .try_reserve(owner_counts.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.heads_any
            .try_reserve(count)
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        if !is_large_cycle {
            self.heads_small
                .try_reserve(count)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        }
        for (owner_key, owner_count) in owner_counts {
            let owner = self.owners.entry(owner_key).or_default();
            let (heap, reserved) = if is_large_cycle {
                (&mut owner.large, &mut owner.reserved_large)
            } else {
                (&mut owner.small, &mut owner.reserved_small)
            };
            heap.try_reserve(owner_count)
                .map_err(|_| CoordinatorError::QueueReservationFailed)?;
            *reserved = reserved
                .checked_add(owner_count)
                .ok_or(CoordinatorError::QueueReservationFailed)?;
        }
        Ok(())
    }

    pub(super) fn push_reserved(
        &mut self,
        kind: QueueKind,
        ticket: CoordinatorTicket,
        priority: bool,
    ) -> Result<(), CoordinatorError> {
        if ticket.priority != priority {
            return Err(CoordinatorError::QueueInvariant(kind));
        }
        let owner_key = ticket.owner;
        let is_large = ticket.verify_schedule.is_large_cycle;
        let ranked = self.ranked(ticket);
        if self.live.contains(&ranked.ticket) {
            return Err(CoordinatorError::QueueInvariant(kind));
        }
        let next_physical_len = self
            .physical_len
            .checked_add(1)
            .ok_or(CoordinatorError::QueueInvariant(kind))?;
        let owner = self
            .owners
            .get_mut(&owner_key)
            .ok_or(CoordinatorError::QueueInvariant(kind))?;
        let next_owner_live = if is_large {
            owner.large_live.checked_add(1)
        } else {
            owner.small_live.checked_add(1)
        }
        .ok_or(CoordinatorError::QueueInvariant(kind))?;
        if is_large {
            owner.reserved_large = owner
                .reserved_large
                .checked_sub(1)
                .ok_or(CoordinatorError::QueueInvariant(kind))?;
        } else {
            owner.reserved_small = owner
                .reserved_small
                .checked_sub(1)
                .ok_or(CoordinatorError::QueueInvariant(kind))?;
        }
        let inserted = self.live.insert(ranked.ticket.clone());
        debug_assert!(inserted, "duplicate live ticket was prevalidated");
        if is_large {
            owner.large.push(ranked);
            owner.large_live = next_owner_live;
        } else {
            owner.small.push(ranked);
            owner.small_live = next_owner_live;
        }
        self.physical_len = next_physical_len;
        self.refresh_owner(owner_key);
        self.compact();
        Ok(())
    }

    pub(super) fn peek_eligible<F>(
        &mut self,
        capability: WorkerCapability,
        mut owner_eligible: F,
    ) -> Option<CoordinatorTicket>
    where
        F: FnMut(QueueOwner) -> bool,
    {
        let mut blocked = Vec::new();
        loop {
            let Some(head) = self.peek_current_head(capability) else {
                for head in blocked {
                    self.head_heap_mut(capability).push(head);
                }
                return None;
            };
            if owner_eligible(head.ranked.ticket.owner) {
                for blocked_head in blocked {
                    self.head_heap_mut(capability).push(blocked_head);
                }
                return Some(head.ranked.ticket);
            }
            let skipped = self
                .head_heap_mut(capability)
                .pop()
                .expect("peeked queue head remains present");
            blocked.push(skipped);
        }
    }

    pub(super) fn consume(
        &mut self,
        kind: QueueKind,
        ticket: &CoordinatorTicket,
    ) -> Result<(), CoordinatorError> {
        self.remove_ticket(kind, ticket)
    }

    pub(super) fn remove_live(
        &mut self,
        kind: QueueKind,
        ticket: &CoordinatorTicket,
    ) -> Result<(), CoordinatorError> {
        self.remove_ticket(kind, ticket)
    }

    fn remove_ticket(
        &mut self,
        kind: QueueKind,
        ticket: &CoordinatorTicket,
    ) -> Result<(), CoordinatorError> {
        if !self.live.contains(ticket) {
            return Err(CoordinatorError::QueueInvariant(kind));
        }
        let owner_key = ticket.owner;
        let owner = self
            .owners
            .get_mut(&owner_key)
            .ok_or(CoordinatorError::QueueInvariant(kind))?;
        let live = if ticket.verify_schedule.is_large_cycle {
            &mut owner.large_live
        } else {
            &mut owner.small_live
        };
        let next_live = live
            .checked_sub(1)
            .ok_or(CoordinatorError::QueueInvariant(kind))?;
        let removed = self.live.remove(ticket);
        debug_assert!(removed, "live ticket was prevalidated before removal");
        *live = next_live;
        self.refresh_owner(owner_key);
        self.compact();
        Ok(())
    }

    fn head_heap_mut(&mut self, capability: WorkerCapability) -> &mut BinaryHeap<RankedHead> {
        match capability {
            WorkerCapability::Any => &mut self.heads_any,
            WorkerCapability::SmallCycleOnly => &mut self.heads_small,
        }
    }

    fn published_head(
        &self,
        owner: QueueOwner,
        capability: WorkerCapability,
    ) -> Option<&RankedHead> {
        let owner = self.owners.get(&owner)?;
        match capability {
            WorkerCapability::Any => owner.published_any.as_ref(),
            WorkerCapability::SmallCycleOnly => owner.published_small.as_ref(),
        }
    }

    fn peek_current_head(&mut self, capability: WorkerCapability) -> Option<RankedHead> {
        loop {
            let head = self.head_heap_mut(capability).peek()?.clone();
            #[cfg(test)]
            {
                self.selection_probes = self.selection_probes.saturating_add(1);
            }
            if self.published_head(head.ranked.ticket.owner, capability) == Some(&head) {
                return Some(head);
            }
            self.head_heap_mut(capability).pop();
        }
    }

    fn clean_owner_heap(
        heap: &mut BinaryHeap<RankedTicket>,
        live: &HashSet<CoordinatorTicket>,
        physical_len: &mut usize,
    ) {
        while heap
            .peek()
            .is_some_and(|ranked| !live.contains(&ranked.ticket))
        {
            heap.pop();
            *physical_len = physical_len
                .checked_sub(1)
                .expect("physical ticket count matches owner heaps");
        }
    }

    fn refresh_owner(&mut self, owner_key: QueueOwner) {
        let (next_any, next_small, remove_owner) = {
            let Some(owner) = self.owners.get_mut(&owner_key) else {
                return;
            };
            Self::clean_owner_heap(&mut owner.small, &self.live, &mut self.physical_len);
            Self::clean_owner_heap(&mut owner.large, &self.live, &mut self.physical_len);
            let next_small = owner.small.peek().cloned();
            let next_any = match (next_small.as_ref(), owner.large.peek()) {
                (Some(small), Some(large)) => Some(small.max(large).clone()),
                (Some(small), None) => Some(small.clone()),
                (None, Some(large)) => Some(large.clone()),
                (None, None) => None,
            };
            let any_changed =
                owner.published_any.as_ref().map(|head| &head.ranked) != next_any.as_ref();
            let small_changed =
                owner.published_small.as_ref().map(|head| &head.ranked) != next_small.as_ref();
            if any_changed || small_changed {
                // A head changes at most a small constant number of times per
                // queue transition. Queue tickets exhaust their u64 sequence
                // space before this wider publication generation can wrap.
                owner.head_generation = owner
                    .head_generation
                    .checked_add(1)
                    .expect("owner head generation cannot exhaust before queue sequence");
            }
            if any_changed {
                owner.published_any = next_any.map(|ranked| RankedHead {
                    ranked,
                    generation: owner.head_generation,
                });
            }
            if small_changed {
                owner.published_small = next_small.map(|ranked| RankedHead {
                    ranked,
                    generation: owner.head_generation,
                });
            }
            (
                any_changed.then(|| owner.published_any.clone()).flatten(),
                small_changed
                    .then(|| owner.published_small.clone())
                    .flatten(),
                owner.live_len() == 0 && owner.reserved_len() == 0,
            )
        };
        if let Some(head) = next_any {
            self.heads_any.push(head);
        }
        if let Some(head) = next_small {
            self.heads_small.push(head);
        }
        if remove_owner {
            self.owners.remove(&owner_key);
        }
    }

    pub(super) fn compact(&mut self) {
        if self.physical_len > lazy_ticket_compaction_limit(self.live.len()) {
            for owner in self.owners.values_mut() {
                owner
                    .small
                    .retain(|ranked| self.live.contains(&ranked.ticket));
                owner
                    .large
                    .retain(|ranked| self.live.contains(&ranked.ticket));
            }
            self.physical_len = self.live.len();
        }
        let head_limit = lazy_ticket_compaction_limit(self.owners.len());
        if self.heads_any.len() > head_limit {
            self.heads_any = self
                .owners
                .values()
                .filter_map(|owner| owner.published_any.clone())
                .collect();
        }
        if self.heads_small.len() > head_limit {
            self.heads_small = self
                .owners
                .values()
                .filter_map(|owner| owner.published_small.clone())
                .collect();
        }
    }

    pub(super) fn rebuild_live(
        &mut self,
        kind: QueueKind,
        tickets: Vec<CoordinatorTicket>,
    ) -> Result<(), CoordinatorError> {
        self.clear();
        self.live
            .try_reserve(tickets.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.owners
            .try_reserve(tickets.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.heads_any
            .try_reserve(tickets.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        self.heads_small
            .try_reserve(tickets.len())
            .map_err(|_| CoordinatorError::QueueReservationFailed)?;
        for ticket in tickets {
            self.reserve_live(ticket.owner, ticket.verify_schedule.is_large_cycle)?;
            let priority = ticket.priority;
            self.push_reserved(kind, ticket, priority)?;
        }
        self.compact();
        Ok(())
    }

    pub(super) fn clear(&mut self) {
        self.owners.clear();
        self.heads_any.clear();
        self.heads_small.clear();
        self.physical_len = 0;
        self.live.clear();
        #[cfg(test)]
        {
            self.selection_probes = 0;
        }
    }

    pub(super) fn ordering(&self) -> QueueOrdering {
        self.ordering
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
