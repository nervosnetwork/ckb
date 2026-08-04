//! Production construction and the single physical authority lock domain.
//!
//! This module is the only bridge from process configuration and chain
//! snapshots into the otherwise policy-focused authority kernel.  Keeping the
//! conversion here prevents runtime callers from inventing resource limits,
//! synthetic chain identities, or replacement policy independently.

#[cfg(any(test, feature = "internal"))]
use super::internal::InternalPlugBuildError;
use super::rejection::{
    CommittedPublicReject, DirectTransactionRejection, RecentRejectEncodingError,
    serialized_recent_reject,
};
#[cfg(test)]
use super::state::ValidatedAdmission;
use super::{
    chain::{
        AcceptedValidityTransition, ChainValidationError, DirectAdmissionWork, FinalAdmissionWork,
    },
    chain_boundary::{
        ChainBoundaryError, ChainUpdateCommand, ChainUpdateFailure, CommittedChainUpdate,
    },
    effect::{EffectConfigError, EffectLease, EffectLimits, EffectSettlement},
    ingress::{
        DirectCommand, DirectTransaction, RetainedIngress, RetainedIngressCommit,
        RetainedIngressRejection, direct,
    },
    plan::{
        AuthorityConfigError, AuthorityFault, Backpressure, CandidateDispositionPlan,
        CommittedDelta, ComputeCancellation, ComputeCancellationError, ComputeSettlementFailure,
        DirectAdmissionDisposition, DirectAdmissionEvaluation, EffectCheckoutError,
        EffectCloseError, EffectSettlementFailure, FinalAdmissionDispositionPlan,
        IndependentCandidate, MembershipConfig, MembershipReject, PlanError,
        RetainedAdmissionDisposition, SettlementBatch, SettlementPlan, TxPoolAuthority,
    },
    query::{
        AuthorityPoolSummary, AuthorityQueryError, AuthorityTransactionLookup,
        CompactBlockReadReceipt, FeeEstimateReadReceipt, LiveCellReadReceipt, PersistenceReceipt,
    },
    read::{
        RelayParentRebuildCursor, RelayParentRebuildCut, RelayParentRebuildError,
        RelayParentRebuildPage, RelayParentRebuildScratch,
    },
    resolver::{
        CacheBoundDirectVerification, CacheBoundTxPoolVerification, DirectComputationError,
        DirectResolutionEvaluation, DirectResolutionJob, DirectResolutionPreparation,
        DirectResolutionProbeObservation, DirectVerificationOutcome, DirectVerificationRequest,
        DirectVerifiedCandidate, ResolutionEvaluation, ResolutionExecutionKind, ResolutionJob,
        ResolutionProbeObservation, TxPoolVerificationRequest, VerificationCacheUpdate,
        VerificationJob,
    },
    resources::{
        AcceptedResources, ComputeLimits, ResidencyPolicy, ResourceConfigError, ResourceLimits,
        ResourceVector,
    },
    scheduler::VerifyOrder,
    state::{
        AcceptedAtMillis, AcceptedStatus, ApplySequence, ChainRevision, ChainViewId, RawTxHash,
        RemoteDeadline, WorkPermit,
    },
    template::{AuthorityTemplateInput, TemplateReadError},
    validation::{
        DirectAdmissionValidation, DirectAdmissionValidationOutcome, FinalAdmissionValidation,
        FinalAdmissionValidationError, FinalAdmissionValidationOutcome,
        PreparedFinalAdmissionValidation, verification_environment,
    },
    work::{CheckedOutWork, ComputeSettlement},
};
#[cfg(any(test, feature = "internal"))]
use super::{
    plan::{InternalPlugDisposition, InternalPlugPlanError},
    state::InputEvidenceDisposition,
};
#[cfg(any(test, feature = "internal"))]
use crate::component::entry::TxEntry;
use ckb_app_config::{TxPoolConfig, VerifyOrdering};
use ckb_chain_spec::consensus::Consensus;
use ckb_snapshot::Snapshot;
use ckb_stop_handler::CancellationToken;
use ckb_types::core::FeeRate;
use ckb_types::core::{EntryCompleted, TransactionView};
use ckb_types::packed::{Byte32, OutPoint, ProposalShortId};
use ckb_util::{RwLock, parking_lot::RwLockUpgradableReadGuard};
use ckb_verification::cache::ScriptVerificationRules;
use lru::LruCache;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::{
    num::NonZeroUsize,
    ops::ControlFlow,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, watch};

const PREACCEPTED_ENTRY_BYTES: usize = 768;
const DEPENDENCY_EDGE_BYTES: usize = 160;
// One primary/proposal/source/deadline/scheduler envelope and the maximum
// dependency-index multiplicity used by the retired PrePool charge model.
// These multipliers deliberately compile into the one byte coordinate; the
// independent entry/edge coordinates remain hostile-work ceilings, not extra
// memory allowances.
const FIXED_INDEX_SLOTS_PER_ENTRY: usize = 5;
const INDEX_SLOTS_PER_DEPENDENCY: usize = 7;
const REPLACEMENT_HISTORY_MAX_ENTRIES: usize = 10_000;
const REPLACEMENT_HISTORY_MAX_BYTES: usize = 50_000_000;
const REMOTE_RESOURCE_NUMERATOR: usize = 7;
const REMOTE_RESOURCE_DENOMINATOR: usize = 8;
const PER_PEER_RESOURCE_DIVISOR: usize = 8;
const HISTORY_RESOURCE_DIVISOR: usize = 16;
/// One configured pipeline budget is split into retained ownership and
/// transient compute reservations. The split is a capacity policy, not two
/// independently consumable budgets: their checked sum remains exactly the
/// configured ceiling.
const COMPUTE_RESOURCE_DIVISOR: usize = 4;
const COMMITTED_HASH_CACHE_SIZE: usize = 100_000;
const ADMIN_MAINTENANCE_SLICE: usize = 32;
const MILLIS_PER_HOUR: u64 = 60 * 60 * 1_000;

/// Construction capability for a validator job captured from the paired
/// authority/snapshot store. Its field is private to this module, so no other
/// production caller can assemble a mixed read cut.
pub(super) struct AuthorityStoreCaptureSeal(());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeConfigError {
    PipelineBudgetTooSmall,
    Arithmetic,
    ResourceConfiguration,
    EffectConfiguration,
    AuthorityAllocation,
}

#[derive(Debug)]
pub(crate) enum AuthorityRecentRejectReadError {
    Projection,
    Encoding(RecentRejectEncodingError),
}

impl std::fmt::Display for AuthorityRecentRejectReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Projection => formatter.write_str("pending recent-reject projection mismatch"),
            Self::Encoding(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for AuthorityRecentRejectReadError {}

#[derive(Clone, Copy, Debug)]
struct AuthorityRuntimeConfig {
    resources: ResourceLimits,
    verify_order: VerifyOrder,
    effects: EffectLimits,
    membership: MembershipConfig,
    resolution_policy: ResolutionPolicy,
    expiry_policy: ExpiryPolicy,
    verify_workers: NonZeroUsize,
    transient_compute_permits: NonZeroUsize,
}

/// Wall-clock retention and bounded maintenance policy compiled once from
/// process configuration. Callers may trigger maintenance, but cannot invent
/// a nearby cutoff or silently expand one authority critical section.
#[derive(Clone, Copy, Debug)]
struct ExpiryPolicy {
    accepted_residency_millis: u64,
    remote_slice: NonZeroUsize,
}

/// Immutable resolution policy compiled once at runtime assembly. Jobs never
/// accept caller-supplied fee or cycle thresholds, so the evidence they
/// retain cannot be evaluated under a nearby service configuration.
#[derive(Clone, Copy, Debug)]
struct ResolutionPolicy {
    min_fee_rate: FeeRate,
    large_cycle_threshold: u64,
    direct_max_resident_bytes: usize,
    direct_max_edges: usize,
}

impl ResolutionPolicy {
    fn from_runtime(
        config: &TxPoolConfig,
        direct_max_resident_bytes: usize,
        direct_max_edges: usize,
    ) -> Self {
        Self {
            min_fee_rate: config.min_fee_rate,
            large_cycle_threshold: config.max_tx_verify_cycles,
            direct_max_resident_bytes,
            direct_max_edges,
        }
    }
}

impl AuthorityRuntimeConfig {
    fn from_runtime(
        config: &TxPoolConfig,
        consensus: &Consensus,
    ) -> Result<Self, RuntimeConfigError> {
        let entry_metadata_bytes = PREACCEPTED_ENTRY_BYTES
            .checked_add(
                DEPENDENCY_EDGE_BYTES
                    .checked_mul(FIXED_INDEX_SLOTS_PER_ENTRY)
                    .ok_or(RuntimeConfigError::Arithmetic)?,
            )
            .ok_or(RuntimeConfigError::Arithmetic)?;
        let edge_metadata_bytes = DEPENDENCY_EDGE_BYTES
            .checked_mul(INDEX_SLOTS_PER_DEPENDENCY)
            .ok_or(RuntimeConfigError::Arithmetic)?;
        let pipeline_bytes = config.tx_pipeline_resident_size_budget();
        if pipeline_bytes <= entry_metadata_bytes {
            return Err(RuntimeConfigError::PipelineBudgetTooSmall);
        }

        let retained_entries = pipeline_bytes / PREACCEPTED_ENTRY_BYTES;
        let pipeline_edges = pipeline_bytes / DEPENDENCY_EDGE_BYTES;
        let verify_workers = NonZeroUsize::new(config.max_tx_verify_workers.max(1))
            .ok_or(RuntimeConfigError::Arithmetic)?;
        let accepted_residency_millis = u64::from(config.expiry_hours)
            .checked_mul(MILLIS_PER_HOUR)
            .ok_or(RuntimeConfigError::Arithmetic)?;
        let remote_slice =
            NonZeroUsize::new(ADMIN_MAINTENANCE_SLICE).ok_or(RuntimeConfigError::Arithmetic)?;
        // One ordered resolver preserves dependency fairness. Verification
        // workers opportunistically take ResolveThenVerify when their own
        // verify lane is empty, so no second resolve-worker population or VM
        // concurrency budget exists.
        let active_work = verify_workers
            .get()
            .checked_add(1)
            .ok_or(RuntimeConfigError::Arithmetic)?;
        if active_work > Semaphore::MAX_PERMITS {
            return Err(RuntimeConfigError::ResourceConfiguration);
        }
        let transient_compute_permits =
            NonZeroUsize::new(active_work).ok_or(RuntimeConfigError::Arithmetic)?;

        let compute_partition_bytes = pipeline_bytes / COMPUTE_RESOURCE_DIVISOR;
        let compute_bytes_per_work = compute_partition_bytes
            .checked_div(active_work)
            .filter(|bytes| *bytes != 0)
            .ok_or(RuntimeConfigError::PipelineBudgetTooSmall)?;
        // A compute capability must be able to settle at least one
        // authority-owned entry. Reject an unusable worker/budget ratio at
        // assembly instead of turning every legal admission into a runtime
        // resource rejection.
        if compute_bytes_per_work <= entry_metadata_bytes {
            return Err(RuntimeConfigError::PipelineBudgetTooSmall);
        }
        let compute_bytes = compute_bytes_per_work
            .checked_mul(active_work)
            .ok_or(RuntimeConfigError::Arithmetic)?;
        let retained_bytes = pipeline_bytes
            .checked_sub(compute_bytes)
            .ok_or(RuntimeConfigError::Arithmetic)?;

        let desired_expanded_edges = (consensus.max_block_bytes() as usize)
            .checked_div(32)
            .and_then(|count| count.checked_add(1))
            .ok_or(RuntimeConfigError::Arithmetic)?
            .min(pipeline_edges);
        let compute_edge_partition = pipeline_edges / COMPUTE_RESOURCE_DIVISOR;
        let compute_edges_per_work = compute_edge_partition
            .checked_div(active_work)
            .filter(|edges| *edges != 0)
            .ok_or(RuntimeConfigError::PipelineBudgetTooSmall)?
            .min(desired_expanded_edges);
        let compute_edges = compute_edges_per_work
            .checked_mul(active_work)
            .ok_or(RuntimeConfigError::Arithmetic)?;
        let retained_edges = pipeline_edges
            .checked_sub(compute_edges)
            .ok_or(RuntimeConfigError::Arithmetic)?;

        let preaccepted = ResourceVector::new(
            retained_entries,
            retained_bytes,
            retained_edges,
            active_work,
        )
        .with_compute_capacity(compute_bytes, compute_edges)
        .ok_or(RuntimeConfigError::Arithmetic)?;
        let remote_active_work = checked_fraction(
            active_work,
            REMOTE_RESOURCE_NUMERATOR,
            REMOTE_RESOURCE_DENOMINATOR,
        )?
        .max(1);
        let remote_compute_bytes = compute_bytes_per_work
            .checked_mul(remote_active_work)
            .ok_or(RuntimeConfigError::Arithmetic)?;
        let remote_compute_edges = compute_edges_per_work
            .checked_mul(remote_active_work)
            .ok_or(RuntimeConfigError::Arithmetic)?;
        let remote = ResourceVector::new(
            checked_fraction(
                retained_entries,
                REMOTE_RESOURCE_NUMERATOR,
                REMOTE_RESOURCE_DENOMINATOR,
            )?,
            checked_fraction(
                retained_bytes,
                REMOTE_RESOURCE_NUMERATOR,
                REMOTE_RESOURCE_DENOMINATOR,
            )?,
            checked_fraction(
                retained_edges,
                REMOTE_RESOURCE_NUMERATOR,
                REMOTE_RESOURCE_DENOMINATOR,
            )?,
            remote_active_work,
        )
        .with_compute_capacity(remote_compute_bytes, remote_compute_edges)
        .ok_or(RuntimeConfigError::Arithmetic)?;
        let per_peer_active_work = (remote.active_work / 4).max(1);
        let per_peer = ResourceVector::new(
            (remote.entries / PER_PEER_RESOURCE_DIVISOR).max(1),
            (remote.bytes / PER_PEER_RESOURCE_DIVISOR).max(1),
            (remote.edges / PER_PEER_RESOURCE_DIVISOR).max(1),
            per_peer_active_work,
        )
        .with_compute_capacity(
            compute_bytes_per_work
                .checked_mul(per_peer_active_work)
                .ok_or(RuntimeConfigError::Arithmetic)?,
            compute_edges_per_work
                .checked_mul(per_peer_active_work)
                .ok_or(RuntimeConfigError::Arithmetic)?,
        )
        .ok_or(RuntimeConfigError::Arithmetic)?;

        let compute = ComputeLimits::new(
            compute_bytes_per_work,
            compute_bytes_per_work,
            compute_edges_per_work,
        );
        let accepted_resident = config.tx_pool_resident_size_budget();
        let accepted = AcceptedResources::new(
            accepted_resident / 1_024,
            config.max_tx_pool_size,
            accepted_resident,
            u64::MAX,
        );
        let residency = ResidencyPolicy::production(
            NonZeroUsize::new(entry_metadata_bytes)
                .ok_or(RuntimeConfigError::ResourceConfiguration)?,
            NonZeroUsize::new(edge_metadata_bytes)
                .ok_or(RuntimeConfigError::ResourceConfiguration)?,
        );
        let resources = ResourceLimits::with_residency_policy(
            preaccepted,
            remote,
            per_peer,
            accepted,
            compute,
            residency,
        )
        .map_err(runtime_resource_config_error)?
        .with_replacement_history_limit(ResourceVector::new(
            REPLACEMENT_HISTORY_MAX_ENTRIES
                .min((retained_entries / HISTORY_RESOURCE_DIVISOR).max(1)),
            REPLACEMENT_HISTORY_MAX_BYTES.min((retained_bytes / HISTORY_RESOURCE_DIVISOR).max(1)),
            (retained_edges / HISTORY_RESOURCE_DIVISOR).max(1),
            0,
        ))
        .map_err(runtime_resource_config_error)?;

        let effects = EffectLimits::production(
            config.max_tx_pool_size,
            retained_bytes,
            consensus.max_block_bytes() as usize,
            compute_edges_per_work,
        )
        .map_err(|error| match error {
            EffectConfigError::Arithmetic => RuntimeConfigError::Arithmetic,
            EffectConfigError::EmptyRemoteRegion
            | EffectConfigError::EmptyBatchBound
            | EffectConfigError::IndivisibleBatch
            | EffectConfigError::Allocation => RuntimeConfigError::EffectConfiguration,
        })?;

        let verify_order = match config.verify_ordering {
            VerifyOrdering::ArrivalTime => VerifyOrder::Arrival,
            VerifyOrdering::FeeRate => VerifyOrder::FeeRate,
        };
        let replacement_rate =
            (config.min_rbf_rate > config.min_fee_rate).then_some(config.min_rbf_rate);
        let membership = MembershipConfig::from_runtime(
            config.max_ancestors_count,
            crate::constants::MAX_POOL_MUTATION_CANDIDATES,
            replacement_rate,
        );

        Ok(Self {
            resources,
            verify_order,
            effects,
            membership,
            resolution_policy: ResolutionPolicy::from_runtime(
                config,
                compute_bytes_per_work,
                compute_edges_per_work,
            ),
            expiry_policy: ExpiryPolicy {
                accepted_residency_millis,
                remote_slice,
            },
            verify_workers,
            transient_compute_permits,
        })
    }
}

fn checked_fraction(
    value: usize,
    numerator: usize,
    denominator: usize,
) -> Result<usize, RuntimeConfigError> {
    value
        .checked_mul(numerator)
        .and_then(|scaled| scaled.checked_div(denominator))
        .ok_or(RuntimeConfigError::Arithmetic)
}

/// Preserve the source error algebra at the runtime assembly boundary.  In
/// particular, checked-capacity overflow is not a malformed resource policy.
/// An exhaustive match also makes a future resource error fail compilation
/// here instead of being silently folded into the wrong startup diagnosis.
fn runtime_resource_config_error(error: ResourceConfigError) -> RuntimeConfigError {
    match error {
        ResourceConfigError::TransientComputeOverflow => RuntimeConfigError::Arithmetic,
        ResourceConfigError::LimitHierarchy
        | ResourceConfigError::MissingComputeCapacity
        | ResourceConfigError::NonMonotonicComputeEnvelope => {
            RuntimeConfigError::ResourceConfiguration
        }
    }
}

/// Authority construction currently allocates only the already-validated
/// effect queue, but keep this conversion exhaustive so moving validation
/// ownership cannot erase the distinction between arithmetic, configuration,
/// and allocation failures again.
fn runtime_authority_config_error(error: AuthorityConfigError) -> RuntimeConfigError {
    match error {
        AuthorityConfigError::Effect(EffectConfigError::Arithmetic) => {
            RuntimeConfigError::Arithmetic
        }
        AuthorityConfigError::Effect(
            EffectConfigError::EmptyRemoteRegion
            | EffectConfigError::EmptyBatchBound
            | EffectConfigError::IndivisibleBatch,
        ) => RuntimeConfigError::EffectConfiguration,
        AuthorityConfigError::Effect(EffectConfigError::Allocation) => {
            RuntimeConfigError::AuthorityAllocation
        }
    }
}

/// The single physical lock domain of the production tx-pool.
///
/// `snapshot` is chain evidence, not a second transaction owner.  It is kept
/// beside the kernel so a caller cannot publish a new snapshot under an old
/// `ChainViewId`, or vice versa. The compact-block cache is non-authoritative
/// chain-administration metadata.
pub(crate) struct AuthorityStore {
    authority: TxPoolAuthority,
    snapshot: Arc<Snapshot>,
    committed_txs_hash_cache: LruCache<ProposalShortId, Byte32>,
}

/// Read-only capability for rebuilding the relayer's missing-parent level.
///
/// The relayer receives this projection reader rather than an
/// [`AuthorityRuntime`], so it cannot acquire admission, administration, or
/// settlement authority. The store remains the sole owner and every page is
/// bound to an exact source cut.
pub(in crate::authority) struct AuthorityRelayParentReader {
    store: Arc<RwLock<AuthorityStore>>,
}

/// Lossy wake hints around the one authoritative scheduler. A hint carries no
/// queue state: every waiter first attempts capability-aware checkout under
/// the store guard, and subscribes before that attempt so a concurrent Apply
/// cannot be missed.
struct AuthoritySignals {
    mutation: Arc<Notify>,
    effect_publisher_running: AtomicBool,
}

impl AuthoritySignals {
    fn new() -> Self {
        Self {
            mutation: Arc::new(Notify::new()),
            effect_publisher_running: AtomicBool::new(false),
        }
    }

    fn publish_mutation(&self) {
        // Publication occurs only after the authority guard has opened and
        // retirement carriers have been destroyed. One shared hint avoids a
        // hand-maintained transition-to-signal table and duplicate wake
        // publication; each consumer reads its own authoritative level.
        self.mutation.notify_waiters();
    }
}

/// Move-only claim for the sole consumer of the authority effect sequence.
/// The effect log already serializes checkout; this claim additionally makes
/// a second idle consumer fail immediately instead of waiting forever behind
/// the first consumer's active lease.
pub(in crate::authority) struct AuthorityEffectPublisherClaim {
    signals: Arc<AuthoritySignals>,
}

impl Drop for AuthorityEffectPublisherClaim {
    fn drop(&mut self) {
        self.signals
            .effect_publisher_running
            .store(false, Ordering::Release);
    }
}

/// Narrow production shell around the single UAK store lock.
///
/// No method accepts a generic mutation closure. Plan/Apply, snapshot pairing,
/// retirement and wake publication stay centralized so a service caller
/// cannot choose a second ordering or forget one post-commit edge.
#[derive(Clone)]
pub(crate) struct AuthorityRuntime {
    store: Arc<RwLock<AuthorityStore>>,
    signals: Arc<AuthoritySignals>,
    resolution_policy: ResolutionPolicy,
    expiry_policy: ExpiryPolicy,
    verify_workers: NonZeroUsize,
    transient_compute: ComputeGate,
    #[cfg(test)]
    template_captures: Arc<AtomicUsize>,
}

/// Private count-only execution gate. No close operation is exposed: service
/// shutdown cancels waiters, so Tokio's semaphore-close state cannot become a
/// third runtime outcome or a background-lane quarantine protocol.
#[derive(Clone)]
struct ComputeGate {
    permits: Arc<Semaphore>,
}

impl ComputeGate {
    fn new(permits: NonZeroUsize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(permits.get())),
        }
    }

    async fn acquire(&self, cancel: &CancellationToken) -> Option<OwnedSemaphorePermit> {
        let acquire = Arc::clone(&self.permits).acquire_owned();
        tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            result = acquire => {
                // `ComputeGate` exposes no close capability. An acquire error
                // therefore has the same only legal disposition as topology
                // cancellation and cannot be triggered by transaction input.
                let permit = result.ok()?;
                if cancel.is_cancelled() {
                    drop(permit);
                    None
                } else {
                    Some(permit)
                }
            }
        }
    }

    #[cfg(test)]
    fn try_acquire(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.permits).try_acquire_owned().ok()
    }

    #[cfg(test)]
    fn available_permits(&self) -> usize {
        self.permits.available_permits()
    }
}

/// One slot in the tx-pool-only transient compute partition. It carries no
/// transaction identity or membership right. The move-only value is acquired
/// before authority checkout or direct capture and retained until the exact
/// computation has settled or become stale.
#[derive(Debug)]
#[must_use = "a transient compute permit must guard one complete execution"]
pub(in crate::authority) struct AuthorityComputeExecutionPermit {
    _permit: OwnedSemaphorePermit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use = "maintenance progress determines whether another bounded step is useful"]
pub(super) enum AuthorityMaintenanceOutcome {
    Idle,
    Applied { owners: usize },
}

/// Closed progress contract shared by authority-owned background drivers.
///
/// A driver may retry only after an observed stale cut, allocator recovery or
/// effect capacity publication. Every other producer outcome is translated at
/// the authority boundary into a structural fault, so adding a broad
/// `PlanError` variant cannot silently create a retry loop in a worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) enum AuthorityDriverError {
    Stale,
    Allocation,
    EffectCapacity,
    LifecycleClosed,
    Fault(AuthorityFault),
}

/// Closed result for synchronous administrative commands planned entirely
/// under the authority write guard. These commands carry no lock-external OCC
/// evidence, so stale or membership-only planner outcomes are structural;
/// allocator and effect-capacity pressure remain retryable service outcomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AuthorityAdministrationError {
    Allocation,
    EffectCapacity,
    LifecycleClosed,
    Fault(AuthorityFault),
}

impl AuthorityAdministrationError {
    fn from_plan(error: PlanError) -> Self {
        match error {
            PlanError::Backpressure(Backpressure::Allocation) => Self::Allocation,
            PlanError::Backpressure(Backpressure::EffectCapacity) => Self::EffectCapacity,
            PlanError::EffectClosed => Self::LifecycleClosed,
            PlanError::Fault(fault) => Self::Fault(fault),
            PlanError::Backpressure(Backpressure::ProposalCollision) => {
                Self::Fault(AuthorityFault::IndexProjection)
            }
            PlanError::Backpressure(
                Backpressure::TotalResources
                | Backpressure::RemoteResources
                | Backpressure::PeerResources
                | Backpressure::AcceptedResources,
            ) => Self::Fault(AuthorityFault::ResourceProjection),
            PlanError::Backpressure(
                Backpressure::ComputeResources | Backpressure::GenerationReplacement,
            ) => Self::Fault(AuthorityFault::SchedulerProjection),
            PlanError::Duplicate
            | PlanError::PayloadVariant
            | PlanError::Membership(_)
            | PlanError::IngressRevoked(_)
            | PlanError::Stale(_) => Self::Fault(AuthorityFault::MembershipProjection),
        }
    }
}

impl AuthorityDriverError {
    /// Translate a Plan that consumes lock-external OCC evidence. A stale
    /// owner or view is an ordinary concurrent outcome only at this boundary.
    fn from_ready_plan(error: PlanError) -> Self {
        match error {
            PlanError::Stale(_) => Self::Stale,
            PlanError::Backpressure(Backpressure::Allocation) => Self::Allocation,
            PlanError::Backpressure(Backpressure::EffectCapacity) => Self::EffectCapacity,
            PlanError::EffectClosed => Self::LifecycleClosed,
            PlanError::Fault(fault) => Self::Fault(fault),
            PlanError::Backpressure(Backpressure::ProposalCollision) => {
                Self::Fault(AuthorityFault::IndexProjection)
            }
            PlanError::Backpressure(
                Backpressure::TotalResources
                | Backpressure::RemoteResources
                | Backpressure::PeerResources
                | Backpressure::AcceptedResources,
            ) => Self::Fault(AuthorityFault::ResourceProjection),
            PlanError::Backpressure(
                Backpressure::ComputeResources | Backpressure::GenerationReplacement,
            ) => Self::Fault(AuthorityFault::SchedulerProjection),
            PlanError::Duplicate
            | PlanError::PayloadVariant
            | PlanError::Membership(_)
            | PlanError::IngressRevoked(_) => Self::Fault(AuthorityFault::MembershipProjection),
        }
    }

    /// Translate maintenance planned entirely under one authority write
    /// guard. No external receipt can race this Plan, so a stale owner here
    /// proves a producer/projection contradiction instead of useful progress.
    fn from_maintenance_plan(error: PlanError) -> Self {
        match error {
            PlanError::Stale(_) => Self::Fault(AuthorityFault::MembershipProjection),
            error => Self::from_ready_plan(error),
        }
    }

    fn from_validation_defect(error: FinalAdmissionValidationError) -> Self {
        match error {
            FinalAdmissionValidationError::StaleView => {
                Self::Fault(AuthorityFault::MembershipProjection)
            }
            FinalAdmissionValidationError::Allocation => Self::Allocation,
            FinalAdmissionValidationError::Arithmetic => {
                Self::Fault(AuthorityFault::ResourceProjection)
            }
            FinalAdmissionValidationError::MissingChainLocation(_)
            | FinalAdmissionValidationError::CellContentMismatch(_)
            | FinalAdmissionValidationError::ContextReceipt => {
                Self::Fault(AuthorityFault::MembershipProjection)
            }
        }
    }

    /// The scheduler selection and first Ready capture share one immutable
    /// authority read cut. Stale ownership at this boundary is therefore a
    /// scheduler projection defect, not lock-external OCC progress.
    fn from_initial_ready_capture(error: FinalAdmissionCaptureError) -> Self {
        match error {
            FinalAdmissionCaptureError::Plan(PlanError::Stale(_)) => {
                Self::Fault(AuthorityFault::SchedulerProjection)
            }
            FinalAdmissionCaptureError::Plan(error) => Self::from_ready_plan(error),
            FinalAdmissionCaptureError::Validation(error) => Self::from_validation_defect(error),
            FinalAdmissionCaptureError::Allocation => Self::Allocation,
        }
    }

    /// Preparation owns the snapshot and Ready work captured from the same
    /// cut. A stale view here cannot be caused by a concurrent writer.
    fn from_ready_preparation(error: FinalAdmissionCaptureError) -> Self {
        match error {
            FinalAdmissionCaptureError::Validation(error) => Self::from_validation_defect(error),
            FinalAdmissionCaptureError::Allocation => Self::Allocation,
            FinalAdmissionCaptureError::Plan(_) => Self::Fault(AuthorityFault::SchedulerProjection),
        }
    }

    /// The second Ready read deliberately crosses a lock-external preparation
    /// window, so exact owner/view staleness is an ordinary OCC miss here.
    fn from_ready_recheck(error: FinalAdmissionCaptureError) -> Self {
        match error {
            FinalAdmissionCaptureError::Plan(error) => Self::from_ready_plan(error),
            FinalAdmissionCaptureError::Validation(FinalAdmissionValidationError::StaleView) => {
                Self::Stale
            }
            FinalAdmissionCaptureError::Validation(error) => Self::from_validation_defect(error),
            FinalAdmissionCaptureError::Allocation => Self::Allocation,
        }
    }

    fn from_ready_validation(error: ReadyValidationError) -> Self {
        match error {
            ReadyValidationError::Candidate(error) => Self::from_validation_defect(error),
            ReadyValidationError::Allocation => Self::Allocation,
        }
    }
}

#[derive(Debug)]
#[must_use = "checked-out authority work must be executed and settled"]
pub(crate) struct AuthorityComputeJob {
    inner: AuthorityComputeKind,
    execution: AuthorityComputeExecutionPermit,
}

#[derive(Debug)]
enum AuthorityComputeKind {
    Resolution(ResolutionJob),
    Verification(VerificationJob),
}

impl AuthorityComputeJob {
    fn retry(self) -> AuthorityComputeSettlement {
        let settlement = match self.inner {
            AuthorityComputeKind::Resolution(job) => job.retry(),
            AuthorityComputeKind::Verification(job) => job.retry(),
        };
        AuthorityComputeSettlement {
            settlement,
            execution: self.execution,
        }
    }

    #[cfg(test)]
    pub(super) fn retry_for_foundation(self) -> AuthorityComputeSettlement {
        self.retry()
    }
}

#[derive(Debug)]
#[must_use = "a retained compute settlement must preserve its execution permit"]
pub(in crate::authority) struct AuthorityComputeSettlement {
    settlement: ComputeSettlement,
    execution: AuthorityComputeExecutionPermit,
}

#[derive(Debug)]
#[must_use = "verification work must bind the exact cache lookup and execute"]
pub(in crate::authority) struct AuthorityVerificationRequest {
    request: TxPoolVerificationRequest,
    execution: AuthorityComputeExecutionPermit,
}

#[derive(Debug)]
#[must_use = "cache-bound verification still owns its transient execution slot"]
pub(in crate::authority) struct AuthorityCacheBoundVerification {
    request: CacheBoundTxPoolVerification,
    execution: AuthorityComputeExecutionPermit,
}

impl AuthorityVerificationRequest {
    pub(in crate::authority) fn bind_cache(
        self,
        cache: &ckb_verification::cache::TxVerificationCache,
    ) -> AuthorityCacheBoundVerification {
        AuthorityCacheBoundVerification {
            request: self.request.bind_cache(cache),
            execution: self.execution,
        }
    }

    fn retry(self) -> AuthorityComputeSettlement {
        AuthorityComputeSettlement {
            settlement: self.request.retry(),
            execution: self.execution,
        }
    }
}

#[derive(Debug)]
pub(in crate::authority) enum AuthorityComputeError {
    Allocation,
    ComputeCapacity,
    EffectCapacity,
    LifecycleClosed,
    Fault(AuthorityFault),
    Resolution(ResolutionExecutionKind),
}

impl AuthorityComputeError {
    /// Checkout selects and consumes its scheduler ticket under one authority
    /// write guard. Unlike Ready validation, it has no lock-external OCC cut;
    /// a stale ticket therefore cannot be retried as ordinary progress.
    fn from_checkout_plan(error: PlanError) -> Self {
        match error {
            PlanError::Stale(_) => Self::Fault(AuthorityFault::SchedulerProjection),
            error => match AuthorityDriverError::from_ready_plan(error) {
                AuthorityDriverError::Stale => Self::Fault(AuthorityFault::SchedulerProjection),
                AuthorityDriverError::Allocation => Self::Allocation,
                AuthorityDriverError::EffectCapacity => Self::EffectCapacity,
                AuthorityDriverError::LifecycleClosed => Self::LifecycleClosed,
                AuthorityDriverError::Fault(fault) => Self::Fault(fault),
            },
        }
    }
}

/// Semantic result whose exact linear capability was being committed when
/// settlement encountered backpressure or a structural error. The worker
/// retains this origin while it drains the capability, so an invalid receipt
/// cannot be hidden by a temporary full effect region.
#[derive(Clone, Copy, Debug)]
pub(in crate::authority) enum SettlementOrigin {
    Capture(ResolutionExecutionKind),
    Resolution(ResolutionExecutionKind),
    Completion,
}

/// A recoverable settlement interruption is normal capability flow, not a
/// structural runtime error. Keeping it on the `ControlFlow::Break` path
/// prevents every unrelated runtime `Result` from carrying this large linear
/// value while retaining it allocation-free during effect backpressure.
#[derive(Debug)]
#[must_use = "an interrupted settlement still owns its active compute capability"]
pub(in crate::authority) struct AuthorityPendingSettlement {
    failure: ComputeSettlementFailure,
    origin: SettlementOrigin,
    execution: AuthorityComputeExecutionPermit,
}

impl AuthorityPendingSettlement {
    pub(in crate::authority) fn new(
        failure: ComputeSettlementFailure,
        origin: SettlementOrigin,
        execution: AuthorityComputeExecutionPermit,
    ) -> Self {
        Self {
            failure,
            origin,
            execution,
        }
    }

    pub(in crate::authority) fn into_parts(
        self,
    ) -> (
        ComputeSettlementFailure,
        SettlementOrigin,
        AuthorityComputeExecutionPermit,
    ) {
        (self.failure, self.origin, self.execution)
    }

    #[cfg(test)]
    pub(super) fn recovery(&self) -> &super::plan::ComputeSettlementRecovery {
        self.failure.recovery()
    }
}

#[derive(Debug)]
pub(in crate::authority) enum ReadyValidationError {
    Candidate(FinalAdmissionValidationError),
    Allocation,
}

#[derive(Debug)]
#[must_use = "resolved authority work either settled or still owns verification"]
pub(in crate::authority) enum AuthorityComputeOutcome {
    Settled,
    Verification(AuthorityVerificationRequest),
}

#[derive(Debug)]
#[must_use = "direct computation must continue verification or return its exact rejection"]
pub(super) enum AuthorityDirectResolutionOutcome {
    Verification(AuthorityDirectVerificationRequest),
    Rejected(AuthorityDirectRejection),
}

#[derive(Debug)]
#[must_use = "direct verification must bind its exact cache evidence"]
pub(super) struct AuthorityDirectVerificationRequest {
    request: DirectVerificationRequest,
    execution: AuthorityComputeExecutionPermit,
}

#[derive(Debug)]
#[must_use = "cache-bound direct verification still owns its execution slot"]
pub(super) struct AuthorityCacheBoundDirectVerification {
    request: CacheBoundDirectVerification,
    execution: AuthorityComputeExecutionPermit,
}

impl AuthorityDirectVerificationRequest {
    pub(super) fn bind_cache(
        self,
        cache: &ckb_verification::cache::TxVerificationCache,
    ) -> AuthorityCacheBoundDirectVerification {
        AuthorityCacheBoundDirectVerification {
            request: self.request.bind_cache(cache),
            execution: self.execution,
        }
    }
}

#[derive(Debug)]
#[must_use = "a direct rejection must be settled under its execution slot"]
pub(super) struct AuthorityDirectRejection {
    rejection: DirectTransactionRejection,
    execution: AuthorityComputeExecutionPermit,
}

impl AuthorityDirectRejection {
    #[cfg(test)]
    pub(super) fn reason(&self) -> &CommittedPublicReject {
        self.rejection.reason()
    }
}

#[derive(Debug)]
#[must_use = "direct verification output must reach its source-sealed settlement"]
pub(super) enum AuthorityDirectVerificationOutcome {
    Candidate(AuthorityDirectVerifiedCandidate),
    Rejected(AuthorityDirectRejection),
}

#[derive(Debug)]
#[must_use = "verified direct work must settle while retaining its execution slot"]
pub(super) struct AuthorityDirectVerifiedCandidate {
    candidate: DirectVerifiedCandidate,
    execution: AuthorityComputeExecutionPermit,
}

#[cfg(test)]
impl AuthorityDirectVerifiedCandidate {
    pub(super) fn with_cache_update_for_foundation(
        mut self,
        key: ckb_verification::cache::TxVerificationCacheKey,
        completed: ckb_verification::cache::Completed,
    ) -> Self {
        self.candidate = self
            .candidate
            .with_cache_update_for_foundation(key, completed);
        self
    }
}

#[derive(Debug)]
pub(super) enum DirectAdmissionRejectionKind {
    Validation(CommittedPublicReject),
    Membership(MembershipReject),
}

#[derive(Debug)]
#[must_use = "Local disposition is the committed synchronous outcome or an exact retry"]
pub(super) enum AuthorityLocalAdmissionOutcome {
    Accepted(EntryCompleted),
    Duplicate(RawTxHash),
    Rejected(DirectAdmissionRejectionKind),
    Retry(Arc<TransactionView>),
}

#[derive(Debug, PartialEq, Eq)]
#[must_use = "TestAccept evaluation is read-only and must be returned or retried"]
pub(super) enum AuthorityTestAcceptOutcome {
    Accepted(EntryCompleted),
    Duplicate(RawTxHash),
    RejectedValidation(CommittedPublicReject),
    RejectedMembership(MembershipReject),
    Retry,
}

#[cfg(any(test, feature = "internal"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AuthorityInternalPlugOutcome {
    Inserted,
    Duplicate,
}

/// Closed failure surface for the feature-internal synthetic admission hook.
/// Legal fixture rejection never becomes a generation fault, while stale OCC
/// evidence remains an ordinary retry owned by the service adapter.
#[cfg(any(test, feature = "internal"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AuthorityInternalPlugError {
    Stale,
    WouldDisplace,
    Rejected(MembershipReject),
    Capacity,
    ResourceUnavailable,
    ProposalCollision,
    LifecycleClosed,
    Fault(AuthorityFault),
}

#[cfg(any(test, feature = "internal"))]
impl AuthorityInternalPlugError {
    fn from_build(error: InternalPlugBuildError) -> Self {
        match error {
            InternalPlugBuildError::Evidence(error) => match error.disposition() {
                InputEvidenceDisposition::ResourceDenied => Self::Capacity,
                InputEvidenceDisposition::ResourceUnavailable => Self::ResourceUnavailable,
                InputEvidenceDisposition::MalformedTransaction
                | InputEvidenceDisposition::Structural => {
                    Self::Fault(AuthorityFault::MembershipProjection)
                }
            },
            InternalPlugBuildError::Allocation => Self::ResourceUnavailable,
            InternalPlugBuildError::Context => Self::Fault(AuthorityFault::MembershipProjection),
        }
    }

    fn from_plan(error: InternalPlugPlanError) -> Self {
        match error {
            InternalPlugPlanError::WouldDisplace => Self::WouldDisplace,
            InternalPlugPlanError::Plan(error) => match error {
                PlanError::Stale(_) => Self::Stale,
                PlanError::Membership(reason) => Self::Rejected(reason),
                PlanError::Backpressure(Backpressure::Allocation) => Self::ResourceUnavailable,
                PlanError::Backpressure(
                    Backpressure::TotalResources | Backpressure::AcceptedResources,
                ) => Self::Capacity,
                PlanError::Backpressure(Backpressure::ProposalCollision) => Self::ProposalCollision,
                PlanError::EffectClosed => Self::LifecycleClosed,
                PlanError::Fault(fault) => Self::Fault(fault),
                PlanError::Backpressure(Backpressure::EffectCapacity) => {
                    Self::Fault(AuthorityFault::EffectProjection)
                }
                PlanError::Backpressure(
                    Backpressure::RemoteResources | Backpressure::PeerResources,
                ) => Self::Fault(AuthorityFault::ResourceProjection),
                PlanError::Backpressure(
                    Backpressure::ComputeResources | Backpressure::GenerationReplacement,
                ) => Self::Fault(AuthorityFault::SchedulerProjection),
                PlanError::Duplicate | PlanError::PayloadVariant | PlanError::IngressRevoked(_) => {
                    Self::Fault(AuthorityFault::MembershipProjection)
                }
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AuthorityDirectAdmissionError {
    Stale,
    ResourceUnavailable,
    EffectCapacity,
    ProposalCollision,
    LifecycleClosed,
    Fault(AuthorityFault),
}

impl AuthorityDirectAdmissionError {
    fn from_validation(error: FinalAdmissionValidationError) -> Self {
        match error {
            FinalAdmissionValidationError::StaleView => Self::Stale,
            FinalAdmissionValidationError::Allocation => Self::ResourceUnavailable,
            FinalAdmissionValidationError::Arithmetic => {
                Self::Fault(AuthorityFault::ResourceProjection)
            }
            FinalAdmissionValidationError::MissingChainLocation(_)
            | FinalAdmissionValidationError::CellContentMismatch(_)
            | FinalAdmissionValidationError::ContextReceipt => {
                Self::Fault(AuthorityFault::MembershipProjection)
            }
        }
    }

    fn from_plan(error: PlanError) -> Self {
        match error {
            PlanError::Stale(_) => Self::Stale,
            PlanError::Backpressure(Backpressure::Allocation) => Self::ResourceUnavailable,
            PlanError::Backpressure(Backpressure::EffectCapacity) => Self::EffectCapacity,
            PlanError::Backpressure(Backpressure::ProposalCollision) => Self::ProposalCollision,
            PlanError::EffectClosed => Self::LifecycleClosed,
            PlanError::Fault(fault) => Self::Fault(fault),
            PlanError::Backpressure(
                Backpressure::TotalResources
                | Backpressure::RemoteResources
                | Backpressure::PeerResources
                | Backpressure::AcceptedResources,
            ) => Self::Fault(AuthorityFault::ResourceProjection),
            PlanError::Backpressure(
                Backpressure::ComputeResources | Backpressure::GenerationReplacement,
            ) => Self::Fault(AuthorityFault::SchedulerProjection),
            PlanError::Duplicate
            | PlanError::PayloadVariant
            | PlanError::Membership(_)
            | PlanError::IngressRevoked(_) => Self::Fault(AuthorityFault::MembershipProjection),
        }
    }
}

#[derive(Debug)]
#[must_use = "direct rejection settlement preserves the sealed command semantics"]
pub(super) enum AuthorityDirectRejectionExecution {
    Local(CommittedPublicReject),
    TestAccept(CommittedPublicReject),
}

#[derive(Debug)]
#[must_use = "direct admission settlement preserves the sealed command semantics"]
pub(super) enum AuthorityDirectAdmissionExecution {
    Local(AuthorityLocalAdmissionExecution),
    TestAccept(AuthorityTestAcceptOutcome),
}

/// A synchronous Local outcome plus the only cache consequence that may be
/// published by its caller. `cache_update` is present only after an Accepted
/// membership Apply completed; every other disposition consumes the verified
/// cache evidence inside the authority boundary.
#[derive(Debug)]
#[must_use = "the Local result and its post-commit cache consequence must be handled"]
pub(super) struct AuthorityLocalAdmissionExecution {
    outcome: AuthorityLocalAdmissionOutcome,
    cache_update: Option<VerificationCacheUpdate>,
    cache_hit: bool,
}

impl AuthorityLocalAdmissionExecution {
    pub(super) fn into_parts(
        self,
    ) -> (
        AuthorityLocalAdmissionOutcome,
        Option<VerificationCacheUpdate>,
        bool,
    ) {
        (self.outcome, self.cache_update, self.cache_hit)
    }
}

#[derive(Debug)]
#[must_use = "verification cache evidence is a best-effort post-settlement effect"]
pub(in crate::authority) struct AuthorityVerificationOutcome {
    pub(in crate::authority) cache_update: Option<VerificationCacheUpdate>,
    pub(in crate::authority) cache_hit: bool,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use = "Ready is a level; an applied count is only progress evidence"]
pub(in crate::authority) enum AuthorityReadyOutcome {
    Idle,
    Applied { owners: usize },
}

enum EffectCheckoutState {
    Idle,
    Lease(EffectLease),
    ClosedAndDrained,
}

#[must_use = "an idle checkout returns the still-owned execution slot"]
pub(in crate::authority) enum AuthorityComputeCheckout {
    Job(AuthorityComputeJob),
    Idle(AuthorityComputeExecutionPermit),
}

#[must_use = "captured Ready validation work must be validated or discarded as stale"]
struct ReadyValidationBatch {
    head: FinalAdmissionValidation,
    tail: Vec<FinalAdmissionValidation>,
}

#[must_use = "Ready work must be preallocated and rechecked against one later authority cut"]
struct ReadyWorkBatch {
    snapshot: Arc<Snapshot>,
    head: FinalAdmissionWork,
    tail: Vec<FinalAdmissionWork>,
}

#[must_use = "prepared Ready work must complete its OCC capture"]
struct PreparedReadyValidationBatch {
    head: PreparedFinalAdmissionValidation,
    tail: Vec<PreparedFinalAdmissionValidation>,
    completed_tail: Vec<FinalAdmissionValidation>,
}

enum ReadyDisposition {
    Candidates(SettlementBatch),
    Head(FinalAdmissionValidationOutcome),
}

impl AuthorityRuntime {
    /// Construct the runtime and relay frontier bound from one validated
    /// configuration. Returning both prevents service assembly from compiling
    /// the policy twice or pairing a relay drain with another runtime.
    pub(in crate::authority) fn new_with_relay_parent_limit(
        config: &TxPoolConfig,
        consensus: &Consensus,
        snapshot: Arc<Snapshot>,
    ) -> Result<(Self, usize), RuntimeConfigError> {
        let runtime = AuthorityRuntimeConfig::from_runtime(config, consensus)?;
        let relay_parent_limit = runtime.resolution_policy.direct_max_edges;
        Self::from_config(runtime, snapshot).map(|runtime| (runtime, relay_parent_limit))
    }

    #[cfg(test)]
    pub(crate) fn new(
        config: &TxPoolConfig,
        consensus: &Consensus,
        snapshot: Arc<Snapshot>,
    ) -> Result<Self, RuntimeConfigError> {
        let runtime = AuthorityRuntimeConfig::from_runtime(config, consensus)?;
        Self::from_config(runtime, snapshot)
    }

    fn from_config(
        runtime: AuthorityRuntimeConfig,
        snapshot: Arc<Snapshot>,
    ) -> Result<Self, RuntimeConfigError> {
        let resolution_policy = runtime.resolution_policy;
        let expiry_policy = runtime.expiry_policy;
        let verify_workers = runtime.verify_workers;
        let transient_compute = ComputeGate::new(runtime.transient_compute_permits);
        Ok(Self {
            store: Arc::new(RwLock::new(AuthorityStore::from_runtime(
                runtime, snapshot,
            )?)),
            signals: Arc::new(AuthoritySignals::new()),
            resolution_policy,
            expiry_policy,
            verify_workers,
            transient_compute,
            #[cfg(test)]
            template_captures: Arc::new(AtomicUsize::new(0)),
        })
    }

    #[cfg(test)]
    pub(super) fn new_with_effect_limits_for_foundation(
        config: &TxPoolConfig,
        consensus: &Consensus,
        snapshot: Arc<Snapshot>,
        effects: EffectLimits,
    ) -> Result<Self, RuntimeConfigError> {
        let mut runtime = AuthorityRuntimeConfig::from_runtime(config, consensus)?;
        runtime.effects = effects;
        Self::from_config(runtime, snapshot)
    }

    pub(crate) fn transaction_lookup(
        &self,
        hash: &Byte32,
    ) -> Result<AuthorityTransactionLookup, AuthorityQueryError> {
        let store = self.store.read();
        super::query::transaction_lookup(
            &store.authority.read_view(),
            &store.snapshot,
            &RawTxHash(hash.clone()),
        )
        .map_err(Into::into)
    }

    pub(crate) fn pool_summary(&self) -> Result<AuthorityPoolSummary, AuthorityQueryError> {
        let store = self.store.read();
        let summary = store.authority.read_view().summary()?;
        AuthorityPoolSummary::capture(&store.snapshot, summary).map_err(Into::into)
    }

    pub(crate) fn filter_fresh_proposals(
        &self,
        mut proposals: Vec<ProposalShortId>,
    ) -> Result<Vec<ProposalShortId>, AuthorityQueryError> {
        let store = self.store.read();
        super::query::filter_fresh_proposals(&store.authority.read_view(), &mut proposals)?;
        Ok(proposals)
    }

    pub(crate) fn capture_compact_block(
        &self,
        mut requested: Vec<ProposalShortId>,
    ) -> Result<CompactBlockReadReceipt, AuthorityQueryError> {
        requested.sort_unstable();
        requested.dedup();
        let store = self.store.read();
        let view = store.authority.read_view();
        let mut committed = Vec::new();
        committed
            .try_reserve(requested.len())
            .map_err(|_| AuthorityQueryError::Allocation)?;
        for proposal in &requested {
            if view
                .entry_by_proposal(&super::state::ProposalId(proposal.clone()))?
                .is_none()
                && let Some(hash) = store.committed_txs_hash_cache.peek(proposal)
            {
                committed.push((proposal.clone(), hash.clone()));
            }
        }
        CompactBlockReadReceipt::capture(&view, Arc::clone(&store.snapshot), &requested, committed)
            .map_err(Into::into)
    }

    pub(crate) fn accepted_with_cycles(
        &self,
        mut requested: Vec<ProposalShortId>,
    ) -> Result<
        std::collections::HashMap<ProposalShortId, (TransactionView, u64)>,
        AuthorityQueryError,
    > {
        requested.sort_unstable();
        requested.dedup();
        let store = self.store.read();
        super::query::accepted_with_cycles(&store.authority.read_view(), &requested)
            .map_err(Into::into)
    }

    pub(crate) fn pool_ids(
        &self,
    ) -> Result<ckb_types::core::tx_pool::TxPoolIds, AuthorityQueryError> {
        let store = self.store.read();
        super::query::pool_ids(&store.authority.read_view()).map_err(Into::into)
    }

    pub(crate) fn all_entry_info(
        &self,
    ) -> Result<ckb_types::core::tx_pool::TxPoolEntryInfo, AuthorityQueryError> {
        let store = self.store.read();
        super::query::all_entry_info(&store.authority.read_view()).map_err(Into::into)
    }

    pub(crate) fn pool_detail(
        &self,
        hash: &Byte32,
    ) -> Result<Option<ckb_types::core::tx_pool::PoolTxDetailInfo>, AuthorityQueryError> {
        let store = self.store.read();
        super::query::pool_detail(&store.authority.read_view(), &RawTxHash(hash.clone()))
            .map_err(Into::into)
    }

    pub(crate) fn live_cell_receipt(&self, out_point: OutPoint) -> LiveCellReadReceipt {
        let store = self.store.read();
        LiveCellReadReceipt::capture(
            &store.authority.read_view(),
            Arc::clone(&store.snapshot),
            out_point,
        )
    }

    pub(crate) fn persistence_receipt(&self) -> Result<PersistenceReceipt, AuthorityQueryError> {
        let store = self.store.read();
        PersistenceReceipt::capture(&store.authority.read_view()).map_err(Into::into)
    }

    pub(crate) fn fee_estimate_receipt(
        &self,
    ) -> Result<FeeEstimateReadReceipt, AuthorityQueryError> {
        let store = self.store.read();
        FeeEstimateReadReceipt::capture(
            &store.authority.read_view(),
            &store.snapshot,
            self.resolution_policy.min_fee_rate,
        )
        .map_err(Into::into)
    }

    /// Capture immutable block-template payloads and the paired chain snapshot
    /// under one store guard, then canonicalize the owned selection only after
    /// releasing that guard.
    pub(in crate::authority) fn template_input(
        &self,
    ) -> Result<AuthorityTemplateInput, TemplateReadError> {
        #[cfg(test)]
        self.template_captures.fetch_add(1, Ordering::Relaxed);
        let (snapshot, receipt) = {
            let store = self.store.read();
            (
                Arc::clone(&store.snapshot),
                store.authority.read_view().capture_template()?,
            )
        };
        AuthorityTemplateInput::from_capture(snapshot, receipt)
    }

    #[cfg(test)]
    pub(super) fn template_capture_count_for_foundation(&self) -> usize {
        self.template_captures.load(Ordering::Relaxed)
    }

    /// Exact chain source used only at the short template publication Apply.
    /// Reading this Copy token under the authority guard prevents an old-tip
    /// build from publishing after a committed chain transition without
    /// extending the guard over construction or an await point.
    pub(in crate::authority) fn template_chain_source(&self) -> ApplySequence {
        self.store.read().authority.template_source_versions().chain
    }

    /// Copy-only source versions used by the template driver's level wait.
    /// This does not capture payloads or create another template authority.
    pub(in crate::authority) fn template_source_versions(
        &self,
    ) -> super::source::PoolTemplateVersions {
        self.store.read().authority.template_source_versions()
    }

    pub(in crate::authority) fn relay_parent_reader(&self) -> AuthorityRelayParentReader {
        AuthorityRelayParentReader {
            store: Arc::clone(&self.store),
        }
    }

    /// Remove one explicit owner through the total administrative compiler.
    /// Accepted roots include their descendant closure. Active preaccepted
    /// work is invalidated by ownership loss, so this operation never waits
    /// for a worker and a late completion is an ordinary stale outcome.
    pub(super) fn remove_local_transaction(
        &self,
        hash: &Byte32,
    ) -> Result<bool, AuthorityAdministrationError> {
        let committed = {
            let mut store = self.store.write();
            store
                .authority
                .plan_local_removal(&RawTxHash(hash.clone()))
                .map_err(AuthorityAdministrationError::from_plan)?
                .map(|plan| plan.apply())
        };
        let changed = committed.is_some();
        drop(committed);
        if changed {
            self.signals.publish_mutation();
        }
        Ok(changed)
    }

    /// Clear only preaccepted and replacement-history ownership. Accepted
    /// membership and the paired snapshot remain unchanged; advancing the
    /// generation makes every old compute/recovery capability stale without
    /// introducing a drain protocol.
    pub(super) fn clear_pipeline(&self) -> Result<(), AuthorityAdministrationError> {
        let committed = {
            let mut store = self.store.write();
            store
                .authority
                .plan_clear_pipeline()
                .map_err(AuthorityAdministrationError::from_plan)?
                .apply()
        };
        drop(committed);
        self.signals.publish_mutation();
        Ok(())
    }

    /// Replace all transaction ownership and its chain evidence under the one
    /// store guard. The authority derives the next revision from its current
    /// state; the supplied snapshot contributes only its exact tip and is
    /// installed in the same indivisible mutation.
    pub(super) fn clear_pool(
        &self,
        new_snapshot: Arc<Snapshot>,
    ) -> Result<(), AuthorityAdministrationError> {
        let tip_hash = new_snapshot.tip_hash();
        let fresh_hash_cache = LruCache::new(COMMITTED_HASH_CACHE_SIZE);
        let (committed, retired_snapshot, retired_hash_cache) = {
            let mut store = self.store.write();
            let committed = store
                .authority
                .plan_clear_pool(tip_hash)
                .map_err(AuthorityAdministrationError::from_plan)?
                .apply();
            let retired_snapshot = std::mem::replace(&mut store.snapshot, new_snapshot);
            let retired_hash_cache =
                std::mem::replace(&mut store.committed_txs_hash_cache, fresh_hash_cache);
            (committed, retired_snapshot, retired_hash_cache)
        };
        drop(committed);
        drop(retired_snapshot);
        drop(retired_hash_cache);
        self.signals.publish_mutation();
        Ok(())
    }

    /// Commit one ordered chain transition against the exact supplied
    /// snapshot. The upgradable read excludes intervening writers while the
    /// semantic disposition and proposal evidence are compiled, without
    /// adding a gate to ordinary admission. After upgrade, only capacity and
    /// derived-projection preparation plus total Apply remain.
    pub(super) fn apply_chain_update(
        &self,
        command: ChainUpdateCommand,
    ) -> Result<CommittedChainUpdate, ChainUpdateFailure> {
        let store = self.store.upgradable_read();
        let Some(next_revision) = store
            .authority
            .chain_revision()
            .0
            .checked_add(1)
            .map(ChainRevision)
        else {
            return Err(ChainUpdateFailure::new(
                ChainBoundaryError::CounterExhausted,
                command,
            ));
        };
        let old_environment = verification_environment(AcceptedStatus::Pending, &store.snapshot);
        let new_environment = verification_environment(AcceptedStatus::Pending, &command.snapshot);
        let old_rules =
            ScriptVerificationRules::from_env(store.snapshot.consensus(), &old_environment);
        let new_rules =
            ScriptVerificationRules::from_env(command.snapshot.consensus(), &new_environment);
        let accepted_validity = if old_rules != new_rules {
            AcceptedValidityTransition::RulesChanged
        } else if command.had_detached_chain {
            AcceptedValidityTransition::ContextChanged
        } else {
            AcceptedValidityTransition::Preserved
        };
        let new_view = ChainViewId::new(next_revision, command.snapshot.tip_hash());
        let facts = command.facts.bind(
            new_view.clone(),
            accepted_validity,
            command.packaging.authority_mode(),
        );
        let mut detached_recoveries = Vec::new();
        if detached_recoveries
            .try_reserve(command.facts.detached.len())
            .is_err()
        {
            return Err(ChainUpdateFailure::new(
                ChainBoundaryError::Allocation,
                command,
            ));
        }
        detached_recoveries.extend(facts.detached.iter().cloned());
        let receipt = match store.authority.chain_validation_work_from_view(facts) {
            Ok(work) => match work.validate(&command.snapshot) {
                Ok(receipt) => Some(receipt),
                Err(error) => {
                    let error = match error {
                        ChainValidationError::Allocation
                        | ChainValidationError::RecoveryAdmission(
                            super::state::AdmissionValidationError::ResourceAllocation,
                        ) => ChainBoundaryError::Allocation,
                        ChainValidationError::SnapshotMismatch
                        | ChainValidationError::MissingProposalPosition
                        | ChainValidationError::UnexpectedProposalPosition
                        | ChainValidationError::DuplicateProposalPosition => {
                            ChainBoundaryError::InvalidSnapshotEvidence
                        }
                        ChainValidationError::RecoveryAdmission(
                            super::state::AdmissionValidationError::EmptyTransaction
                            | super::state::AdmissionValidationError::ResourceArithmetic,
                        ) => ChainBoundaryError::InvalidFacts,
                    };
                    return Err(ChainUpdateFailure::new(error, command));
                }
            },
            Err(PlanError::Backpressure(Backpressure::GenerationReplacement)) => None,
            Err(error) => {
                return Err(ChainUpdateFailure::new(error.into(), command));
            }
        };
        let fallback_recoveries = match &receipt {
            Some(receipt) => match store.authority.chain_generation_recoveries(receipt) {
                Ok(recoveries) => recoveries,
                Err(error) => {
                    return Err(ChainUpdateFailure::new(error.into(), command));
                }
            },
            None => detached_recoveries,
        };

        let mut store = RwLockUpgradableReadGuard::upgrade(store);
        let plan = match receipt {
            Some(receipt) => match store.authority.plan_chain_transition(receipt) {
                Ok(plan) => plan,
                Err(PlanError::Backpressure(Backpressure::GenerationReplacement)) => {
                    match store
                        .authority
                        .plan_chain_generation_replacement(new_view, fallback_recoveries)
                    {
                        Ok(plan) => plan,
                        Err(error) => {
                            return Err(ChainUpdateFailure::new(error.into(), command));
                        }
                    }
                }
                Err(error) => {
                    return Err(ChainUpdateFailure::new(error.into(), command));
                }
            },
            None => match store
                .authority
                .plan_chain_generation_replacement(new_view, fallback_recoveries)
            {
                Ok(plan) => plan,
                Err(error) => {
                    return Err(ChainUpdateFailure::new(error.into(), command));
                }
            },
        };
        let committed = plan.apply();
        let ChainUpdateCommand {
            facts: _,
            committed_hashes,
            candidate_uncles,
            attached_blocks,
            had_detached_chain: _,
            packaging: _,
            snapshot,
        } = command;
        let retired_snapshot = std::mem::replace(&mut store.snapshot, Arc::clone(&snapshot));
        for (proposal, hash) in committed_hashes {
            store.committed_txs_hash_cache.put(proposal, hash);
        }
        drop(store);
        drop(committed);
        drop(retired_snapshot);
        self.signals.publish_mutation();
        Ok(CommittedChainUpdate {
            candidate_uncles,
            attached_blocks,
            snapshot,
        })
    }

    /// Expire one bounded due prefix of retained Remote owners. The wall clock
    /// and slice are runtime policy, not caller-provided transition evidence.
    pub(super) fn expire_remote_due(
        &self,
    ) -> Result<AuthorityMaintenanceOutcome, AuthorityDriverError> {
        let cutoff = RemoteDeadline(ckb_systemtime::unix_time().as_secs());
        let committed = {
            let mut store = self.store.write();
            store
                .authority
                .plan_remote_expiry(cutoff, self.expiry_policy.remote_slice)
                .map_err(AuthorityDriverError::from_maintenance_plan)?
                .map(|plan| plan.apply())
        };
        let Some(committed) = committed else {
            return Ok(AuthorityMaintenanceOutcome::Idle);
        };
        let owners = committed.changed_owner_count();
        drop(committed);
        self.signals.publish_mutation();
        Ok(AuthorityMaintenanceOutcome::Applied { owners })
    }

    /// Expire the oldest due Accepted root and its full descendant closure.
    /// One root is selected per Apply; the component and its exact rejection
    /// effects remain bounded by accepted-pool admission limits.
    pub(super) fn expire_accepted_due(
        &self,
    ) -> Result<AuthorityMaintenanceOutcome, AuthorityDriverError> {
        let Some(cutoff) = ckb_systemtime::unix_time_as_millis()
            .checked_sub(self.expiry_policy.accepted_residency_millis)
            .map(AcceptedAtMillis)
        else {
            return Ok(AuthorityMaintenanceOutcome::Idle);
        };
        let committed = {
            let mut store = self.store.write();
            store
                .authority
                .plan_accepted_expiry(cutoff)
                .map_err(AuthorityDriverError::from_maintenance_plan)?
                .map(|plan| plan.apply())
        };
        let Some(committed) = committed else {
            return Ok(AuthorityMaintenanceOutcome::Idle);
        };
        let owners = committed.changed_owner_count();
        drop(committed);
        self.signals.publish_mutation();
        Ok(AuthorityMaintenanceOutcome::Applied { owners })
    }

    /// Advance one dirty dependency edge or completion marker. The dependency
    /// frontier is level-triggered, so callers may repeat this bounded step
    /// until `Idle` without owning a second queue or cursor.
    pub(super) fn maintain_dependency(
        &self,
    ) -> Result<AuthorityMaintenanceOutcome, AuthorityDriverError> {
        let committed = {
            let mut store = self.store.write();
            store
                .authority
                .plan_dependency_maintenance()
                .map_err(AuthorityDriverError::from_maintenance_plan)?
                .map(|plan| plan.apply())
        };
        let Some(committed) = committed else {
            return Ok(AuthorityMaintenanceOutcome::Idle);
        };
        let owners = committed.changed_owner_count();
        drop(committed);
        self.signals.publish_mutation();
        Ok(AuthorityMaintenanceOutcome::Applied { owners })
    }

    pub(super) fn mutation_signal(&self) -> Arc<Notify> {
        Arc::clone(&self.signals.mutation)
    }

    /// Acquire one shared tx-pool execution slot before any retained checkout
    /// or owner-free direct capture. Waiting owns no authority capability and
    /// holds no guard. Normal shutdown cancels the waiter; the semaphore itself
    /// is never closed as a control protocol.
    pub(in crate::authority) async fn acquire_compute_execution(
        &self,
        cancel: &CancellationToken,
    ) -> Option<AuthorityComputeExecutionPermit> {
        self.transient_compute
            .acquire(cancel)
            .await
            .map(|permit| AuthorityComputeExecutionPermit { _permit: permit })
    }

    #[cfg(test)]
    pub(super) fn try_compute_execution_for_foundation(
        &self,
    ) -> Option<AuthorityComputeExecutionPermit> {
        self.transient_compute
            .try_acquire()
            .map(|permit| AuthorityComputeExecutionPermit { _permit: permit })
    }

    #[cfg(test)]
    pub(super) fn available_compute_permits_for_foundation(&self) -> usize {
        self.transient_compute.available_permits()
    }

    #[cfg(test)]
    pub(super) fn try_checkout_for_foundation(
        &self,
        permit: WorkPermit,
    ) -> Result<
        ControlFlow<AuthorityPendingSettlement, Option<AuthorityComputeJob>>,
        AuthorityComputeError,
    > {
        let execution = self
            .try_compute_execution_for_foundation()
            .ok_or(AuthorityComputeError::ComputeCapacity)?;
        match self.try_checkout(permit, execution)? {
            ControlFlow::Break(pending) => Ok(ControlFlow::Break(pending)),
            ControlFlow::Continue(AuthorityComputeCheckout::Job(job)) => {
                Ok(ControlFlow::Continue(Some(job)))
            }
            ControlFlow::Continue(AuthorityComputeCheckout::Idle(execution)) => {
                drop(execution);
                Ok(ControlFlow::Continue(None))
            }
        }
    }

    /// Capture the immutable consensus paired with the authority snapshot.
    /// Ingress callers cannot substitute a separately sourced consensus for
    /// non-contextual validation.
    pub(super) fn paired_consensus(&self) -> Arc<Consensus> {
        self.store.read().snapshot.cloned_consensus()
    }

    /// Attempt one feature-internal synthetic insertion. Immutable fixture
    /// evidence is built after releasing the coherent store read guard; the
    /// write-side Plan then rechecks its exact chain/dependency cut before
    /// compiling the ordinary atomic membership transition.
    #[cfg(any(test, feature = "internal"))]
    pub(super) fn plug_internal_entry(
        &self,
        entry: &TxEntry,
        status: AcceptedStatus,
    ) -> Result<AuthorityInternalPlugOutcome, AuthorityInternalPlugError> {
        let (view, dependency_cut, snapshot) = {
            let store = self.store.read();
            (
                store.authority.chain_view().clone(),
                store.authority.dependency_observation_cut(),
                Arc::clone(&store.snapshot),
            )
        };
        let receipt = super::internal::build_receipt(
            entry,
            status,
            view,
            dependency_cut,
            &snapshot,
            self.resolution_policy.direct_max_edges,
        )
        .map_err(AuthorityInternalPlugError::from_build)?;
        let committed = {
            let mut store = self.store.write();
            match store
                .authority
                .plan_internal_plug(receipt)
                .map_err(AuthorityInternalPlugError::from_plan)?
            {
                InternalPlugDisposition::Insert(plan) => Some(plan.apply()),
                InternalPlugDisposition::Duplicate => None,
            }
        };
        let Some(committed) = committed else {
            return Ok(AuthorityInternalPlugOutcome::Duplicate);
        };
        drop(committed);
        self.signals.publish_mutation();
        Ok(AuthorityInternalPlugOutcome::Inserted)
    }

    #[cfg(test)]
    pub(super) fn normalized_snapshot_for_foundation(&self) -> super::plan::AuthoritySnapshot {
        self.store.read().authority.normalized_snapshot()
    }

    #[cfg(test)]
    pub(super) fn paired_chain_for_foundation(&self) -> (ChainViewId, Arc<Snapshot>) {
        let store = self.store.read();
        (
            store.authority.chain_view().clone(),
            Arc::clone(&store.snapshot),
        )
    }

    #[cfg(test)]
    pub(super) fn committed_hash_for_foundation(
        &self,
        proposal: &ProposalShortId,
    ) -> Option<Byte32> {
        self.store
            .write()
            .committed_txs_hash_cache
            .get(proposal)
            .cloned()
    }

    #[cfg(test)]
    pub(super) fn with_authority_for_foundation<T>(
        &self,
        inspect: impl FnOnce(&mut TxPoolAuthority) -> T,
    ) -> T {
        inspect(&mut self.store.write().authority)
    }

    /// Capture and evaluate one already-resolved direct candidate without
    /// acquiring membership or effect authority. Local and TestAccept share
    /// this immutable result; only Local may later compile it through Plan.
    fn validate_direct_admission(
        &self,
        work: DirectAdmissionWork,
    ) -> Result<DirectAdmissionValidationOutcome, FinalAdmissionValidationError> {
        let snapshot = {
            let store = self.store.read();
            Arc::clone(&store.snapshot)
        };
        let prepared = DirectAdmissionValidation::prepare(snapshot, work)?;
        let validation = {
            let store = self.store.read();
            prepared.complete(AuthorityStoreCaptureSeal(()), &store.authority)?
        };
        validation.validate()
    }

    /// Publish a stable or still-current direct ingress/compute rejection for
    /// Local. The validity fence and effect append are one authoritative
    /// Plan/Apply; a stale chain or dependency observation commits nothing.
    fn commit_direct_transaction_rejection(
        &self,
        rejection: DirectTransactionRejection,
    ) -> Result<CommittedPublicReject, PlanError> {
        let (reason, committed) = {
            let mut store = self.store.write();
            store
                .authority
                .plan_direct_transaction_rejection(rejection)?
                .apply()
        };
        drop(committed);
        self.signals.publish_mutation();
        Ok(reason)
    }

    /// Evaluate the same direct rejection for TestAccept without publishing
    /// an effect. A stale rejection is returned as stale work so the caller
    /// recomputes from its original transaction.
    fn evaluate_direct_transaction_rejection(
        &self,
        rejection: DirectTransactionRejection,
    ) -> Result<CommittedPublicReject, PlanError> {
        let store = self.store.read();
        store
            .authority
            .direct_rejection_is_current(rejection.validity())?;
        Ok(rejection.reason().clone())
    }

    /// Commit one final synchronous Local disposition. Candidate membership,
    /// same-raw PreAccepted replacement, RBF/capacity policy, resource charge,
    /// and publication are applied atomically under the sole authority. A
    /// rules transition asks the caller to recompute and performs no Apply.
    fn commit_direct_admission(
        &self,
        outcome: DirectAdmissionValidationOutcome,
    ) -> Result<AuthorityLocalAdmissionOutcome, PlanError> {
        let (outcome, committed) = match outcome {
            DirectAdmissionValidationOutcome::Candidate(receipt) => {
                let completed = receipt.completed();
                let mut store = self.store.write();
                match store.authority.plan_direct_admission(receipt)? {
                    DirectAdmissionDisposition::Accepted(plan) => (
                        AuthorityLocalAdmissionOutcome::Accepted(completed),
                        Some(plan.apply()),
                    ),
                    DirectAdmissionDisposition::Duplicate(plan) => {
                        let (key, committed) = plan.apply();
                        (
                            AuthorityLocalAdmissionOutcome::Duplicate(key),
                            Some(committed),
                        )
                    }
                    DirectAdmissionDisposition::Rejected(plan) => {
                        let (reason, committed) = plan.apply();
                        (
                            AuthorityLocalAdmissionOutcome::Rejected(
                                DirectAdmissionRejectionKind::Membership(reason),
                            ),
                            Some(committed),
                        )
                    }
                }
            }
            DirectAdmissionValidationOutcome::Rejected(rejection) => {
                let mut store = self.store.write();
                let (reason, committed) = store
                    .authority
                    .plan_direct_validation_rejection(rejection)?
                    .apply();
                (
                    AuthorityLocalAdmissionOutcome::Rejected(
                        DirectAdmissionRejectionKind::Validation(reason),
                    ),
                    Some(committed),
                )
            }
            DirectAdmissionValidationOutcome::Reresolve(retry) => (
                AuthorityLocalAdmissionOutcome::Retry(retry.into_subject().into_transaction()),
                None,
            ),
        };
        let changed = committed.is_some();
        drop(committed);
        if changed {
            self.signals.publish_mutation();
        }
        Ok(outcome)
    }

    /// Evaluate one final TestAccept disposition from the same validation and
    /// membership policy while holding only a read guard. No mutating Plan,
    /// effect, cache update, clock increment, or owner is constructed.
    fn evaluate_direct_admission(
        &self,
        outcome: DirectAdmissionValidationOutcome,
    ) -> Result<AuthorityTestAcceptOutcome, PlanError> {
        let store = self.store.read();
        match outcome {
            DirectAdmissionValidationOutcome::Candidate(receipt) => {
                match store.authority.evaluate_direct_admission(receipt)? {
                    DirectAdmissionEvaluation::Accepted(completed) => {
                        Ok(AuthorityTestAcceptOutcome::Accepted(completed))
                    }
                    DirectAdmissionEvaluation::Duplicate(key) => {
                        Ok(AuthorityTestAcceptOutcome::Duplicate(key))
                    }
                    DirectAdmissionEvaluation::Rejected(reason) => {
                        Ok(AuthorityTestAcceptOutcome::RejectedMembership(reason))
                    }
                }
            }
            DirectAdmissionValidationOutcome::Rejected(rejection) => store
                .authority
                .evaluate_direct_validation_rejection(rejection)
                .map(AuthorityTestAcceptOutcome::RejectedValidation),
            DirectAdmissionValidationOutcome::Reresolve(_) => Ok(AuthorityTestAcceptOutcome::Retry),
        }
    }

    /// Settle one source-sealed direct rejection. Local publishes the rejection
    /// through the authoritative effect log; TestAccept only validates the
    /// evidence cut. Callers cannot choose the behavior after computation.
    pub(super) fn settle_direct_transaction_rejection(
        &self,
        rejection: AuthorityDirectRejection,
    ) -> Result<AuthorityDirectRejectionExecution, AuthorityDirectAdmissionError> {
        let AuthorityDirectRejection {
            rejection,
            execution,
        } = rejection;
        let result = match rejection.command() {
            DirectCommand::Local => self
                .commit_direct_transaction_rejection(rejection)
                .map(AuthorityDirectRejectionExecution::Local)
                .map_err(AuthorityDirectAdmissionError::from_plan),
            DirectCommand::TestAccept => self
                .evaluate_direct_transaction_rejection(rejection)
                .map(AuthorityDirectRejectionExecution::TestAccept)
                .map_err(AuthorityDirectAdmissionError::from_plan),
        };
        drop(execution);
        result
    }

    /// Validate and settle one source-sealed verified direct capability. The
    /// optional cache update remains sealed until the exact Local Accepted
    /// Apply succeeds. TestAccept consumes the same evidence without owner,
    /// effect, clock, or cache mutation.
    pub(super) fn settle_verified_direct_admission(
        &self,
        candidate: AuthorityDirectVerifiedCandidate,
    ) -> Result<AuthorityDirectAdmissionExecution, AuthorityDirectAdmissionError> {
        let AuthorityDirectVerifiedCandidate {
            candidate,
            execution,
        } = candidate;
        let (command, work, pending_cache_update, cache_hit) = candidate.into_parts();
        let validated = self
            .validate_direct_admission(work)
            .map_err(AuthorityDirectAdmissionError::from_validation)?;
        let result = match command {
            DirectCommand::Local => {
                let outcome = self
                    .commit_direct_admission(validated)
                    .map_err(AuthorityDirectAdmissionError::from_plan)?;
                let cache_update = matches!(&outcome, AuthorityLocalAdmissionOutcome::Accepted(_))
                    .then_some(pending_cache_update)
                    .flatten();
                Ok(AuthorityDirectAdmissionExecution::Local(
                    AuthorityLocalAdmissionExecution {
                        outcome,
                        cache_update,
                        cache_hit,
                    },
                ))
            }
            DirectCommand::TestAccept => self
                .evaluate_direct_admission(validated)
                .map(AuthorityDirectAdmissionExecution::TestAccept)
                .map_err(AuthorityDirectAdmissionError::from_plan),
        };
        drop(execution);
        result
    }

    /// Execute one cache-bound owner-free direct request outside every
    /// authority guard. The shared transient slot remains inside the returned
    /// source-sealed candidate or rejection until final settlement.
    pub(super) async fn execute_direct_verification(
        &self,
        request: AuthorityCacheBoundDirectVerification,
        command_rx: Option<&mut watch::Receiver<ckb_script::ChunkCommand>>,
    ) -> Result<AuthorityDirectVerificationOutcome, DirectComputationError> {
        let AuthorityCacheBoundDirectVerification { request, execution } = request;
        match request.execute(command_rx).await? {
            DirectVerificationOutcome::Candidate(candidate) => Ok(
                AuthorityDirectVerificationOutcome::Candidate(AuthorityDirectVerifiedCandidate {
                    candidate,
                    execution,
                }),
            ),
            DirectVerificationOutcome::Rejected(rejection) => Ok(
                AuthorityDirectVerificationOutcome::Rejected(AuthorityDirectRejection {
                    rejection,
                    execution,
                }),
            ),
        }
    }

    /// Non-contextually validate and resolve a synchronous Local/TestAccept
    /// transaction without creating a retained owner. Missing-frontier
    /// enrichment rereads only the bounded Accepted overlay and keeps all
    /// allocation outside the authority guard.
    pub(super) fn resolve_local_transaction(
        &self,
        tx: &ckb_types::core::TransactionView,
        execution: AuthorityComputeExecutionPermit,
    ) -> Result<AuthorityDirectResolutionOutcome, DirectComputationError> {
        self.resolve_direct_transaction(tx, DirectCommand::Local, execution)
    }

    pub(super) fn resolve_test_accept_transaction(
        &self,
        tx: &ckb_types::core::TransactionView,
        execution: AuthorityComputeExecutionPermit,
    ) -> Result<AuthorityDirectResolutionOutcome, DirectComputationError> {
        self.resolve_direct_transaction(tx, DirectCommand::TestAccept, execution)
    }

    fn resolve_direct_transaction(
        &self,
        tx: &ckb_types::core::TransactionView,
        command: DirectCommand,
        execution: AuthorityComputeExecutionPermit,
    ) -> Result<AuthorityDirectResolutionOutcome, DirectComputationError> {
        let consensus = self.paired_consensus();
        let direct = match direct(tx, &consensus, command) {
            Ok(direct) => direct,
            Err(rejection) => {
                return Ok(AuthorityDirectResolutionOutcome::Rejected(
                    AuthorityDirectRejection {
                        rejection,
                        execution,
                    },
                ));
            }
        };
        self.prepare_direct_resolution(direct, execution)
    }

    fn prepare_direct_resolution(
        &self,
        direct: DirectTransaction,
        execution: AuthorityComputeExecutionPermit,
    ) -> Result<AuthorityDirectResolutionOutcome, DirectComputationError> {
        let prepared = match DirectResolutionJob::prepare(
            direct,
            self.resolution_policy.direct_max_resident_bytes,
            self.resolution_policy.direct_max_edges,
        )? {
            DirectResolutionPreparation::Prepared(prepared) => prepared,
            DirectResolutionPreparation::Rejected(rejection) => {
                return Ok(AuthorityDirectResolutionOutcome::Rejected(
                    AuthorityDirectRejection {
                        rejection,
                        execution,
                    },
                ));
            }
        };
        let mut job = {
            let store = self.store.read();
            prepared.complete(
                AuthorityStoreCaptureSeal(()),
                Arc::clone(&store.snapshot),
                &store.authority,
            )
        };
        loop {
            let evaluation =
                crate::util::block_offload(|| job.evaluate(self.resolution_policy.min_fee_rate))?;
            match evaluation {
                DirectResolutionEvaluation::Verify(request) => {
                    return Ok(AuthorityDirectResolutionOutcome::Verification(
                        AuthorityDirectVerificationRequest { request, execution },
                    ));
                }
                DirectResolutionEvaluation::Rejected(rejection) => {
                    return Ok(AuthorityDirectResolutionOutcome::Rejected(
                        AuthorityDirectRejection {
                            rejection,
                            execution,
                        },
                    ));
                }
                DirectResolutionEvaluation::Enrich(probe) => {
                    let prepared = probe.prepare_enrichment()?;
                    let observation = {
                        let store = self.store.read();
                        prepared.observe(&store.authority)?
                    };
                    match observation {
                        DirectResolutionProbeObservation::Retry(retry) => job = retry,
                        DirectResolutionProbeObservation::Rejected(rejection) => {
                            return Ok(AuthorityDirectResolutionOutcome::Rejected(
                                AuthorityDirectRejection {
                                    rejection,
                                    execution,
                                },
                            ));
                        }
                    }
                }
            }
        }
    }

    pub(in crate::authority) fn claim_effect_publisher(
        &self,
    ) -> Option<AuthorityEffectPublisherClaim> {
        self.signals
            .effect_publisher_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| AuthorityEffectPublisherClaim {
                signals: Arc::clone(&self.signals),
            })
    }

    fn try_effect_checkout(&self) -> Result<EffectCheckoutState, EffectCheckoutError> {
        let (lease, retirement) = {
            let mut store = self.store.write();
            let Some(plan) = store.authority.plan_effect_checkout()? else {
                return if store.authority.effects_closed_and_drained() {
                    Ok(EffectCheckoutState::ClosedAndDrained)
                } else {
                    Ok(EffectCheckoutState::Idle)
                };
            };
            plan.apply().into_parts()
        };
        drop(retirement);
        self.signals.publish_mutation();
        Ok(EffectCheckoutState::Lease(lease))
    }

    /// Wait for the next committed effect capability. `None` means the log is
    /// closed and fully drained. The sole publisher calls this only while it
    /// owns no prior lease; cancellation during the wait owns no capability,
    /// and there is no suspension point after checkout succeeds.
    pub(in crate::authority) async fn wait_effect_checkout(
        &self,
    ) -> Result<Option<EffectLease>, EffectCheckoutError> {
        loop {
            let signal = self.mutation_signal();
            let notified = signal.notified();
            match self.try_effect_checkout()? {
                EffectCheckoutState::Idle => notified.await,
                EffectCheckoutState::Lease(lease) => return Ok(Some(lease)),
                EffectCheckoutState::ClosedAndDrained => return Ok(None),
            }
        }
    }

    pub(in crate::authority) fn settle_effect(
        &self,
        settlement: EffectSettlement,
    ) -> Result<(), EffectSettlementFailure> {
        let rejection_metrics = settlement.rejection_metrics();
        let retirement = {
            let mut store = self.store.write();
            store.authority.apply_effect_settlement(settlement)?
        };
        drop(retirement);
        self.signals.publish_mutation();
        rejection_metrics.publish();
        Ok(())
    }

    /// Publish a coalesced operational projection without creating another
    /// state owner. The complete copy is captured under one authority read
    /// guard; metric calls happen only after the guard is released.
    pub(in crate::authority) fn publish_operational_metrics(&self) {
        let metrics = {
            let store = self.store.read();
            store.authority.operational_metrics()
        };
        metrics.publish();
    }

    /// Close effect production after every state producer and compute
    /// capability has drained. Already committed queued/active effects remain
    /// checkout-able until `effects_closed_and_drained` becomes true.
    pub(in crate::authority) fn close_effects(&self) -> Result<(), EffectCloseError> {
        let retirement = {
            let mut store = self.store.write();
            store.authority.plan_effect_close()?.apply()
        };
        drop(retirement);
        self.signals.publish_mutation();
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn queue_effect_for_foundation(
        &self,
        policy: super::effect::EffectPolicy,
        effect: super::effect::CommittedEffect,
    ) -> Result<(), FoundationEffectQueueError> {
        self.queue_effects_for_foundation(policy, vec![effect])
    }

    #[cfg(test)]
    pub(super) fn queue_effects_for_foundation(
        &self,
        policy: super::effect::EffectPolicy,
        effects: Vec<super::effect::CommittedEffect>,
    ) -> Result<(), FoundationEffectQueueError> {
        let retirement = {
            let mut store = self.store.write();
            let publication = store
                .authority
                .effect_publication_for_foundation(policy, effects)
                .map_err(FoundationEffectQueueError::Build)?;
            store
                .authority
                .plan_effect_publication_for_foundation(&publication)
                .map_err(FoundationEffectQueueError::Plan)?
                .apply()
        };
        drop(retirement);
        self.signals.publish_mutation();
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn queue_generation_reset_for_foundation(&self) -> Result<(), PlanError> {
        let retirement = {
            let mut store = self.store.write();
            store
                .authority
                .plan_generation_reset_for_foundation()?
                .apply()
        };
        drop(retirement);
        self.signals.publish_mutation();
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn effect_observation_for_foundation(&self) -> super::effect::EffectObservation {
        self.store
            .read()
            .authority
            .effect_observation_for_foundation()
    }

    pub(in crate::authority) fn effects_closed_and_drained(&self) -> bool {
        self.store.read().authority.effects_closed_and_drained()
    }

    /// Return a rejection already committed to the charged effect log but not
    /// necessarily persisted yet. Only the immutable batch pointer is cloned
    /// under the authority guard; public conversion and JSON allocation happen
    /// after the guard opens.
    pub(crate) fn pending_recent_reject(
        &self,
        hash: &Byte32,
    ) -> Result<Option<String>, AuthorityRecentRejectReadError> {
        let pending = {
            self.store
                .read()
                .authority
                .pending_recent_reject(&RawTxHash(hash.clone()))
        };
        let Some(pending) = pending else {
            return Ok(None);
        };
        let public = pending
            .public_reject()
            .map_err(|_| AuthorityRecentRejectReadError::Projection)?;
        serialized_recent_reject(public.reject())
            .map(Some)
            .map_err(AuthorityRecentRejectReadError::Encoding)
    }

    pub(super) const fn verify_worker_count(&self) -> usize {
        self.verify_workers.get()
    }

    #[cfg(test)]
    pub(in crate::authority) fn admit(
        &self,
        admission: ValidatedAdmission,
    ) -> Result<(), PlanError> {
        let committed = {
            let mut store = self.store.write();
            store.authority.plan_admission(admission)?.apply()
        };
        // Potential last-owner payload/effect destruction must never extend
        // the authority critical section.
        drop(committed);
        self.signals.publish_mutation();
        Ok(())
    }

    pub(super) fn commit_retained_ingress(
        &self,
        ingress: RetainedIngress,
    ) -> Result<RetainedIngressCommit, PlanError> {
        let (outcome, committed) = {
            let mut store = self.store.write();
            match store.authority.plan_retained_admission(ingress)? {
                RetainedAdmissionDisposition::Retained(plan) => {
                    (RetainedIngressCommit::Retained, Some(plan.apply()))
                }
                RetainedAdmissionDisposition::AcceptedDuplicate(plan) => {
                    (RetainedIngressCommit::AcceptedDuplicate, Some(plan.apply()))
                }
                RetainedAdmissionDisposition::RemoteReleased(plan) => {
                    (RetainedIngressCommit::RemoteReleased, Some(plan.apply()))
                }
                RetainedAdmissionDisposition::ProposalUnchanged => {
                    (RetainedIngressCommit::ProposalUnchanged, None)
                }
            }
        };
        let changed = committed.is_some();
        // Effect and transaction retirement carriers are destroyed only after
        // the single authority guard is open.
        drop(committed);
        if changed {
            self.signals.publish_mutation();
        }
        Ok(outcome)
    }

    pub(super) fn commit_retained_ingress_rejection(
        &self,
        rejection: RetainedIngressRejection,
    ) -> Result<RetainedIngressCommit, PlanError> {
        let committed = {
            let mut store = self.store.write();
            store
                .authority
                .plan_retained_ingress_rejection(rejection)?
                .apply()
        };
        drop(committed);
        self.signals.publish_mutation();
        Ok(RetainedIngressCommit::Rejected)
    }

    pub(in crate::authority) fn try_checkout(
        &self,
        permit: WorkPermit,
        execution: AuthorityComputeExecutionPermit,
    ) -> Result<
        ControlFlow<AuthorityPendingSettlement, AuthorityComputeCheckout>,
        AuthorityComputeError,
    > {
        let (result, checkout_retirement, settlement_retirement) = {
            let mut store = self.store.write();
            let plan = store
                .authority
                .plan_checkout_next(permit)
                .map_err(AuthorityComputeError::from_checkout_plan)?;
            let Some(plan) = plan else {
                return Ok(ControlFlow::Continue(AuthorityComputeCheckout::Idle(
                    execution,
                )));
            };
            let (work, checkout) = plan.apply().into_parts();
            let snapshot = Arc::clone(&store.snapshot);
            let captured = match work {
                CheckedOutWork::Resolve(work) => {
                    ResolutionJob::capture_resolve(&store.authority, snapshot, work)
                        .map(AuthorityComputeKind::Resolution)
                }
                CheckedOutWork::ContinuousResolve(work) => {
                    ResolutionJob::capture_continuous(&store.authority, snapshot, work)
                        .map(AuthorityComputeKind::Resolution)
                }
                CheckedOutWork::Verify(work) => VerificationJob::from_checkout(work, snapshot)
                    .map(AuthorityComputeKind::Verification),
            };
            match captured {
                Ok(inner) => (
                    Ok(ControlFlow::Continue(AuthorityComputeCheckout::Job(
                        AuthorityComputeJob { inner, execution },
                    ))),
                    checkout,
                    None,
                ),
                Err(failure) => {
                    let kind = failure.kind();
                    match store.authority.apply_settlement(failure.into_settlement()) {
                        Ok(settlement) => (
                            Err(AuthorityComputeError::Resolution(kind)),
                            checkout,
                            Some(settlement),
                        ),
                        Err(failure) => (
                            Ok(ControlFlow::Break(AuthorityPendingSettlement::new(
                                failure,
                                SettlementOrigin::Capture(kind),
                                execution,
                            ))),
                            checkout,
                            None,
                        ),
                    }
                }
            }
        };
        Self::finish_checkout(
            result,
            checkout_retirement,
            settlement_retirement,
            &self.signals,
        )
    }

    fn finish_checkout(
        result: Result<
            ControlFlow<AuthorityPendingSettlement, AuthorityComputeCheckout>,
            AuthorityComputeError,
        >,
        checkout_retirement: super::plan::CommittedDelta,
        settlement_retirement: Option<super::plan::CommittedDelta>,
        signals: &AuthoritySignals,
    ) -> Result<
        ControlFlow<AuthorityPendingSettlement, AuthorityComputeCheckout>,
        AuthorityComputeError,
    > {
        drop(checkout_retirement);
        drop(settlement_retirement);
        signals.publish_mutation();
        result
    }

    /// Level-triggered checkout with no lock held across the wait. Cancelling
    /// this future while it waits owns no compute capability; after checkout
    /// succeeds there is no suspension point before the job is returned.
    #[cfg(test)]
    pub(in crate::authority) async fn wait_checkout(
        &self,
        permit: WorkPermit,
        cancel: &CancellationToken,
    ) -> Result<
        ControlFlow<AuthorityPendingSettlement, Option<AuthorityComputeJob>>,
        AuthorityComputeError,
    > {
        loop {
            let signal = self.mutation_signal();
            let notified = signal.notified();
            let Some(execution) = self.acquire_compute_execution(cancel).await else {
                return Ok(ControlFlow::Continue(None));
            };
            match self.try_checkout(permit, execution)? {
                ControlFlow::Break(pending) => return Ok(ControlFlow::Break(pending)),
                ControlFlow::Continue(AuthorityComputeCheckout::Job(job)) => {
                    return Ok(ControlFlow::Continue(Some(job)));
                }
                ControlFlow::Continue(AuthorityComputeCheckout::Idle(execution)) => {
                    drop(execution);
                }
            }
            tokio::select! {
                _ = cancel.cancelled() => return Ok(ControlFlow::Continue(None)),
                _ = notified => {}
            }
        }
    }

    pub(in crate::authority) fn settle_compute(
        &self,
        retained: AuthorityComputeSettlement,
        origin: SettlementOrigin,
    ) -> ControlFlow<AuthorityPendingSettlement> {
        let AuthorityComputeSettlement {
            settlement,
            execution,
        } = retained;
        match self.settle(settlement) {
            Ok(()) => {
                drop(execution);
                ControlFlow::Continue(())
            }
            Err(failure) => {
                ControlFlow::Break(AuthorityPendingSettlement::new(failure, origin, execution))
            }
        }
    }

    pub(in crate::authority) fn retry_unexpected_verification(
        &self,
        request: AuthorityVerificationRequest,
    ) -> ControlFlow<AuthorityPendingSettlement> {
        self.settle_compute(request.retry(), SettlementOrigin::Completion)
    }

    pub(in crate::authority) fn settle(
        &self,
        settlement: ComputeSettlement,
    ) -> Result<(), ComputeSettlementFailure> {
        let committed = {
            let mut store = self.store.write();
            store.authority.apply_settlement(settlement)?
        };
        drop(committed);
        self.signals.publish_mutation();
        Ok(())
    }

    pub(in crate::authority) fn cancel_compute_after_allocation(
        &self,
        cancellation: ComputeCancellation,
    ) -> Result<(), ComputeCancellationError> {
        let committed = {
            let mut store = self.store.write();
            store.authority.apply_compute_cancellation(cancellation)?
        };
        drop(committed);
        self.signals.publish_mutation();
        Ok(())
    }

    /// Execute one resolve capability entirely outside the authority guard.
    /// A bounded dep-group miss may take allocation-free Accepted read cuts;
    /// every terminal or retry result is settled before this method returns.
    pub(in crate::authority) fn execute_compute(
        &self,
        job: AuthorityComputeJob,
    ) -> Result<
        ControlFlow<AuthorityPendingSettlement, AuthorityComputeOutcome>,
        AuthorityComputeError,
    > {
        let AuthorityComputeJob { inner, execution } = job;
        match inner {
            AuthorityComputeKind::Resolution(job) => self.execute_resolution(job, execution),
            AuthorityComputeKind::Verification(job) => Ok(ControlFlow::Continue(
                AuthorityComputeOutcome::Verification(AuthorityVerificationRequest {
                    request: job.prepare(),
                    execution,
                }),
            )),
        }
    }

    fn execute_resolution(
        &self,
        mut job: ResolutionJob,
        execution: AuthorityComputeExecutionPermit,
    ) -> Result<
        ControlFlow<AuthorityPendingSettlement, AuthorityComputeOutcome>,
        AuthorityComputeError,
    > {
        loop {
            let policy = self.resolution_policy;
            let evaluated = crate::util::block_offload(|| {
                job.evaluate(policy.min_fee_rate, policy.large_cycle_threshold)
            });
            let evaluation = match evaluated {
                Ok(evaluation) => evaluation,
                Err(failure) => return self.settle_resolution_failure(failure, execution),
            };
            match evaluation {
                ResolutionEvaluation::Settle(settlement) => match self.settle(settlement) {
                    Ok(()) => {
                        drop(execution);
                        return Ok(ControlFlow::Continue(AuthorityComputeOutcome::Settled));
                    }
                    Err(failure) => {
                        return Ok(ControlFlow::Break(AuthorityPendingSettlement::new(
                            failure,
                            SettlementOrigin::Completion,
                            execution,
                        )));
                    }
                },
                ResolutionEvaluation::Verify(verification) => {
                    return Ok(ControlFlow::Continue(
                        AuthorityComputeOutcome::Verification(AuthorityVerificationRequest {
                            request: verification.prepare(),
                            execution,
                        }),
                    ));
                }
                ResolutionEvaluation::Enrich(probe) => {
                    let prepared = match probe.prepare_enrichment() {
                        Ok(prepared) => prepared,
                        Err(failure) => {
                            return self.settle_resolution_failure(failure, execution);
                        }
                    };
                    let observed = {
                        let store = self.store.read();
                        prepared.observe(&store.authority)
                    };
                    match observed {
                        ResolutionProbeObservation::Retry(retry) => job = retry,
                        ResolutionProbeObservation::Missing(probe) => {
                            let settlement = match probe.settle_missing() {
                                Ok(settlement) => settlement,
                                Err(failure) => {
                                    return self.settle_resolution_failure(failure, execution);
                                }
                            };
                            match self.settle(settlement) {
                                Ok(()) => {
                                    drop(execution);
                                    return Ok(ControlFlow::Continue(
                                        AuthorityComputeOutcome::Settled,
                                    ));
                                }
                                Err(failure) => {
                                    return Ok(ControlFlow::Break(
                                        AuthorityPendingSettlement::new(
                                            failure,
                                            SettlementOrigin::Completion,
                                            execution,
                                        ),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn settle_resolution_failure(
        &self,
        failure: super::resolver::ResolutionExecutionFailure,
        execution: AuthorityComputeExecutionPermit,
    ) -> Result<
        ControlFlow<AuthorityPendingSettlement, AuthorityComputeOutcome>,
        AuthorityComputeError,
    > {
        let kind = failure.kind();
        match self.settle(failure.into_settlement()) {
            Ok(()) => {
                drop(execution);
                Err(AuthorityComputeError::Resolution(kind))
            }
            Err(failure) => Ok(ControlFlow::Break(AuthorityPendingSettlement::new(
                failure,
                SettlementOrigin::Resolution(kind),
                execution,
            ))),
        }
    }

    /// Execute one snapshot-bound tx-pool verification request and settle its
    /// exact capability before returning. The request already owns the result
    /// of its exact cache-key lookup, so callers cannot provide nearby cached
    /// evidence while the cache guard remains open across no await.
    pub(in crate::authority) async fn execute_verification(
        &self,
        request: AuthorityCacheBoundVerification,
        command_rx: Option<&mut watch::Receiver<ckb_script::ChunkCommand>>,
    ) -> Result<
        ControlFlow<AuthorityPendingSettlement, AuthorityVerificationOutcome>,
        AuthorityComputeError,
    > {
        let AuthorityCacheBoundVerification {
            request,
            execution: compute_execution,
        } = request;
        let verification = request.execute(command_rx).await;
        let cache_update = verification.cache_update;
        let cache_hit = verification.cache_hit;
        let result = self.settle(verification.settlement);
        match result {
            Ok(()) => {
                drop(compute_execution);
                Ok(ControlFlow::Continue(AuthorityVerificationOutcome {
                    cache_update,
                    cache_hit,
                }))
            }
            Err(failure) => Ok(ControlFlow::Break(AuthorityPendingSettlement::new(
                failure,
                SettlementOrigin::Completion,
                compute_execution,
            ))),
        }
    }

    /// Capture, validate and commit one bounded strongest-first Ready slice.
    /// Common independent candidates share one membership Apply. If any
    /// member has a special validation outcome, only the strongest owner is
    /// disposed and the next iteration captures a fresh coherent cut.
    pub(in crate::authority) fn try_drive_ready(
        &self,
    ) -> Result<AuthorityReadyOutcome, AuthorityDriverError> {
        let Some(work) = ({
            let store = self.store.read();
            store.capture_ready_work_batch()
        })
        .map_err(AuthorityDriverError::from_initial_ready_capture)?
        else {
            return Ok(AuthorityReadyOutcome::Idle);
        };
        let prepared = work
            .prepare()
            .map_err(AuthorityDriverError::from_ready_preparation)?;
        let batch = {
            let store = self.store.read();
            store.complete_ready_batch(prepared)
        }
        .map_err(AuthorityDriverError::from_ready_recheck)?;

        let disposition = batch
            .validate()
            .map_err(AuthorityDriverError::from_ready_validation)?;
        let (owners, committed) = {
            let mut store = self.store.write();
            match disposition {
                ReadyDisposition::Candidates(batch) => {
                    let plan = store
                        .authority
                        .plan_settlement(&batch)
                        .map_err(AuthorityDriverError::from_ready_plan)?;
                    match plan {
                        SettlementPlan::IndependentRun(plan) => {
                            let committed = plan.apply();
                            (committed.changed_owner_count(), committed)
                        }
                        SettlementPlan::CoupledComponent {
                            disposition,
                            reason: _,
                        } => (1, apply_candidate_disposition(disposition)),
                    }
                }
                ReadyDisposition::Head(outcome) => {
                    let plan = store
                        .authority
                        .plan_final_admission(outcome)
                        .map_err(AuthorityDriverError::from_ready_plan)?;
                    (1, apply_final_disposition(plan))
                }
            }
        };
        committed.publish_async_process_metrics();
        drop(committed);
        self.signals.publish_mutation();
        Ok(AuthorityReadyOutcome::Applied { owners })
    }
}

#[cfg(test)]
#[derive(Debug)]
pub(super) enum FoundationEffectQueueError {
    Build(super::effect::EffectBuildError),
    Plan(PlanError),
}

impl ReadyWorkBatch {
    fn prepare(self) -> Result<PreparedReadyValidationBatch, FinalAdmissionCaptureError> {
        let head = FinalAdmissionValidation::prepare(Arc::clone(&self.snapshot), self.head)
            .map_err(FinalAdmissionCaptureError::Validation)?;
        let mut tail = Vec::new();
        tail.try_reserve(self.tail.len())
            .map_err(|_| FinalAdmissionCaptureError::Allocation)?;
        let mut completed_tail = Vec::new();
        completed_tail
            .try_reserve(self.tail.len())
            .map_err(|_| FinalAdmissionCaptureError::Allocation)?;
        for work in self.tail {
            tail.push(
                FinalAdmissionValidation::prepare(Arc::clone(&self.snapshot), work)
                    .map_err(FinalAdmissionCaptureError::Validation)?,
            );
        }
        Ok(PreparedReadyValidationBatch {
            head,
            tail,
            completed_tail,
        })
    }
}

impl ReadyValidationBatch {
    fn validate(self) -> Result<ReadyDisposition, ReadyValidationError> {
        let head = self
            .head
            .validate()
            .map_err(ReadyValidationError::Candidate)?;
        let mut outcomes = Vec::new();
        outcomes
            .try_reserve(self.tail.len())
            .map_err(|_| ReadyValidationError::Allocation)?;
        for validation in self.tail {
            outcomes.push(
                validation
                    .validate()
                    .map_err(ReadyValidationError::Candidate)?,
            );
        }

        let head = match head {
            FinalAdmissionValidationOutcome::Candidate(head) => head,
            other @ (FinalAdmissionValidationOutcome::Rejected(_)
            | FinalAdmissionValidationOutcome::Reresolve(_)) => {
                return Ok(ReadyDisposition::Head(other));
            }
        };
        let head = IndependentCandidate::new(head);
        let mut tail = Vec::new();
        tail.try_reserve(outcomes.len())
            .map_err(|_| ReadyValidationError::Allocation)?;
        for outcome in outcomes {
            match outcome {
                FinalAdmissionValidationOutcome::Candidate(candidate) => {
                    tail.push(IndependentCandidate::new(candidate));
                }
                FinalAdmissionValidationOutcome::Rejected(_)
                | FinalAdmissionValidationOutcome::Reresolve(_) => {
                    // The strongest Ready owner remains the only admissible
                    // action from this cut. Drop weaker receipts and re-read
                    // after its disposition commits.
                    return Ok(ReadyDisposition::Head(
                        FinalAdmissionValidationOutcome::Candidate(head.into_receipt()),
                    ));
                }
            }
        }
        Ok(ReadyDisposition::Candidates(
            SettlementBatch::from_validated_ready(head, tail),
        ))
    }
}

fn apply_candidate_disposition(plan: CandidateDispositionPlan<'_>) -> CommittedDelta {
    match plan {
        CandidateDispositionPlan::Accepted(plan) => plan.apply(),
        CandidateDispositionPlan::Rejected(plan) => plan.apply().1,
    }
}

fn apply_final_disposition(plan: FinalAdmissionDispositionPlan<'_>) -> CommittedDelta {
    match plan {
        FinalAdmissionDispositionPlan::Candidate(plan) => apply_candidate_disposition(plan),
        FinalAdmissionDispositionPlan::ValidationRejected(plan) => plan.apply().1,
        FinalAdmissionDispositionPlan::Reresolve(plan) => plan.apply(),
    }
}

#[derive(Debug)]
pub(in crate::authority) enum FinalAdmissionCaptureError {
    Plan(PlanError),
    Validation(FinalAdmissionValidationError),
    Allocation,
}

impl AuthorityRelayParentReader {
    /// Capture one bounded page of the authoritative Remote missing-parent
    /// level. Scratch reservation happens before the authority guard and
    /// request compilation happens after it.
    pub(in crate::authority) fn page(
        &self,
        cursor: Option<RelayParentRebuildCursor>,
        scan_limit: NonZeroUsize,
    ) -> Result<RelayParentRebuildPage, RelayParentRebuildError> {
        let scratch = RelayParentRebuildScratch::try_new(scan_limit)?;
        let prepared = {
            let store = self.store.read();
            store
                .authority
                .read_view()
                .capture_relay_parent_rebuild(cursor, scan_limit, scratch)?
        };
        prepared.finish()
    }

    /// False is ordinary OCC staleness. The derived relayer projection must
    /// restart from the first page and retains no authority-side cursor.
    pub(in crate::authority) fn cut_is_current(&self, cut: &RelayParentRebuildCut) -> bool {
        self.store
            .read()
            .authority
            .read_view()
            .relay_parent_rebuild_cut_is_current(cut)
    }
}

impl AuthorityStore {
    fn from_runtime(
        runtime: AuthorityRuntimeConfig,
        snapshot: Arc<Snapshot>,
    ) -> Result<Self, RuntimeConfigError> {
        let chain_view = ChainViewId::new(ChainRevision(0), snapshot.tip_hash());
        let authority = TxPoolAuthority::from_runtime(
            runtime.resources,
            runtime.verify_order,
            runtime.effects,
            runtime.membership,
            chain_view,
        )
        .map_err(runtime_authority_config_error)?;
        Ok(Self {
            authority,
            snapshot,
            committed_txs_hash_cache: LruCache::new(COMMITTED_HASH_CACHE_SIZE),
        })
    }

    /// First OCC read: clone only the bounded Ready proof shells and paired
    /// snapshot. Per-cell overlay allocation happens after this guard opens.
    fn capture_ready_work_batch(
        &self,
    ) -> Result<Option<ReadyWorkBatch>, FinalAdmissionCaptureError> {
        let mut candidates = self.authority.ready_candidates().into_iter();
        let Some((head_key, head_expected)) = candidates.next() else {
            return Ok(None);
        };
        let head = self
            .authority
            .final_admission_work(&head_key, head_expected)
            .map_err(FinalAdmissionCaptureError::Plan)?;
        let mut tail = Vec::new();
        tail.try_reserve(candidates.len())
            .map_err(|_| FinalAdmissionCaptureError::Allocation)?;
        for (key, expected) in candidates {
            tail.push(
                self.authority
                    .final_admission_work(&key, expected)
                    .map_err(FinalAdmissionCaptureError::Plan)?,
            );
        }
        Ok(Some(ReadyWorkBatch {
            snapshot: Arc::clone(&self.snapshot),
            head,
            tail,
        }))
    }

    /// Second OCC read: recheck each exact Ready version and fill only the
    /// preallocated Accepted-origin bits. Any intervening mutation makes the
    /// capture stale rather than mixing two authority cuts.
    fn complete_ready_batch(
        &self,
        mut batch: PreparedReadyValidationBatch,
    ) -> Result<ReadyValidationBatch, FinalAdmissionCaptureError> {
        let head_work = self
            .authority
            .final_admission_work(batch.head.key(), batch.head.expected())
            .map_err(FinalAdmissionCaptureError::Plan)?;
        let head = batch
            .head
            .complete(AuthorityStoreCaptureSeal(()), &self.authority, head_work)
            .map_err(FinalAdmissionCaptureError::Validation)?;
        for prepared in batch.tail {
            let work = self
                .authority
                .final_admission_work(prepared.key(), prepared.expected())
                .map_err(FinalAdmissionCaptureError::Plan)?;
            batch.completed_tail.push(
                prepared
                    .complete(AuthorityStoreCaptureSeal(()), &self.authority, work)
                    .map_err(FinalAdmissionCaptureError::Validation)?,
            );
        }
        Ok(ReadyValidationBatch {
            head,
            tail: batch.completed_tail,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AuthorityAdministrationError, AuthorityComputeCheckout, AuthorityComputeError,
        AuthorityComputeJob, AuthorityComputeOutcome, AuthorityComputeSettlement,
        AuthorityDirectAdmissionError, AuthorityDirectRejectionExecution,
        AuthorityDirectResolutionOutcome, AuthorityDriverError, AuthorityReadyOutcome,
        AuthorityRuntime, AuthorityRuntimeConfig, FinalAdmissionCaptureError,
        PREACCEPTED_ENTRY_BYTES, PlanError, ReadyValidationError, RuntimeConfigError,
        SettlementOrigin, runtime_authority_config_error, runtime_resource_config_error,
    };
    use crate::authority::effect::{
        CommittedEffect, CommittedRejection, EffectBatchBound, EffectBatchBounds, EffectCapacity,
        EffectConfigError, EffectLimits, EffectPolicy, RejectionAudience,
    };
    use crate::authority::plan::{AuthorityConfigError, AuthorityFault, Backpressure, StalePlan};
    use crate::authority::resources::ResourceConfigError;
    use crate::authority::state::{
        ChainRevision, ChainViewId, OwnedTx, PreAcceptedPhase, QueuedWork, RejectionKind,
        ValidatedAdmission, VerifyCapability, WorkPermit,
    };
    use crate::authority::validation::FinalAdmissionValidationError;
    use ckb_app_config::{TxPoolConfig, VerifyOrdering};
    use ckb_async_runtime::Handle;
    use ckb_chain_spec::consensus::ConsensusBuilder;
    use ckb_network::PeerIndex;
    use ckb_script::ChunkCommand;
    use ckb_snapshot::Snapshot;
    use ckb_stop_handler::CancellationToken;
    use ckb_test_chain_utils::MockStore;
    use ckb_types::{
        U256,
        core::{FeeRate, TransactionBuilder},
        prelude::Unpack,
    };
    use std::ops::ControlFlow;
    use std::sync::Arc;
    use tokio::sync::{RwLock as TokioRwLock, mpsc, watch};

    fn runtime_config() -> TxPoolConfig {
        TxPoolConfig {
            max_tx_pool_size: 180_000_000,
            max_tx_pool_resident_size: 1_000_000_000,
            min_fee_rate: FeeRate::zero(),
            min_rbf_rate: FeeRate::zero(),
            max_tx_verify_cycles: 70_000_000,
            max_tx_verify_workers: 4,
            max_ancestors_count: 125,
            keep_rejected_tx_hashes_days: 1,
            keep_rejected_tx_hashes_count: 1_000,
            persisted_data: Default::default(),
            recent_reject: Default::default(),
            expiry_hours: 24,
            verify_ordering: VerifyOrdering::ArrivalTime,
            max_tx_pipeline_resident_size: 384_000_000,
        }
    }

    fn genesis_snapshot() -> Arc<Snapshot> {
        let consensus = Arc::new(ConsensusBuilder::default().build());
        let store = MockStore::default();
        let genesis = consensus.genesis_block();
        Arc::new(Snapshot::new(
            genesis.header(),
            U256::zero(),
            consensus.genesis_epoch_ext().clone(),
            store.store().get_snapshot(),
            Default::default(),
            consensus,
        ))
    }

    fn runtime() -> AuthorityRuntime {
        let snapshot = genesis_snapshot();
        AuthorityRuntime::new(&runtime_config(), snapshot.consensus(), snapshot.clone())
            .expect("the production authority runtime fixture is valid")
    }

    fn runtime_with_effect_limits(
        config: &TxPoolConfig,
        snapshot: Arc<Snapshot>,
        effects: EffectLimits,
    ) -> AuthorityRuntime {
        AuthorityRuntime::new_with_effect_limits_for_foundation(
            config,
            snapshot.consensus(),
            Arc::clone(&snapshot),
            effects,
        )
        .expect("the narrow effect runtime reserves every bounded projection")
    }

    fn queue_remote_rejection(runtime: &AuthorityRuntime, nonce: u32) {
        let publication = {
            let store = runtime.store.read();
            store
                .authority
                .effect_publication_for_foundation(
                    EffectPolicy::Remote,
                    vec![CommittedEffect::Rejected(CommittedRejection::Foundation {
                        tx: Arc::new(TransactionBuilder::default().version(nonce).build()),
                        audience: RejectionAudience::foundation(),
                        reason: RejectionKind::Policy,
                    })],
                )
                .expect("the runtime effect fixture is bounded")
        };
        let retirement = {
            let mut store = runtime.store.write();
            store
                .authority
                .plan_effect_publication_for_foundation(&publication)
                .expect("the runtime effect fixture fits its region")
                .apply()
        };
        drop(retirement);
        runtime.signals.publish_mutation();
    }

    fn admission(nonce: u32, peer: usize) -> ValidatedAdmission {
        ValidatedAdmission::remote(
            TransactionBuilder::default().version(nonce).build(),
            PeerIndex::from(peer),
        )
        .expect("the runtime fixture has valid ingress evidence")
    }

    fn retry(job: AuthorityComputeJob) -> AuthorityComputeSettlement {
        job.retry()
    }

    fn is_queued_resolve(runtime: &AuthorityRuntime, key: &super::super::state::RawTxHash) -> bool {
        let store = runtime.store.read();
        matches!(
            store.authority.entry(key),
            Some(OwnedTx::PreAccepted(entry))
                if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
        )
    }

    fn continued<T>(flow: ControlFlow<super::AuthorityPendingSettlement, T>) -> T {
        match flow {
            ControlFlow::Continue(value) => value,
            ControlFlow::Break(_) => panic!("the fixture has sufficient effect capacity"),
        }
    }

    #[test]
    fn runtime_checkout_observes_preexisting_level_without_a_wake_hint() {
        let runtime = runtime();
        let admission = admission(901, 91);
        let key = admission.identity.raw.clone();
        runtime.admit(admission).expect("admission commits");

        let job = continued(
            runtime
                .try_checkout_for_foundation(WorkPermit::ResolveOnly)
                .expect("checkout remains healthy"),
        )
        .expect("queued work is an authoritative level");
        assert!(matches!(
            runtime.settle_compute(retry(job), SettlementOrigin::Completion),
            ControlFlow::Continue(())
        ));
        assert!(is_queued_resolve(&runtime, &key));
    }

    #[test]
    fn runtime_stale_plan_disposition_depends_on_the_producer_boundary() {
        assert!(matches!(
            AuthorityDriverError::from_ready_plan(PlanError::Stale(StalePlan::Version)),
            AuthorityDriverError::Stale
        ));
        assert!(matches!(
            AuthorityDriverError::from_maintenance_plan(PlanError::Stale(StalePlan::Version)),
            AuthorityDriverError::Fault(AuthorityFault::MembershipProjection)
        ));
        assert!(matches!(
            AuthorityComputeError::from_checkout_plan(PlanError::Stale(StalePlan::Version)),
            AuthorityComputeError::Fault(AuthorityFault::SchedulerProjection)
        ));
        assert!(matches!(
            AuthorityDriverError::from_initial_ready_capture(FinalAdmissionCaptureError::Plan(
                PlanError::Stale(StalePlan::Version),
            )),
            AuthorityDriverError::Fault(AuthorityFault::SchedulerProjection)
        ));
        assert!(matches!(
            AuthorityDriverError::from_ready_preparation(FinalAdmissionCaptureError::Validation(
                FinalAdmissionValidationError::StaleView,
            ),),
            AuthorityDriverError::Fault(AuthorityFault::MembershipProjection)
        ));
        assert!(matches!(
            AuthorityDriverError::from_ready_recheck(FinalAdmissionCaptureError::Plan(
                PlanError::Stale(StalePlan::Version),
            )),
            AuthorityDriverError::Stale
        ));
        assert!(matches!(
            AuthorityDriverError::from_ready_validation(ReadyValidationError::Candidate(
                FinalAdmissionValidationError::StaleView,
            )),
            AuthorityDriverError::Fault(AuthorityFault::MembershipProjection)
        ));
        assert_eq!(
            AuthorityDirectAdmissionError::from_validation(
                FinalAdmissionValidationError::StaleView,
            ),
            AuthorityDirectAdmissionError::Stale
        );
        assert_eq!(
            AuthorityDirectAdmissionError::from_plan(PlanError::Backpressure(
                Backpressure::ProposalCollision,
            )),
            AuthorityDirectAdmissionError::ProposalCollision
        );
        assert_eq!(
            AuthorityDirectAdmissionError::from_plan(PlanError::Backpressure(
                Backpressure::EffectCapacity,
            )),
            AuthorityDirectAdmissionError::EffectCapacity
        );
        assert_eq!(
            AuthorityAdministrationError::from_plan(PlanError::Backpressure(
                Backpressure::Allocation,
            )),
            AuthorityAdministrationError::Allocation
        );
        assert_eq!(
            AuthorityAdministrationError::from_plan(PlanError::Backpressure(
                Backpressure::EffectCapacity,
            )),
            AuthorityAdministrationError::EffectCapacity
        );
        assert_eq!(
            AuthorityAdministrationError::from_plan(PlanError::Stale(StalePlan::Version)),
            AuthorityAdministrationError::Fault(AuthorityFault::MembershipProjection)
        );
    }

    #[test]
    fn runtime_resolution_uses_assembled_policy_and_settles_before_returning() {
        let runtime = runtime();
        let admission = admission(904, 94);
        let key = admission.identity.raw.clone();
        runtime.admit(admission).expect("admission commits");
        let job = continued(
            runtime
                .try_checkout_for_foundation(WorkPermit::ResolveOnly)
                .expect("checkout remains healthy"),
        )
        .expect("resolve work is ready");
        assert!(matches!(
            continued(
                runtime
                    .execute_compute(job)
                    .expect("the assembled zero-fee policy accepts this fixture")
            ),
            AuthorityComputeOutcome::Settled
        ));
        let store = runtime.store.read();
        assert!(matches!(
            store.authority.entry(&key),
            Some(OwnedTx::PreAccepted(entry))
                if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Verify(_)))
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn runtime_continuous_worker_and_ready_driver_close_one_owner_lifecycle() {
        let runtime = runtime();
        let admission = admission(905, 95);
        let key = admission.identity.raw.clone();
        runtime.admit(admission).expect("admission commits");
        let job = continued(
            runtime
                .try_checkout_for_foundation(WorkPermit::ResolveThenVerify(VerifyCapability::Any))
                .expect("checkout remains healthy"),
        )
        .expect("resolve work is ready");
        let AuthorityComputeOutcome::Verification(request) = continued(
            runtime
                .execute_compute(job)
                .expect("resolution continues under the same worker capability"),
        ) else {
            panic!("the empty-script fixture fits continuous verification")
        };
        let cache = ckb_verification::cache::init_cache();
        let verification = continued(
            runtime
                .execute_verification(request.bind_cache(&cache), None)
                .await
                .expect("verification settles Ready ownership"),
        );
        assert!(!verification.cache_hit);
        assert!(verification.cache_update.is_some());
        assert_eq!(
            runtime
                .try_drive_ready()
                .expect("the sealed Ready batch commits"),
            AuthorityReadyOutcome::Applied { owners: 1 }
        );
        let store = runtime.store.read();
        assert!(matches!(
            store.authority.entry(&key),
            Some(OwnedTx::Accepted(_))
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn runtime_shared_compute_gate_bounds_mixed_retained_and_direct_work() {
        let runtime = runtime();
        runtime
            .admit(admission(906, 96))
            .expect("the retained fixture enters the authority");
        let retained_execution = runtime
            .try_compute_execution_for_foundation()
            .expect("the retained worker acquires one shared slot");
        let retained = match runtime
            .try_checkout(WorkPermit::ResolveOnly, retained_execution)
            .expect("retained checkout remains healthy")
        {
            ControlFlow::Continue(AuthorityComputeCheckout::Job(job)) => job,
            ControlFlow::Continue(AuthorityComputeCheckout::Idle(execution)) => {
                drop(execution);
                panic!("the retained fixture has queued resolve work")
            }
            ControlFlow::Break(_) => panic!("the fixture has sufficient effect capacity"),
        };

        let direct_execution = runtime
            .try_compute_execution_for_foundation()
            .expect("direct work shares the same partition");
        let direct_tx = TransactionBuilder::default().version(1u32).build();
        let AuthorityDirectResolutionOutcome::Rejected(direct) = runtime
            .resolve_test_accept_transaction(&direct_tx, direct_execution)
            .expect("the direct fixture reaches a stable typed rejection")
        else {
            panic!("the non-zero version fixture must reject before verification")
        };

        let remaining = runtime.available_compute_permits_for_foundation();
        let mut holders = Vec::new();
        holders
            .try_reserve(remaining)
            .expect("the bounded test holder vector allocates");
        for _ in 0..remaining {
            holders.push(
                runtime
                    .try_compute_execution_for_foundation()
                    .expect("every remaining configured slot is obtainable exactly once"),
            );
        }
        assert_eq!(runtime.available_compute_permits_for_foundation(), 0);
        assert!(runtime.try_compute_execution_for_foundation().is_none());
        {
            let store = runtime.store.read();
            assert_eq!(store.authority.resources().preaccepted().active_work, 1);
        }

        let waiter = {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                let cancel = CancellationToken::new();
                runtime.acquire_compute_execution(&cancel).await
            })
        };
        tokio::task::yield_now().await;
        assert!(
            !waiter.is_finished(),
            "the next execution cannot start while retained and direct work saturate the gate"
        );
        let released = holders
            .pop()
            .expect("the fixture retained one spare holder");
        drop(released);
        let replacement = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("one released slot wakes exactly one waiter")
            .expect("the compute waiter task remains healthy")
            .expect("the compute waiter was not cancelled");

        assert!(matches!(
            runtime.settle_compute(retained.retry(), SettlementOrigin::Completion),
            ControlFlow::Continue(())
        ));
        assert!(matches!(
            runtime
                .settle_direct_transaction_rejection(direct)
                .expect("the direct TestAccept rejection settles read-only"),
            AuthorityDirectRejectionExecution::TestAccept(_)
        ));
        drop(holders);
        drop(replacement);
        assert_eq!(
            runtime.available_compute_permits_for_foundation(),
            runtime.verify_worker_count() + 1
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn runtime_sealed_worker_set_honors_pause_and_closes_the_owner_lifecycle() {
        let runtime = runtime();
        let handle = Handle::new(tokio::runtime::Handle::current(), None);
        let cache = Arc::new(TokioRwLock::new(ckb_verification::cache::init_cache()));
        let (cache_tx, mut cache_rx) = mpsc::channel(4);
        let (command_tx, command_rx) = watch::channel(ChunkCommand::Suspend);
        let cancel = CancellationToken::new();
        let handles = runtime
            .spawn_workers(&handle, cache, cache_tx, command_rx, cancel.clone())
            .expect("the validated worker topology reserves its handle vector");
        assert_eq!(
            handles
                .tasks
                .iter()
                .filter(|task| matches!(
                    task.role,
                    crate::authority::worker::AuthorityWorkerRole::Verifier(_)
                ))
                .count(),
            4
        );

        let admission = admission(906, 96);
        let key = admission.identity.raw.clone();
        runtime.admit(admission).expect("admission commits");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(is_queued_resolve(&runtime, &key));
        assert_eq!(
            runtime
                .store
                .read()
                .authority
                .resources()
                .preaccepted()
                .active_work,
            0,
            "a suspended topology must not check out a linear capability"
        );

        command_tx
            .send(ChunkCommand::Resume)
            .expect("the worker command authority remains live");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if matches!(
                    runtime.store.read().authority.entry(&key),
                    Some(OwnedTx::Accepted(_))
                ) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the sealed workers converge the transaction to Accepted");
        let update = tokio::time::timeout(std::time::Duration::from_secs(1), cache_rx.recv())
            .await
            .expect("the best-effort cache effect is not delayed")
            .expect("the cache receiver remains open");
        let expected_witness: [u8; 32] = TransactionBuilder::default()
            .version(906u32)
            .build()
            .witness_hash()
            .unpack();
        assert_eq!(update.key.witness_hash(), &expected_witness);

        cancel.cancel();
        for task in handles.tasks {
            task.handle
                .await
                .expect("authority worker task remains healthy")
                .expect("authority worker exits without a structural fault");
        }
        assert_eq!(
            runtime
                .store
                .read()
                .authority
                .resources()
                .preaccepted()
                .active_work,
            0,
            "structured cancellation cannot strand checked-out work"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn runtime_worker_retains_rejected_settlement_until_effect_capacity_returns() {
        const EFFECT_BYTES: usize = 1024 * 1024;
        let mut config = runtime_config();
        config.min_fee_rate = FeeRate::from_u64(1_000);
        let snapshot = genesis_snapshot();
        let effects = EffectLimits::partitioned(
            EffectCapacity::new(1, EFFECT_BYTES),
            EffectCapacity::new(1, EFFECT_BYTES),
            EffectCapacity::new(1, EFFECT_BYTES),
            EffectBatchBounds::new(
                EffectBatchBound::new(1, EFFECT_BYTES),
                EffectBatchBound::new(1, EFFECT_BYTES),
                EffectBatchBound::new(1, EFFECT_BYTES),
            ),
        )
        .expect("the narrow fixture admits one effect in each region");
        let runtime = runtime_with_effect_limits(&config, snapshot, effects);

        queue_remote_rejection(&runtime, 907);

        let handle = Handle::new(tokio::runtime::Handle::current(), None);
        let cache = Arc::new(TokioRwLock::new(ckb_verification::cache::init_cache()));
        let (cache_tx, _cache_rx) = mpsc::channel(1);
        let (_command_tx, command_rx) = watch::channel(ChunkCommand::Resume);
        let cancel = CancellationToken::new();
        let handles = runtime
            .spawn_workers(&handle, cache, cache_tx, command_rx, cancel.clone())
            .expect("the validated topology reserves its handle vector");

        let admission = admission(908, 98);
        let key = admission.identity.raw.clone();
        runtime.admit(admission).expect("admission commits");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if matches!(
                    runtime.store.read().authority.entry(&key),
                    Some(OwnedTx::PreAccepted(entry))
                        if matches!(entry.phase, PreAcceptedPhase::Computing(_))
                ) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the rejected settlement remains Computing while publication is full");

        let occupied_lease = runtime
            .wait_effect_checkout()
            .await
            .expect("effect checkout remains healthy")
            .expect("the occupied effect is queued");
        runtime
            .settle_effect(occupied_lease.complete_for_foundation().published())
            .expect("the occupied publication settles through the runtime facade");

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if runtime.store.read().authority.entry(&key).is_none() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the exact rejection commits after effect capacity returns");
        assert_eq!(
            runtime
                .store
                .read()
                .authority
                .resources()
                .preaccepted()
                .active_work,
            0
        );

        cancel.cancel();
        for task in handles.tasks {
            task.handle
                .await
                .expect("authority worker task remains healthy")
                .expect("authority worker exits without a structural fault");
        }
    }

    #[tokio::test]
    async fn runtime_effect_facade_retains_and_drains_a_closed_log_in_sequence() {
        let runtime = runtime();
        queue_remote_rejection(&runtime, 909);
        queue_remote_rejection(&runtime, 910);

        let first = runtime
            .wait_effect_checkout()
            .await
            .expect("the first checkout remains healthy")
            .expect("the first effect is committed");
        let first_sequence = first.sequence();
        runtime
            .close_effects()
            .expect("zero active compute permits effect close");
        assert!(!runtime.effects_closed_and_drained());
        assert_eq!(
            runtime.admit(admission(911, 99)).err(),
            Some(PlanError::EffectClosed),
            "closing the effect authority freezes new state producers"
        );

        runtime
            .settle_effect(first.retain())
            .expect("Retain returns the exact active capability to the head");
        let retained = runtime
            .wait_effect_checkout()
            .await
            .expect("retained checkout remains healthy")
            .expect("the retained head is still committed");
        assert_eq!(retained.sequence(), first_sequence);
        runtime
            .settle_effect(retained.complete_for_foundation().published())
            .expect("the retained head publishes exactly once");

        let second = runtime
            .wait_effect_checkout()
            .await
            .expect("the second checkout remains healthy")
            .expect("the second effect remains queued after close");
        assert!(second.sequence() > first_sequence);
        assert!(!runtime.effects_closed_and_drained());
        runtime
            .settle_effect(second.complete_for_foundation().circuit_disposed())
            .expect("a stable endpoint circuit may dispose its exact batch");

        assert!(
            runtime
                .wait_effect_checkout()
                .await
                .expect("the drained observation remains healthy")
                .is_none()
        );
        assert!(runtime.effects_closed_and_drained());
    }

    #[tokio::test]
    async fn runtime_effect_close_wakes_an_idle_level_waiter() {
        let runtime = runtime();
        let waiter = {
            let runtime = runtime.clone();
            tokio::spawn(async move { runtime.wait_effect_checkout().await })
        };
        tokio::task::yield_now().await;

        runtime
            .close_effects()
            .expect("an idle authority closes without a synthetic effect");
        let checkout = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("close cannot lose the idle publisher wake")
            .expect("the publisher task remains healthy")
            .expect("effect checkout remains healthy");
        assert!(checkout.is_none());
        assert!(runtime.effects_closed_and_drained());
    }

    #[tokio::test]
    async fn runtime_waiter_wakes_after_post_commit_admission_publication() {
        let runtime = runtime();
        let waiter = {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                let cancel = CancellationToken::new();
                runtime
                    .wait_checkout(WorkPermit::ResolveOnly, &cancel)
                    .await
            })
        };
        tokio::task::yield_now().await;

        runtime
            .admit(admission(902, 92))
            .expect("admission commits before publication");
        let job = continued(
            tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
                .await
                .expect("the post-commit wake cannot be lost")
                .expect("the waiter task remains healthy")
                .expect("the authority runtime remains healthy"),
        )
        .expect("the waiter was not cancelled");
        assert!(matches!(
            runtime.settle_compute(retry(job), SettlementOrigin::Completion),
            ControlFlow::Continue(())
        ));
    }

    #[test]
    fn runtime_capture_failure_requeues_before_returning_the_typed_error() {
        let runtime = runtime();
        let admission = admission(903, 93);
        let key = admission.identity.raw.clone();
        runtime.admit(admission).expect("admission commits");
        {
            let mut store = runtime.store.write();
            store.authority.force_chain_view(ChainViewId::new(
                ChainRevision(1),
                ckb_types::packed::Byte32::new([0x93; 32]),
            ));
        }

        assert!(matches!(
            runtime.try_checkout_for_foundation(WorkPermit::ResolveOnly),
            Err(AuthorityComputeError::Resolution(
                super::super::resolver::ResolutionExecutionKind::StaleView
            ))
        ));
        assert!(is_queued_resolve(&runtime, &key));
    }

    #[test]
    fn runtime_configuration_builds_every_authority_policy_together() {
        let config = runtime_config();
        let consensus = ConsensusBuilder::default().build();
        let runtime = AuthorityRuntimeConfig::from_runtime(&config, &consensus)
            .expect("the production fixture compiles into one authority policy");
        let limit = runtime.resources.preaccepted_limit_for_foundation();
        assert_eq!(
            limit.total_bytes(),
            Some(config.tx_pipeline_resident_size_budget()),
            "retained ownership and every simultaneous compute reservation share one physical ceiling"
        );
        assert!(limit.compute_bytes() > 0);
        assert!(limit.compute_edges() > 0);
        assert!(limit.bytes < config.tx_pipeline_resident_size_budget());
    }

    #[test]
    fn runtime_configuration_rejects_an_unusable_pipeline_budget() {
        let mut config = runtime_config();
        config.max_tx_pipeline_resident_size = PREACCEPTED_ENTRY_BYTES - 1;
        let consensus = ConsensusBuilder::default().build();
        assert_eq!(
            AuthorityRuntimeConfig::from_runtime(&config, &consensus).err(),
            Some(RuntimeConfigError::PipelineBudgetTooSmall)
        );
    }

    #[test]
    fn runtime_configuration_rejects_an_unusable_per_work_grant() {
        let mut config = runtime_config();
        config.max_tx_pipeline_resident_size = 1_000_000;
        config.max_tx_verify_workers = 10_000;
        let consensus = ConsensusBuilder::default().build();
        assert_eq!(
            AuthorityRuntimeConfig::from_runtime(&config, &consensus).err(),
            Some(RuntimeConfigError::PipelineBudgetTooSmall)
        );
    }

    #[test]
    fn runtime_configuration_rejects_effect_capacity_arithmetic_overflow() {
        let mut config = runtime_config();
        config.max_tx_pool_size = usize::MAX;
        config.max_tx_pool_resident_size = usize::MAX;
        let consensus = ConsensusBuilder::default().build();
        assert_eq!(
            AuthorityRuntimeConfig::from_runtime(&config, &consensus).err(),
            Some(RuntimeConfigError::Arithmetic)
        );
    }

    #[test]
    fn runtime_configuration_error_conversions_preserve_failure_domains() {
        assert_eq!(
            runtime_resource_config_error(ResourceConfigError::TransientComputeOverflow),
            RuntimeConfigError::Arithmetic
        );
        assert_eq!(
            runtime_resource_config_error(ResourceConfigError::LimitHierarchy),
            RuntimeConfigError::ResourceConfiguration
        );
        assert_eq!(
            runtime_authority_config_error(AuthorityConfigError::Effect(
                EffectConfigError::Arithmetic,
            )),
            RuntimeConfigError::Arithmetic
        );
        assert_eq!(
            runtime_authority_config_error(AuthorityConfigError::Effect(
                EffectConfigError::Allocation,
            )),
            RuntimeConfigError::AuthorityAllocation
        );
    }
}
