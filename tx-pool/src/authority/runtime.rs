//! Production construction and the single physical authority lock domain.
//!
//! This module is the only bridge from process configuration and chain
//! snapshots into the otherwise policy-focused authority kernel.  Keeping the
//! conversion here prevents runtime callers from inventing resource limits,
//! synthetic chain identities, or replacement policy independently.

use super::{
    effect::{EffectBatchBounds, EffectCapacity, EffectConfigError, EffectLimits},
    plan::{
        AuthorityConfigError, ComputeSettlementFailure, MembershipConfig, PlanError,
        TxPoolAuthority,
    },
    resolver::{ResolutionExecutionKind, ResolutionJob, VerificationJob},
    resources::{
        AcceptedResources, ComputeLimits, ResidencyPolicy, ResourceConfigError, ResourceLimits,
        ResourceVector,
    },
    scheduler::VerifyOrder,
    state::{ChainRevision, ChainViewId, EntryVersion, RawTxHash, ValidatedAdmission, WorkPermit},
    validation::{FinalAdmissionValidation, FinalAdmissionValidationError},
    work::{CheckedOutWork, ComputeSettlement},
};
use ckb_app_config::{TxPoolConfig, VerifyOrdering};
use ckb_chain_spec::consensus::Consensus;
use ckb_snapshot::Snapshot;
use ckb_types::packed::{Byte32, ProposalShortId};
use ckb_util::RwLock;
use lru::LruCache;
use std::{num::NonZeroUsize, sync::Arc};
use tokio::sync::Notify;

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

#[derive(Clone, Copy, Debug)]
struct AuthorityRuntimeConfig {
    resources: ResourceLimits,
    verify_order: VerifyOrder,
    effects: EffectLimits,
    membership: MembershipConfig,
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
        let verify_workers = config.max_tx_verify_workers.max(1);
        let resolve_workers = verify_workers
            .min(std::thread::available_parallelism().map_or(4, |count| count.get()))
            .checked_add(1)
            .ok_or(RuntimeConfigError::Arithmetic)?;
        let active_work = resolve_workers
            .checked_add(verify_workers)
            .ok_or(RuntimeConfigError::Arithmetic)?;

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
        .map_err(|_: ResourceConfigError| RuntimeConfigError::ResourceConfiguration)?
        .with_replacement_history_limit(ResourceVector::new(
            REPLACEMENT_HISTORY_MAX_ENTRIES
                .min((retained_entries / HISTORY_RESOURCE_DIVISOR).max(1)),
            REPLACEMENT_HISTORY_MAX_BYTES.min((retained_bytes / HISTORY_RESOURCE_DIVISOR).max(1)),
            (retained_edges / HISTORY_RESOURCE_DIVISOR).max(1),
            0,
        ))
        .map_err(|_: ResourceConfigError| RuntimeConfigError::ResourceConfiguration)?;

        let resident_effect_bytes = config
            .max_tx_pool_size
            .checked_add(retained_bytes)
            .and_then(|bytes| bytes.checked_mul(2))
            .ok_or(RuntimeConfigError::Arithmetic)?;
        let submit_effect_bytes = crate::service::effects::max_submit_effect_bytes(
            config.max_tx_pool_size,
            consensus.max_block_bytes() as usize,
        );
        let reorg_effect_bytes =
            crate::service::effects::max_pool_mutation_effect_bytes(config.max_tx_pool_size)
                .max(4_096);
        if submit_effect_bytes == usize::MAX || reorg_effect_bytes == usize::MAX {
            return Err(RuntimeConfigError::Arithmetic);
        }
        let ordinary_effect_bytes = resident_effect_bytes.max(submit_effect_bytes);
        let max_effects = crate::constants::MAX_POOL_MUTATION_CANDIDATES
            .checked_add(1)
            .ok_or(RuntimeConfigError::Arithmetic)?;
        let effects = EffectLimits::partitioned(
            EffectCapacity::new(
                crate::constants::EFFECT_JOURNAL_REMOTE_MAX_BATCHES,
                ordinary_effect_bytes,
            ),
            EffectCapacity::new(
                crate::constants::EFFECT_TRUSTED_HEADROOM_BATCHES,
                submit_effect_bytes,
            ),
            EffectCapacity::new(1, reorg_effect_bytes),
            EffectBatchBounds::new(
                max_effects,
                ordinary_effect_bytes,
                submit_effect_bytes,
                reorg_effect_bytes,
            ),
        )
        .map_err(|_: EffectConfigError| RuntimeConfigError::EffectConfiguration)?;

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

/// The single physical lock domain of the production tx-pool.
///
/// `snapshot` is chain evidence, not a second transaction owner.  It is kept
/// beside the kernel so a caller cannot publish a new snapshot under an old
/// `ChainViewId`, or vice versa.  The compact-block cache and startup marker
/// are non-authoritative chain-administration metadata.
pub(crate) struct AuthorityStore {
    authority: TxPoolAuthority,
    snapshot: Arc<Snapshot>,
    committed_txs_hash_cache: LruCache<ProposalShortId, Byte32>,
    onchain_reconcile_done: bool,
}

/// Lossy wake hints around the one authoritative scheduler. A hint carries no
/// queue state: every waiter first attempts capability-aware checkout under
/// the store guard, and subscribes before that attempt so a concurrent Apply
/// cannot be missed.
struct AuthoritySignals {
    resolve: Arc<Notify>,
    verify: Arc<Notify>,
}

impl AuthoritySignals {
    fn new() -> Self {
        Self {
            resolve: Arc::new(Notify::new()),
            verify: Arc::new(Notify::new()),
        }
    }

    fn for_permit(&self, permit: WorkPermit) -> &Arc<Notify> {
        match permit {
            WorkPermit::ResolveOnly | WorkPermit::ResolveThenVerify(_) => &self.resolve,
            WorkPermit::VerifyOnly(_) => &self.verify,
        }
    }

    fn publish_mutation(&self) {
        // Publication occurs only after the authority guard has opened and
        // retirement carriers have been destroyed. Waking both lanes avoids
        // a hand-maintained transition-to-signal table; Notify coalesces hints
        // while the scheduler remains the sole level authority.
        self.resolve.notify_waiters();
        self.verify.notify_waiters();
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
}

#[derive(Debug)]
#[must_use = "checked-out authority work must be executed and settled"]
pub(in crate::authority) enum AuthorityComputeJob {
    Resolution(ResolutionJob),
    Verification(VerificationJob),
}

#[derive(Debug)]
#[must_use = "a checkout failure may still own an active settlement capability"]
pub(in crate::authority) enum AuthorityRuntimeError {
    Plan(PlanError),
    Capture(ResolutionExecutionKind),
    Settlement(ComputeSettlementFailure),
}

impl AuthorityRuntime {
    pub(crate) fn new(
        config: &TxPoolConfig,
        consensus: &Consensus,
        snapshot: Arc<Snapshot>,
    ) -> Result<Self, RuntimeConfigError> {
        Ok(Self {
            store: Arc::new(RwLock::new(AuthorityStore::new(
                config, consensus, snapshot,
            )?)),
            signals: Arc::new(AuthoritySignals::new()),
        })
    }

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

    pub(in crate::authority) fn try_checkout(
        &self,
        permit: WorkPermit,
    ) -> Result<Option<AuthorityComputeJob>, AuthorityRuntimeError> {
        let (result, checkout_retirement, settlement_retirement) = {
            let mut store = self.store.write();
            let plan = store
                .authority
                .plan_checkout_next(permit)
                .map_err(AuthorityRuntimeError::Plan)?;
            let Some(plan) = plan else {
                return Ok(None);
            };
            let (work, checkout) = plan.apply().into_parts();
            let snapshot = Arc::clone(&store.snapshot);
            let captured = match work {
                CheckedOutWork::Resolve(work) => {
                    ResolutionJob::capture_resolve(&store.authority, snapshot, work)
                        .map(AuthorityComputeJob::Resolution)
                }
                CheckedOutWork::ContinuousResolve(work) => {
                    ResolutionJob::capture_continuous(&store.authority, snapshot, work)
                        .map(AuthorityComputeJob::Resolution)
                }
                CheckedOutWork::Verify(work) => VerificationJob::from_checkout(work, snapshot)
                    .map(AuthorityComputeJob::Verification),
            };
            match captured {
                Ok(job) => (Ok(Some(job)), checkout, None),
                Err(failure) => {
                    let kind = failure.kind();
                    match store.authority.apply_settlement(failure.into_settlement()) {
                        Ok(settlement) => (
                            Err(AuthorityRuntimeError::Capture(kind)),
                            checkout,
                            Some(settlement),
                        ),
                        Err(failure) => (
                            Err(AuthorityRuntimeError::Settlement(failure)),
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
        result: Result<Option<AuthorityComputeJob>, AuthorityRuntimeError>,
        checkout_retirement: super::plan::CommittedDelta,
        settlement_retirement: Option<super::plan::CommittedDelta>,
        signals: &AuthoritySignals,
    ) -> Result<Option<AuthorityComputeJob>, AuthorityRuntimeError> {
        drop(checkout_retirement);
        drop(settlement_retirement);
        signals.publish_mutation();
        result
    }

    /// Level-triggered checkout with no lock held across the wait. Cancelling
    /// this future while it waits owns no compute capability; after checkout
    /// succeeds there is no suspension point before the job is returned.
    pub(in crate::authority) async fn wait_checkout(
        &self,
        permit: WorkPermit,
    ) -> Result<AuthorityComputeJob, AuthorityRuntimeError> {
        loop {
            let signal = Arc::clone(self.signals.for_permit(permit));
            let notified = signal.notified();
            if let Some(job) = self.try_checkout(permit)? {
                return Ok(job);
            }
            notified.await;
        }
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
}

#[derive(Debug)]
pub(in crate::authority) enum FinalAdmissionCaptureError {
    Plan(PlanError),
    Validation(FinalAdmissionValidationError),
}

impl AuthorityStore {
    pub(crate) fn new(
        config: &TxPoolConfig,
        consensus: &Consensus,
        snapshot: Arc<Snapshot>,
    ) -> Result<Self, RuntimeConfigError> {
        let runtime = AuthorityRuntimeConfig::from_runtime(config, consensus)?;
        let chain_view = ChainViewId::new(ChainRevision(0), snapshot.tip_hash());
        let authority = TxPoolAuthority::from_runtime(
            runtime.resources,
            runtime.verify_order,
            runtime.effects,
            runtime.membership,
            chain_view,
        )
        .map_err(|_: AuthorityConfigError| RuntimeConfigError::AuthorityAllocation)?;
        Ok(Self {
            authority,
            snapshot,
            committed_txs_hash_cache: LruCache::new(COMMITTED_HASH_CACHE_SIZE),
            onchain_reconcile_done: false,
        })
    }

    pub(crate) fn snapshot(&self) -> &Arc<Snapshot> {
        &self.snapshot
    }

    /// Capture Ready ownership, paired chain evidence and the exact bounded
    /// Accepted-origin overlay under this store's single physical guard.
    /// Snapshot and authority view therefore cannot be mixed by a caller.
    pub(in crate::authority) fn capture_final_admission(
        &self,
        key: &RawTxHash,
        expected: EntryVersion,
    ) -> Result<FinalAdmissionValidation, FinalAdmissionCaptureError> {
        let work = self
            .authority
            .final_admission_work(key, expected)
            .map_err(FinalAdmissionCaptureError::Plan)?;
        FinalAdmissionValidation::capture(
            AuthorityStoreCaptureSeal(()),
            &self.authority,
            Arc::clone(&self.snapshot),
            work,
        )
        .map_err(FinalAdmissionCaptureError::Validation)
    }

    pub(crate) fn committed_txs_hash_cache(&mut self) -> &mut LruCache<ProposalShortId, Byte32> {
        &mut self.committed_txs_hash_cache
    }

    pub(crate) fn onchain_reconcile_done(&self) -> bool {
        self.onchain_reconcile_done
    }

    pub(crate) fn mark_onchain_reconciled(&mut self) {
        self.onchain_reconcile_done = true;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AuthorityComputeJob, AuthorityRuntime, AuthorityRuntimeConfig, AuthorityRuntimeError,
        PREACCEPTED_ENTRY_BYTES, RuntimeConfigError,
    };
    use crate::authority::state::{
        ChainRevision, ChainViewId, OwnedTx, PreAcceptedPhase, QueuedWork, ValidatedAdmission,
        WorkPermit,
    };
    use ckb_app_config::{TxPoolConfig, VerifyOrdering};
    use ckb_chain_spec::consensus::ConsensusBuilder;
    use ckb_network::PeerIndex;
    use ckb_snapshot::Snapshot;
    use ckb_test_chain_utils::MockStore;
    use ckb_types::{
        U256,
        core::{FeeRate, TransactionBuilder},
    };
    use std::sync::Arc;

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

    fn admission(nonce: u32, peer: usize) -> ValidatedAdmission {
        ValidatedAdmission::remote(
            TransactionBuilder::default().version(nonce).build(),
            PeerIndex::from(peer),
        )
        .expect("the runtime fixture has valid ingress evidence")
    }

    fn retry(job: AuthorityComputeJob) -> super::super::work::ComputeSettlement {
        match job {
            AuthorityComputeJob::Resolution(job) => job.retry(),
            AuthorityComputeJob::Verification(job) => job.retry(),
        }
    }

    fn is_queued_resolve(runtime: &AuthorityRuntime, key: &super::super::state::RawTxHash) -> bool {
        let store = runtime.store.read();
        matches!(
            store.authority.entry(key),
            Some(OwnedTx::PreAccepted(entry))
                if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
        )
    }

    #[test]
    fn runtime_checkout_observes_preexisting_level_without_a_wake_hint() {
        let runtime = runtime();
        let admission = admission(901, 91);
        let key = admission.identity.raw.clone();
        runtime.admit(admission).expect("admission commits");

        let job = runtime
            .try_checkout(WorkPermit::ResolveOnly)
            .expect("checkout remains healthy")
            .expect("queued work is an authoritative level");
        runtime
            .settle(retry(job))
            .expect("the exact capability returns to resolve");
        assert!(is_queued_resolve(&runtime, &key));
    }

    #[tokio::test]
    async fn runtime_waiter_wakes_after_post_commit_admission_publication() {
        let runtime = runtime();
        let waiter = {
            let runtime = runtime.clone();
            tokio::spawn(async move { runtime.wait_checkout(WorkPermit::ResolveOnly).await })
        };
        tokio::task::yield_now().await;

        runtime
            .admit(admission(902, 92))
            .expect("admission commits before publication");
        let job = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("the post-commit wake cannot be lost")
            .expect("the waiter task remains healthy")
            .expect("the authority runtime remains healthy");
        runtime
            .settle(retry(job))
            .expect("the exact checked-out capability returns");
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
            runtime.try_checkout(WorkPermit::ResolveOnly),
            Err(AuthorityRuntimeError::Capture(
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
}
