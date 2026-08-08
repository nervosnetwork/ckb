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
        AcceptedValidityTransition, ChainValidationError, DirectAdmissionWork, FinalAdmissionWork,
    },
    chain_boundary::{
        ChainBoundaryError, ChainUpdateCommand, ChainUpdateFailure, CommittedChainUpdate,
    },
    effect::{
        EffectConfigError, EffectLimits, EffectProgressError, EffectReceipt, EffectSettlement,
        EffectWork,
    },
    exchange::AuthorityComputeExecutionPermit,
    ingress::{
        DirectCommand, DirectTransaction, RetainedAdmissionBatch, RetainedIngressAttempt, direct,
    },
    plan::{
        AuthorityConfigError, AuthorityFault, AuthorityPostCommit, AuthorityWakeTransition,
        Backpressure, CandidateDispositionPlan, CommittedDelta, ComputeCancellation,
        ComputeCancellationError, ComputeSettlementFailure, DirectAdmissionDisposition,
        DirectAdmissionEvaluation, EffectCloseError, EffectSettlementCommit,
        EffectSettlementFailure, FinalAdmissionDispositionPlan, IndependentCandidate,
        MembershipConfig, MembershipReject, PlanError, SettlementBatch, SettlementPlan,
        TxPoolAuthority,
    },
    query::{
        AuthorityPoolSummary, AuthorityQueryError, AuthorityTransactionLookup,
        AuthorityTransactionStatusLookup, CompactBlockReadReceipt, FeeEstimateReadReceipt,
        LiveCellReadReceipt, PersistenceReceipt,
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
use ckb_logger::error;
use ckb_snapshot::Snapshot;
use ckb_stop_handler::CancellationToken;
use ckb_types::core::FeeRate;
use ckb_types::core::{EntryCompleted, TransactionView};
use ckb_types::packed::{Byte32, OutPoint, ProposalShortId};
use ckb_util::{RwLock, RwLockReadGuard, RwLockWriteGuard, parking_lot::RwLockUpgradableReadGuard};
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
    error: PlanError,
    batch: RetainedAdmissionBatch,
}

impl RetainedIngressBatchFailure {
    pub(super) fn into_parts(self) -> (PlanError, RetainedAdmissionBatch) {
        (self.error, self.batch)
    }
}
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, watch};

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

/// The one physical authority lock and its centralized profiling boundary.
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
    resolve: Notify,
    verify_small: Notify,
    verify_any: Notify,
    ready: Notify,
    maintenance: Notify,
    effect_publisher: Notify,
    effect_capacity: Notify,
    template: Notify,
    effect_publisher_running: AtomicBool,
}

impl AuthoritySignals {
    fn new() -> Self {
        Self {
            resolve: Notify::new(),
            verify_small: Notify::new(),
            verify_any: Notify::new(),
            ready: Notify::new(),
            maintenance: Notify::new(),
            effect_publisher: Notify::new(),
            effect_capacity: Notify::new(),
            template: Notify::new(),
            effect_publisher_running: AtomicBool::new(false),
        }
    }

    fn publish_post_commit(&self, post_commit: AuthorityPostCommit) {
        self.publish_wake(post_commit.publish_metrics_and_take_wake());
    }

    fn publish_post_commit_pair(
        &self,
        first: AuthorityPostCommit,
        second: Option<AuthorityPostCommit>,
    ) {
        // This composition is valid only for checkout followed immediately by
        // capture-failure settlement under the same store guard. No waiter can
        // observe the intermediate Computing owner, and the caller retains the
        // execution capability and must probe again (or report a structural
        // fault). Do not use `then` to combine independently observable Applies:
        // doing so could erase a transient scheduler or resource-ready edge.
        let first = first.publish_metrics_and_take_wake();
        let wake = second.map_or(first, |second| {
            first.then(second.publish_metrics_and_take_wake())
        });
        self.publish_wake(wake);
    }

    fn publish_wake(&self, wake: AuthorityWakeTransition) {
        let compute = wake.compute();
        if compute.resolve() {
            // Resolver and verifier helpers subscribe to this one level. The
            // selected worker receives a typed Resolve intent and therefore
            // cannot consume the baton by servicing an unrelated Verify head.
            self.resolve.notify_one();
        }
        if compute.verify_small() {
            // Small work is executable by every verifier. Any workers share
            // this signal instead of receiving a duplicate class hint.
            self.verify_small.notify_one();
        }
        if compute.verify_any() {
            // This head is distinct from the Small head and requires an Any
            // verifier; the small-only worker never subscribes here.
            self.verify_any.notify_one();
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
        if wake.template_source_advanced() {
            // The five existing lanes retain independent source-cut OCC and
            // publication guards. This is only their common lossy prompt.
            self.template.notify_waiters();
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

    pub(in crate::authority) fn publish(self) -> Result<(), AuthorityEffectPublicationFault> {
        self.settle(EffectTerminalDisposition::Published)
    }

    pub(in crate::authority) fn circuit_dispose(
        self,
    ) -> Result<(), AuthorityEffectPublicationFault> {
        self.settle(EffectTerminalDisposition::CircuitDisposed)
    }

    fn settle(
        mut self,
        disposition: EffectTerminalDisposition,
    ) -> Result<(), AuthorityEffectPublicationFault> {
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
    resolution_policy: ResolutionPolicy,
    expiry_policy: ExpiryPolicy,
    verify_workers: NonZeroUsize,
    transient_compute: ComputeGate,
    #[cfg(test)]
    template_captures: Arc<std::sync::atomic::AtomicUsize>,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use = "maintenance progress determines whether another bounded step is useful"]
pub(super) enum AuthorityMaintenanceOutcome {
    Idle,
    Applied,
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
            PlanError::Duplicate | PlanError::PayloadVariant | PlanError::Membership(_) => {
                Self::Fault(AuthorityFault::MembershipProjection)
            }
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

#[derive(Debug)]
#[must_use = "an executing retained settlement must preserve its execution permit"]
pub(in crate::authority) struct AuthorityComputeSettlement {
    settlement: ComputeSettlement,
    execution: AuthorityComputeExecutionPermit,
}

/// Lock-external retained-compute result. It owns the exact settlement,
/// execution permit and post-commit cache consequence until execution is
/// explicitly finished. Finishing returns the permit to the shared fair gate
/// before any authority settlement attempt. No execution path can mutate
/// authority by constructing this value.
#[derive(Debug)]
#[must_use = "a completed retained computation must be settled exactly once"]
pub(in crate::authority) struct AuthorityComputeCompletion {
    retained: AuthorityComputeSettlement,
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

impl AuthorityComputeCompletion {
    fn new(
        settlement: ComputeSettlement,
        execution: AuthorityComputeExecutionPermit,
        origin: SettlementOrigin,
        cache_update: Option<VerificationCacheUpdate>,
    ) -> Self {
        Self {
            retained: AuthorityComputeSettlement {
                settlement,
                execution,
            },
            aftermath: AuthorityComputeAftermath {
                origin,
                cache_update,
            },
        }
    }

    pub(in crate::authority) fn finish_execution(self) -> AuthorityFinishedCompute {
        let Self {
            retained:
                AuthorityComputeSettlement {
                    settlement,
                    execution,
                },
            aftermath,
        } = self;
        drop(execution);
        AuthorityFinishedCompute {
            settlement,
            aftermath,
        }
    }
}

impl AuthorityComputeAftermath {
    pub(in crate::authority) fn origin(&self) -> SettlementOrigin {
        self.origin
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

    fn retry(self) -> AuthorityComputeCompletion {
        AuthorityComputeCompletion::new(
            self.request.retry(),
            self.execution,
            SettlementOrigin::Completion,
            None,
        )
    }
}

#[derive(Debug)]
pub(in crate::authority) enum AuthorityComputeError {
    Allocation,
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
    aftermath: AuthorityComputeAftermath,
}

/// A capture failure is discovered while the authority guard is held. This
/// short-lived seed carries the execution permit out of that guard so its fair
/// semaphore wake cannot contend with the same critical section.
struct PendingSettlementWithExecution {
    failure: ComputeSettlementFailure,
    origin: SettlementOrigin,
    execution: AuthorityComputeExecutionPermit,
}

impl PendingSettlementWithExecution {
    fn finish_execution(self) -> AuthorityPendingSettlement {
        let Self {
            failure,
            origin,
            execution,
        } = self;
        drop(execution);
        AuthorityPendingSettlement {
            failure,
            aftermath: AuthorityComputeAftermath {
                origin,
                cache_update: None,
            },
        }
    }
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
}

enum EffectPublicationState {
    Idle,
    Receipt(EffectReceipt),
    ClosedAndDrained,
}

#[must_use = "an idle checkout returns the still-owned execution slot"]
#[expect(
    clippy::large_enum_variant,
    reason = "boxing would allocate on every non-idle checkout; the enum crosses no storage boundary and is consumed immediately"
)]
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

    fn from_config(
        runtime: AuthorityRuntimeConfig,
        snapshot: Arc<Snapshot>,
    ) -> Result<Self, RuntimeConfigError> {
        let resolution_policy = runtime.resolution_policy;
        let expiry_policy = runtime.expiry_policy;
        let verify_workers = runtime.verify_workers;
        let transient_compute = ComputeGate::new(runtime.transient_compute_permits);
        Ok(Self {
            store: Arc::new(AuthorityStoreLock::new(AuthorityStore::from_runtime(
                runtime, snapshot,
            )?)),
            signals: Arc::new(AuthoritySignals::new()),
            resolution_policy,
            expiry_policy,
            verify_workers,
            transient_compute,
            #[cfg(test)]
            template_captures: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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
            let plan = store
                .authority
                .plan_local_removal(&RawTxHash(hash.clone()))
                .map_err(AuthorityAdministrationError::from_plan)?;
            let Some(plan) = plan else {
                return Ok(false);
            };
            plan.apply()
        };
        self.publish_committed(committed);
        Ok(true)
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
        self.publish_committed(committed);
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
        let post_commit = committed.into_post_commit();
        drop(retired_snapshot);
        drop(retired_hash_cache);
        self.signals.publish_post_commit(post_commit);
        Ok(())
    }

    /// Commit one ordered chain transition against the exact supplied
    /// snapshot. The upgradable read excludes intervening writers while the
    /// semantic disposition and proposal evidence are compiled, without
    /// adding a gate to ordinary admission. After upgrade, only capacity and
    /// derived-projection preparation plus total Apply remain.
    #[expect(
        clippy::result_large_err,
        reason = "failure returns the exact sealed chain command for ordered recovery; boxing would allocate on reorg backpressure"
    )]
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

        let mut store = AuthorityStoreLock::upgrade(store);
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
        let post_commit = committed.into_post_commit();
        drop(retired_snapshot);
        self.signals.publish_post_commit(post_commit);
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
            let plan = store
                .authority
                .plan_remote_expiry(cutoff, self.expiry_policy.remote_slice)
                .map_err(AuthorityDriverError::from_maintenance_plan)?;
            let Some(plan) = plan else {
                return Ok(AuthorityMaintenanceOutcome::Idle);
            };
            plan.apply()
        };
        self.publish_committed(committed);
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
        let committed = {
            let mut store = self.store.write();
            let plan = store
                .authority
                .plan_accepted_expiry(cutoff)
                .map_err(AuthorityDriverError::from_maintenance_plan)?;
            let Some(plan) = plan else {
                return Ok(AuthorityMaintenanceOutcome::Idle);
            };
            plan.apply()
        };
        self.publish_committed(committed);
        Ok(AuthorityMaintenanceOutcome::Applied)
    }

    /// Advance one dirty dependency edge or completion marker. The dependency
    /// frontier is level-triggered, so callers may repeat this bounded step
    /// until `Idle` without owning a second queue or cursor.
    pub(super) fn maintain_dependency(
        &self,
    ) -> Result<AuthorityMaintenanceOutcome, AuthorityDriverError> {
        let committed = {
            let mut store = self.store.write();
            let plan = store
                .authority
                .plan_dependency_maintenance()
                .map_err(AuthorityDriverError::from_maintenance_plan)?;
            let Some(plan) = plan else {
                return Ok(AuthorityMaintenanceOutcome::Idle);
            };
            plan.apply()
        };
        self.publish_committed(committed);
        Ok(AuthorityMaintenanceOutcome::Applied)
    }

    fn publish_committed(&self, committed: CommittedDelta) {
        self.signals
            .publish_post_commit(committed.into_post_commit());
    }

    pub(super) fn resolve_signal(&self) -> &Notify {
        &self.signals.resolve
    }

    pub(super) fn verify_small_signal(&self) -> &Notify {
        &self.signals.verify_small
    }

    pub(super) fn verify_any_signal(&self) -> &Notify {
        &self.signals.verify_any
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
        self.transient_compute
            .acquire(cancel)
            .await
            .map(AuthorityComputeExecutionPermit::new)
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
        self.publish_committed(committed);
        Ok(AuthorityInternalPlugOutcome::Inserted)
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
        self.publish_committed(committed);
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
            DirectAdmissionValidationOutcome::Reresolve(retry) => {
                return Ok(AuthorityLocalAdmissionOutcome::Retry(
                    retry.into_subject().into_transaction(),
                ));
            }
            DirectAdmissionValidationOutcome::Candidate(receipt) => {
                let completed = receipt.completed();
                let mut store = self.store.write();
                match store.authority.plan_direct_admission(receipt)? {
                    DirectAdmissionDisposition::Accepted(plan) => (
                        AuthorityLocalAdmissionOutcome::Accepted(completed),
                        plan.apply(),
                    ),
                    DirectAdmissionDisposition::Duplicate(plan) => {
                        let (key, committed) = plan.apply();
                        (AuthorityLocalAdmissionOutcome::Duplicate(key), committed)
                    }
                    DirectAdmissionDisposition::Rejected(plan) => {
                        let (reason, committed) = plan.apply();
                        (
                            AuthorityLocalAdmissionOutcome::Rejected(
                                DirectAdmissionRejectionKind::Membership(reason),
                            ),
                            committed,
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
                    committed,
                )
            }
        };
        self.publish_committed(committed);
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

    fn try_effect_publication(&self) -> EffectPublicationState {
        let store = self.store.read();
        match store.authority.effect_publication_receipt() {
            Some(receipt) => EffectPublicationState::Receipt(receipt),
            None if store.authority.effects_closed_and_drained() => {
                EffectPublicationState::ClosedAndDrained
            }
            None => EffectPublicationState::Idle,
        }
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
                EffectPublicationState::Idle => notified.await,
                EffectPublicationState::Receipt(receipt) => {
                    return Some(AuthorityEffectPublicationLease {
                        runtime: self,
                        receipt: Some(receipt),
                        _claim: claim,
                    });
                }
                EffectPublicationState::ClosedAndDrained => return None,
            }
        }
    }

    fn settle_effect(&self, settlement: EffectSettlement) -> Result<(), EffectSettlementFailure> {
        let rejection_metrics = settlement.rejection_metrics();
        let commit = {
            let mut store = self.store.write();
            store.authority.apply_effect_settlement(settlement)?
        };
        match commit {
            EffectSettlementCommit::Applied(retirement) => self.publish_committed(retirement),
            EffectSettlementCommit::Superseded(settlement) => drop(settlement),
        }
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
    /// capability has drained. Already committed queued/reset effects remain
    /// publishable until `effects_closed_and_drained` becomes true.
    pub(in crate::authority) fn close_effects(&self) -> Result<(), EffectCloseError> {
        let retirement = {
            let mut store = self.store.write();
            store.authority.plan_effect_close()?.apply()
        };
        self.publish_committed(retirement);
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

    #[expect(
        clippy::result_large_err,
        reason = "a failed Plan returns the exact move-only ingress batch without allocating, including when allocation pressure caused the failure"
    )]
    pub(super) fn commit_retained_ingress_batch(
        &self,
        batch: RetainedAdmissionBatch,
    ) -> Result<
        (usize, std::collections::VecDeque<RetainedIngressAttempt>),
        RetainedIngressBatchFailure,
    > {
        let (retirement, consumed) = {
            let mut store = self.store.write();
            let prepared = match store.authority.plan_retained_admission_batch(&batch) {
                Ok(prepared) => prepared,
                Err(error) => return Err(RetainedIngressBatchFailure { error, batch }),
            };
            if prepared.consumed() == 0 || prepared.consumed() > batch.len() {
                return Err(RetainedIngressBatchFailure {
                    error: PlanError::Fault(AuthorityFault::MembershipProjection),
                    batch,
                });
            }
            match prepared.apply() {
                super::plan::CommittedRetainedAdmissionBatch::Applied {
                    retirement,
                    consumed,
                    ..
                } => (retirement, consumed),
                super::plan::CommittedRetainedAdmissionBatch::Unchanged { consumed, .. } => {
                    let mut remaining = batch.into_attempts();
                    for _ in 0..consumed {
                        drop(remaining.pop_front());
                    }
                    return Ok((consumed, remaining));
                }
            }
        };
        self.publish_committed(retirement);
        let mut remaining = batch.into_attempts();
        for _ in 0..consumed {
            drop(remaining.pop_front());
        }
        Ok((consumed, remaining))
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
                            Ok(ControlFlow::Break(PendingSettlementWithExecution {
                                failure,
                                origin: SettlementOrigin::Capture(kind),
                                execution,
                            })),
                            checkout,
                            None,
                        ),
                    }
                }
            }
        };
        let checkout_post_commit = checkout_retirement.into_post_commit();
        let settlement_post_commit = settlement_retirement.map(CommittedDelta::into_post_commit);
        self.signals
            .publish_post_commit_pair(checkout_post_commit, settlement_post_commit);
        result.map(|flow| match flow {
            ControlFlow::Continue(checkout) => ControlFlow::Continue(checkout),
            ControlFlow::Break(pending) => ControlFlow::Break(pending.finish_execution()),
        })
    }

    pub(in crate::authority) fn settle_compute(
        &self,
        retained: AuthorityComputeSettlement,
        origin: SettlementOrigin,
    ) -> ControlFlow<AuthorityPendingSettlement> {
        let completion = AuthorityComputeCompletion {
            retained,
            aftermath: AuthorityComputeAftermath {
                origin,
                cache_update: None,
            },
        };
        match self.settle_completion(completion) {
            ControlFlow::Continue(_) => ControlFlow::Continue(()),
            ControlFlow::Break(pending) => ControlFlow::Break(pending),
        }
    }

    pub(in crate::authority) fn settle_completion(
        &self,
        completion: AuthorityComputeCompletion,
    ) -> ControlFlow<AuthorityPendingSettlement, AuthorityComputeAftermath> {
        self.settle_finished(completion.finish_execution())
    }

    pub(in crate::authority) fn settle_finished(
        &self,
        finished: AuthorityFinishedCompute,
    ) -> ControlFlow<AuthorityPendingSettlement, AuthorityComputeAftermath> {
        let AuthorityFinishedCompute {
            settlement,
            aftermath,
        } = finished;
        match self.settle(settlement) {
            Ok(()) => ControlFlow::Continue(aftermath),
            Err(failure) => ControlFlow::Break(
                AuthorityPendingSettlement::from_completion_failure(failure, aftermath),
            ),
        }
    }

    pub(in crate::authority) fn retry_unexpected_verification(
        &self,
        request: AuthorityVerificationRequest,
    ) -> ControlFlow<AuthorityPendingSettlement> {
        match self.settle_completion(request.retry()) {
            ControlFlow::Continue(_) => ControlFlow::Continue(()),
            ControlFlow::Break(pending) => ControlFlow::Break(pending),
        }
    }

    #[expect(
        clippy::result_large_err,
        reason = "failure returns the exact move-only compute settlement for retry or cancellation; boxing would allocate on backpressure"
    )]
    pub(in crate::authority) fn settle(
        &self,
        settlement: ComputeSettlement,
    ) -> Result<(), ComputeSettlementFailure> {
        let committed = {
            let mut store = self.store.write();
            store.authority.apply_settlement(settlement)?
        };
        self.publish_committed(committed);
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
        self.publish_committed(committed);
        Ok(())
    }

    /// Execute one resolve capability entirely outside the authority guard.
    /// A bounded dep-group miss may take allocation-free Accepted read cuts;
    /// every terminal or retry result is returned as one linear completion.
    /// This method performs no authoritative mutation.
    pub(in crate::authority) fn execute_compute(
        &self,
        job: AuthorityComputeJob,
    ) -> Result<AuthorityComputeOutcome, AuthorityComputeError> {
        let AuthorityComputeJob { inner, execution } = job;
        match inner {
            AuthorityComputeKind::Resolution(job) => self.execute_resolution(job, execution),
            AuthorityComputeKind::Verification(job) => Ok(AuthorityComputeOutcome::Verification(
                AuthorityVerificationRequest {
                    request: job.prepare(),
                    execution,
                },
            )),
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
    ) -> Result<AuthorityComputeOutcome, AuthorityComputeError> {
        loop {
            let policy = self.resolution_policy;
            let evaluated = crate::util::block_offload(|| {
                job.evaluate(policy.min_fee_rate, policy.large_cycle_threshold)
            });
            let evaluation = match evaluated {
                Ok(evaluation) => evaluation,
                Err(failure) => return Ok(Self::resolution_failure(failure, execution)),
            };
            match evaluation {
                ResolutionEvaluation::Settle(settlement) => {
                    return Ok(AuthorityComputeOutcome::Completion(
                        AuthorityComputeCompletion::new(
                            settlement,
                            execution,
                            SettlementOrigin::Completion,
                            None,
                        ),
                    ));
                }
                ResolutionEvaluation::Verify(verification) => {
                    return Ok(AuthorityComputeOutcome::Verification(
                        AuthorityVerificationRequest {
                            request: verification.prepare(),
                            execution,
                        },
                    ));
                }
                ResolutionEvaluation::Enrich(probe) => {
                    let prepared = match probe.prepare_enrichment() {
                        Ok(prepared) => prepared,
                        Err(failure) => {
                            return Ok(Self::resolution_failure(failure, execution));
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
                                    return Ok(Self::resolution_failure(failure, execution));
                                }
                            };
                            return Ok(AuthorityComputeOutcome::Completion(
                                AuthorityComputeCompletion::new(
                                    settlement,
                                    execution,
                                    SettlementOrigin::Completion,
                                    None,
                                ),
                            ));
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
        command_rx: Option<&mut watch::Receiver<ckb_script::ChunkCommand>>,
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
    ) -> Result<AuthorityReadyOutcome, AuthorityDriverError> {
        let Some(work) = ({
            let store = self.store.read();
            store.capture_ready_work_batch()
        })
        .map_err(AuthorityDriverError::from_initial_ready_capture)?
        else {
            return Ok(AuthorityReadyOutcome::Idle);
        };
        #[cfg(feature = "profiling")]
        let _ready_work_span = tracing::trace_span!(
            target: "ckb_tx_pool_profile",
            "tx_pool.stage.ready_work"
        )
        .entered();
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
        let committed = {
            let mut store = self.store.write();
            match disposition {
                ReadyDisposition::Candidates(batch) => {
                    let plan = store
                        .authority
                        .plan_settlement(&batch)
                        .map_err(AuthorityDriverError::from_ready_plan)?;
                    match plan {
                        SettlementPlan::IndependentRun(plan) => plan.apply(),
                        SettlementPlan::CoupledComponent(disposition) => match disposition {
                            CandidateDispositionPlan::Accepted(plan) => plan.apply(),
                            CandidateDispositionPlan::Rejected(plan) => plan.apply().1,
                        },
                    }
                }
                ReadyDisposition::Head(outcome) => {
                    let plan = store
                        .authority
                        .plan_final_admission(outcome)
                        .map_err(AuthorityDriverError::from_ready_plan)?;
                    match plan {
                        FinalAdmissionDispositionPlan::Candidate(plan) => match plan {
                            CandidateDispositionPlan::Accepted(plan) => plan.apply(),
                            CandidateDispositionPlan::Rejected(plan) => plan.apply().1,
                        },
                        FinalAdmissionDispositionPlan::ValidationRejected(plan) => plan.apply().1,
                        FinalAdmissionDispositionPlan::Reresolve(plan) => plan.apply(),
                    }
                }
            }
        };
        self.publish_committed(committed);
        Ok(AuthorityReadyOutcome::Applied)
    }
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
    /// capture stale rather than mixing two store snapshots.
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
#[path = "tests/support/runtime.rs"]
pub(in crate::authority) mod test_support;

#[cfg(test)]
#[path = "tests/runtime_unit.rs"]
mod tests;
