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
use super::{
    chain::{
        AcceptedValidityTransition, ChainValidationError, DirectAdmissionWork,
        FinalAdmissionPreparation, ProposalTransitionFacts,
    },
    chain_boundary::{
        ChainBoundaryError, ChainGenerationReplacement, ChainUpdateCommand, ChainUpdateFailure,
        CommittedChainUpdate,
    },
    effect::{
        EffectConfigError, EffectLimits, EffectProgressError, EffectPublicationObservation,
        EffectReceipt, EffectSettlement, EffectWakeTransition, EffectWork,
    },
    exchange::{AuthorityComputeExecutionPermit, ComputeWorkerGrant, ComputeWorkerSlot},
    ingress::{
        DirectCommand, DirectIngressTransaction, DirectTransaction, RetainedAdmissionBatch,
        RetainedIngressAttempt, direct,
    },
    plan::{
        AuthorityConfigError, AuthorityFault, AuthorityPostCommit, AuthorityWakeTransition,
        Backpressure, CommittedComputeExchange, CommittedDelta, ComputeExchangeAssignment,
        ComputeExchangeCompletion, ComputeExchangeDeferred, ComputeExchangePlanFailure,
        ComputeExchangeSettled, ComputeSettlementFailure, ConcurrentIndependentError,
        ConcurrentRetainedIngressError, CoupledSettlementContinuation, DirectAdmissionEvaluation,
        EffectCloseError, EffectSettlementCommit, EffectSettlementFailure,
        GenerationReplacementPlanError, IndependentCandidate, MembershipConfig, MembershipReject,
        PlanError, ReadyHeadCommitOutcome, ReadyJobCommitOutcome, RecoveredComputeExchange,
        SettlementBatch, SharedComputeExchangeOutcome, SharedComputeSettlementOutcome,
        SharedDirectAdmissionCommitOutcome, SharedDirectRejectionTerminalOutcome,
        SharedIndependentSettlementCompilation, SharedReadyWaveCompilation,
        SharedRetainedIngressHead, TxPoolAuthority,
    },
    query::{
        AcceptedTransactionsWithCycles, AuthorityPoolSummary, AuthorityQueryError,
        AuthorityQueryScratch, AuthorityTransactionLookup, AuthorityTransactionStatusLookup,
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
        ResolutionProbeObservation, ResolutionReceiptDefect, TxPoolVerificationRequest,
        VerificationCacheUpdate, VerificationJob, VerificationTimePolicy,
        VerificationTimePolicyError,
    },
    resources::{
        AcceptedResources, ComputeLimits, ResidencyPolicy, ResourceCapacityWaitIdentity,
        ResourceConfigError, ResourceLimits, ResourceVector,
    },
    scheduler::{ReadyReservation, VerifyOrder},
    state::{
        AcceptedAtMillis, AcceptedStatus, ApplySequence, ChainRevision, ChainViewId,
        InputEvidenceError, PayloadPolicy, RawTxHash, RemoteDeadline,
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
use crate::{constants::MAX_READY_BATCH, util::TxPoolVerificationBudget};
use ckb_app_config::{TxPoolConfig, VerifyOrdering};
use ckb_chain_spec::consensus::Consensus;
use ckb_logger::{error, warn};
use ckb_script::{InitialProgramLoadLimit, TxPoolVmExecutionMode};
use ckb_snapshot::Snapshot;
use ckb_stop_handler::CancellationToken;
use ckb_types::core::{EntryCompleted, FeeRate, TransactionView};
use ckb_types::packed::{Byte32, OutPoint, ProposalShortId};
use ckb_util::{
    Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard, parking_lot::RwLockUpgradableReadGuard,
};
use ckb_verification::cache::ScriptVerificationRules;
use lru::LruCache;
use std::{
    num::NonZeroUsize,
    ops::ControlFlow,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(super) struct RetainedIngressBatchFailure {
    reason: RetainedIngressBatchFailureReason,
    batch: RetainedAdmissionBatch,
}

pub(super) enum RetainedIngressBatchFailureReason {
    Plan(PlanError),
    SharedContention,
}

impl RetainedIngressBatchFailure {
    fn plan(error: PlanError, batch: RetainedAdmissionBatch) -> Self {
        Self {
            reason: RetainedIngressBatchFailureReason::Plan(error),
            batch,
        }
    }

    fn shared_contention(batch: RetainedAdmissionBatch) -> Self {
        Self {
            reason: RetainedIngressBatchFailureReason::SharedContention,
            batch,
        }
    }

    pub(super) fn into_parts(self) -> (RetainedIngressBatchFailureReason, RetainedAdmissionBatch) {
        (self.reason, self.batch)
    }
}
use tokio::{
    runtime::RuntimeFlavor,
    sync::{Notify, Semaphore, watch},
};

const PREACCEPTED_ENTRY_BYTES: usize = 768;
const DEPENDENCY_EDGE_BYTES: usize = 160;
// One primary/proposal/source/deadline/scheduler envelope and the maximum
// dependency-index multiplicity used by the conservative retained-entry
// charge formula.
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

/// Derive the only accepted-validity transition from primitive chain facts.
/// Rules changes dominate a simultaneous detach because every retained script
/// proof then leaves its validity domain.
pub(super) fn accepted_validity_transition(
    old_rules: ScriptVerificationRules,
    new_rules: ScriptVerificationRules,
    had_detached_chain: bool,
) -> AcceptedValidityTransition {
    if old_rules != new_rules {
        AcceptedValidityTransition::RulesChanged
    } else if had_detached_chain {
        AcceptedValidityTransition::ContextChanged
    } else {
        AcceptedValidityTransition::Preserved
    }
}

/// One configured pipeline budget is split into retained ownership and
/// transient compute reservations. The split is a capacity policy, not two
/// independently consumable budgets: their checked sum remains exactly the
/// configured ceiling.
const COMPUTE_RESOURCE_DIVISOR: usize = 4;
const COMMITTED_HASH_CACHE_SIZE: usize = 100_000;
const ADMIN_MAINTENANCE_SLICE: usize = 32;
/// Bound one dependency maintenance authority hold without turning the
/// level-triggered dirty cursor into a second queue. Each element remains one
/// existing Plan/Apply step and publishes only after the guard is released.
const DEPENDENCY_MAINTENANCE_APPLY_BATCH: usize = 8;
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
    VerificationTimeConfiguration,
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
    verification_time: VerificationTimePolicy,
    initial_load_limit: InitialProgramLoadLimit,
    expiry_policy: ExpiryPolicy,
    verify_workers: NonZeroUsize,
    transient_compute_permits: NonZeroUsize,
    vm_execution_mode: TxPoolVmExecutionMode,
    full_query_max_rows: usize,
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
    #[cfg(test)]
    fn from_runtime(
        config: &TxPoolConfig,
        consensus: &Consensus,
    ) -> Result<Self, RuntimeConfigError> {
        Self::from_runtime_on_executor(config, consensus, None)
    }

    fn from_runtime_with_handle(
        config: &TxPoolConfig,
        consensus: &Consensus,
        handle: &ckb_async_runtime::Handle,
    ) -> Result<Self, RuntimeConfigError> {
        Self::from_runtime_on_executor(config, consensus, Some(handle))
    }

    fn from_runtime_on_executor(
        config: &TxPoolConfig,
        consensus: &Consensus,
        handle: Option<&ckb_async_runtime::Handle>,
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
        let max_block_bytes = usize::try_from(consensus.max_block_bytes())
            .map_err(|_| RuntimeConfigError::Arithmetic)?;
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
        let (transient_compute_permits, vm_execution_mode) = match handle {
            Some(handle) => {
                let tokio_handle = handle.clone().into_inner();
                if tokio_handle.runtime_flavor() != RuntimeFlavor::MultiThread {
                    return Err(RuntimeConfigError::ResourceConfiguration);
                }
                let runtime_workers = tokio_handle.metrics().num_workers().max(1);
                let maximum_inline_compute = runtime_workers.saturating_sub(1);
                let effective_compute = active_work.min(maximum_inline_compute).max(1);
                if effective_compute < active_work {
                    warn!(
                        "tx-pool compute concurrency {} exceeds the {}-worker Tokio runtime's parent-progress bound; limiting active compute to {}",
                        active_work, runtime_workers, effective_compute
                    );
                }
                (
                    NonZeroUsize::new(effective_compute).ok_or(RuntimeConfigError::Arithmetic)?,
                    if runtime_workers == 1 {
                        TxPoolVmExecutionMode::YieldRuntimeWorker
                    } else {
                        TxPoolVmExecutionMode::Inline
                    },
                )
            }
            None => (
                NonZeroUsize::new(active_work).ok_or(RuntimeConfigError::Arithmetic)?,
                TxPoolVmExecutionMode::Inline,
            ),
        };

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

        let desired_expanded_edges = max_block_bytes
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
            max_block_bytes,
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
        let full_query_max_rows = resources
            .max_owner_entries()
            .ok_or(RuntimeConfigError::Arithmetic)?;
        let verification_time = VerificationTimePolicy::from_runtime(
            config.min_tx_verify_time_ms,
            config.tx_verify_cycles_per_ms,
            config.max_tx_verify_time_ms,
        )
        .map_err(|error| match error {
            VerificationTimePolicyError::ZeroCycleRate
            | VerificationTimePolicyError::InvalidDurationRange => {
                RuntimeConfigError::VerificationTimeConfiguration
            }
        })?;
        let initial_load_limit =
            InitialProgramLoadLimit::new(config.max_tx_verify_initial_load_bytes)
                .ok_or(RuntimeConfigError::VerificationTimeConfiguration)?;

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
            verification_time,
            initial_load_limit,
            expiry_policy: ExpiryPolicy {
                accepted_residency_millis,
                remote_slice,
            },
            verify_workers,
            transient_compute_permits,
            vm_execution_mode,
            full_query_max_rows,
        })
    }
}

#[cfg(test)]
impl AuthorityRuntimeConfig {
    pub(in crate::authority) fn executor_shape_for_test(&self) -> (usize, TxPoolVmExecutionMode) {
        (self.transient_compute_permits.get(), self.vm_execution_mode)
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

/// Generation/chain barrier around the production tx-pool.
///
/// `snapshot` is chain evidence, not a second transaction owner.  It is kept
/// beside the kernel so a caller cannot publish a new snapshot under an old
/// `ChainViewId`, or vice versa. Ordinary production mutation holds the shared
/// side while compiling and committing exact shard cuts; it never falls back
/// to the outer write side. Generation replacement is exclusive. At the
/// terminal-audit base, chain reconciliation captures under an upgradable read
/// before upgrading; that shape does not by itself exclude ordinary readers
/// and must not be cited as proof that its causal receipt is stable. The
/// compact-block cache is non-authoritative chain-administration metadata.
pub(crate) struct AuthorityStore {
    authority: TxPoolAuthority,
    snapshot: Arc<Snapshot>,
    /// Rebuildable compact-block projection. Generation replacement checks the
    /// cache out under the authority guard, clears its potentially large
    /// contents after the guard opens, and returns the same allocation. A
    /// temporary `None` is an ordinary cache miss, never policy state.
    committed_txs_hash_cache: Option<LruCache<ProposalShortId, Byte32>>,
}

/// The generation barrier and its centralized profiling boundary.
///
/// The default build returns the native parking-lot guards directly. The
/// profiling build wraps each guard only long enough to close its hold span
/// after the guard has released, so trace formatting never extends the
/// measured critical section. Keeping acquisition here prevents manual span
/// sites from drifting away from newly added authority reads or writes.
#[repr(transparent)]
struct AuthorityStoreLock {
    inner: RwLock<AuthorityStore>,
}

#[cfg(not(feature = "profiling"))]
type AuthorityStoreGuard<G> = G;

#[cfg(feature = "profiling")]
struct AuthorityStoreGuard<G> {
    // Rust drops fields in declaration order: release the authority lock
    // before closing the hold span and writing its trace record.
    guard: G,
    _hold_span: tracing::span::EnteredSpan,
}

#[cfg(feature = "profiling")]
impl<G: std::ops::Deref> std::ops::Deref for AuthorityStoreGuard<G> {
    type Target = G::Target;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

#[cfg(feature = "profiling")]
impl<G: std::ops::DerefMut> std::ops::DerefMut for AuthorityStoreGuard<G> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

/// Opaque proof that one owned authority read guard was consumed before its
/// terminal escaped. Keeping the constructor in this child module prevents a
/// caller from manufacturing publication authority from an ordinary value.
mod store_release {
    use super::{AuthorityStore, AuthorityStoreLock};

    #[must_use = "a released authority terminal must be consumed"]
    pub(super) struct Released<T>(T);

    pub(super) fn with_read<T, F>(lock: &AuthorityStoreLock, capture: F) -> Released<T>
    where
        F: for<'guard> FnOnce(&'guard AuthorityStore) -> T,
    {
        let terminal = {
            let guard = lock.read();
            capture(&guard)
        };
        Released(terminal)
    }

    impl<T> Released<T> {
        pub(super) fn into_inner(self) -> T {
            self.0
        }
    }
}

use store_release::Released;

impl AuthorityStoreLock {
    fn new(store: AuthorityStore) -> Self {
        Self {
            inner: RwLock::new(store),
        }
    }

    #[inline]
    fn read(&self) -> AuthorityStoreGuard<RwLockReadGuard<'_, AuthorityStore>> {
        #[cfg(not(feature = "profiling"))]
        {
            self.inner.read()
        }
        #[cfg(feature = "profiling")]
        {
            let wait_span = tracing::trace_span!(
                target: "ckb_tx_pool_profile",
                "tx_pool.authority.read_wait"
            )
            .entered();
            let guard = self.inner.read();
            drop(wait_span);
            AuthorityStoreGuard {
                guard,
                _hold_span: tracing::trace_span!(
                    target: "ckb_tx_pool_profile",
                    "tx_pool.authority.read_hold"
                )
                .entered(),
            }
        }
    }

    /// Produce an owned terminal under one exact read guard, then consume the
    /// guard before the opaque [`Released`] value escapes. The higher-ranked
    /// closure cannot return a value borrowing the guarded store. It does not
    /// constrain closure side effects; callers must still return any wake or
    /// publication capability and perform the observable action after release.
    #[inline]
    fn with_read_released<T, F>(&self, capture: F) -> Released<T>
    where
        F: for<'guard> FnOnce(&'guard AuthorityStore) -> T,
    {
        store_release::with_read(self, capture)
    }

    #[inline]
    fn write(&self) -> AuthorityStoreGuard<RwLockWriteGuard<'_, AuthorityStore>> {
        #[cfg(not(feature = "profiling"))]
        {
            self.inner.write()
        }
        #[cfg(feature = "profiling")]
        {
            let wait_span = tracing::trace_span!(
                target: "ckb_tx_pool_profile",
                "tx_pool.authority.write_wait"
            )
            .entered();
            let guard = self.inner.write();
            drop(wait_span);
            AuthorityStoreGuard {
                guard,
                _hold_span: tracing::trace_span!(
                    target: "ckb_tx_pool_profile",
                    "tx_pool.authority.write_hold"
                )
                .entered(),
            }
        }
    }

    #[inline]
    fn upgradable_read(
        &self,
    ) -> AuthorityStoreGuard<RwLockUpgradableReadGuard<'_, AuthorityStore>> {
        #[cfg(not(feature = "profiling"))]
        {
            self.inner.upgradable_read()
        }
        #[cfg(feature = "profiling")]
        {
            let wait_span = tracing::trace_span!(
                target: "ckb_tx_pool_profile",
                "tx_pool.authority.upgradable_read_wait"
            )
            .entered();
            let guard = self.inner.upgradable_read();
            drop(wait_span);
            AuthorityStoreGuard {
                guard,
                _hold_span: tracing::trace_span!(
                    target: "ckb_tx_pool_profile",
                    "tx_pool.authority.upgradable_read_hold"
                )
                .entered(),
            }
        }
    }

    #[inline]
    fn upgrade(
        store: AuthorityStoreGuard<RwLockUpgradableReadGuard<'_, AuthorityStore>>,
    ) -> AuthorityStoreGuard<RwLockWriteGuard<'_, AuthorityStore>> {
        #[cfg(not(feature = "profiling"))]
        {
            RwLockUpgradableReadGuard::upgrade(store)
        }
        #[cfg(feature = "profiling")]
        {
            let AuthorityStoreGuard { guard, _hold_span } = store;
            drop(_hold_span);
            let wait_span = tracing::trace_span!(
                target: "ckb_tx_pool_profile",
                "tx_pool.authority.upgrade_wait"
            )
            .entered();
            let guard = RwLockUpgradableReadGuard::upgrade(guard);
            drop(wait_span);
            AuthorityStoreGuard {
                guard,
                _hold_span: tracing::trace_span!(
                    target: "ckb_tx_pool_profile",
                    "tx_pool.authority.write_hold"
                )
                .entered(),
            }
        }
    }

    /// Close the shared Direct transaction-rejection terminal entirely inside
    /// this lock owner. The returned outcome cannot borrow the guard, so every
    /// runtime publication necessarily occurs after this method releases it.
    fn commit_direct_transaction_rejection(
        &self,
        rejection: DirectTransactionRejection,
    ) -> Result<SharedDirectRejectionTerminalOutcome, PlanError> {
        let store = self.read();
        Ok(store
            .authority
            .plan_shared_direct_transaction_rejection(rejection)?
            .apply())
    }

    /// The final-validation producer shares the same sealed effect-only
    /// terminal and the same structural guard-release boundary.
    fn commit_direct_validation_rejection(
        &self,
        rejection: super::chain::DirectAdmissionRejection,
    ) -> Result<SharedDirectRejectionTerminalOutcome, PlanError> {
        let store = self.read();
        Ok(store
            .authority
            .plan_shared_direct_validation_rejection(rejection)?
            .apply())
    }
}

/// Read-only capability for rebuilding the relayer's missing-parent level.
///
/// The relayer receives this projection reader rather than an
/// [`AuthorityRuntime`], so it cannot acquire admission, administration, or
/// settlement authority. The store remains the sole owner and every page is
/// bound to an exact source cut.
pub(in crate::authority) struct AuthorityRelayParentReader {
    store: Arc<AuthorityStoreLock>,
}

/// Lossy wake hints around the one authoritative scheduler. A hint carries no
/// queue state: every waiter first attempts capability-aware checkout under
/// the store guard, and subscribes before that attempt so a concurrent Apply
/// cannot be missed.
struct AuthoritySignals {
    compute: Notify,
    ready: Notify,
    maintenance: Notify,
    effect_publisher: Notify,
    effect_capacity: Notify,
    template: Notify,
    effect_publisher_running: AtomicBool,
    #[cfg(test)]
    post_commit: Notify,
}

#[derive(Default)]
struct AuthorityLifecycleFenceState {
    lifecycle_writers_waiting: usize,
    lifecycle_writer_active: bool,
}

#[derive(Default)]
struct AuthorityLifecycleFence {
    state: Mutex<AuthorityLifecycleFenceState>,
    changed: Notify,
}

struct AuthorityLifecycleWriterWaiter {
    fence: Arc<AuthorityLifecycleFence>,
    registered: bool,
}

struct AuthorityLifecycleWriterPermit {
    fence: Arc<AuthorityLifecycleFence>,
    active: bool,
}

impl AuthorityLifecycleFence {
    #[expect(
        clippy::expect_used,
        reason = "every waiting lifecycle call owns a live task/stack; exhausting usize waiters is impossible before process allocation is exhausted, and this exact increment/decrement remains paired under the mutex"
    )]
    async fn acquire_writer(self: &Arc<Self>) -> AuthorityLifecycleWriterPermit {
        {
            let mut state = self.state.lock();
            state.lifecycle_writers_waiting = state
                .lifecycle_writers_waiting
                .checked_add(1)
                .expect("the fixed authority topology bounds lifecycle writers");
        }
        let mut waiter = AuthorityLifecycleWriterWaiter {
            fence: Arc::clone(self),
            registered: true,
        };
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            let _ = changed.as_mut().enable();
            let acquired = {
                let mut state = self.state.lock();
                if state.lifecycle_writer_active {
                    false
                } else {
                    state.lifecycle_writers_waiting = state
                        .lifecycle_writers_waiting
                        .checked_sub(1)
                        .expect("this writer registered exactly one waiter");
                    state.lifecycle_writer_active = true;
                    waiter.registered = false;
                    true
                }
            };
            if acquired {
                return AuthorityLifecycleWriterPermit {
                    fence: Arc::clone(self),
                    active: true,
                };
            }
            changed.await;
        }
    }
}

impl Drop for AuthorityLifecycleWriterWaiter {
    #[expect(
        clippy::expect_used,
        reason = "a registered waiter increments the exact counter once and cannot be duplicated"
    )]
    fn drop(&mut self) {
        if !self.registered {
            return;
        }
        let mut state = self.fence.state.lock();
        state.lifecycle_writers_waiting = state
            .lifecycle_writers_waiting
            .checked_sub(1)
            .expect("a registered lifecycle waiter owns one counter unit");
        self.registered = false;
        drop(state);
        self.fence.changed.notify_waiters();
    }
}

impl Drop for AuthorityLifecycleWriterPermit {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mut state = self.fence.state.lock();
        state.lifecycle_writer_active = false;
        self.active = false;
        drop(state);
        self.fence.changed.notify_waiters();
    }
}

impl AuthoritySignals {
    fn new() -> Self {
        Self {
            compute: Notify::new(),
            ready: Notify::new(),
            maintenance: Notify::new(),
            effect_publisher: Notify::new(),
            effect_capacity: Notify::new(),
            template: Notify::new(),
            effect_publisher_running: AtomicBool::new(false),
            #[cfg(test)]
            post_commit: Notify::new(),
        }
    }

    #[must_use = "dependency poison must be forwarded after publishing the committed wake"]
    fn publish_post_commit(&self, post_commit: AuthorityPostCommit) -> Option<AuthorityFault> {
        let (wake, post_commit_fault) = post_commit.publish_metrics_and_take_wake();
        self.publish_wake(wake);
        #[cfg(test)]
        self.post_commit.notify_waiters();
        post_commit_fault
    }

    fn publish_wake(&self, wake: AuthorityWakeTransition) {
        if wake.compute_advanced() {
            // The coordinator is the only compute feeder. One coalesced level
            // is sufficient because its bounded role probe rechecks the
            // authoritative scheduler wave; the signal carries no lane or
            // transaction decision.
            self.compute.notify_one();
        }
        if wake.ready_advanced() {
            self.ready.notify_one();
        }
        if wake.dependency_maintenance_activated() {
            self.maintenance.notify_one();
        }
        if wake.effect_publisher_advanced() {
            self.effect_publisher.notify_one();
        }
        if wake.effect_capacity_released() {
            // Waiters carry heterogeneous effect classes and batch sizes. A
            // wake-one protocol cannot prove that its selected waiter fits the
            // released region while another waiter would make progress.
            self.effect_capacity.notify_waiters();
        }
        if wake.owner_source_advanced() {
            // The five existing lanes retain independent source-cut OCC and
            // publication guards. This is only their common lossy prompt.
            self.template.notify_waiters();
        }
    }

    fn publish_effect_wake(&self, wake: EffectWakeTransition) {
        if wake.publisher_advanced() {
            self.effect_publisher.notify_one();
        }
        if wake.capacity_released() {
            self.effect_capacity.notify_waiters();
        }
    }
}

/// Move-only claim for the sole consumer of the authority effect sequence.
/// The claim is the only publication-exclusivity authority: the effect log
/// retains each committed record in place until settlement and carries no
/// second in-flight flag or active location.
pub(in crate::authority) struct AuthorityEffectPublisherClaim {
    signals: Arc<AuthoritySignals>,
}

/// Read-only committed effect evidence tied to the sole publisher capability.
/// The mutable claim borrow makes a second concurrent receipt unrepresentable
/// while the first receipt or its cancellation guard remains live.
pub(in crate::authority) struct AuthorityEffectPublicationLease<'runtime, 'claim> {
    runtime: &'runtime AuthorityRuntime,
    receipt: Option<EffectReceipt>,
    _claim: &'claim mut AuthorityEffectPublisherClaim,
}

#[derive(Debug)]
pub(in crate::authority) enum AuthorityEffectPublicationFault {
    Settlement(
        #[expect(
            dead_code,
            reason = "the exact linear settlement capability is retained through generation shutdown"
        )]
        EffectSettlementFailure,
    ),
    Progress(
        #[expect(
            dead_code,
            reason = "the exact progress fault is rendered by the operational Debug boundary"
        )]
        EffectProgressError,
    ),
}

#[derive(Clone, Copy)]
enum EffectTerminalDisposition {
    Published,
    CircuitDisposed,
}

impl AuthorityEffectPublicationLease<'_, '_> {
    pub(in crate::authority) fn current(&self) -> Option<EffectWork<'_>> {
        self.receipt.as_ref().and_then(EffectReceipt::current)
    }

    pub(in crate::authority) fn mark_current_processed(
        &mut self,
    ) -> Result<bool, EffectProgressError> {
        self.receipt
            .as_mut()
            .ok_or(EffectProgressError::Incomplete)?
            .mark_current_processed()
    }

    pub(in crate::authority) fn publish(
        self,
    ) -> Result<EffectPublicationObservation, AuthorityEffectPublicationFault> {
        self.settle(EffectTerminalDisposition::Published)
    }

    pub(in crate::authority) fn circuit_dispose(
        self,
    ) -> Result<EffectPublicationObservation, AuthorityEffectPublicationFault> {
        self.settle(EffectTerminalDisposition::CircuitDisposed)
    }

    fn settle(
        mut self,
        disposition: EffectTerminalDisposition,
    ) -> Result<EffectPublicationObservation, AuthorityEffectPublicationFault> {
        let receipt = self
            .receipt
            .take()
            .ok_or(AuthorityEffectPublicationFault::Progress(
                EffectProgressError::Incomplete,
            ))?;
        let completed = match receipt.into_complete() {
            Ok(completed) => completed,
            Err(failure) => {
                let (error, receipt) = failure.into_parts();
                self.receipt = Some(receipt);
                return Err(AuthorityEffectPublicationFault::Progress(error));
            }
        };
        let settlement = match disposition {
            EffectTerminalDisposition::Published => completed.published(),
            EffectTerminalDisposition::CircuitDisposed => completed.circuit_disposed(),
        };
        self.runtime
            .settle_effect(settlement)
            .map_err(AuthorityEffectPublicationFault::Settlement)
    }
}

impl Drop for AuthorityEffectPublicationLease<'_, '_> {
    fn drop(&mut self) {
        let Some(receipt) = self.receipt.take() else {
            return;
        };
        if let Err(failure) = self.runtime.settle_effect(receipt.retain()) {
            error!(
                "failed to retain cancelled tx-pool effect publication: {:?}",
                failure.error()
            );
        }
    }
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
    store: Arc<AuthorityStoreLock>,
    signals: Arc<AuthoritySignals>,
    lifecycle_fence: Arc<AuthorityLifecycleFence>,
    resolution_policy: ResolutionPolicy,
    expiry_policy: ExpiryPolicy,
    verify_workers: NonZeroUsize,
    transient_compute: ComputeGate,
    vm_execution_mode: TxPoolVmExecutionMode,
    initial_load_limit: InitialProgramLoadLimit,
    full_query: AuthorityQueryScratch,
    #[cfg(test)]
    template_captures: Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    ready_commit_probes: Arc<[Mutex<Option<Arc<super::shard::ConcurrentRemovalProbe>>>; 2]>,
}

/// Private count-only execution gate. No close operation is exposed: service
/// shutdown cancels waiters, so Tokio's semaphore-close state cannot become a
/// third runtime outcome or a background-lane quarantine protocol.
#[derive(Clone)]
struct ComputeGate {
    permits: Arc<Semaphore>,
    released: Arc<Notify>,
    verification_time: VerificationTimePolicy,
}

impl ComputeGate {
    fn new(permits: NonZeroUsize, verification_time: VerificationTimePolicy) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(permits.get())),
            released: Arc::new(Notify::new()),
            verification_time,
        }
    }

    async fn acquire(&self, cancel: &CancellationToken) -> Option<AuthorityComputeExecutionPermit> {
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
                    Some(AuthorityComputeExecutionPermit::new(
                        permit,
                        Arc::clone(&self.released),
                    ))
                }
            }
        }
    }

    fn try_acquire(&self) -> Option<AuthorityComputeExecutionPermit> {
        Arc::clone(&self.permits)
            .try_acquire_owned()
            .ok()
            .map(|permit| AuthorityComputeExecutionPermit::new(permit, Arc::clone(&self.released)))
    }

    fn release_signal(&self) -> &Notify {
        &self.released
    }

    fn verification_time(&self) -> VerificationTimePolicy {
        self.verification_time
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use = "maintenance progress determines whether another bounded step is useful"]
pub(super) enum AuthorityMaintenanceOutcome {
    Idle,
    Applied,
}

/// Closed progress contract shared by authority-owned background drivers.
///
/// A driver may retry only after an observed stale cut or effect-capacity
/// publication. Allocation has no authority-owned monotonic releaser and is
/// consumed by the no-retry generation terminal before a worker reports
/// progress. Its prepared carrier exposes no recoverable allocation edge and
/// Apply performs only a fixed 64-shard payload swap. Every other producer outcome is translated at the authority
/// boundary into a structural fault, so adding a broad `PlanError` variant
/// cannot silently create a retry loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) enum AuthorityDriverError {
    Stale,
    Allocation,
    EffectCapacity,
    LifecycleClosed,
    Fault(AuthorityFault),
}

/// Closed result for synchronous administrative commands. Exclusive commands
/// still classify stale plans as structural; public local removal carries an
/// exact shared OCC cut and reports a lost cut as `CompetingProgress` without
/// retry. Allocator pressure is an ordinary returned service outcome, while
/// only effect-capacity pressure may wait on its named publisher releaser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AuthorityAdministrationError {
    Allocation,
    EffectCapacity,
    LifecycleClosed,
    CompetingProgress,
    Fault(AuthorityFault),
}

/// Closed failure surface of generation replacement after its fixed carrier
/// has been constructed.
///
/// Allocation is intentionally absent: admitting it here would recreate the
/// unchanged-cut retry problem that this transition terminates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) enum AuthorityGenerationReplacementError {
    LifecycleClosed,
    Fault(AuthorityFault),
}

impl AuthorityGenerationReplacementError {
    fn from_plan(error: GenerationReplacementPlanError) -> Self {
        match error {
            GenerationReplacementPlanError::LifecycleClosed => Self::LifecycleClosed,
            GenerationReplacementPlanError::Fault(fault) => Self::Fault(fault),
        }
    }
}

impl AuthorityAdministrationError {
    fn from_plan(error: PlanError) -> Self {
        match error {
            PlanError::Backpressure(Backpressure::Allocation) => Self::Allocation,
            PlanError::Backpressure(Backpressure::DependencyStageCapacity) => {
                Self::CompetingProgress
            }
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
            PlanError::ResourceContended(_) => Self::Fault(AuthorityFault::ResourceProjection),
            PlanError::Backpressure(
                Backpressure::ComputeResources | Backpressure::GenerationReplacement,
            ) => Self::Fault(AuthorityFault::SchedulerProjection),
            PlanError::Duplicate
            | PlanError::PayloadVariant
            | PlanError::Membership(_)
            | PlanError::Stale(_) => Self::Fault(AuthorityFault::MembershipProjection),
        }
    }

    fn from_local_plan(error: PlanError) -> Self {
        match error {
            PlanError::Stale(_) => Self::CompetingProgress,
            error => Self::from_plan(error),
        }
    }

    fn from_local_apply(error: ConcurrentRetainedIngressError) -> Self {
        match error {
            ConcurrentRetainedIngressError::Stale => Self::CompetingProgress,
            ConcurrentRetainedIngressError::Fault(fault) => Self::Fault(fault),
            ConcurrentRetainedIngressError::Backpressure(Backpressure::Allocation) => {
                Self::Allocation
            }
            ConcurrentRetainedIngressError::Backpressure(Backpressure::EffectCapacity) => {
                Self::EffectCapacity
            }
            ConcurrentRetainedIngressError::Backpressure(pressure) => {
                Self::from_plan(PlanError::Backpressure(pressure))
            }
        }
    }
}

impl AuthorityDriverError {
    /// Translate a Plan that consumes lock-external OCC evidence. A stale
    /// owner or view is an ordinary concurrent outcome only at this boundary.
    fn from_ready_plan(error: PlanError) -> Self {
        match error {
            PlanError::Stale(_) => Self::Stale,
            PlanError::Backpressure(Backpressure::DependencyStageCapacity) => Self::Stale,
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
            PlanError::ResourceContended(_) => Self::Fault(AuthorityFault::ResourceProjection),
            PlanError::Backpressure(
                Backpressure::ComputeResources | Backpressure::GenerationReplacement,
            ) => Self::Fault(AuthorityFault::SchedulerProjection),
            PlanError::Duplicate | PlanError::PayloadVariant | PlanError::Membership(_) => {
                Self::Fault(AuthorityFault::MembershipProjection)
            }
        }
    }

    /// Dependency maintenance plans under the shared generation guard and
    /// revalidates an exact mixed cut. OCC loss is therefore committed
    /// external progress, not a projection fault; the level-triggered driver
    /// yields and probes again.
    fn from_concurrent_maintenance_plan(error: PlanError) -> Self {
        Self::from_ready_plan(error)
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
            FinalAdmissionCaptureError::Plan(PlanError::Stale(_)) => {
                Self::Fault(AuthorityFault::SchedulerProjection)
            }
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
    fn into_retry_finished(self) -> AuthorityFinishedCompute {
        let Self { inner, execution } = self;
        let settlement = match inner {
            AuthorityComputeKind::Resolution(job) => job.retry(),
            AuthorityComputeKind::Verification(job) => job.retry(),
        };
        drop(execution);
        AuthorityFinishedCompute::from_parts(
            settlement,
            AuthorityComputeAftermath::completed_without_cache(),
        )
    }
}

/// Lock-external retained-compute result. It owns the exact settlement,
/// execution permit and post-commit cache consequence until execution is
/// explicitly finished. Finishing returns the permit to the shared fair gate
/// before any authority settlement attempt. No execution path can mutate
/// authority by constructing this value.
#[derive(Debug)]
#[must_use = "a completed retained computation must be settled exactly once"]
pub(in crate::authority) struct AuthorityComputeCompletion {
    settlement: ComputeSettlement,
    execution: AuthorityComputeExecutionPermit,
    aftermath: AuthorityComputeAftermath,
}

/// Finished retained work after its fair execution token has been returned.
/// The authority owner may still be `Computing`, but this capability consumes
/// no CPU slot and can wait behind effect capacity without starving Direct.
#[derive(Debug)]
#[must_use = "finished retained work must be settled or discharged exactly once"]
pub(in crate::authority) struct AuthorityFinishedCompute {
    settlement: ComputeSettlement,
    aftermath: AuthorityComputeAftermath,
}

#[derive(Debug)]
#[must_use = "post-commit compute consequences must be classified"]
pub(in crate::authority) struct AuthorityComputeAftermath {
    origin: SettlementOrigin,
    cache_update: Option<VerificationCacheUpdate>,
}

#[must_use = "a committed compute settlement must consume its aftermath and any post-commit fault"]
pub(in crate::authority) struct AuthorityComputeSettlementCommit {
    aftermath: AuthorityComputeAftermath,
    post_commit_fault: Option<AuthorityFault>,
}

impl AuthorityComputeSettlementCommit {
    pub(in crate::authority) fn into_parts(
        self,
    ) -> (AuthorityComputeAftermath, Option<AuthorityFault>) {
        (self.aftermath, self.post_commit_fault)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) enum AuthorityComputeAftermathDisposition {
    Progress,
    ReplaceGeneration,
    Fault(AuthorityFault),
}

impl AuthorityComputeCompletion {
    fn new(
        settlement: ComputeSettlement,
        execution: AuthorityComputeExecutionPermit,
        origin: SettlementOrigin,
        cache_update: Option<VerificationCacheUpdate>,
    ) -> Self {
        Self {
            settlement,
            execution,
            aftermath: AuthorityComputeAftermath {
                origin,
                cache_update,
            },
        }
    }

    pub(in crate::authority) fn finish_execution(self) -> AuthorityFinishedCompute {
        let Self {
            settlement,
            execution,
            aftermath,
        } = self;
        drop(execution);
        AuthorityFinishedCompute {
            settlement,
            aftermath,
        }
    }
}

impl AuthorityFinishedCompute {
    pub(in crate::authority) fn from_parts(
        settlement: ComputeSettlement,
        aftermath: AuthorityComputeAftermath,
    ) -> Self {
        Self {
            settlement,
            aftermath,
        }
    }

    pub(in crate::authority) fn settlement(&self) -> &ComputeSettlement {
        &self.settlement
    }

    pub(in crate::authority) fn aftermath(&self) -> &AuthorityComputeAftermath {
        &self.aftermath
    }

    pub(in crate::authority) fn into_parts(self) -> (ComputeSettlement, AuthorityComputeAftermath) {
        (self.settlement, self.aftermath)
    }
}

impl AuthorityComputeAftermath {
    pub(in crate::authority) fn completed_without_cache() -> Self {
        Self {
            origin: SettlementOrigin::Completion,
            cache_update: None,
        }
    }

    pub(in crate::authority) fn permits_immediate_refill(&self) -> bool {
        self.disposition() == AuthorityComputeAftermathDisposition::Progress
    }

    pub(in crate::authority) fn disposition(&self) -> AuthorityComputeAftermathDisposition {
        match self.origin {
            SettlementOrigin::Completion
            | SettlementOrigin::Capture(
                ResolutionExecutionKind::StaleView | ResolutionExecutionKind::ComputeBudget,
            )
            | SettlementOrigin::Resolution(
                ResolutionExecutionKind::StaleView | ResolutionExecutionKind::ComputeBudget,
            ) => AuthorityComputeAftermathDisposition::Progress,
            SettlementOrigin::Capture(ResolutionExecutionKind::ResourceUnavailable)
            | SettlementOrigin::Resolution(ResolutionExecutionKind::ResourceUnavailable) => {
                AuthorityComputeAftermathDisposition::ReplaceGeneration
            }
            SettlementOrigin::Capture(ResolutionExecutionKind::InvalidReceipt(error))
            | SettlementOrigin::Resolution(ResolutionExecutionKind::InvalidReceipt(error)) => {
                AuthorityComputeAftermathDisposition::Fault(match error {
                    ResolutionReceiptDefect::TransactionMismatch => {
                        AuthorityFault::MembershipProjection
                    }
                    ResolutionReceiptDefect::EmptyDependencies => {
                        AuthorityFault::DependencyProjection
                    }
                    ResolutionReceiptDefect::InvalidEvidence(error) => match error {
                        InputEvidenceError::Footprint(_)
                        | InputEvidenceError::ResidentBelowSerialized => {
                            AuthorityFault::ResourceProjection
                        }
                        InputEvidenceError::DependencySet(_) => {
                            AuthorityFault::DependencyProjection
                        }
                    },
                })
            }
        }
    }

    pub(in crate::authority) fn into_parts(
        self,
    ) -> (SettlementOrigin, Option<VerificationCacheUpdate>) {
        (self.origin, self.cache_update)
    }
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

    pub(in crate::authority) fn retry(self) -> AuthorityComputeCompletion {
        AuthorityComputeCompletion::new(
            self.request.retry(),
            self.execution,
            SettlementOrigin::Completion,
            None,
        )
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
    aftermath: AuthorityComputeAftermath,
}

impl AuthorityPendingSettlement {
    pub(in crate::authority) fn from_completion_failure(
        failure: ComputeSettlementFailure,
        aftermath: AuthorityComputeAftermath,
    ) -> Self {
        Self { failure, aftermath }
    }

    pub(in crate::authority) fn into_parts(
        self,
    ) -> (ComputeSettlementFailure, AuthorityComputeAftermath) {
        (self.failure, self.aftermath)
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
    Completion(AuthorityComputeCompletion),
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
    Retry(DirectIngressTransaction),
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
        }
    }

    fn from_plan(error: InternalPlugPlanError) -> Self {
        match error {
            InternalPlugPlanError::WouldDisplace => Self::WouldDisplace,
            InternalPlugPlanError::Plan(error) => match error {
                PlanError::Stale(_) => Self::Stale,
                PlanError::Membership(reason) => Self::Rejected(reason),
                PlanError::Backpressure(Backpressure::Allocation) => Self::ResourceUnavailable,
                PlanError::Backpressure(Backpressure::DependencyStageCapacity) => {
                    Self::ResourceUnavailable
                }
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
                PlanError::ResourceContended(_) => Self::Fault(AuthorityFault::ResourceProjection),
                PlanError::Backpressure(
                    Backpressure::ComputeResources | Backpressure::GenerationReplacement,
                ) => Self::Fault(AuthorityFault::SchedulerProjection),
                PlanError::Duplicate | PlanError::PayloadVariant => {
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
    ResourceContended(ResourceCapacityWaitIdentity),
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
            PlanError::Backpressure(Backpressure::DependencyStageCapacity) => {
                Self::ResourceUnavailable
            }
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
            PlanError::ResourceContended(wait) => Self::ResourceContended(wait),
            PlanError::Backpressure(
                Backpressure::ComputeResources | Backpressure::GenerationReplacement,
            ) => Self::Fault(AuthorityFault::SchedulerProjection),
            PlanError::Duplicate | PlanError::PayloadVariant | PlanError::Membership(_) => {
                Self::Fault(AuthorityFault::MembershipProjection)
            }
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
}

impl AuthorityLocalAdmissionExecution {
    pub(super) fn into_parts(
        self,
    ) -> (
        AuthorityLocalAdmissionOutcome,
        Option<VerificationCacheUpdate>,
    ) {
        (self.outcome, self.cache_update)
    }
}

#[derive(Debug, PartialEq, Eq)]
#[must_use = "Ready is a level; a committed Apply is progress evidence"]
pub(in crate::authority) enum AuthorityReadyOutcome {
    Idle,
    Applied,
    /// The existing effect journal is the sole releaser. The continuation is
    /// bounded by the captured Ready batch and owns no authority state.
    EffectCapacity(AuthorityReadyContinuation),
}

#[derive(Debug, PartialEq, Eq)]
enum ReadyPlanInput {
    Initial(SettlementBatch),
    Coupled(CoupledSettlementContinuation),
}

impl ReadyPlanInput {
    fn batch(&self) -> &SettlementBatch {
        match self {
            Self::Initial(batch) => batch,
            Self::Coupled(continuation) => continuation.batch(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ReservedReadyPlanInput {
    input: ReadyPlanInput,
    reservation: ReadyReservation,
}

#[must_use = "a compiled Ready assignment must reach its fixed commit lane"]
pub(in crate::authority) struct AuthorityReadyCommitAssignment {
    compiled: super::plan::CompiledSharedIndependent,
    reservation: super::scheduler::ReadySlotReservation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum AuthorityReadyCommitLane {
    First,
    Second,
    Other(usize),
}

impl AuthorityReadyCommitLane {
    pub(in crate::authority) const fn role_id(self) -> usize {
        match self {
            Self::First => 0,
            Self::Second => 1,
            Self::Other(index) => index,
        }
    }

    pub(in crate::authority) const fn from_index(index: usize) -> Self {
        match index {
            0 => Self::First,
            1 => Self::Second,
            _ => Self::Other(index),
        }
    }
}

pub(in crate::authority) enum AuthorityReadyCommitTerminal {
    Applied,
    Stale,
    Fault(AuthorityFault),
}

#[must_use = "every bounded Ready-wave assignment must reach one commit worker"]
pub(in crate::authority) struct AuthorityReadyWave {
    assignments: Vec<AuthorityReadyCommitAssignment>,
    wave_ends: Vec<usize>,
}

impl AuthorityReadyWave {
    pub(in crate::authority) fn into_parts(
        self,
    ) -> (Vec<AuthorityReadyCommitAssignment>, Vec<usize>) {
        (self.assignments, self.wave_ends)
    }
}

pub(in crate::authority) enum AuthorityReadyDispatch {
    Outcome(AuthorityReadyOutcome),
    Wave(AuthorityReadyWave),
}

enum ReadyWaveCompilation {
    Wave(AuthorityReadyWave),
    Fallback(ReadyReservation),
    Error {
        reservation: ReadyReservation,
        error: PlanError,
    },
}

/// Owned terminal for the shared Ready read cut. Every variant contains only
/// values that may outlive the read guard; publication consumes the enclosing
/// [`Released`] proof rather than depending on a caller-written `drop(store)`.
#[must_use = "a shared Ready read terminal must be released and finished"]
#[expect(
    clippy::large_enum_variant,
    reason = "the committed arm owns already-applied retirement storage; boxing after irreversible Apply would add an unplanned fallible allocation to the publication boundary"
)]
enum SharedReadyReadTerminal {
    Wave(AuthorityReadyWave),
    Committed {
        committed: CommittedDelta,
        post_commit_fault: Option<AuthorityFault>,
    },
    ReleaseApplied {
        before: super::plan::AuthorityWakeProjection,
    },
    ReleaseFault {
        before: super::plan::AuthorityWakeProjection,
        fault: AuthorityFault,
    },
    RequiresCanonical {
        reservation: ReadyReservation,
        continuation: Option<CoupledSettlementContinuation>,
    },
    ClockContended {
        reservation: ReadyReservation,
        effect_wake: Option<EffectWakeTransition>,
    },
    EffectCapacity(ReadyReservation),
    Error {
        reservation: ReadyReservation,
        error: AuthorityDriverError,
    },
}

enum SharedReadyReadDisposition {
    Terminal(Result<AuthorityReadyDispatch, AuthorityDriverError>),
    RequiresCanonical(ReservedReadyPlanInput),
}

enum SharedCanonicalReadyOutcome {
    Complete,
    Progress,
    Error(PlanError),
}

struct SharedCanonicalReadyTerminal {
    committed: Vec<CommittedDelta>,
    post_commit_fault: Option<AuthorityFault>,
    effect_wake: Option<EffectWakeTransition>,
    effect_capacity: Option<ReservedReadyPlanInput>,
    outcome: SharedCanonicalReadyOutcome,
}

impl std::fmt::Debug for AuthorityReadyDispatch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Outcome(outcome) => formatter.debug_tuple("Outcome").field(outcome).finish(),
            Self::Wave(_) => formatter.write_str("Wave(<sealed>)"),
        }
    }
}

/// Exact validated Ready work retained only across the publisher's existing
/// effect-capacity wait. The single Ready task owns this value; dropping it is
/// safe because the authoritative owners remain at the level-triggered Ready
/// frontier and every receipt is revalidated before a later Apply.
#[must_use = "effect-blocked Ready work must resume or be dropped back to the Ready level"]
pub(in crate::authority) struct AuthorityReadyContinuation {
    runtime: AuthorityRuntime,
    input: Option<Box<ReservedReadyPlanInput>>,
    armed: bool,
}

impl std::fmt::Debug for AuthorityReadyContinuation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthorityReadyContinuation")
            .field("input", &self.input)
            .finish_non_exhaustive()
    }
}

impl PartialEq for AuthorityReadyContinuation {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.runtime.store, &other.runtime.store) && self.input == other.input
    }
}

impl Eq for AuthorityReadyContinuation {}

impl AuthorityReadyContinuation {
    fn new(runtime: &AuthorityRuntime, input: ReservedReadyPlanInput) -> Self {
        Self {
            runtime: runtime.clone(),
            input: Some(Box::new(input)),
            armed: true,
        }
    }

    fn take_input(mut self) -> Result<ReservedReadyPlanInput, AuthorityDriverError> {
        self.armed = false;
        self.input
            .take()
            .map(|input| *input)
            .ok_or(AuthorityDriverError::Fault(
                AuthorityFault::SchedulerProjection,
            ))
    }
}

impl Drop for AuthorityReadyContinuation {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let before = self.runtime.store.read().authority.wake_projection();
        drop(self.input.take());
        self.runtime.publish_authority_release(before);
        self.armed = false;
    }
}

#[derive(Debug)]
#[must_use = "an exchanged assignment must reach its exact stable worker or be requeued"]
pub(in crate::authority) struct AuthorityComputeAssignment {
    slot: ComputeWorkerSlot,
    job: AuthorityComputeJob,
}

/// Capture could not construct an executable job after checkout committed.
/// The exact settlement and execution permit remain paired until this value
/// leaves the authority guard; finishing it releases the fair CPU slot before
/// the coordinator attempts settlement.
#[derive(Debug)]
#[must_use = "a failed capture must release execution and settle exactly once"]
pub(in crate::authority) struct AuthorityComputeCaptureCompletion {
    slot: ComputeWorkerSlot,
    completion: AuthorityComputeCompletion,
}

impl AuthorityComputeCaptureCompletion {
    pub(in crate::authority) fn finish_execution(self) -> ComputeExchangeCompletion {
        ComputeExchangeCompletion::from_finished(self.slot, self.completion.finish_execution())
    }
}

impl AuthorityComputeAssignment {
    pub(in crate::authority) fn from_parts(
        slot: ComputeWorkerSlot,
        job: AuthorityComputeJob,
    ) -> Self {
        Self { slot, job }
    }

    pub(in crate::authority) fn slot(&self) -> ComputeWorkerSlot {
        self.slot
    }

    pub(in crate::authority) fn into_parts(self) -> (ComputeWorkerSlot, AuthorityComputeJob) {
        (self.slot, self.job)
    }

    pub(in crate::authority) fn into_requeue_completion(self) -> ComputeExchangeCompletion {
        ComputeExchangeCompletion::from_finished(self.slot, self.job.into_retry_finished())
    }
}

#[must_use = "a committed compute exchange must route every returned linear capability"]
pub(in crate::authority) struct AuthorityCommittedComputeExchange {
    pub(in crate::authority) settled: Vec<ComputeExchangeSettled>,
    pub(in crate::authority) obsolete: Vec<ComputeWorkerSlot>,
    pub(in crate::authority) deferred: Vec<ComputeExchangeDeferred>,
    pub(in crate::authority) capture_failures: Vec<AuthorityComputeCaptureCompletion>,
    pub(in crate::authority) assignments: Vec<AuthorityComputeAssignment>,
    pub(in crate::authority) unused_grants: Vec<ComputeWorkerGrant>,
    pub(in crate::authority) follow_up: AuthorityComputeExchangeFollowUp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum AuthorityComputeExchangeFollowUp {
    None,
    RetryProbe,
    Fault(AuthorityFault),
}

#[must_use = "a failed compute exchange still owns every submitted capability"]
pub(in crate::authority) enum AuthorityComputeExchangeFailure {
    Allocation {
        completions: Vec<ComputeExchangeCompletion>,
        grants: Vec<ComputeWorkerGrant>,
    },
    Plan(ComputeExchangePlanFailure),
}

#[must_use = "captured Ready validation work must be validated or discarded as stale"]
struct ReadyValidationBatch {
    reservation: ReadyReservation,
    head: FinalAdmissionValidation,
    tail: Vec<FinalAdmissionValidation>,
}

#[must_use = "Ready work must be preallocated and rechecked against one later authority cut"]
struct ReadyWorkBatch {
    reservation: ReadyReservation,
    snapshot: Arc<Snapshot>,
    head: FinalAdmissionPreparation,
    tail: Vec<FinalAdmissionPreparation>,
}

#[must_use = "prepared Ready work must complete its OCC capture"]
struct PreparedReadyValidationBatch {
    reservation: ReadyReservation,
    head: PreparedFinalAdmissionValidation,
    tail: Vec<PreparedFinalAdmissionValidation>,
    completed_tail: Vec<FinalAdmissionValidation>,
}

/// Result of comparing one captured Ready batch with the later scheduler cut.
/// Scratch that lost the comparison remains owned by this value until the
/// authority read guard has been released.
#[must_use = "the Ready recheck scratch must retire outside the authority guard"]
enum ReadyRecheckOutcome {
    HeadChanged(PreparedReadyValidationBatch),
    UnchangedPrefix {
        batch: ReadyValidationBatch,
        discarded_tail: std::vec::IntoIter<PreparedFinalAdmissionValidation>,
    },
}

impl ReadyRecheckOutcome {
    /// Consume stale scratch only after the store guard has been released.
    fn finish(self) -> Option<ReadyValidationBatch> {
        match self {
            Self::HeadChanged(stale) => {
                drop(stale);
                None
            }
            Self::UnchangedPrefix {
                batch,
                discarded_tail,
            } => {
                drop(discarded_tail);
                Some(batch)
            }
        }
    }
}

enum ReadyDisposition {
    Candidates {
        batch: SettlementBatch,
        reservation: ReadyReservation,
    },
    Head {
        outcome: FinalAdmissionValidationOutcome,
        reservation: ReadyReservation,
    },
}

/// Exact lock-external wake bridge for a Ready reservation or staged effect
/// that is released without producing a [`CommittedDelta`]. It observes no
/// policy and carries no queue state; the ordinary before/after relation
/// decides whether any waiter must be prompted.
struct ReadyReleaseWake<'runtime> {
    runtime: &'runtime AuthorityRuntime,
    before: super::plan::AuthorityWakeProjection,
    armed: bool,
}

impl<'runtime> ReadyReleaseWake<'runtime> {
    fn new(runtime: &'runtime AuthorityRuntime) -> Self {
        let before = runtime.store.read().authority.wake_projection();
        Self {
            runtime,
            before,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ReadyReleaseWake<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.runtime.publish_authority_release(self.before);
        }
    }
}

impl AuthorityRuntime {
    /// Construct the runtime and relay frontier bound from one validated
    /// configuration. Returning both prevents service assembly from compiling
    /// the policy twice or pairing a relay drain with another runtime.
    pub(in crate::authority) fn new_with_relay_parent_limit(
        handle: &ckb_async_runtime::Handle,
        config: &TxPoolConfig,
        consensus: &Consensus,
        snapshot: Arc<Snapshot>,
    ) -> Result<(Self, usize), RuntimeConfigError> {
        let runtime = AuthorityRuntimeConfig::from_runtime_with_handle(config, consensus, handle)?;
        let relay_parent_limit = runtime.resolution_policy.direct_max_edges;
        Self::from_config(runtime, snapshot).map(|runtime| (runtime, relay_parent_limit))
    }

    fn from_config(
        runtime: AuthorityRuntimeConfig,
        snapshot: Arc<Snapshot>,
    ) -> Result<Self, RuntimeConfigError> {
        let resolution_policy = runtime.resolution_policy;
        let expiry_policy = runtime.expiry_policy;
        let verify_workers = runtime.verify_workers;
        let initial_load_limit = runtime.initial_load_limit;
        let vm_execution_mode = runtime.vm_execution_mode;
        let transient_compute =
            ComputeGate::new(runtime.transient_compute_permits, runtime.verification_time);
        let full_query = AuthorityQueryScratch::new(runtime.full_query_max_rows);
        Ok(Self {
            store: Arc::new(AuthorityStoreLock::new(AuthorityStore::from_runtime(
                runtime, snapshot,
            )?)),
            signals: Arc::new(AuthoritySignals::new()),
            lifecycle_fence: Arc::new(AuthorityLifecycleFence::default()),
            resolution_policy,
            expiry_policy,
            verify_workers,
            transient_compute,
            vm_execution_mode,
            initial_load_limit,
            full_query,
            #[cfg(test)]
            template_captures: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            #[cfg(test)]
            ready_commit_probes: Arc::new(std::array::from_fn(|_| Mutex::new(None))),
        })
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

    pub(crate) fn transaction_status_lookup(
        &self,
        hash: &Byte32,
    ) -> AuthorityTransactionStatusLookup {
        let store = self.store.read();
        super::query::transaction_status_lookup(
            &store.authority.read_view(),
            &store.snapshot,
            &RawTxHash(hash.clone()),
        )
    }

    pub(crate) async fn pool_summary(&self) -> Result<AuthorityPoolSummary, AuthorityQueryError> {
        let mut permit = self.full_query.acquire().await;
        let store = self.store.read();
        permit
            .capture_pool_summary(&store.authority.read_view(), &store.snapshot)
            .map_err(Into::into)
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
        let mut captured = Vec::new();
        captured
            .try_reserve(requested.len())
            .map_err(|_| AuthorityQueryError::Allocation)?;
        {
            let cut = view.full_read_cut()?;
            for proposal in &requested {
                if cut
                    .entry_by_proposal(&super::state::ProposalId(proposal.clone()))?
                    .is_none()
                    && let Some(hash) = store
                        .committed_txs_hash_cache
                        .as_ref()
                        .and_then(|cache| cache.peek(proposal))
                {
                    committed.push((proposal.clone(), hash.clone()));
                }
            }
            cut.capture_compact_transactions_into(&requested, &mut captured)?;
        }
        let snapshot = Arc::clone(&store.snapshot);
        drop(store);
        CompactBlockReadReceipt::capture(snapshot, captured, committed).map_err(Into::into)
    }

    pub(crate) fn accepted_with_cycles(
        &self,
        mut requested: Vec<ckb_types::packed::Byte32>,
    ) -> Result<AcceptedTransactionsWithCycles, AuthorityQueryError> {
        requested.sort_unstable();
        requested.dedup();
        let store = self.store.read();
        super::query::accepted_with_cycles(&store.authority.read_view(), &requested)
            .map_err(Into::into)
    }

    pub(crate) async fn pool_ids(
        &self,
    ) -> Result<ckb_types::core::tx_pool::TxPoolIds, AuthorityQueryError> {
        let mut permit = self.full_query.acquire().await;
        loop {
            let store = self.store.read();
            let view = store.authority.read_view();
            let captured = permit.capture_pool_ids(&view);
            drop(view);
            drop(store);
            match captured.map_err(AuthorityQueryError::from)? {
                super::query::FullQueryCapture::Prepared(captured) => {
                    return captured.finish().map_err(Into::into);
                }
                super::query::FullQueryCapture::NeedsGrow(observed_rows) => {
                    permit.grow(observed_rows)?;
                }
            }
        }
    }

    pub(crate) async fn all_entry_info(
        &self,
    ) -> Result<ckb_types::core::tx_pool::TxPoolEntryInfo, AuthorityQueryError> {
        let mut permit = self.full_query.acquire().await;
        loop {
            let store = self.store.read();
            let view = store.authority.read_view();
            let captured = permit.capture_entry_info(&view);
            drop(view);
            drop(store);
            match captured.map_err(AuthorityQueryError::from)? {
                super::query::FullQueryCapture::Prepared(captured) => {
                    return captured.finish().map_err(Into::into);
                }
                super::query::FullQueryCapture::NeedsGrow(observed_rows) => {
                    permit.grow(observed_rows)?;
                }
            }
        }
    }

    pub(crate) async fn pool_detail(
        &self,
        hash: &Byte32,
    ) -> Result<Option<ckb_types::core::tx_pool::PoolTxDetailInfo>, AuthorityQueryError> {
        let hash = RawTxHash(hash.clone());
        let mut permit = self.full_query.acquire().await;
        let store = self.store.read();
        let captured = permit.capture_pool_detail(&store.authority.read_view(), &hash);
        drop(store);
        captured
            .map_err(AuthorityQueryError::from)?
            .finish()
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

    pub(crate) async fn fee_estimate_receipt(
        &self,
    ) -> Result<FeeEstimateReadReceipt, AuthorityQueryError> {
        let mut permit = self.full_query.acquire().await;
        loop {
            let store = self.store.read();
            let view = store.authority.read_view();
            let captured = permit.capture_fee_estimate(
                &view,
                &store.snapshot,
                self.resolution_policy.min_fee_rate,
            );
            drop(view);
            drop(store);
            match captured.map_err(AuthorityQueryError::from)? {
                super::query::FullQueryCapture::Prepared(captured) => {
                    return captured.finish().map_err(Into::into);
                }
                super::query::FullQueryCapture::NeedsGrow(observed_rows) => {
                    permit.grow(observed_rows)?;
                }
            }
        }
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
        let (committed, post_commit_fault) = {
            let store = self.store.read();
            let Some(compiled) = store
                .authority
                .compile_shared_local_removal(&RawTxHash(hash.clone()))
                .map_err(AuthorityAdministrationError::from_local_plan)?
            else {
                return Ok(false);
            };
            #[cfg(test)]
            store.authority.enter_concurrent_removal_plan_probe();
            let prepared = compiled
                .bind(&store.authority)
                .map_err(AuthorityAdministrationError::from_local_plan)?;
            let shared = match prepared.apply() {
                Ok(shared) => shared,
                Err(failure) => {
                    let (error, effect_wake) = failure.into_parts();
                    drop(store);
                    if let Some(effect_wake) = effect_wake {
                        self.signals.publish_effect_wake(effect_wake);
                    }
                    return Err(AuthorityAdministrationError::from_local_apply(error));
                }
            };
            let committed = shared.into_parts();
            drop(store);
            committed
        };
        let post_commit_fault = self.publish_committed(committed).or(post_commit_fault);
        if let Some(fault) = post_commit_fault {
            return Err(AuthorityAdministrationError::Fault(fault));
        }
        Ok(true)
    }

    /// Clear only preaccepted and replacement-history ownership. Accepted
    /// membership and the paired snapshot remain unchanged; advancing the
    /// generation makes every old compute/recovery capability stale without
    /// introducing a drain protocol.
    pub(super) async fn clear_pipeline(&self) -> Result<(), AuthorityAdministrationError> {
        let _lifecycle = self.lifecycle_fence.acquire_writer().await;
        let committed = {
            let mut store = self.store.write();
            store
                .authority
                .plan_clear_pipeline()
                .map_err(AuthorityAdministrationError::from_plan)?
                .apply()
        };
        match self.publish_committed(committed) {
            Some(fault) => Err(AuthorityAdministrationError::Fault(fault)),
            None => Ok(()),
        }
    }

    /// Replace all transaction ownership and its chain evidence under the one
    /// store guard. The authority derives the next revision from its current
    /// state; the supplied snapshot contributes only its exact tip and is
    /// installed in the same indivisible mutation.
    pub(super) async fn clear_pool(
        &self,
        new_snapshot: Arc<Snapshot>,
    ) -> Result<(), AuthorityAdministrationError> {
        self.replace_generation(Some(new_snapshot))
            .await
            .map_err(|error| match error {
                AuthorityGenerationReplacementError::LifecycleClosed => {
                    AuthorityAdministrationError::LifecycleClosed
                }
                AuthorityGenerationReplacementError::Fault(fault) => {
                    AuthorityAdministrationError::Fault(fault)
                }
            })
    }

    /// Terminate allocation pressure owned by the current authority generation.
    /// The replacement observes the current paired snapshot under the same
    /// write cut, so it cannot reinstall an older chain view after a concurrent
    /// ordered transition.
    pub(in crate::authority) async fn replace_current_generation_after_allocation(
        &self,
    ) -> Result<(), AuthorityGenerationReplacementError> {
        self.replace_generation(None).await
    }

    /// Install an empty generation at either the current snapshot (`None`) or
    /// one exact ordered-chain snapshot (`Some`). `plan_clear_pool` constructs
    /// only empty containers and clones the prebuilt GenerationReset batch.
    async fn replace_generation(
        &self,
        new_snapshot: Option<Arc<Snapshot>>,
    ) -> Result<(), AuthorityGenerationReplacementError> {
        let _lifecycle = self.lifecycle_fence.acquire_writer().await;
        let (committed, retired_snapshot, retired_hash_cache) = {
            let mut store = self.store.write();
            let tip_hash = new_snapshot
                .as_ref()
                .map_or_else(|| store.snapshot.tip_hash(), |snapshot| snapshot.tip_hash());
            let committed = store
                .authority
                .plan_clear_pool(tip_hash)
                .map_err(AuthorityGenerationReplacementError::from_plan)?
                .apply();
            let retired_snapshot =
                new_snapshot.map(|snapshot| std::mem::replace(&mut store.snapshot, snapshot));
            let retired_hash_cache = store.committed_txs_hash_cache.take();
            (committed, retired_snapshot, retired_hash_cache)
        };
        let post_commit = committed.into_post_commit();
        drop(retired_snapshot);
        self.recycle_committed_hash_cache(retired_hash_cache);
        match self.signals.publish_post_commit(post_commit) {
            Some(fault) => Err(AuthorityGenerationReplacementError::Fault(fault)),
            None => Ok(()),
        }
    }

    fn recycle_committed_hash_cache(&self, mut retired: Option<LruCache<ProposalShortId, Byte32>>) {
        let Some(mut cache) = retired.take() else {
            return;
        };
        cache.clear();
        let mut store = self.store.write();
        if store.committed_txs_hash_cache.is_none() {
            store.committed_txs_hash_cache = Some(cache);
        }
    }

    /// Commit one ordered chain transition against the exact supplied
    /// snapshot. The bounded proposal delta is derived from immutable paired
    /// snapshots before the authority guard, then the old snapshot identity is
    /// rechecked before the read-only owner compiler and atomic Apply.
    pub(super) async fn apply_chain_update(
        &self,
        command: ChainUpdateCommand,
    ) -> Result<CommittedChainUpdate, ChainUpdateFailure> {
        let old_snapshot = {
            let store = self.store.read();
            Arc::clone(&store.snapshot)
        };
        let proposal_transition =
            match ProposalTransitionFacts::between(&old_snapshot, &command.snapshot) {
                Ok(transition) => transition,
                Err(super::chain::ChainFactsError::Allocation) => {
                    return Err(ChainUpdateFailure::new(
                        ChainBoundaryError::Allocation,
                        command,
                    ));
                }
                Err(
                    super::chain::ChainFactsError::DuplicateTransaction
                    | super::chain::ChainFactsError::DuplicateHeader,
                ) => {
                    return Err(ChainUpdateFailure::new(
                        ChainBoundaryError::InvalidFacts,
                        command,
                    ));
                }
            };
        let _lifecycle = self.lifecycle_fence.acquire_writer().await;
        let store = self.store.upgradable_read();
        if !Arc::ptr_eq(&store.snapshot, &old_snapshot) {
            return Err(ChainUpdateFailure::new(
                ChainBoundaryError::InvalidSnapshotEvidence,
                command,
            ));
        }
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
        let accepted_validity =
            accepted_validity_transition(old_rules, new_rules, command.had_detached_chain);
        let new_view = ChainViewId::new(next_revision, command.snapshot.tip_hash());
        let facts = command
            .facts
            .bind(new_view.clone(), accepted_validity, &proposal_transition);
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
                            super::state::RecoveryAdmissionError::ResourceUnavailable,
                        ) => ChainBoundaryError::Allocation,
                        ChainValidationError::SnapshotMismatch
                        | ChainValidationError::MissingProposalPosition
                        | ChainValidationError::UnexpectedProposalPosition
                        | ChainValidationError::DuplicateProposalPosition => {
                            ChainBoundaryError::InvalidSnapshotEvidence
                        }
                        ChainValidationError::RecoveryAdmission(
                            super::state::RecoveryAdmissionError::InvalidTransaction,
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
            None => Vec::new(),
        };

        let mut store = AuthorityStoreLock::upgrade(store);
        let plan = match receipt {
            Some(receipt) => match store.authority.plan_chain_transition(receipt) {
                Ok(plan) => plan,
                Err(PlanError::Backpressure(Backpressure::GenerationReplacement)) => {
                    match store
                        .authority
                        .plan_chain_generation_replacement_preserving_sources(
                            new_view,
                            fallback_recoveries,
                        ) {
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
                .plan_chain_generation_replacement(new_view, detached_recoveries)
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
            snapshot,
        } = command;
        let retired_snapshot = std::mem::replace(&mut store.snapshot, Arc::clone(&snapshot));
        if let Some(cache) = store.committed_txs_hash_cache.as_mut() {
            for (proposal, hash) in committed_hashes {
                cache.put(proposal, hash);
            }
        }
        drop(store);
        let post_commit = committed.into_post_commit();
        drop(retired_snapshot);
        let post_commit_fault = self.signals.publish_post_commit(post_commit);
        Ok(CommittedChainUpdate {
            candidate_uncles,
            attached_blocks,
            snapshot,
            post_commit_fault,
        })
    }

    /// Commit the minimum ordered-chain consequence after detailed
    /// reconciliation encounters allocation pressure. The empty generation
    /// and prebuilt reset effect require no fallible scratch.
    pub(super) async fn apply_chain_generation_replacement(
        &self,
        replacement: ChainGenerationReplacement,
    ) -> Result<CommittedChainUpdate, AuthorityGenerationReplacementError> {
        let snapshot = replacement.into_snapshot();
        let committed_snapshot = Arc::clone(&snapshot);
        self.replace_generation(Some(snapshot)).await?;
        Ok(CommittedChainUpdate {
            candidate_uncles: Vec::new(),
            attached_blocks: std::collections::VecDeque::new(),
            snapshot: committed_snapshot,
            post_commit_fault: None,
        })
    }

    /// Expire one bounded due prefix of retained Remote owners. The wall clock
    /// and slice are runtime policy, not caller-provided transition evidence.
    pub(super) fn expire_remote_due(
        &self,
    ) -> Result<AuthorityMaintenanceOutcome, AuthorityDriverError> {
        let cutoff = RemoteDeadline(ckb_systemtime::unix_time().as_secs());
        let (committed, post_commit_fault) = {
            let store = self.store.read();
            let compiled = store
                .authority
                .plan_remote_expiry(cutoff, self.expiry_policy.remote_slice)
                .map_err(AuthorityDriverError::from_concurrent_maintenance_plan)?;
            let Some(compiled) = compiled else {
                return Ok(AuthorityMaintenanceOutcome::Idle);
            };
            #[cfg(test)]
            store.authority.enter_concurrent_removal_plan_probe();
            let prepared = compiled
                .bind(&store.authority)
                .map_err(AuthorityDriverError::from_concurrent_maintenance_plan)?;
            let shared = match prepared.apply() {
                Ok(shared) => shared,
                Err(failure) => {
                    let (error, effect_wake) = failure.into_parts();
                    drop(store);
                    if let Some(effect_wake) = effect_wake {
                        self.signals.publish_effect_wake(effect_wake);
                    }
                    return Err(match error {
                        ConcurrentRetainedIngressError::Stale => AuthorityDriverError::Stale,
                        ConcurrentRetainedIngressError::Fault(fault) => {
                            AuthorityDriverError::Fault(fault)
                        }
                        ConcurrentRetainedIngressError::Backpressure(pressure) => {
                            AuthorityDriverError::from_concurrent_maintenance_plan(
                                PlanError::Backpressure(pressure),
                            )
                        }
                    });
                }
            };
            let committed = shared.into_parts();
            drop(store);
            committed
        };
        let post_commit_fault = self.publish_committed(committed).or(post_commit_fault);
        if let Some(fault) = post_commit_fault {
            return Err(AuthorityDriverError::Fault(fault));
        }
        Ok(AuthorityMaintenanceOutcome::Applied)
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
        let (committed, post_commit_fault) = {
            let store = self.store.read();
            let compiled = store
                .authority
                .compile_shared_accepted_expiry(cutoff)
                .map_err(AuthorityDriverError::from_concurrent_maintenance_plan)?;
            let Some(compiled) = compiled else {
                return Ok(AuthorityMaintenanceOutcome::Idle);
            };
            #[cfg(test)]
            store.authority.enter_concurrent_removal_plan_probe();
            let prepared = compiled
                .bind(&store.authority)
                .map_err(AuthorityDriverError::from_concurrent_maintenance_plan)?;
            let shared = match prepared.apply() {
                Ok(shared) => shared,
                Err(failure) => {
                    let (error, effect_wake) = failure.into_parts();
                    drop(store);
                    if let Some(effect_wake) = effect_wake {
                        self.signals.publish_effect_wake(effect_wake);
                    }
                    return Err(match error {
                        ConcurrentRetainedIngressError::Stale => AuthorityDriverError::Stale,
                        ConcurrentRetainedIngressError::Fault(fault) => {
                            AuthorityDriverError::Fault(fault)
                        }
                        ConcurrentRetainedIngressError::Backpressure(pressure) => {
                            AuthorityDriverError::from_concurrent_maintenance_plan(
                                PlanError::Backpressure(pressure),
                            )
                        }
                    });
                }
            };
            let committed = shared.into_parts();
            drop(store);
            committed
        };
        let post_commit_fault = self.publish_committed(committed).or(post_commit_fault);
        if let Some(fault) = post_commit_fault {
            return Err(AuthorityDriverError::Fault(fault));
        }
        Ok(AuthorityMaintenanceOutcome::Applied)
    }

    /// Advance one dirty dependency edge or completion marker. The dependency
    /// frontier is level-triggered, so callers may repeat this bounded step
    /// until `Idle` without owning a second queue or cursor.
    pub(super) fn maintain_dependency(
        &self,
    ) -> Result<AuthorityMaintenanceOutcome, AuthorityDriverError> {
        let mut committed: [Option<CommittedDelta>; DEPENDENCY_MAINTENANCE_APPLY_BATCH] =
            std::array::from_fn(|_| None);
        let mut failure = None;
        {
            let store = self.store.read();
            for committed in &mut committed {
                let plan = match store.authority.plan_dependency_maintenance() {
                    Ok(plan) => plan,
                    Err(error) => {
                        failure = Some(AuthorityDriverError::from_concurrent_maintenance_plan(
                            error,
                        ));
                        break;
                    }
                };
                let Some(plan) = plan else {
                    break;
                };
                match plan.apply() {
                    Ok(shared) => {
                        let (applied, post_commit_fault) = shared.into_parts();
                        *committed = Some(applied);
                        if let Some(fault) = post_commit_fault {
                            failure = Some(AuthorityDriverError::Fault(fault));
                            break;
                        }
                    }
                    Err(ConcurrentIndependentError::ChangedCut(_)) => {
                        failure = Some(AuthorityDriverError::Stale);
                        break;
                    }
                    Err(ConcurrentIndependentError::Fault(fault)) => {
                        failure = Some(AuthorityDriverError::Fault(fault));
                        break;
                    }
                }
            }
        }
        let applied = committed.iter().any(Option::is_some);
        let mut publication_fault = None;
        for committed in committed.into_iter().flatten() {
            publication_fault = publication_fault.or_else(|| self.publish_committed(committed));
        }
        if let Some(fault) = publication_fault {
            return Err(AuthorityDriverError::Fault(fault));
        }
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(if applied {
            AuthorityMaintenanceOutcome::Applied
        } else {
            AuthorityMaintenanceOutcome::Idle
        })
    }

    #[must_use = "dependency poison must be forwarded after publishing the committed delta"]
    fn publish_committed(&self, committed: CommittedDelta) -> Option<AuthorityFault> {
        self.signals
            .publish_post_commit(committed.into_post_commit())
    }

    /// Execute and fully terminalize one move-owned Ready job. There is no
    /// driver-owned finalizer: the synchronous section binds the generation,
    /// commits the exact shard cut, activates its existing staged effect and
    /// produces the wake/retirement receipt. Publication happens immediately
    /// after the shared lifecycle guard is released and before this function
    /// returns a terminal to the driver.
    pub(in crate::authority) fn commit_ready_assignment(
        &self,
        lane: AuthorityReadyCommitLane,
        assignment: AuthorityReadyCommitAssignment,
    ) -> AuthorityReadyCommitTerminal {
        #[cfg(test)]
        self.enter_ready_commit_probe(lane);
        #[cfg(not(test))]
        let _ = lane;
        let outcome = {
            let store = self.store.read();
            assignment
                .compiled
                .commit_ready_job(&store.authority, assignment.reservation)
        };
        match outcome {
            ReadyJobCommitOutcome::Committed(committed) => {
                let (committed, post_commit_fault) = committed.into_parts();
                let post_commit_fault = self.publish_committed(committed).or(post_commit_fault);
                post_commit_fault.map_or(AuthorityReadyCommitTerminal::Applied, |fault| {
                    AuthorityReadyCommitTerminal::Fault(fault)
                })
            }
            ReadyJobCommitOutcome::Stale(wake) => {
                self.signals.publish_effect_wake(wake);
                AuthorityReadyCommitTerminal::Stale
            }
            ReadyJobCommitOutcome::Fault { fault, effect_wake } => {
                if let Some(wake) = effect_wake {
                    self.signals.publish_effect_wake(wake);
                }
                AuthorityReadyCommitTerminal::Fault(fault)
            }
        }
    }

    /// Explicitly terminalize work that could not enter a permanent Ready
    /// lane. This remains strictly pre-owner-mutation and publishes the exact
    /// effect/capacity edge produced by the sole journal.
    pub(in crate::authority) fn cancel_ready_assignment(
        &self,
        assignment: AuthorityReadyCommitAssignment,
    ) -> Result<(), AuthorityFault> {
        let wake = assignment
            .compiled
            .cancel_ready_job(assignment.reservation)?;
        self.signals.publish_effect_wake(wake);
        Ok(())
    }

    fn cancel_unassigned_ready_jobs(
        &self,
        assignments: Vec<super::plan::CompiledSharedIndependent>,
    ) -> Result<(), PlanError> {
        let mut fault = None;
        for assignment in assignments {
            match assignment.cancel_unassigned_ready_job() {
                Ok(wake) => self.signals.publish_effect_wake(wake),
                Err(error) => {
                    fault.get_or_insert(error);
                }
            }
        }
        fault.map_or(Ok(()), |fault| Err(PlanError::Fault(fault)))
    }

    #[cfg(test)]
    pub(in crate::authority) fn set_ready_commit_probe_for_foundation(
        &self,
        lane: AuthorityReadyCommitLane,
        probe: Option<Arc<super::shard::ConcurrentRemovalProbe>>,
    ) {
        let [first, second] = self.ready_commit_probes.as_ref();
        let slot = match lane {
            AuthorityReadyCommitLane::First => first,
            AuthorityReadyCommitLane::Second => second,
            AuthorityReadyCommitLane::Other(_) => return,
        };
        *slot.lock() = probe;
    }

    #[cfg(test)]
    fn enter_ready_commit_probe(&self, lane: AuthorityReadyCommitLane) {
        let [first, second] = self.ready_commit_probes.as_ref();
        let slot = match lane {
            AuthorityReadyCommitLane::First => first,
            AuthorityReadyCommitLane::Second => second,
            AuthorityReadyCommitLane::Other(_) => return,
        };
        if let Some(probe) = slot.lock().clone() {
            probe.enter();
        }
    }

    fn publish_authority_release(&self, before: super::plan::AuthorityWakeProjection) {
        let after = self.store.read().authority.wake_projection();
        self.signals
            .publish_wake(super::plan::AuthorityWakeTransition::between(before, after));
    }

    pub(super) fn compute_signal(&self) -> &Notify {
        &self.signals.compute
    }

    pub(super) fn ready_signal(&self) -> &Notify {
        &self.signals.ready
    }

    pub(super) fn maintenance_signal(&self) -> &Notify {
        &self.signals.maintenance
    }

    pub(super) fn effect_publisher_signal(&self) -> &Notify {
        &self.signals.effect_publisher
    }

    pub(super) fn effect_capacity_signal(&self) -> &Notify {
        &self.signals.effect_capacity
    }

    #[cfg(test)]
    pub(in crate::authority) fn post_commit_signal_for_foundation(&self) -> &Notify {
        &self.signals.post_commit
    }

    pub(in crate::authority) fn template_signal(&self) -> &Notify {
        &self.signals.template
    }

    /// Acquire one shared tx-pool execution slot before any retained checkout
    /// or owner-free direct capture. Waiting owns no authority capability and
    /// holds no guard. Normal shutdown cancels the waiter; the semaphore itself
    /// is never closed as a control protocol.
    pub(in crate::authority) async fn acquire_compute_execution(
        &self,
        cancel: &CancellationToken,
    ) -> Option<AuthorityComputeExecutionPermit> {
        self.transient_compute.acquire(cancel).await
    }

    /// Attempt one fair-gate acquisition without joining the semaphore wait
    /// queue. The compute coordinator uses this while finished capabilities
    /// are pending so settlement can never wait behind Direct execution.
    pub(in crate::authority) fn try_acquire_compute_execution(
        &self,
    ) -> Option<AuthorityComputeExecutionPermit> {
        self.transient_compute.try_acquire()
    }

    pub(in crate::authority) fn compute_capacity_signal(&self) -> &Notify {
        self.transient_compute.release_signal()
    }

    pub(super) fn resource_capacity_wait_identity(&self) -> ResourceCapacityWaitIdentity {
        self.store
            .read()
            .authority
            .resource_capacity_wait_identity()
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
            let disposition = store
                .authority
                .plan_internal_plug(receipt)
                .map_err(AuthorityInternalPlugError::from_plan)?;
            let InternalPlugDisposition::Insert(plan) = disposition else {
                return Ok(AuthorityInternalPlugOutcome::Duplicate);
            };
            plan.apply()
        };
        match self.publish_committed(committed) {
            Some(fault) => Err(AuthorityInternalPlugError::Fault(fault)),
            None => Ok(AuthorityInternalPlugOutcome::Inserted),
        }
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
        let prepared = DirectAdmissionValidation::prepare(
            snapshot,
            work,
            self.resolution_policy.min_fee_rate,
        )?;
        let validation = {
            let store = self.store.read();
            prepared.complete(AuthorityStoreCaptureSeal(()), &store.authority)?
        };
        validation.validate()
    }

    /// Publish a stable or still-current direct ingress/compute rejection for
    /// Local through the shared owner-free terminal. The outer read guard
    /// binds chain/generation while exact Accepted shards fence activation;
    /// publication consumes the terminal only after that guard is released.
    fn commit_direct_transaction_rejection(
        &self,
        rejection: DirectTransactionRejection,
    ) -> Result<CommittedPublicReject, PlanError> {
        let outcome = self.store.commit_direct_transaction_rejection(rejection)?;
        self.publish_shared_direct_rejection_terminal(outcome)
    }

    fn commit_direct_validation_rejection(
        &self,
        rejection: super::chain::DirectAdmissionRejection,
    ) -> Result<CommittedPublicReject, PlanError> {
        let outcome = self.store.commit_direct_validation_rejection(rejection)?;
        self.publish_shared_direct_rejection_terminal(outcome)
    }

    fn publish_shared_direct_rejection_terminal(
        &self,
        outcome: SharedDirectRejectionTerminalOutcome,
    ) -> Result<CommittedPublicReject, PlanError> {
        match outcome {
            SharedDirectRejectionTerminalOutcome::Committed { reason, committed } => {
                let (committed, post_commit_fault) = committed.into_parts();
                let post_commit_fault = self.publish_committed(committed).or(post_commit_fault);
                if let Some(fault) = post_commit_fault {
                    return Err(PlanError::Fault(fault));
                }
                Ok(reason)
            }
            SharedDirectRejectionTerminalOutcome::Failed { error, effect_wake } => {
                if let Some(effect_wake) = effect_wake {
                    self.signals.publish_effect_wake(effect_wake);
                }
                Err(error)
            }
        }
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
        match outcome {
            DirectAdmissionValidationOutcome::Reresolve(retry) => {
                Ok(AuthorityLocalAdmissionOutcome::Retry(
                    DirectIngressTransaction::from_retry(retry.into_transaction()),
                ))
            }
            DirectAdmissionValidationOutcome::Candidate(receipt) => {
                let completed = receipt.completed();
                let retry = receipt.retry_transaction();
                let terminal = self.store.with_read_released(|store| {
                    store
                        .authority
                        .prepare_shared_direct_admission(receipt)
                        .map(|prepared| prepared.commit())
                });
                self.finish_released_direct_admission(terminal, completed, retry)
            }
            DirectAdmissionValidationOutcome::Rejected(rejection) => {
                let reason = self.commit_direct_validation_rejection(rejection)?;
                Ok(AuthorityLocalAdmissionOutcome::Rejected(
                    DirectAdmissionRejectionKind::Validation(reason),
                ))
            }
        }
    }

    fn finish_released_direct_admission(
        &self,
        terminal: Released<Result<SharedDirectAdmissionCommitOutcome, PlanError>>,
        completed: EntryCompleted,
        retry: Arc<TransactionView>,
    ) -> Result<AuthorityLocalAdmissionOutcome, PlanError> {
        let publish = |committed: super::plan::CommittedSharedApply| {
            let (committed, post_commit_fault) = committed.into_parts();
            self.publish_committed(committed)
                .or(post_commit_fault)
                .map_or(Ok(()), |fault| Err(PlanError::Fault(fault)))
        };
        match terminal.into_inner() {
            Err(PlanError::Stale(_)) => Ok(AuthorityLocalAdmissionOutcome::Retry(
                DirectIngressTransaction::from_retry(retry),
            )),
            Err(error) => Err(error),
            Ok(SharedDirectAdmissionCommitOutcome::Accepted(committed)) => {
                publish(committed)?;
                Ok(AuthorityLocalAdmissionOutcome::Accepted(completed))
            }
            Ok(SharedDirectAdmissionCommitOutcome::Duplicate { key, committed }) => {
                publish(committed)?;
                Ok(AuthorityLocalAdmissionOutcome::Duplicate(key))
            }
            Ok(SharedDirectAdmissionCommitOutcome::Rejected { reason, committed }) => {
                publish(committed)?;
                Ok(AuthorityLocalAdmissionOutcome::Rejected(
                    DirectAdmissionRejectionKind::Membership(reason),
                ))
            }
            Ok(SharedDirectAdmissionCommitOutcome::Stale { effect_wake }) => {
                if let Some(wake) = effect_wake {
                    self.signals.publish_effect_wake(wake);
                }
                Ok(AuthorityLocalAdmissionOutcome::Retry(
                    DirectIngressTransaction::from_retry(retry),
                ))
            }
            Ok(SharedDirectAdmissionCommitOutcome::Fault { fault, effect_wake }) => {
                if let Some(wake) = effect_wake {
                    self.signals.publish_effect_wake(wake);
                }
                Err(PlanError::Fault(fault))
            }
        }
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
        let (command, work, pending_cache_update) = candidate.into_parts();
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
        command_rx: &mut watch::Receiver<ckb_script::ChunkCommand>,
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
        tx: &DirectIngressTransaction,
        execution: AuthorityComputeExecutionPermit,
    ) -> Result<AuthorityDirectResolutionOutcome, DirectComputationError> {
        self.resolve_direct_transaction(tx, DirectCommand::Local, execution)
    }

    pub(super) fn resolve_test_accept_transaction(
        &self,
        tx: &DirectIngressTransaction,
        execution: AuthorityComputeExecutionPermit,
    ) -> Result<AuthorityDirectResolutionOutcome, DirectComputationError> {
        self.resolve_direct_transaction(tx, DirectCommand::TestAccept, execution)
    }

    fn resolve_direct_transaction(
        &self,
        tx: &DirectIngressTransaction,
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
        let duration = self
            .transient_compute
            .verification_time()
            .duration(PayloadPolicy::Trusted);
        let budget = TxPoolVerificationBudget::new(duration, self.initial_load_limit)
            .with_vm_execution_mode(self.vm_execution_mode);
        loop {
            let evaluation = crate::util::block_offload(|| {
                job.evaluate(self.resolution_policy.min_fee_rate, budget)
            })?;
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
                    let rechecked = {
                        let store = self.store.read();
                        prepared.observe(&store.authority)
                    };
                    let observation = rechecked.finish()?;
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

    fn try_effect_publication(&self) -> EffectPublicationObservation {
        self.store.read().authority.effect_publication_observation()
    }

    /// Wait for the next committed effect receipt. `None` means the log is
    /// closed and fully drained. The mutable publisher claim remains borrowed
    /// by the returned receipt, so safe production code cannot observe the same
    /// resident head concurrently or release the claim before settlement.
    pub(in crate::authority) async fn wait_effect_publication<'runtime, 'claim>(
        &'runtime self,
        claim: &'claim mut AuthorityEffectPublisherClaim,
    ) -> Option<AuthorityEffectPublicationLease<'runtime, 'claim>> {
        loop {
            let notified = self.effect_publisher_signal().notified();
            match self.try_effect_publication() {
                EffectPublicationObservation::Idle => notified.await,
                EffectPublicationObservation::Receipt(receipt) => {
                    return Some(AuthorityEffectPublicationLease {
                        runtime: self,
                        receipt: Some(receipt),
                        _claim: claim,
                    });
                }
                EffectPublicationObservation::ClosedAndDrained => return None,
            }
        }
    }

    /// Reuse an observation captured by the preceding settlement Apply.
    ///
    /// The sole mutable publisher claim still makes the receipt linear. This
    /// constructor only removes the otherwise redundant authority read between
    /// two already-resident FIFO heads; endpoint I/O and journal settlement
    /// remain one batch at a time and in exactly the same order.
    pub(in crate::authority) fn effect_publication_lease<'runtime, 'claim>(
        &'runtime self,
        claim: &'claim mut AuthorityEffectPublisherClaim,
        receipt: EffectReceipt,
    ) -> AuthorityEffectPublicationLease<'runtime, 'claim> {
        AuthorityEffectPublicationLease {
            runtime: self,
            receipt: Some(receipt),
            _claim: claim,
        }
    }

    fn settle_effect(
        &self,
        settlement: EffectSettlement,
    ) -> Result<EffectPublicationObservation, EffectSettlementFailure> {
        let rejection_metrics = settlement.rejection_metrics();
        let (commit, next) = {
            let store = self.store.read();
            store.authority.apply_effect_settlement(settlement)?
        };
        let _dependency_free = match commit {
            EffectSettlementCommit::Applied(retirement) => self.publish_committed(retirement),
            EffectSettlementCommit::Superseded(settlement) => {
                drop(settlement);
                None
            }
        };
        rejection_metrics.publish();
        Ok(next)
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
    /// capability has drained. Already committed queued/reset effects remain
    /// publishable until `effects_closed_and_drained` becomes true.
    pub(in crate::authority) async fn close_effects(&self) -> Result<(), EffectCloseError> {
        let _lifecycle = self.lifecycle_fence.acquire_writer().await;
        let retirement = {
            let mut store = self.store.write();
            store.authority.plan_effect_close()?.apply()
        };
        // Effect-only Apply cannot stage a dependency row; its retirement
        // carries `dependency: None` by construction.
        let _dependency_free = self.publish_committed(retirement);
        Ok(())
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

    fn try_commit_shared_retained_ingress_batch(
        &self,
        batch: &RetainedAdmissionBatch,
    ) -> Result<super::plan::CommittedRetainedAdmissionBatch, PlanError> {
        let route = {
            let store = self.store.read();
            store
                .authority
                .classify_shared_retained_ingress_head(batch)?
        };
        #[cfg(test)]
        self.with_authority_read_for_foundation(|authority| {
            authority
                .entries_for_reference()
                .enter_shared_ingress_probe(
                    super::shard::SharedIngressProbePhase::AfterRetainedIngressHeadClassification,
                );
        });
        match route {
            SharedRetainedIngressHead::Owner => {
                let store = self.store.read();
                let compiled = match store.authority.compile_shared_retained_ingress_batch(batch) {
                    Ok(Some(compiled)) => compiled,
                    Ok(None) | Err(PlanError::Stale(_)) => {
                        return Err(PlanError::Stale(super::plan::StalePlan::Version));
                    }
                    Err(error) => return Err(error),
                };
                compiled
                    .bind(&store.authority)
                    .and_then(|prepared| prepared.apply())
                    .map_err(|error| match error {
                        ConcurrentRetainedIngressError::Stale => {
                            PlanError::Stale(super::plan::StalePlan::Version)
                        }
                        ConcurrentRetainedIngressError::Fault(fault) => PlanError::Fault(fault),
                        ConcurrentRetainedIngressError::Backpressure(pressure) => {
                            PlanError::Backpressure(pressure)
                        }
                    })
            }
            SharedRetainedIngressHead::EffectOrNoop => {
                let store = self.store.read();
                let prepared = match store.authority.plan_shared_retained_effect_prefix(batch) {
                    Ok(Some(prepared)) => prepared,
                    Ok(None) | Err(PlanError::Stale(_)) => {
                        return Err(PlanError::Stale(super::plan::StalePlan::Version));
                    }
                    Err(error) => return Err(error),
                };
                Ok(prepared.apply())
            }
        }
    }

    #[expect(
        clippy::result_large_err,
        reason = "a failed Plan returns the exact move-only ingress batch without allocating, including when allocation pressure caused the failure"
    )]
    pub(super) fn commit_retained_ingress_batch(
        &self,
        batch: RetainedAdmissionBatch,
    ) -> Result<
        (
            usize,
            std::collections::VecDeque<RetainedIngressAttempt>,
            Option<AuthorityFault>,
        ),
        RetainedIngressBatchFailure,
    > {
        let malformed = batch
            .attempts()
            .any(RetainedIngressAttempt::is_malformed_remote);
        if !malformed {
            let committed = match self.try_commit_shared_retained_ingress_batch(&batch) {
                Ok(committed) => committed,
                Err(PlanError::Stale(_)) => {
                    return Err(RetainedIngressBatchFailure::shared_contention(batch));
                }
                Err(error) => return Err(RetainedIngressBatchFailure::plan(error, batch)),
            };
            match committed {
                super::plan::CommittedRetainedAdmissionBatch::Applied {
                    retirement,
                    consumed,
                } => {
                    let (retirement, post_commit_fault) = retirement.into_parts();
                    let post_commit_fault =
                        self.publish_committed(retirement).or(post_commit_fault);
                    let mut remaining = batch.into_attempts();
                    for _ in 0..consumed {
                        drop(remaining.pop_front());
                    }
                    return Ok((consumed, remaining, post_commit_fault));
                }
                super::plan::CommittedRetainedAdmissionBatch::Unchanged { consumed } => {
                    let mut remaining = batch.into_attempts();
                    for _ in 0..consumed {
                        drop(remaining.pop_front());
                    }
                    return Ok((consumed, remaining, None));
                }
            }
        }

        let committed = {
            let store = self.store.read();
            let prepared = match store.authority.plan_shared_peer_revocation(&batch) {
                Ok(Some(prepared)) => prepared,
                Ok(None) => {
                    return Err(RetainedIngressBatchFailure::plan(
                        PlanError::Fault(AuthorityFault::MembershipProjection),
                        batch,
                    ));
                }
                Err(PlanError::Stale(_)) => {
                    return Err(RetainedIngressBatchFailure::shared_contention(batch));
                }
                Err(error) => return Err(RetainedIngressBatchFailure::plan(error, batch)),
            };
            match prepared.apply() {
                Ok(committed) => committed,
                Err(failure) => {
                    let (error, effect_wake) = failure.into_parts();
                    drop(store);
                    if let Some(effect_wake) = effect_wake {
                        self.signals.publish_effect_wake(effect_wake);
                    }
                    return match error {
                        super::plan::ConcurrentRetainedIngressError::Stale => {
                            Err(RetainedIngressBatchFailure::shared_contention(batch))
                        }
                        super::plan::ConcurrentRetainedIngressError::Fault(fault) => Err(
                            RetainedIngressBatchFailure::plan(PlanError::Fault(fault), batch),
                        ),
                        super::plan::ConcurrentRetainedIngressError::Backpressure(pressure) => {
                            Err(RetainedIngressBatchFailure::plan(
                                PlanError::Backpressure(pressure),
                                batch,
                            ))
                        }
                    };
                }
            }
        };
        let (retirement, consumed) = match committed {
            super::plan::CommittedRetainedAdmissionBatch::Applied {
                retirement,
                consumed,
            } => (retirement, consumed),
            super::plan::CommittedRetainedAdmissionBatch::Unchanged { .. } => {
                return Err(RetainedIngressBatchFailure::plan(
                    PlanError::Fault(AuthorityFault::MembershipProjection),
                    batch,
                ));
            }
        };
        let (retirement, post_commit_fault) = retirement.into_parts();
        let post_commit_fault = self.publish_committed(retirement).or(post_commit_fault);
        let mut remaining = batch.into_attempts();
        for _ in 0..consumed {
            drop(remaining.pop_front());
        }
        Ok((consumed, remaining, post_commit_fault))
    }

    #[expect(
        clippy::result_large_err,
        reason = "failure returns the exact bounded completion and fair-grant capabilities without allocation or rollback"
    )]
    #[cfg_attr(
        feature = "profiling",
        tracing::instrument(
            name = "tx_pool.stage.compute_exchange",
            target = "ckb_tx_pool_profile",
            level = "trace",
            skip_all
        )
    )]
    pub(in crate::authority) fn exchange_compute(
        &self,
        completions: Vec<ComputeExchangeCompletion>,
        grants: Vec<ComputeWorkerGrant>,
    ) -> Result<AuthorityCommittedComputeExchange, AuthorityComputeExchangeFailure> {
        let inputs = {
            let store = self.store.read();
            store
                .authority
                .validate_compute_exchange_inputs(completions, grants)
        }
        .map_err(AuthorityComputeExchangeFailure::Plan)?;
        #[cfg(feature = "profiling")]
        let completion_count = inputs.completion_len();
        let grant_count = inputs.grant_len();
        #[cfg(feature = "profiling")]
        {
            let shape = match (completion_count == 0, grant_count == 0) {
                (false, false) => Some(tracing::trace_span!(
                    target: "ckb_tx_pool_profile",
                    "tx_pool.stage.compute_exchange_both"
                )),
                (false, true) => Some(tracing::trace_span!(
                    target: "ckb_tx_pool_profile",
                    "tx_pool.stage.compute_exchange_completion_only"
                )),
                (true, false) => Some(tracing::trace_span!(
                    target: "ckb_tx_pool_profile",
                    "tx_pool.stage.compute_exchange_grant_only"
                )),
                (true, true) => None,
            };
            let _shape = shape.as_ref().map(tracing::Span::enter);
            for _ in 0..completion_count {
                let _completion = tracing::trace_span!(
                    target: "ckb_tx_pool_profile",
                    "tx_pool.stage.compute_exchange_completion"
                )
                .entered();
            }
            for _ in 0..grant_count {
                let _grant = tracing::trace_span!(
                    target: "ckb_tx_pool_profile",
                    "tx_pool.stage.compute_exchange_grant"
                )
                .entered();
            }
        }
        let mut assignments = Vec::new();
        if assignments.try_reserve(grant_count).is_err() {
            let (completions, grants) = inputs.into_parts();
            return Err(AuthorityComputeExchangeFailure::Allocation {
                completions,
                grants,
            });
        }
        let mut capture_failures = Vec::new();
        if capture_failures.try_reserve(grant_count).is_err() {
            let (completions, grants) = inputs.into_parts();
            return Err(AuthorityComputeExchangeFailure::Allocation {
                completions,
                grants,
            });
        }

        {
            let store = self.store.read();
            let prepared = store
                .authority
                .prepare_shared_compute_exchange(inputs)
                .map_err(AuthorityComputeExchangeFailure::Plan)?;
            let outcome = prepared.apply();
            let (exchange, follow_up) = match outcome {
                SharedComputeExchangeOutcome::Committed {
                    exchange,
                    post_commit_fault,
                } => {
                    let follow_up = post_commit_fault.map_or(
                        AuthorityComputeExchangeFollowUp::None,
                        AuthorityComputeExchangeFollowUp::Fault,
                    );
                    (exchange, follow_up)
                }
                SharedComputeExchangeOutcome::RetryProbe(recovered) => {
                    let RecoveredComputeExchange {
                        obsolete,
                        deferred,
                        unused_grants,
                    } = recovered;
                    drop(store);
                    return Ok(AuthorityCommittedComputeExchange {
                        settled: Vec::new(),
                        obsolete,
                        deferred,
                        capture_failures,
                        assignments,
                        unused_grants,
                        follow_up: AuthorityComputeExchangeFollowUp::RetryProbe,
                    });
                }
                SharedComputeExchangeOutcome::Fault { fault, recovered } => {
                    let RecoveredComputeExchange {
                        obsolete,
                        deferred,
                        unused_grants,
                    } = recovered;
                    drop(store);
                    return Ok(AuthorityCommittedComputeExchange {
                        settled: Vec::new(),
                        obsolete,
                        deferred,
                        capture_failures,
                        assignments,
                        unused_grants,
                        follow_up: AuthorityComputeExchangeFollowUp::Fault(fault),
                    });
                }
            };
            let CommittedComputeExchange {
                retirement,
                settled,
                obsolete,
                deferred,
                assignments: planned_assignments,
                unused_grants,
            } = exchange;
            for assignment in planned_assignments {
                Self::capture_compute_assignment(
                    &store.authority,
                    Arc::clone(&store.snapshot),
                    assignment,
                    &mut assignments,
                    &mut capture_failures,
                );
            }
            drop(store);
            let dependency_fault =
                retirement.and_then(|retirement| self.publish_committed(retirement));
            let follow_up =
                dependency_fault.map_or(follow_up, AuthorityComputeExchangeFollowUp::Fault);
            return Ok(AuthorityCommittedComputeExchange {
                settled,
                obsolete,
                deferred,
                capture_failures,
                assignments,
                unused_grants,
                follow_up,
            });
        }
    }

    fn capture_compute_assignment(
        authority: &TxPoolAuthority,
        snapshot: Arc<Snapshot>,
        assignment: ComputeExchangeAssignment,
        assignments: &mut Vec<AuthorityComputeAssignment>,
        capture_failures: &mut Vec<AuthorityComputeCaptureCompletion>,
    ) {
        let (slot, execution, work) = assignment.into_parts();
        let captured = match work {
            CheckedOutWork::Resolve(work) => {
                ResolutionJob::capture_resolve(authority, snapshot, work)
                    .map(AuthorityComputeKind::Resolution)
            }
            CheckedOutWork::ContinuousResolve(work) => {
                ResolutionJob::capture_continuous(authority, snapshot, work)
                    .map(AuthorityComputeKind::Resolution)
            }
            CheckedOutWork::Verify(work) => VerificationJob::from_checkout(work, snapshot)
                .map(AuthorityComputeKind::Verification),
        };
        match captured {
            Ok(inner) => assignments.push(AuthorityComputeAssignment {
                slot,
                job: AuthorityComputeJob { inner, execution },
            }),
            Err(failure) => {
                let kind = failure.kind();
                capture_failures.push(AuthorityComputeCaptureCompletion {
                    slot,
                    completion: AuthorityComputeCompletion::new(
                        failure.into_settlement(),
                        execution,
                        SettlementOrigin::Capture(kind),
                        None,
                    ),
                });
            }
        }
    }

    pub(in crate::authority) fn settle_finished(
        &self,
        finished: AuthorityFinishedCompute,
    ) -> ControlFlow<AuthorityPendingSettlement, AuthorityComputeSettlementCommit> {
        let AuthorityFinishedCompute {
            settlement,
            aftermath,
        } = finished;
        match self.settle(settlement) {
            Ok(post_commit_fault) => ControlFlow::Continue(AuthorityComputeSettlementCommit {
                aftermath,
                post_commit_fault,
            }),
            Err(failure) => ControlFlow::Break(
                AuthorityPendingSettlement::from_completion_failure(failure, aftermath),
            ),
        }
    }

    #[expect(
        clippy::result_large_err,
        reason = "failure returns the exact move-only compute settlement for retry or generation replacement; boxing would allocate on backpressure"
    )]
    pub(in crate::authority) fn settle(
        &self,
        settlement: ComputeSettlement,
    ) -> Result<Option<AuthorityFault>, ComputeSettlementFailure> {
        let outcome = {
            let store = self.store.read();
            let prepared = store
                .authority
                .prepare_shared_compute_settlement(settlement)?;
            prepared.apply()
        };
        match outcome {
            SharedComputeSettlementOutcome::Committed(committed) => {
                let (committed, post_commit_fault) = committed.into_parts();
                Ok(self.publish_committed(committed).or(post_commit_fault))
            }
            SharedComputeSettlementOutcome::Failed {
                failure,
                effect_wake,
            } => {
                if let Some(wake) = effect_wake {
                    self.signals.publish_effect_wake(wake);
                }
                Err(failure)
            }
        }
    }

    /// Execute one resolve capability entirely outside the authority guard.
    /// A bounded dep-group miss may take allocation-free Accepted read cuts;
    /// every terminal or retry result is returned as one linear completion.
    /// This method performs no authoritative mutation.
    pub(in crate::authority) fn execute_compute(
        &self,
        job: AuthorityComputeJob,
    ) -> AuthorityComputeOutcome {
        let AuthorityComputeJob { inner, execution } = job;
        match inner {
            AuthorityComputeKind::Resolution(job) => self.execute_resolution(job, execution),
            AuthorityComputeKind::Verification(job) => {
                let duration = self
                    .transient_compute
                    .verification_time()
                    .duration(job.payload_policy());
                let budget = TxPoolVerificationBudget::new(duration, self.initial_load_limit)
                    .with_vm_execution_mode(self.vm_execution_mode);
                AuthorityComputeOutcome::Verification(AuthorityVerificationRequest {
                    request: job.prepare(budget),
                    execution,
                })
            }
        }
    }

    #[expect(
        clippy::result_large_err,
        reason = "the offloaded resolver closure returns exact allocation-free failure evidence to the linear completion"
    )]
    #[cfg_attr(
        feature = "profiling",
        tracing::instrument(
            name = "tx_pool.stage.resolve",
            target = "ckb_tx_pool_profile",
            level = "trace",
            skip_all
        )
    )]
    fn execute_resolution(
        &self,
        mut job: ResolutionJob,
        execution: AuthorityComputeExecutionPermit,
    ) -> AuthorityComputeOutcome {
        loop {
            let policy = self.resolution_policy;
            let evaluated = crate::util::block_offload(|| {
                job.evaluate(policy.min_fee_rate, policy.large_cycle_threshold)
            });
            let evaluation = match evaluated {
                Ok(evaluation) => evaluation,
                Err(failure) => return Self::resolution_failure(failure, execution),
            };
            match evaluation {
                ResolutionEvaluation::Settle(settlement) => {
                    return AuthorityComputeOutcome::Completion(AuthorityComputeCompletion::new(
                        settlement,
                        execution,
                        SettlementOrigin::Completion,
                        None,
                    ));
                }
                ResolutionEvaluation::Verify(verification) => {
                    let duration = self
                        .transient_compute
                        .verification_time()
                        .duration(verification.payload_policy());
                    let budget = TxPoolVerificationBudget::new(duration, self.initial_load_limit)
                        .with_vm_execution_mode(self.vm_execution_mode);
                    return AuthorityComputeOutcome::Verification(AuthorityVerificationRequest {
                        request: verification.prepare(budget),
                        execution,
                    });
                }
                ResolutionEvaluation::Enrich(probe) => {
                    let prepared = match probe.prepare_enrichment() {
                        Ok(prepared) => prepared,
                        Err(failure) => {
                            return Self::resolution_failure(failure, execution);
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
                                    return Self::resolution_failure(failure, execution);
                                }
                            };
                            return AuthorityComputeOutcome::Completion(
                                AuthorityComputeCompletion::new(
                                    settlement,
                                    execution,
                                    SettlementOrigin::Completion,
                                    None,
                                ),
                            );
                        }
                    }
                }
            }
        }
    }

    fn resolution_failure(
        failure: super::resolver::ResolutionExecutionFailure,
        execution: AuthorityComputeExecutionPermit,
    ) -> AuthorityComputeOutcome {
        let kind = failure.kind();
        AuthorityComputeOutcome::Completion(AuthorityComputeCompletion::new(
            failure.into_settlement(),
            execution,
            SettlementOrigin::Resolution(kind),
            None,
        ))
    }

    /// Execute one snapshot-bound tx-pool verification request and return its
    /// exact linear completion without mutating authority. The request already
    /// owns the result of its exact cache-key lookup, so callers cannot provide
    /// nearby cached evidence while the cache guard remains open across no
    /// await.
    #[cfg_attr(
        feature = "profiling",
        tracing::instrument(
            name = "tx_pool.stage.verify",
            target = "ckb_tx_pool_profile",
            level = "trace",
            skip_all
        )
    )]
    pub(in crate::authority) async fn execute_verification(
        &self,
        request: AuthorityCacheBoundVerification,
        command_rx: &mut watch::Receiver<ckb_script::ChunkCommand>,
    ) -> AuthorityComputeCompletion {
        let AuthorityCacheBoundVerification {
            request,
            execution: compute_execution,
        } = request;
        let verification = request.execute(command_rx).await;
        AuthorityComputeCompletion::new(
            verification.settlement,
            compute_execution,
            SettlementOrigin::Completion,
            verification.cache_update,
        )
    }

    /// Capture, validate and commit one bounded strongest-first Ready slice.
    /// Common independent candidates share one membership Apply. If any
    /// member has a special validation outcome, only the strongest owner is
    /// disposed and the next iteration captures a fresh coherent cut.
    #[cfg_attr(
        feature = "profiling",
        tracing::instrument(
            name = "tx_pool.stage.ready_attempt",
            target = "ckb_tx_pool_profile",
            level = "trace",
            skip_all
        )
    )]
    pub(in crate::authority) fn try_drive_ready(
        &self,
    ) -> Result<AuthorityReadyDispatch, AuthorityDriverError> {
        let Some(work) = ({
            let store = self.store.read();
            store.capture_ready_work_batch()
        })
        .map_err(AuthorityDriverError::from_initial_ready_capture)?
        else {
            return Ok(AuthorityReadyDispatch::Outcome(AuthorityReadyOutcome::Idle));
        };
        #[cfg(feature = "profiling")]
        let _ready_work_span = tracing::trace_span!(
            target: "ckb_tx_pool_profile",
            "tx_pool.stage.ready_work"
        )
        .entered();
        let prepared = work
            .prepare(self.resolution_policy.min_fee_rate)
            .map_err(AuthorityDriverError::from_ready_preparation)?;
        let rechecked = {
            let store = self.store.read();
            store.complete_ready_batch(prepared)
        }
        .map_err(AuthorityDriverError::from_ready_recheck)?;
        let Some(batch) = rechecked.finish() else {
            return Err(AuthorityDriverError::Stale);
        };

        let mut release_wake = ReadyReleaseWake::new(self);
        let disposition = batch
            .validate()
            .map_err(AuthorityDriverError::from_ready_validation)?;
        let (outcome, reservation) = match disposition {
            ReadyDisposition::Candidates { batch, reservation } => {
                release_wake.disarm();
                return self.apply_ready_input(ReservedReadyPlanInput {
                    input: ReadyPlanInput::Initial(batch),
                    reservation,
                });
            }
            ReadyDisposition::Head {
                outcome,
                reservation,
            } => (outcome, reservation),
        };

        let terminal = self.store.with_read_released(|store| {
            store
                .authority
                .prepare_shared_ready_head_disposition(outcome)
                .map(|prepared| prepared.commit(reservation))
        });
        self.finish_released_ready_head(terminal)
    }

    /// Commit one validated Ready batch. Pure independent members compile
    /// under one coherent outer read cut, release it, then bind and commit
    /// under a shared generation barrier plus exact owner-version shard OCC.
    /// Coupled members retain the exclusive canonical planner. Retired owners and
    /// effects stay move-owned until every authority guard is released and
    /// are then published in commit order.
    pub(in crate::authority) fn resume_ready(
        &self,
        continuation: AuthorityReadyContinuation,
    ) -> Result<AuthorityReadyDispatch, AuthorityDriverError> {
        self.apply_ready_input(continuation.take_input()?)
    }

    /// Compile one strongest-first bounded Ready cohort into exactly one
    /// mechanically compatible wave. Any physical collision, coupling or
    /// capacity failure explicitly cancels every precompiled job and returns
    /// the untouched reservation to the existing canonical aggregate planner.
    /// A later conflict wave cannot be preplanned from this cut because its
    /// aggregate prestate depends on the first wave's actual terminal result.
    fn try_compile_ready_wave(
        &self,
        authority: &TxPoolAuthority,
        batch: &SettlementBatch,
        reservation: ReadyReservation,
    ) -> ReadyWaveCompilation {
        if batch.len() < 2 {
            return ReadyWaveCompilation::Fallback(reservation);
        }
        let compiled = match authority.compile_shared_ready_wave(batch) {
            SharedReadyWaveCompilation::Complete(compiled) => compiled,
            SharedReadyWaveCompilation::Fallback(compiled) => {
                if let Err(error) = self.cancel_unassigned_ready_jobs(compiled) {
                    return ReadyWaveCompilation::Error { reservation, error };
                }
                return ReadyWaveCompilation::Fallback(reservation);
            }
            SharedReadyWaveCompilation::ClockContended {
                compiled,
                contention,
            } => {
                if let Err(error) = self.cancel_unassigned_ready_jobs(compiled) {
                    return ReadyWaveCompilation::Error { reservation, error };
                }
                if let Some(wake) = contention.into_effect_wake() {
                    self.signals.publish_effect_wake(wake);
                }
                return ReadyWaveCompilation::Error {
                    reservation,
                    error: PlanError::Stale(super::plan::StalePlan::ClockBase),
                };
            }
            SharedReadyWaveCompilation::Error { compiled, error } => {
                if let Err(cancel_error) = self.cancel_unassigned_ready_jobs(compiled) {
                    return ReadyWaveCompilation::Error {
                        reservation,
                        error: cancel_error,
                    };
                }
                return ReadyWaveCompilation::Error { reservation, error };
            }
        };
        let mut wave_ends = Vec::new();
        if wave_ends.try_reserve_exact(1).is_err() {
            if let Err(error) = self.cancel_unassigned_ready_jobs(compiled) {
                return ReadyWaveCompilation::Error { reservation, error };
            }
            return ReadyWaveCompilation::Fallback(reservation);
        }
        let compatible = compiled.iter().enumerate().all(|(index, candidate)| {
            compiled
                .iter()
                .take(index)
                .all(|prior| prior.is_compatible_with(candidate))
        });
        if !compatible {
            if let Err(error) = self.cancel_unassigned_ready_jobs(compiled) {
                return ReadyWaveCompilation::Error { reservation, error };
            }
            return ReadyWaveCompilation::Fallback(reservation);
        }
        if compiled.is_empty() {
            return ReadyWaveCompilation::Fallback(reservation);
        }
        wave_ends.push(compiled.len());
        let mut assignments = Vec::new();
        if assignments.try_reserve_exact(compiled.len()).is_err() {
            if let Err(error) = self.cancel_unassigned_ready_jobs(compiled) {
                return ReadyWaveCompilation::Error { reservation, error };
            }
            return ReadyWaveCompilation::Fallback(reservation);
        }
        let (reservations, remainder) = match reservation.try_split_prefix(compiled.len()) {
            Ok(split) => split,
            Err(reservation) => {
                if let Err(error) = self.cancel_unassigned_ready_jobs(compiled) {
                    return ReadyWaveCompilation::Error { reservation, error };
                }
                return ReadyWaveCompilation::Fallback(reservation);
            }
        };
        assignments.extend(compiled.into_iter().zip(reservations).map(
            |(compiled, reservation)| AuthorityReadyCommitAssignment {
                compiled,
                reservation,
            },
        ));
        drop(remainder);
        ReadyWaveCompilation::Wave(AuthorityReadyWave {
            assignments,
            wave_ends,
        })
    }

    /// Publish or route one shared Ready terminal only after the read guard
    /// that produced it has been consumed. This is the only shared aggregate
    /// terminal allowed to call the committed or release-wake publishers.
    fn finish_released_ready_read(
        &self,
        terminal: Released<SharedReadyReadTerminal>,
        input: ReadyPlanInput,
        release_wake: &mut ReadyReleaseWake<'_>,
    ) -> SharedReadyReadDisposition {
        match terminal.into_inner() {
            SharedReadyReadTerminal::Wave(wave) => {
                release_wake.disarm();
                SharedReadyReadDisposition::Terminal(Ok(AuthorityReadyDispatch::Wave(wave)))
            }
            SharedReadyReadTerminal::Committed {
                committed,
                post_commit_fault,
            } => {
                release_wake.disarm();
                let post_commit_fault = self.publish_committed(committed).or(post_commit_fault);
                SharedReadyReadDisposition::Terminal(post_commit_fault.map_or_else(
                    || {
                        Ok(AuthorityReadyDispatch::Outcome(
                            AuthorityReadyOutcome::Applied,
                        ))
                    },
                    |fault| Err(AuthorityDriverError::Fault(fault)),
                ))
            }
            SharedReadyReadTerminal::ReleaseApplied { before } => {
                release_wake.disarm();
                self.publish_authority_release(before);
                SharedReadyReadDisposition::Terminal(Ok(AuthorityReadyDispatch::Outcome(
                    AuthorityReadyOutcome::Applied,
                )))
            }
            SharedReadyReadTerminal::ReleaseFault { before, fault } => {
                release_wake.disarm();
                self.publish_authority_release(before);
                SharedReadyReadDisposition::Terminal(Err(AuthorityDriverError::Fault(fault)))
            }
            SharedReadyReadTerminal::RequiresCanonical {
                reservation,
                continuation,
            } => SharedReadyReadDisposition::RequiresCanonical(ReservedReadyPlanInput {
                input: continuation.map_or(input, ReadyPlanInput::Coupled),
                reservation,
            }),
            SharedReadyReadTerminal::ClockContended {
                reservation,
                effect_wake,
            } => {
                drop(reservation);
                if let Some(wake) = effect_wake {
                    self.signals.publish_effect_wake(wake);
                }
                SharedReadyReadDisposition::Terminal(Err(AuthorityDriverError::Stale))
            }
            SharedReadyReadTerminal::EffectCapacity(reservation) => {
                release_wake.disarm();
                SharedReadyReadDisposition::Terminal(Ok(AuthorityReadyDispatch::Outcome(
                    AuthorityReadyOutcome::EffectCapacity(AuthorityReadyContinuation::new(
                        self,
                        ReservedReadyPlanInput { input, reservation },
                    )),
                )))
            }
            SharedReadyReadTerminal::Error { reservation, error } => {
                drop(reservation);
                SharedReadyReadDisposition::Terminal(Err(error))
            }
        }
    }

    /// Publish one non-candidate Ready-head terminal only after its shared
    /// generation guard has been consumed. The surrounding release bridge stays
    /// armed because every head disposition may return an unselected suffix to
    /// the level-triggered scheduler.
    fn finish_released_ready_head(
        &self,
        terminal: Released<Result<ReadyHeadCommitOutcome, PlanError>>,
    ) -> Result<AuthorityReadyDispatch, AuthorityDriverError> {
        match terminal.into_inner() {
            Err(error) => Err(AuthorityDriverError::from_ready_plan(error)),
            Ok(ReadyHeadCommitOutcome::Committed(committed)) => {
                let (committed, post_commit_fault) = committed.into_parts();
                let post_commit_fault = self.publish_committed(committed).or(post_commit_fault);
                post_commit_fault.map_or_else(
                    || {
                        Ok(AuthorityReadyDispatch::Outcome(
                            AuthorityReadyOutcome::Applied,
                        ))
                    },
                    |fault| Err(AuthorityDriverError::Fault(fault)),
                )
            }
            Ok(ReadyHeadCommitOutcome::Stale { effect_wake }) => {
                if let Some(effect_wake) = effect_wake {
                    self.signals.publish_effect_wake(effect_wake);
                }
                Err(AuthorityDriverError::Stale)
            }
            Ok(ReadyHeadCommitOutcome::Backpressure {
                pressure,
                effect_wake,
            }) => {
                if let Some(effect_wake) = effect_wake {
                    self.signals.publish_effect_wake(effect_wake);
                }
                Err(AuthorityDriverError::from_ready_plan(
                    PlanError::Backpressure(pressure),
                ))
            }
            Ok(ReadyHeadCommitOutcome::Fault { fault, effect_wake }) => {
                if let Some(effect_wake) = effect_wake {
                    self.signals.publish_effect_wake(effect_wake);
                }
                Err(AuthorityDriverError::Fault(fault))
            }
        }
    }

    /// Apply the canonical strongest-first Ready tail without the outer
    /// authority writer.  Each component binds one exact scheduler claim and
    /// one exact routed owner/projection cut; the still-captured tail remains
    /// unplanned until the preceding component has actually committed.
    fn apply_shared_canonical_ready_loop(
        &self,
        authority: &TxPoolAuthority,
        mut input: ReadyPlanInput,
        mut reservation: ReadyReservation,
        mut committed: Vec<CommittedDelta>,
    ) -> SharedCanonicalReadyTerminal {
        debug_assert!(committed.capacity() >= MAX_READY_BATCH);
        let mut post_commit_fault = None;
        for _ in 0..MAX_READY_BATCH {
            let head = match authority.compile_shared_canonical_ready_head(input.batch()) {
                Ok(head) => head,
                Err(PlanError::Backpressure(Backpressure::EffectCapacity)) => {
                    return SharedCanonicalReadyTerminal {
                        committed,
                        post_commit_fault,
                        effect_wake: None,
                        effect_capacity: Some(ReservedReadyPlanInput { input, reservation }),
                        outcome: SharedCanonicalReadyOutcome::Complete,
                    };
                }
                Err(error) => {
                    drop(reservation);
                    return SharedCanonicalReadyTerminal {
                        committed,
                        post_commit_fault,
                        effect_wake: None,
                        effect_capacity: None,
                        outcome: SharedCanonicalReadyOutcome::Error(error),
                    };
                }
            };
            let (compiled, continuation) = head.into_parts();
            if !authority.shared_ready_head_is_current(&reservation, input.batch()) {
                drop(reservation);
                return match compiled.cancel_unassigned_ready_job() {
                    Ok(effect_wake) => SharedCanonicalReadyTerminal {
                        committed,
                        post_commit_fault,
                        effect_wake: Some(effect_wake),
                        effect_capacity: None,
                        outcome: SharedCanonicalReadyOutcome::Progress,
                    },
                    Err(fault) => SharedCanonicalReadyTerminal {
                        committed,
                        post_commit_fault,
                        effect_wake: None,
                        effect_capacity: None,
                        outcome: SharedCanonicalReadyOutcome::Error(PlanError::Fault(fault)),
                    },
                };
            }
            let (mut slots, remainder) = match reservation.try_split_prefix(1) {
                Ok(split) => split,
                Err(reservation) => {
                    let cancelled = compiled.cancel_unassigned_ready_job();
                    return match cancelled {
                        Ok(effect_wake) => SharedCanonicalReadyTerminal {
                            committed,
                            post_commit_fault,
                            effect_wake: Some(effect_wake),
                            effect_capacity: None,
                            outcome: if authority
                                .shared_ready_head_is_current(&reservation, input.batch())
                            {
                                SharedCanonicalReadyOutcome::Error(PlanError::Backpressure(
                                    Backpressure::Allocation,
                                ))
                            } else {
                                SharedCanonicalReadyOutcome::Progress
                            },
                        },
                        Err(fault) => SharedCanonicalReadyTerminal {
                            committed,
                            post_commit_fault,
                            effect_wake: None,
                            effect_capacity: None,
                            outcome: SharedCanonicalReadyOutcome::Error(PlanError::Fault(fault)),
                        },
                    };
                }
            };
            let Some(slot) = slots.pop() else {
                drop(remainder);
                let cancelled = compiled.cancel_unassigned_ready_job();
                return match cancelled {
                    Ok(effect_wake) => SharedCanonicalReadyTerminal {
                        committed,
                        post_commit_fault,
                        effect_wake: Some(effect_wake),
                        effect_capacity: None,
                        outcome: SharedCanonicalReadyOutcome::Error(PlanError::Fault(
                            AuthorityFault::SchedulerProjection,
                        )),
                    },
                    Err(fault) => SharedCanonicalReadyTerminal {
                        committed,
                        post_commit_fault,
                        effect_wake: None,
                        effect_capacity: None,
                        outcome: SharedCanonicalReadyOutcome::Error(PlanError::Fault(fault)),
                    },
                };
            };
            match compiled.commit_ready_job(authority, slot) {
                ReadyJobCommitOutcome::Committed(shared) => {
                    let (committed_delta, fault) = shared.into_parts();
                    committed.push(committed_delta);
                    if let Some(fault) = fault {
                        post_commit_fault = Some(fault);
                        drop(remainder);
                        return SharedCanonicalReadyTerminal {
                            committed,
                            post_commit_fault,
                            effect_wake: None,
                            effect_capacity: None,
                            outcome: SharedCanonicalReadyOutcome::Complete,
                        };
                    }
                }
                ReadyJobCommitOutcome::Stale(effect_wake) => {
                    drop(remainder);
                    return SharedCanonicalReadyTerminal {
                        committed,
                        post_commit_fault,
                        effect_wake: Some(effect_wake),
                        effect_capacity: None,
                        outcome: SharedCanonicalReadyOutcome::Progress,
                    };
                }
                ReadyJobCommitOutcome::Fault { fault, effect_wake } => {
                    drop(remainder);
                    return SharedCanonicalReadyTerminal {
                        committed,
                        post_commit_fault,
                        effect_wake,
                        effect_capacity: None,
                        outcome: SharedCanonicalReadyOutcome::Error(PlanError::Fault(fault)),
                    };
                }
            }
            match (continuation, remainder) {
                (Some(continuation), Some(next_reservation)) => {
                    input = ReadyPlanInput::Coupled(continuation);
                    reservation = next_reservation;
                }
                (None, remainder) => {
                    drop(remainder);
                    return SharedCanonicalReadyTerminal {
                        committed,
                        post_commit_fault,
                        effect_wake: None,
                        effect_capacity: None,
                        outcome: SharedCanonicalReadyOutcome::Complete,
                    };
                }
                (Some(_), None) => {
                    return SharedCanonicalReadyTerminal {
                        committed,
                        post_commit_fault,
                        effect_wake: None,
                        effect_capacity: None,
                        outcome: SharedCanonicalReadyOutcome::Error(PlanError::Fault(
                            AuthorityFault::SchedulerProjection,
                        )),
                    };
                }
            }
        }
        drop(reservation);
        SharedCanonicalReadyTerminal {
            committed,
            post_commit_fault,
            effect_wake: None,
            effect_capacity: None,
            outcome: SharedCanonicalReadyOutcome::Error(PlanError::Fault(
                AuthorityFault::SchedulerProjection,
            )),
        }
    }

    fn finish_released_canonical_ready(
        &self,
        terminal: Released<SharedCanonicalReadyTerminal>,
        release_wake: &mut ReadyReleaseWake<'_>,
    ) -> Result<AuthorityReadyDispatch, AuthorityDriverError> {
        let SharedCanonicalReadyTerminal {
            committed,
            mut post_commit_fault,
            effect_wake,
            effect_capacity,
            outcome,
        } = terminal.into_inner();
        let applied = !committed.is_empty();
        for committed in committed {
            post_commit_fault = self.publish_committed(committed).or(post_commit_fault);
        }
        if let Some(effect_wake) = effect_wake {
            self.signals.publish_effect_wake(effect_wake);
        }
        // An effect-capacity continuation still owns its exact scheduler
        // reservation. Every other canonical terminal may have returned a
        // current or unselected suffix; leave the before/after release bridge
        // armed so that work cannot become visible without a Ready wake.
        if effect_capacity.is_some() {
            release_wake.disarm();
        }
        if let Some(fault) = post_commit_fault {
            return Err(AuthorityDriverError::Fault(fault));
        }
        if let Some(reserved) = effect_capacity {
            return Ok(AuthorityReadyDispatch::Outcome(
                AuthorityReadyOutcome::EffectCapacity(AuthorityReadyContinuation::new(
                    self, reserved,
                )),
            ));
        }
        match outcome {
            SharedCanonicalReadyOutcome::Complete => {
                Ok(AuthorityReadyDispatch::Outcome(if applied {
                    AuthorityReadyOutcome::Applied
                } else {
                    AuthorityReadyOutcome::Idle
                }))
            }
            SharedCanonicalReadyOutcome::Progress => Ok(AuthorityReadyDispatch::Outcome(
                AuthorityReadyOutcome::Applied,
            )),
            SharedCanonicalReadyOutcome::Error(error)
                if applied && matches!(error, PlanError::Stale(_)) =>
            {
                Ok(AuthorityReadyDispatch::Outcome(
                    AuthorityReadyOutcome::Applied,
                ))
            }
            SharedCanonicalReadyOutcome::Error(error) => {
                Err(AuthorityDriverError::from_ready_plan(error))
            }
        }
    }

    fn apply_ready_input(
        &self,
        reserved: ReservedReadyPlanInput,
    ) -> Result<AuthorityReadyDispatch, AuthorityDriverError> {
        let mut release_wake = ReadyReleaseWake::new(self);
        let ReservedReadyPlanInput {
            mut input,
            mut reservation,
        } = reserved;
        if let ReadyPlanInput::Initial(batch) = &input {
            let terminal = self.store.with_read_released(|store| {
                let mut reservation = reservation;
                match self.try_compile_ready_wave(&store.authority, batch, reservation) {
                    ReadyWaveCompilation::Wave(wave) => {
                        return SharedReadyReadTerminal::Wave(wave);
                    }
                    ReadyWaveCompilation::Fallback(returned) => reservation = returned,
                    ReadyWaveCompilation::Error { reservation, error } => {
                        return SharedReadyReadTerminal::Error {
                            reservation,
                            error: AuthorityDriverError::from_ready_plan(error),
                        };
                    }
                }
                match store.authority.compile_shared_independent_settlement(batch) {
                    Ok(SharedIndependentSettlementCompilation::Compiled(compiled)) => {
                        let staged_before = store.authority.wake_projection();
                        match compiled
                            .bind(&store.authority)
                            .and_then(|plan| plan.apply_reserved(reservation))
                        {
                            Ok(committed) => {
                                let (committed, post_commit_fault) = committed.into_parts();
                                SharedReadyReadTerminal::Committed {
                                    committed,
                                    post_commit_fault,
                                }
                            }
                            Err(ConcurrentIndependentError::ChangedCut(_)) => {
                                // Exact generation or owner-version OCC lost to
                                // a competing committed cut. That competing cut
                                // is progress; Ready is recaptured on a later
                                // turn. The released terminal makes the later
                                // store reread structurally lock-external.
                                SharedReadyReadTerminal::ReleaseApplied {
                                    before: staged_before,
                                }
                            }
                            Err(ConcurrentIndependentError::Fault(fault)) => {
                                SharedReadyReadTerminal::ReleaseFault {
                                    before: staged_before,
                                    fault,
                                }
                            }
                        }
                    }
                    Ok(SharedIndependentSettlementCompilation::RequiresCanonical(continuation)) => {
                        SharedReadyReadTerminal::RequiresCanonical {
                            reservation,
                            continuation,
                        }
                    }
                    Ok(SharedIndependentSettlementCompilation::ClockContended(contention)) => {
                        SharedReadyReadTerminal::ClockContended {
                            reservation,
                            effect_wake: contention.into_effect_wake(),
                        }
                    }
                    Err(PlanError::Backpressure(Backpressure::EffectCapacity)) => {
                        SharedReadyReadTerminal::EffectCapacity(reservation)
                    }
                    Err(error) => SharedReadyReadTerminal::Error {
                        reservation,
                        error: AuthorityDriverError::from_ready_plan(error),
                    },
                }
            });
            match self.finish_released_ready_read(terminal, input, &mut release_wake) {
                SharedReadyReadDisposition::Terminal(result) => return result,
                SharedReadyReadDisposition::RequiresCanonical(reserved) => {
                    let ReservedReadyPlanInput {
                        input: returned_input,
                        reservation: returned_reservation,
                    } = reserved;
                    input = returned_input;
                    reservation = returned_reservation;
                }
            }
        }
        let mut committed = Vec::new();
        committed.try_reserve_exact(MAX_READY_BATCH).map_err(|_| {
            AuthorityDriverError::from_ready_plan(PlanError::Backpressure(Backpressure::Allocation))
        })?;
        let terminal = self.store.with_read_released(|store| {
            self.apply_shared_canonical_ready_loop(&store.authority, input, reservation, committed)
        });
        self.finish_released_canonical_ready(terminal, &mut release_wake)
    }
}

impl ReadyWorkBatch {
    fn prepare(
        self,
        min_fee_rate: FeeRate,
    ) -> Result<PreparedReadyValidationBatch, FinalAdmissionCaptureError> {
        let Self {
            reservation,
            snapshot,
            head,
            tail: work_tail,
        } = self;
        let head = FinalAdmissionValidation::prepare(Arc::clone(&snapshot), head, min_fee_rate)
            .map_err(FinalAdmissionCaptureError::Validation)?;
        let mut tail = Vec::new();
        tail.try_reserve(work_tail.len())
            .map_err(|_| FinalAdmissionCaptureError::Allocation)?;
        let mut completed_tail = Vec::new();
        completed_tail
            .try_reserve(work_tail.len())
            .map_err(|_| FinalAdmissionCaptureError::Allocation)?;
        for work in work_tail {
            tail.push(
                FinalAdmissionValidation::prepare(Arc::clone(&snapshot), work, min_fee_rate)
                    .map_err(FinalAdmissionCaptureError::Validation)?,
            );
        }
        Ok(PreparedReadyValidationBatch {
            reservation,
            head,
            tail,
            completed_tail,
        })
    }
}

impl ReadyValidationBatch {
    fn validate(self) -> Result<ReadyDisposition, ReadyValidationError> {
        let Self {
            reservation,
            head,
            tail: validations,
        } = self;
        let head = head.validate().map_err(ReadyValidationError::Candidate)?;
        let mut outcomes = Vec::new();
        outcomes
            .try_reserve(validations.len())
            .map_err(|_| ReadyValidationError::Allocation)?;
        for validation in validations {
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
                return Ok(ReadyDisposition::Head {
                    outcome: other,
                    reservation,
                });
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
                    return Ok(ReadyDisposition::Head {
                        outcome: FinalAdmissionValidationOutcome::Candidate(head.into_receipt()),
                        reservation,
                    });
                }
            }
        }
        Ok(ReadyDisposition::Candidates {
            batch: SettlementBatch::from_validated_ready(head, tail),
            reservation,
        })
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

/// Constructor capability owned by the runtime boundary.
///
/// Planner modules can name this type but cannot construct it, so an
/// `&mut TxPoolAuthority` cannot be replaced with a freshly assembled value
/// outside the runtime initialization cut.
pub(in crate::authority) struct AuthorityInitToken(());

impl AuthorityStore {
    fn from_runtime(
        runtime: AuthorityRuntimeConfig,
        snapshot: Arc<Snapshot>,
    ) -> Result<Self, RuntimeConfigError> {
        let chain_view = ChainViewId::new(ChainRevision(0), snapshot.tip_hash());
        let authority = TxPoolAuthority::from_runtime(
            AuthorityInitToken(()),
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
            committed_txs_hash_cache: Some(LruCache::new(COMMITTED_HASH_CACHE_SIZE)),
        })
    }

    /// First OCC read: clone only the bounded Ready proof shells and paired
    /// snapshot. Per-cell overlay allocation happens after this guard opens.
    fn capture_ready_work_batch(
        &self,
    ) -> Result<Option<ReadyWorkBatch>, FinalAdmissionCaptureError> {
        let Some(reservation) = self
            .authority
            .reserve_ready_candidates()
            .map_err(FinalAdmissionCaptureError::Plan)?
        else {
            return Ok(None);
        };
        let mut candidates = reservation.candidates();
        let Some((head_key, head_expected)) = candidates.next() else {
            return Err(FinalAdmissionCaptureError::Plan(PlanError::Fault(
                AuthorityFault::SchedulerProjection,
            )));
        };
        let head = self
            .authority
            .final_admission_preparation(head_key, head_expected)
            .map_err(FinalAdmissionCaptureError::Plan)?;
        let mut tail = Vec::new();
        tail.try_reserve(candidates.len())
            .map_err(|_| FinalAdmissionCaptureError::Allocation)?;
        for (key, expected) in candidates {
            tail.push(
                self.authority
                    .final_admission_preparation(key, expected)
                    .map_err(FinalAdmissionCaptureError::Plan)?,
            );
        }
        Ok(Some(ReadyWorkBatch {
            reservation,
            snapshot: Arc::clone(&self.snapshot),
            head,
            tail,
        }))
    }

    /// Second OCC read: recheck each exact Ready version and fill only the
    /// preallocated Accepted-origin bits. Any intervening mutation makes the
    /// capture stale rather than mixing two store snapshots.
    fn complete_ready_batch(
        &self,
        batch: PreparedReadyValidationBatch,
    ) -> Result<ReadyRecheckOutcome, FinalAdmissionCaptureError> {
        let prefix_len = self.authority.reserved_ready_common_prefix_len(
            &batch.reservation,
            std::iter::once((batch.head.key(), batch.head.expected())).chain(
                batch
                    .tail
                    .iter()
                    .map(|prepared| (prepared.key(), prepared.expected())),
            ),
        );
        if prefix_len == 0 {
            return Ok(ReadyRecheckOutcome::HeadChanged(batch));
        }
        let PreparedReadyValidationBatch {
            reservation,
            head,
            tail,
            mut completed_tail,
        } = batch;
        let head_work = self
            .authority
            .final_admission_work(head.key(), head.expected())
            .map_err(FinalAdmissionCaptureError::Plan)?;
        let head = head
            .complete(AuthorityStoreCaptureSeal(()), &self.authority, head_work)
            .map_err(FinalAdmissionCaptureError::Validation)?;
        let mut tail = tail.into_iter();
        for prepared in tail.by_ref().take(prefix_len.saturating_sub(1)) {
            let work = self
                .authority
                .final_admission_work(prepared.key(), prepared.expected())
                .map_err(FinalAdmissionCaptureError::Plan)?;
            completed_tail.push(
                prepared
                    .complete(AuthorityStoreCaptureSeal(()), &self.authority, work)
                    .map_err(FinalAdmissionCaptureError::Validation)?,
            );
        }
        Ok(ReadyRecheckOutcome::UnchangedPrefix {
            batch: ReadyValidationBatch {
                reservation,
                head,
                tail: completed_tail,
            },
            discarded_tail: tail,
        })
    }
}

#[cfg(test)]
#[path = "tests/support/runtime.rs"]
pub(in crate::authority) mod test_support;

#[cfg(test)]
#[path = "tests/runtime_unit.rs"]
mod tests;
