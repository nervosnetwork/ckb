use super::super::chain::{AcceptedProof, ProposalContextReceipt};
use super::super::effect::{
    CommittedAcceptance, CommittedEffect, CommittedRejection, EffectBatchBound, EffectBatchBounds,
    EffectCapacity, EffectLimits, EffectPolicy, RejectionAudience,
};
use super::super::ingress::{
    RetainedAdmissionBatch, RetainedIngressAttempt, test_support::proposal_for_foundation,
};
use super::super::plan::{
    AuthorityFault, Backpressure, CandidateDispositionPlan, CommittedDelta,
    ComputeSettlementFailure, ComputeSettlementRecovery, ConcurrentRetainedIngressError,
    MembershipReject, PlanError, PreparedApply, PreparedIndependentApply,
    PreparedSharedDirectAdmissionDisposition, PreparedSharedLocalRemoval, RemovalCause,
    SettlementBatch, SettlementPlan, SharedDirectAdmissionCommitOutcome, StalePlan,
    TxPoolAuthority,
    test_support::{CandidateBatchError, CommittedCheckout, ComponentLimitKind},
};
use super::super::resources::{
    AcceptedCost, AcceptedResources, ChargeRecord, ComputeGrant, ComputeLimits, ResidencyPolicy,
    ResourceConfigError, ResourceError, ResourceLimits, ResourceVector,
    test_support::{ResourceSnapshot, TestResourceLedger},
};
use super::super::runtime::{
    AuthorityAdministrationError, AuthorityMaintenanceOutcome, AuthorityRuntime,
};
use super::super::scheduler::VerifyOrder;
use super::super::shard::{
    AUTHORITY_SHARD_COUNT, AuthorityShardRouter, ConcurrentRemovalProbe, ShardedOwnerMap,
};
use super::super::source::TemplateSelectionSource;
use super::super::state::{
    AcceptedAtMillis, AcceptedEntry, AcceptedProvenance, AcceptedStatus, ActiveWork, ApplySequence,
    CandidateMetrics, ChainRevision, ChainViewId, ComputeAttribution, DependencyCut, DependencyKey,
    EntryVersion, ExpandedFootprint, FootprintError, KnownDependencies, ObservedDependencies,
    OwnedTx, PayloadPolicy, PoolGeneration, PreAcceptedPhase, PreAcceptedSource, ProposalBase,
    QueuedWork, RawTxHash, RemoteDeadline, RemoteResidencyLease, ResolvedPayload, TxIdentity,
    ValidatedAdmission, VerifiedFacts, VerifyCapability, VerifyCycleClass, WorkPermit,
    test_support::{FoundationResolution, RejectionKind},
};
use super::super::work::{
    CheckedOutWork, ComputeSettlement, ContinuousResolution, ContinuousResolveWork,
    ContinuousVerifyWork, ResolutionEvidence, ResolutionReceiptError, ResolveWork, SettlementNext,
    SettlementToken, VerifyWork,
};
use crate::{
    component::entry::{accepted_transaction_charge_bytes, resolved_transaction_charge_bytes},
    constants::MAX_READY_BATCH,
    error::Reject,
};
use ckb_app_config::{TxPoolConfig, VerifyOrdering};
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_network::PeerIndex;
use ckb_snapshot::Snapshot;
use ckb_test_chain_utils::MockStore;
use ckb_types::{
    U256,
    bytes::Bytes,
    core::{
        Capacity, Cycle, EpochNumberWithFraction, FeeRate, TransactionBuilder, TransactionInfo,
        TransactionView,
        cell::{CellMetaBuilder, ResolvedTransaction},
        tx_pool::get_transaction_weight,
    },
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint},
    prelude::{Builder, Entity, Pack},
};
use ckb_verification::cache::ScriptVerificationRules;
use std::collections::{HashMap, HashSet, VecDeque};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

pub(super) fn runtime_config() -> TxPoolConfig {
    TxPoolConfig {
        max_tx_pool_size: 180_000_000,
        max_tx_pool_resident_size: 1_000_000_000,
        min_fee_rate: FeeRate::zero(),
        min_rbf_rate: FeeRate::zero(),
        max_tx_verify_cycles: 70_000_000,
        min_tx_verify_time_ms: 250,
        tx_verify_cycles_per_ms: 10_000,
        max_tx_verify_time_ms: 30_000,
        max_tx_verify_initial_load_bytes: 256 * 1024 * 1024,
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

pub(super) fn genesis_snapshot() -> Arc<Snapshot> {
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

pub(super) fn limits() -> ResourceLimits {
    ResourceLimits::new(
        ResourceVector::new(8, 64 * 1024, 64, 8),
        ResourceVector::new(4, 32 * 1024, 32, 4),
        ResourceVector::new(2, 16 * 1024, 16, 2),
        AcceptedResources::new(8, 64 * 1024, 64 * 1024, 64),
        ComputeLimits::new(4 * 1024, 4 * 1024, 16),
    )
    .and_then(|limits| {
        limits.with_replacement_history_limit(ResourceVector::new(4, 32 * 1024, 32, 0))
    })
    .expect("fixture limits admit one indivisible grant")
}

#[test]
fn uak_resource_configuration_rejects_invalid_hierarchy_and_transient_bounds() {
    assert!(matches!(
        ResourceLimits::new(
            ResourceVector::new(1, 1024, 8, 1),
            ResourceVector::new(2, 1024, 8, 1),
            ResourceVector::new(1, 1024, 8, 1),
            AcceptedResources::new(1, 1024, 1024, 1),
            ComputeLimits::new(512, 512, 4),
        ),
        Err(ResourceConfigError::LimitHierarchy)
    ));
    assert!(matches!(
        ResourceLimits::new(
            ResourceVector::new(1, 1024, 8, 1),
            ResourceVector::new(1, 1024, 8, 1),
            ResourceVector::new(1, 256, 8, 0),
            AcceptedResources::new(1, 1024, 1024, 1),
            ComputeLimits::new(512, 512, 4),
        ),
        Err(ResourceConfigError::MissingComputeCapacity)
    ));
    assert!(matches!(
        ResourceLimits::new(
            ResourceVector::new(1, 1024, 8, 1),
            ResourceVector::new(1, 1024, 8, 1),
            ResourceVector::new(1, 1024, 8, 1),
            AcceptedResources::new(1, 1024, 1024, 1),
            ComputeLimits::new(513, 512, 4),
        ),
        Err(ResourceConfigError::NonMonotonicComputeEnvelope)
    ));
    assert!(matches!(
        ResourceLimits::new(
            ResourceVector::new(1, usize::MAX, 8, 1),
            ResourceVector::new(1, usize::MAX, 8, 1),
            ResourceVector::new(1, usize::MAX, 8, 1),
            AcceptedResources::new(1, 1024, 1024, 1),
            ComputeLimits::new(1, 1, 4),
        ),
        Err(ResourceConfigError::TransientComputeOverflow)
    ));

    let history_base = ResourceLimits::new(
        ResourceVector::new(2, 2048, 16, 2),
        ResourceVector::new(2, 2048, 16, 2),
        ResourceVector::new(1, 1024, 8, 1),
        AcceptedResources::new(2, 2048, 2048, 2),
        ComputeLimits::new(512, 512, 8),
    )
    .expect("replacement-history fixture has a valid base hierarchy");
    assert!(matches!(
        history_base.with_replacement_history_limit(ResourceVector::new(3, 2048, 16, 0)),
        Err(ResourceConfigError::LimitHierarchy)
    ));
    assert!(matches!(
        history_base.with_replacement_history_limit(ResourceVector::new(1, 1024, 8, 1)),
        Err(ResourceConfigError::LimitHierarchy)
    ));
}

#[test]
fn uak_admission_must_fit_the_static_compute_envelope() {
    let constrained = ResourceLimits::new(
        ResourceVector::new(4, 4096, 32, 2),
        ResourceVector::new(4, 4096, 32, 2),
        ResourceVector::new(4, 4096, 32, 2),
        AcceptedResources::new(4, 4096, 4096, 64),
        ComputeLimits::new(64, 64, 8),
    )
    .expect("the fixed envelope has a checked physical ceiling");
    let mut authority = TxPoolAuthority::for_foundation(constrained);
    let oversized = TransactionBuilder::default()
        .output(CellOutput::default())
        .output_data(Bytes::from(vec![0; 128]).pack())
        .build();
    let admission = ValidatedAdmission::remote(oversized, PeerIndex::from(1))
        .expect("the transaction itself has valid ingress facts");
    let before = authority.normalized_snapshot();
    assert_eq!(
        authority.plan_admission(admission).err(),
        Some(PlanError::Backpressure(Backpressure::ComputeResources))
    );
    assert_eq!(authority.normalized_snapshot(), before);
}

pub(super) fn tx(nonce: u64) -> ckb_types::core::TransactionView {
    TransactionBuilder::default().version(nonce as u32).build()
}

fn observed(epoch: u64) -> ObservedDependencies {
    ObservedDependencies::for_foundation(
        vec![DependencyKey::Cell(OutPoint::default())],
        DependencyCut(ApplySequence(u128::from(epoch))),
    )
    .expect("fixture dependency set is non-empty")
}

pub(in crate::authority) fn missing_keys() -> Vec<DependencyKey> {
    vec![DependencyKey::Cell(OutPoint::default())]
}

pub(super) fn admit_remote(
    authority: &mut TxPoolAuthority,
    nonce: u64,
    peer: usize,
) -> super::super::state::RawTxHash {
    let admission = ValidatedAdmission::remote(tx(nonce), PeerIndex::from(peer))
        .expect("fixture admission is valid");
    let hash = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("fixture admission plans"),
    );
    hash
}

pub(in crate::authority) fn admit_remote_until(
    authority: &mut TxPoolAuthority,
    nonce: u64,
    peer: usize,
    expires_at: u64,
) -> RawTxHash {
    let admission = ValidatedAdmission::remote_with_lease(
        tx(nonce),
        RemoteResidencyLease::new(PeerIndex::from(peer), RemoteDeadline(expires_at)),
        0,
    )
    .expect("fixture admission has one checked residency lease");
    let hash = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("deadline fixture admission plans"),
    );
    hash
}

fn queue_remote_for_verify(
    authority: &mut TxPoolAuthority,
    transaction: TransactionView,
    peer: usize,
    fee: Capacity,
) -> RawTxHash {
    let admission = ValidatedAdmission::remote(transaction.clone(), PeerIndex::from(peer))
        .expect("fixture admission is valid");
    let hash = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("fixture admission plans"),
    );
    let version = owner_version(authority, &hash);
    let (_, resolve) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(&hash, version, WorkPermit::ResolveOnly)
            .expect("fixture resolve checkout plans")
            .apply(),
    );
    let payload = resolved_payload_with_facts(&transaction, Vec::new(), Vec::new(), fee);
    apply_plan(
        authority
            .apply_settlement(
                resolve
                    .yield_verify(payload)
                    .expect("fixture payload belongs to resolve work"),
            )
            .expect("fixture resolve settlement plans"),
    );
    hash
}

pub(super) fn checkout_remote_for_verify_with_claim(
    authority: &mut TxPoolAuthority,
    transaction: &TransactionView,
    peer: PeerIndex,
    declared_cycles: u64,
) -> (RawTxHash, VerifyWork) {
    let admission = ValidatedAdmission::remote_with_lease(
        transaction.clone(),
        RemoteResidencyLease::for_foundation(peer),
        declared_cycles,
    )
    .expect("remote cycle claim is admitted with its exact payload");
    let hash = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("remote cycle fixture enters ownership"),
    );
    let (_, resolve) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(
                &hash,
                owner_version(authority, &hash),
                WorkPermit::ResolveOnly,
            )
            .expect("remote resolve checkout plans")
            .apply(),
    );
    apply_plan(
        authority
            .apply_settlement(
                resolve
                    .yield_verify(resolved_payload_with_facts(
                        transaction,
                        Vec::new(),
                        Vec::new(),
                        Capacity::shannons(1),
                    ))
                    .expect("resolved payload belongs to the remote work"),
            )
            .expect("remote resolution queues verification"),
    );
    let checkout = authority
        .plan_checkout_for_foundation(
            &hash,
            owner_version(authority, &hash),
            WorkPermit::VerifyOnly(VerifyCapability::Any),
        )
        .expect("remote verify checkout plans")
        .apply();
    let CheckedOutWork::Verify(verify) = checkout.into_work() else {
        panic!("verify-only checkout returns verify work");
    };
    (hash, verify)
}

pub(super) fn owner_version(
    authority: &TxPoolAuthority,
    hash: &super::super::state::RawTxHash,
) -> EntryVersion {
    authority
        .entry(hash)
        .expect("owner exists")
        .record()
        .version
}

pub(super) trait FixtureCommit {
    fn into_committed(self) -> CommittedDelta;
}

impl FixtureCommit for PreparedApply<'_> {
    fn into_committed(self) -> CommittedDelta {
        self.apply()
    }
}

impl FixtureCommit for PreparedIndependentApply<'_> {
    fn into_committed(self) -> CommittedDelta {
        let (committed, post_commit_fault) = self
            .apply()
            .expect("an exclusively held fixture Independent Apply is current")
            .into_parts();
        assert_eq!(post_commit_fault, None);
        committed
    }
}

impl FixtureCommit for PreparedSharedLocalRemoval<'_> {
    fn into_committed(self) -> CommittedDelta {
        let (committed, post_commit_fault) = self
            .apply()
            .expect("an exclusively held fixture local-removal cut is current")
            .into_parts();
        assert_eq!(post_commit_fault, None);
        committed
    }
}

impl FixtureCommit for CommittedDelta {
    fn into_committed(self) -> CommittedDelta {
        self
    }
}

pub(super) fn apply_plan(commit: impl FixtureCommit) {
    let _ = apply_plan_for_delta(commit);
}

fn apply_plan_for_delta(commit: impl FixtureCommit) -> CommittedDelta {
    commit.into_committed()
}

fn drain_fixture_effects(authority: &mut TxPoolAuthority) {
    loop {
        let Some(lease) = authority.effect_publication_receipt_for_foundation() else {
            break;
        };
        let committed = authority
            .apply_effect_settlement_for_foundation(lease.complete_for_foundation().published())
            .expect("fixture effect publication settles");
        drop(committed);
    }
}

fn drain_fixture_dependency(authority: &mut TxPoolAuthority) {
    drop(
        authority
            .drain_dependency_maintenance_for_foundation()
            .expect("fixture dependency maintenance strictly decreases its rank to zero"),
    );
}

pub(super) fn take_resolve_work(committed: CommittedCheckout) -> (RawTxHash, ResolveWork) {
    let CheckedOutWork::Resolve(work) = committed.into_work() else {
        panic!("resolve-only checkout returns resolve work");
    };
    let hash = TxIdentity::from_transaction(work.transaction()).raw;
    (hash, work)
}

fn continue_fixture_verify(
    resolve: ContinuousResolveWork,
    payload: FoundationResolution,
) -> (ContinuousVerifyWork, usize) {
    let accepted_resident_bytes = accepted_transaction_charge_bytes(
        payload.serialized_bytes(),
        payload.resolved_transaction(),
    );
    let ContinuousResolution::Verify(verify) = resolve
        .into_verify(payload)
        .expect("fixture payload belongs to the checked-out transaction")
    else {
        panic!("fixture payload fits the reserved compute grant");
    };
    (verify, accepted_resident_bytes)
}

fn add_resources(left: ResourceVector, right: ResourceVector) -> ResourceVector {
    left.checked_add(right).expect("fixture fits")
}

fn add_accepted(left: AcceptedResources, right: AcceptedResources) -> AcceptedResources {
    AcceptedResources::new(
        left.entries
            .checked_add(right.entries)
            .expect("fixture fits"),
        left.serialized_bytes
            .checked_add(right.serialized_bytes)
            .expect("fixture fits"),
        left.resident_bytes
            .checked_add(right.resident_bytes)
            .expect("fixture fits"),
        left.cycles.checked_add(right.cycles).expect("fixture fits"),
    )
}

pub(super) fn resolved_payload(tx: &TransactionView) -> FoundationResolution {
    resolved_payload_with_deps(tx, Vec::new())
}

fn resolution_evidence(
    tx: &TransactionView,
    fee: Capacity,
    resident_bytes: usize,
    verify_class: VerifyCycleClass,
) -> ResolutionEvidence {
    ResolutionEvidence::for_foundation(
        Arc::new(ResolvedTransaction::dummy_resolve(tx.clone())),
        fee,
        resident_bytes,
        verify_class,
    )
}

fn resolved_payload_with_deps(
    tx: &TransactionView,
    expanded_dependencies: Vec<OutPoint>,
) -> FoundationResolution {
    resolved_payload_with_facts(tx, expanded_dependencies, Vec::new(), Capacity::shannons(1))
}

pub(super) fn resolved_payload_with_facts(
    tx: &TransactionView,
    expanded_dependencies: Vec<OutPoint>,
    chain_inputs: Vec<OutPoint>,
    fee: Capacity,
) -> FoundationResolution {
    let bytes = tx.data().total_size();
    let mut chain_dependencies = expanded_dependencies.clone();
    chain_dependencies.extend(
        tx.cell_deps()
            .into_iter()
            .map(|dependency| dependency.out_point()),
    );
    ResolvedPayload::for_foundation(
        tx,
        expanded_dependencies,
        64,
        fee,
        bytes,
        chain_inputs,
        chain_dependencies,
    )
    .expect("fixture chain evidence is a subset of resolved cells")
}

pub(super) fn direct_verified_facts(
    transaction: &TransactionView,
    expanded_dependencies: Vec<OutPoint>,
    chain_inputs: Vec<OutPoint>,
    fee: Capacity,
) -> VerifiedFacts {
    direct_verified_facts_for_view(
        transaction,
        ChainViewId::new(ChainRevision(0), Byte32::zero()),
        expanded_dependencies,
        chain_inputs,
        fee,
    )
}

pub(super) fn direct_verified_facts_for_view(
    transaction: &TransactionView,
    chain_view: ChainViewId,
    expanded_dependencies: Vec<OutPoint>,
    chain_inputs: Vec<OutPoint>,
    fee: Capacity,
) -> VerifiedFacts {
    let mut chain_dependencies = expanded_dependencies.clone();
    chain_dependencies.extend(
        transaction
            .cell_deps()
            .into_iter()
            .map(|dependency| dependency.out_point()),
    );
    let payload = Arc::new(
        resolved_payload_with_facts(
            transaction,
            expanded_dependencies,
            chain_inputs.clone(),
            fee,
        )
        .into_payload(),
    );
    let serialized_bytes = payload.serialized_bytes();
    let resident_bytes =
        accepted_transaction_charge_bytes(serialized_bytes, payload.resolved_transaction());
    VerifiedFacts::for_foundation_view_with_cells(
        chain_view,
        DependencyCut(ApplySequence(0)),
        payload,
        chain_inputs,
        chain_dependencies,
        CandidateMetrics {
            fee,
            cost: AcceptedCost::new(serialized_bytes, resident_bytes, 0),
        },
    )
}

pub(super) fn accept_remote_transaction(
    authority: &mut TxPoolAuthority,
    transaction: TransactionView,
    peer: usize,
    status: AcceptedStatus,
    expanded_dependencies: Vec<OutPoint>,
) -> super::super::state::RawTxHash {
    let payload = resolved_payload_with_deps(&transaction, expanded_dependencies);
    accept_remote_transaction_with_payload(authority, transaction, peer, status, payload)
}

pub(super) fn accept_remote_transaction_with_payload(
    authority: &mut TxPoolAuthority,
    transaction: TransactionView,
    peer: usize,
    status: AcceptedStatus,
    payload: FoundationResolution,
) -> super::super::state::RawTxHash {
    let hash = verify_remote_transaction_with_payload(authority, transaction, peer, payload);
    let version = owner_version(authority, &hash);
    apply_plan(
        authority
            .plan_accept_for_foundation(&hash, version, status)
            .expect("fixture membership plans"),
    );
    // Setup helpers model a healthy endpoint publisher. Tests that exercise
    // effect backpressure or inspect a candidate's exact committed outcome
    // build and Apply that candidate explicitly instead of inheriting stale
    // setup effects.
    drain_fixture_effects(authority);
    hash
}

pub(super) fn accept_remote_transaction_with_payload_and_cycles(
    authority: &mut TxPoolAuthority,
    transaction: TransactionView,
    peer: usize,
    status: AcceptedStatus,
    payload: FoundationResolution,
    cycles: Cycle,
) -> super::super::state::RawTxHash {
    let hash = verify_remote_transaction_with_payload_under_and_cycles(
        authority,
        transaction,
        peer,
        payload,
        ScriptVerificationRules::V0,
        cycles,
    );
    let version = owner_version(authority, &hash);
    apply_plan(
        authority
            .plan_accept_for_foundation(&hash, version, status)
            .expect("fixture membership plans"),
    );
    drain_fixture_effects(authority);
    hash
}

fn accept_remote_transaction_with_payload_at(
    authority: &mut TxPoolAuthority,
    transaction: TransactionView,
    peer: usize,
    status: AcceptedStatus,
    payload: FoundationResolution,
    accepted_at: AcceptedAtMillis,
) -> RawTxHash {
    let hash = verify_remote_transaction_with_payload(authority, transaction, peer, payload);
    let version = owner_version(authority, &hash);
    apply_plan(
        authority
            .plan_accept_at_for_foundation(&hash, version, status, accepted_at)
            .expect("timestamped fixture membership plans"),
    );
    drain_fixture_effects(authority);
    hash
}

pub(in crate::authority) fn accepted_parent_child_at(
    authority: &mut TxPoolAuthority,
    nonce: u8,
    parent_at: AcceptedAtMillis,
    child_at: AcceptedAtMillis,
) -> (RawTxHash, RawTxHash) {
    let chain_input = OutPoint::new(Byte32::new([nonce; 32]), 0);
    let parent_tx = TransactionBuilder::default()
        .version(u32::from(nonce))
        .input(CellInput::new(chain_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent_payload = resolved_payload_with_facts(
        &parent_tx,
        Vec::new(),
        vec![chain_input],
        Capacity::shannons(1_000),
    );
    let parent = accept_remote_transaction_with_payload_at(
        authority,
        parent_tx.clone(),
        usize::from(nonce),
        AcceptedStatus::Pending,
        parent_payload,
        parent_at,
    );
    let child_tx = TransactionBuilder::default()
        .version(u32::from(nonce) + 1)
        .input(CellInput::new(OutPoint::new(parent_tx.hash(), 0), 0))
        .build();
    let child_payload =
        resolved_payload_with_facts(&child_tx, Vec::new(), Vec::new(), Capacity::shannons(500));
    let child = accept_remote_transaction_with_payload_at(
        authority,
        child_tx,
        usize::from(nonce) + 1,
        AcceptedStatus::Pending,
        child_payload,
        child_at,
    );
    (parent, child)
}

fn independent_accepted_at(
    authority: &mut TxPoolAuthority,
    marker: u8,
    accepted_at: AcceptedAtMillis,
    status: AcceptedStatus,
) -> (RawTxHash, TransactionView) {
    let chain_input = OutPoint::new(Byte32::new([marker; 32]), 0);
    let transaction = TransactionBuilder::default()
        .version(u32::from(marker))
        .input(CellInput::new(chain_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let payload = resolved_payload_with_facts(
        &transaction,
        Vec::new(),
        vec![chain_input],
        Capacity::shannons(1_000),
    );
    let hash = accept_remote_transaction_with_payload_at(
        authority,
        transaction.clone(),
        usize::from(marker),
        status,
        payload,
        accepted_at,
    );
    (hash, transaction)
}

pub(in crate::authority) fn accepted_parent_with_ready_children(
    authority: &mut TxPoolAuthority,
    nonce: u8,
    child_count: usize,
) -> (RawTxHash, Vec<RawTxHash>) {
    let chain_input = OutPoint::new(Byte32::new([nonce; 32]), 0);
    let mut parent_builder = TransactionBuilder::default()
        .version(u32::from(nonce))
        .input(CellInput::new(chain_input.clone(), 0));
    for _ in 0..child_count {
        parent_builder = parent_builder
            .output(CellOutput::default())
            .output_data(Bytes::new().pack());
    }
    let parent_tx = parent_builder.build();
    let parent_payload = resolved_payload_with_facts(
        &parent_tx,
        Vec::new(),
        vec![chain_input],
        Capacity::shannons(1_000),
    );
    let parent = accept_remote_transaction_with_payload_at(
        authority,
        parent_tx.clone(),
        usize::from(nonce),
        AcceptedStatus::Pending,
        parent_payload,
        AcceptedAtMillis(0),
    );
    let mut children = Vec::new();
    children
        .try_reserve_exact(child_count)
        .expect("the finite Ready sibling fixture is bounded");
    for index in 0..child_count {
        let output_index = u32::try_from(index).expect("the finite output index fits u32");
        let version = u32::from(nonce)
            .checked_add(output_index)
            .and_then(|value| value.checked_add(1))
            .expect("the finite child version is bounded");
        let child_tx = TransactionBuilder::default()
            .version(version)
            .input(CellInput::new(
                OutPoint::new(parent_tx.hash(), output_index),
                0,
            ))
            .build();
        let child_payload =
            resolved_payload_with_facts(&child_tx, Vec::new(), Vec::new(), Capacity::shannons(500));
        children.push(verify_remote_transaction_with_payload(
            authority,
            child_tx,
            usize::from(nonce) + index + 1,
            child_payload,
        ));
    }
    (parent, children)
}

pub(super) fn verify_remote_transaction(
    authority: &mut TxPoolAuthority,
    transaction: TransactionView,
    peer: usize,
    expanded_dependencies: Vec<OutPoint>,
) -> super::super::state::RawTxHash {
    let payload = resolved_payload_with_deps(&transaction, expanded_dependencies);
    verify_remote_transaction_with_payload(authority, transaction, peer, payload)
}

pub(super) fn verify_remote_transaction_with_payload(
    authority: &mut TxPoolAuthority,
    transaction: TransactionView,
    peer: usize,
    payload: FoundationResolution,
) -> super::super::state::RawTxHash {
    verify_remote_transaction_with_payload_under(
        authority,
        transaction,
        peer,
        payload,
        ScriptVerificationRules::V0,
    )
}

pub(super) fn verify_remote_transaction_with_payload_under(
    authority: &mut TxPoolAuthority,
    transaction: TransactionView,
    peer: usize,
    payload: FoundationResolution,
    rules: ScriptVerificationRules,
) -> super::super::state::RawTxHash {
    verify_remote_transaction_with_payload_under_and_cycles(
        authority,
        transaction,
        peer,
        payload,
        rules,
        0,
    )
}

fn verify_remote_transaction_with_payload_under_and_cycles(
    authority: &mut TxPoolAuthority,
    transaction: TransactionView,
    peer: usize,
    payload: FoundationResolution,
    rules: ScriptVerificationRules,
    cycles: Cycle,
) -> super::super::state::RawTxHash {
    let admission = ValidatedAdmission::remote_with_lease(
        transaction,
        RemoteResidencyLease::for_foundation(PeerIndex::from(peer)),
        cycles,
    )
    .expect("fixture admission is valid");
    let hash = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("fixture admission plans"),
    );
    let version = owner_version(authority, &hash);
    let checkout = authority
        .plan_checkout_for_foundation(
            &hash,
            version,
            WorkPermit::ResolveThenVerify(VerifyCapability::Any),
        )
        .expect("fixture checkout plans")
        .apply();
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.into_work() else {
        panic!("continuous permit returns continuous resolve work");
    };
    let (verify, _) = continue_fixture_verify(resolve, payload);
    apply_plan(
        authority
            .apply_settlement(verify.verified_under(cycles, rules))
            .expect("fixture verification settles"),
    );
    hash
}

fn independent_fixture(count: usize) -> (TxPoolAuthority, Vec<RawTxHash>) {
    let fixture_limits = if count > 4 {
        leaf_rbf_cohort_limits()
    } else {
        limits()
    };
    let mut authority = TxPoolAuthority::for_foundation(fixture_limits);
    let shared_chain_dependency = OutPoint::new(Byte32::new([190; 32]), 0);
    let mut hashes = Vec::with_capacity(count);
    for index in 0..count {
        let input = OutPoint::new(Byte32::new([191 + index as u8; 32]), 0);
        let transaction = TransactionBuilder::default()
            .version(200 + index as u32)
            .input(CellInput::new(input.clone(), 0))
            .build();
        let payload = resolved_payload_with_facts(
            &transaction,
            vec![shared_chain_dependency.clone()],
            vec![input],
            Capacity::shannons(1_000 * (index as u64 + 1)),
        );
        hashes.push(verify_remote_transaction_with_payload(
            &mut authority,
            transaction,
            200 + index,
            payload,
        ));
    }
    (authority, hashes)
}

pub(in crate::authority) fn independent_batch(
    authority: &TxPoolAuthority,
    hashes: &[RawTxHash],
) -> SettlementBatch {
    SettlementBatch::new(
        hashes
            .iter()
            .map(|hash| {
                authority
                    .independent_candidate_for_foundation(
                        hash,
                        owner_version(authority, hash),
                        AcceptedStatus::Pending,
                    )
                    .expect("fixture candidate has current final evidence")
            })
            .collect(),
    )
    .expect("fixture batch is non-empty, unique and bounded")
}

fn assert_coupled_and_drop(plan: SettlementPlan<'_>) {
    let SettlementPlan::CoupledComponent(disposition) = plan else {
        panic!("fixture expected a coupled settlement");
    };
    drop(disposition);
}

fn accepted_disposition(disposition: CandidateDispositionPlan<'_>) -> PreparedApply<'_> {
    let CandidateDispositionPlan::Accepted(plan) = disposition else {
        panic!("fixture candidate must be accepted");
    };
    plan
}

fn rejected_coupled_reason_and_drop(plan: SettlementPlan<'_>) -> MembershipReject {
    let SettlementPlan::CoupledComponent(disposition) = plan else {
        panic!("fixture expected a coupled settlement");
    };
    let disposition = disposition.into_disposition();
    let CandidateDispositionPlan::Rejected(rejection) = disposition else {
        panic!("fixture candidate must be rejected");
    };
    rejection.reason().clone()
}

pub(super) fn assert_resource_reference(authority: &TxPoolAuthority) {
    let mut preaccepted = ResourceVector::default();
    let mut remote = ResourceVector::default();
    let mut peers = HashMap::new();
    let mut replacement_history = ResourceVector::default();
    let mut accepted = AcceptedResources::default();
    for (_, owner) in authority.entries_for_reference().snapshot_for_test() {
        let charge = owner.charge_record();
        match charge {
            ChargeRecord::PreAccepted {
                resources,
                residency_peer,
                compute_peer,
            } => {
                preaccepted = add_resources(preaccepted, resources);
                if let Some(peer) = residency_peer {
                    assert!(compute_peer.is_none() || compute_peer == Some(peer));
                    let peer_resources = if compute_peer == Some(peer) {
                        resources
                    } else {
                        resources.without_compute()
                    };
                    remote = add_resources(remote, peer_resources);
                    let usage = peers.entry(peer).or_default();
                    *usage = add_resources(*usage, peer_resources);
                } else {
                    assert!(compute_peer.is_none());
                }
            }
            ChargeRecord::Accepted(resources) => {
                accepted = add_accepted(accepted, resources);
            }
            ChargeRecord::ReplacementHistory(resources) => {
                preaccepted = add_resources(preaccepted, resources);
                replacement_history = add_resources(replacement_history, resources);
            }
        }
    }
    assert_eq!(
        authority.resources().snapshot(),
        ResourceSnapshot {
            preaccepted,
            remote,
            peers,
            replacement_history,
            accepted,
        }
    );
    assert_membership_reference(authority);
}

pub(super) fn assert_membership_reference(authority: &TxPoolAuthority) {
    assert!(authority.membership_projection_consistent());
}

#[test]
fn uak_remote_admission_owns_and_charges_once() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = TransactionBuilder::default()
        .version(1u32)
        .input(CellInput::new(OutPoint::new(Byte32::new([1; 32]), 0), 0))
        .cell_dep(CellDep::new_builder().build())
        .header_dep(Byte32::new([2; 32]))
        .build();
    let expected_bytes = transaction.data().total_size();
    let admission = ValidatedAdmission::remote(transaction, PeerIndex::from(7))
        .expect("fixture admission is valid");
    let hash = admission.identity.raw.clone();
    drop(
        authority
            .plan_admission(admission)
            .expect("bounded first admission plans")
            .apply(),
    );

    assert_eq!(authority.owner_count(), 1);
    assert_eq!(authority.charged_count(), 1);
    assert!(authority.primary_projection_consistent());
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(_))
                && entry.original_charge() == ResourceVector::new(1, expected_bytes, 3, 0)
                && entry.charge == entry.original_charge()
    ));
}

#[test]
fn uak_recovery_admission_requires_the_current_generation_capability() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let stale = ValidatedAdmission::recovery(tx(1_701), PoolGeneration(1))
        .expect("fixture recovery payload is structurally valid");
    let before = authority.normalized_snapshot();
    assert_eq!(
        authority.plan_admission(stale).err(),
        Some(PlanError::Stale(StalePlan::Generation))
    );
    assert_eq!(authority.normalized_snapshot(), before);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_owner_changes_compile_proposal_and_peer_indexes_together() {
    let peer = PeerIndex::from(702);
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let queued = admit_remote(&mut authority, 1_702, 702);
    assert_eq!(
        authority.preaccepted_for_peer_for_reference(peer),
        vec![queued.clone()]
    );

    let accepted = accept_remote_transaction(
        &mut authority,
        tx(1_703),
        702,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    assert_eq!(
        authority.preaccepted_for_peer_for_reference(peer),
        vec![queued.clone()],
        "accepted membership is no longer peer-owned pre-acceptance work"
    );
    assert!(matches!(
        authority.entry(&accepted),
        Some(OwnedTx::Accepted(_))
    ));

    let version = owner_version(&authority, &queued);
    apply_plan(
        authority
            .plan_terminalize_for_foundation(&queued, version)
            .expect("terminalization removes the final peer-indexed owner"),
    );
    assert!(
        authority
            .preaccepted_for_peer_for_reference(peer)
            .is_empty()
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_non_status_accepted_change_advances_both_template_sources() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = accept_remote_transaction(
        &mut authority,
        tx(1_708),
        708,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let before = authority
        .entry(&hash)
        .expect("fixture accepted owner exists");
    let OwnedTx::Accepted(mut changed) = before.clone() else {
        panic!("fixture owner is accepted");
    };
    changed.accepted_at = AcceptedAtMillis(1);
    assert!(
        super::super::source::test_support::replacement_changes_both_template_sources_for_foundation(
            &before,
            &OwnedTx::Accepted(changed),
        ),
        "future accepted metadata changes cannot degrade to status-only publication"
    );
}

#[test]
fn uak_clear_pipeline_preserves_accepted_and_invalidates_active_work() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let active = admit_remote(&mut authority, 1_706, 706);
    let version = owner_version(&authority, &active);
    let (_, work) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(&active, version, WorkPermit::ResolveOnly)
            .expect("active fixture checks out")
            .apply(),
    );
    let accepted = accept_remote_transaction(
        &mut authority,
        tx(1_707),
        707,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    assert!(matches!(
        authority.entry(&accepted),
        Some(OwnedTx::Accepted(_))
    ));

    let before = authority.normalized_snapshot();
    drop(
        authority
            .plan_clear_pipeline()
            .expect("pipeline clear plans with active work"),
    );
    assert!(
        authority
            .normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before, 0, 0, 1),
        "a dropped clear Plan burns exactly its issued Apply stamp and commits no authority fact"
    );

    let old_clocks = authority.clocks();
    let old_chain = authority.chain_view().clone();
    let committed = authority
        .plan_clear_pipeline()
        .expect("clear replans")
        .apply();
    assert_eq!(committed.retired_len(), 1);
    assert_eq!(authority.owner_count(), 1);
    assert_eq!(authority.charged_count(), 1);
    assert!(authority.entry(&active).is_none());
    assert!(matches!(
        authority.entry(&accepted),
        Some(OwnedTx::Accepted(_))
    ));
    assert_eq!(authority.generation(), PoolGeneration(1));
    assert_eq!(authority.chain_view(), &old_chain);
    assert_eq!(authority.clocks().next_version, old_clocks.next_version);
    assert_eq!(authority.clocks().next_arrival, old_clocks.next_arrival);
    assert!(authority.primary_projection_consistent());

    let stale = authority
        .apply_settlement(work.internal_failure())
        .expect_err("the removed active owner makes its completion stale");
    assert_eq!(
        stale.recovery(),
        &ComputeSettlementRecovery::Obsolete(StalePlan::Missing)
    );
    drop(stale);

    let reset = authority
        .effect_publication_receipt_for_foundation()
        .expect("generation swap commits one reset");
    assert_eq!(reset.effects(), &[CommittedEffect::GenerationReset]);
}

#[test]
fn uak_clear_pool_derives_the_next_revision_without_draining_active_work() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let active = admit_remote(&mut authority, 1_709, 709);
    let version = owner_version(&authority, &active);
    let (_, work) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(&active, version, WorkPermit::ResolveOnly)
            .expect("active fixture checks out")
            .apply(),
    );
    let next_tip = Byte32::new([73; 32]);
    let committed = authority
        .plan_clear_pool(next_tip.clone())
        .expect("clear derives the next chain revision")
        .apply();
    assert_eq!(committed.retired_len(), 1);
    assert_eq!(authority.owner_count(), 0);
    assert_eq!(authority.chain_view().revision(), ChainRevision(1));
    assert_eq!(authority.chain_view().tip().0, next_tip);
    assert_eq!(authority.generation(), PoolGeneration(1));
    let stale = authority
        .apply_settlement(work.internal_failure())
        .expect_err("the swapped generation rejects late work as stale");
    assert_eq!(
        stale.recovery(),
        &ComputeSettlementRecovery::Obsolete(StalePlan::Missing)
    );
    drop(stale);
    assert!(authority.primary_projection_consistent());
}

#[tokio::test]
async fn uak_runtime_clear_scopes_and_snapshot_pairing_are_indivisible() {
    let original_snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        original_snapshot.consensus(),
        Arc::clone(&original_snapshot),
    )
    .expect("the authority runtime fixture is valid");
    let (preaccepted, accepted) = runtime.with_authority_for_foundation(|authority| {
        let preaccepted = admit_remote(authority, 1_731, 731);
        let accepted = accept_remote_transaction(
            authority,
            tx(1_732),
            732,
            AcceptedStatus::Pending,
            Vec::new(),
        );
        (preaccepted, accepted)
    });

    runtime
        .clear_pipeline()
        .await
        .expect("runtime pipeline clear commits through one authority guard");
    runtime.with_authority_for_foundation(|authority| {
        assert!(authority.entry(&preaccepted).is_none());
        assert!(matches!(
            authority.entry(&accepted),
            Some(OwnedTx::Accepted(_))
        ));
        assert_eq!(authority.generation(), PoolGeneration(1));
    });
    let (pipeline_view, paired_before) = runtime.paired_chain_for_foundation();
    assert_eq!(pipeline_view.revision(), ChainRevision(0));
    assert!(Arc::ptr_eq(&paired_before, &original_snapshot));

    let replacement_snapshot = genesis_snapshot();
    runtime
        .clear_pool(Arc::clone(&replacement_snapshot))
        .await
        .expect("runtime pool clear commits authority and snapshot together");
    let (pool_view, paired_after) = runtime.paired_chain_for_foundation();
    assert_eq!(pool_view.revision(), ChainRevision(1));
    assert_eq!(pool_view.tip().0, replacement_snapshot.tip_hash());
    assert!(Arc::ptr_eq(&paired_after, &replacement_snapshot));
    runtime.with_authority_for_foundation(|authority| {
        assert_eq!(authority.owner_count(), 0);
        assert_eq!(authority.generation(), PoolGeneration(2));
        assert!(authority.primary_projection_consistent());
    });
}

#[test]
fn uak_runtime_local_removal_has_no_active_work_drain() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the authority runtime fixture is valid");
    let (hash, work) = runtime.with_authority_for_foundation(|authority| {
        let hash = admit_remote(authority, 1_733, 733);
        let version = owner_version(authority, &hash);
        let (_, work) = take_resolve_work(
            authority
                .plan_checkout_for_foundation(&hash, version, WorkPermit::ResolveOnly)
                .expect("the runtime fixture checks out")
                .apply(),
        );
        (hash, work)
    });

    assert!(
        runtime
            .remove_local_transaction(&hash.0)
            .expect("active ownership removal is a total transition")
    );
    assert!(
        !runtime
            .remove_local_transaction(&hash.0)
            .expect("an absent owner is a normal boolean outcome")
    );
    runtime.with_authority_for_foundation(|authority| {
        let stale = authority
            .apply_settlement(work.internal_failure())
            .expect_err("late active work observes missing ownership");
        assert_eq!(
            stale.recovery(),
            &ComputeSettlementRecovery::Obsolete(StalePlan::Missing)
        );
        drop(stale);
        assert!(authority.primary_projection_consistent());
    });
}

#[test]
fn uak_disjoint_accepted_local_removals_overlap_inside_the_real_runtime_cut() {
    const CUT_ENTRY_TIMEOUT: Duration = Duration::from_secs(5);
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the authority runtime fixture is valid");
    let (first, first_seed, second, second_seed, second_support) = runtime
        .with_authority_for_foundation(|authority| {
            let accept_independent = |authority: &mut TxPoolAuthority, seed: u8| {
                let input = OutPoint::new(Byte32::new([seed; 32]), 0);
                let transaction = TransactionBuilder::default()
                    .version(u32::from(seed))
                    .input(CellInput::new(input.clone(), 0))
                    .output(CellOutput::default())
                    .output_data(Bytes::new().pack())
                    .build();
                accept_remote_transaction_with_payload(
                    authority,
                    transaction.clone(),
                    usize::from(seed),
                    AcceptedStatus::Pending,
                    resolved_payload_with_facts(
                        &transaction,
                        Vec::new(),
                        vec![input],
                        Capacity::shannons(1_000),
                    ),
                )
            };
            let first = accept_independent(authority, 200);
            let first_owner = authority.entry(&first).expect("first owner exists");
            let OwnedTx::Accepted(first_entry) = &first_owner else {
                panic!("first owner is Accepted");
            };
            assert_eq!(authority.accepted_parents(&first), Some(HashSet::new()));
            assert_eq!(authority.accepted_children(&first), Some(HashSet::new()));
            for input in first_entry.proof.payload().footprint().inputs() {
                assert_eq!(authority.accepted_spender(input), Some(first.clone()));
            }
            for key in first_owner.dependencies().keys() {
                assert_eq!(
                    authority.dependency_consumers_for_foundation(key),
                    Some(std::collections::BTreeSet::from([first.clone()]))
                );
            }
            let first_support = match authority
                .prepare_shared_local_removal_for_foundation(&first)
                .expect("the first independent Accepted removal plans")
            {
                Some(plan) => plan.physical_write_support_for_foundation(),
                None => panic!("the first independent Accepted owner remains present"),
            };
            let mut candidates = vec![(first, 200u8, first_support)];
            for seed in 201u8..=255 {
                let candidate = accept_independent(authority, seed);
                let support = match authority
                    .prepare_shared_local_removal_for_foundation(&candidate)
                    .expect("an independent Accepted removal plans")
                {
                    Some(plan) => plan.physical_write_support_for_foundation(),
                    None => panic!("every candidate remains present"),
                };
                candidates.push((candidate, seed, support));
            }
            for (candidate, _, support) in &mut candidates {
                *support = match authority
                    .prepare_shared_local_removal_for_foundation(candidate)
                    .expect("the frozen initial-state removal replans")
                {
                    Some(plan) => plan.physical_write_support_for_foundation(),
                    None => panic!("the frozen candidate remains present"),
                };
            }
            (0..candidates.len())
                .find_map(|left| {
                    ((left + 1)..candidates.len()).find_map(|right| {
                        let (left_hash, left_seed, left_support) = &candidates[left];
                        let (right_hash, right_seed, right_support) = &candidates[right];
                        left_support.is_disjoint(*right_support).then(|| {
                            (
                                left_hash.clone(),
                                *left_seed,
                                right_hash.clone(),
                                *right_seed,
                                *right_support,
                            )
                        })
                    })
                })
                .expect("the fixed layout admits a complete disjoint production removal pair")
        });
    let shared_entries = runtime
        .with_authority_for_foundation(|authority| authority.entries_for_reference().clone());

    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_shared_owner_commit_probe(Some(Arc::clone(&probe)));
    });
    std::thread::scope(|scope| {
        let first_remove = scope.spawn(|| runtime.remove_local_transaction(&first.0));
        let first_entered = entered.recv_timeout(CUT_ENTRY_TIMEOUT);
        assert!(first_entered.is_ok());
        assert!(
            shared_entries.try_write_cut(second_support).is_some(),
            "the first live cut unexpectedly overlaps the frozen second support"
        );
        let (second_started_tx, second_started_rx) = std::sync::mpsc::channel();
        let runtime_ref = &runtime;
        let second_hash = second.0.clone();
        let second_remove = scope.spawn(move || {
            let _ = second_started_tx.send(());
            runtime_ref.remove_local_transaction(&second_hash)
        });
        assert!(second_started_rx.recv_timeout(CUT_ENTRY_TIMEOUT).is_ok());
        let second_entered = entered.recv_timeout(CUT_ENTRY_TIMEOUT);
        assert!(
            shared_entries.try_read_all().is_none(),
            "a complete read cut must not splice two in-flight write cuts"
        );
        let _ = release.send(());
        let _ = release.send(());
        assert!(
            first_remove
                .join()
                .expect("first removal thread joins")
                .unwrap()
        );
        assert!(second_entered.is_ok());
        assert!(
            second_remove
                .join()
                .expect("second removal thread joins")
                .unwrap()
        );
    });
    let concurrent = runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_shared_owner_commit_probe(None);
        assert!(authority.primary_projection_consistent());
        authority.normalized_snapshot()
    });
    let template = runtime
        .template_input()
        .expect("the post-commit template input captures one complete cut");
    assert_eq!(
        template.pool_source_cut(),
        super::super::template::TemplatePoolSourceCut::new(runtime.template_source_versions())
    );
    assert_eq!(
        template
            .selection()
            .pending_rank(&first)
            .expect("the removed first owner is absent from a coherent template cut"),
        None
    );
    assert_eq!(
        template
            .selection()
            .pending_rank(&second)
            .expect("the removed second owner is absent from a coherent template cut"),
        None
    );
    let persistence = runtime
        .persistence_receipt()
        .expect("the post-commit persistence receipt captures one complete cut")
        .into_parent_first()
        .expect("the captured persistence graph is acyclic");
    assert!(persistence.accepted().iter().all(|transaction| {
        let hash = transaction.hash();
        hash != first.0 && hash != second.0
    }));

    let baseline_snapshot = genesis_snapshot();
    let baseline = AuthorityRuntime::new(
        &runtime_config(),
        baseline_snapshot.consensus(),
        Arc::clone(&baseline_snapshot),
    )
    .expect("the sequential baseline runtime is valid");
    let (baseline_first, baseline_second) = baseline.with_authority_for_foundation(|authority| {
        let mut accepted = Vec::new();
        for seed in 200u8..=255 {
            let input = OutPoint::new(Byte32::new([seed; 32]), 0);
            let transaction = TransactionBuilder::default()
                .version(u32::from(seed))
                .input(CellInput::new(input.clone(), 0))
                .output(CellOutput::default())
                .output_data(Bytes::new().pack())
                .build();
            let hash = accept_remote_transaction_with_payload(
                authority,
                transaction.clone(),
                usize::from(seed),
                AcceptedStatus::Pending,
                resolved_payload_with_facts(
                    &transaction,
                    Vec::new(),
                    vec![input],
                    Capacity::shannons(1_000),
                ),
            );
            let plan = authority
                .prepare_shared_local_removal_for_foundation(&hash)
                .expect("the baseline probe plans");
            assert!(plan.is_some());
            accepted.push((seed, hash));
        }
        for (_, hash) in &accepted {
            let plan = authority
                .prepare_shared_local_removal_for_foundation(hash)
                .expect("the baseline frozen-state probe plans");
            assert!(plan.is_some());
        }
        (
            accepted
                .iter()
                .find_map(|(seed, hash)| (*seed == first_seed).then(|| hash.clone()))
                .expect("first baseline owner"),
            accepted
                .iter()
                .find_map(|(seed, hash)| (*seed == second_seed).then(|| hash.clone()))
                .expect("second baseline owner"),
        )
    });
    assert!(
        baseline
            .remove_local_transaction(&baseline_first.0)
            .expect("first sequential removal commits")
    );
    assert!(
        baseline
            .remove_local_transaction(&baseline_second.0)
            .expect("second sequential removal commits")
    );
    let sequential = baseline.with_authority_for_foundation(|authority| {
        assert!(authority.primary_projection_consistent());
        authority.normalized_snapshot()
    });
    assert_eq!(
        concurrent.first_difference(&sequential),
        None,
        "entry difference: {:?}",
        concurrent.first_entry_difference(&sequential)
    );
}

#[test]
fn uak_same_shard_distinct_local_removals_preserve_aggregate_projection() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the authority runtime fixture is valid");
    let (first, second) = runtime.with_authority_for_foundation(|authority| {
        let mut first_by_shard = HashMap::new();
        let mut pair = None;
        for seed in 100u8..=180 {
            let input = OutPoint::new(Byte32::new([seed; 32]), 0);
            let transaction = TransactionBuilder::default()
                .version(u32::from(seed))
                .input(CellInput::new(input.clone(), 0))
                .output(CellOutput::default())
                .output_data(Bytes::new().pack())
                .build();
            let hash = accept_remote_transaction_with_payload(
                authority,
                transaction.clone(),
                usize::from(seed),
                AcceptedStatus::Proposed,
                resolved_payload_with_facts(
                    &transaction,
                    Vec::new(),
                    vec![input],
                    Capacity::shannons(1_000),
                ),
            );
            let shard = authority.entries_for_reference().owner_shard(&hash);
            if let Some(first) = first_by_shard.insert(shard, hash.clone()) {
                pair = Some((first, hash));
                break;
            }
        }
        let pair = pair.expect("more than 64 independent owners contain a same-shard pair");
        for hash in [&pair.0, &pair.1] {
            assert!(
                authority
                    .prepare_shared_local_removal_for_foundation(hash)
                    .expect("the independent Proposed owner removal plans")
                    .is_some()
            );
        }
        pair
    });
    let plan_barrier = Arc::new(std::sync::Barrier::new(2));
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_concurrent_removal_plan_probe(Some(Arc::clone(&plan_barrier)));
    });

    let outcomes = std::thread::scope(|scope| {
        let left = scope.spawn(|| runtime.remove_local_transaction(&first.0));
        let right = scope.spawn(|| runtime.remove_local_transaction(&second.0));
        [
            left.join().expect("left removal thread joins").unwrap(),
            right.join().expect("right removal thread joins").unwrap(),
        ]
    });
    assert_eq!(outcomes, [true, true]);
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_concurrent_removal_plan_probe(None);
        assert!(authority.entry(&first).is_none());
        assert!(authority.entry(&second).is_none());
        assert!(
            authority.primary_projection_consistent(),
            "serialized same-shard concurrent Apply must not install stale absolute aggregates"
        );
    });
}

#[test]
fn uak_same_root_local_removal_returns_exact_competing_progress_then_absence() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the authority runtime fixture is valid");
    let hash = runtime.with_authority_for_foundation(|authority| {
        let input = OutPoint::new(Byte32::new([199; 32]), 0);
        let transaction = TransactionBuilder::default()
            .version(199u32)
            .input(CellInput::new(input.clone(), 0))
            .output(CellOutput::default())
            .output_data(Bytes::new().pack())
            .build();
        let hash = accept_remote_transaction_with_payload(
            authority,
            transaction.clone(),
            199,
            AcceptedStatus::Pending,
            resolved_payload_with_facts(
                &transaction,
                Vec::new(),
                vec![input],
                Capacity::shannons(1_000),
            ),
        );
        assert!(
            authority
                .prepare_shared_local_removal_for_foundation(&hash)
                .expect("the independent Accepted removal plans")
                .is_some()
        );
        hash
    });
    let plan_barrier = Arc::new(std::sync::Barrier::new(2));
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_concurrent_removal_plan_probe(Some(Arc::clone(&plan_barrier)));
    });

    let outcomes = std::thread::scope(|scope| {
        let left = scope.spawn(|| runtime.remove_local_transaction(&hash.0));
        let right = scope.spawn(|| runtime.remove_local_transaction(&hash.0));
        [
            left.join().expect("left removal thread joins"),
            right.join().expect("right removal thread joins"),
        ]
    });
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, Ok(true)))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(
                outcome,
                Err(AuthorityAdministrationError::CompetingProgress)
            ))
            .count(),
        1
    );
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_concurrent_removal_plan_probe(None);
        assert!(authority.entry(&hash).is_none());
        assert!(authority.primary_projection_consistent());
    });
    assert_eq!(
        runtime.remove_local_transaction(&hash.0),
        Ok(false),
        "a new public operation linearizes the already-committed absence"
    );
}

#[test]
fn uak_runtime_expiry_owns_wall_clock_policy_and_bounded_progress() {
    let snapshot = genesis_snapshot();
    let mut config = runtime_config();
    config.expiry_hours = 0;
    let runtime = AuthorityRuntime::new(&config, snapshot.consensus(), Arc::clone(&snapshot))
        .expect("the authority runtime fixture is valid");
    let (remote, parent, child) = runtime.with_authority_for_foundation(|authority| {
        let remote = admit_remote_until(authority, 1_734, 734, 0);
        let (parent, child) =
            accepted_parent_child_at(authority, 91, AcceptedAtMillis(0), AcceptedAtMillis(1));
        (remote, parent, child)
    });

    assert_eq!(
        runtime
            .expire_remote_due()
            .expect("runtime derives the remote cutoff and bounded slice"),
        AuthorityMaintenanceOutcome::Applied
    );
    assert_eq!(
        runtime
            .expire_accepted_due()
            .expect("runtime derives the accepted cutoff from expiry_hours"),
        AuthorityMaintenanceOutcome::Applied
    );
    assert_eq!(
        runtime
            .expire_remote_due()
            .expect("an empty remote prefix is normal"),
        AuthorityMaintenanceOutcome::Idle
    );
    assert_eq!(
        runtime
            .expire_accepted_due()
            .expect("an empty accepted prefix is normal"),
        AuthorityMaintenanceOutcome::Idle
    );
    runtime.with_authority_for_foundation(|authority| {
        assert!(authority.entry(&remote).is_none());
        assert!(authority.entry(&parent).is_none());
        assert!(authority.entry(&child).is_none());
        assert_eq!(authority.owner_count(), 0);
        assert!(authority.primary_projection_consistent());
    });
}

#[test]
fn uak_local_accepted_removal_is_one_total_descendant_transition() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let (parent, child) = accepted_parent_child_at(
        &mut authority,
        80,
        AcceptedAtMillis(10),
        AcceptedAtMillis(20),
    );

    let mut alternate_bytes = parent.0.as_slice().to_vec();
    alternate_bytes[31] ^= 1;
    let alternate = RawTxHash(
        Byte32::from_slice(&alternate_bytes).expect("the alternate raw hash is fixed-size"),
    );
    assert_eq!(
        ckb_types::packed::ProposalShortId::from_tx_hash(&parent.0),
        ckb_types::packed::ProposalShortId::from_tx_hash(&alternate.0)
    );
    let before = authority.normalized_snapshot();
    assert!(
        authority
            .prepare_shared_local_removal_for_foundation(&alternate)
            .expect("a colliding raw-hash miss is a normal lookup")
            .is_none()
    );
    assert_eq!(authority.normalized_snapshot(), before);
    assert!(authority.entry(&parent).is_some());
    assert!(authority.entry(&child).is_some());

    let committed = authority
        .prepare_shared_local_removal_for_foundation(&parent)
        .expect("the complete descendant closure plans")
        .expect("the root is present")
        .apply_for_foundation();
    assert_eq!(committed.retired_len(), 2);
    assert!(authority.entry(&parent).is_none());
    assert!(authority.entry(&child).is_none());
    assert_eq!(authority.owner_count(), 0);
    assert_eq!(authority.charged_count(), 0);
    assert!(
        authority
            .effect_publication_receipt_for_foundation()
            .is_none(),
        "trusted local removal must not invent an Accepted rejection"
    );
    assert!(
        authority
            .prepare_shared_local_removal_for_foundation(&parent)
            .expect("an absent lookup is not a structural error")
            .is_none()
    );
    assert_resource_reference(&authority);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_disjoint_local_accepted_removals_commute_without_effect_observations() {
    fn fixture() -> (TxPoolAuthority, RawTxHash, RawTxHash) {
        let mut authority = TxPoolAuthority::for_foundation(limits());
        let left = accept_remote_transaction(
            &mut authority,
            tx(1_724),
            1_724,
            AcceptedStatus::Pending,
            Vec::new(),
        );
        let right = accept_remote_transaction(
            &mut authority,
            tx(1_725),
            1_725,
            AcceptedStatus::Pending,
            Vec::new(),
        );
        (authority, left, right)
    }

    fn remove(authority: &mut TxPoolAuthority, hash: &RawTxHash) {
        let plan = authority
            .prepare_shared_local_removal_for_foundation(hash)
            .expect("a disjoint accepted owner has one total removal plan")
            .expect("the accepted owner is present");
        assert_eq!(
            plan.administrative_removal_keys_for_claim(),
            Some(vec![hash.clone()]),
            "the local-removal Plan must expose the real administrative closure owner support"
        );
        let committed = plan.apply_for_foundation();
        assert_eq!(committed.retired_len(), 1);
        assert!(
            authority
                .effect_publication_receipt_for_foundation()
                .is_none(),
            "trusted local removal has no externally committed effect observation"
        );
    }

    let (mut left_then_right, left, right) = fixture();
    remove(&mut left_then_right, &left);
    remove(&mut left_then_right, &right);

    let (mut right_then_left, same_left, same_right) = fixture();
    assert_eq!((&same_left, &same_right), (&left, &right));
    remove(&mut right_then_left, &same_right);
    remove(&mut right_then_left, &same_left);

    assert_eq!(
        left_then_right.normalized_snapshot(),
        right_then_left.normalized_snapshot(),
        "adjacent disjoint removals have the same complete authority observation in either order"
    );
    assert!(left_then_right.primary_projection_consistent());
    assert!(right_then_left.primary_projection_consistent());
    assert_resource_reference(&left_then_right);
    assert_resource_reference(&right_then_left);
}

#[test]
fn qhc_admin_owner_keys_alone_do_not_determine_legal_continuations() {
    fn fixture() -> (TxPoolAuthority, RawTxHash, RawTxHash) {
        let effect_bytes = 64 * 1024;
        let effect_limits = EffectLimits::partitioned(
            EffectCapacity::new(1, effect_bytes),
            EffectCapacity::new(1, effect_bytes),
            EffectCapacity::new(1, effect_bytes),
            EffectBatchBounds::new(
                EffectBatchBound::new(1, effect_bytes),
                EffectBatchBound::new(1, effect_bytes),
                EffectBatchBound::new(1, effect_bytes),
            ),
        )
        .expect("one batch in each effect region is a valid bounded configuration");
        let mut authority =
            TxPoolAuthority::for_foundation_with_effect_limits(limits(), effect_limits)
                .expect("the bounded authority fixture is valid");
        let first = admit_remote_until(&mut authority, 1_728, 728, 10);
        let second = admit_remote_until(&mut authority, 1_729, 729, 20);
        (authority, first, second)
    }

    let (local, local_hash, local_continuation_hash) = fixture();
    let (expired, expired_hash, expired_continuation_hash) = fixture();
    assert_eq!(
        (&expired_hash, &expired_continuation_hash),
        (&local_hash, &local_continuation_hash)
    );
    assert_eq!(
        local.normalized_snapshot(),
        expired.normalized_snapshot(),
        "both histories start from the same complete production authority observation"
    );

    let local_plan = local
        .prepare_shared_local_removal_for_foundation(&local_hash)
        .expect("the explicit removal plans")
        .expect("the remote owner is present");
    let local_keys = local_plan
        .administrative_removal_keys_for_claim()
        .expect("the real administrative delta carries its owner keys");
    let local_committed = local_plan.apply_for_foundation();
    assert_eq!(local_committed.retired_len(), 1);

    let expiry_plan = expired
        .plan_remote_expiry(
            RemoteDeadline(10),
            NonZeroUsize::new(1).expect("the fixture limit is non-zero"),
        )
        .expect("the expiry lookup is valid")
        .expect("the remote owner is due");
    let expiry_keys = expiry_plan
        .administrative_removal_keys_for_claim()
        .expect("the real administrative delta carries its owner keys");
    let expiry_committed = expiry_plan.apply_for_foundation(&expired);
    assert_eq!(expiry_committed.retired_len(), 1);

    assert_eq!(
        local_keys, expiry_keys,
        "both production deltas have the same exact owner-key support"
    );
    let local_usage = local.operational_metrics().effects;
    let expiry_usage = expired.operational_metrics().effects;
    assert_eq!(
        (local_usage.remote_batches, local_usage.ordinary_batches),
        (0, 1),
        "explicit removal consumes trusted effect capacity"
    );
    assert_eq!(
        (expiry_usage.remote_batches, expiry_usage.ordinary_batches),
        (1, 1),
        "remote expiry consumes both its attacker-bounded subregion and cumulative ordinary capacity"
    );

    let local_continuation = local
        .plan_remote_expiry(
            RemoteDeadline(20),
            NonZeroUsize::new(1).expect("the continuation limit is non-zero"),
        )
        .expect("the same expiry continuation has remote capacity after local removal")
        .expect("the second owner is due");
    assert_eq!(
        local_continuation
            .apply_for_foundation(&local)
            .retired_len(),
        1
    );
    let expired_before_continuation = expired.normalized_snapshot();
    assert_eq!(
        expired
            .plan_remote_expiry(
                RemoteDeadline(20),
                NonZeroUsize::new(1).expect("the continuation limit is non-zero"),
            )
            .err(),
        Some(PlanError::Backpressure(Backpressure::EffectCapacity)),
        "the same continuation observes the occupied remote effect region"
    );
    assert_eq!(
        expired.normalized_snapshot(),
        expired_before_continuation,
        "backpressure preserves the complete pre-continuation authority cut"
    );
    assert!(local.entry(&local_hash).is_none());
    assert!(expired.entry(&local_hash).is_none());
    assert!(local.entry(&local_continuation_hash).is_none());
    assert!(expired.entry(&expired_continuation_hash).is_some());
    assert_resource_reference(&local);
    assert_resource_reference(&expired);
    assert!(local.primary_projection_consistent());
    assert!(expired.primary_projection_consistent());
}

#[test]
fn uak_local_preaccepted_removal_invalidates_work_and_releases_relay_state() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 1_725, 725);
    let version = owner_version(&authority, &hash);
    let (_, work) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(&hash, version, WorkPermit::ResolveOnly)
            .expect("the exact owner checks out")
            .apply(),
    );

    let committed = authority
        .prepare_shared_local_removal_for_foundation(&hash)
        .expect("active removal plans without a drain")
        .expect("the owner is present")
        .apply_for_foundation();
    assert_eq!(committed.retired_len(), 1);
    assert!(authority.entry(&hash).is_none());
    let stale = authority
        .apply_settlement(work.internal_failure())
        .expect_err("the removed owner makes late work stale");
    assert_eq!(
        stale.recovery(),
        &ComputeSettlementRecovery::Obsolete(StalePlan::Missing)
    );
    drop(stale);

    let effect = authority
        .effect_publication_receipt_for_foundation()
        .expect("removal commits relay cleanup");
    assert!(matches!(
        effect.effects(),
        [CommittedEffect::RemoteIngressReleased(release)] if release.tx_hash() == &hash
    ));
    assert_resource_reference(&authority);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_local_non_remote_preaccepted_removal_does_not_release_relay_state() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let proposal =
        ValidatedAdmission::proposal(tx(1_726)).expect("fixture proposal admission is valid");
    let proposal_hash = proposal.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(proposal)
            .expect("trusted proposal admission plans"),
    );
    let recovery = ValidatedAdmission::recovery(tx(1_727), PoolGeneration(0))
        .expect("fixture recovery admission is valid");
    let recovery_hash = recovery.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(recovery)
            .expect("recovery admission plans"),
    );

    for hash in [&proposal_hash, &recovery_hash] {
        drop(
            authority
                .prepare_shared_local_removal_for_foundation(hash)
                .expect("local removal plans")
                .expect("the owner exists")
                .apply_for_foundation(),
        );
    }

    assert!(
        authority
            .effect_publication_receipt_for_foundation()
            .is_none(),
        "owners without remote ingress attribution must not mutate relay projections"
    );
    assert_resource_reference(&authority);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_local_replacement_history_removal_uses_the_shared_owner_batch_without_effect() {
    let mut authority = TxPoolAuthority::with_replacement(limits(), FeeRate::from_u64(1_000));
    let input = OutPoint::new(Byte32::new([0x73; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(0x73u32)
        .input(CellInput::new(input.clone(), 0))
        .build();
    let victim = accept_remote_transaction_with_payload(
        &mut authority,
        victim_tx.clone(),
        0x73,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &victim_tx,
            Vec::new(),
            vec![input.clone()],
            Capacity::shannons(1_000),
        ),
    );
    let replacement_tx = TransactionBuilder::default()
        .version(0x74u32)
        .input(CellInput::new(input.clone(), 0))
        .build();
    let replacement = verify_remote_transaction_with_payload(
        &mut authority,
        replacement_tx.clone(),
        0x74,
        resolved_payload_with_facts(
            &replacement_tx,
            Vec::new(),
            vec![input],
            Capacity::shannons(10_000),
        ),
    );
    let replacement_version = owner_version(&authority, &replacement);
    apply_plan(
        authority
            .plan_accept_for_foundation(&replacement, replacement_version, AcceptedStatus::Pending)
            .expect("the funded replacement commits one history owner"),
    );
    assert!(matches!(
        authority.entry(&victim),
        Some(OwnedTx::ReplacementHistory(_))
    ));
    drain_fixture_effects(&mut authority);

    let committed = authority
        .prepare_shared_local_removal_for_foundation(&victim)
        .expect("the history owner has one shared removal plan")
        .expect("the history owner remains present")
        .apply_for_foundation();
    assert_eq!(committed.retired_len(), 1);
    assert!(authority.entry(&victim).is_none());
    assert!(matches!(
        authority.entry(&replacement),
        Some(OwnedTx::Accepted(_))
    ));
    assert_eq!(authority.resources().replacement_history().entries, 0);
    assert!(
        authority
            .effect_publication_receipt_for_foundation()
            .is_none(),
        "history removal has no relay or rejection audience"
    );
    assert_resource_reference(&authority);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_local_removal_rolls_back_a_late_accepted_child_without_effect() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let (parent, parent_tx) = independent_accepted_at(
        &mut authority,
        0x75,
        AcceptedAtMillis(10),
        AcceptedStatus::Pending,
    );
    let compiled = authority
        .compile_shared_local_removal(&parent)
        .expect("the leaf local removal compiles")
        .expect("the parent remains present");
    let child_tx = TransactionBuilder::default()
        .version(0x76u32)
        .input(CellInput::new(OutPoint::new(parent_tx.hash(), 0), 0))
        .build();
    let child = accept_remote_transaction_with_payload_at(
        &mut authority,
        child_tx.clone(),
        0x76,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(&child_tx, Vec::new(), Vec::new(), Capacity::shannons(500)),
        AcceptedAtMillis(20),
    );
    let before = authority.normalized_snapshot();
    assert!(matches!(
        compiled.bind(&authority),
        Err(PlanError::Stale(StalePlan::Version))
    ));
    assert_eq!(authority.normalized_snapshot(), before);

    let fresh = authority
        .prepare_shared_local_removal_for_foundation(&parent)
        .expect("the enlarged local closure replans")
        .expect("the parent remains present");
    assert_eq!(fresh.apply_for_foundation().retired_len(), 2);
    assert!(authority.entry(&parent).is_none());
    assert!(authority.entry(&child).is_none());
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_local_removal_respects_effect_close_and_chain_reorg_before_bind() {
    let fixture = |marker: u8| {
        let mut authority = TxPoolAuthority::for_foundation(limits());
        let (hash, _) = independent_accepted_at(
            &mut authority,
            marker,
            AcceptedAtMillis(10),
            AcceptedStatus::Pending,
        );
        let compiled = authority
            .compile_shared_local_removal(&hash)
            .expect("the lifecycle local removal compiles")
            .expect("the fixture owner remains present");
        (authority, hash, compiled)
    };

    let (mut closed, closed_hash, closed_plan) = fixture(0x77);
    apply_plan(
        closed
            .plan_effect_close_for_foundation()
            .expect("effect production closes before local Bind"),
    );
    let closed_before = closed.normalized_snapshot();
    assert!(matches!(
        closed_plan.bind(&closed),
        Err(PlanError::EffectClosed)
    ));
    assert_eq!(closed.normalized_snapshot(), closed_before);
    assert!(matches!(
        closed.entry(&closed_hash),
        Some(OwnedTx::Accepted(_))
    ));

    let (mut reorged, reorged_hash, reorged_plan) = fixture(0x78);
    reorged.force_chain_view(ChainViewId::new(ChainRevision(1), Byte32::new([0x78; 32])));
    let reorged_before = reorged.normalized_snapshot();
    assert!(matches!(
        reorged_plan.bind(&reorged),
        Err(PlanError::Stale(StalePlan::Version))
    ));
    assert_eq!(reorged.normalized_snapshot(), reorged_before);
    assert!(matches!(
        reorged.entry(&reorged_hash),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(closed.primary_projection_consistent());
    assert!(reorged.primary_projection_consistent());
}

#[test]
fn uak_accepted_expiry_uses_stable_deadlines_and_expires_the_full_closure() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let (parent, child) = accepted_parent_child_at(
        &mut authority,
        82,
        AcceptedAtMillis(10),
        AcceptedAtMillis(20),
    );
    assert!(
        authority
            .compile_shared_accepted_expiry(AcceptedAtMillis(9))
            .expect("pre-deadline lookup is valid")
            .is_none()
    );

    let version = owner_version(&authority, &parent);
    apply_plan(
        authority
            .plan_status_for_foundation(&parent, version, AcceptedStatus::Gap)
            .expect("status-only version churn plans"),
    );
    drain_fixture_effects(&mut authority);

    let committed = authority
        .compile_shared_accepted_expiry(AcceptedAtMillis(10))
        .expect("the stable accepted deadline remains indexed")
        .expect("the oldest root is due")
        .apply_for_foundation(&authority);
    assert_eq!(committed.retired_len(), 2);
    assert!(authority.entry(&parent).is_none());
    assert!(authority.entry(&child).is_none());

    let effect = authority
        .effect_publication_receipt_for_foundation()
        .expect("the atomic removal publishes every exact outcome");
    let mut expired = effect
        .effects()
        .iter()
        .map(|effect| match effect {
            CommittedEffect::Rejected(CommittedRejection::Expired { entry }) => {
                (RawTxHash(entry.tx.hash()), entry.timestamp)
            }
            other => panic!("unexpected expiry effect: {other:?}"),
        })
        .collect::<Vec<_>>();
    expired.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    let mut expected = vec![(parent, 10), (child, 20)];
    expected.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(expired, expected);
    assert_resource_reference(&authority);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_accepted_expiry_linearizes_before_a_disjoint_earlier_head() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let (selected, _) = independent_accepted_at(
        &mut authority,
        83,
        AcceptedAtMillis(10),
        AcceptedStatus::Pending,
    );
    let compiled = authority
        .compile_shared_accepted_expiry(AcceptedAtMillis(10))
        .expect("the initial oldest head is coherent")
        .expect("the initial Accepted owner is due");
    let (earlier, _) = independent_accepted_at(
        &mut authority,
        84,
        AcceptedAtMillis(5),
        AcceptedStatus::Pending,
    );

    let committed = compiled.apply_for_foundation(&authority);
    assert_eq!(committed.retired_len(), 1);
    assert!(authority.entry(&selected).is_none());
    assert!(matches!(
        authority.entry(&earlier),
        Some(OwnedTx::Accepted(_))
    ));
    drain_fixture_effects(&mut authority);
    let next = authority
        .compile_shared_accepted_expiry(AcceptedAtMillis(10))
        .expect("the interposed head remains a coherent next step")
        .expect("the earlier head is due next");
    assert_eq!(
        next.administrative_removal_keys_for_claim(),
        Some(vec![earlier])
    );
    assert_resource_reference(&authority);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_accepted_expiry_mid_compile_owner_loss_is_stale_not_fault() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let (hash, _) = independent_accepted_at(
        &mut authority,
        91,
        AcceptedAtMillis(10),
        AcceptedStatus::Pending,
    );
    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    authority
        .entries_for_reference()
        .set_accepted_expiry_mid_compile_pause(Some(probe));
    let outcome = std::thread::scope(|scope| {
        let compiling = &authority;
        let compile =
            scope.spawn(move || compiling.compile_shared_accepted_expiry(AcceptedAtMillis(10)));
        entered
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("the expiry compile parks after capturing its owner cohort");
        let removal = authority
            .prepare_shared_local_removal_for_foundation(&hash)
            .expect("the shared local removal compiles")
            .expect("the owner remains present until the competing Apply");
        assert_eq!(removal.apply_for_foundation().retired_len(), 1);
        assert!(authority.entry(&hash).is_none());
        release.send(()).expect("release the mid-compile pause");
        compile.join().expect("the expiry compile does not panic")
    });
    authority
        .entries_for_reference()
        .set_accepted_expiry_mid_compile_pause(None);
    assert!(
        matches!(outcome, Err(PlanError::Stale(_))),
        "committed cohort divergence is OCC progress, not a projection fault"
    );
    assert!(
        authority
            .compile_shared_accepted_expiry(AcceptedAtMillis(10))
            .expect("the retried compile is coherent")
            .is_none()
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_accepted_expiry_mid_compile_stable_contradiction_remains_fault() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let (parent, _child) = accepted_parent_child_at(
        &mut authority,
        92,
        AcceptedAtMillis(10),
        AcceptedAtMillis(20),
    );
    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    authority
        .entries_for_reference()
        .set_accepted_expiry_mid_compile_pause(Some(probe));
    let bogus = RawTxHash(Byte32::new([255u8; 32]));
    let outcome = std::thread::scope(|scope| {
        let compiling = &authority;
        let compile =
            scope.spawn(move || compiling.compile_shared_accepted_expiry(AcceptedAtMillis(10)));
        entered
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("the expiry compile parks after capturing its owner cohort");
        authority
            .entries_for_reference()
            .replace_membership_parents_for_foundation(&parent, HashSet::from([bogus]));
        release.send(()).expect("release the mid-compile pause");
        compile.join().expect("the expiry compile does not panic")
    });
    authority
        .entries_for_reference()
        .set_accepted_expiry_mid_compile_pause(None);
    assert!(matches!(
        outcome,
        Err(PlanError::Fault(AuthorityFault::MembershipProjection))
    ));
    authority
        .entries_for_reference()
        .replace_membership_parents_for_foundation(&parent, HashSet::new());
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_accepted_expiry_and_late_child_choose_one_complete_order() {
    let mut child_first = TxPoolAuthority::for_foundation(limits());
    let (parent, parent_tx) = independent_accepted_at(
        &mut child_first,
        85,
        AcceptedAtMillis(10),
        AcceptedStatus::Pending,
    );
    let stale = child_first
        .compile_shared_accepted_expiry(AcceptedAtMillis(10))
        .expect("the leaf expiry compiles")
        .expect("the parent is due");
    let child_tx = TransactionBuilder::default()
        .version(186u32)
        .input(CellInput::new(OutPoint::new(parent_tx.hash(), 0), 0))
        .build();
    let child = accept_remote_transaction_with_payload_at(
        &mut child_first,
        child_tx.clone(),
        186,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(&child_tx, Vec::new(), Vec::new(), Capacity::shannons(500)),
        AcceptedAtMillis(20),
    );
    let before = child_first.normalized_snapshot();
    let before_effect = child_first.effect_observation_for_foundation();
    assert!(matches!(
        stale.bind(&child_first),
        Err(PlanError::Stale(StalePlan::Version))
    ));
    assert_eq!(child_first.normalized_snapshot(), before);
    assert_eq!(
        child_first.effect_observation_for_foundation(),
        before_effect
    );
    let fresh = child_first
        .compile_shared_accepted_expiry(AcceptedAtMillis(10))
        .expect("the enlarged closure replans")
        .expect("the parent remains due");
    assert_eq!(fresh.apply_for_foundation(&child_first).retired_len(), 2);
    assert!(child_first.entry(&parent).is_none());
    assert!(child_first.entry(&child).is_none());
    assert!(child_first.primary_projection_consistent());

    let mut expiry_first = TxPoolAuthority::for_foundation(limits());
    let (parent, parent_tx) = independent_accepted_at(
        &mut expiry_first,
        87,
        AcceptedAtMillis(10),
        AcceptedStatus::Pending,
    );
    let child_tx = TransactionBuilder::default()
        .version(188u32)
        .input(CellInput::new(OutPoint::new(parent_tx.hash(), 0), 0))
        .build();
    let child = verify_remote_transaction_with_payload(
        &mut expiry_first,
        child_tx.clone(),
        188,
        resolved_payload_with_facts(&child_tx, Vec::new(), Vec::new(), Capacity::shannons(500)),
    );
    let receipt = expiry_first
        .final_admission_receipt_at_for_foundation(
            &child,
            owner_version(&expiry_first, &child),
            AcceptedStatus::Pending,
            AcceptedAtMillis(20),
        )
        .expect("the late child captures its pre-expiry validation receipt");
    let expiry = expiry_first
        .compile_shared_accepted_expiry(AcceptedAtMillis(10))
        .expect("the parent expiry compiles with one transient consumer")
        .expect("the parent is due");
    assert_eq!(expiry.apply_for_foundation(&expiry_first).retired_len(), 1);
    assert!(expiry_first.entry(&parent).is_none());
    assert!(
        expiry_first
            .plan_accept_for_foundation_receipt(receipt)
            .is_err(),
        "a child receipt captured before parent expiry cannot become Accepted afterward"
    );
    assert!(!matches!(
        expiry_first.entry(&child),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(expiry_first.primary_projection_consistent());
}

#[test]
fn uak_shared_accepted_expiry_rejects_same_hash_readmission_aba() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let (hash, transaction) = independent_accepted_at(
        &mut authority,
        89,
        AcceptedAtMillis(10),
        AcceptedStatus::Pending,
    );
    let old_version = owner_version(&authority, &hash);
    let compiled = authority
        .compile_shared_accepted_expiry(AcceptedAtMillis(10))
        .expect("the old incarnation expiry compiles")
        .expect("the old incarnation is due");
    assert_eq!(
        compiled.lifecycle_is_current_for_foundation(&authority),
        (true, true)
    );
    apply_plan(
        authority
            .prepare_shared_local_removal_for_foundation(&hash)
            .expect("the old incarnation removal plans")
            .expect("the old incarnation exists"),
    );
    assert_eq!(
        compiled.lifecycle_is_current_for_foundation(&authority),
        (true, true)
    );
    let chain_input = transaction
        .inputs()
        .get(0)
        .expect("the independent transaction has one input")
        .previous_output();
    let readmitted = accept_remote_transaction_with_payload_at(
        &mut authority,
        transaction.clone(),
        189,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &transaction,
            Vec::new(),
            vec![chain_input],
            Capacity::shannons(1_000),
        ),
        AcceptedAtMillis(10),
    );
    assert_eq!(readmitted, hash);
    assert_ne!(owner_version(&authority, &hash), old_version);
    assert_eq!(
        compiled.lifecycle_is_current_for_foundation(&authority),
        (true, true)
    );
    let before = authority.normalized_snapshot();
    let before_effect = authority.effect_observation_for_foundation();
    assert!(matches!(
        compiled.bind(&authority),
        Err(PlanError::Stale(StalePlan::Version))
    ));
    assert_eq!(authority.normalized_snapshot(), before);
    assert_eq!(authority.effect_observation_for_foundation(), before_effect);
    assert!(matches!(authority.entry(&hash), Some(OwnedTx::Accepted(_))));
    assert_resource_reference(&authority);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_staged_accepted_expiry_excludes_same_root_removal_until_exact_cleanup() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let (root, _) = independent_accepted_at(
        &mut authority,
        90,
        AcceptedAtMillis(10),
        AcceptedStatus::Pending,
    );
    let compiled = authority
        .compile_shared_accepted_expiry(AcceptedAtMillis(10))
        .expect("the root expiry compiles")
        .expect("the root is due");
    let prepared = compiled
        .bind(&authority)
        .expect("the expiry stages every fallible projection first");
    assert!(matches!(
        authority.prepare_shared_local_removal_for_foundation(&root),
        Err(PlanError::Stale(StalePlan::Version))
    ));
    assert!(authority.entry(&root).is_some());

    let expiry = prepared
        .apply()
        .expect("the first exact staged expiry owns the root transition");
    let (committed, fault) = expiry.into_parts();
    assert_eq!(fault, None);
    drop(committed);
    assert!(authority.entry(&root).is_none());
    assert!(
        authority
            .effect_publication_receipt_for_foundation()
            .is_some(),
        "the winning staged expiry publishes its exact Expired effect"
    );
    assert!(
        authority
            .prepare_shared_local_removal_for_foundation(&root)
            .expect("cleanup leaves no stale dependency stage")
            .is_none()
    );
    assert_resource_reference(&authority);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_accepted_expiry_composes_with_same_shard_proposed_removal_in_both_orders() {
    fn fixture() -> (TxPoolAuthority, RawTxHash, RawTxHash) {
        let mut authority = TxPoolAuthority::for_foundation(limits());
        let (expired, _) = independent_accepted_at(
            &mut authority,
            91,
            AcceptedAtMillis(10),
            AcceptedStatus::Proposed,
        );
        let shard = authority.entries_for_reference().owner_shard(&expired);
        let (transaction, chain_input, peer) = (1_000u32..8_000)
            .find_map(|marker| {
                let mut input_hash = [0xa7; 32];
                input_hash[..4].copy_from_slice(&marker.to_le_bytes());
                let chain_input = OutPoint::new(Byte32::new(input_hash), 0);
                let transaction = TransactionBuilder::default()
                    .version(marker)
                    .input(CellInput::new(chain_input.clone(), 0))
                    .output(CellOutput::default())
                    .output_data(Bytes::new().pack())
                    .build();
                let hash = RawTxHash(transaction.hash());
                (authority.entries_for_reference().owner_shard(&hash) == shard).then_some((
                    transaction,
                    chain_input,
                    marker as usize,
                ))
            })
            .expect("the deterministic router fixture finds a same-shard owner");
        let survivor = accept_remote_transaction_with_payload_at(
            &mut authority,
            transaction.clone(),
            peer,
            AcceptedStatus::Proposed,
            resolved_payload_with_facts(
                &transaction,
                Vec::new(),
                vec![chain_input],
                Capacity::shannons(2_000),
            ),
            AcceptedAtMillis(20),
        );
        assert_eq!(
            authority.entries_for_reference().owner_shard(&expired),
            authority.entries_for_reference().owner_shard(&survivor)
        );
        (authority, expired, survivor)
    }

    fn apply(reverse: bool) -> super::super::plan::test_support::AuthoritySnapshot {
        let (authority, expired, disjoint) = fixture();
        let expiry = authority
            .compile_shared_accepted_expiry(AcceptedAtMillis(10))
            .expect("the exact Accepted head compiles")
            .expect("the oldest Proposed owner is due");
        let local = authority
            .prepare_shared_local_removal_for_foundation(&disjoint)
            .expect("the disjoint same-shard removal compiles")
            .expect("the disjoint Accepted owner remains present");
        let expiry = expiry
            .bind(&authority)
            .expect("the Accepted expiry stages before either owner cut");

        let finish_expiry = |expiry: super::super::plan::PreparedSharedAcceptedExpiry<'_>| {
            let shared = expiry.apply().unwrap_or_else(|failure| {
                let (error, _effect_wake) = failure.into_parts();
                panic!("the relatively rebased Accepted expiry commits: {error:?}")
            });
            let (committed, fault) = shared.into_parts();
            assert_eq!(fault, None);
            drop(committed);
        };
        let finish_local = |local: super::super::plan::PreparedSharedLocalRemoval<'_>| {
            let shared = local
                .apply()
                .expect("the relatively rebased disjoint local removal commits");
            let (committed, fault) = shared.into_parts();
            assert_eq!(fault, None);
            drop(committed);
        };
        if reverse {
            finish_local(local);
            finish_expiry(expiry);
        } else {
            finish_expiry(expiry);
            finish_local(local);
        }
        assert!(authority.entry(&expired).is_none());
        assert!(authority.entry(&disjoint).is_none());
        assert_resource_reference(&authority);
        assert!(authority.primary_projection_consistent());
        authority.normalized_snapshot()
    }

    let compiled_order = apply(false);
    let reverse_completion = apply(true);
    assert_eq!(
        compiled_order.first_difference(&reverse_completion),
        None,
        "same-shard resource, Proposed, source and index rows equal canonical sequential state"
    );
}

#[test]
fn uak_shared_accepted_expiry_revalidates_surviving_parent_incarnation() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_chain_input = OutPoint::new(Byte32::new([0x92; 32]), 0);
    let parent_tx = TransactionBuilder::default()
        .version(920u32)
        .input(CellInput::new(parent_chain_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent = accept_remote_transaction_with_payload_at(
        &mut authority,
        parent_tx.clone(),
        920,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &parent_tx,
            Vec::new(),
            vec![parent_chain_input],
            Capacity::shannons(1_000),
        ),
        AcceptedAtMillis(20),
    );
    let root_tx = TransactionBuilder::default()
        .version(921u32)
        .input(CellInput::new(OutPoint::new(parent_tx.hash(), 0), 0))
        .build();
    let root = accept_remote_transaction_with_payload_at(
        &mut authority,
        root_tx.clone(),
        921,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(&root_tx, Vec::new(), Vec::new(), Capacity::shannons(500)),
        AcceptedAtMillis(10),
    );
    let old_parent_version = owner_version(&authority, &parent);
    let compiled = authority
        .compile_shared_accepted_expiry(AcceptedAtMillis(10))
        .expect("the child root and surviving parent compile")
        .expect("the child root is due before its parent");
    assert_eq!(
        compiled.administrative_removal_keys_for_claim(),
        Some(vec![root.clone()])
    );

    apply_plan(
        authority
            .plan_status_for_foundation(&parent, old_parent_version, AcceptedStatus::Gap)
            .expect("the surviving parent changes incarnation without changing ownership"),
    );
    drain_fixture_effects(&mut authority);
    assert_ne!(owner_version(&authority, &parent), old_parent_version);
    let before = authority.normalized_snapshot();
    let before_effect = authority.effect_observation_for_foundation();
    let prepared = compiled
        .bind(&authority)
        .expect("the unrelated parent version is a final-cut witness");
    let failure = match prepared.apply() {
        Ok(_) => panic!("a changed backing parent cannot publish stale availability"),
        Err(failure) => failure,
    };
    let (error, effect_wake) = failure.into_parts();
    assert!(matches!(error, ConcurrentRetainedIngressError::Stale));
    assert!(effect_wake.is_some_and(|wake| wake.capacity_released()));
    assert_eq!(authority.normalized_snapshot(), before);
    assert_eq!(authority.effect_observation_for_foundation(), before_effect);
    assert!(matches!(
        authority.entry(&parent),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(matches!(authority.entry(&root), Some(OwnedTx::Accepted(_))));

    let fresh = authority
        .compile_shared_accepted_expiry(AcceptedAtMillis(10))
        .expect("the current backing incarnation replans")
        .expect("the root remains due");
    assert_eq!(fresh.apply_for_foundation(&authority).retired_len(), 1);
    assert!(matches!(
        authority.entry(&parent),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(authority.entry(&root).is_none());
    assert_resource_reference(&authority);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_accepted_expiry_stages_dependency_loss_before_owner_removal() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_input = OutPoint::new(Byte32::new([0x93; 32]), 0);
    let parent_tx = TransactionBuilder::default()
        .version(930u32)
        .input(CellInput::new(parent_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent = accept_remote_transaction_with_payload_at(
        &mut authority,
        parent_tx.clone(),
        930,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &parent_tx,
            Vec::new(),
            vec![parent_input],
            Capacity::shannons(1_000),
        ),
        AcceptedAtMillis(10),
    );
    let parent_output = OutPoint::new(parent.0.clone(), 0);
    let child_tx = TransactionBuilder::default()
        .version(931u32)
        .cell_dep(
            CellDep::new_builder()
                .out_point(parent_output.clone())
                .build(),
        )
        .build();
    let child = RawTxHash(child_tx.hash());
    apply_plan(
        authority
            .plan_admission(
                ValidatedAdmission::remote(child_tx, PeerIndex::from(931usize))
                    .expect("the dependency consumer admission is valid"),
            )
            .expect("the dependency consumer enters PreAccepted ownership"),
    );
    assert_eq!(
        authority.dependency_consumers_for_foundation(&DependencyKey::Cell(parent_output.clone())),
        Some(std::collections::BTreeSet::from([child.clone()]))
    );

    let compiled = authority
        .compile_shared_accepted_expiry(AcceptedAtMillis(10))
        .expect("the parent loss and exact dependency consumer compile")
        .expect("the parent is due");
    assert_eq!(compiled.apply_for_foundation(&authority).retired_len(), 1);
    assert!(authority.entry(&parent).is_none());
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::PreAccepted(_))
    ));
    assert_eq!(
        authority.dependency_consumers_for_foundation(&DependencyKey::Cell(parent_output.clone())),
        Some(std::collections::BTreeSet::from([child.clone()]))
    );
    assert!(matches!(
        authority
            .dependency_maintenance_observation_for_foundation()
            .expect("the dependency frontier remains coherent"),
        Some((DependencyKey::Cell(key), Some(owner)))
            if key == parent_output && owner == child
    ));
    assert_resource_reference(&authority);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_accepted_expiry_closes_accepted_cell_dependency_descendants() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_input = OutPoint::new(Byte32::new([0x98; 32]), 0);
    let parent_tx = TransactionBuilder::default()
        .version(980u32)
        .input(CellInput::new(parent_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent = accept_remote_transaction_with_payload_at(
        &mut authority,
        parent_tx.clone(),
        980,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &parent_tx,
            Vec::new(),
            vec![parent_input],
            Capacity::shannons(1_000),
        ),
        AcceptedAtMillis(10),
    );
    let parent_output = OutPoint::new(parent.0.clone(), 0);
    let child_tx = TransactionBuilder::default()
        .version(981u32)
        .cell_dep(
            CellDep::new_builder()
                .out_point(parent_output.clone())
                .build(),
        )
        .build();
    let child_payload = ResolvedPayload::for_foundation(
        &child_tx,
        vec![parent_output],
        64,
        Capacity::shannons(500),
        child_tx.data().total_size(),
        Vec::new(),
        Vec::new(),
    )
    .expect("the cell dependency is resolved from the live pool parent");
    let child = accept_remote_transaction_with_payload_at(
        &mut authority,
        child_tx,
        981,
        AcceptedStatus::Pending,
        child_payload,
        AcceptedAtMillis(20),
    );
    assert!(
        authority
            .accepted_children(&parent)
            .is_some_and(|children| children.contains(&child))
    );

    let compiled = authority
        .compile_shared_accepted_expiry(AcceptedAtMillis(10))
        .expect("the complete causal closure compiles")
        .expect("the parent is due");
    assert_eq!(compiled.apply_for_foundation(&authority).retired_len(), 2);
    assert!(authority.entry(&parent).is_none());
    assert!(authority.entry(&child).is_none());
    assert_resource_reference(&authority);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_accepted_expiry_respects_close_and_reorg_lifecycle_guards() {
    let fixture = |marker: u8| {
        let mut authority = TxPoolAuthority::for_foundation(limits());
        let (hash, _) = independent_accepted_at(
            &mut authority,
            marker,
            AcceptedAtMillis(10),
            AcceptedStatus::Pending,
        );
        let compiled = authority
            .compile_shared_accepted_expiry(AcceptedAtMillis(10))
            .expect("the lifecycle fixture compiles")
            .expect("the lifecycle fixture is due");
        (authority, hash, compiled)
    };

    let (mut closed, closed_hash, closed_plan) = fixture(94);
    apply_plan(
        closed
            .plan_effect_close_for_foundation()
            .expect("the effect lifecycle closes before Bind"),
    );
    let closed_before = closed.normalized_snapshot();
    assert!(matches!(
        closed_plan.bind(&closed),
        Err(PlanError::EffectClosed)
    ));
    assert_eq!(closed.normalized_snapshot(), closed_before);
    assert!(matches!(
        closed.entry(&closed_hash),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(closed.primary_projection_consistent());

    let (mut reorged, reorged_hash, reorged_plan) = fixture(95);
    reorged.force_chain_view(ChainViewId::new(ChainRevision(1), Byte32::new([0x95; 32])));
    let reorged_before = reorged.normalized_snapshot();
    assert!(matches!(
        reorged_plan.bind(&reorged),
        Err(PlanError::Stale(StalePlan::Version))
    ));
    assert_eq!(reorged.normalized_snapshot(), reorged_before);
    assert!(matches!(
        reorged.entry(&reorged_hash),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(reorged.primary_projection_consistent());
}

#[test]
fn uak_shared_accepted_expiry_classifies_sealed_plan_mismatch_as_fault() {
    let mut resource_authority = TxPoolAuthority::for_foundation(limits());
    let (resource_parent, resource_child) = accepted_parent_child_at(
        &mut resource_authority,
        96,
        AcceptedAtMillis(10),
        AcceptedAtMillis(20),
    );
    let mut resource_plan = resource_authority
        .compile_shared_accepted_expiry(AcceptedAtMillis(10))
        .expect("the two-owner resource fixture compiles")
        .expect("the resource fixture is due");
    assert!(resource_plan.corrupt_resource_witness_for_foundation());
    let resource_before = resource_authority.normalized_snapshot();
    let resource_failure = match resource_plan
        .bind(&resource_authority)
        .expect("resource witness corruption is adjudicated in the final cut")
        .apply()
    {
        Ok(_) => panic!("a sealed resource contradiction cannot commit or become stale"),
        Err(failure) => failure,
    };
    let (resource_error, resource_wake) = resource_failure.into_parts();
    assert!(matches!(
        resource_error,
        ConcurrentRetainedIngressError::Fault(AuthorityFault::ResourceProjection)
    ));
    assert!(resource_wake.is_some_and(|wake| wake.capacity_released()));
    assert_eq!(resource_authority.normalized_snapshot(), resource_before);
    assert!(resource_authority.entry(&resource_parent).is_some());
    assert!(resource_authority.entry(&resource_child).is_some());
    assert!(resource_authority.primary_projection_consistent());

    let mut proposed_authority = TxPoolAuthority::for_foundation(limits());
    let (proposed, _) = independent_accepted_at(
        &mut proposed_authority,
        97,
        AcceptedAtMillis(10),
        AcceptedStatus::Proposed,
    );
    let mut proposed_plan = proposed_authority
        .compile_shared_accepted_expiry(AcceptedAtMillis(10))
        .expect("the Proposed-count fixture compiles")
        .expect("the Proposed-count fixture is due");
    assert!(proposed_plan.corrupt_proposed_witness_for_foundation());
    let proposed_before = proposed_authority.normalized_snapshot();
    let proposed_failure = match proposed_plan
        .bind(&proposed_authority)
        .expect("Proposed witness corruption is adjudicated in the final cut")
        .apply()
    {
        Ok(_) => panic!("a sealed Proposed-count contradiction cannot commit or become stale"),
        Err(failure) => failure,
    };
    let (proposed_error, proposed_wake) = proposed_failure.into_parts();
    assert!(matches!(
        proposed_error,
        ConcurrentRetainedIngressError::Fault(AuthorityFault::MembershipProjection)
    ));
    assert!(proposed_wake.is_some_and(|wake| wake.capacity_released()));
    assert_eq!(proposed_authority.normalized_snapshot(), proposed_before);
    assert!(proposed_authority.entry(&proposed).is_some());
    assert!(proposed_authority.primary_projection_consistent());
}

#[test]
fn uak_peer_revocation_removes_only_preaccepted_ingress_owners() {
    let banned = PeerIndex::from(708);
    let survivor_peer = PeerIndex::from(709);
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let committed = accept_remote_transaction(
        &mut authority,
        tx(1_710),
        708,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let queued_tx = tx(1_708);
    let queued = admit_remote(&mut authority, 1_708, 708);
    let promoted_tx = tx(1_709);
    let promoted = verify_remote_transaction(&mut authority, promoted_tx.clone(), 708, Vec::new());
    apply_plan(
        authority
            .plan_admission(
                ValidatedAdmission::proposal(promoted_tx).expect("promotion fixture is valid"),
            )
            .expect("promotion preserves immutable ingress"),
    );
    assert!(matches!(
        authority.entry(&promoted),
        Some(OwnedTx::PreAccepted(entry)) if matches!(entry.phase, PreAcceptedPhase::Ready(_))
    ));
    let survivor = admit_remote(&mut authority, 1_711, 709);

    let revoked = authority
        .plan_peer_revocation_for_foundation(banned)
        .expect("bounded peer cohort plans")
        .apply();
    assert_eq!(revoked.retired_len(), 2);
    assert!(authority.entry(&queued).is_none());
    assert!(authority.entry(&promoted).is_none());
    assert!(matches!(
        authority.entry(&committed),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(matches!(
        authority.entry(&survivor),
        Some(OwnedTx::PreAccepted(_))
    ));
    assert!(
        authority
            .preaccepted_for_peer_for_reference(banned)
            .is_empty()
    );
    assert_eq!(
        authority.resources().peer(banned),
        ResourceVector::default()
    );
    assert_eq!(
        authority.preaccepted_for_peer_for_reference(survivor_peer),
        vec![survivor]
    );

    let effect = authority
        .effect_publication_receipt_for_foundation()
        .expect("revocation committed one cleanup batch");
    assert!(matches!(
        effect.effects(),
        [CommittedEffect::PeerCohortRevoked(revocation)]
            if revocation.peer() == banned && revocation.culprit().is_none()
    ));

    let resubmitted = ValidatedAdmission::remote(queued_tx, survivor_peer)
        .expect("another peer may provide the same raw transaction");
    apply_plan(
        authority
            .plan_admission(resubmitted)
            .expect("peer cleanup does not install a raw-hash tombstone"),
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_peer_revocation_removes_active_owner_and_stales_checked_out_work() {
    let peer = PeerIndex::from(710);
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(1_712);
    let hash = admit_remote(&mut authority, 1_712, 710);
    apply_plan(
        authority
            .plan_admission(
                ValidatedAdmission::proposal(transaction).expect("promotion fixture is valid"),
            )
            .expect("promotion plans"),
    );
    let version = owner_version(&authority, &hash);
    let (_, work) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(&hash, version, WorkPermit::ResolveOnly)
            .expect("promoted owner checks out under trusted attribution")
            .apply(),
    );
    assert_eq!(
        authority.resources().peer(peer).active_work,
        0,
        "compute attribution alone cannot prove ingress drain"
    );
    let revoked = authority
        .plan_peer_revocation_for_foundation(peer)
        .expect("active cohort plans without a drain protocol")
        .apply();
    assert_eq!(revoked.retired_len(), 1);
    assert!(authority.entry(&hash).is_none());
    assert_eq!(authority.resources().peer(peer), ResourceVector::default());
    let stale = authority
        .apply_settlement(work.internal_failure())
        .expect_err("a removed active owner cannot publish late state");
    assert_eq!(
        stale.recovery(),
        &ComputeSettlementRecovery::Obsolete(StalePlan::Missing)
    );
    drop(stale);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_clear_pipeline_preserves_live_peer_revocation() {
    let peer = PeerIndex::from(711);
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let _ = admit_remote(&mut authority, 1_715, 711);
    drop(
        authority
            .plan_peer_revocation_for_foundation(peer)
            .expect("peer revocation plans")
            .apply(),
    );
    drop(
        authority
            .plan_clear_pipeline()
            .expect("pool clear plans independently of the peer fence")
            .apply(),
    );

    assert!(
        authority.peer_is_banned_for_reference(peer),
        "clear must not erase an unrelated live network-security decision"
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_peer_revocation_without_resident_owner_still_fences_queued_ingress() {
    let peer = PeerIndex::from(714);
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(1_722);

    let revoked = authority
        .plan_peer_revocation_for_foundation(peer)
        .expect("a ban decision does not depend on a resident cohort")
        .apply();
    assert_eq!(revoked.retired_len(), 0);
    assert!(authority.peer_is_banned_for_reference(peer));

    apply_plan(
        authority
            .plan_admission(
                ValidatedAdmission::remote(transaction, PeerIndex::from(715))
                    .expect("another peer can provide the same transaction"),
            )
            .expect("the fence is peer-scoped"),
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_remote_expiry_is_a_bounded_derived_transition_and_allows_refetch() {
    let next_peer = PeerIndex::from(713);
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let expired = admit_remote_until(&mut authority, 1_715, 712, 10);
    let future = admit_remote_until(&mut authority, 1_716, 712, 20);
    let slice = NonZeroUsize::new(4).expect("fixture slice is non-zero");

    assert!(
        authority
            .plan_remote_expiry(RemoteDeadline(9), slice)
            .expect("a pre-deadline lookup is valid")
            .is_none()
    );
    let committed = authority
        .plan_remote_expiry(RemoteDeadline(10), slice)
        .expect("the due deadline cohort plans")
        .expect("one owner is due")
        .apply_for_foundation(&authority);
    assert_eq!(committed.retired_len(), 1);
    assert!(authority.entry(&expired).is_none());
    assert!(authority.entry(&future).is_some());

    let effect = authority
        .effect_publication_receipt_for_foundation()
        .expect("expiry committed one cleanup batch");
    assert_eq!(
        effect.effects(),
        &[CommittedEffect::RemoteExpired {
            tx_hash: expired.clone(),
        }]
    );

    let resubmitted = ValidatedAdmission::remote_with_lease(
        tx(1_715),
        RemoteResidencyLease::new(next_peer, RemoteDeadline(30)),
        0,
    )
    .expect("another peer can provide the expired raw transaction");
    apply_plan(
        authority
            .plan_admission(resubmitted)
            .expect("expiry installs no raw-hash tombstone"),
    );
    assert!(authority.entry(&expired).is_some());
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_proposal_promotion_suspends_but_retains_the_remote_deadline() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(1_717);
    let hash = admit_remote_until(&mut authority, 1_717, 714, 10);
    apply_plan(
        authority
            .plan_admission(
                ValidatedAdmission::proposal(transaction).expect("proposal fixture is valid"),
            )
            .expect("same-witness promotion plans"),
    );

    let slice = NonZeroUsize::new(4).expect("fixture slice is non-zero");
    assert!(
        authority
            .plan_remote_expiry(RemoteDeadline(20), slice)
            .expect("inactive proposal lookup is valid")
            .is_none(),
        "a live proposal lease, not its retained remote base, controls residency"
    );
    let Some(OwnedTx::PreAccepted(entry)) = authority.entry(&hash) else {
        panic!("promoted owner remains pre-accepted");
    };
    assert!(matches!(
        entry.source,
        PreAcceptedSource::Proposal {
            base: ProposalBase::Remote(residency),
            ..
        } if residency.expires_at == RemoteDeadline(10)
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_proposal_promotion_reclassifies_remote_missing_wait_under_trusted_policy() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(1_718);
    let hash = admit_remote_until(&mut authority, 1_718, 715, 10);
    let checkout = authority
        .plan_checkout_for_foundation(
            &hash,
            owner_version(&authority, &hash),
            WorkPermit::ResolveOnly,
        )
        .expect("remote resolve checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.into_work() else {
        panic!("resolve-only permit returns resolve work");
    };
    apply_plan(
        authority
            .apply_settlement(
                resolve
                    .missing(missing_keys())
                    .expect("the missing parent fixture is bounded"),
            )
            .expect("Remote missing evidence enters the waiting level"),
    );
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Waiting(_))
    ));

    apply_plan(
        authority
            .plan_admission(
                ValidatedAdmission::proposal(transaction)
                    .expect("same-witness proposal promotion is valid"),
            )
            .expect("promotion reclassifies source-dependent waiting state"),
    );
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(
                entry.source,
                PreAcceptedSource::Proposal {
                    base: ProposalBase::Remote(_),
                }
            ) && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert_resource_reference(&authority);

    let checkout = authority
        .plan_checkout_for_foundation(
            &hash,
            owner_version(&authority, &hash),
            WorkPermit::ResolveOnly,
        )
        .expect("the reclassified Proposal is executable")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.into_work() else {
        panic!("resolve-only permit returns resolve work");
    };
    apply_plan(
        authority
            .apply_settlement(
                resolve
                    .missing(missing_keys())
                    .expect("the repeated missing evidence is bounded"),
            )
            .expect("Proposal missing policy reaches a terminal disposition"),
    );
    assert!(authority.entry(&hash).is_none());
    assert!(authority.primary_projection_consistent());
    assert_resource_reference(&authority);
}

#[test]
fn uak_remote_expiry_removes_active_work_without_a_drain_or_prefix_expansion() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let active = admit_remote_until(&mut authority, 1_718, 715, 10);
    let inactive = admit_remote_until(&mut authority, 1_719, 716, 11);
    let active_version = owner_version(&authority, &active);
    let (_, work) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(&active, active_version, WorkPermit::ResolveOnly)
            .expect("due owner checks out one exact capability")
            .apply(),
    );
    let slice = NonZeroUsize::new(2).expect("fixture slice is non-zero");

    let committed = authority
        .plan_remote_expiry(RemoteDeadline(12), slice)
        .expect("the exact due prefix plans without a drain")
        .expect("both due owners are removable")
        .apply_for_foundation(&authority);
    assert_eq!(committed.retired_len(), 2);
    assert!(authority.entry(&active).is_none());
    assert!(authority.entry(&inactive).is_none());
    assert!(
        authority
            .plan_remote_expiry(RemoteDeadline(12), slice)
            .expect("the drained prefix lookup remains valid")
            .is_none()
    );

    let stale = authority
        .apply_settlement(work.internal_failure())
        .expect_err("the removed active owner rejects late settlement as stale");
    assert_eq!(
        stale.recovery(),
        &ComputeSettlementRecovery::Obsolete(StalePlan::Missing)
    );
    drop(stale);

    let effect = authority
        .effect_publication_receipt_for_foundation()
        .expect("the exact due prefix committed one batch");
    assert_eq!(
        effect.effects(),
        &[
            CommittedEffect::RemoteExpired { tx_hash: active },
            CommittedEffect::RemoteExpired { tx_hash: inactive },
        ]
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_duplicate_and_promotion_never_create_second_owner() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(2);
    let first = ValidatedAdmission::remote(transaction.clone(), PeerIndex::from(9))
        .expect("fixture admission is valid");
    let hash = first.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(first)
            .expect("first admission plans"),
    );

    let version = owner_version(&authority, &hash);
    let checkout = authority
        .plan_checkout_for_foundation(
            &hash,
            version,
            WorkPermit::ResolveThenVerify(VerifyCapability::Any),
        )
        .expect("remote resolve checkout plans")
        .apply();
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.into_work() else {
        panic!("continuous permit returns continuous resolve work");
    };

    let duplicate = ValidatedAdmission::proposal(transaction).expect("fixture promotion is valid");
    apply_plan(
        authority
            .plan_admission(duplicate)
            .expect("proposal promotes the existing owner"),
    );
    assert_eq!(authority.owner_count(), 1);
    assert_eq!(authority.charged_count(), 1);
    let owner = authority.entry(&hash).expect("promoted owner exists");
    assert!(matches!(
        owner,
        OwnedTx::PreAccepted(ref entry)
            if matches!(
                entry.source,
                PreAcceptedSource::Proposal {
                    base: ProposalBase::Remote(_),
                }
            )
    ));
    assert_eq!(
        authority.resources().peer(PeerIndex::from(9)),
        owner.preaccepted_charge().expect("owner is preaccepted")
    );
    assert_eq!(
        authority.resources().remote(),
        owner.preaccepted_charge().expect("owner is preaccepted")
    );
    assert!(matches!(
        owner,
        OwnedTx::PreAccepted(ref entry)
            if matches!(entry.phase, PreAcceptedPhase::Computing(_))
    ));
    assert!(authority.primary_projection_consistent());
    assert_resource_reference(&authority);

    apply_plan(
        authority
            .apply_settlement(resolve.rejected(RejectionKind::Policy))
            .expect("promotion does not invalidate the checked-out work"),
    );
}

#[test]
fn uak_payload_variant_is_not_misclassified_as_duplicate() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let raw = tx(23);
    let first = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"first").pack()])
        .build();
    let second = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"second").pack()])
        .build();
    let admission =
        ValidatedAdmission::remote(first, PeerIndex::from(42)).expect("fixture admission is valid");
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("first witness variant plans"),
    );
    let before = authority.normalized_snapshot();
    let variant = ValidatedAdmission::remote(second, PeerIndex::from(43))
        .expect("second witness variant is structurally valid");
    assert_eq!(
        authority.plan_admission(variant).err(),
        Some(PlanError::PayloadVariant)
    );
    assert_eq!(authority.normalized_snapshot(), before);
}

#[test]
fn uak_trusted_witness_replacement_preserves_ingress_and_changes_payload_blame() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let raw = tx(24);
    let remote = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"remote").pack()])
        .build();
    let trusted = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"trusted").pack()])
        .build();
    let peer = PeerIndex::from(42);
    let declared_cycles = 123;
    let initial = ValidatedAdmission::remote_with_lease(
        remote,
        RemoteResidencyLease::for_foundation(peer),
        declared_cycles,
    )
    .expect("remote variant is valid");
    let hash = initial.identity.raw.clone();
    let initial_version = authority.clocks().next_version;
    apply_plan(
        authority
            .plan_admission(initial)
            .expect("remote variant enters ownership"),
    );
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if entry.source.payload_policy()
                == PayloadPolicy::remote_for_foundation(declared_cycles)
    ));

    let replacement =
        ValidatedAdmission::proposal(trusted.clone()).expect("trusted replacement is valid");
    apply_plan(
        authority
            .plan_admission(replacement)
            .expect("trusted witness replaces the inactive remote payload"),
    );

    let owner = authority.entry(&hash).expect("replacement owner exists");
    assert_eq!(owner.record().tx.witness_hash(), trusted.witness_hash());
    assert_eq!(owner.ingress_peer(), Some(peer));
    assert!(matches!(
        owner,
        OwnedTx::PreAccepted(ref entry)
            if entry.source.payload_policy() == PayloadPolicy::Trusted
    ));
    assert!(owner.record().version > initial_version);
    assert!(matches!(
        owner,
        OwnedTx::PreAccepted(ref entry)
            if matches!(
                entry.source,
                PreAcceptedSource::Proposal {
                    base: ProposalBase::Remote(residency),
                } if residency.peer == peer
            ) && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert_eq!(
        authority.resources().peer(peer),
        owner
            .preaccepted_charge()
            .expect("replacement remains peer-resident")
    );
    assert_resource_reference(&authority);
}

#[test]
fn uak_stale_remote_cycle_rejection_requeues_after_same_witness_proposal_promotion() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(626);
    let peer = PeerIndex::from(626);
    let declared_cycles = 100;
    let (hash, verify) =
        checkout_remote_for_verify_with_claim(&mut authority, &transaction, peer, declared_cycles);
    assert_eq!(
        verify.payload_policy(),
        PayloadPolicy::remote_for_foundation(declared_cycles)
    );
    let stale_rejection = verify.verified(declared_cycles + 1);

    apply_plan(
        authority
            .plan_admission(
                ValidatedAdmission::proposal(transaction)
                    .expect("same-witness proposal is trusted"),
            )
            .expect("proposal promotion preserves the checked-out work"),
    );
    apply_plan(
        authority
            .apply_settlement(stale_rejection)
            .expect("stale peer-policy rejection settles under current source authority"),
    );

    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if entry.source.payload_policy() == PayloadPolicy::Trusted
                && entry.source.ingress_peer() == Some(peer)
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Verify(_)))
    ));
    assert!(
        authority
            .effect_publication_receipt_for_foundation()
            .is_none(),
        "a stale peer cycle claim must not publish a trusted payload rejection"
    );

    let checkout = authority
        .plan_checkout_for_foundation(
            &hash,
            owner_version(&authority, &hash),
            WorkPermit::VerifyOnly(VerifyCapability::Any),
        )
        .expect("trusted payload is requeued for verification")
        .apply();
    let CheckedOutWork::Verify(verify) = checkout.into_work() else {
        panic!("verify-only checkout returns verify work");
    };
    assert_eq!(verify.payload_policy(), PayloadPolicy::Trusted);
    apply_plan(
        authority
            .apply_settlement(verify.verified(declared_cycles + 2))
            .expect("trusted verification settles the retained payload"),
    );
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(
                &entry.phase,
                PreAcceptedPhase::Ready(verified)
                    if verified.metrics().cost.cycles == declared_cycles + 2
            )
    ));
    assert_resource_reference(&authority);
}

#[test]
fn uak_remote_verify_failure_requeues_after_same_witness_proposal_promotion() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(628);
    let peer = PeerIndex::from(628);
    let declared_cycles = 1;
    let (hash, verify) =
        checkout_remote_for_verify_with_claim(&mut authority, &transaction, peer, declared_cycles);
    assert_eq!(
        verify.payload_policy(),
        PayloadPolicy::remote_for_foundation(declared_cycles)
    );
    let stale_rejection = verify.rejected(RejectionKind::Verification);

    apply_plan(
        authority
            .plan_admission(
                ValidatedAdmission::proposal(transaction)
                    .expect("same-witness proposal is trusted"),
            )
            .expect("proposal promotion preserves the checked-out work"),
    );
    apply_plan(
        authority
            .apply_settlement(stale_rejection)
            .expect("peer-bounded verify failure settles under current source authority"),
    );

    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if entry.source.payload_policy() == PayloadPolicy::Trusted
                && entry.source.ingress_peer() == Some(peer)
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Verify(_)))
    ));
    assert!(
        authority
            .effect_publication_receipt_for_foundation()
            .is_none(),
        "a verify failure under the superseded peer cycle cap must not reject trusted work"
    );
    assert_resource_reference(&authority);
}

#[test]
fn uak_current_remote_cycle_rejection_terminalizes_with_peer_attribution() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(627);
    let peer = PeerIndex::from(627);
    let declared_cycles = 200;
    let (hash, verify) =
        checkout_remote_for_verify_with_claim(&mut authority, &transaction, peer, declared_cycles);
    let cohort_member = admit_remote(&mut authority, 6_270, 627);
    let rejection = verify.verified(declared_cycles + 1);
    apply_plan(
        authority
            .apply_settlement(rejection)
            .expect("current peer-policy rejection terminalizes atomically"),
    );
    assert!(authority.entry(&hash).is_none());
    assert!(authority.entry(&cohort_member).is_none());
    assert!(authority.peer_is_banned_for_reference(peer));
    let pending = authority
        .pending_recent_reject(&hash)
        .expect("the same Apply indexes the culprit's committed rejection")
        .public_reject()
        .expect("the indexed cohort effect carries exact reject evidence");
    assert!(matches!(
        pending.reject(),
        Reject::DeclaredWrongCycles(200, 201)
    ));

    let lease = authority
        .effect_publication_receipt_for_foundation()
        .expect("rejection is committed with terminalization");
    assert!(matches!(
        lease.effects(),
        [CommittedEffect::PeerCohortRevoked(revocation)]
            if revocation.peer() == peer
                && revocation.culprit().is_some_and(|culprit|
                    culprit.tx_hash() == &hash
                        && matches!(culprit.reason().reject(), Reject::DeclaredWrongCycles(200, 201)))
    ));

    let other_peer = PeerIndex::from(6_271);
    apply_plan(
        authority
            .plan_admission(
                ValidatedAdmission::remote(transaction, other_peer)
                    .expect("another peer may provide the same transaction"),
            )
            .expect("the ban marker is peer-scoped, not a tx tombstone"),
    );
    assert_resource_reference(&authority);
}

fn commit_production_direct_accepted(
    disposition: PreparedSharedDirectAdmissionDisposition<'_>,
) -> CommittedDelta {
    let SharedDirectAdmissionCommitOutcome::Accepted(committed) = disposition.commit() else {
        panic!("the canonical Direct disposition must commit Accepted ownership")
    };
    let (committed, post_commit_fault) = committed.into_parts();
    assert_eq!(post_commit_fault, None);
    committed
}

#[test]
fn uak_direct_local_admission_moves_from_absent_to_accepted_in_one_apply() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let transaction = Arc::new(tx(270));
    let hash = RawTxHash(transaction.hash());
    let verified = direct_verified_facts(
        &transaction,
        Vec::new(),
        Vec::new(),
        Capacity::shannons(1_000),
    );
    let disposition = authority
        .prepare_production_shared_direct_admission_for_foundation(
            Arc::clone(&transaction),
            verified,
            AcceptedStatus::Pending,
        )
        .expect("a validated local transaction has one direct disposition");
    let PreparedSharedDirectAdmissionDisposition::Accepted { .. } = &disposition else {
        panic!("vacant local admission must acquire Accepted ownership");
    };
    let committed = commit_production_direct_accepted(disposition);
    assert_eq!(committed.retired_len(), 0);
    assert_eq!(committed.async_process_observation_count(), 0);
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::Accepted(entry))
            if entry.provenance == AcceptedProvenance::Trusted
                && entry.status() == AcceptedStatus::Pending
                && entry.record.tx.witness_hash() == transaction.witness_hash()
    ));
    assert_eq!(authority.owner_count(), 1);
    assert_eq!(
        authority.resources().preaccepted(),
        ResourceVector::default()
    );
    assert_eq!(authority.resources().accepted().entries, 1);
    assert_membership_reference(&authority);
    assert_resource_reference(&authority);

    let lease = authority
        .effect_publication_receipt_for_foundation()
        .expect("direct admission publishes one outcome");
    assert!(matches!(
        lease.effects(),
        [CommittedEffect::Accepted(CommittedAcceptance::Admission {
            entry,
            status: AcceptedStatus::Pending,
            ingress_peer: None,
        })] if entry.tx.hash() == transaction.hash()
            && entry.ancestors_count == 1
            && entry.descendants_count == 1
    ));
}

#[test]
fn uak_shared_direct_vacancy_occ_rejects_a_second_same_raw_insertion() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let transaction = Arc::new(tx(277));
    let verified = || {
        direct_verified_facts(
            &transaction,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(1_000),
        )
    };
    let first = authority
        .prepare_production_shared_direct_admission_for_foundation(
            Arc::clone(&transaction),
            verified(),
            AcceptedStatus::Pending,
        )
        .expect("the first all-new Direct candidate compiles");
    let second = authority
        .prepare_production_shared_direct_admission_for_foundation(
            Arc::clone(&transaction),
            verified(),
            AcceptedStatus::Pending,
        )
        .expect("the second plan observes the same vacant cut");
    let PreparedSharedDirectAdmissionDisposition::Accepted { .. } = &first else {
        panic!("the first all-new Direct candidate must use shared Apply");
    };
    let PreparedSharedDirectAdmissionDisposition::Accepted { .. } = &second else {
        panic!("the second all-new Direct candidate must use shared Apply");
    };
    assert!(
        first
            .matches_vacant_canonical_for_foundation(&authority)
            .expect("the canonical evaluator remains executable"),
        "Shared eligibility must imply the canonical zero-relation leaf result"
    );
    assert!(matches!(
        first.commit(),
        SharedDirectAdmissionCommitOutcome::Accepted(_)
    ));
    let SharedDirectAdmissionCommitOutcome::Stale {
        effect_wake: Some(wake),
    } = second.commit()
    else {
        panic!("the second vacancy claimant must become stale")
    };
    assert!(wake.capacity_released());
    assert_eq!(authority.owner_count(), 1);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_direct_vacancy_revision_rejects_absent_present_absent_aba() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let transaction = Arc::new(tx(277));
    let hash = RawTxHash(transaction.hash());
    let verified = || {
        direct_verified_facts(
            &transaction,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(1_000),
        )
    };
    let compile = || {
        authority
            .prepare_production_shared_direct_admission_for_foundation(
                Arc::clone(&transaction),
                verified(),
                AcceptedStatus::Pending,
            )
            .expect("the all-new Direct candidate compiles")
    };
    let old = compile();
    let PreparedSharedDirectAdmissionDisposition::Accepted { .. } = &old else {
        panic!("the old vacant candidate must use shared Apply")
    };
    let winner = compile();
    let PreparedSharedDirectAdmissionDisposition::Accepted { .. } = &winner else {
        panic!("the competing vacant candidate must use shared Apply")
    };
    assert!(matches!(
        winner.commit(),
        SharedDirectAdmissionCommitOutcome::Accepted(_)
    ));
    let removal = match authority
        .prepare_shared_local_removal_for_foundation(&hash)
        .expect("the independent Accepted removal plans")
    {
        Some(removal) => removal,
        None => panic!("the independent Accepted owner remains present"),
    };
    drop(
        removal
            .apply()
            .expect("the competing owner is removed through the shared cut"),
    );
    assert!(authority.entry(&hash).is_none());

    let SharedDirectAdmissionCommitOutcome::Stale {
        effect_wake: Some(wake),
    } = old.commit()
    else {
        panic!("the pre-ABA vacant plan must not resurrect the removed owner")
    };
    assert!(wake.capacity_released());
    assert!(authority.entry(&hash).is_none());
    assert_eq!(authority.owner_count(), 0);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_membership_policy_witness_refuses_exhausted_vacancy_identity() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let transaction = Arc::new(tx(278));
    let hash = RawTxHash(transaction.hash());
    authority
        .entries_for_reference()
        .exhaust_owner_vacancy_revision_for_foundation(&hash);
    let before = authority.normalized_snapshot();
    let error = match authority.prepare_production_shared_direct_admission_for_foundation(
        Arc::clone(&transaction),
        direct_verified_facts(
            &transaction,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(1_000),
        ),
        AcceptedStatus::Pending,
    ) {
        Err(error) => error,
        Ok(_) => panic!("exhausted vacancy cannot mint shared insertion authority"),
    };
    assert!(matches!(
        error,
        PlanError::Stale(StalePlan::AcceptedObservation)
    ));
    assert!(
        authority
            .normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before, 0, 0, 0),
        "an incomplete vacancy witness fails before reserving any owner or Apply identity"
    );
}

#[test]
fn uak_shared_direct_same_input_plans_separate_at_the_final_spender_cut() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let shared_input = OutPoint::new(Byte32::new([0xd7; 32]), 0);
    let build = |version: u32| {
        Arc::new(
            TransactionBuilder::default()
                .version(version)
                .input(CellInput::new(shared_input.clone(), 0))
                .build(),
        )
    };
    let left_tx = build(278);
    let right_tx = build(279);
    let compile = |transaction: &Arc<TransactionView>| {
        authority
            .prepare_production_shared_direct_admission_for_foundation(
                Arc::clone(transaction),
                direct_verified_facts(
                    transaction,
                    Vec::new(),
                    vec![shared_input.clone()],
                    Capacity::shannons(1_000),
                ),
                AcceptedStatus::Pending,
            )
            .expect("the chain-backed candidate compiles from the vacant spender cut")
    };
    let left = compile(&left_tx);
    let PreparedSharedDirectAdmissionDisposition::Accepted { .. } = &left else {
        panic!("the left candidate must use canonical shared Apply")
    };
    let right = compile(&right_tx);
    let PreparedSharedDirectAdmissionDisposition::Accepted { .. } = &right else {
        panic!("the right candidate must use canonical shared Apply")
    };
    assert!(matches!(
        left.commit(),
        SharedDirectAdmissionCommitOutcome::Accepted(_)
    ));
    let SharedDirectAdmissionCommitOutcome::Stale {
        effect_wake: Some(wake),
    } = right.commit()
    else {
        panic!("the losing spender must become stale")
    };
    assert!(wake.capacity_released());
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_direct_planning_never_sweeps_an_unrelated_shard_write_lock() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let transaction = Arc::new(tx(281));
    let compile = || {
        authority
            .prepare_production_shared_direct_admission_for_foundation(
                Arc::clone(&transaction),
                direct_verified_facts(
                    &transaction,
                    Vec::new(),
                    Vec::new(),
                    Capacity::shannons(1_000),
                ),
                AcceptedStatus::Pending,
            )
            .expect("the independent Direct candidate compiles")
    };
    let first = compile();
    let PreparedSharedDirectAdmissionDisposition::Accepted { .. } = &first else {
        panic!("the candidate must use shared Apply")
    };
    let support = first
        .physical_apply_support_for_foundation()
        .expect("Accepted Direct owns one physical Apply support");
    let unrelated = (0..AUTHORITY_SHARD_COUNT)
        .find(|shard| !support.touches_for_foundation(*shard))
        .expect("one transaction cannot require every physical shard");
    drop(first);

    let unrelated_guard = authority.entries_for_reference().layout.shards[unrelated].write();
    let (terminal_tx, terminal_rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let handle = scope.spawn(|| {
            let result = compile();
            let _ = terminal_tx.send(result);
        });
        let result = terminal_rx.recv_timeout(std::time::Duration::from_secs(2));
        drop(unrelated_guard);
        handle.join().expect("the bounded planner does not panic");
        assert!(
            matches!(
                result,
                Ok(PreparedSharedDirectAdmissionDisposition::Accepted { .. })
            ),
            "an unrelated physical shard lock cannot block Direct planning"
        );
    });
}

#[test]
fn uak_shared_direct_commit_ignores_the_empty_global_scheduler_domain() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let transaction = Arc::new(tx(282));
    let scheduler = authority.scheduler_frontier_for_foundation();
    let scheduler_guard = scheduler.lock();
    let source_guard = authority.source_versions_lock_for_foundation();
    std::thread::scope(|scope| {
        let (compile_tx, compile_rx) = std::sync::mpsc::channel();
        let compile_authority = &authority;
        let compile_transaction = Arc::clone(&transaction);
        let compile_handle = scope.spawn(move || {
            let result = compile_authority
                .prepare_production_shared_direct_admission_for_foundation(
                    Arc::clone(&compile_transaction),
                    direct_verified_facts(
                        &compile_transaction,
                        Vec::new(),
                        Vec::new(),
                        Capacity::shannons(1_000),
                    ),
                    AcceptedStatus::Pending,
                );
            let _ = compile_tx.send(result);
        });
        let compiled = compile_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("an empty scheduler domain cannot block Direct planning")
            .expect("the independent Direct candidate compiles");
        compile_handle
            .join()
            .expect("the bounded compiler does not panic");
        let PreparedSharedDirectAdmissionDisposition::Accepted { .. } = &compiled else {
            panic!("the candidate must use shared Apply")
        };

        let (probe, owner_committed, release_owner_cut) = ConcurrentRemovalProbe::new();
        authority
            .entries_for_reference()
            .set_shared_owner_commit_probe(Some(probe));
        let (terminal_tx, terminal_rx) = std::sync::mpsc::channel();
        let commit_handle = scope.spawn(move || {
            let result = compiled.commit();
            let _ = terminal_tx.send(result);
        });
        owner_committed
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("the owner cut commits without acquiring the scheduler mutex");
        release_owner_cut
            .send(())
            .expect("release the committed owner cut");
        drop(source_guard);
        drop(scheduler_guard);
        let result = terminal_rx.recv_timeout(std::time::Duration::from_secs(2));
        commit_handle
            .join()
            .expect("the bounded Apply does not panic");
        assert!(
            matches!(result, Ok(SharedDirectAdmissionCommitOutcome::Accepted(_))),
            "an empty scheduler delta cannot serialize Direct owner mutation"
        );
    });
    authority
        .entries_for_reference()
        .set_shared_owner_commit_probe(None);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_two_compatible_direct_candidates_overlap_inside_their_owner_cuts() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let compile = |version: u64| {
        let transaction = Arc::new(tx(version));
        let compiled = authority
            .prepare_production_shared_direct_admission_for_foundation(
                Arc::clone(&transaction),
                direct_verified_facts(
                    &transaction,
                    Vec::new(),
                    Vec::new(),
                    Capacity::shannons(1_000),
                ),
                AcceptedStatus::Pending,
            )
            .expect("the independent Direct candidate compiles");
        let PreparedSharedDirectAdmissionDisposition::Accepted { .. } = &compiled else {
            panic!("the independent candidate must use shared Apply")
        };
        compiled
    };
    let left = compile(286);
    let mut right = None;
    for version in 287..303 {
        let candidate = compile(version);
        if left.is_compatible_with_for_foundation(&candidate) {
            right = Some(candidate);
            break;
        }
        drop(candidate);
    }
    let right = right.expect("the bounded fixture finds one physically disjoint Direct peer");

    let (probe, owner_committed, release_owner_cut) = ConcurrentRemovalProbe::new();
    authority
        .entries_for_reference()
        .set_shared_owner_commit_probe(Some(probe));
    let (terminal_tx, terminal_rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let left_tx = terminal_tx.clone();
        let left_handle = scope.spawn(move || {
            let _ = left_tx.send(left.commit());
        });
        let right_handle = scope.spawn(move || {
            let _ = terminal_tx.send(right.commit());
        });
        owner_committed
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("the first Direct owner cut commits");
        owner_committed
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("the disjoint Direct owner cut overlaps before either is released");
        release_owner_cut
            .send(())
            .expect("release the first Direct owner cut");
        release_owner_cut
            .send(())
            .expect("release the second Direct owner cut");
        let first = terminal_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("the first Direct terminal returns");
        let second = terminal_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("the second Direct terminal returns");
        left_handle
            .join()
            .expect("the left Direct worker does not panic");
        right_handle
            .join()
            .expect("the right Direct worker does not panic");
        assert!(matches!(
            first,
            SharedDirectAdmissionCommitOutcome::Accepted(_)
        ));
        assert!(matches!(
            second,
            SharedDirectAdmissionCommitOutcome::Accepted(_)
        ));
    });
    authority
        .entries_for_reference()
        .set_shared_owner_commit_probe(None);
    assert_eq!(authority.owner_count(), 2);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_direct_capacity_pressure_is_canonical_or_typed_contention() {
    let mut accepted_one = ResourceLimits::new(
        ResourceVector::new(8, 64 * 1024, 64, 8),
        ResourceVector::new(4, 32 * 1024, 32, 4),
        ResourceVector::new(2, 16 * 1024, 16, 2),
        AcceptedResources::new(1, 64 * 1024, 64 * 1024, 64),
        ComputeLimits::new(4 * 1024, 4 * 1024, 16),
    )
    .and_then(|limits| {
        limits.with_replacement_history_limit(ResourceVector::new(4, 32 * 1024, 32, 0))
    })
    .expect("the one-entry Accepted fixture is valid");
    let authority = TxPoolAuthority::for_foundation(accepted_one);
    let incumbent = Arc::new(tx(283));
    let incumbent_compiled = authority
        .prepare_production_shared_direct_admission_for_foundation(
            Arc::clone(&incumbent),
            direct_verified_facts(&incumbent, Vec::new(), Vec::new(), Capacity::shannons(1)),
            AcceptedStatus::Pending,
        )
        .expect("the vacant incumbent compiles");
    let PreparedSharedDirectAdmissionDisposition::Accepted { .. } = &incumbent_compiled else {
        panic!("the incumbent must use shared Apply")
    };
    assert!(matches!(
        incumbent_compiled.commit(),
        SharedDirectAdmissionCommitOutcome::Accepted(_)
    ));

    let challenger = Arc::new(tx(284));
    let challenger_verified = || {
        direct_verified_facts(
            &challenger,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(10_000),
        )
    };
    let challenger_disposition = authority
        .prepare_production_shared_direct_admission_for_foundation(
            Arc::clone(&challenger),
            challenger_verified(),
            AcceptedStatus::Pending,
        )
        .expect("stable full capacity uses the canonical shared evaluator");
    let PreparedSharedDirectAdmissionDisposition::Accepted { .. } = &challenger_disposition else {
        panic!("the stronger candidate must be accepted canonically")
    };
    assert!(matches!(
        challenger_disposition.commit(),
        SharedDirectAdmissionCommitOutcome::Accepted(_)
    ));
    assert!(authority.entry(&RawTxHash(incumbent.hash())).is_none());
    assert!(matches!(
        authority.entry(&RawTxHash(challenger.hash())),
        Some(OwnedTx::Accepted(_))
    ));

    accepted_one = limits();
    let authority = TxPoolAuthority::for_foundation(accepted_one);
    let held = authority
        .hold_positive_accepted_reservation_for_foundation()
        .expect("the sibling plan reserves the Accepted bank");
    let transaction = Arc::new(tx(285));
    assert!(matches!(
        authority.prepare_production_shared_direct_admission_for_foundation(
            Arc::clone(&transaction),
            direct_verified_facts(
                &transaction,
                Vec::new(),
                Vec::new(),
                Capacity::shannons(1_000),
            ),
            AcceptedStatus::Pending,
        ),
        Err(PlanError::ResourceContended(_))
    ));
    held.release();
    assert!(matches!(
        authority
            .prepare_production_shared_direct_admission_for_foundation(
                Arc::clone(&transaction),
                direct_verified_facts(
                    &transaction,
                    Vec::new(),
                    Vec::new(),
                    Capacity::shannons(1_000),
                ),
                AcceptedStatus::Pending,
            )
            .expect("the same candidate compiles after reservation release"),
        PreparedSharedDirectAdmissionDisposition::Accepted { .. }
    ));
}

#[test]
fn uak_dropped_direct_local_plan_is_semantically_mutation_free() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let transaction = Arc::new(tx(271));
    let verified = direct_verified_facts(
        &transaction,
        Vec::new(),
        Vec::new(),
        Capacity::shannons(1_000),
    );
    let before = authority.normalized_snapshot();
    let disposition = authority
        .prepare_production_shared_direct_admission_for_foundation(
            transaction,
            verified,
            AcceptedStatus::Pending,
        )
        .expect("direct admission plans without mutating authority state");
    drop(disposition);
    assert!(
        authority
            .normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before, 1, 1, 1),
        "the dropped direct admission burns exactly its owner identity and Apply stamp"
    );
}

#[test]
fn uak_direct_local_replaces_inactive_remote_payload_without_losing_attribution() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let raw = tx(272);
    let remote = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"remote-direct").pack()])
        .build();
    let local = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"local-direct").pack()])
        .build();
    let peer = PeerIndex::from(72);
    let admission =
        ValidatedAdmission::remote(remote, peer).expect("remote fixture admission is valid");
    let hash = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("remote fixture enters PreAccepted ownership"),
    );
    let arrival = authority
        .entry(&hash)
        .expect("remote owner exists")
        .record()
        .arrival;
    let verified = direct_verified_facts(&local, Vec::new(), Vec::new(), Capacity::shannons(2_000));
    let disposition = authority
        .prepare_production_shared_direct_admission_for_foundation(
            Arc::new(local.clone()),
            verified,
            AcceptedStatus::Pending,
        )
        .expect("local payload supersedes the inactive same-raw owner");
    let PreparedSharedDirectAdmissionDisposition::Accepted { .. } = &disposition else {
        panic!("same-raw inactive PreAccepted owner is settled by local acceptance");
    };
    let committed = commit_production_direct_accepted(disposition);
    assert_eq!(committed.retired_len(), 1);
    let owner = authority.entry(&hash).expect("local accepted owner exists");
    assert_eq!(owner.record().arrival, arrival);
    assert_eq!(owner.record().tx.witness_hash(), local.witness_hash());
    assert_eq!(owner.ingress_peer(), Some(peer));
    assert!(matches!(
        owner,
        OwnedTx::Accepted(AcceptedEntry {
            provenance: AcceptedProvenance::Peer { ingress },
            ..
        }) if ingress == peer
    ));
    assert_eq!(
        authority.resources().preaccepted(),
        ResourceVector::default()
    );
    assert_eq!(authority.resources().remote(), ResourceVector::default());
    assert_eq!(authority.resources().peer(peer), ResourceVector::default());
    assert_resource_reference(&authority);

    let lease = authority
        .effect_publication_receipt_for_foundation()
        .expect("direct replacement publishes one outcome");
    assert!(matches!(
        lease.effects(),
        [CommittedEffect::Accepted(CommittedAcceptance::Admission {
            ingress_peer: Some(effect_peer),
            ..
        })] if *effect_peer == peer
    ));
}

#[test]
fn uak_direct_local_atomically_stales_the_matching_remote_compute_capability() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let raw = tx(276);
    let remote = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"remote-active-direct").pack()])
        .build();
    let local = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"local-active-direct").pack()])
        .build();
    let admission = ValidatedAdmission::remote(remote, PeerIndex::from(76))
        .expect("active remote fixture admission is valid");
    let hash = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("active remote fixture enters ownership"),
    );
    let (_, work) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(
                &hash,
                owner_version(&authority, &hash),
                WorkPermit::ResolveOnly,
            )
            .expect("remote owner checks out its unique capability")
            .apply(),
    );
    let verified = direct_verified_facts(&local, Vec::new(), Vec::new(), Capacity::shannons(2_000));
    let old_version = owner_version(&authority, &hash);
    let disposition = authority
        .prepare_production_shared_direct_admission_for_foundation(
            Arc::new(local.clone()),
            verified,
            AcceptedStatus::Pending,
        )
        .expect("validated Local acceptance replaces obsolete active work");
    let PreparedSharedDirectAdmissionDisposition::Accepted { .. } = &disposition else {
        panic!("the direct result must install Accepted ownership");
    };
    let committed = commit_production_direct_accepted(disposition);
    assert_eq!(committed.retired_len(), 1);
    let owner = authority
        .entry(&hash)
        .expect("direct Accepted owner exists");
    assert_ne!(owner.record().version, old_version);
    assert_eq!(owner.record().tx.witness_hash(), local.witness_hash());

    let stale = authority
        .apply_settlement(work.internal_failure())
        .expect_err("the obsolete Remote capability is stale after direct acceptance");
    assert_eq!(
        stale.recovery(),
        &ComputeSettlementRecovery::Obsolete(StalePlan::Version)
    );
    drop(stale);
    assert_resource_reference(&authority);
}

#[test]
fn uak_direct_local_duplicate_commits_an_outcome_without_owner_mutation() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(273);
    let hash = accept_remote_transaction(
        &mut authority,
        transaction.clone(),
        73,
        AcceptedStatus::Proposed,
        Vec::new(),
    );
    let version = owner_version(&authority, &hash);
    let accepted_resources = authority.resources().accepted();
    let verified = direct_verified_facts(
        &transaction,
        Vec::new(),
        Vec::new(),
        Capacity::shannons(1_000),
    );
    let disposition = authority
        .prepare_production_shared_direct_admission_for_foundation(
            Arc::new(transaction),
            verified,
            AcceptedStatus::Pending,
        )
        .expect("an accepted raw hash has a deterministic duplicate outcome");
    let SharedDirectAdmissionCommitOutcome::Duplicate {
        key: duplicate_hash,
        committed,
    } = disposition.commit()
    else {
        panic!("Accepted ownership dominates a racing direct receipt");
    };
    let (retirement, post_commit_fault) = committed.into_parts();
    assert_eq!(post_commit_fault, None);
    drop(retirement);
    assert_eq!(duplicate_hash, hash);
    assert_eq!(owner_version(&authority, &hash), version);
    assert_eq!(authority.resources().accepted(), accepted_resources);

    let lease = authority
        .effect_publication_receipt_for_foundation()
        .expect("duplicate commits one accepted relay outcome");
    assert!(matches!(
        lease.effects(),
        [CommittedEffect::Accepted(CommittedAcceptance::Duplicate {
            tx_hash,
            requesting_peer: None,
        })] if tx_hash == &hash
    ));
    assert_membership_reference(&authority);
    assert_resource_reference(&authority);
}

#[test]
fn uak_direct_local_under_fee_rbf_rejects_without_touching_any_owner() {
    let mut authority = TxPoolAuthority::with_replacement(limits(), FeeRate::from_u64(1_000));
    let chain_input = OutPoint::new(Byte32::new([74; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(274u32)
        .input(CellInput::new(chain_input.clone(), 0))
        .build();
    let victim = accept_remote_transaction_with_payload(
        &mut authority,
        victim_tx.clone(),
        74,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &victim_tx,
            Vec::new(),
            vec![chain_input.clone()],
            Capacity::shannons(10_000),
        ),
    );
    let victim_version = owner_version(&authority, &victim);
    let accepted_resources = authority.resources().accepted();
    let replacement = TransactionBuilder::default()
        .version(275u32)
        .input(CellInput::new(chain_input.clone(), 0))
        .build();
    let replacement_hash = RawTxHash(replacement.hash());
    let verified = direct_verified_facts(
        &replacement,
        Vec::new(),
        vec![chain_input],
        Capacity::shannons(10_000),
    );
    let disposition = authority
        .prepare_production_shared_direct_admission_for_foundation(
            Arc::new(replacement.clone()),
            verified,
            AcceptedStatus::Pending,
        )
        .expect("under-fee RBF is a transaction outcome, not an authority fault");
    let SharedDirectAdmissionCommitOutcome::Rejected { reason, committed } = disposition.commit()
    else {
        panic!("replacement must pay the victim fee plus the configured increment");
    };
    let (retirement, post_commit_fault) = committed.into_parts();
    assert_eq!(post_commit_fault, None);
    drop(retirement);
    assert!(matches!(
        reason,
        MembershipReject::InsufficientReplacementFee { .. }
    ));
    assert!(authority.entry(&replacement_hash).is_none());
    assert_eq!(owner_version(&authority, &victim), victim_version);
    assert_eq!(authority.resources().accepted(), accepted_resources);
    assert_eq!(authority.owner_count(), 1);

    let lease = authority
        .effect_publication_receipt_for_foundation()
        .expect("direct reject commits one exact outcome");
    assert!(matches!(
        lease.effects(),
        [CommittedEffect::Rejected(CommittedRejection::Membership {
            tx,
            audience,
            reason: MembershipReject::InsufficientReplacementFee { .. },
        })] if tx.hash() == replacement.hash() && audience.ingress_peer().is_none()
    ));
    assert_membership_reference(&authority);
    assert_resource_reference(&authority);
}

#[test]
fn uak_active_trusted_witness_replacement_atomically_stales_obsolete_work() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let raw = tx(25);
    let remote = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"remote-active").pack()])
        .build();
    let trusted = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"trusted-active").pack()])
        .build();
    let admission = ValidatedAdmission::remote(remote, PeerIndex::from(43))
        .expect("active remote variant is valid");
    let hash = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("active remote variant enters ownership"),
    );
    let (_, work) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(
                &hash,
                owner_version(&authority, &hash),
                WorkPermit::ResolveOnly,
            )
            .expect("remote variant checks out")
            .apply(),
    );
    let old_version = owner_version(&authority, &hash);
    let replacement =
        ValidatedAdmission::proposal(trusted.clone()).expect("trusted replacement is valid");
    let committed = authority
        .plan_admission(replacement)
        .expect("trusted payload atomically replaces obsolete active work")
        .apply();
    assert_eq!(committed.retired_len(), 1);
    drop(committed);
    let owner = authority.entry(&hash).expect("replacement owner exists");
    assert_ne!(owner.record().version, old_version);
    assert_eq!(owner.record().tx.witness_hash(), trusted.witness_hash());
    let stale = authority
        .apply_settlement(work.internal_failure())
        .expect_err("the old payload completion is stale after replacement");
    assert_eq!(
        stale.recovery(),
        &ComputeSettlementRecovery::Obsolete(StalePlan::Version)
    );
    drop(stale);
    assert_resource_reference(&authority);
}

#[test]
fn uak_accepted_or_recovery_ownership_cannot_be_replaced_by_a_proposal_witness() {
    let raw = tx(26);
    let first = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"first").pack()])
        .build();
    let second = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"second").pack()])
        .build();

    let mut accepted_authority = TxPoolAuthority::for_foundation(limits());
    let accepted = accept_remote_transaction(
        &mut accepted_authority,
        first.clone(),
        44,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let before = accepted_authority.normalized_snapshot();
    let accepted_variant = ValidatedAdmission::proposal(second.clone())
        .expect("accepted duplicate variant is structurally valid");
    assert_eq!(
        accepted_authority.plan_admission(accepted_variant).err(),
        Some(PlanError::Duplicate)
    );
    assert_eq!(accepted_authority.normalized_snapshot(), before);
    assert!(matches!(
        accepted_authority.entry(&accepted),
        Some(OwnedTx::Accepted(_))
    ));

    let mut recovery_authority = TxPoolAuthority::for_foundation(limits());
    apply_plan(
        recovery_authority
            .plan_admission(
                ValidatedAdmission::recovery(first, PoolGeneration(0))
                    .expect("recovery variant is valid"),
            )
            .expect("recovery variant enters ownership"),
    );
    let before = recovery_authority.normalized_snapshot();
    let proposal_variant = ValidatedAdmission::proposal(second)
        .expect("lower-priority proposal variant is structurally valid");
    assert_eq!(
        recovery_authority.plan_admission(proposal_variant).err(),
        Some(PlanError::PayloadVariant)
    );
    assert_eq!(recovery_authority.normalized_snapshot(), before);
}

#[test]
fn uak_short_id_collision_cannot_alias_primary_identity() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let first =
        ValidatedAdmission::remote(tx(3), PeerIndex::from(11)).expect("fixture admission is valid");
    let proposal = first.identity.proposal.clone();
    apply_plan(
        authority
            .plan_admission(first)
            .expect("first admission plans"),
    );

    let mut second =
        ValidatedAdmission::remote(tx(4), PeerIndex::from(12)).expect("fixture admission is valid");
    second.identity.proposal = proposal;
    let result = authority.plan_admission(second).err();
    assert_eq!(
        result,
        Some(PlanError::Backpressure(Backpressure::ProposalCollision))
    );
    assert_eq!(authority.owner_count(), 1);
    assert_eq!(authority.charged_count(), 1);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_stale_membership_plan_is_semantically_mutation_free() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let admission =
        ValidatedAdmission::recovery(tx(5), PoolGeneration(0)).expect("fixture admission is valid");
    let hash = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("admission plans"),
    );
    let before = authority.normalized_snapshot();

    let result = authority
        .plan_accept_for_foundation(&hash, EntryVersion(u128::MAX), AcceptedStatus::Pending)
        .err();
    assert_eq!(result, Some(PlanError::Stale(StalePlan::Version)));
    assert_eq!(authority.normalized_snapshot(), before);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_terminal_outcome_and_effect_commit_together() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let admission = ValidatedAdmission::proposal(tx(6)).expect("fixture admission is valid");
    let hash = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("admission plans"),
    );
    let retained_tx = Arc::clone(&authority.entry(&hash).expect("owner exists").record().tx);
    let version = authority
        .entry(&hash)
        .expect("owner exists")
        .record()
        .version;
    let publication = authority
        .effect_publication_for_foundation(
            EffectPolicy::Trusted,
            vec![CommittedEffect::Rejected(
                CommittedRejection::for_foundation(
                    Arc::clone(&retained_tx),
                    RejectionAudience::foundation(),
                    RejectionKind::Policy,
                ),
            )],
        )
        .expect("fixture effect is bounded");
    let terminal = authority
        .plan_terminalize_with_effect_for_foundation(&hash, version, &publication)
        .expect("terminal plan is complete")
        .apply();

    assert_eq!(authority.owner_count(), 0);
    assert_eq!(authority.charged_count(), 0);
    assert!(authority.primary_projection_consistent());
    assert_eq!(terminal.retired_len(), 1);
    assert_eq!(terminal.retired_effect_len(), 0);
    assert_eq!(Arc::strong_count(&retained_tx), 3);
    drop(terminal);
    drop(publication);
    assert_eq!(Arc::strong_count(&retained_tx), 2);

    let lease = authority
        .effect_publication_receipt_for_foundation()
        .expect("committed effect is available");
    assert_eq!(lease.effects().len(), 1);
    assert!(matches!(
        &lease.effects()[0],
        CommittedEffect::Rejected(CommittedRejection::Validation { tx, reason, .. })
            if Arc::ptr_eq(tx, &retained_tx)
                && *reason == RejectionKind::Policy.into()
    ));
    let published = authority
        .apply_effect_settlement_for_foundation(lease.complete_for_foundation().published())
        .expect("published effect settles");
    assert_eq!(published.retired_effect_len(), 1);
    assert_eq!(Arc::strong_count(&retained_tx), 2);
    drop(published);
    assert_eq!(Arc::strong_count(&retained_tx), 1);
}

#[test]
fn uak_all_four_preaccepted_phases_are_closed_variants() {
    let transaction = tx(0);
    let bytes = transaction.data().total_size();
    let phases = [
        PreAcceptedPhase::Queued(QueuedWork::Resolve),
        PreAcceptedPhase::Computing(ActiveWork {
            chain_view: ChainViewId::initial(),
            permit: WorkPermit::ResolveThenVerify(VerifyCapability::Any),
            grant: ComputeGrant::for_foundation(bytes, 0),
            attribution: ComputeAttribution::Trusted,
            payload_policy: PayloadPolicy::Trusted,
            dependency_cut: DependencyCut(ApplySequence(1)),
            dependencies: KnownDependencies::default(),
        }),
        PreAcceptedPhase::Waiting(observed(1)),
        PreAcceptedPhase::Ready(VerifiedFacts::for_foundation(
            ChainRevision(0),
            DependencyCut(ApplySequence(1)),
            Arc::new(resolved_payload(&transaction).into_payload()),
            CandidateMetrics {
                fee: Capacity::shannons(1),
                cost: AcceptedCost::new(bytes, bytes, 0),
            },
        )),
    ];
    assert_eq!(phases.len(), 4);
}

#[test]
fn uak_foundation_types_preserve_distinct_domains_without_dead_state() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let admission =
        ValidatedAdmission::remote(tx(7), PeerIndex::from(17)).expect("fixture admission is valid");
    let hash = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("admission plans"),
    );
    let owner = authority.entry(&hash).expect("owner exists");
    let record = owner.record();
    assert_eq!(record.tx.hash(), hash.0);
    assert_eq!(owner.ingress_peer(), Some(PeerIndex::from(17)));
    assert!(matches!(
        owner,
        OwnedTx::PreAccepted(ref entry)
            if entry.source.payload_blame_peer() == Some(PeerIndex::from(17))
    ));
    assert_eq!(record.arrival.0, 0);
    assert_eq!(authority.chain_revision(), ChainRevision(0));
    assert_eq!(authority.chain_view(), &ChainViewId::initial());
    assert_eq!(authority.generation(), PoolGeneration(0));
    assert_eq!(authority.resources().remote().entries, 1);
    let declared_dependencies = match &owner {
        OwnedTx::PreAccepted(entry) => entry.basis.dependencies().clone(),
        OwnedTx::Accepted(_) | OwnedTx::ReplacementHistory(_) => {
            unreachable!("fixture starts preaccepted")
        }
    };

    let resolved = super::super::state::ResolvedFacts::for_foundation(
        ChainRevision(0),
        DependencyCut(ApplySequence(1)),
        Arc::new(resolved_payload(&tx(0)).into_payload()),
        VerifyCycleClass::Small,
    );
    let variants = [
        PreAcceptedPhase::Queued(QueuedWork::Verify(resolved)),
        PreAcceptedPhase::Computing(ActiveWork {
            chain_view: ChainViewId::initial(),
            permit: WorkPermit::ResolveOnly,
            grant: ComputeGrant::for_foundation(1, 1),
            attribution: ComputeAttribution::Peer(PeerIndex::from(17)),
            payload_policy: PayloadPolicy::remote_for_foundation(0),
            dependency_cut: DependencyCut(ApplySequence(1)),
            dependencies: declared_dependencies.clone(),
        }),
        PreAcceptedPhase::Computing(ActiveWork {
            chain_view: ChainViewId::initial(),
            permit: WorkPermit::VerifyOnly(VerifyCapability::Any),
            grant: ComputeGrant::for_foundation(1, 1),
            attribution: ComputeAttribution::Peer(PeerIndex::from(17)),
            payload_policy: PayloadPolicy::remote_for_foundation(0),
            dependency_cut: DependencyCut(ApplySequence(1)),
            dependencies: declared_dependencies,
        }),
        PreAcceptedPhase::Waiting(observed(1)),
        PreAcceptedPhase::Ready(VerifiedFacts::for_foundation(
            ChainRevision(0),
            DependencyCut(ApplySequence(1)),
            Arc::new(resolved_payload(&tx(0)).into_payload()),
            CandidateMetrics {
                fee: Capacity::shannons(1),
                cost: AcceptedCost::new(1, 1, 0),
            },
        )),
    ];
    assert_eq!(variants.len(), 5);

    let verified_transaction = Arc::clone(&owner.record().tx);
    let verified_bytes = verified_transaction.data().total_size();
    let verified = VerifiedFacts::for_foundation(
        ChainRevision(0),
        DependencyCut(ApplySequence(1)),
        Arc::new(resolved_payload(&verified_transaction).into_payload()),
        CandidateMetrics {
            fee: Capacity::shannons(1),
            cost: AcceptedCost::new(verified_bytes, verified_bytes, 0),
        },
    );
    let changed = owner
        .with_preaccepted_phase(
            PreAcceptedPhase::Ready(verified.clone()),
            EntryVersion(9),
            owner.preaccepted_charge().expect("owner is preaccepted"),
        )
        .expect("preaccepted owner accepts a preaccepted phase");
    let accepted = match changed {
        OwnedTx::PreAccepted(entry) => OwnedTx::Accepted(AcceptedEntry {
            record: entry.record,
            provenance: entry.source.accepted_provenance(),
            proof: AcceptedProof::for_foundation(verified),
            proposal: ProposalContextReceipt::from_internal_status(AcceptedStatus::Gap),
            accepted_at: AcceptedAtMillis::FOUNDATION,
        }),
        OwnedTx::Accepted(_) | OwnedTx::ReplacementHistory(_) => {
            unreachable!("fixture starts preaccepted")
        }
    };
    assert!(matches!(
        accepted,
        OwnedTx::Accepted(ref entry) if entry.status() == AcceptedStatus::Gap
    ));
    assert_ne!(AcceptedStatus::Proposed, AcceptedStatus::Pending);
}

#[test]
fn uak_expanded_footprint_is_canonical_bounded_and_role_aware() {
    let input = OutPoint::new(Byte32::new([1; 32]), 0);
    let dependency = OutPoint::new(Byte32::new([2; 32]), 0);
    let declared_dependency = OutPoint::new(Byte32::new([5; 32]), 0);
    let header = Byte32::new([3; 32]);
    let transaction = TransactionBuilder::default()
        .input(CellInput::new(input.clone(), 0))
        .cell_dep(
            CellDep::new_builder()
                .out_point(declared_dependency.clone())
                .build(),
        )
        .header_dep(header.clone())
        .build();
    let footprint = ExpandedFootprint::from_transaction(
        &transaction,
        vec![dependency.clone(), input.clone(), dependency.clone()],
        4,
    )
    .expect("normalized footprint fits the exact edge bound");
    assert_eq!(footprint.inputs(), std::slice::from_ref(&input));
    assert_eq!(
        footprint.dependencies(),
        &[dependency.clone(), declared_dependency]
    );
    assert_eq!(footprint.header_dependencies(), &[header]);
    assert_eq!(footprint.edge_count(), 4);
    assert_eq!(
        ExpandedFootprint::from_transaction(&transaction, Vec::new(), 1),
        Err(FootprintError::TooManyEdges)
    );
    let resident_bytes = transaction.data().total_size();
    let payload = ResolvedPayload::for_foundation(
        &transaction,
        vec![dependency.clone(), input.clone(), dependency.clone()],
        4,
        Capacity::shannons(1),
        resident_bytes,
        vec![input.clone()],
        vec![dependency.clone()],
    )
    .expect("fixture chain evidence names one exact input");
    assert!(payload.is_chain_input(&input));
    assert!(payload.is_chain_dependency(&dependency));
    assert!(matches!(
        ResolvedPayload::for_foundation(
            &transaction,
            vec![dependency],
            4,
            Capacity::shannons(1),
            resident_bytes,
            vec![OutPoint::new(Byte32::new([4; 32]), 0)],
            Vec::new(),
        ),
        Err(super::super::state::test_support::FoundationInputEvidenceError::NotAnInput)
    ));

    let duplicate_input = TransactionBuilder::default()
        .input(CellInput::new(input.clone(), 0))
        .input(CellInput::new(input, 0))
        .build();
    assert_eq!(
        ExpandedFootprint::from_transaction(&duplicate_input, Vec::new(), 3),
        Err(FootprintError::DuplicateInput)
    );
}

#[test]
fn uak_membership_projects_one_spender_and_one_causal_graph() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = TransactionBuilder::default()
        .version(40u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent_hash = accept_remote_transaction(
        &mut authority,
        parent_tx.clone(),
        52,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let parent_output = OutPoint::new(parent_tx.hash(), 0);
    let child_tx = TransactionBuilder::default()
        .version(41u32)
        .input(CellInput::new(parent_output.clone(), 0))
        .build();
    let child_hash = accept_remote_transaction(
        &mut authority,
        child_tx,
        53,
        AcceptedStatus::Proposed,
        Vec::new(),
    );

    assert_eq!(
        authority.accepted_spender(&parent_output),
        Some(child_hash.clone())
    );
    assert_eq!(
        authority
            .accepted_parents(&child_hash)
            .expect("accepted child has a graph row"),
        HashSet::from([parent_hash.clone()])
    );
    assert_eq!(
        authority
            .accepted_children(&parent_hash)
            .expect("accepted parent has a graph row"),
        HashSet::from([child_hash])
    );
    let counts = authority.membership_counts();
    assert_eq!((counts.pending, counts.gap, counts.proposed), (1, 0, 1));
    assert_eq!(authority.resources().preaccepted().entries, 0);
    assert_eq!(authority.resources().accepted().entries, 2);
    assert_resource_reference(&authority);
}

#[test]
fn uak_fan_in_updates_each_ancestor_from_one_canonical_graph_delta() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let left_tx = TransactionBuilder::default()
        .version(225u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let right_tx = TransactionBuilder::default()
        .version(226u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let left = accept_remote_transaction(
        &mut authority,
        left_tx.clone(),
        225,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let right = accept_remote_transaction(
        &mut authority,
        right_tx.clone(),
        226,
        AcceptedStatus::Gap,
        Vec::new(),
    );
    let child_tx = TransactionBuilder::default()
        .version(227u32)
        .input(CellInput::new(OutPoint::new(left_tx.hash(), 0), 0))
        .input(CellInput::new(OutPoint::new(right_tx.hash(), 0), 0))
        .build();
    let child = accept_remote_transaction(
        &mut authority,
        child_tx,
        227,
        AcceptedStatus::Proposed,
        Vec::new(),
    );

    assert_eq!(
        authority
            .accepted_parents(&child)
            .expect("fan-in child has one parent row"),
        HashSet::from([left.clone(), right.clone()])
    );
    assert_eq!(
        authority
            .accepted_children(&left)
            .expect("left parent has one child row"),
        HashSet::from([child.clone()])
    );
    assert_eq!(
        authority
            .accepted_children(&right)
            .expect("right parent has one child row"),
        HashSet::from([child])
    );
    assert_resource_reference(&authority);
}

#[test]
fn uak_status_reconcile_updates_count_and_eviction_projection_once() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash =
        accept_remote_transaction(&mut authority, tx(70), 70, AcceptedStatus::Gap, Vec::new());
    let version = owner_version(&authority, &hash);
    let demotion_plan = authority
        .plan_status_for_foundation(&hash, version, AcceptedStatus::Pending)
        .expect("Gap demotion is one membership transition");
    assert_eq!(
        demotion_plan.proposed_count_delta_len_for_foundation(),
        Some(0),
        "Pending and Gap share the public aggregate class and need no stored counter write"
    );
    let demotion = apply_plan_for_delta(demotion_plan);
    assert_eq!(demotion.retired_len(), 1);
    let counts = authority.membership_counts();
    assert_eq!((counts.pending, counts.gap, counts.proposed), (1, 0, 0));
    assert_resource_reference(&authority);

    let before = authority.normalized_snapshot();
    let version = owner_version(&authority, &hash);
    assert_eq!(
        authority
            .plan_status_for_foundation(&hash, version, AcceptedStatus::Pending)
            .err(),
        Some(PlanError::Duplicate)
    );
    assert_eq!(authority.normalized_snapshot(), before);

    let promotion_plan = authority
        .plan_status_for_foundation(&hash, version, AcceptedStatus::Proposed)
        .expect("Pending promotion is one membership transition");
    assert_eq!(
        promotion_plan.proposed_count_delta_len_for_foundation(),
        Some(1),
        "crossing the Proposed boundary changes exactly one shard scalar"
    );
    apply_plan(promotion_plan);
    let counts = authority.membership_counts();
    assert_eq!((counts.pending, counts.gap, counts.proposed), (0, 0, 1));
    assert_resource_reference(&authority);
}

#[test]
fn uak_proposed_count_batch_elides_same_shard_net_zero() {
    let entries = ShardedOwnerMap::new(AuthorityShardRouter::new());
    let removed = RawTxHash(Byte32::new([1; 32]));
    let shard = entries.owner_shard(&removed);
    let inserted = (2u8..=u8::MAX)
        .map(|byte| RawTxHash(Byte32::new([byte; 32])))
        .find(|hash| entries.owner_shard(hash) == shard)
        .expect("the fixed 64-shard layout has another key in this bounded search");

    let stale_candidate = entries
        .plan_proposed_counts(std::iter::once((
            &inserted,
            None,
            Some(AcceptedStatus::Proposed),
        )))
        .expect("the independent proposed target plans from the empty shard");
    {
        let cut = entries.write_cut(entries.owner_write_support([&inserted]));
        assert!(cut.proposed_count_plan_is_fresh(&stale_candidate));
    }

    let initial = entries
        .plan_proposed_counts(std::iter::once((
            &removed,
            None,
            Some(AcceptedStatus::Proposed),
        )))
        .expect("one proposed owner is representable");
    assert_eq!(initial.len(), 1);
    let mut initial_cut = entries.write_cut(entries.owner_write_support([&removed]));
    initial_cut.apply_proposed_counts(initial);
    drop(initial_cut);

    {
        let cut = entries.write_cut(entries.owner_write_support([&inserted]));
        assert!(
            !cut.proposed_count_plan_is_fresh(&stale_candidate),
            "a distinct same-shard status commit invalidates the captured scalar base"
        );
    }

    let replacement = entries
        .plan_proposed_counts([
            (&removed, Some(AcceptedStatus::Proposed), None),
            (&inserted, None, Some(AcceptedStatus::Proposed)),
        ])
        .expect("a same-shard proposed replacement preserves the aggregate");
    assert_eq!(
        replacement.len(),
        0,
        "a same-shard net-zero batch must not acquire or write an aggregate scalar"
    );
    assert_eq!(
        entries.read_all().proposed_count(),
        Some(1),
        "eliding the net-zero plan preserves the existing exact aggregate"
    );
}

#[test]
fn uak_independent_batch_shape_is_non_empty_unique_and_bounded_by_type() {
    assert_eq!(
        SettlementBatch::new(Vec::new()),
        Err(CandidateBatchError::Empty)
    );

    let (authority, hashes) = independent_fixture(1);
    let hash = hashes.first().expect("fixture has one candidate").clone();
    let candidate = authority
        .independent_candidate_for_foundation(
            &hash,
            owner_version(&authority, &hash),
            AcceptedStatus::Pending,
        )
        .expect("fixture candidate has current final evidence");
    let candidates = (0..9).map(|_| candidate.clone()).collect();
    assert_eq!(
        SettlementBatch::new(candidates),
        Err(CandidateBatchError::TooLarge { limit: 8 })
    );

    assert_eq!(
        SettlementBatch::new(vec![candidate.clone(), candidate]),
        Err(CandidateBatchError::Duplicate(hash))
    );
}

#[test]
fn uak_independent_run_matches_every_canonical_single_prefix() {
    for count in 1..=4 {
        let (mut aggregate, hashes) = independent_fixture(count);
        let batch_sequence = aggregate.clocks().next_sequence;
        let batch = independent_batch(&aggregate, &hashes);
        let SettlementPlan::IndependentRun(plan) = aggregate
            .plan_settlement(&batch)
            .expect("independent cohort classification is total")
        else {
            panic!("chain-backed disjoint cohort must remain independent");
        };
        let committed_order = plan
            .independent_order_for_foundation()
            .expect("the sealed Plan exposes its test-only independent order");
        assert_eq!(committed_order.len(), count);
        assert_eq!(
            committed_order,
            hashes.iter().rev().cloned().collect::<Vec<_>>()
        );
        let aggregate_committed = apply_plan_for_delta(plan);
        assert!(aggregate_committed.removals.is_empty());
        assert_eq!(aggregate_committed.retired_len(), count);

        let (mut reference, reference_hashes) = independent_fixture(count);
        assert_eq!(reference_hashes, hashes);
        for expected in &committed_order {
            let version = owner_version(&reference, expected);
            drop(apply_plan_for_delta(
                reference
                    .plan_accept_for_foundation(expected, version, AcceptedStatus::Pending)
                    .expect("canonical single reference accepts the same candidate"),
            ));
        }

        let canonical_next_sequence = ApplySequence(
            batch_sequence.0 + u128::try_from(count).expect("fixture count fits u128"),
        );
        assert_eq!(
            aggregate.clocks().next_sequence,
            ApplySequence(batch_sequence.0 + 1)
        );
        assert_eq!(reference.clocks().next_sequence, canonical_next_sequence);

        assert!(
            aggregate
                .normalized_snapshot()
                .equivalent_modulo_atomic_batch_stamp(
                    &reference.normalized_snapshot(),
                    batch_sequence,
                    canonical_next_sequence,
                ),
            "commuting Apply must equal the canonical no-interleave fold under the one-stamp quotient"
        );
        assert_resource_reference(&aggregate);
        assert_membership_reference(&aggregate);
    }
}

#[test]
fn uak_popular_dependency_appends_one_key_routed_consumer_row() {
    const READER_COUNT: usize = 48;
    let bounded = limits().with_accepted_for_foundation(AcceptedResources::new(
        READER_COUNT + 1,
        1024 * 1024,
        1024 * 1024,
        1024 * 1024,
    ));
    let mut authority = TxPoolAuthority::for_foundation(bounded);
    let shared_dependency = OutPoint::new(Byte32::new([199; 32]), 0);
    let mut expected_readers = HashSet::new();
    for index in 0..READER_COUNT {
        let marker = u8::try_from(index + 1).expect("fixture marker fits");
        let input = OutPoint::new(Byte32::new([marker; 32]), 0);
        let transaction = TransactionBuilder::default()
            .version(400 + u32::from(marker))
            .input(CellInput::new(input.clone(), 0))
            .build();
        let payload = resolved_payload_with_facts(
            &transaction,
            vec![shared_dependency.clone()],
            vec![input],
            Capacity::shannons(1_000 + u64::from(marker)),
        );
        expected_readers.insert(accept_remote_transaction_with_payload(
            &mut authority,
            transaction,
            400 + index,
            AcceptedStatus::Pending,
            payload,
        ));
    }

    assert_eq!(
        authority.dependency_consumers_for_foundation(&DependencyKey::Cell(shared_dependency)),
        Some(expected_readers.into_iter().collect())
    );
    assert_membership_reference(&authority);
    assert_resource_reference(&authority);
}

#[test]
fn uak_resource_batch_is_a_commutative_set_transition() {
    let bound = usize::MAX / 8;
    let unbounded = ResourceVector::new(bound, bound, bound, 1);
    let mut ledger = TestResourceLedger::new(
        ResourceLimits::new(
            unbounded,
            unbounded,
            unbounded,
            AcceptedResources::new(bound, bound, bound, u64::MAX),
            ComputeLimits::new(1, 1, 1),
        )
        .expect("large finite fixture has a checked transient ceiling"),
    );
    let first = RawTxHash(tx(480).hash());
    let second = RawTxHash(tx(481).hash());
    let first_before = ChargeRecord::PreAccepted {
        resources: ResourceVector::new(1, bound - 1, 0, 0),
        residency_peer: None,
        compute_peer: None,
    };
    let second_before = ChargeRecord::PreAccepted {
        resources: ResourceVector::new(1, 1, 0, 0),
        residency_peer: None,
        compute_peer: None,
    };
    let first_after = ChargeRecord::PreAccepted {
        resources: ResourceVector::new(1, bound, 0, 0),
        residency_peer: None,
        compute_peer: None,
    };
    let first_plan = ledger
        .plan_replace(first.clone(), None, Some(first_before))
        .expect("first exact charge fits");
    ledger.apply(first_plan);
    let second_plan = ledger
        .plan_replace(second.clone(), None, Some(second_before))
        .expect("second exact charge fills the byte limit");
    ledger.apply(second_plan);

    let plan = ledger
        .plan_batch(vec![
            (first, Some(first_before), Some(first_after)),
            (second, Some(second_before), None),
        ])
        .expect("net-neutral batch does not depend on caller order");
    ledger.apply_batch(plan);
    let snapshot = ledger.snapshot();
    assert_eq!(snapshot.preaccepted, ResourceVector::new(1, bound, 0, 0));
}

#[test]
fn uak_independent_ready_order_is_invariant_to_worker_completion_permutations() {
    let permutations = [
        [0usize, 1, 2, 3],
        [3, 2, 1, 0],
        [1, 3, 0, 2],
        [2, 0, 3, 1],
        [0, 2, 1, 3],
        [3, 1, 2, 0],
    ];
    let mut expected_snapshot = None;
    let mut expected_order = None;
    for permutation in permutations {
        let (mut authority, hashes) = independent_fixture(4);
        let requested = permutation
            .into_iter()
            .map(|index| hashes[index].clone())
            .collect::<Vec<_>>();
        let batch = independent_batch(&authority, &requested);
        let SettlementPlan::IndependentRun(plan) = authority
            .plan_settlement(&batch)
            .expect("permutation remains a valid settlement request")
        else {
            panic!("worker completion order cannot create coupling");
        };
        let order = plan
            .independent_order_for_foundation()
            .expect("cohort has one canonical sealed order");
        drop(apply_plan_for_delta(plan));
        let snapshot = authority.normalized_snapshot();
        if let Some(expected) = &expected_snapshot {
            assert_eq!(&snapshot, expected);
            assert_eq!(Some(&order), expected_order.as_ref());
        } else {
            expected_snapshot = Some(snapshot);
            expected_order = Some(order);
        }
    }
}

#[test]
fn uak_mixed_ready_settlement_preserves_effect_headroom_by_source_control() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let remote_tx = tx(1_781);
    let remote = verify_remote_transaction_with_payload(
        &mut authority,
        remote_tx,
        1_781,
        resolved_payload_with_facts(&tx(1_781), Vec::new(), Vec::new(), Capacity::shannons(1)),
    );
    let proposal_tx = tx(1_782);
    let proposal = verify_remote_transaction_with_payload(
        &mut authority,
        proposal_tx.clone(),
        1_782,
        resolved_payload_with_facts(&proposal_tx, Vec::new(), Vec::new(), Capacity::shannons(1)),
    );
    apply_plan(
        authority
            .plan_admission(
                ValidatedAdmission::proposal(proposal_tx)
                    .expect("proposal promotion is structurally valid"),
            )
            .expect("same-witness proposal promotion preserves Ready work"),
    );

    for nonce in 0..8u8 {
        let publication = authority
            .effect_publication_for_foundation(
                EffectPolicy::Remote,
                vec![CommittedEffect::Accepted(CommittedAcceptance::Duplicate {
                    tx_hash: RawTxHash(Byte32::new([nonce; 32])),
                    requesting_peer: Some(PeerIndex::from(1_800 + usize::from(nonce))),
                })],
            )
            .expect("fixture Remote effect fits its static batch bound");
        apply_plan(
            authority
                .plan_effect_publication_for_foundation(&publication)
                .expect("fixture fills the Remote region exactly"),
        );
    }

    let batch = independent_batch(&authority, &[remote.clone(), proposal.clone()]);
    let SettlementPlan::IndependentRun(plan) = authority
        .plan_settlement(&batch)
        .expect("trusted control keeps its independent headroom")
    else {
        panic!("disjoint Ready candidates remain independent");
    };
    assert_eq!(
        plan.independent_order_for_foundation()
            .expect("trusted control selects one independent owner")
            .len(),
        1
    );
    let committed = apply_plan_for_delta(plan);
    drop(committed);
    assert!(matches!(
        authority.entry(&proposal),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(matches!(
        authority.entry(&remote),
        Some(OwnedTx::PreAccepted(entry)) if matches!(entry.phase, PreAcceptedPhase::Ready(_))
    ));

    drain_fixture_effects(&mut authority);
    let batch = independent_batch(&authority, std::slice::from_ref(&remote));
    let SettlementPlan::IndependentRun(plan) = authority
        .plan_settlement(&batch)
        .expect("Remote owner remains level-triggered after capacity returns")
    else {
        panic!("the remaining chain-backed candidate is independent");
    };
    assert_eq!(
        plan.independent_order_for_foundation()
            .expect("the remaining Remote owner is selected")
            .len(),
        1
    );
    let committed = apply_plan_for_delta(plan);
    drop(committed);
    assert!(matches!(
        authority.entry(&remote),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_independent_batch_reserves_one_apply_sequence_and_distinct_versions() {
    let (mut authority, hashes) = independent_fixture(3);
    let before = authority.clocks();
    let batch = independent_batch(&authority, &hashes);
    let SettlementPlan::IndependentRun(plan) = authority
        .plan_settlement(&batch)
        .expect("independent Plan reserves one complete clock range")
    else {
        panic!("fixture is independent");
    };
    drop(apply_plan_for_delta(plan));

    let after = authority.clocks();
    assert_eq!(
        after.next_sequence,
        ApplySequence(before.next_sequence.0 + 1)
    );
    assert_eq!(
        after.next_version,
        EntryVersion(
            before.next_version.0 + u128::try_from(hashes.len()).expect("fixture length fits u128")
        )
    );
    let template = authority.template_source_versions_for_reference();
    let initial_source = TemplateSelectionSource::from_barrier(ApplySequence(0));
    assert_ne!(template.proposals, initial_source);
    assert_ne!(template.transactions, initial_source);

    let versions = hashes
        .iter()
        .map(|hash| owner_version(&authority, hash))
        .collect::<HashSet<_>>();
    assert_eq!(versions.len(), hashes.len());
    for offset in 0..hashes.len() {
        assert!(versions.contains(&EntryVersion(
            before.next_version.0 + u128::try_from(offset).expect("fixture offset fits u128")
        )));
    }
    assert_resource_reference(&authority);
}

#[test]
fn uak_independent_plan_drop_and_batch_clock_failure_are_mutation_free() {
    let (mut dropped, hashes) = independent_fixture(3);
    let before = dropped.normalized_snapshot();
    let batch = independent_batch(&dropped, &hashes);
    let SettlementPlan::IndependentRun(plan) = dropped
        .plan_settlement(&batch)
        .expect("independent Plan can be prepared")
    else {
        panic!("fixture is independent");
    };
    drop(plan);
    assert!(
        dropped
            .normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before, 3, 0, 1),
        "the dropped three-member batch burns exactly three replacement versions and one Apply stamp"
    );

    let (mut final_sequence, hashes) = independent_fixture(2);
    final_sequence.force_next_sequence(ApplySequence(u128::MAX - 1));
    let batch = independent_batch(&final_sequence, &hashes);
    let SettlementPlan::IndependentRun(plan) = final_sequence
        .plan_settlement(&batch)
        .expect("one batch may reserve the final available Apply sequence")
    else {
        panic!("fixture is independent");
    };
    drop(apply_plan_for_delta(plan));
    assert_eq!(
        final_sequence.clocks().next_sequence,
        ApplySequence(u128::MAX)
    );
    let template = final_sequence.template_source_versions_for_reference();
    let initial_source = TemplateSelectionSource::from_barrier(ApplySequence(0));
    assert_ne!(template.proposals, initial_source);
    assert_ne!(template.transactions, initial_source);

    let (mut sequence_exhausted, hashes) = independent_fixture(2);
    sequence_exhausted.force_next_sequence(ApplySequence(u128::MAX));
    let before = sequence_exhausted.normalized_snapshot();
    let batch = independent_batch(&sequence_exhausted, &hashes);
    assert_eq!(
        sequence_exhausted.plan_settlement(&batch).err(),
        Some(PlanError::Fault(AuthorityFault::CounterExhausted))
    );
    assert_eq!(sequence_exhausted.normalized_snapshot(), before);
    assert_resource_reference(&sequence_exhausted);

    let (mut version_exhausted, hashes) = independent_fixture(2);
    version_exhausted.force_next_version(EntryVersion(u128::MAX - 1));
    let before = version_exhausted.normalized_snapshot();
    let batch = independent_batch(&version_exhausted, &hashes);
    assert_eq!(
        version_exhausted.plan_settlement(&batch).err(),
        Some(PlanError::Fault(AuthorityFault::CounterExhausted))
    );
    assert_eq!(version_exhausted.normalized_snapshot(), before);
    assert_resource_reference(&version_exhausted);
}

#[test]
fn uak_shared_independent_apply_rechecks_owner_versions_before_mutation() {
    let (authority, hashes) = independent_fixture(1);
    let hash = hashes[0].clone();
    let before = authority.effect_observation_for_foundation();
    let batch = independent_batch(&authority, &hashes);
    let first = authority
        .compile_shared_independent_settlement(&batch)
        .expect("the first pure Accepted transition plans")
        .into_option_for_foundation()
        .expect("the first pure Accepted transition reserves a staged effect");
    let second = authority
        .compile_shared_independent_settlement(&batch)
        .expect("the competing pure Accepted transition plans")
        .into_option_for_foundation()
        .expect("the competing transition reserves a later staged effect");
    assert!(second.scheduler_prestate_is_fresh_for_foundation(&authority));
    let first = match first.bind(&authority) {
        Ok(plan) => plan,
        Err(_) => panic!("the first compiled generation remains current"),
    };
    let staged = authority.effect_observation_for_foundation();
    assert_eq!(staged.queued, before.queued);
    assert!(staged.total_usage.batches > before.total_usage.batches);

    let _committed = first.apply().expect("the first exact version commits");
    let after_first = authority.effect_observation_for_foundation();
    assert!(
        !second.scheduler_prestate_is_fresh_for_foundation(&authority),
        "the first Apply removes the exact Ready slot captured by the second"
    );
    let second = match second.bind(&authority) {
        Ok(plan) => plan,
        Err(_) => panic!("the second compiled generation remains current"),
    };
    assert!(second.apply().is_err());

    let after = authority.effect_observation_for_foundation();
    assert_eq!(after.queued, after_first.queued);
    assert_eq!(
        after.total_usage.batches.checked_add(1),
        Some(after_first.total_usage.batches)
    );
    assert!(after.total_usage.bytes < after_first.total_usage.bytes);
    assert!(matches!(authority.entry(&hash), Some(OwnedTx::Accepted(_))));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_multi_member_stale_middle_rolls_back_the_whole_reserved_delta() {
    let (authority, hashes) = independent_fixture(3);
    let batch = independent_batch(&authority, &hashes);
    let aggregate = authority
        .compile_shared_independent_settlement(&batch)
        .expect("the three-member batch compiles")
        .into_option_for_foundation()
        .expect("the three members share one mechanical delta");
    let middle_batch = independent_batch(&authority, &hashes[1..2]);
    let middle = authority
        .compile_shared_independent_settlement(&middle_batch)
        .expect("the interposed middle member compiles")
        .into_option_for_foundation()
        .expect("the middle member has one exact cut");
    drop(
        middle
            .bind(&authority)
            .unwrap_or_else(|_| panic!("generation remains current"))
            .apply()
            .expect("the middle member commits first"),
    );
    assert!(authority.has_reserved_resource_capacity_for_foundation());
    let effect_before = authority.effect_observation_for_foundation();
    let prepared = aggregate
        .bind(&authority)
        .unwrap_or_else(|_| panic!("generation remains current"));
    assert!(prepared.apply().is_err());
    let effect_after = authority.effect_observation_for_foundation();
    assert_eq!(effect_before.queued.len(), 0);
    assert_eq!(effect_after.queued.len(), 1);
    assert_eq!(
        effect_after.total_usage.batches.checked_add(1),
        Some(effect_before.total_usage.batches),
        "the stale aggregate rolls back exactly its one hidden effect batch"
    );
    assert!(!authority.has_reserved_resource_capacity_for_foundation());
    assert!(matches!(
        authority.entry(&hashes[1]),
        Some(OwnedTx::Accepted(_))
    ));
    for hash in [&hashes[0], &hashes[2]] {
        assert!(matches!(
            authority.entry(hash),
            Some(OwnedTx::PreAccepted(entry)) if matches!(entry.phase, PreAcceptedPhase::Ready(_))
        ));
    }
    assert_eq!(authority.ready_reserved_len_for_foundation(), 0);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_independent_apply_rechecks_resource_rows_before_mutation() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let peer = 240usize;
    let shared_chain_dependency = OutPoint::new(Byte32::new([239; 32]), 0);
    let mut hashes = Vec::new();
    for index in 0..2u8 {
        let input = OutPoint::new(Byte32::new([240 + index; 32]), 0);
        let transaction = TransactionBuilder::default()
            .version(240 + u32::from(index))
            .input(CellInput::new(input.clone(), 0))
            .build();
        let payload = resolved_payload_with_facts(
            &transaction,
            vec![shared_chain_dependency.clone()],
            vec![input],
            Capacity::shannons(1_000 * (u64::from(index) + 1)),
        );
        hashes.push(verify_remote_transaction_with_payload(
            &mut authority,
            transaction,
            peer,
            payload,
        ));
    }

    let first_batch = independent_batch(&authority, &hashes[0..1]);
    let second_batch = independent_batch(&authority, &hashes[1..2]);
    let first = authority
        .compile_shared_independent_settlement(&first_batch)
        .expect("the first same-peer transition plans")
        .into_option_for_foundation()
        .expect("the first same-peer transition stages its effect");
    let second = authority
        .compile_shared_independent_settlement(&second_batch)
        .expect("the second same-peer transition plans from the same cut")
        .into_option_for_foundation()
        .expect("the second same-peer transition stages its effect");
    assert!(second.index_prestate_is_fresh_for_foundation(&authority));
    let first = first
        .bind(&authority)
        .unwrap_or_else(|_| panic!("the first compiled generation remains current"));

    let _committed = first
        .apply()
        .expect("the first same-peer resource row commits");
    assert!(
        !second.resource_prestate_is_fresh_for_foundation(&authority),
        "the first acceptance changes the peer-resource row captured by the second"
    );
    let second = second
        .bind(&authority)
        .unwrap_or_else(|_| panic!("the second compiled generation remains current"));
    assert!(
        second.apply().is_err(),
        "the stale absolute peer-resource target must be rejected before owner mutation"
    );
    assert!(matches!(
        authority.entry(&hashes[0]),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(matches!(
        authority.entry(&hashes[1]),
        Some(OwnedTx::PreAccepted(_))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_ready_apply_rechecks_membership_rows_before_mutation() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let shared_input = OutPoint::new(Byte32::new([242; 32]), 0);
    let mut hashes = Vec::new();
    for seed in 242..=243u8 {
        let transaction = TransactionBuilder::default()
            .version(u32::from(seed))
            .input(CellInput::new(shared_input.clone(), 0))
            .build();
        hashes.push(verify_remote_transaction_with_payload(
            &mut authority,
            transaction.clone(),
            usize::from(seed),
            resolved_payload_with_facts(
                &transaction,
                Vec::new(),
                vec![shared_input.clone()],
                Capacity::shannons(1_000),
            ),
        ));
    }
    let first_batch = independent_batch(&authority, &hashes[0..1]);
    let second_batch = independent_batch(&authority, &hashes[1..2]);
    let first = authority
        .compile_shared_independent_settlement(&first_batch)
        .expect("the first singleton plans")
        .into_option_for_foundation()
        .expect("the first singleton is independently commit-capable");
    let second = authority
        .compile_shared_independent_settlement(&second_batch)
        .expect("the second singleton plans from the same membership cut")
        .into_option_for_foundation()
        .expect("the second singleton is independently commit-capable");
    assert!(second.membership_prestate_is_fresh_for_foundation(&authority));
    let first = first
        .bind(&authority)
        .unwrap_or_else(|_| panic!("the authority generation remains current"));
    drop(
        first
            .apply()
            .expect("the first spender wins the shared input"),
    );
    assert!(
        !second.membership_prestate_is_fresh_for_foundation(&authority),
        "the first Apply changes the exact spender prestate captured by the second"
    );
    let second = second
        .bind(&authority)
        .unwrap_or_else(|_| panic!("the authority generation remains current"));
    assert!(
        second.apply().is_err(),
        "the losing shared-input plan must reject before owner mutation"
    );
    assert!(matches!(
        authority.entry(&hashes[0]),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(matches!(
        authority.entry(&hashes[1]),
        Some(OwnedTx::PreAccepted(_))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_ready_apply_rechecks_dependency_rows_before_mutation() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let input = OutPoint::new(Byte32::new([244; 32]), 0);
    let transaction = TransactionBuilder::default()
        .version(244u32)
        .input(CellInput::new(input.clone(), 0))
        .build();
    let hash = verify_remote_transaction_with_payload(
        &mut authority,
        transaction.clone(),
        244,
        resolved_payload_with_facts(
            &transaction,
            Vec::new(),
            vec![input.clone()],
            Capacity::shannons(1_000),
        ),
    );
    let batch = independent_batch(&authority, std::slice::from_ref(&hash));
    let compiled = authority
        .compile_shared_independent_settlement(&batch)
        .expect("the singleton Ready transition plans")
        .into_option_for_foundation()
        .expect("the singleton is independently commit-capable");
    assert!(compiled.dependency_prestate_is_fresh_for_foundation(&authority));

    let loss = authority
        .plan_dependency_loss_for_foundation(vec![DependencyKey::Cell(input)])
        .expect("the definitive loss plans")
        .expect("the Ready owner is an indexed consumer");
    apply_plan(loss);
    assert!(
        !compiled.dependency_prestate_is_fresh_for_foundation(&authority),
        "the loss event changes the exact dependency level captured by Ready"
    );
    let prepared = compiled
        .bind(&authority)
        .unwrap_or_else(|_| panic!("the authority generation remains current"));
    assert!(
        prepared.apply().is_err(),
        "a dependency event after Plan must reject before owner mutation"
    );
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(_))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_two_disjoint_shared_ready_applies_overlap_inside_their_complete_physical_cuts() {
    const CUT_ENTRY_TIMEOUT: Duration = Duration::from_secs(5);
    let (authority, hashes) = independent_fixture(8);
    let mut selected = None;
    'left: for left_index in 0..hashes.len() {
        let left_batch = independent_batch(&authority, &hashes[left_index..=left_index]);
        let Some(left) = authority
            .compile_shared_independent_settlement(&left_batch)
            .expect("a singleton pure Ready candidate compiles")
            .into_option_for_foundation()
        else {
            continue;
        };
        let left_support = left.physical_write_support_for_foundation(&authority);
        let left_reads = left.physical_read_support_for_foundation(&authority);
        let left_stage = left.dependency_stage_write_support_for_foundation(&authority);
        assert!(left.dependency_ready_phase_shape_for_foundation());
        for right_index in (left_index + 1)..hashes.len() {
            let right_batch = independent_batch(&authority, &hashes[right_index..=right_index]);
            let Some(right) = authority
                .compile_shared_independent_settlement(&right_batch)
                .expect("a later singleton pure Ready candidate compiles")
                .into_option_for_foundation()
            else {
                continue;
            };
            let right_support = right.physical_write_support_for_foundation(&authority);
            let right_reads = right.physical_read_support_for_foundation(&authority);
            let right_stage = right.dependency_stage_write_support_for_foundation(&authority);
            assert!(right.dependency_ready_phase_shape_for_foundation());
            if left_support.is_disjoint(right_support)
                && left_reads.is_disjoint_from_writes(right_support)
                && right_reads.is_disjoint_from_writes(left_support)
            {
                selected = Some((
                    left_index,
                    right_index,
                    left,
                    right,
                    left_reads,
                    right_reads,
                    left_stage,
                    right_stage,
                ));
                break 'left;
            }
            drop(right);
        }
        drop(left);
    }
    let (left_index, right_index, left, right, left_reads, right_reads, left_stage, right_stage) =
        selected.expect("the fixed layout contains two complete compatible mixed cuts");
    assert!(
        left_reads.is_disjoint_from_writes(right_stage)
            && right_reads.is_disjoint_from_writes(left_stage),
        "the compiled final support omitted a dependency stage/final collision: left_reads={:#018x}, right_reads={:#018x}, left_stage={:#018x}, right_stage={:#018x}",
        left_reads.mask_for_foundation(),
        right_reads.mask_for_foundation(),
        left_stage.mask_for_foundation(),
        right_stage.mask_for_foundation(),
    );
    let left_reservation =
        authority.reserve_ready_exact_for_foundation(&hashes[left_index..=left_index]);
    let right_reservation =
        authority.reserve_ready_exact_for_foundation(&hashes[right_index..=right_index]);
    let remaining = hashes
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != left_index && *index != right_index)
        .map(|(_, hash)| hash.clone())
        .collect::<Vec<_>>();
    let _remaining_reservation =
        (!remaining.is_empty()).then(|| authority.reserve_ready_exact_for_foundation(&remaining));
    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    authority
        .entries_for_reference()
        .set_concurrent_removal_probe(Some(probe));
    let left = match left.bind(&authority) {
        Ok(plan) => plan,
        Err(_) => panic!("the left compiled generation remains current"),
    };
    let right = match right.bind(&authority) {
        Ok(plan) => plan,
        Err(_) => panic!("the right compiled generation remains current"),
    };

    let (first_entered, second_entered, committed) = std::thread::scope(|scope| {
        let left = scope.spawn(move || left.apply_reserved(left_reservation));
        let first_entered = entered.recv_timeout(CUT_ENTRY_TIMEOUT);
        let right = scope.spawn(move || right.apply_reserved(right_reservation));
        let second_entered = entered.recv_timeout(CUT_ENTRY_TIMEOUT);
        let _ = release.send(());
        let _ = release.send(());
        let committed = [
            left.join().expect("left shared Apply thread joins"),
            right.join().expect("right shared Apply thread joins"),
        ];
        (first_entered, second_entered, committed)
    });
    let commit_errors = committed
        .iter()
        .map(|result| result.as_ref().err())
        .collect::<Vec<_>>();
    assert!(
        first_entered.is_ok(),
        "the first complete shared cut must become live: errors={commit_errors:?}"
    );
    assert!(
        second_entered.is_ok(),
        "the disjoint second cut must enter before the first releases: errors={commit_errors:?}"
    );
    assert!(committed.into_iter().all(|result| result.is_ok()));
    authority
        .entries_for_reference()
        .set_concurrent_removal_probe(None);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_reverse_shared_commits_change_the_exact_template_source_after_each_owner_commit() {
    let (authority, hashes) = independent_fixture(8);
    let mut selected = None;
    'left: for left_index in 0..hashes.len() {
        let left_batch = independent_batch(&authority, &hashes[left_index..=left_index]);
        let Some(left) = authority
            .compile_shared_independent_settlement(&left_batch)
            .expect("a singleton pure Ready candidate compiles")
            .into_option_for_foundation()
        else {
            continue;
        };
        for right_index in (left_index + 1)..hashes.len() {
            let right_batch = independent_batch(&authority, &hashes[right_index..=right_index]);
            let Some(right) = authority
                .compile_shared_independent_settlement(&right_batch)
                .expect("a later singleton pure Ready candidate compiles")
                .into_option_for_foundation()
            else {
                continue;
            };
            if left
                .physical_apply_support_for_foundation()
                .is_compatible(right.physical_apply_support_for_foundation())
            {
                selected = Some((left_index, right_index, left, right));
                break 'left;
            }
            drop(right);
        }
        drop(left);
    }
    let (left_index, right_index, left, right) =
        selected.expect("the fixed layout contains two compatible exact cuts");
    let left_reservation =
        authority.reserve_ready_exact_for_foundation(&hashes[left_index..=left_index]);
    let right_reservation =
        authority.reserve_ready_exact_for_foundation(&hashes[right_index..=right_index]);
    let remaining = hashes
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != left_index && *index != right_index)
        .map(|(_, hash)| hash.clone())
        .collect::<Vec<_>>();
    let _remaining_reservation =
        (!remaining.is_empty()).then(|| authority.reserve_ready_exact_for_foundation(&remaining));

    let right_committed = right
        .bind(&authority)
        .expect("the later plan binds to the current generation")
        .apply_reserved(right_reservation)
        .expect("the later plan commits first");
    let after_later = authority.template_source_versions_for_reference();
    let left_committed = left
        .bind(&authority)
        .expect("the earlier plan still binds to the current generation")
        .apply_reserved(left_reservation)
        .expect("the earlier plan commits second");
    let after_earlier = authority.template_source_versions_for_reference();

    assert_ne!(
        after_later, after_earlier,
        "actual reverse owner commits must never share one template source identity"
    );
    let _committed = (right_committed, left_committed);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_cell_dependency_phase_transitions_are_real_and_final_read_only() {
    let (authority, hashes) = independent_fixture(2);
    let left = authority
        .compile_shared_independent_settlement(&independent_batch(&authority, &hashes[0..=0]))
        .expect("the first shared Ready transition compiles")
        .into_option_for_foundation()
        .expect("the first shared Ready transition is independent");
    let right = authority
        .compile_shared_independent_settlement(&independent_batch(&authority, &hashes[1..=1]))
        .expect("the second shared Ready transition compiles")
        .into_option_for_foundation()
        .expect("the second shared Ready transition is independent");

    assert!(left.dependency_phase_transition_is_staged_for_foundation(&authority));
    assert!(right.dependency_phase_transition_is_staged_for_foundation(&authority));
    let (left_reads, left_writes) = left.dependency_final_support_masks_for_foundation(&authority);
    let (right_reads, right_writes) =
        right.dependency_final_support_masks_for_foundation(&authority);
    assert!(
        left_reads & right_reads != 0 && left_writes == 0 && right_writes == 0,
        "the shared cell-dependency row is exact read support in both final cuts, never a final write collision: left=({left_reads}, {left_writes}), right=({right_reads}, {right_writes})",
    );
}

#[test]
fn uak_independent_classifier_routes_pairwise_edges_without_mutation() {
    let shared_input = OutPoint::new(Byte32::new([211; 32]), 0);
    let left_tx = TransactionBuilder::default()
        .version(211u32)
        .input(CellInput::new(shared_input.clone(), 0))
        .build();
    let right_tx = TransactionBuilder::default()
        .version(212u32)
        .input(CellInput::new(shared_input.clone(), 0))
        .build();
    let mut conflicts = TxPoolAuthority::for_foundation(limits());
    let left = verify_remote_transaction_with_payload(
        &mut conflicts,
        left_tx.clone(),
        211,
        resolved_payload_with_facts(
            &left_tx,
            Vec::new(),
            vec![shared_input.clone()],
            Capacity::shannons(1_000),
        ),
    );
    let right = verify_remote_transaction_with_payload(
        &mut conflicts,
        right_tx.clone(),
        212,
        resolved_payload_with_facts(
            &right_tx,
            Vec::new(),
            vec![shared_input],
            Capacity::shannons(2_000),
        ),
    );
    let before = conflicts.normalized_snapshot();
    let batch = independent_batch(&conflicts, &[left, right]);
    assert_coupled_and_drop(
        conflicts
            .plan_settlement(&batch)
            .expect("classification itself is valid"),
    );
    assert!(
        conflicts
            .normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before, 1, 0, 1),
        "the dropped strongest coupled candidate burns one replacement version and one Apply stamp"
    );

    let spent = OutPoint::new(Byte32::new([213; 32]), 0);
    let independent_input = OutPoint::new(Byte32::new([214; 32]), 0);
    let spender_tx = TransactionBuilder::default()
        .version(213u32)
        .input(CellInput::new(spent.clone(), 0))
        .build();
    let reader_tx = TransactionBuilder::default()
        .version(214u32)
        .input(CellInput::new(independent_input.clone(), 0))
        .build();
    let mut conditional = TxPoolAuthority::for_foundation(limits());
    let spender = verify_remote_transaction_with_payload(
        &mut conditional,
        spender_tx.clone(),
        213,
        resolved_payload_with_facts(
            &spender_tx,
            Vec::new(),
            vec![spent.clone()],
            Capacity::shannons(1_000),
        ),
    );
    let reader = verify_remote_transaction_with_payload(
        &mut conditional,
        reader_tx.clone(),
        214,
        resolved_payload_with_facts(
            &reader_tx,
            vec![spent],
            vec![independent_input],
            Capacity::shannons(2_000),
        ),
    );
    let before = conditional.normalized_snapshot();
    let batch = independent_batch(&conditional, &[spender, reader]);
    assert_coupled_and_drop(
        conditional
            .plan_settlement(&batch)
            .expect("classification itself is valid"),
    );
    assert!(
        conditional
            .normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before, 1, 0, 1),
        "the dropped conditional-edge candidate burns one replacement version and one Apply stamp"
    );
}

#[test]
fn uak_independent_capacity_is_aggregate_and_never_partially_applied() {
    let bounded =
        limits().with_accepted_for_foundation(AcceptedResources::new(1, 64 * 1024, 64 * 1024, 64));
    let mut authority = TxPoolAuthority::for_foundation(bounded);
    let first_tx = tx(215);
    let second_tx = tx(216);
    let first = verify_remote_transaction(&mut authority, first_tx, 215, Vec::new());
    let second = verify_remote_transaction(&mut authority, second_tx, 216, Vec::new());
    let before = authority.normalized_snapshot();
    let batch = independent_batch(&authority, &[first, second]);

    assert_coupled_and_drop(
        authority
            .plan_settlement(&batch)
            .expect("capacity coupling is a normal classification"),
    );
    assert!(
        authority
            .normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before, 1, 0, 1),
        "the dropped strongest capacity candidate burns one replacement version and one Apply stamp"
    );
    assert_resource_reference(&authority);
}

#[test]
fn uak_independent_classifier_routes_every_accepted_relation_without_mutation() {
    let conflicted_input = OutPoint::new(Byte32::new([217; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(217u32)
        .input(CellInput::new(conflicted_input.clone(), 0))
        .build();
    let mut conflict = TxPoolAuthority::for_foundation(limits());
    accept_remote_transaction_with_payload(
        &mut conflict,
        victim_tx.clone(),
        217,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &victim_tx,
            Vec::new(),
            vec![conflicted_input.clone()],
            Capacity::shannons(1_000),
        ),
    );
    let candidate_tx = TransactionBuilder::default()
        .version(218u32)
        .input(CellInput::new(conflicted_input.clone(), 0))
        .build();
    let candidate = verify_remote_transaction_with_payload(
        &mut conflict,
        candidate_tx.clone(),
        218,
        resolved_payload_with_facts(
            &candidate_tx,
            Vec::new(),
            vec![conflicted_input.clone()],
            Capacity::shannons(2_000),
        ),
    );
    let before = conflict.normalized_snapshot();
    let batch = independent_batch(&conflict, &[candidate]);
    let reason = rejected_coupled_reason_and_drop(
        conflict
            .plan_settlement(&batch)
            .expect("final membership rejection is a closed disposition"),
    );
    assert_eq!(reason, MembershipReject::InputConflict(conflicted_input));
    assert!(
        conflict
            .normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before, 0, 0, 1),
        "the dropped conflict rejection burns only its effect Apply stamp"
    );

    let conditional_cell = OutPoint::new(Byte32::new([219; 32]), 0);
    let reader_tx = tx(219);
    let mut conditional = TxPoolAuthority::for_foundation(limits());
    accept_remote_transaction(
        &mut conditional,
        reader_tx,
        219,
        AcceptedStatus::Pending,
        vec![conditional_cell.clone()],
    );
    let spender_tx = TransactionBuilder::default()
        .version(220u32)
        .input(CellInput::new(conditional_cell.clone(), 0))
        .build();
    let spender = verify_remote_transaction_with_payload(
        &mut conditional,
        spender_tx.clone(),
        220,
        resolved_payload_with_facts(
            &spender_tx,
            Vec::new(),
            vec![conditional_cell],
            Capacity::shannons(2_000),
        ),
    );
    let before = conditional.normalized_snapshot();
    let batch = independent_batch(&conditional, &[spender]);
    assert_coupled_and_drop(
        conditional
            .plan_settlement(&batch)
            .expect("accepted conditional edge routes normally"),
    );
    assert!(
        conditional
            .normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before, 1, 0, 1),
        "the dropped conditional-edge candidate burns one replacement version and one Apply stamp"
    );

    let parent_tx = TransactionBuilder::default()
        .version(221u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let mut causal = TxPoolAuthority::for_foundation(limits());
    let parent = accept_remote_transaction(
        &mut causal,
        parent_tx.clone(),
        221,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let child_tx = TransactionBuilder::default()
        .version(222u32)
        .input(CellInput::new(OutPoint::new(parent_tx.hash(), 0), 0))
        .build();
    let child = verify_remote_transaction(&mut causal, child_tx, 222, Vec::new());
    let before = causal.normalized_snapshot();
    let batch = independent_batch(&causal, &[child]);
    assert_coupled_and_drop(
        causal
            .plan_settlement(&batch)
            .expect("pool parent routes normally"),
    );
    assert!(
        causal
            .normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before, 1, 0, 1),
        "the dropped child admission burns one replacement version and one Apply stamp"
    );
    assert!(matches!(causal.entry(&parent), Some(OwnedTx::Accepted(_))));

    let late_parent_tx = TransactionBuilder::default()
        .version(223u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let late_parent_output = OutPoint::new(late_parent_tx.hash(), 0);
    let late_child_tx = TransactionBuilder::default()
        .version(224u32)
        .input(CellInput::new(late_parent_output.clone(), 0))
        .build();
    let mut late = TxPoolAuthority::for_foundation(limits());
    let late_child = accept_remote_transaction_with_payload(
        &mut late,
        late_child_tx.clone(),
        224,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &late_child_tx,
            Vec::new(),
            vec![late_parent_output],
            Capacity::shannons(1_000),
        ),
    );
    let late_parent = verify_remote_transaction(&mut late, late_parent_tx, 223, Vec::new());
    let before = late.normalized_snapshot();
    let batch = independent_batch(&late, &[late_parent]);
    assert_coupled_and_drop(
        late.plan_settlement(&batch)
            .expect("accepted child routes normally"),
    );
    assert!(
        late.normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before, 1, 0, 1),
        "the dropped late-parent admission burns one replacement version and one Apply stamp"
    );
    assert!(matches!(
        late.entry(&late_child),
        Some(OwnedTx::Accepted(_))
    ));
}

#[test]
fn uak_coupled_membership_requires_exact_positive_input_evidence() {
    let missing = OutPoint::new(Byte32::new([238; 32]), 0);
    let missing_tx = TransactionBuilder::default()
        .version(238u32)
        .input(CellInput::new(missing.clone(), 0))
        .build();
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let candidate = verify_remote_transaction_with_payload(
        &mut authority,
        missing_tx.clone(),
        238,
        resolved_payload_with_facts(
            &missing_tx,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(1_000),
        ),
    );
    let before = authority.normalized_snapshot();
    let batch = independent_batch(&authority, &[candidate]);
    let reason = rejected_coupled_reason_and_drop(
        authority
            .plan_settlement(&batch)
            .expect("final membership rejection is a closed disposition"),
    );
    assert_eq!(reason, MembershipReject::MissingInputEvidence(missing));
    assert!(
        authority
            .normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before, 0, 0, 1),
        "the dropped terminal rejection burns only its effect Apply stamp"
    );

    let parent_tx = TransactionBuilder::default()
        .version(239u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let mut authority = TxPoolAuthority::for_foundation(limits());
    accept_remote_transaction(
        &mut authority,
        parent_tx.clone(),
        239,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let nonexistent_output = OutPoint::new(parent_tx.hash(), 1);
    let child_tx = TransactionBuilder::default()
        .version(240u32)
        .input(CellInput::new(nonexistent_output.clone(), 0))
        .build();
    let child = verify_remote_transaction_with_payload(
        &mut authority,
        child_tx.clone(),
        240,
        resolved_payload_with_facts(&child_tx, Vec::new(), Vec::new(), Capacity::shannons(1_000)),
    );
    let before = authority.normalized_snapshot();
    let batch = independent_batch(&authority, &[child]);
    let reason = rejected_coupled_reason_and_drop(
        authority
            .plan_settlement(&batch)
            .expect("final membership rejection is a closed disposition"),
    );
    assert_eq!(
        reason,
        MembershipReject::MissingPoolOutput(nonexistent_output)
    );
    assert!(
        authority
            .normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before, 0, 0, 1),
        "the dropped missing-output rejection burns only its effect Apply stamp"
    );
    assert_membership_reference(&authority);
    assert_resource_reference(&authority);

    let nonexistent_dependency = OutPoint::new(parent_tx.hash(), 2);
    let dependent_tx = TransactionBuilder::default()
        .version(245u32)
        .cell_dep(
            CellDep::new_builder()
                .out_point(nonexistent_dependency.clone())
                .build(),
        )
        .build();
    let dependent = verify_remote_transaction_with_payload(
        &mut authority,
        dependent_tx.clone(),
        245,
        resolved_payload_with_facts(
            &dependent_tx,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(1_000),
        ),
    );
    let before = authority.normalized_snapshot();
    let batch = independent_batch(&authority, &[dependent]);
    let reason = rejected_coupled_reason_and_drop(
        authority
            .plan_settlement(&batch)
            .expect("final membership rejection is a closed disposition"),
    );
    assert_eq!(
        reason,
        MembershipReject::MissingPoolOutput(nonexistent_dependency)
    );
    assert!(
        authority
            .normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before, 0, 0, 1),
        "the dropped missing-dependency rejection burns only its effect Apply stamp"
    );
    assert_membership_reference(&authority);
    assert_resource_reference(&authority);

    // A resolver is not allowed to turn an output owned only by PreAccepted
    // work into final dependency evidence. The final membership proof seals
    // that boundary even if an upstream resolver regresses in the future.
    let preaccepted_parent_tx = TransactionBuilder::default()
        .version(246u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let preaccepted_parent =
        ValidatedAdmission::remote(preaccepted_parent_tx.clone(), PeerIndex::from(246usize))
            .expect("fixture parent admission is valid");
    apply_plan(
        authority
            .plan_admission(preaccepted_parent)
            .expect("fixture parent enters PreAccepted ownership"),
    );
    let unsupported_dependency = OutPoint::new(preaccepted_parent_tx.hash(), 0);
    let unsupported_tx = TransactionBuilder::default()
        .version(247u32)
        .cell_dep(
            CellDep::new_builder()
                .out_point(unsupported_dependency.clone())
                .build(),
        )
        .build();
    let unsupported_bytes = unsupported_tx.data().total_size();
    let unsupported_payload = ResolvedPayload::for_foundation(
        &unsupported_tx,
        Vec::new(),
        64,
        Capacity::shannons(1_000),
        unsupported_bytes,
        Vec::new(),
        Vec::new(),
    )
    .expect("fixture deliberately carries no chain dependency evidence");
    let unsupported = verify_remote_transaction_with_payload(
        &mut authority,
        unsupported_tx,
        247,
        unsupported_payload,
    );
    let before = authority.normalized_snapshot();
    let batch = independent_batch(&authority, &[unsupported]);
    let reason = rejected_coupled_reason_and_drop(
        authority
            .plan_settlement(&batch)
            .expect("final membership rejection is a closed disposition"),
    );
    assert_eq!(
        reason,
        MembershipReject::MissingDependencyEvidence(unsupported_dependency)
    );
    assert!(
        authority
            .normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before, 0, 0, 1),
        "the dropped unsupported-dependency rejection burns only its effect Apply stamp"
    );
    assert_membership_reference(&authority);
    assert_resource_reference(&authority);
}

#[test]
fn uak_coupled_reverse_chain_restores_late_parents_atomically() {
    let grandparent_tx = TransactionBuilder::default()
        .version(225u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let grandparent_output = OutPoint::new(grandparent_tx.hash(), 0);
    let parent_tx = TransactionBuilder::default()
        .version(226u32)
        .input(CellInput::new(grandparent_output.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent_output = OutPoint::new(parent_tx.hash(), 0);
    let child_tx = TransactionBuilder::default()
        .version(227u32)
        .input(CellInput::new(parent_output.clone(), 0))
        .build();
    let mut authority = TxPoolAuthority::for_foundation(limits());

    let child = accept_remote_transaction_with_payload(
        &mut authority,
        child_tx.clone(),
        227,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &child_tx,
            Vec::new(),
            vec![parent_output],
            Capacity::shannons(3_000),
        ),
    );
    let parent = verify_remote_transaction_with_payload(
        &mut authority,
        parent_tx.clone(),
        226,
        resolved_payload_with_facts(
            &parent_tx,
            Vec::new(),
            vec![grandparent_output],
            Capacity::shannons(2_000),
        ),
    );
    let batch = independent_batch(&authority, std::slice::from_ref(&parent));
    let SettlementPlan::CoupledComponent(disposition) = authority
        .plan_settlement(&batch)
        .expect("late parent has one bounded coupled Plan")
    else {
        panic!("late parent must not use IndependentRun");
    };
    let _ = accepted_disposition(disposition.into_disposition()).apply();
    assert_eq!(
        authority.accepted_children(&parent),
        Some(HashSet::from([child.clone()]))
    );
    assert_eq!(
        authority.accepted_parents(&child),
        Some(HashSet::from([parent.clone()]))
    );
    assert_membership_reference(&authority);

    let grandparent = verify_remote_transaction_with_payload(
        &mut authority,
        grandparent_tx.clone(),
        225,
        resolved_payload_with_facts(
            &grandparent_tx,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(1_000),
        ),
    );
    let batch = independent_batch(&authority, std::slice::from_ref(&grandparent));
    let SettlementPlan::CoupledComponent(disposition) = authority
        .plan_settlement(&batch)
        .expect("late grandparent has one bounded coupled Plan")
    else {
        panic!("late grandparent must not use IndependentRun");
    };
    let _ = accepted_disposition(disposition.into_disposition()).apply();
    assert_eq!(
        authority.accepted_children(&grandparent),
        Some(HashSet::from([parent.clone()]))
    );
    assert_eq!(
        authority.accepted_parents(&parent),
        Some(HashSet::from([grandparent]))
    );
    assert_membership_reference(&authority);
}

#[test]
fn uak_coupled_late_parent_deduplicates_an_existing_descendant_path() {
    let ancestor_tx = TransactionBuilder::default()
        .version(228u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let ancestor_input = OutPoint::new(ancestor_tx.hash(), 0);
    let parent_input = OutPoint::new(ancestor_tx.hash(), 1);
    let late_parent_tx = TransactionBuilder::default()
        .version(229u32)
        .input(CellInput::new(parent_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let late_parent_output = OutPoint::new(late_parent_tx.hash(), 0);
    let child_tx = TransactionBuilder::default()
        .version(230u32)
        .input(CellInput::new(ancestor_input.clone(), 0))
        .build();
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let ancestor = accept_remote_transaction(
        &mut authority,
        ancestor_tx,
        228,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let child = accept_remote_transaction_with_payload(
        &mut authority,
        child_tx.clone(),
        230,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &child_tx,
            vec![late_parent_output],
            vec![ancestor_input],
            Capacity::shannons(3_000),
        ),
    );
    let parent = verify_remote_transaction_with_payload(
        &mut authority,
        late_parent_tx.clone(),
        229,
        resolved_payload_with_facts(
            &late_parent_tx,
            Vec::new(),
            vec![parent_input],
            Capacity::shannons(2_000),
        ),
    );
    let batch = independent_batch(&authority, std::slice::from_ref(&parent));
    let SettlementPlan::CoupledComponent(disposition) = authority
        .plan_settlement(&batch)
        .expect("shared descendant path has one bounded coupled Plan")
    else {
        panic!("accepted child must route through the coupled planner");
    };
    let _ = accepted_disposition(disposition.into_disposition()).apply();

    assert_eq!(
        authority.accepted_parents(&child),
        Some(HashSet::from([ancestor.clone(), parent.clone()]))
    );
    assert_eq!(
        authority.accepted_children(&ancestor),
        Some(HashSet::from([child.clone(), parent.clone()]))
    );
    assert_eq!(
        authority.accepted_children(&parent),
        Some(HashSet::from([child]))
    );
    assert_membership_reference(&authority);
}

#[test]
fn uak_causal_diamond_is_projection_equivalent_for_every_arrival_order() {
    let root = TransactionBuilder::default()
        .version(241u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let left = TransactionBuilder::default()
        .version(242u32)
        .input(CellInput::new(OutPoint::new(root.hash(), 0), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let right = TransactionBuilder::default()
        .version(243u32)
        .input(CellInput::new(OutPoint::new(root.hash(), 1), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let leaf = TransactionBuilder::default()
        .version(244u32)
        .input(CellInput::new(OutPoint::new(left.hash(), 0), 0))
        .input(CellInput::new(OutPoint::new(right.hash(), 0), 0))
        .build();
    let transactions = [root, left, right, leaf];
    let permutations = [
        [0, 1, 2, 3],
        [0, 1, 3, 2],
        [0, 2, 1, 3],
        [0, 2, 3, 1],
        [0, 3, 1, 2],
        [0, 3, 2, 1],
        [1, 0, 2, 3],
        [1, 0, 3, 2],
        [1, 2, 0, 3],
        [1, 2, 3, 0],
        [1, 3, 0, 2],
        [1, 3, 2, 0],
        [2, 0, 1, 3],
        [2, 0, 3, 1],
        [2, 1, 0, 3],
        [2, 1, 3, 0],
        [2, 3, 0, 1],
        [2, 3, 1, 0],
        [3, 0, 1, 2],
        [3, 0, 2, 1],
        [3, 1, 0, 2],
        [3, 1, 2, 0],
        [3, 2, 0, 1],
        [3, 2, 1, 0],
    ];

    for order in permutations {
        let mut authority = TxPoolAuthority::for_foundation(limits());
        for index in order {
            let transaction = transactions[index].clone();
            let chain_inputs = transaction.input_pts_iter().collect();
            accept_remote_transaction_with_payload(
                &mut authority,
                transaction.clone(),
                241 + index,
                AcceptedStatus::Pending,
                resolved_payload_with_facts(
                    &transaction,
                    Vec::new(),
                    chain_inputs,
                    Capacity::shannons(1_000 + u64::try_from(index).expect("index fits")),
                ),
            );
            assert_membership_reference(&authority);
            assert_resource_reference(&authority);
        }
    }
}

#[test]
fn uak_coupled_late_parent_capacity_evicts_from_the_projected_graph() {
    let bounded =
        limits().with_accepted_for_foundation(AcceptedResources::new(2, 64 * 1024, 64 * 1024, 64));
    let mut authority = TxPoolAuthority::for_foundation(bounded);
    let parent_tx = TransactionBuilder::default()
        .version(231u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent_output = OutPoint::new(parent_tx.hash(), 0);
    let child_tx = TransactionBuilder::default()
        .version(232u32)
        .input(CellInput::new(parent_output.clone(), 0))
        .build();
    let child = accept_remote_transaction_with_payload(
        &mut authority,
        child_tx.clone(),
        232,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &child_tx,
            Vec::new(),
            vec![parent_output],
            Capacity::shannons(10_000),
        ),
    );
    let unrelated_tx = tx(233);
    let unrelated = accept_remote_transaction_with_payload(
        &mut authority,
        unrelated_tx.clone(),
        233,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(&unrelated_tx, Vec::new(), Vec::new(), Capacity::shannons(1)),
    );
    let parent = verify_remote_transaction_with_payload(
        &mut authority,
        parent_tx.clone(),
        231,
        resolved_payload_with_facts(
            &parent_tx,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(2_000),
        ),
    );
    let batch = independent_batch(&authority, std::slice::from_ref(&parent));
    let head = authority
        .compile_shared_canonical_ready_head(&batch)
        .expect("late parent capacity is planned on one exact 64-shard frontier");
    let (compiled, continuation) = head.into_parts();
    assert!(continuation.is_none());
    let support = compiled.physical_apply_support_for_foundation();
    assert!(
        (0..AUTHORITY_SHARD_COUNT).all(|shard| support.touches_for_foundation(shard)),
        "capacity-frontier absence is one exact fixed 64-shard read premise"
    );
    assert!(compiled.membership_prestate_is_fresh_for_foundation(&authority));
    authority
        .entries_for_reference()
        .advance_membership_order_revision_for_foundation(0);
    assert!(
        !compiled.membership_prestate_is_fresh_for_foundation(&authority),
        "an otherwise-unobserved frontier mutation must stale the capacity decision"
    );
    let _effect_wake = compiled
        .cancel_unassigned_ready_job()
        .expect("the stale pre-owner plan rolls back its sole staged effect");

    let head = authority
        .compile_shared_canonical_ready_head(&batch)
        .expect("the changed frontier recompiles from one new coherent cut");
    let (compiled, continuation) = head.into_parts();
    assert!(continuation.is_none());
    let shared = compiled
        .bind(&authority)
        .and_then(PreparedIndependentApply::apply)
        .expect("the exact capacity frontier remains current");
    let (committed, post_commit_fault) = shared.into_parts();
    assert_eq!(post_commit_fault, None);

    assert_eq!(committed.removals.len(), 1);
    assert_eq!(
        committed.retired_len(),
        committed.removals.len().saturating_add(1)
    );
    assert_eq!(committed.removals[0].hash, unrelated);
    assert_eq!(committed.removals[0].cause, RemovalCause::Capacity);
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(matches!(
        authority.entry(&parent),
        Some(OwnedTx::Accepted(_))
    ));
    assert_membership_reference(&authority);
    assert_resource_reference(&authority);
}

#[test]
fn uak_coupled_capacity_can_remove_a_late_child_without_stale_parent_weight() {
    let bounded =
        limits().with_accepted_for_foundation(AcceptedResources::new(2, 64 * 1024, 64 * 1024, 64));
    let mut authority = TxPoolAuthority::for_foundation(bounded);
    let parent_tx = TransactionBuilder::default()
        .version(235u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent_output = OutPoint::new(parent_tx.hash(), 0);
    let child_tx = TransactionBuilder::default()
        .version(236u32)
        .input(CellInput::new(parent_output.clone(), 0))
        .build();
    let child = accept_remote_transaction_with_payload(
        &mut authority,
        child_tx.clone(),
        236,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &child_tx,
            Vec::new(),
            vec![parent_output],
            Capacity::shannons(1),
        ),
    );
    let unrelated_tx = tx(237);
    let unrelated = accept_remote_transaction_with_payload(
        &mut authority,
        unrelated_tx.clone(),
        237,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &unrelated_tx,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(10_000),
        ),
    );
    let parent = verify_remote_transaction_with_payload(
        &mut authority,
        parent_tx.clone(),
        235,
        resolved_payload_with_facts(
            &parent_tx,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(5_000),
        ),
    );
    let batch = independent_batch(&authority, std::slice::from_ref(&parent));
    let SettlementPlan::CoupledComponent(disposition) = authority
        .plan_settlement(&batch)
        .expect("late-child eviction is compiled before Apply")
    else {
        panic!("accepted child must route through the coupled planner");
    };
    let committed = accepted_disposition(disposition.into_disposition()).apply();

    assert_eq!(committed.removals.len(), 1);
    assert_eq!(
        committed.retired_len(),
        committed.removals.len().saturating_add(1)
    );
    assert_eq!(committed.removals[0].hash, child);
    assert_eq!(committed.removals[0].cause, RemovalCause::Capacity);
    assert!(matches!(
        authority.entry(&parent),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(matches!(
        authority.entry(&unrelated),
        Some(OwnedTx::Accepted(_))
    ));
    assert_eq!(authority.accepted_children(&parent), Some(HashSet::new()));
    assert_membership_reference(&authority);
    assert_resource_reference(&authority);
}

#[test]
fn uak_late_parent_component_bound_fails_before_authority_mutation() {
    let bounded = limits().with_accepted_for_foundation(AcceptedResources::new(
        128,
        1024 * 1024,
        1024 * 1024,
        1024,
    ));
    let mut authority = TxPoolAuthority::for_foundation(bounded);
    let parent_tx = TransactionBuilder::default()
        .version(234u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent_output = OutPoint::new(parent_tx.hash(), 0);
    for nonce in 0..101usize {
        let child_tx = tx(300 + u64::try_from(nonce).expect("fixture nonce fits"));
        accept_remote_transaction_with_payload(
            &mut authority,
            child_tx.clone(),
            300 + nonce,
            AcceptedStatus::Pending,
            resolved_payload_with_facts(
                &child_tx,
                vec![parent_output.clone()],
                Vec::new(),
                Capacity::shannons(1_000),
            ),
        );
    }
    let parent = verify_remote_transaction_with_payload(
        &mut authority,
        parent_tx.clone(),
        234,
        resolved_payload_with_facts(
            &parent_tx,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(2_000),
        ),
    );
    let before = authority.normalized_snapshot();
    let batch = independent_batch(&authority, &[parent]);
    let reason = rejected_coupled_reason_and_drop(
        authority
            .plan_settlement(&batch)
            .expect("final membership rejection is a closed disposition"),
    );
    assert_eq!(
        reason,
        MembershipReject::ComponentLimit {
            kind: ComponentLimitKind::Mutation,
            limit: crate::constants::MAX_POOL_MUTATION_CANDIDATES,
        }
    );
    assert!(
        authority
            .normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before, 0, 0, 1),
        "the dropped component-limit rejection burns only its effect Apply stamp"
    );
    assert_membership_reference(&authority);
    assert_resource_reference(&authority);
}

#[test]
fn uak_nested_late_child_fanout_is_sliced_by_the_same_component_bound() {
    let bounded = limits().with_accepted_for_foundation(AcceptedResources::new(
        128,
        1024 * 1024,
        1024 * 1024,
        1024,
    ));
    let mut authority = TxPoolAuthority::for_foundation(bounded);
    let candidate_tx = TransactionBuilder::default()
        .version(526u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let candidate_output = OutPoint::new(candidate_tx.hash(), 0);
    let root_tx = TransactionBuilder::default()
        .version(527u32)
        .input(CellInput::new(candidate_output.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let root_output = OutPoint::new(root_tx.hash(), 0);
    accept_remote_transaction_with_payload(
        &mut authority,
        root_tx.clone(),
        527,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &root_tx,
            Vec::new(),
            vec![candidate_output],
            Capacity::shannons(1_000),
        ),
    );
    for nonce in 0..100usize {
        let child_tx = tx(600 + u64::try_from(nonce).expect("fixture nonce fits"));
        accept_remote_transaction_with_payload(
            &mut authority,
            child_tx.clone(),
            600 + nonce,
            AcceptedStatus::Pending,
            resolved_payload_with_facts(
                &child_tx,
                vec![root_output.clone()],
                Vec::new(),
                Capacity::shannons(1_000),
            ),
        );
    }
    let candidate = verify_remote_transaction_with_payload(
        &mut authority,
        candidate_tx.clone(),
        526,
        resolved_payload_with_facts(
            &candidate_tx,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(2_000),
        ),
    );
    let before = authority.normalized_snapshot();
    let batch = independent_batch(&authority, &[candidate]);
    let reason = rejected_coupled_reason_and_drop(
        authority
            .plan_settlement(&batch)
            .expect("final membership rejection is a closed disposition"),
    );
    assert_eq!(
        reason,
        MembershipReject::ComponentLimit {
            kind: ComponentLimitKind::Mutation,
            limit: crate::constants::MAX_POOL_MUTATION_CANDIDATES,
        }
    );
    assert!(
        authority
            .normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before, 0, 0, 1),
        "the dropped nested-component rejection burns only its effect Apply stamp"
    );
    assert_membership_reference(&authority);
    assert_resource_reference(&authority);
}

#[test]
fn uak_late_parent_cannot_bypass_the_descendant_ancestor_bound() {
    let bounded = limits().with_accepted_for_foundation(AcceptedResources::new(
        130,
        8 * 1024 * 1024,
        8 * 1024 * 1024,
        1024,
    ));
    let mut authority = TxPoolAuthority::for_foundation(bounded);
    let late_parent_tx = TransactionBuilder::default()
        .version(400u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let late_parent_output = OutPoint::new(late_parent_tx.hash(), 0);
    let root_tx = TransactionBuilder::default()
        .version(401u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    accept_remote_transaction(
        &mut authority,
        root_tx.clone(),
        401,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let mut previous = root_tx;
    for version in 402u32..=524u32 {
        let next = TransactionBuilder::default()
            .version(version)
            .input(CellInput::new(OutPoint::new(previous.hash(), 0), 0))
            .output(CellOutput::default())
            .output_data(Bytes::new().pack())
            .build();
        accept_remote_transaction(
            &mut authority,
            next.clone(),
            usize::try_from(version).expect("fixture peer index fits"),
            AcceptedStatus::Pending,
            Vec::new(),
        );
        previous = next;
    }
    let descendant_tx = TransactionBuilder::default()
        .version(525u32)
        .input(CellInput::new(OutPoint::new(previous.hash(), 0), 0))
        .build();
    accept_remote_transaction_with_payload(
        &mut authority,
        descendant_tx.clone(),
        525,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &descendant_tx,
            vec![late_parent_output],
            Vec::new(),
            Capacity::shannons(1_000),
        ),
    );
    let late_parent = verify_remote_transaction_with_payload(
        &mut authority,
        late_parent_tx.clone(),
        400,
        resolved_payload_with_facts(
            &late_parent_tx,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(2_000),
        ),
    );
    let before = authority.normalized_snapshot();
    let batch = independent_batch(&authority, &[late_parent]);
    let reason = rejected_coupled_reason_and_drop(
        authority
            .plan_settlement(&batch)
            .expect("final membership rejection is a closed disposition"),
    );
    assert_eq!(reason, MembershipReject::TooManyAncestors);
    assert!(
        authority
            .normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before, 0, 0, 1),
        "the dropped ancestor-bound rejection burns only its effect Apply stamp"
    );
    assert_membership_reference(&authority);
    assert_resource_reference(&authority);
}

#[test]
fn uak_develop_stale_parent_eviction_shape_is_total_rejection_without_mutation() {
    let bounded =
        limits().with_accepted_for_foundation(AcceptedResources::new(4, 64 * 1024, 64 * 1024, 64));
    let mut authority = TxPoolAuthority::with_max_ancestors_for_foundation(bounded, 2);
    let dependency = OutPoint::new(Byte32::new([230; 32]), 0);

    let reader_tx = TransactionBuilder::default()
        .version(1_330u32)
        .cell_dep(CellDep::new_builder().out_point(dependency.clone()).build())
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let reader = accept_remote_transaction_with_payload(
        &mut authority,
        reader_tx.clone(),
        1_330,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(&reader_tx, Vec::new(), Vec::new(), Capacity::shannons(100)),
    );

    let descendant_tx = TransactionBuilder::default()
        .version(1_331u32)
        .input(CellInput::new(OutPoint::new(reader_tx.hash(), 0), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let descendant = accept_remote_transaction_with_payload(
        &mut authority,
        descendant_tx.clone(),
        1_331,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &descendant_tx,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(100),
        ),
    );

    // This is the exact legal shape that made develop's legacy pool_map evict
    // the dependency reader and its descendant, then continue from a stale
    // parent snapshot. V1 has no partial-eviction transition: the candidate
    // still has both ancestors, so the total read-only Plan rejects it and
    // leaves the coherent authority cut unchanged.
    let candidate_tx = TransactionBuilder::default()
        .version(1_332u32)
        .input(CellInput::new(dependency.clone(), 0))
        .input(CellInput::new(OutPoint::new(descendant_tx.hash(), 0), 0))
        .build();
    let candidate = verify_remote_transaction_with_payload(
        &mut authority,
        candidate_tx.clone(),
        1_332,
        resolved_payload_with_facts(
            &candidate_tx,
            Vec::new(),
            vec![dependency],
            Capacity::shannons(10_000),
        ),
    );
    let before = authority.normalized_snapshot();
    let batch = independent_batch(&authority, &[candidate]);
    let reason = rejected_coupled_reason_and_drop(
        authority
            .plan_settlement(&batch)
            .expect("the legacy stale-parent shape has one total disposition"),
    );

    assert_eq!(reason, MembershipReject::TooManyAncestors);
    assert!(
        authority
            .normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before, 0, 0, 1),
        "the dropped stale-parent-shape rejection burns only its effect Apply stamp"
    );
    assert!(matches!(
        authority.entry(&reader),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(matches!(
        authority.entry(&descendant),
        Some(OwnedTx::Accepted(_))
    ));
    assert_membership_reference(&authority);
    assert_resource_reference(&authority);
}

#[test]
fn uak_capacity_self_eviction_is_precomputed_and_mutation_free() {
    let bounded =
        limits().with_accepted_for_foundation(AcceptedResources::new(1, 64 * 1024, 64 * 1024, 64));
    let mut authority = TxPoolAuthority::for_foundation(bounded);
    let first_tx = tx(42);
    let first_payload = resolved_payload_with_facts(
        &first_tx,
        Vec::new(),
        Vec::new(),
        Capacity::shannons(10_000),
    );
    let first = accept_remote_transaction_with_payload(
        &mut authority,
        first_tx,
        54,
        AcceptedStatus::Pending,
        first_payload,
    );
    let second = verify_remote_transaction(&mut authority, tx(43), 55, Vec::new());
    let before = authority.normalized_snapshot();
    let version = owner_version(&authority, &second);

    assert!(matches!(
        authority
            .plan_accept_for_foundation(&second, version, AcceptedStatus::Pending)
            .err(),
        Some(PlanError::Membership(
            MembershipReject::CandidateEvicted { .. }
        ))
    ));
    assert_eq!(authority.normalized_snapshot(), before);
    assert!(matches!(
        authority.entry(&first),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(matches!(
        authority.entry(&second),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Ready(_))
    ));
    assert_resource_reference(&authority);
}

#[test]
fn uak_capacity_eviction_removes_one_complete_causal_component() {
    let bounded =
        limits().with_accepted_for_foundation(AcceptedResources::new(2, 64 * 1024, 64 * 1024, 64));
    let mut authority = TxPoolAuthority::for_foundation(bounded);
    let root_tx = TransactionBuilder::default()
        .version(67u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let root = accept_remote_transaction(
        &mut authority,
        root_tx.clone(),
        67,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let child_tx = TransactionBuilder::default()
        .version(68u32)
        .input(CellInput::new(OutPoint::new(root_tx.hash(), 0), 0))
        .build();
    let child = accept_remote_transaction(
        &mut authority,
        child_tx,
        68,
        AcceptedStatus::Proposed,
        Vec::new(),
    );
    let candidate_tx = tx(69);
    let candidate_payload = resolved_payload_with_facts(
        &candidate_tx,
        Vec::new(),
        Vec::new(),
        Capacity::shannons(10_000),
    );
    let candidate =
        verify_remote_transaction_with_payload(&mut authority, candidate_tx, 69, candidate_payload);
    let expected_eviction_rates = [&root, &child]
        .into_iter()
        .map(|hash| {
            let Some(OwnedTx::Accepted(entry)) = authority.entry(hash) else {
                panic!("capacity fixture victim must be Accepted");
            };
            let cost = entry.proof.metrics().cost;
            (
                hash.clone(),
                FeeRate::calculate(
                    entry.proof.metrics().fee,
                    get_transaction_weight(cost.serialized_bytes, cost.cycles),
                ),
            )
        })
        .collect::<HashMap<_, _>>();
    let version = owner_version(&authority, &candidate);
    let committed = apply_plan_for_delta(
        authority
            .plan_accept_for_foundation(&candidate, version, AcceptedStatus::Pending)
            .expect("higher-fee candidate atomically evicts a closed component"),
    );
    assert_eq!(committed.removals.len(), 2);
    assert_eq!(
        committed.retired_len(),
        committed.removals.len().saturating_add(1)
    );
    assert!(
        committed
            .removals
            .iter()
            .all(|removal| removal.cause == RemovalCause::Capacity)
    );

    assert!(authority.entry(&root).is_none());
    assert!(authority.entry(&child).is_none());
    assert!(matches!(
        authority.entry(&candidate),
        Some(OwnedTx::Accepted(_))
    ));
    assert_eq!(authority.resources().accepted().entries, 1);
    assert_resource_reference(&authority);

    let lease = authority
        .effect_publication_receipt_for_foundation()
        .expect("membership Apply commits one complete outcome batch");
    let [
        CommittedEffect::Accepted(CommittedAcceptance::Admission {
            entry,
            status: AcceptedStatus::Pending,
            ingress_peer: Some(peer),
        }),
        rejected @ ..,
    ] = lease.effects()
    else {
        panic!("capacity Apply must publish admission before exact victim outcomes");
    };
    assert_eq!(entry.tx.hash(), candidate.0);
    assert_eq!(entry.ancestors_count, 1);
    assert_eq!(entry.descendants_count, 1);
    assert_eq!(*peer, PeerIndex::from(69));
    assert_eq!(rejected.len(), expected_eviction_rates.len());
    for effect in rejected {
        let CommittedEffect::Rejected(CommittedRejection::CapacityEvicted {
            entry, fee_rate, ..
        }) = effect
        else {
            panic!("every capacity victim retains its exact eviction evidence");
        };
        let hash = RawTxHash(entry.tx.hash());
        assert!(entry.ancestors_count >= 1);
        assert!(entry.descendants_count >= 1);
        assert_eq!(Some(fee_rate), expected_eviction_rates.get(&hash));
    }
    let published = authority
        .apply_effect_settlement_for_foundation(lease.complete_for_foundation().published())
        .expect("capacity outcome publication settles");
    assert_eq!(published.retired_effect_len(), 1);
}

#[test]
fn uak_input_conflict_failure_is_precomputed_and_mutation_free() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let chain_input = OutPoint::new(Byte32::new([44; 32]), 0);
    let first_tx = TransactionBuilder::default()
        .version(44u32)
        .input(CellInput::new(chain_input.clone(), 0))
        .build();
    accept_remote_transaction_with_payload(
        &mut authority,
        first_tx.clone(),
        56,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &first_tx,
            Vec::new(),
            vec![chain_input.clone()],
            Capacity::shannons(1_000),
        ),
    );
    let second_tx = TransactionBuilder::default()
        .version(45u32)
        .input(CellInput::new(chain_input.clone(), 0))
        .build();
    let second = verify_remote_transaction_with_payload(
        &mut authority,
        second_tx.clone(),
        57,
        resolved_payload_with_facts(
            &second_tx,
            Vec::new(),
            vec![chain_input.clone()],
            Capacity::shannons(2_000),
        ),
    );
    let before = authority.normalized_snapshot();
    let version = owner_version(&authority, &second);

    assert_eq!(
        authority
            .plan_accept_for_foundation(&second, version, AcceptedStatus::Pending)
            .err(),
        Some(PlanError::Membership(MembershipReject::InputConflict(
            chain_input
        )))
    );
    assert_eq!(authority.normalized_snapshot(), before);
    assert!(matches!(
        authority.entry(&second),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Ready(_))
    ));
    assert_resource_reference(&authority);
}

#[test]
fn uak_rbf_replaces_the_complete_descendant_closure_atomically() {
    let mut authority = TxPoolAuthority::with_replacement(limits(), FeeRate::from_u64(1_000));
    let chain_input = OutPoint::new(Byte32::new([58; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(58u32)
        .input(CellInput::new(chain_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let victim_payload = resolved_payload_with_facts(
        &victim_tx,
        Vec::new(),
        vec![chain_input.clone()],
        Capacity::shannons(1_000),
    );
    let victim = accept_remote_transaction_with_payload(
        &mut authority,
        victim_tx.clone(),
        58,
        AcceptedStatus::Pending,
        victim_payload,
    );
    let victim_output = OutPoint::new(victim_tx.hash(), 0);
    let child_tx = TransactionBuilder::default()
        .version(59u32)
        .input(CellInput::new(victim_output, 0))
        .build();
    let child_payload =
        resolved_payload_with_facts(&child_tx, Vec::new(), Vec::new(), Capacity::shannons(500));
    let child = accept_remote_transaction_with_payload(
        &mut authority,
        child_tx,
        59,
        AcceptedStatus::Proposed,
        child_payload,
    );

    let replacement_tx = TransactionBuilder::default()
        .version(60u32)
        .input(CellInput::new(chain_input.clone(), 0))
        .build();
    let replacement_payload = resolved_payload_with_facts(
        &replacement_tx,
        Vec::new(),
        vec![chain_input.clone()],
        Capacity::shannons(10_000),
    );
    let replacement = verify_remote_transaction_with_payload(
        &mut authority,
        replacement_tx,
        60,
        replacement_payload,
    );
    let version = owner_version(&authority, &replacement);
    let disposition = authority
        .plan_candidate_disposition_for_foundation(&replacement, version, AcceptedStatus::Pending)
        .expect("complete replacement closure has one deterministic disposition");
    let CandidateDispositionPlan::Accepted(plan) = disposition else {
        panic!("a sufficiently funded replacement must be accepted");
    };
    let committed = apply_plan_for_delta(plan);
    assert_eq!(committed.removals.len(), 2);
    assert_eq!(
        committed.retired_len(),
        committed.removals.len().saturating_add(1)
    );
    assert!(
        committed
            .removals
            .iter()
            .all(|removal| removal.cause == RemovalCause::Replacement)
    );

    assert!(matches!(
        authority.entry(&victim),
        Some(OwnedTx::ReplacementHistory(_))
    ));
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::ReplacementHistory(_))
    ));
    assert!(matches!(
        authority.entry(&replacement),
        Some(OwnedTx::Accepted(_))
    ));
    assert_eq!(
        authority.accepted_spender(&chain_input),
        Some(replacement.clone())
    );
    assert_eq!(authority.owner_count(), 3);
    assert_eq!(authority.resources().accepted().entries, 1);
    assert_eq!(authority.resources().replacement_history().entries, 2);

    let view = authority.read_view();
    let mut expected_history = vec![victim.clone(), child.clone()];
    expected_history.sort_unstable();
    assert_eq!(
        view.replacement_history_hashes()
            .expect("replacement history has one coherent read projection"),
        expected_history
    );
    let ids = view.pool_ids().expect("accepted pool ids remain coherent");
    assert_eq!(ids.pending, vec![replacement.clone()]);
    assert!(ids.proposed.is_empty());
    let summary = view.summary().expect("history has an explicit read state");
    assert_eq!(summary.owners, 3);
    assert_eq!(summary.replacement_history, 2);
    let snapshot = genesis_snapshot();
    for hash in [&victim, &child] {
        assert!(
            view.entry_by_raw(hash)
                .expect("replacement history remains internally queryable")
                .rpc_status(&snapshot)
                .is_none(),
            "replacement history must not acquire a live-pool RPC status"
        );
    }
    assert_eq!(summary.accepted_pending, 1);
    assert_eq!(
        view.capture_persistence()
            .expect("history is excluded from restart ownership")
            .selected_len(),
        1
    );
    assert_eq!(
        view.capture_template()
            .expect("history is excluded from template ownership")
            .selected_len(),
        1
    );
    drop(view);
    assert_resource_reference(&authority);

    let lease = authority
        .effect_publication_receipt_for_foundation()
        .expect("replacement Apply commits one complete outcome batch");
    let [
        CommittedEffect::Accepted(CommittedAcceptance::Admission {
            entry,
            status: AcceptedStatus::Pending,
            ingress_peer: Some(peer),
        }),
        rejected @ ..,
    ] = lease.effects()
    else {
        panic!("replacement Apply must publish admission before victim outcomes");
    };
    assert_eq!(entry.tx.hash(), replacement.0);
    assert_eq!(entry.ancestors_count, 1);
    assert_eq!(entry.descendants_count, 1);
    assert_eq!(*peer, PeerIndex::from(60));
    let expected_victims = HashSet::from([victim, child]);
    assert_eq!(rejected.len(), expected_victims.len());
    for effect in rejected {
        let CommittedEffect::Rejected(CommittedRejection::Replaced { entry, winner }) = effect
        else {
            panic!("every retained history victim keeps an exact replacement outcome");
        };
        let victim_hash = RawTxHash(entry.tx.hash());
        assert!(entry.ancestors_count >= 1);
        assert!(entry.descendants_count >= 1);
        assert!(
            expected_victims.contains(&victim_hash),
            "effect belongs to the exact replacement closure"
        );
        assert_eq!(winner, &replacement);
    }
    let published = authority
        .apply_effect_settlement_for_foundation(lease.complete_for_foundation().published())
        .expect("replacement outcome publication settles");
    assert_eq!(published.retired_effect_len(), 1);
}

#[test]
fn uak_isolated_rbf_preserves_exact_projection_and_effect_contract() {
    let mut authority = TxPoolAuthority::with_replacement(limits(), FeeRate::from_u64(1_000));
    let input = OutPoint::new(Byte32::new([62; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(62u32)
        .input(CellInput::new(input.clone(), 0))
        .build();
    let victim = accept_remote_transaction_with_payload(
        &mut authority,
        victim_tx.clone(),
        62,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &victim_tx,
            Vec::new(),
            vec![input.clone()],
            Capacity::shannons(1_000),
        ),
    );

    let replacement_tx = TransactionBuilder::default()
        .version(63u32)
        .input(CellInput::new(input.clone(), 0))
        .build();
    let replacement = verify_remote_transaction_with_payload(
        &mut authority,
        replacement_tx.clone(),
        63,
        resolved_payload_with_facts(
            &replacement_tx,
            Vec::new(),
            vec![input.clone()],
            Capacity::shannons(10_000),
        ),
    );
    let replacement_version = owner_version(&authority, &replacement);
    let committed = apply_plan_for_delta(
        authority
            .plan_accept_for_foundation(&replacement, replacement_version, AcceptedStatus::Pending)
            .expect("a funded isolated replacement has one canonical plan"),
    );

    let [removal] = committed.removals.as_slice() else {
        panic!("the isolated replacement removes exactly one victim");
    };
    assert_eq!(removal.hash, victim);
    assert_eq!(removal.cause, RemovalCause::Replacement);
    assert!(matches!(
        authority.entry(&victim),
        Some(OwnedTx::ReplacementHistory(_))
    ));
    assert!(matches!(
        authority.entry(&replacement),
        Some(OwnedTx::Accepted(_))
    ));
    assert_eq!(
        authority.accepted_spender(&input),
        Some(replacement.clone())
    );
    assert_membership_reference(&authority);
    assert_resource_reference(&authority);

    let lease = authority
        .effect_publication_receipt_for_foundation()
        .expect("the isolated replacement publishes one exact effect batch");
    assert!(matches!(
        lease.effects(),
        [
            CommittedEffect::Accepted(CommittedAcceptance::Admission { entry, .. }),
            CommittedEffect::Rejected(CommittedRejection::Replaced {
                entry: victim_entry,
                winner,
            }),
        ] if entry.tx.hash() == replacement.0
            && victim_entry.tx.hash() == victim.0
            && winner == &replacement
    ));
    drop(
        authority
            .apply_effect_settlement_for_foundation(lease.complete_for_foundation().published())
            .expect("the isolated replacement effect batch settles"),
    );
}

fn leaf_rbf_cohort_limits() -> ResourceLimits {
    ResourceLimits::new(
        ResourceVector::new(16, 128 * 1024, 128, 16),
        ResourceVector::new(16, 128 * 1024, 128, 16),
        ResourceVector::new(2, 16 * 1024, 16, 2),
        AcceptedResources::new(16, 128 * 1024, 128 * 1024, 128),
        ComputeLimits::new(4 * 1024, 4 * 1024, 16),
    )
    .and_then(|limits| {
        limits.with_replacement_history_limit(ResourceVector::new(4, 32 * 1024, 32, 0))
    })
    .expect("leaf-RBF cohort limits admit eight Ready owners and four histories")
}

fn disjoint_parent_append_limits() -> ResourceLimits {
    ResourceLimits::new(
        ResourceVector::new(256, 16 * 1024 * 1024, 4 * 1024, 256),
        ResourceVector::new(256, 16 * 1024 * 1024, 4 * 1024, 256),
        ResourceVector::new(8, 512 * 1024, 128, 8),
        AcceptedResources::new(256, 16 * 1024 * 1024, 16 * 1024 * 1024, 4 * 1024),
        ComputeLimits::new(16 * 1024 * 1024, 16 * 1024 * 1024, 256),
    )
    .and_then(|limits| {
        limits.with_replacement_history_limit(ResourceVector::new(8, 512 * 1024, 128, 0))
    })
    .expect("the disjoint parent-append fixture admits every bounded Ready owner")
}

fn disjoint_parent_append_fixture(
    count: usize,
) -> (TxPoolAuthority, Vec<RawTxHash>, Vec<RawTxHash>) {
    assert!((2..=MAX_READY_BATCH).contains(&count));
    let mut authority = TxPoolAuthority::for_foundation(disjoint_parent_append_limits());
    let mut parents = Vec::with_capacity(count);
    let mut candidates = Vec::with_capacity(count);
    for index in 0..count {
        let marker = u8::try_from(index + 128).expect("the bounded fixture marker fits u8");
        let (parent, children) = accepted_parent_with_ready_children(&mut authority, marker, 1);
        let [candidate] = children.as_slice() else {
            panic!("one parent owns one Ready append candidate")
        };
        parents.push(parent);
        candidates.push(candidate.clone());
    }
    (authority, candidates, parents)
}

fn accepted_output_parent(
    authority: &mut TxPoolAuthority,
    marker: u8,
    output_count: usize,
) -> TransactionView {
    let chain_input = OutPoint::new(Byte32::new([marker; 32]), 0);
    let mut builder = TransactionBuilder::default()
        .version(40_000 + u32::from(marker))
        .input(CellInput::new(chain_input.clone(), 0));
    for _ in 0..output_count {
        builder = builder
            .output(CellOutput::default())
            .output_data(Bytes::new().pack());
    }
    let transaction = builder.build();
    accept_remote_transaction_with_payload(
        authority,
        transaction.clone(),
        40_000 + usize::from(marker),
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &transaction,
            Vec::new(),
            vec![chain_input],
            Capacity::shannons(1_000),
        ),
    );
    transaction
}

fn ready_input_append(
    authority: &mut TxPoolAuthority,
    version: u32,
    parents: &[OutPoint],
) -> RawTxHash {
    let mut builder = TransactionBuilder::default().version(version);
    for parent in parents {
        builder = builder.input(CellInput::new(parent.clone(), 0));
    }
    let transaction = builder.build();
    verify_remote_transaction_with_payload(
        authority,
        transaction.clone(),
        usize::try_from(version).expect("fixture version fits usize"),
        resolved_payload_with_facts(
            &transaction,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(500),
        ),
    )
}

fn ready_dependency_append(
    authority: &mut TxPoolAuthority,
    marker: u8,
    parent: OutPoint,
) -> RawTxHash {
    let chain_input = OutPoint::new(Byte32::new([marker; 32]), 0);
    let transaction = TransactionBuilder::default()
        .version(50_000 + u32::from(marker))
        .input(CellInput::new(chain_input.clone(), 0))
        .cell_dep(CellDep::new_builder().out_point(parent.clone()).build())
        .build();
    let resident_bytes = transaction.data().total_size();
    let payload = ResolvedPayload::for_foundation(
        &transaction,
        vec![parent],
        64,
        Capacity::shannons(500),
        resident_bytes,
        vec![chain_input],
        Vec::new(),
    )
    .expect("the Accepted dependency is deliberately not chain-backed");
    verify_remote_transaction_with_payload(
        authority,
        transaction,
        50_000 + usize::from(marker),
        payload,
    )
}

#[test]
fn uak_disjoint_parent_append_cohort_matches_every_canonical_single_prefix() {
    for count in 2..=MAX_READY_BATCH {
        let (mut aggregate, candidates, parents) = disjoint_parent_append_fixture(count);
        let before_clocks = aggregate.clocks();
        let batch = independent_batch(&aggregate, &candidates);
        let SettlementPlan::IndependentRun(plan) = aggregate
            .plan_settlement(&batch)
            .expect("disjoint Accepted-parent components have one batch Plan")
        else {
            panic!("disjoint Accepted-parent components must use the existing independent Apply")
        };
        let order = plan
            .independent_order_for_foundation()
            .expect("the append cohort seals strongest-first candidate order");
        assert_eq!(order.len(), count);

        let (mut reversed, mut reversed_candidates, reversed_parents) =
            disjoint_parent_append_fixture(count);
        assert_eq!(reversed_parents, parents);
        reversed_candidates.reverse();
        let reversed_batch = independent_batch(&reversed, &reversed_candidates);
        let SettlementPlan::IndependentRun(reversed_plan) = reversed
            .plan_settlement(&reversed_batch)
            .expect("request permutation preserves the canonical cohort Plan")
        else {
            panic!("request permutation cannot change component independence")
        };
        assert_eq!(
            reversed_plan
                .independent_order_for_foundation()
                .expect("the reversed request has one canonical order"),
            order
        );
        drop(reversed_plan);

        let committed = apply_plan_for_delta(plan);
        assert!(committed.removals.is_empty());
        assert_eq!(committed.retired_len(), count);

        let lease = aggregate
            .effect_publication_receipt_for_foundation()
            .expect("the append cohort publishes one resident effect batch");
        assert_eq!(lease.effects().len(), count);
        for (effect, winner) in lease.effects().iter().zip(&order) {
            let CommittedEffect::Accepted(CommittedAcceptance::Admission { entry, .. }) = effect
            else {
                panic!("every append member publishes exactly one Accepted effect")
            };
            assert_eq!(RawTxHash(entry.tx.hash()), *winner);
        }

        let (mut reference, reference_candidates, reference_parents) =
            disjoint_parent_append_fixture(count);
        assert_eq!(reference_candidates, candidates);
        assert_eq!(reference_parents, parents);
        for winner in &order {
            let version = owner_version(&reference, winner);
            drop(apply_plan_for_delta(
                reference
                    .plan_accept_for_foundation(winner, version, AcceptedStatus::Pending)
                    .expect("canonical strongest-first parent append succeeds"),
            ));
        }
        let canonical_next_sequence = ApplySequence(
            before_clocks.next_sequence.0
                + u128::try_from(count).expect("the bounded cohort count fits u128"),
        );
        assert_eq!(
            aggregate.clocks().next_sequence,
            ApplySequence(before_clocks.next_sequence.0 + 1)
        );
        assert_eq!(reference.clocks().next_sequence, canonical_next_sequence);
        assert!(
            aggregate
                .normalized_snapshot()
                .equivalent_modulo_atomic_batch_stamp(
                    &reference.normalized_snapshot(),
                    before_clocks.next_sequence,
                    canonical_next_sequence,
                ),
            "one disjoint append Apply must equal the canonical no-interleave fold"
        );
        for (candidate, parent) in candidates.iter().zip(&parents) {
            assert_eq!(
                aggregate.accepted_parents(candidate),
                Some(HashSet::from([parent.clone()]))
            );
        }
        assert_resource_reference(&aggregate);
        assert_membership_reference(&aggregate);
        assert!(aggregate.primary_projection_consistent());
    }
}

#[test]
fn uak_parent_append_cohort_accepts_multiple_input_and_dependency_parents() {
    let mut input_authority = TxPoolAuthority::for_foundation(disjoint_parent_append_limits());
    let input_parents = [
        accepted_output_parent(&mut input_authority, 160, 1),
        accepted_output_parent(&mut input_authority, 161, 1),
        accepted_output_parent(&mut input_authority, 162, 1),
        accepted_output_parent(&mut input_authority, 163, 1),
    ];
    let input_parent_hashes: Vec<_> = input_parents
        .iter()
        .map(|parent| RawTxHash(parent.hash()))
        .collect();
    let input_candidates = vec![
        ready_input_append(
            &mut input_authority,
            51_000,
            &[
                OutPoint::new(input_parents[0].hash(), 0),
                OutPoint::new(input_parents[1].hash(), 0),
            ],
        ),
        ready_input_append(
            &mut input_authority,
            51_001,
            &[
                OutPoint::new(input_parents[2].hash(), 0),
                OutPoint::new(input_parents[3].hash(), 0),
            ],
        ),
    ];
    let batch = independent_batch(&input_authority, &input_candidates);
    let SettlementPlan::IndependentRun(plan) = input_authority
        .plan_settlement(&batch)
        .expect("disjoint multi-parent input components plan")
    else {
        panic!("disjoint multi-parent input components share one independent Apply")
    };
    drop(apply_plan_for_delta(plan));
    for (candidate, parents) in input_candidates
        .iter()
        .zip([&input_parent_hashes[0..2], &input_parent_hashes[2..4]])
    {
        assert_eq!(
            input_authority.accepted_parents(candidate),
            Some(parents.iter().cloned().collect())
        );
    }
    assert!(input_authority.primary_projection_consistent());

    let mut dependency_authority = TxPoolAuthority::for_foundation(disjoint_parent_append_limits());
    let first_parent = accepted_output_parent(&mut dependency_authority, 164, 1);
    let second_parent = accepted_output_parent(&mut dependency_authority, 165, 1);
    let dependency_candidates = vec![
        ready_dependency_append(
            &mut dependency_authority,
            166,
            OutPoint::new(first_parent.hash(), 0),
        ),
        ready_dependency_append(
            &mut dependency_authority,
            167,
            OutPoint::new(second_parent.hash(), 0),
        ),
    ];
    let batch = independent_batch(&dependency_authority, &dependency_candidates);
    let SettlementPlan::IndependentRun(plan) = dependency_authority
        .plan_settlement(&batch)
        .expect("disjoint Accepted dependency components plan")
    else {
        panic!("read-only Accepted dependency parents share one independent Apply")
    };
    drop(apply_plan_for_delta(plan));
    for (candidate, parent) in dependency_candidates
        .iter()
        .zip([first_parent.hash(), second_parent.hash()])
    {
        assert_eq!(
            dependency_authority.accepted_parents(candidate),
            Some(HashSet::from([RawTxHash(parent)]))
        );
    }
    assert!(dependency_authority.primary_projection_consistent());
}

#[test]
fn uak_parent_append_cohort_falls_back_on_deep_overlap_mixed_shape_and_effect_bound() {
    let mut deep_overlap = TxPoolAuthority::for_foundation(disjoint_parent_append_limits());
    let root = accepted_output_parent(&mut deep_overlap, 168, 2);
    let first_parent =
        ready_input_append(&mut deep_overlap, 52_000, &[OutPoint::new(root.hash(), 0)]);
    let second_parent =
        ready_input_append(&mut deep_overlap, 52_001, &[OutPoint::new(root.hash(), 1)]);
    for parent in [&first_parent, &second_parent] {
        let version = owner_version(&deep_overlap, parent);
        drop(apply_plan_for_delta(
            deep_overlap
                .plan_accept_for_foundation(parent, version, AcceptedStatus::Pending)
                .expect("the intermediate Accepted parent commits"),
        ));
        drain_fixture_effects(&mut deep_overlap);
    }
    let candidates = [
        ready_input_append(
            &mut deep_overlap,
            52_002,
            &[OutPoint::new(first_parent.0.clone(), 0)],
        ),
        ready_input_append(
            &mut deep_overlap,
            52_003,
            &[OutPoint::new(second_parent.0.clone(), 0)],
        ),
    ];
    let batch = independent_batch(&deep_overlap, &candidates);
    assert_coupled_and_drop(
        deep_overlap
            .plan_settlement(&batch)
            .expect("a shared deep ancestor preserves the canonical coupled route"),
    );

    let mut mixed = TxPoolAuthority::with_replacement(
        disjoint_parent_append_limits(),
        FeeRate::from_u64(1_000),
    );
    let (_, append_children) = accepted_parent_with_ready_children(&mut mixed, 169, 1);
    let [append] = append_children.as_slice() else {
        panic!("the mixed fixture owns one append candidate")
    };
    let (_, replacement) = add_leaf_rbf_pair(&mut mixed, 0, 170, Vec::new(), 30_000);
    let batch = independent_batch(&mixed, &[append.clone(), replacement]);
    assert_coupled_and_drop(
        mixed
            .plan_settlement(&batch)
            .expect("parent append and replacement cannot share a composite Apply"),
    );

    let effect_bytes = 128 * 1024;
    let effect_limits = EffectLimits::partitioned(
        EffectCapacity::new(4, effect_bytes),
        EffectCapacity::new(1, effect_bytes),
        EffectCapacity::new(1, effect_bytes),
        EffectBatchBounds::new(
            EffectBatchBound::new(1, effect_bytes),
            EffectBatchBound::new(1, effect_bytes),
            EffectBatchBound::new(1, effect_bytes),
        ),
    )
    .expect("one-effect batches are a valid immutable envelope");
    let mut effect_bound = TxPoolAuthority::for_foundation_with_effect_limits(
        disjoint_parent_append_limits(),
        effect_limits,
    )
    .expect("the append fixture accepts the bounded effect log");
    let (_, first) = accepted_parent_with_ready_children(&mut effect_bound, 171, 1);
    let (_, second) = accepted_parent_with_ready_children(&mut effect_bound, 172, 1);
    let batch = independent_batch(&effect_bound, &[first[0].clone(), second[0].clone()]);
    assert_coupled_and_drop(
        effect_bound
            .plan_settlement(&batch)
            .expect("cohort effect pressure falls back before any Apply"),
    );
    assert!(effect_bound.primary_projection_consistent());
}

#[test]
fn uak_parent_append_cohort_falls_back_when_component_support_overlaps() {
    let mut authority = TxPoolAuthority::for_foundation(disjoint_parent_append_limits());
    let (_parent, children) =
        accepted_parent_with_ready_children(&mut authority, 127, MAX_READY_BATCH);
    let batch = independent_batch(&authority, &children);
    assert_coupled_and_drop(
        authority
            .plan_settlement(&batch)
            .expect("overlapping sibling appends retain one canonical strongest Plan"),
    );
    assert!(authority.primary_projection_consistent());
}

pub(in crate::authority) fn add_leaf_rbf_pair(
    authority: &mut TxPoolAuthority,
    index: usize,
    marker: u8,
    dependencies: Vec<OutPoint>,
    fee: u64,
) -> (RawTxHash, RawTxHash) {
    let input = OutPoint::new(Byte32::new([marker; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(10_000 + u32::from(marker))
        .input(CellInput::new(input.clone(), 0))
        .build();
    let victim = accept_remote_transaction_with_payload(
        authority,
        victim_tx.clone(),
        10_000 + index,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &victim_tx,
            dependencies.clone(),
            vec![input.clone()],
            Capacity::shannons(100),
        ),
    );
    let candidate_tx = TransactionBuilder::default()
        .version(20_000 + u32::from(marker))
        .input(CellInput::new(input.clone(), 0))
        .build();
    let candidate = verify_remote_transaction_with_payload(
        authority,
        candidate_tx.clone(),
        20_000 + index,
        resolved_payload_with_facts(
            &candidate_tx,
            dependencies,
            vec![input],
            Capacity::shannons(fee),
        ),
    );
    (victim, candidate)
}

fn leaf_rbf_cohort_fixture(count: usize) -> (TxPoolAuthority, Vec<RawTxHash>, Vec<RawTxHash>) {
    assert!((2..=MAX_READY_BATCH).contains(&count));
    let mut authority =
        TxPoolAuthority::with_replacement(leaf_rbf_cohort_limits(), FeeRate::from_u64(1_000));
    // The production always-success workload shares one immutable chain
    // cell-dep across every pair. Read sharing is not membership coupling;
    // only an owner transition which can change that dependency is.
    let shared_chain_dependency = OutPoint::new(Byte32::new([99; 32]), 0);
    let mut candidates = Vec::with_capacity(count);
    let mut victims = Vec::with_capacity(count);
    for index in 0..count {
        let marker = u8::try_from(index + 100).expect("bounded cohort marker fits u8");
        let rank = u64::try_from(count - index).expect("bounded rank fits u64");
        let (victim, candidate) = add_leaf_rbf_pair(
            &mut authority,
            index,
            marker,
            vec![shared_chain_dependency.clone()],
            10_000u64
                .checked_mul(rank)
                .expect("bounded cohort fee fits u64"),
        );
        victims.push(victim);
        candidates.push(candidate);
    }
    (authority, candidates, victims)
}

#[test]
fn uak_exclusive_membership_evaluation_disables_policy_witness_recording() {
    let (mut authority, candidates, _victims) = leaf_rbf_cohort_fixture(2);
    let candidate = candidates[0].clone();
    let receipt = authority
        .independent_candidate_for_foundation(
            &candidate,
            owner_version(&authority, &candidate),
            AcceptedStatus::Pending,
        )
        .expect("the exclusive fixture has current final evidence")
        .into_receipt();
    assert_eq!(
        authority
            .exclusive_membership_witness_activity_for_foundation(receipt)
            .expect("the canonical exclusive RBF evaluation completes"),
        (0, 0),
        "disabled policy reads neither record rows nor capture AcceptedEntry clones"
    );
    assert!(matches!(
        authority.entry(&candidate),
        Some(OwnedTx::PreAccepted(_))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_singleton_rbf_policy_witness_rechecks_victim_before_owner_mutation() {
    let (mut authority, candidates, victims) = leaf_rbf_cohort_fixture(2);
    let candidate = candidates[0].clone();
    let victim = victims[0].clone();
    let batch = independent_batch(&authority, std::slice::from_ref(&candidate));
    let compiled = authority
        .compile_shared_independent_settlement(&batch)
        .expect("one canonical non-capacity RBF result compiles")
        .into_option_for_foundation()
        .expect("the singleton RBF result has a bounded shared terminal");

    let victim_input = match authority.entry(&victim) {
        Some(OwnedTx::Accepted(entry)) => entry
            .proof
            .payload()
            .footprint
            .inputs()
            .first()
            .expect("the leaf victim has one input")
            .clone(),
        _ => panic!("the fixture victim is Accepted"),
    };
    let competitor_tx = TransactionBuilder::default()
        .version(60_200u32)
        .input(CellInput::new(victim_input.clone(), 0))
        .build();
    let competitor = verify_remote_transaction_with_payload(
        &mut authority,
        competitor_tx.clone(),
        60_200,
        resolved_payload_with_facts(
            &competitor_tx,
            Vec::new(),
            vec![victim_input],
            Capacity::shannons(40_000),
        ),
    );
    let competitor_version = owner_version(&authority, &competitor);
    drop(apply_plan_for_delta(
        authority
            .plan_accept_for_foundation(&competitor, competitor_version, AcceptedStatus::Pending)
            .expect("the competing replacement mutates the observed victim"),
    ));
    let candidate_version = owner_version(&authority, &candidate);
    let owner_count = authority.owner_count();
    let prepared = compiled
        .bind(&authority)
        .unwrap_or_else(|_| panic!("generation remains current"));
    assert!(matches!(
        prepared.apply(),
        Err(super::super::plan::ConcurrentIndependentError::ChangedCut(
            _
        ))
    ));
    assert_eq!(owner_version(&authority, &candidate), candidate_version);
    assert_eq!(authority.owner_count(), owner_count);
    assert!(!matches!(
        authority.entry(&victim),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(matches!(
        authority.entry(&candidate),
        Some(OwnedTx::PreAccepted(_))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_repeated_policy_parent_reads_preserve_every_observation() {
    let (authority, candidates, victims) = leaf_rbf_cohort_fixture(2);
    let candidate = candidates[0].clone();
    let victim = victims[0].clone();
    let interposed_parent = victims[1].clone();
    let batch = independent_batch(&authority, std::slice::from_ref(&candidate));
    let (probe, entered, release) = ConcurrentRemovalProbe::new();
    authority
        .entries_for_reference()
        .set_membership_parent_read_probe(victim.clone(), probe);

    std::thread::scope(|scope| {
        let compile = scope.spawn(|| authority.compile_shared_independent_settlement(&batch));
        entered
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("the evaluator records the first victim-parent read");
        authority
            .entries_for_reference()
            .replace_membership_parents_for_foundation(&victim, HashSet::from([interposed_parent]));
        release.send(()).expect("release the repeated policy read");
        let result = compile.join().expect("the bounded evaluator joins");
        let compiled = result.expect("a changed policy cut is a typed fallback");
        assert!(
            compiled.into_option_for_foundation().is_none(),
            "no shared terminal may retain only the later observation"
        );
    });

    assert!(matches!(
        authority.entry(&candidate),
        Some(OwnedTx::PreAccepted(_))
    ));
    authority
        .entries_for_reference()
        .replace_membership_parents_for_foundation(&victim, HashSet::new());
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_singleton_membership_policy_witness_rechecks_parent_rows() {
    let mut authority = TxPoolAuthority::for_foundation(disjoint_parent_append_limits());
    let (_parent, candidates) = accepted_parent_with_ready_children(&mut authority, 179, 2);
    let first_batch = independent_batch(&authority, &candidates[0..1]);
    let second_batch = independent_batch(&authority, &candidates[1..2]);
    let first = authority
        .compile_shared_independent_settlement(&first_batch)
        .expect("the first parent append compiles")
        .into_option_for_foundation()
        .expect("the first parent append has a sparse witness");
    let second = authority
        .compile_shared_independent_settlement(&second_batch)
        .expect("the sibling parent append compiles")
        .into_option_for_foundation()
        .expect("the sibling parent append has a sparse witness");
    drop(
        second
            .bind(&authority)
            .unwrap_or_else(|_| panic!("generation remains current"))
            .apply()
            .expect("the sibling changes the observed parent rows"),
    );
    let first_version = owner_version(&authority, &candidates[0]);
    assert!(
        first
            .bind(&authority)
            .unwrap_or_else(|_| panic!("generation remains current"))
            .apply()
            .is_err()
    );
    assert_eq!(owner_version(&authority, &candidates[0]), first_version);
    assert!(matches!(
        authority.entry(&candidates[0]),
        Some(OwnedTx::PreAccepted(_))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_ready_policy_absent_parent_aba_stales_the_old_singleton_cut() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = TransactionBuilder::default()
        .version(60_179u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent = RawTxHash(parent_tx.hash());
    let parent_output = OutPoint::new(parent_tx.hash(), 0);
    let candidate_tx = TransactionBuilder::default()
        .version(60_180u32)
        .input(CellInput::new(parent_output.clone(), 0))
        .build();
    let candidate = verify_remote_transaction_with_payload(
        &mut authority,
        candidate_tx.clone(),
        60_180,
        resolved_payload_with_facts(
            &candidate_tx,
            Vec::new(),
            vec![parent_output],
            Capacity::shannons(1_000),
        ),
    );
    let batch = independent_batch(&authority, std::slice::from_ref(&candidate));
    let compiled = authority
        .compile_shared_independent_settlement(&batch)
        .expect("the chain-backed candidate records its absent origin owner")
        .into_option_for_foundation()
        .expect("the original vacancy revision is bounded");
    let transient = authority
        .entry(&candidate)
        .expect("the Ready candidate owns a transient test carrier");
    authority
        .entries_for_reference()
        .cycle_absent_owner_for_foundation(&parent, transient);
    let candidate_version = owner_version(&authority, &candidate);
    assert!(
        compiled
            .bind(&authority)
            .unwrap_or_else(|_| panic!("generation remains current"))
            .apply()
            .is_err()
    );
    assert_eq!(owner_version(&authority, &candidate), candidate_version);
    assert!(authority.entry(&parent).is_none());
    assert!(matches!(
        authority.entry(&candidate),
        Some(OwnedTx::PreAccepted(_))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_singleton_membership_policy_witness_rechecks_dependency_consumers() {
    let mut authority = TxPoolAuthority::for_foundation(disjoint_parent_append_limits());
    let parent_tx = TransactionBuilder::default()
        .version(55_179u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent_output = OutPoint::new(parent_tx.hash(), 0);
    let first_reader_tx = TransactionBuilder::default()
        .version(55_180u32)
        .cell_dep(
            CellDep::new_builder()
                .out_point(parent_output.clone())
                .build(),
        )
        .build();
    accept_remote_transaction_with_payload(
        &mut authority,
        first_reader_tx.clone(),
        55_180,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &first_reader_tx,
            vec![parent_output.clone()],
            Vec::new(),
            Capacity::shannons(1_000),
        ),
    );
    let parent = verify_remote_transaction_with_payload(
        &mut authority,
        parent_tx.clone(),
        55_179,
        resolved_payload_with_facts(
            &parent_tx,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(2_000),
        ),
    );
    let batch = independent_batch(&authority, std::slice::from_ref(&parent));
    let compiled = authority
        .compile_shared_independent_settlement(&batch)
        .expect("the late dependency parent compiles")
        .into_option_for_foundation()
        .expect("the dependency-consumer row is bounded");

    let second_reader_tx = TransactionBuilder::default()
        .version(55_181u32)
        .cell_dep(
            CellDep::new_builder()
                .out_point(parent_output.clone())
                .build(),
        )
        .build();
    accept_remote_transaction_with_payload(
        &mut authority,
        second_reader_tx.clone(),
        55_181,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &second_reader_tx,
            vec![parent_output],
            Vec::new(),
            Capacity::shannons(1_000),
        ),
    );
    let parent_version = owner_version(&authority, &parent);
    assert!(
        compiled
            .bind(&authority)
            .unwrap_or_else(|_| panic!("generation remains current"))
            .apply()
            .is_err()
    );
    assert_eq!(owner_version(&authority, &parent), parent_version);
    assert!(matches!(
        authority.entry(&parent),
        Some(OwnedTx::PreAccepted(_))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_shared_membership_terminalizes_stable_high_dependency_fanout() {
    let mut authority = TxPoolAuthority::for_foundation(disjoint_parent_append_limits());
    let producer_tx = TransactionBuilder::default()
        .version(56_179u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let producer_output = OutPoint::new(producer_tx.hash(), 0);
    for index in 0..=crate::constants::MAX_POOL_MUTATION_CANDIDATES {
        let version = 56_200u32
            .checked_add(u32::try_from(index).expect("the fixed fanout index fits u32"))
            .expect("the fixed fanout version fits u32");
        let reader_tx = TransactionBuilder::default()
            .version(version)
            .cell_dep(
                CellDep::new_builder()
                    .out_point(producer_output.clone())
                    .build(),
            )
            .build();
        accept_remote_transaction_with_payload(
            &mut authority,
            reader_tx.clone(),
            index,
            AcceptedStatus::Pending,
            resolved_payload_with_facts(
                &reader_tx,
                vec![producer_output.clone()],
                Vec::new(),
                Capacity::shannons(1_000),
            ),
        );
    }
    let producer = verify_remote_transaction_with_payload(
        &mut authority,
        producer_tx.clone(),
        56_179,
        resolved_payload_with_facts(
            &producer_tx,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(2_000),
        ),
    );
    let batch = independent_batch(&authority, std::slice::from_ref(&producer));
    let head = authority
        .compile_shared_canonical_ready_head(&batch)
        .expect("the stable over-bound predicate compiles one exact rejection");
    let (compiled, continuation) = head.into_parts();
    assert!(continuation.is_none());
    let shared = compiled
        .bind(&authority)
        .and_then(PreparedIndependentApply::apply)
        .expect("the over-limit relation predicate remains current");
    let (committed, post_commit_fault) = shared.into_parts();
    assert_eq!(post_commit_fault, None);
    drop(committed);
    assert!(authority.entry(&producer).is_none());
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_unrelated_singleton_rbf_interposition_preserves_both_sparse_cuts() {
    let mut authority =
        TxPoolAuthority::with_replacement(leaf_rbf_cohort_limits(), FeeRate::from_u64(1_000));
    let mut candidates = Vec::new();
    let mut victims = Vec::new();
    for index in 0..8usize {
        let marker = u8::try_from(200 + index).expect("bounded marker fits u8");
        let (victim, candidate) =
            add_leaf_rbf_pair(&mut authority, index, marker, Vec::new(), 30_000);
        victims.push(victim);
        candidates.push(candidate);
    }
    let mut compiled = candidates
        .iter()
        .map(|candidate| {
            let batch = independent_batch(&authority, std::slice::from_ref(candidate));
            authority
                .compile_shared_independent_settlement(&batch)
                .expect("every singleton RBF candidate compiles")
                .into_option_for_foundation()
                .expect("every singleton is non-capacity")
        })
        .map(Some)
        .collect::<Vec<_>>();
    let pair = (0..compiled.len()).find_map(|left| {
        ((left + 1)..compiled.len()).find_map(|right| {
            compiled[left]
                .as_ref()
                .zip(compiled[right].as_ref())
                .filter(|(left, right)| left.is_compatible_with(right))
                .map(|_| (left, right))
        })
    });
    let (left_index, right_index) = pair.expect("the fixed layout has two unrelated sparse cuts");
    let left = compiled[left_index]
        .take()
        .expect("selected left cut exists");
    let right = compiled[right_index]
        .take()
        .expect("selected right cut exists");
    drop(compiled);
    drop(
        right
            .bind(&authority)
            .unwrap_or_else(|_| panic!("generation remains current"))
            .apply()
            .expect("the unrelated right cut commits"),
    );
    drop(
        left.bind(&authority)
            .unwrap_or_else(|_| panic!("generation remains current"))
            .apply()
            .expect("the unrelated left cut remains fresh"),
    );
    for index in [left_index, right_index] {
        assert!(matches!(
            authority.entry(&candidates[index]),
            Some(OwnedTx::Accepted(_))
        ));
        assert!(matches!(
            authority.entry(&victims[index]),
            Some(OwnedTx::ReplacementHistory(_)) | None
        ));
    }
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_leaf_rbf_cohort_matches_every_canonical_single_prefix() {
    for count in 2..=MAX_READY_BATCH {
        let (mut aggregate, candidates, victims) = leaf_rbf_cohort_fixture(count);
        let before_clocks = aggregate.clocks();
        let victim_by_candidate = candidates
            .iter()
            .cloned()
            .zip(victims.iter().cloned())
            .collect::<HashMap<_, _>>();
        let batch = independent_batch(&aggregate, &candidates);
        let SettlementPlan::IndependentRun(plan) = aggregate
            .plan_settlement(&batch)
            .expect("strict disjoint leaf replacements have one batch Plan")
        else {
            panic!("strict disjoint leaf replacements must use the composite batch")
        };
        let order = plan
            .independent_order_for_foundation()
            .expect("the batch seals strongest-first candidate order");
        assert_eq!(order.len(), count);
        let committed = apply_plan_for_delta(plan);
        assert_eq!(committed.removals.len(), count);
        assert_eq!(committed.retired_len(), count * 2);
        for (removal, winner) in committed.removals.iter().zip(&order) {
            assert_eq!(removal.cause, RemovalCause::Replacement);
            assert_eq!(
                Some(&removal.hash),
                victim_by_candidate.get(winner),
                "each candidate retires only its own victim"
            );
        }
        let mut expected_version = before_clocks.next_version.0;
        let mut expected_arrival = before_clocks.next_arrival.0;
        for (index, winner) in order.iter().enumerate() {
            assert_eq!(owner_version(&aggregate, winner).0, expected_version);
            expected_version += 1;
            let victim = victim_by_candidate
                .get(winner)
                .expect("every winner has one fixture victim");
            if index < 4 {
                let Some(OwnedTx::ReplacementHistory(history)) = aggregate.entry(victim) else {
                    panic!("strongest four victims retain optional history")
                };
                assert_eq!(history.record().version.0, expected_version);
                assert_eq!(history.record().arrival.0, expected_arrival);
                expected_version += 1;
                expected_arrival += 1;
            } else {
                assert!(
                    aggregate.entry(victim).is_none(),
                    "history pressure terminalizes only the weaker member"
                );
            }
        }
        assert_eq!(aggregate.clocks().next_version.0, expected_version);
        assert_eq!(aggregate.clocks().next_arrival.0, expected_arrival);

        let lease = aggregate
            .effect_publication_receipt_for_foundation()
            .expect("the composite publishes one resident effect batch");
        assert_eq!(lease.effects().len(), count * 2);
        for (effects, winner) in lease.effects().chunks_exact(2).zip(&order) {
            let [
                CommittedEffect::Accepted(CommittedAcceptance::Admission { entry, .. }),
                CommittedEffect::Rejected(CommittedRejection::Replaced {
                    entry: victim,
                    winner: observed_winner,
                }),
            ] = effects
            else {
                panic!("every cohort member publishes Accepted then Replaced")
            };
            assert_eq!(RawTxHash(entry.tx.hash()), *winner);
            assert_eq!(observed_winner, winner);
            assert_eq!(
                Some(&RawTxHash(victim.tx.hash())),
                victim_by_candidate.get(winner)
            );
        }

        let (mut reference, reference_candidates, reference_victims) =
            leaf_rbf_cohort_fixture(count);
        assert_eq!(reference_candidates, candidates);
        assert_eq!(reference_victims, victims);
        for winner in &order {
            let version = owner_version(&reference, winner);
            drop(apply_plan_for_delta(
                reference
                    .plan_accept_for_foundation(winner, version, AcceptedStatus::Pending)
                    .expect("canonical strongest-first single replacement succeeds"),
            ));
        }
        let canonical_next_sequence = ApplySequence(
            before_clocks.next_sequence.0
                + u128::try_from(count).expect("bounded cohort count fits u128"),
        );
        assert_eq!(
            aggregate.clocks().next_sequence,
            ApplySequence(before_clocks.next_sequence.0 + 1)
        );
        assert_eq!(reference.clocks().next_sequence, canonical_next_sequence);
        assert!(
            aggregate
                .normalized_snapshot()
                .equivalent_modulo_atomic_batch_stamp(
                    &reference.normalized_snapshot(),
                    before_clocks.next_sequence,
                    canonical_next_sequence,
                ),
            "one leaf-RBF Apply must equal the canonical no-interleave fold"
        );
        assert_eq!(
            aggregate
                .read_view()
                .capture_template()
                .expect("composite template read is coherent")
                .selected_len(),
            count
        );
        assert_eq!(
            aggregate
                .read_view()
                .capture_persistence()
                .expect("history is excluded from persistence")
                .selected_len(),
            count
        );
        assert_resource_reference(&aggregate);
        assert_membership_reference(&aggregate);
        assert!(aggregate.primary_projection_consistent());
    }
}

fn assert_leaf_rbf_cohort_falls_back(authority: &mut TxPoolAuthority, candidates: &[RawTxHash]) {
    let batch = independent_batch(authority, candidates);
    assert_coupled_and_drop(
        authority
            .plan_settlement(&batch)
            .expect("a non-admitted leaf cohort still has one canonical strongest Plan"),
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_leaf_rbf_cohort_rejects_every_logical_overlap_and_non_leaf_shape() {
    // Two candidates name the same resident spender and exact input.
    let mut shared_victim =
        TxPoolAuthority::with_replacement(leaf_rbf_cohort_limits(), FeeRate::from_u64(1_000));
    let shared_input = OutPoint::new(Byte32::new([140; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(30_140u32)
        .input(CellInput::new(shared_input.clone(), 0))
        .build();
    accept_remote_transaction_with_payload(
        &mut shared_victim,
        victim_tx.clone(),
        30_140,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &victim_tx,
            Vec::new(),
            vec![shared_input.clone()],
            Capacity::shannons(100),
        ),
    );
    let mut shared_candidates = Vec::new();
    for (index, fee) in [30_000u64, 20_000].into_iter().enumerate() {
        let candidate_tx = TransactionBuilder::default()
            .version(31_140 + u32::try_from(index).expect("bounded index fits u32"))
            .input(CellInput::new(shared_input.clone(), 0))
            .build();
        shared_candidates.push(verify_remote_transaction_with_payload(
            &mut shared_victim,
            candidate_tx.clone(),
            31_140 + index,
            resolved_payload_with_facts(
                &candidate_tx,
                Vec::new(),
                vec![shared_input.clone()],
                Capacity::shannons(fee),
            ),
        ));
    }
    assert_leaf_rbf_cohort_falls_back(&mut shared_victim, &shared_candidates);

    // C2 reads an output whose Accepted owner V1 is lost by C1. Both
    // before-cut evaluations can succeed, but their loss/dependency
    // footprints forbid one composite Apply.
    let mut dependency_loss =
        TxPoolAuthority::with_replacement(leaf_rbf_cohort_limits(), FeeRate::from_u64(1_000));
    let first_input = OutPoint::new(Byte32::new([150; 32]), 0);
    let first_victim_tx = TransactionBuilder::default()
        .version(30_150u32)
        .input(CellInput::new(first_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    accept_remote_transaction_with_payload(
        &mut dependency_loss,
        first_victim_tx.clone(),
        30_150,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &first_victim_tx,
            Vec::new(),
            vec![first_input.clone()],
            Capacity::shannons(100),
        ),
    );
    let first_candidate_tx = TransactionBuilder::default()
        .version(31_150u32)
        .input(CellInput::new(first_input.clone(), 0))
        .build();
    let first_candidate = verify_remote_transaction_with_payload(
        &mut dependency_loss,
        first_candidate_tx.clone(),
        31_150,
        resolved_payload_with_facts(
            &first_candidate_tx,
            Vec::new(),
            vec![first_input],
            Capacity::shannons(30_000),
        ),
    );
    let second_input = OutPoint::new(Byte32::new([151; 32]), 0);
    let second_victim_tx = TransactionBuilder::default()
        .version(30_151u32)
        .input(CellInput::new(second_input.clone(), 0))
        .build();
    accept_remote_transaction_with_payload(
        &mut dependency_loss,
        second_victim_tx.clone(),
        30_151,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &second_victim_tx,
            Vec::new(),
            vec![second_input.clone()],
            Capacity::shannons(100),
        ),
    );
    let second_candidate_tx = TransactionBuilder::default()
        .version(31_151u32)
        .input(CellInput::new(second_input.clone(), 0))
        .build();
    let second_candidate = verify_remote_transaction_with_payload(
        &mut dependency_loss,
        second_candidate_tx.clone(),
        31_151,
        resolved_payload_with_facts(
            &second_candidate_tx,
            vec![OutPoint::new(first_victim_tx.hash(), 0)],
            vec![second_input],
            Capacity::shannons(20_000),
        ),
    );
    assert_leaf_rbf_cohort_falls_back(&mut dependency_loss, &[first_candidate, second_candidate]);

    // Each replacement fits against the before cut, but the second ordered
    // prefix would exceed Accepted serialized bytes after the first winner.
    let capacity_victim_probe = TransactionBuilder::default()
        .version(30_154u32)
        .input(CellInput::new(OutPoint::new(Byte32::new([154; 32]), 0), 0))
        .build();
    let capacity_candidate_probe = TransactionBuilder::default()
        .version(31_154u32)
        .input(CellInput::new(OutPoint::new(Byte32::new([155; 32]), 0), 0))
        .output(CellOutput::default())
        .output_data(Bytes::from(vec![0; 1024]).pack())
        .build();
    let one_prefix_bytes = capacity_victim_probe
        .data()
        .serialized_size_in_block()
        .checked_add(capacity_candidate_probe.data().serialized_size_in_block())
        .expect("two bounded serialized costs fit usize");
    let capacity_limits = ResourceLimits::new(
        ResourceVector::new(16, 128 * 1024, 128, 16),
        ResourceVector::new(16, 128 * 1024, 128, 16),
        ResourceVector::new(2, 16 * 1024, 16, 2),
        AcceptedResources::new(8, one_prefix_bytes, 128 * 1024, 128),
        ComputeLimits::new(4 * 1024, 4 * 1024, 16),
    )
    .and_then(|limits| {
        limits.with_replacement_history_limit(ResourceVector::new(4, 32 * 1024, 32, 0))
    })
    .expect("ordered capacity fixture has valid resource partitions");
    let mut ordered_capacity =
        TxPoolAuthority::with_replacement(capacity_limits, FeeRate::from_u64(1_000));
    let mut capacity_candidates = Vec::new();
    for index in 0..2u8 {
        let input = OutPoint::new(Byte32::new([154 + index; 32]), 0);
        let victim_tx = TransactionBuilder::default()
            .version(30_154 + u32::from(index))
            .input(CellInput::new(input.clone(), 0))
            .build();
        accept_remote_transaction_with_payload(
            &mut ordered_capacity,
            victim_tx.clone(),
            30_154 + usize::from(index),
            AcceptedStatus::Pending,
            resolved_payload_with_facts(
                &victim_tx,
                Vec::new(),
                vec![input.clone()],
                Capacity::shannons(100),
            ),
        );
        let candidate_tx = TransactionBuilder::default()
            .version(31_154 + u32::from(index))
            .input(CellInput::new(input.clone(), 0))
            .output(CellOutput::default())
            .output_data(Bytes::from(vec![index; 1024]).pack())
            .build();
        capacity_candidates.push(verify_remote_transaction_with_payload(
            &mut ordered_capacity,
            candidate_tx.clone(),
            31_154 + usize::from(index),
            resolved_payload_with_facts(
                &candidate_tx,
                Vec::new(),
                vec![input],
                Capacity::shannons(30_000 - u64::from(index) * 10_000),
            ),
        ));
    }
    assert_leaf_rbf_cohort_falls_back(&mut ordered_capacity, &capacity_candidates);

    // Batch-slot amortization is allowed, but the immutable effect envelope
    // is not. Four cohort effects exceed this hard bound of two, while the
    // canonical strongest replacement still fits.
    let effect_bytes = 128 * 1024;
    let effect_limits = EffectLimits::partitioned(
        EffectCapacity::new(4, effect_bytes),
        EffectCapacity::new(1, effect_bytes),
        EffectCapacity::new(1, effect_bytes),
        EffectBatchBounds::new(
            EffectBatchBound::new(2, effect_bytes),
            EffectBatchBound::new(2, effect_bytes),
            EffectBatchBound::new(2, effect_bytes),
        ),
    )
    .expect("two-effect immutable batches are a valid hard envelope");
    let mut effect_bound = TxPoolAuthority::with_replacement_and_effect_limits(
        leaf_rbf_cohort_limits(),
        FeeRate::from_u64(1_000),
        effect_limits,
    )
    .expect("replacement fixture accepts the bounded effect log");
    let (_, first) = add_leaf_rbf_pair(&mut effect_bound, 0, 152, Vec::new(), 30_000);
    let (_, second) = add_leaf_rbf_pair(&mut effect_bound, 1, 153, Vec::new(), 20_000);
    assert_leaf_rbf_cohort_falls_back(&mut effect_bound, &[first, second]);

    // A victim with an Accepted child is not a leaf pair even when the other
    // member is strictly disjoint.
    let mut child =
        TxPoolAuthority::with_replacement(leaf_rbf_cohort_limits(), FeeRate::from_u64(1_000));
    let child_input = OutPoint::new(Byte32::new([144; 32]), 0);
    let child_victim_tx = TransactionBuilder::default()
        .version(30_144u32)
        .input(CellInput::new(child_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    accept_remote_transaction_with_payload(
        &mut child,
        child_victim_tx.clone(),
        30_144,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &child_victim_tx,
            Vec::new(),
            vec![child_input.clone()],
            Capacity::shannons(100),
        ),
    );
    let descendant_tx = TransactionBuilder::default()
        .version(30_145u32)
        .input(CellInput::new(OutPoint::new(child_victim_tx.hash(), 0), 0))
        .build();
    accept_remote_transaction_with_payload(
        &mut child,
        descendant_tx.clone(),
        30_145,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &descendant_tx,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(100),
        ),
    );
    let child_replacement_tx = TransactionBuilder::default()
        .version(31_144u32)
        .input(CellInput::new(child_input.clone(), 0))
        .build();
    let child_replacement = verify_remote_transaction_with_payload(
        &mut child,
        child_replacement_tx.clone(),
        31_144,
        resolved_payload_with_facts(
            &child_replacement_tx,
            Vec::new(),
            vec![child_input],
            Capacity::shannons(30_000),
        ),
    );
    let (_, disjoint) = add_leaf_rbf_pair(&mut child, 1, 145, Vec::new(), 20_000);
    assert_leaf_rbf_cohort_falls_back(&mut child, &[child_replacement, disjoint]);

    // Distinct victims below one Accepted parent share an ancestor footprint
    // and therefore remain coupled.
    let mut ancestor =
        TxPoolAuthority::with_replacement(leaf_rbf_cohort_limits(), FeeRate::from_u64(1_000));
    let parent_input = OutPoint::new(Byte32::new([146; 32]), 0);
    let parent_tx = TransactionBuilder::default()
        .version(30_146u32)
        .input(CellInput::new(parent_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    accept_remote_transaction_with_payload(
        &mut ancestor,
        parent_tx.clone(),
        30_146,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &parent_tx,
            Vec::new(),
            vec![parent_input],
            Capacity::shannons(100),
        ),
    );
    let mut ancestor_candidates = Vec::new();
    for index in 0..2u32 {
        let input = OutPoint::new(parent_tx.hash(), index);
        let victim_tx = TransactionBuilder::default()
            .version(30_147 + index)
            .input(CellInput::new(input.clone(), 0))
            .build();
        accept_remote_transaction_with_payload(
            &mut ancestor,
            victim_tx.clone(),
            30_147 + usize::try_from(index).expect("bounded index fits usize"),
            AcceptedStatus::Pending,
            resolved_payload_with_facts(
                &victim_tx,
                Vec::new(),
                Vec::new(),
                Capacity::shannons(100),
            ),
        );
        let replacement_tx = TransactionBuilder::default()
            .version(31_147 + index)
            .input(CellInput::new(input, 0))
            .build();
        ancestor_candidates.push(verify_remote_transaction_with_payload(
            &mut ancestor,
            replacement_tx.clone(),
            31_147 + usize::try_from(index).expect("bounded index fits usize"),
            resolved_payload_with_facts(
                &replacement_tx,
                Vec::new(),
                Vec::new(),
                Capacity::shannons(30_000 - u64::from(index) * 10_000),
            ),
        ));
    }
    assert_leaf_rbf_cohort_falls_back(&mut ancestor, &ancestor_candidates);
}

#[test]
fn uak_coupled_continuation_restores_one_independent_tail_apply() {
    let mut authority = TxPoolAuthority::with_replacement(limits(), FeeRate::from_u64(1_000));
    let conflicted_input = OutPoint::new(Byte32::new([63; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(63u32)
        .input(CellInput::new(conflicted_input.clone(), 0))
        .build();
    let victim = accept_remote_transaction_with_payload(
        &mut authority,
        victim_tx.clone(),
        63,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &victim_tx,
            Vec::new(),
            vec![conflicted_input.clone()],
            Capacity::shannons(1_000),
        ),
    );

    let replacement_tx = TransactionBuilder::default()
        .version(64u32)
        .input(CellInput::new(conflicted_input.clone(), 0))
        .build();
    let replacement = verify_remote_transaction_with_payload(
        &mut authority,
        replacement_tx.clone(),
        64,
        resolved_payload_with_facts(
            &replacement_tx,
            Vec::new(),
            vec![conflicted_input],
            Capacity::shannons(10_000),
        ),
    );
    let independent = [
        verify_remote_transaction_with_payload(
            &mut authority,
            tx(65),
            65,
            resolved_payload_with_facts(&tx(65), Vec::new(), Vec::new(), Capacity::shannons(2_000)),
        ),
        verify_remote_transaction_with_payload(
            &mut authority,
            tx(66),
            66,
            resolved_payload_with_facts(&tx(66), Vec::new(), Vec::new(), Capacity::shannons(1_000)),
        ),
    ];
    let before = authority.clocks();
    let batch = independent_batch(
        &authority,
        &[
            replacement.clone(),
            independent[0].clone(),
            independent[1].clone(),
        ],
    );
    let SettlementPlan::CoupledComponent(plan) = authority
        .plan_settlement(&batch)
        .expect("the strongest RBF candidate owns the first coupled component")
    else {
        panic!("the accepted victim must couple the first settlement")
    };
    let (replacement_committed, Some(continuation)) = plan.apply() else {
        panic!("the coupled replacement must retain its weaker Ready tail")
    };
    assert!(matches!(
        authority.entry(&victim),
        Some(OwnedTx::ReplacementHistory(_))
    ));

    let SettlementPlan::IndependentRun(plan) = authority
        .plan_coupled_continuation(continuation)
        .expect("the unrelated tail is replanned against the committed replacement")
    else {
        panic!("the unrelated tail must return to the canonical independent planner")
    };
    assert_eq!(
        plan.independent_order_for_foundation()
            .expect("the exact independent tail order is observable in tests")
            .len(),
        independent.len()
    );
    let independent_committed = apply_plan_for_delta(plan);

    assert!(matches!(
        authority.entry(&replacement),
        Some(OwnedTx::Accepted(_))
    ));
    for hash in independent {
        assert!(matches!(authority.entry(&hash), Some(OwnedTx::Accepted(_))));
    }
    assert_eq!(
        authority.clocks().next_sequence,
        ApplySequence(before.next_sequence.0 + 2),
        "one coupled Apply followed by one independent-tail Apply consumes two stamps"
    );
    drop((replacement_committed, independent_committed));
    assert_resource_reference(&authority);
    assert_membership_reference(&authority);
}

#[test]
fn uak_independent_rbf_churn_never_exceeds_replacement_history_budget() {
    let probe_input = OutPoint::new(Byte32::new([90; 32]), 0);
    let probe = TransactionBuilder::default()
        .version(90u32)
        .input(CellInput::new(probe_input, 0))
        .build();
    let small_history_bytes = probe
        .data()
        .total_size()
        .checked_mul(4)
        .expect("four fixture transactions fit usize");
    let limits = ResourceLimits::new(
        ResourceVector::new(16, 1024 * 1024, 128, 8),
        ResourceVector::new(16, 1024 * 1024, 128, 8),
        ResourceVector::new(2, 128 * 1024, 16, 2),
        AcceptedResources::new(16, 1024 * 1024, 1024 * 1024, 1_000_000),
        ComputeLimits::new(128 * 1024, 128 * 1024, 128),
    )
    .and_then(|limits| {
        limits.with_replacement_history_limit(ResourceVector::new(4, small_history_bytes, 64, 0))
    })
    .expect("independent-RBF fixture has a valid hard history partition");
    let mut authority = TxPoolAuthority::with_replacement(limits, FeeRate::from_u64(1_000));
    let mut winners = Vec::new();

    for offset in 0u8..6 {
        let version = 90u32 + u32::from(offset);
        let input = OutPoint::new(Byte32::new([90 + offset; 32]), 0);
        let victim_tx = if offset == 3 {
            TransactionBuilder::default()
                .version(version)
                .input(CellInput::new(input.clone(), 0))
                .output(CellOutput::default())
                .output_data(Bytes::from(vec![0; small_history_bytes + 1024]).pack())
                .build()
        } else {
            TransactionBuilder::default()
                .version(version)
                .input(CellInput::new(input.clone(), 0))
                .build()
        };
        let victim = accept_remote_transaction_with_payload(
            &mut authority,
            victim_tx.clone(),
            usize::from(offset) + 90,
            AcceptedStatus::Pending,
            resolved_payload_with_facts(
                &victim_tx,
                Vec::new(),
                vec![input.clone()],
                Capacity::shannons(100),
            ),
        );
        let replacement_tx = TransactionBuilder::default()
            .version(version + 100)
            .input(CellInput::new(input.clone(), 0))
            .build();
        let replacement = verify_remote_transaction_with_payload(
            &mut authority,
            replacement_tx.clone(),
            usize::from(offset) + 190,
            resolved_payload_with_facts(
                &replacement_tx,
                Vec::new(),
                vec![input],
                Capacity::shannons(10_000),
            ),
        );
        let replacement_version = owner_version(&authority, &replacement);
        apply_plan(
            authority
                .plan_accept_for_foundation(
                    &replacement,
                    replacement_version,
                    AcceptedStatus::Pending,
                )
                .expect("history pressure never rejects the funded winner"),
        );
        winners.push(replacement);

        let expected_history = match offset {
            0..=2 => usize::from(offset) + 1,
            3 => 3,
            _ => 4,
        };
        assert_eq!(
            authority.resources().replacement_history().entries,
            expected_history
        );
        if offset == 3 || offset == 5 {
            assert!(
                authority.entry(&victim).is_none(),
                "the optional victim history must terminalize when its byte or entry bound is full"
            );
        } else {
            assert!(matches!(
                authority.entry(&victim),
                Some(OwnedTx::ReplacementHistory(_))
            ));
        }
        assert_resource_reference(&authority);
    }

    assert_eq!(
        authority.resources().replacement_history(),
        ResourceVector::new(4, small_history_bytes, 4, 0)
    );
    assert!(
        winners
            .iter()
            .all(|winner| matches!(authority.entry(winner), Some(OwnedTx::Accepted(_))))
    );
}

#[test]
fn uak_optional_history_resource_fallback_discards_only_its_clock_branch() {
    let no_history = limits()
        .with_replacement_history_limit(ResourceVector::new(0, 0, 0, 0))
        .expect("zero optional history is a valid bounded configuration");
    let mut authority = TxPoolAuthority::with_replacement(no_history, FeeRate::from_u64(1_000));
    let input = OutPoint::new(Byte32::new([196; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(196u32)
        .input(CellInput::new(input.clone(), 0))
        .build();
    let victim = accept_remote_transaction_with_payload(
        &mut authority,
        victim_tx.clone(),
        196,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &victim_tx,
            Vec::new(),
            vec![input.clone()],
            Capacity::shannons(100),
        ),
    );
    let replacement_tx = TransactionBuilder::default()
        .version(197u32)
        .input(CellInput::new(input.clone(), 0))
        .build();
    let replacement = verify_remote_transaction_with_payload(
        &mut authority,
        replacement_tx.clone(),
        197,
        resolved_payload_with_facts(
            &replacement_tx,
            Vec::new(),
            vec![input],
            Capacity::shannons(10_000),
        ),
    );
    let before = authority.clocks();
    let replacement_version = owner_version(&authority, &replacement);

    apply_plan(
        authority
            .plan_accept_for_foundation(&replacement, replacement_version, AcceptedStatus::Pending)
            .expect("optional history pressure cannot reject the funded winner"),
    );

    let after = authority.clocks();
    assert_eq!(
        after.next_version,
        EntryVersion(before.next_version.0 + 1),
        "only the mandatory winner replacement owns a version"
    );
    assert_eq!(
        after.next_arrival, before.next_arrival,
        "terminalized optional history owns no arrival"
    );
    assert_eq!(
        after.next_sequence,
        ApplySequence(before.next_sequence.0 + 1),
        "the winning membership transition remains one Apply"
    );
    assert!(authority.entry(&victim).is_none());
    assert!(matches!(
        authority.entry(&replacement),
        Some(OwnedTx::Accepted(_))
    ));
    assert_resource_reference(&authority);
}

#[test]
fn uak_replacement_history_reserves_raw_edges_until_wake() {
    let limits = ResourceLimits::new(
        ResourceVector::new(8, 1024 * 1024, 4, 4),
        ResourceVector::new(8, 1024 * 1024, 4, 4),
        ResourceVector::new(2, 128 * 1024, 4, 2),
        AcceptedResources::new(8, 1024 * 1024, 1024 * 1024, 1_000_000),
        ComputeLimits::new(128 * 1024, 128 * 1024, 64),
    )
    .and_then(|limits| {
        limits.with_replacement_history_limit(ResourceVector::new(4, 1024 * 1024, 4, 0))
    })
    .expect("fixture has one exact four-edge history partition");
    let mut authority = TxPoolAuthority::with_replacement(limits, FeeRate::from_u64(1_000));
    let released_input = OutPoint::new(Byte32::new([91; 32]), 0);
    let retained_input = OutPoint::new(Byte32::new([92; 32]), 0);

    // CKB permits one cell to appear in different roles. The dependency
    // frontier canonicalizes this input + cell-dep to one key, but Recovery
    // admission still charges both encoded edges.
    let oldest_tx = TransactionBuilder::default()
        .version(91u32)
        .input(CellInput::new(released_input.clone(), 0))
        .cell_dep(
            CellDep::new_builder()
                .out_point(released_input.clone())
                .build(),
        )
        .build();
    let oldest = accept_remote_transaction_with_payload(
        &mut authority,
        oldest_tx.clone(),
        91,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &oldest_tx,
            Vec::new(),
            vec![released_input.clone()],
            Capacity::shannons(100),
        ),
    );
    let retained_tx = TransactionBuilder::default()
        .version(92u32)
        .input(CellInput::new(retained_input.clone(), 0))
        .build();
    let retained = accept_remote_transaction_with_payload(
        &mut authority,
        retained_tx.clone(),
        92,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &retained_tx,
            Vec::new(),
            vec![retained_input.clone()],
            Capacity::shannons(100),
        ),
    );

    let middle_tx = TransactionBuilder::default()
        .version(93u32)
        .input(CellInput::new(released_input.clone(), 0))
        .input(CellInput::new(retained_input.clone(), 0))
        .build();
    let middle = verify_remote_transaction_with_payload(
        &mut authority,
        middle_tx.clone(),
        93,
        resolved_payload_with_facts(
            &middle_tx,
            Vec::new(),
            vec![released_input, retained_input.clone()],
            Capacity::shannons(10_000),
        ),
    );
    let middle_version = owner_version(&authority, &middle);
    apply_plan(
        authority
            .plan_accept_for_foundation(&middle, middle_version, AcceptedStatus::Pending)
            .expect("the first closure fits its exact retained edge charge"),
    );
    assert_eq!(authority.resources().replacement_history().edges, 3);

    let newest_tx = TransactionBuilder::default()
        .version(94u32)
        .input(CellInput::new(retained_input.clone(), 0))
        .build();
    let newest = verify_remote_transaction_with_payload(
        &mut authority,
        newest_tx.clone(),
        94,
        resolved_payload_with_facts(
            &newest_tx,
            Vec::new(),
            vec![retained_input],
            Capacity::shannons(20_000),
        ),
    );
    let newest_version = owner_version(&authority, &newest);
    apply_plan(
        authority
            .plan_accept_for_foundation(&newest, newest_version, AcceptedStatus::Pending)
            .expect("history pressure cannot reject the second funded winner"),
    );

    // The new victim would exceed the hard edge partition and is therefore
    // terminalized. The two older histories retain exactly the charge needed
    // for the released owner to become Recovery without capacity retry.
    assert!(authority.entry(&middle).is_none());
    assert!(matches!(
        authority.entry(&retained),
        Some(OwnedTx::ReplacementHistory(_))
    ));
    assert_eq!(authority.resources().replacement_history().edges, 3);
    drain_fixture_dependency(&mut authority);
    assert!(matches!(
        authority.entry(&oldest),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Recovery(_))
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert_eq!(authority.resources().replacement_history().edges, 1);
    assert_resource_reference(&authority);
}

#[test]
fn uak_replacement_history_observes_only_finally_unavailable_dependencies() {
    let mut authority = TxPoolAuthority::with_replacement(limits(), FeeRate::from_u64(1_000));
    let conflicting_input = OutPoint::new(Byte32::new([91; 32]), 0);
    let unrelated_input = OutPoint::new(Byte32::new([92; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(91u32)
        .input(CellInput::new(conflicting_input.clone(), 0))
        .input(CellInput::new(unrelated_input.clone(), 0))
        .build();
    let victim = accept_remote_transaction_with_payload(
        &mut authority,
        victim_tx.clone(),
        91,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &victim_tx,
            Vec::new(),
            vec![conflicting_input.clone(), unrelated_input.clone()],
            Capacity::shannons(100),
        ),
    );
    let winner_tx = TransactionBuilder::default()
        .version(92u32)
        .input(CellInput::new(conflicting_input.clone(), 0))
        .build();
    let winner = verify_remote_transaction_with_payload(
        &mut authority,
        winner_tx.clone(),
        92,
        resolved_payload_with_facts(
            &winner_tx,
            Vec::new(),
            vec![conflicting_input.clone()],
            Capacity::shannons(10_000),
        ),
    );
    let winner_version = owner_version(&authority, &winner);
    apply_plan(
        authority
            .plan_accept_for_foundation(&winner, winner_version, AcceptedStatus::Pending)
            .expect("the funded replacement retains its victim"),
    );

    let Some(OwnedTx::ReplacementHistory(history)) = authority.entry(&victim) else {
        panic!("the accepted victim must become replacement history");
    };
    assert!(
        history
            .observation()
            .contains(&DependencyKey::Cell(conflicting_input.clone()))
    );
    assert!(
        !history
            .observation()
            .contains(&DependencyKey::Cell(unrelated_input.clone()))
    );
    assert!(
        history
            .dependencies()
            .contains(&DependencyKey::Cell(unrelated_input.clone())),
        "the full recovery basis remains retained even when it is not a wake trigger"
    );

    if let Some(unrelated) = authority
        .plan_dependency_availability_for_foundation(vec![DependencyKey::Cell(unrelated_input)])
        .expect("an unrelated chain level remains coherent")
    {
        apply_plan(unrelated);
    }
    drain_fixture_dependency(&mut authority);
    assert!(matches!(
        authority.entry(&victim),
        Some(OwnedTx::ReplacementHistory(_))
    ));

    let released = authority
        .plan_dependency_availability_for_foundation(vec![DependencyKey::Cell(conflicting_input)])
        .expect("the conflicting input availability plans")
        .expect("the exact history trigger has an indexed waiter");
    apply_plan(released);
    drain_fixture_dependency(&mut authority);
    assert!(matches!(
        authority.entry(&victim),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Recovery(_))
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert_resource_reference(&authority);
}

#[test]
fn uak_replacement_history_proposal_promotion_uses_the_shared_owner_cut() {
    let mut authority = TxPoolAuthority::with_replacement(limits(), FeeRate::from_u64(1_000));
    let conflicting_input = OutPoint::new(Byte32::new([96; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .input(CellInput::new(conflicting_input.clone(), 0))
        .build();
    let victim = accept_remote_transaction_with_payload(
        &mut authority,
        victim_tx.clone(),
        96,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &victim_tx,
            Vec::new(),
            vec![conflicting_input.clone()],
            Capacity::shannons(100),
        ),
    );
    let winner_tx = TransactionBuilder::default()
        .version(1u32)
        .input(CellInput::new(conflicting_input.clone(), 0))
        .build();
    let winner = verify_remote_transaction_with_payload(
        &mut authority,
        winner_tx.clone(),
        97,
        resolved_payload_with_facts(
            &winner_tx,
            Vec::new(),
            vec![conflicting_input],
            Capacity::shannons(10_000),
        ),
    );
    apply_plan(
        authority
            .plan_accept_for_foundation(
                &winner,
                owner_version(&authority, &winner),
                AcceptedStatus::Pending,
            )
            .expect("the funded replacement retains its victim"),
    );
    let history_version = match authority.entry(&victim) {
        Some(OwnedTx::ReplacementHistory(history)) => history.record().version,
        _ => panic!("the accepted victim becomes replacement history"),
    };

    let attempt = RetainedIngressAttempt::Validated(proposal_for_foundation(victim_tx));
    let batch = RetainedAdmissionBatch::new(attempt, VecDeque::new())
        .expect("one Proposal recovery is a homogeneous batch");
    let promotion = authority
        .compile_shared_retained_ingress_batch(&batch)
        .expect("the history Proposal promotion compiles")
        .expect("the history Proposal promotion has the shared shape");
    drop(
        promotion
            .bind(&authority)
            .expect("the generation remains current")
            .apply()
            .expect("the exact history promotion prestate commits"),
    );
    assert!(matches!(
        authority.entry(&victim),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Proposal { .. })
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
                && entry.record.version != history_version
    ));
    assert!(matches!(
        authority.entry(&winner),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_replacement_history_waits_for_every_observed_blocker() {
    let mut authority = TxPoolAuthority::with_replacement(limits(), FeeRate::from_u64(1_000));
    let first_input = OutPoint::new(Byte32::new([93; 32]), 0);
    let second_input = OutPoint::new(Byte32::new([94; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(93u32)
        .input(CellInput::new(first_input.clone(), 0))
        .input(CellInput::new(second_input.clone(), 0))
        .build();
    let victim = accept_remote_transaction_with_payload(
        &mut authority,
        victim_tx.clone(),
        93,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &victim_tx,
            Vec::new(),
            vec![first_input.clone(), second_input.clone()],
            Capacity::shannons(100),
        ),
    );
    let winner_tx = TransactionBuilder::default()
        .version(94u32)
        .input(CellInput::new(first_input.clone(), 0))
        .input(CellInput::new(second_input.clone(), 0))
        .build();
    let winner = verify_remote_transaction_with_payload(
        &mut authority,
        winner_tx.clone(),
        94,
        resolved_payload_with_facts(
            &winner_tx,
            Vec::new(),
            vec![first_input.clone(), second_input.clone()],
            Capacity::shannons(10_000),
        ),
    );
    let winner_version = owner_version(&authority, &winner);
    apply_plan(
        authority
            .plan_accept_for_foundation(&winner, winner_version, AcceptedStatus::Pending)
            .expect("the funded winner retains its two-input victim"),
    );

    let Some(OwnedTx::ReplacementHistory(history)) = authority.entry(&victim) else {
        panic!("the accepted victim must become replacement history");
    };
    assert_eq!(history.observation().len(), 2);

    let first_release = authority
        .plan_dependency_availability_for_foundation(vec![DependencyKey::Cell(first_input)])
        .expect("the first exact blocker has a coherent availability plan")
        .expect("the first exact blocker has an indexed history waiter");
    apply_plan(first_release);
    drain_fixture_dependency(&mut authority);
    assert!(
        matches!(
            authority.entry(&victim),
            Some(OwnedTx::ReplacementHistory(_))
        ),
        "one released input cannot consume history while another blocker remains"
    );

    let second_release = authority
        .plan_dependency_availability_for_foundation(vec![DependencyKey::Cell(second_input)])
        .expect("the second exact blocker has a coherent availability plan")
        .expect("the second exact blocker has an indexed history waiter");
    apply_plan(second_release);
    drain_fixture_dependency(&mut authority);
    assert!(matches!(
        authority.entry(&victim),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Recovery(_))
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert_resource_reference(&authority);
}

#[test]
fn uak_replacement_history_wakes_only_on_newer_projected_availability() {
    let mut authority = TxPoolAuthority::with_replacement(limits(), FeeRate::from_u64(1_000));
    let released_input = OutPoint::new(Byte32::new([81; 32]), 0);
    let retained_input = OutPoint::new(Byte32::new([82; 32]), 0);

    let oldest_tx = TransactionBuilder::default()
        .version(81u32)
        .input(CellInput::new(released_input.clone(), 0))
        .build();
    let oldest = accept_remote_transaction_with_payload(
        &mut authority,
        oldest_tx.clone(),
        81,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &oldest_tx,
            Vec::new(),
            vec![released_input.clone()],
            Capacity::shannons(100),
        ),
    );

    let retained_tx = TransactionBuilder::default()
        .version(84u32)
        .input(CellInput::new(retained_input.clone(), 0))
        .build();
    let retained = accept_remote_transaction_with_payload(
        &mut authority,
        retained_tx.clone(),
        84,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &retained_tx,
            Vec::new(),
            vec![retained_input.clone()],
            Capacity::shannons(100),
        ),
    );

    let middle_tx = TransactionBuilder::default()
        .version(82u32)
        .input(CellInput::new(released_input.clone(), 0))
        .input(CellInput::new(retained_input.clone(), 0))
        .build();
    let middle = verify_remote_transaction_with_payload(
        &mut authority,
        middle_tx.clone(),
        82,
        resolved_payload_with_facts(
            &middle_tx,
            Vec::new(),
            vec![released_input, retained_input.clone()],
            Capacity::shannons(10_000),
        ),
    );
    let middle_version = owner_version(&authority, &middle);
    apply_plan(
        authority
            .plan_accept_for_foundation(&middle, middle_version, AcceptedStatus::Pending)
            .expect("first replacement retains the accepted victim"),
    );
    assert!(matches!(
        authority.entry(&oldest),
        Some(OwnedTx::ReplacementHistory(_))
    ));
    assert!(matches!(
        authority.entry(&retained),
        Some(OwnedTx::ReplacementHistory(_))
    ));

    let newest_tx = TransactionBuilder::default()
        .version(83u32)
        .input(CellInput::new(retained_input, 0))
        .build();
    let newest = verify_remote_transaction_with_payload(
        &mut authority,
        newest_tx.clone(),
        83,
        resolved_payload_with_facts(
            &newest_tx,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(20_000),
        ),
    );
    let newest_version = owner_version(&authority, &newest);
    apply_plan(
        authority
            .plan_accept_for_foundation(&newest, newest_version, AcceptedStatus::Pending)
            .expect("second replacement releases only its projected free input"),
    );

    assert!(matches!(
        authority.entry(&middle),
        Some(OwnedTx::ReplacementHistory(_))
    ));
    assert!(matches!(
        authority.entry(&retained),
        Some(OwnedTx::ReplacementHistory(_))
    ));
    assert!(matches!(
        authority.entry(&oldest),
        Some(OwnedTx::ReplacementHistory(_))
    ));

    drain_fixture_dependency(&mut authority);
    assert!(matches!(
        authority.entry(&oldest),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(
                entry.source,
                PreAcceptedSource::Recovery(lease) if lease.generation == PoolGeneration(0)
            ) && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(matches!(
        authority.entry(&middle),
        Some(OwnedTx::ReplacementHistory(_))
    ));
    assert!(matches!(
        authority.entry(&retained),
        Some(OwnedTx::ReplacementHistory(_))
    ));
    assert_eq!(authority.resources().replacement_history().entries, 2);
    assert_resource_reference(&authority);
}

#[test]
fn uak_replacement_history_requires_trusted_proposal_to_promote() {
    let mut authority = TxPoolAuthority::with_replacement(limits(), FeeRate::from_u64(1_000));
    let chain_input = OutPoint::new(Byte32::new([85; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(85u32)
        .input(CellInput::new(chain_input.clone(), 0))
        .build();
    let victim = accept_remote_transaction_with_payload(
        &mut authority,
        victim_tx.clone(),
        85,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &victim_tx,
            Vec::new(),
            vec![chain_input.clone()],
            Capacity::shannons(100),
        ),
    );
    let winner_tx = TransactionBuilder::default()
        .version(86u32)
        .input(CellInput::new(chain_input.clone(), 0))
        .build();
    let winner = verify_remote_transaction_with_payload(
        &mut authority,
        winner_tx.clone(),
        86,
        resolved_payload_with_facts(
            &winner_tx,
            Vec::new(),
            vec![chain_input],
            Capacity::shannons(10_000),
        ),
    );
    let winner_version = owner_version(&authority, &winner);
    apply_plan(
        authority
            .plan_accept_for_foundation(&winner, winner_version, AcceptedStatus::Pending)
            .expect("the funded winner retains its victim"),
    );
    assert!(matches!(
        authority.entry(&victim),
        Some(OwnedTx::ReplacementHistory(_))
    ));

    let remote_retry = ValidatedAdmission::remote(victim_tx.clone(), PeerIndex::from(87))
        .expect("same-witness remote retry is valid ingress");
    assert_eq!(
        authority.plan_admission(remote_retry).err(),
        Some(PlanError::Duplicate)
    );
    assert!(matches!(
        authority.entry(&victim),
        Some(OwnedTx::ReplacementHistory(_))
    ));

    let proposal =
        ValidatedAdmission::proposal(victim_tx).expect("trusted proposal retry is valid ingress");
    apply_plan(
        authority
            .plan_admission(proposal)
            .expect("only the trusted proposal lease promotes history"),
    );
    assert!(matches!(
        authority.entry(&victim),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(
                entry.source,
                PreAcceptedSource::Proposal {
                    base: ProposalBase::Trusted,
                    ..
                }
            ) && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(matches!(
        authority.entry(&winner),
        Some(OwnedTx::Accepted(_))
    ));
    assert_eq!(authority.resources().replacement_history().entries, 0);
    assert_resource_reference(&authority);
}

#[test]
fn uak_rbf_removal_subtracts_deep_descendants_from_a_surviving_ancestor() {
    let mut authority = TxPoolAuthority::with_replacement(limits(), FeeRate::from_u64(1_000));
    let ancestor_input = OutPoint::new(Byte32::new([71; 32]), 0);
    let conflict_input = OutPoint::new(Byte32::new([72; 32]), 0);
    let ancestor_tx = TransactionBuilder::default()
        .version(71u32)
        .input(CellInput::new(ancestor_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let ancestor = accept_remote_transaction_with_payload(
        &mut authority,
        ancestor_tx.clone(),
        71,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &ancestor_tx,
            Vec::new(),
            vec![ancestor_input],
            Capacity::shannons(100),
        ),
    );
    let victim_tx = TransactionBuilder::default()
        .version(72u32)
        .input(CellInput::new(OutPoint::new(ancestor_tx.hash(), 0), 0))
        .input(CellInput::new(conflict_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let victim = accept_remote_transaction_with_payload(
        &mut authority,
        victim_tx.clone(),
        72,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &victim_tx,
            Vec::new(),
            vec![conflict_input.clone()],
            Capacity::shannons(100),
        ),
    );
    let child_tx = TransactionBuilder::default()
        .version(73u32)
        .input(CellInput::new(OutPoint::new(victim_tx.hash(), 0), 0))
        .build();
    let child = accept_remote_transaction_with_payload(
        &mut authority,
        child_tx.clone(),
        73,
        AcceptedStatus::Gap,
        resolved_payload_with_facts(&child_tx, Vec::new(), Vec::new(), Capacity::shannons(100)),
    );
    let replacement_tx = TransactionBuilder::default()
        .version(74u32)
        .input(CellInput::new(conflict_input.clone(), 0))
        .build();
    let replacement = verify_remote_transaction_with_payload(
        &mut authority,
        replacement_tx.clone(),
        74,
        resolved_payload_with_facts(
            &replacement_tx,
            Vec::new(),
            vec![conflict_input],
            Capacity::shannons(10_000),
        ),
    );

    let version = owner_version(&authority, &replacement);
    let committed = apply_plan_for_delta(
        authority
            .plan_accept_for_foundation(&replacement, version, AcceptedStatus::Pending)
            .expect("replacement removes the victim closure below a surviving ancestor"),
    );

    assert_eq!(committed.removals.len(), 2);
    assert!(matches!(
        authority.entry(&victim),
        Some(OwnedTx::ReplacementHistory(_))
    ));
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::ReplacementHistory(_))
    ));
    assert!(matches!(
        authority.entry(&ancestor),
        Some(OwnedTx::Accepted(_))
    ));
    assert_eq!(
        authority
            .membership_snapshot_for_reference()
            .descendant_aggregates[&ancestor]
            .entries,
        1,
        "the surviving ancestor must not retain removed descendant weight"
    );
    assert_membership_reference(&authority);
    assert_resource_reference(&authority);
}

#[test]
fn uak_rbf_unions_fan_in_descendants_once_and_removes_children_first() {
    let mut authority = TxPoolAuthority::with_replacement(limits(), FeeRate::from_u64(1_000));
    let left_input = OutPoint::new(Byte32::new([228; 32]), 0);
    let right_input = OutPoint::new(Byte32::new([229; 32]), 0);
    let left_tx = TransactionBuilder::default()
        .version(228u32)
        .input(CellInput::new(left_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let right_tx = TransactionBuilder::default()
        .version(229u32)
        .input(CellInput::new(right_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let left = accept_remote_transaction_with_payload(
        &mut authority,
        left_tx.clone(),
        228,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &left_tx,
            Vec::new(),
            vec![left_input.clone()],
            Capacity::shannons(100),
        ),
    );
    let right = accept_remote_transaction_with_payload(
        &mut authority,
        right_tx.clone(),
        229,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &right_tx,
            Vec::new(),
            vec![right_input.clone()],
            Capacity::shannons(100),
        ),
    );
    let left_child_tx = TransactionBuilder::default()
        .version(230u32)
        .input(CellInput::new(OutPoint::new(left_tx.hash(), 0), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let right_child_tx = TransactionBuilder::default()
        .version(231u32)
        .input(CellInput::new(OutPoint::new(right_tx.hash(), 0), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let left_child = accept_remote_transaction_with_payload(
        &mut authority,
        left_child_tx.clone(),
        230,
        AcceptedStatus::Gap,
        resolved_payload_with_facts(
            &left_child_tx,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(100),
        ),
    );
    let right_child = accept_remote_transaction_with_payload(
        &mut authority,
        right_child_tx.clone(),
        231,
        AcceptedStatus::Gap,
        resolved_payload_with_facts(
            &right_child_tx,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(100),
        ),
    );
    let merge_tx = TransactionBuilder::default()
        .version(232u32)
        .input(CellInput::new(OutPoint::new(left_child_tx.hash(), 0), 0))
        .input(CellInput::new(OutPoint::new(right_child_tx.hash(), 0), 0))
        .build();
    let merge = accept_remote_transaction_with_payload(
        &mut authority,
        merge_tx.clone(),
        232,
        AcceptedStatus::Proposed,
        resolved_payload_with_facts(&merge_tx, Vec::new(), Vec::new(), Capacity::shannons(100)),
    );

    let replacement_tx = TransactionBuilder::default()
        .version(233u32)
        .input(CellInput::new(left_input, 0))
        .input(CellInput::new(right_input, 0))
        .build();
    let replacement = verify_remote_transaction_with_payload(
        &mut authority,
        replacement_tx.clone(),
        233,
        resolved_payload_with_facts(
            &replacement_tx,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(10_000),
        ),
    );
    let version = owner_version(&authority, &replacement);
    let committed = apply_plan_for_delta(
        authority
            .plan_accept_for_foundation(&replacement, version, AcceptedStatus::Pending)
            .expect("one virtual component unions both direct-conflict trees"),
    );

    assert_eq!(committed.removals.len(), 5);
    assert_eq!(
        committed.retired_len(),
        committed.removals.len().saturating_add(1)
    );
    assert!(
        committed
            .removals
            .iter()
            .all(|removal| removal.cause == RemovalCause::Replacement)
    );
    let positions = committed
        .removals
        .iter()
        .enumerate()
        .map(|(position, removal)| (removal.hash.clone(), position))
        .collect::<HashMap<_, _>>();
    assert_eq!(positions.len(), 5, "shared descendant is removed once");
    assert!(positions[&merge] < positions[&left_child]);
    assert!(positions[&merge] < positions[&right_child]);
    assert!(positions[&left_child] < positions[&left]);
    assert!(positions[&right_child] < positions[&right]);
    assert!(
        committed
            .removals
            .iter()
            .all(|removal| authority.entry(&removal.hash).is_none()),
        "a five-entry closure cannot partially occupy the four-entry history partition"
    );
    assert_eq!(authority.owner_count(), 1);
    assert_eq!(authority.resources().replacement_history().entries, 0);
    assert!(matches!(
        authority.entry(&replacement),
        Some(OwnedTx::Accepted(_))
    ));
    assert_resource_reference(&authority);
}

#[test]
fn uak_failed_rbf_fee_disposition_preserves_victims_and_terminalizes_candidate() {
    let mut authority = TxPoolAuthority::with_replacement(limits(), FeeRate::from_u64(1_000));
    let chain_input = OutPoint::new(Byte32::new([61; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(61u32)
        .input(CellInput::new(chain_input.clone(), 0))
        .build();
    let victim_payload = resolved_payload_with_facts(
        &victim_tx,
        Vec::new(),
        vec![chain_input.clone()],
        Capacity::shannons(5_000),
    );
    let victim = accept_remote_transaction_with_payload(
        &mut authority,
        victim_tx,
        61,
        AcceptedStatus::Gap,
        victim_payload,
    );
    let replacement_tx = TransactionBuilder::default()
        .version(62u32)
        .input(CellInput::new(chain_input, 0))
        .build();
    let replacement_payload = resolved_payload_with_facts(
        &replacement_tx,
        Vec::new(),
        Vec::new(),
        Capacity::shannons(1),
    );
    let replacement = verify_remote_transaction_with_payload(
        &mut authority,
        replacement_tx,
        62,
        replacement_payload,
    );
    let before = authority.normalized_snapshot();
    let version = owner_version(&authority, &replacement);

    assert!(matches!(
        authority
            .plan_accept_for_foundation(&replacement, version, AcceptedStatus::Pending)
            .err(),
        Some(PlanError::Membership(
            MembershipReject::InsufficientReplacementFee { actual, .. }
        )) if actual == Capacity::shannons(1)
    ));
    assert_eq!(authority.normalized_snapshot(), before);

    let disposition = authority
        .plan_candidate_disposition_for_foundation(&replacement, version, AcceptedStatus::Pending)
        .expect("one driver round compiles a deterministic rejection");
    let CandidateDispositionPlan::Rejected(rejection) = disposition else {
        panic!("under-fee replacement cannot be accepted");
    };
    assert!(matches!(
        rejection.reason(),
        MembershipReject::InsufficientReplacementFee { actual, .. }
            if *actual == Capacity::shannons(1)
    ));
    let (reason, committed) = rejection.apply();
    assert!(matches!(
        reason,
        MembershipReject::InsufficientReplacementFee { actual, .. }
            if actual == Capacity::shannons(1)
    ));
    assert!(committed.removals.is_empty());
    assert!(matches!(
        authority.entry(&victim),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(authority.entry(&replacement).is_none());
    assert_eq!(authority.resources().replacement_history().entries, 0);
    assert_resource_reference(&authority);

    let lease = authority
        .effect_publication_receipt_for_foundation()
        .expect("the terminal Apply committed its rejection");
    assert!(matches!(
        lease.effects(),
        [CommittedEffect::Rejected(CommittedRejection::Membership {
            audience,
            reason: MembershipReject::InsufficientReplacementFee { actual, .. },
            ..
        })] if audience.ingress_peer() == Some(PeerIndex::from(62))
            && *actual == Capacity::shannons(1)
    ));
    let published = authority
        .apply_effect_settlement_for_foundation(lease.complete_for_foundation().published())
        .expect("rejection publication settles");
    assert_eq!(published.retired_effect_len(), 1);
}

#[test]
fn uak_rbf_requires_positive_chain_evidence_for_every_new_input() {
    let mut authority = TxPoolAuthority::with_replacement(limits(), FeeRate::from_u64(1_000));
    let replaced_input = OutPoint::new(Byte32::new([63; 32]), 0);
    let new_input = OutPoint::new(Byte32::new([64; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(63u32)
        .input(CellInput::new(replaced_input.clone(), 0))
        .build();
    let victim_payload = resolved_payload_with_facts(
        &victim_tx,
        Vec::new(),
        vec![replaced_input.clone()],
        Capacity::shannons(100),
    );
    accept_remote_transaction_with_payload(
        &mut authority,
        victim_tx,
        63,
        AcceptedStatus::Pending,
        victim_payload,
    );
    let replacement_tx = TransactionBuilder::default()
        .version(64u32)
        .input(CellInput::new(replaced_input, 0))
        .input(CellInput::new(new_input.clone(), 0))
        .build();
    let replacement_payload = resolved_payload_with_facts(
        &replacement_tx,
        Vec::new(),
        Vec::new(),
        Capacity::shannons(10_000),
    );
    let replacement = verify_remote_transaction_with_payload(
        &mut authority,
        replacement_tx,
        64,
        replacement_payload,
    );
    let before = authority.normalized_snapshot();
    let version = owner_version(&authority, &replacement);

    assert_eq!(
        authority
            .plan_accept_for_foundation(&replacement, version, AcceptedStatus::Pending)
            .err(),
        Some(PlanError::Membership(
            MembershipReject::NewUnconfirmedInput(new_input)
        ))
    );
    assert_eq!(authority.normalized_snapshot(), before);
    assert_resource_reference(&authority);
}

#[test]
fn uak_rbf_accepts_new_input_only_with_positive_chain_evidence() {
    let mut authority = TxPoolAuthority::with_replacement(limits(), FeeRate::from_u64(1_000));
    let replaced_input = OutPoint::new(Byte32::new([71; 32]), 0);
    let confirmed_input = OutPoint::new(Byte32::new([72; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(71u32)
        .input(CellInput::new(replaced_input.clone(), 0))
        .build();
    let victim_payload = resolved_payload_with_facts(
        &victim_tx,
        Vec::new(),
        vec![replaced_input.clone()],
        Capacity::shannons(100),
    );
    let victim = accept_remote_transaction_with_payload(
        &mut authority,
        victim_tx,
        71,
        AcceptedStatus::Pending,
        victim_payload,
    );
    let replacement_tx = TransactionBuilder::default()
        .version(72u32)
        .input(CellInput::new(replaced_input.clone(), 0))
        .input(CellInput::new(confirmed_input.clone(), 0))
        .build();
    let replacement_payload = resolved_payload_with_facts(
        &replacement_tx,
        Vec::new(),
        vec![confirmed_input.clone()],
        Capacity::shannons(10_000),
    );
    let replacement = verify_remote_transaction_with_payload(
        &mut authority,
        replacement_tx,
        72,
        replacement_payload,
    );
    let version = owner_version(&authority, &replacement);
    apply_plan(
        authority
            .plan_accept_for_foundation(&replacement, version, AcceptedStatus::Pending)
            .expect("positive chain evidence satisfies the no-new-unconfirmed-input rule"),
    );

    assert!(matches!(
        authority.entry(&victim),
        Some(OwnedTx::ReplacementHistory(_))
    ));
    assert_eq!(authority.resources().replacement_history().entries, 1);
    assert_eq!(
        authority.accepted_spender(&replaced_input),
        Some(replacement.clone())
    );
    assert_eq!(
        authority.accepted_spender(&confirmed_input),
        Some(replacement.clone())
    );
    assert_resource_reference(&authority);
}

#[test]
fn uak_rbf_dependency_on_any_victim_is_mutation_free() {
    let mut authority = TxPoolAuthority::with_replacement(limits(), FeeRate::from_u64(1_000));
    let chain_input = OutPoint::new(Byte32::new([73; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(73u32)
        .input(CellInput::new(chain_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let victim_payload = resolved_payload_with_facts(
        &victim_tx,
        Vec::new(),
        vec![chain_input.clone()],
        Capacity::shannons(100),
    );
    let victim = accept_remote_transaction_with_payload(
        &mut authority,
        victim_tx.clone(),
        73,
        AcceptedStatus::Pending,
        victim_payload,
    );
    let victim_dependency = OutPoint::new(victim_tx.hash(), 0);
    let replacement_tx = TransactionBuilder::default()
        .version(74u32)
        .input(CellInput::new(chain_input, 0))
        .build();
    let replacement_payload = resolved_payload_with_facts(
        &replacement_tx,
        vec![victim_dependency.clone()],
        Vec::new(),
        Capacity::shannons(10_000),
    );
    let replacement = verify_remote_transaction_with_payload(
        &mut authority,
        replacement_tx,
        74,
        replacement_payload,
    );
    let before = authority.normalized_snapshot();
    let version = owner_version(&authority, &replacement);

    assert_eq!(
        authority
            .plan_accept_for_foundation(&replacement, version, AcceptedStatus::Pending)
            .err(),
        Some(PlanError::Membership(MembershipReject::DependencyOnVictim(
            victim_dependency
        )))
    );
    assert_eq!(authority.normalized_snapshot(), before);
    assert!(matches!(
        authority.entry(&victim),
        Some(OwnedTx::Accepted(_))
    ));
    assert_resource_reference(&authority);
}

#[test]
fn uak_capacity_never_evicts_a_candidate_ancestor() {
    let bounded =
        limits().with_accepted_for_foundation(AcceptedResources::new(1, 64 * 1024, 64 * 1024, 64));
    let mut authority = TxPoolAuthority::for_foundation(bounded);
    let parent_tx = TransactionBuilder::default()
        .version(75u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent = accept_remote_transaction(
        &mut authority,
        parent_tx.clone(),
        75,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let child_tx = TransactionBuilder::default()
        .version(76u32)
        .input(CellInput::new(OutPoint::new(parent_tx.hash(), 0), 0))
        .build();
    let child_payload = resolved_payload_with_facts(
        &child_tx,
        Vec::new(),
        Vec::new(),
        Capacity::shannons(10_000),
    );
    let child = verify_remote_transaction_with_payload(&mut authority, child_tx, 76, child_payload);
    let before = authority.normalized_snapshot();
    let version = owner_version(&authority, &child);

    assert!(matches!(
        authority
            .plan_accept_for_foundation(&child, version, AcceptedStatus::Proposed)
            .err(),
        Some(PlanError::Membership(
            MembershipReject::CandidateEvicted { .. }
        ))
    ));
    assert_eq!(authority.normalized_snapshot(), before);
    assert!(matches!(
        authority.entry(&parent),
        Some(OwnedTx::Accepted(_))
    ));
    assert_resource_reference(&authority);
}

#[test]
fn uak_rbf_component_bound_stops_before_any_authority_mutation() {
    let bounded = limits().with_accepted_for_foundation(AcceptedResources::new(
        110,
        8 * 1024 * 1024,
        8 * 1024 * 1024,
        64,
    ));
    let mut authority = TxPoolAuthority::with_replacement(bounded, FeeRate::from_u64(1_000));
    let chain_input = OutPoint::new(Byte32::new([77; 32]), 0);
    let root_tx = TransactionBuilder::default()
        .version(77u32)
        .input(CellInput::new(chain_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let root_payload = resolved_payload_with_facts(
        &root_tx,
        Vec::new(),
        vec![chain_input.clone()],
        Capacity::shannons(1),
    );
    accept_remote_transaction_with_payload(
        &mut authority,
        root_tx.clone(),
        77,
        AcceptedStatus::Pending,
        root_payload,
    );
    let mut parent = root_tx;
    for version in 78u32..=176u32 {
        let child = TransactionBuilder::default()
            .version(version)
            .input(CellInput::new(OutPoint::new(parent.hash(), 0), 0))
            .output(CellOutput::default())
            .output_data(Bytes::new().pack())
            .build();
        accept_remote_transaction(
            &mut authority,
            child.clone(),
            usize::try_from(version).expect("fixture peer index fits"),
            AcceptedStatus::Pending,
            Vec::new(),
        );
        parent = child;
    }
    let replacement_tx = TransactionBuilder::default()
        .version(177u32)
        .input(CellInput::new(chain_input, 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let replacement_output = OutPoint::new(replacement_tx.hash(), 0);
    let late_child_tx = TransactionBuilder::default()
        .version(178u32)
        .input(CellInput::new(replacement_output.clone(), 0))
        .build();
    accept_remote_transaction_with_payload(
        &mut authority,
        late_child_tx.clone(),
        178,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &late_child_tx,
            Vec::new(),
            vec![replacement_output],
            Capacity::shannons(1_000),
        ),
    );
    let replacement_payload = resolved_payload_with_facts(
        &replacement_tx,
        Vec::new(),
        Vec::new(),
        Capacity::shannons(1_000_000),
    );
    let replacement = verify_remote_transaction_with_payload(
        &mut authority,
        replacement_tx,
        177,
        replacement_payload,
    );
    let before = authority.normalized_snapshot();
    let version = owner_version(&authority, &replacement);

    assert_eq!(
        authority
            .plan_accept_for_foundation(&replacement, version, AcceptedStatus::Pending)
            .err(),
        Some(PlanError::Membership(MembershipReject::ComponentLimit {
            kind: ComponentLimitKind::Mutation,
            limit: crate::constants::MAX_POOL_MUTATION_CANDIDATES,
        }))
    );
    assert_eq!(authority.normalized_snapshot(), before);
    assert_resource_reference(&authority);
}

#[test]
fn uak_accepted_owner_cannot_bypass_membership_removal() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let accepted = accept_remote_transaction(
        &mut authority,
        tx(65),
        65,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let before = authority.normalized_snapshot();
    let version = owner_version(&authority, &accepted);

    assert_eq!(
        authority
            .plan_terminalize_for_foundation(&accepted, version)
            .err(),
        Some(PlanError::Stale(StalePlan::Phase))
    );
    assert_eq!(authority.normalized_snapshot(), before);
    assert_resource_reference(&authority);
}

#[test]
fn uak_resource_limit_failure_preserves_every_observable_fact() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    for nonce in [8, 9] {
        let plan = authority
            .plan_admission(
                ValidatedAdmission::remote(tx(nonce), PeerIndex::from(21))
                    .expect("fixture admission is valid"),
            )
            .expect("peer capacity holds two entries");
        apply_plan(plan);
    }
    let before = authority.normalized_snapshot();
    let result = authority
        .plan_admission(
            ValidatedAdmission::remote(tx(10), PeerIndex::from(21))
                .expect("fixture admission is valid"),
        )
        .err();
    assert_eq!(
        result,
        Some(PlanError::Backpressure(Backpressure::PeerResources))
    );
    assert_eq!(authority.normalized_snapshot(), before);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_resource_aggregate_has_no_separate_ghost_write_surface() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let _hash = admit_remote(&mut authority, 626, 74);

    assert!(
        authority.resources().semantically_matches(),
        "committed resource aggregates are derived in the same owner-shard transition"
    );
}

#[test]
fn uak_resource_planner_binds_expected_charge_to_primary_owner() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 627, 75);
    let actual = authority
        .entries_for_reference()
        .get(&hash)
        .expect("fixture owner exists")
        .charge_record();
    let before = authority.normalized_snapshot();

    assert_eq!(
        authority
            .resources_for_test_plan()
            .plan_replace(hash, None, Some(actual))
            .err(),
        Some(ResourceError::ExistingChargeMismatch),
        "resource planning must read the expected charge from the primary owner map"
    );
    assert_eq!(authority.normalized_snapshot(), before);
}

#[test]
fn uak_owner_resource_plan_must_match_the_committed_owner_transition() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 628, 76);

    assert!(
        authority.entry(&hash).is_some(),
        "the primary owner insertion must commit"
    );
    assert_resource_reference(&authority);
}

#[test]
fn uak_counter_exhaustion_is_typed_and_mutation_free() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    authority.force_next_sequence(ApplySequence(u128::MAX));
    let before = authority.normalized_snapshot();
    let result = authority
        .plan_admission(
            ValidatedAdmission::remote(tx(11), PeerIndex::from(22))
                .expect("fixture admission is valid"),
        )
        .err();
    assert_eq!(
        result,
        Some(PlanError::Fault(AuthorityFault::CounterExhausted))
    );
    assert_eq!(authority.normalized_snapshot(), before);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_settlement_failure_returns_the_exact_terminal_capability() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 111, 22);
    let version = owner_version(&authority, &hash);
    let (_, work) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(&hash, version, WorkPermit::ResolveOnly)
            .expect("fixture checkout plans")
            .apply(),
    );
    let settlement = work.rejected(RejectionKind::Policy);
    let resumable_sequence = authority.clocks().next_sequence;
    authority.force_next_sequence(ApplySequence(u128::MAX));
    let before = authority.normalized_snapshot();

    let exhausted = authority
        .apply_settlement(settlement)
        .expect_err("counter exhaustion cannot consume the compute capability");
    assert_eq!(
        exhausted.recovery(),
        &ComputeSettlementRecovery::Structural(PlanError::Fault(AuthorityFault::CounterExhausted))
    );
    assert_eq!(authority.normalized_snapshot(), before);

    authority.force_next_sequence(resumable_sequence);
    apply_plan(
        authority
            .apply_settlement(exhausted.into_settlement())
            .expect("returned capability commits the original rejection"),
    );
    assert_eq!(authority.resources().preaccepted().active_work, 0);
    assert!(authority.entry(&hash).is_none());
    let lease = authority
        .effect_publication_receipt_for_foundation()
        .expect("terminalization and rejection publish in the same Apply");
    assert!(matches!(
        lease.effects(),
        [CommittedEffect::Rejected(CommittedRejection::Validation {
            reason: rejection,
            ..
        })] if matches!(
            rejection.reject(),
            Reject::Invalidated(message) if message == "foundation policy rejection"
        )
    ));
    apply_plan(
        authority
            .apply_effect_settlement_for_foundation(lease.complete_for_foundation().published())
            .expect("terminal rejection publication settles"),
    );
    assert_resource_reference(&authority);
}

#[test]
fn uak_dropped_prepared_apply_is_semantically_mutation_free() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let before = authority.normalized_snapshot();
    {
        let prepared = authority
            .plan_admission(
                ValidatedAdmission::remote(tx(24), PeerIndex::from(44))
                    .expect("fixture admission is valid"),
            )
            .expect("admission preflight plans");
        drop(prepared);
    }
    assert!(
        authority
            .normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before, 1, 1, 1),
        "the dropped admission burns exactly its owner identity and Apply stamp"
    );
    assert_resource_reference(&authority);
}

#[test]
fn uak_active_work_backpressure_is_precomputed_and_mutation_free() {
    let limits = ResourceLimits::new(
        ResourceVector::new(4, 64 * 1024, 64, 4),
        ResourceVector::new(4, 64 * 1024, 64, 4),
        ResourceVector::new(4, 64 * 1024, 64, 1),
        AcceptedResources::new(4, 64 * 1024, 64 * 1024, 64),
        ComputeLimits::new(4 * 1024, 4 * 1024, 16),
    )
    .expect("fixture limits admit one indivisible grant");
    let mut authority = TxPoolAuthority::for_foundation(limits);
    let first = admit_remote(&mut authority, 25, 45);
    let second = admit_remote(&mut authority, 26, 45);
    let version = owner_version(&authority, &first);
    let checkout = authority
        .plan_checkout_for_foundation(&first, version, WorkPermit::ResolveOnly)
        .expect("first peer work grant fits")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.into_work() else {
        panic!("resolve-only permit returns resolve work");
    };

    let before = authority.normalized_snapshot();
    let version = owner_version(&authority, &second);
    assert_eq!(
        authority
            .plan_checkout_for_foundation(&second, version, WorkPermit::ResolveOnly)
            .err(),
        Some(PlanError::Backpressure(Backpressure::PeerResources))
    );
    assert_eq!(authority.normalized_snapshot(), before);
    assert_resource_reference(&authority);

    apply_plan(
        authority
            .apply_settlement(resolve.rejected(RejectionKind::Policy))
            .expect("live lease still settles after peer backpressure"),
    );
}

#[test]
fn uak_stale_compute_version_is_mutation_free_across_aba() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(27);
    let first = ValidatedAdmission::remote(transaction.clone(), PeerIndex::from(46))
        .expect("fixture admission is valid");
    let hash = first.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(first)
            .expect("first incarnation plans"),
    );
    let version = owner_version(&authority, &hash);
    let checkout = authority
        .plan_checkout_for_foundation(&hash, version, WorkPermit::ResolveOnly)
        .expect("first incarnation checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.into_work() else {
        panic!("resolve-only permit returns resolve work");
    };

    let settlement = resolve.rejected(RejectionKind::Policy);
    let stale_token = SettlementToken {
        hash: settlement.token.hash.clone(),
        version: settlement.token.version,
    };
    apply_plan(
        authority
            .plan_terminalize_for_foundation(&hash, owner_version(&authority, &hash))
            .expect("active terminalization invalidates the exact owner"),
    );
    assert!(authority.entry(&hash).is_none());
    apply_plan(
        authority
            .plan_admission(
                ValidatedAdmission::remote(transaction, PeerIndex::from(47))
                    .expect("readmission is valid"),
            )
            .expect("same raw hash obtains a fresh incarnation"),
    );
    let before_stale = authority.normalized_snapshot();
    let stale = authority
        .apply_settlement(ComputeSettlement {
            token: stale_token,
            next: SettlementNext::Retry,
        })
        .expect_err("the retired incarnation cannot settle its successor");
    assert_eq!(
        stale.recovery(),
        &ComputeSettlementRecovery::Obsolete(StalePlan::Version)
    );
    assert_eq!(authority.normalized_snapshot(), before_stale);
    assert_eq!(
        authority
            .entry(&hash)
            .expect("new incarnation exists")
            .ingress_peer(),
        Some(PeerIndex::from(47))
    );
    assert_resource_reference(&authority);
}

#[test]
fn uak_checkout_is_move_only_and_exactly_charged() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 12, 31);
    let queued_charge = authority
        .entry(&hash)
        .and_then(|owner| owner.preaccepted_charge())
        .expect("queued owner has an exact retained charge");
    let version = owner_version(&authority, &hash);
    let checkout_sequence = authority.clocks().next_sequence;
    let checkout = authority
        .plan_checkout_for_foundation(
            &hash,
            version,
            WorkPermit::ResolveThenVerify(VerifyCapability::Any),
        )
        .expect("queued resolve accepts a continuous permit")
        .apply();
    assert_eq!(checkout_sequence, ApplySequence(2));
    assert_eq!(authority.clocks().next_sequence, ApplySequence(3));
    let grant = match authority.entry(&hash) {
        Some(OwnedTx::PreAccepted(entry)) => match &entry.phase {
            PreAcceptedPhase::Computing(active) => active.grant,
            PreAcceptedPhase::Queued(_)
            | PreAcceptedPhase::Waiting(_)
            | PreAcceptedPhase::Ready(_) => {
                panic!("checkout fixture owner must be computing")
            }
        },
        Some(OwnedTx::Accepted(_) | OwnedTx::ReplacementHistory(_)) | None => {
            panic!("checkout fixture must retain its preaccepted owner")
        }
    };
    let expected_charge = queued_charge
        .reserve_compute(grant)
        .expect("fixture charge accepts exactly one compute reservation");
    assert_eq!(authority.resources().preaccepted(), expected_charge);
    assert!(authority.primary_projection_consistent());
    let before_local_continuation = authority.normalized_snapshot();
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.into_work() else {
        panic!("continuous permit returns continuous resolve work");
    };
    let payload = resolved_payload(resolve.transaction());
    let (verify, accepted_resident_bytes) = continue_fixture_verify(resolve, payload);
    assert_eq!(authority.normalized_snapshot(), before_local_continuation);
    let settlement = verify.verified(0);
    apply_plan(
        authority
            .apply_settlement(settlement)
            .expect("current continuous lease settles"),
    );
    let retained = authority
        .entry(&hash)
        .and_then(|owner| owner.preaccepted_charge())
        .expect("verified candidate remains preaccepted");
    assert_eq!(
        retained,
        ResourceVector::new(1, accepted_resident_bytes, 0, 0)
    );
    let accepted_version = owner_version(&authority, &hash);
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Ready(_))
    ));
    let accepted = authority
        .plan_accept_for_foundation(&hash, accepted_version, AcceptedStatus::Proposed)
        .expect("verified owner has one membership plan")
        .apply();
    assert_eq!(accepted.async_process_observation_count(), 1);
    apply_plan(accepted);
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::Accepted(entry)) if entry.status() == AcceptedStatus::Proposed
    ));
    assert_eq!(authority.resources().preaccepted().entries, 0);
    assert_eq!(authority.resources().remote().entries, 0);
    assert_eq!(authority.resources().peer(PeerIndex::from(31)).entries, 0);
    assert_eq!(authority.resources().accepted().entries, 1);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_allocation_failure_retains_capability_until_generation_replacement() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote_until(&mut authority, 1_733, 733, 10);
    let original_charge = authority
        .entry(&hash)
        .and_then(|owner| owner.preaccepted_charge())
        .expect("the queued owner has one retained charge");
    let checkout = authority
        .plan_checkout_for_foundation(
            &hash,
            owner_version(&authority, &hash),
            WorkPermit::ResolveOnly,
        )
        .expect("the queued owner checks out")
        .apply();
    let (_, resolve) = take_resolve_work(checkout);
    let failure = ComputeSettlementFailure::allocation_for_foundation(resolve.internal_failure());
    assert_eq!(
        failure.recovery(),
        &ComputeSettlementRecovery::CancelAfterAllocation
    );

    let settlement = failure.discard_result_for_generation_replacement();
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computing(_))
                && entry.charge.active_work == 1
    ));
    assert_ne!(
        authority
            .entry(&hash)
            .and_then(|owner| owner.preaccepted_charge()),
        Some(original_charge),
        "active-work charge remains conservative until the generation terminal"
    );

    let tip = authority.chain_view().tip().0.clone();
    let committed = authority
        .plan_clear_pool(tip)
        .expect("the allocation terminal builds one fresh generation")
        .apply();
    assert_eq!(committed.retired_len(), 1);
    assert!(authority.entry(&hash).is_none());
    let obsolete = authority
        .apply_settlement(settlement)
        .expect_err("the retained capability is obsolete after replacement");
    assert_eq!(
        obsolete.recovery(),
        &ComputeSettlementRecovery::Obsolete(StalePlan::Missing)
    );
    drop(obsolete);
    assert_resource_reference(&authority);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_compute_slot_release_wake_is_carried_by_the_sealed_resource_delta() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 112, 22);
    let waiting = admit_remote(&mut authority, 113, 23);
    let version = owner_version(&authority, &hash);
    let (_, work) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(&hash, version, WorkPermit::ResolveOnly)
            .expect("fixture checkout plans")
            .apply(),
    );
    let committed = authority
        .apply_settlement(work.rejected(RejectionKind::Policy))
        .expect("the terminal settlement seals one resource release");
    assert!(
        committed.compute_wake_for_foundation(),
        "the real Apply must publish the resource-carried wake without rereading all shards"
    );
    drop(committed);
    assert_eq!(authority.resources().preaccepted().active_work, 0);
    assert!(authority.entry(&waiting).is_some());
}

#[test]
fn uak_compute_growth_requires_an_authority_issued_grant() {
    let mut resolve_authority = TxPoolAuthority::for_foundation(limits());
    let resolve_hash = admit_remote(&mut resolve_authority, 540, 54);
    let resolve_version = owner_version(&resolve_authority, &resolve_hash);
    let checkout = resolve_authority
        .plan_checkout_for_foundation(
            &resolve_hash,
            resolve_version,
            WorkPermit::ResolveThenVerify(VerifyCapability::Any),
        )
        .expect("bounded resolve grant is available")
        .apply();
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.into_work() else {
        panic!("continuous permit returns continuous resolve work");
    };
    assert_eq!(
        resolve.resolution_grant().max_total_retained_bytes(),
        4 * 1024
    );
    assert_eq!(resolve.resolution_grant().max_edges(), 16);
    let oversized = resolution_evidence(
        resolve.transaction(),
        Capacity::shannons(1),
        4 * 1024 + 1,
        VerifyCycleClass::Small,
    );
    let ContinuousResolution::Settle(denied) = resolve
        .resolved(oversized)
        .expect("resolution evidence is structurally valid")
    else {
        panic!("oversized resolution cannot continue under the grant");
    };
    apply_plan(
        resolve_authority
            .apply_settlement(denied)
            .expect("budget denial releases the active grant"),
    );
    assert!(resolve_authority.entry(&resolve_hash).is_none());
    assert_resource_reference(&resolve_authority);

    let mut verify_authority = TxPoolAuthority::for_foundation(limits());
    let verify_transaction = (0u8..13)
        .fold(
            TransactionBuilder::default().version(541u32),
            |builder, index| {
                builder.input(CellInput::new(
                    OutPoint::new(Byte32::new([index; 32]), 0),
                    0,
                ))
            },
        )
        .build();
    let admission = ValidatedAdmission::remote(verify_transaction, PeerIndex::from(55))
        .expect("large accepted-footprint fixture is valid");
    let verify_hash = admission.identity.raw.clone();
    apply_plan(
        verify_authority
            .plan_admission(admission)
            .expect("large accepted-footprint fixture is admitted"),
    );
    let verify_version = owner_version(&verify_authority, &verify_hash);
    let checkout = verify_authority
        .plan_checkout_for_foundation(
            &verify_hash,
            verify_version,
            WorkPermit::ResolveThenVerify(VerifyCapability::Any),
        )
        .expect("bounded continuous grant is available")
        .apply();
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.into_work() else {
        panic!("continuous permit returns continuous resolve work");
    };
    let payload = resolved_payload(resolve.transaction());
    let (verify, _) = continue_fixture_verify(resolve, payload);
    let denied = verify.verified(0);
    apply_plan(
        verify_authority
            .apply_settlement(denied)
            .expect("verified budget denial releases the active grant"),
    );
    assert!(verify_authority.entry(&verify_hash).is_none());
    assert_resource_reference(&verify_authority);
}

#[test]
fn uak_compute_grant_and_settlement_share_total_retained_byte_units() {
    const GRANT_BYTES: usize = 4 * 1024;
    const ENTRY_METADATA_BYTES: usize = 128;
    const EDGE_METADATA_BYTES: usize = 64;

    let limits = || {
        let compute = ComputeLimits::new(GRANT_BYTES, GRANT_BYTES, 16);
        let retained = ResourceVector::new(8, 64 * 1024, 128, 2)
            .with_compute_capacity(2 * GRANT_BYTES, 32)
            .expect("fixture compute partition fits");
        let per_peer = ResourceVector::new(4, 32 * 1024, 64, 1)
            .with_compute_capacity(GRANT_BYTES, 16)
            .expect("fixture peer compute partition fits");
        ResourceLimits::with_residency_policy(
            retained,
            retained,
            per_peer,
            AcceptedResources::new(8, 64 * 1024, 64 * 1024, u64::MAX),
            compute,
            ResidencyPolicy::production(
                NonZeroUsize::new(ENTRY_METADATA_BYTES).expect("entry metadata is non-zero"),
                NonZeroUsize::new(EDGE_METADATA_BYTES).expect("edge metadata is non-zero"),
            ),
        )
        .expect("production-shaped limits are monotonic")
    };

    let mut exact = TxPoolAuthority::for_foundation(limits());
    let exact_hash = admit_remote(&mut exact, 541, 55);
    let exact_version = owner_version(&exact, &exact_hash);
    let exact_checkout = exact
        .plan_checkout_for_foundation(&exact_hash, exact_version, WorkPermit::ResolveOnly)
        .expect("exact-boundary resolve checkout plans")
        .apply();
    let CheckedOutWork::Resolve(exact_work) = exact_checkout.into_work() else {
        panic!("resolve-only permit returns resolve work");
    };
    let exact_evidence = resolution_evidence(
        exact_work.transaction(),
        Capacity::shannons(1),
        GRANT_BYTES - ENTRY_METADATA_BYTES,
        VerifyCycleClass::Small,
    );
    apply_plan(
        exact
            .apply_settlement(
                exact_work
                    .resolved(exact_evidence)
                    .expect("exact-boundary evidence is structurally valid"),
            )
            .expect("the exact total-retained boundary settles"),
    );
    assert!(matches!(
        exact.entry(&exact_hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Verify(_)))
    ));
    assert_resource_reference(&exact);

    let mut oversized = TxPoolAuthority::for_foundation(limits());
    let oversized_hash = admit_remote(&mut oversized, 542, 56);
    let oversized_version = owner_version(&oversized, &oversized_hash);
    let oversized_checkout = oversized
        .plan_checkout_for_foundation(&oversized_hash, oversized_version, WorkPermit::ResolveOnly)
        .expect("oversized resolve checkout still owns a bounded grant")
        .apply();
    let CheckedOutWork::Resolve(oversized_work) = oversized_checkout.into_work() else {
        panic!("resolve-only permit returns resolve work");
    };
    let oversized_evidence = resolution_evidence(
        oversized_work.transaction(),
        Capacity::shannons(1),
        GRANT_BYTES - ENTRY_METADATA_BYTES + 1,
        VerifyCycleClass::Small,
    );
    apply_plan(
        oversized
            .apply_settlement(
                oversized_work
                    .resolved(oversized_evidence)
                    .expect("over-bound evidence compiles to an ordinary resource result"),
            )
            .expect("resource exclusion consumes the compute capability"),
    );
    assert!(oversized.entry(&oversized_hash).is_none());
    assert_eq!(oversized.resources().preaccepted().active_work, 0);
    assert_resource_reference(&oversized);
}

#[test]
fn uak_invalid_resolution_receipt_preserves_the_only_settlement_capability() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let resolve_hash = admit_remote(&mut authority, 623, 71);
    let resolve_version = owner_version(&authority, &resolve_hash);
    let (_, resolve) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(&resolve_hash, resolve_version, WorkPermit::ResolveOnly)
            .expect("resolve checkout plans")
            .apply(),
    );
    let evidence = resolution_evidence(
        &tx(6_230),
        Capacity::shannons(1),
        resolve.transaction().data().total_size(),
        VerifyCycleClass::Small,
    );
    let failure = resolve
        .resolved(evidence)
        .expect_err("resolved metadata cannot belong to another transaction");
    assert_eq!(
        failure.error(),
        &ResolutionReceiptError::TransactionMismatch
    );
    apply_plan(
        authority
            .apply_settlement(failure.into_settlement())
            .expect("invalid resolve receipt settles its exact work"),
    );

    assert!(matches!(
        authority.entry(&resolve_hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
                && entry.charge.active_work == 0
    ));
    assert!(authority.primary_projection_consistent());
    assert_resource_reference(&authority);
}

#[test]
fn uak_verified_residency_is_derived_from_the_owned_payload() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(625);
    let hash = admit_remote(&mut authority, 625, 73);
    let version = owner_version(&authority, &hash);
    let (_, resolve) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(&hash, version, WorkPermit::ResolveOnly)
            .expect("resolve checkout plans")
            .apply(),
    );
    let serialized_bytes = transaction.data().total_size();
    let resolved_resident_bytes = serialized_bytes
        .checked_add(64)
        .expect("fixture residency fits usize");
    let evidence = resolution_evidence(
        resolve.transaction(),
        Capacity::shannons(1),
        resolved_resident_bytes,
        VerifyCycleClass::Small,
    );
    let resolved = resolve
        .resolved(evidence)
        .expect("resolution evidence is valid");
    apply_plan(
        authority
            .apply_settlement(resolved)
            .expect("resolved payload is retained for verification"),
    );

    let version = owner_version(&authority, &hash);
    let committed = authority
        .plan_checkout_for_foundation(
            &hash,
            version,
            WorkPermit::VerifyOnly(VerifyCapability::Any),
        )
        .expect("verify checkout plans")
        .apply();
    let CheckedOutWork::Verify(verify) = committed.into_work() else {
        panic!("verify permit returns verify work");
    };
    let expected_resident_bytes = accepted_transaction_charge_bytes(
        transaction.data().serialized_size_in_block(),
        verify.resolved_transaction(),
    );
    let settlement = verify.verified(0);
    apply_plan(
        authority
            .apply_settlement(settlement)
            .expect("the internally charged verify receipt settles"),
    );

    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(
                &entry.phase,
                PreAcceptedPhase::Ready(verified)
                    if verified.metrics().cost.resident_bytes == expected_resident_bytes
                        && verified.metrics().cost.serialized_bytes
                            == transaction.data().serialized_size_in_block()
            )
    ));
    assert_resource_reference(&authority);
}

#[test]
fn uak_successful_verification_compacts_deps_but_retains_dao_inputs() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let input = OutPoint::new(Byte32::new([0xb1; 32]), 0);
    let dependency = OutPoint::new(Byte32::new([0xb2; 32]), 0);
    let transaction = TransactionBuilder::default()
        .version(626u32)
        .input(CellInput::new(input.clone(), 0))
        .cell_dep(CellDep::new_builder().out_point(dependency.clone()).build())
        .build();
    let admission = ValidatedAdmission::remote(transaction.clone(), PeerIndex::from(74))
        .expect("compaction fixture admission is valid");
    let hash = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("compaction fixture is admitted"),
    );
    let checkout = authority
        .plan_checkout_for_foundation(
            &hash,
            owner_version(&authority, &hash),
            WorkPermit::ResolveThenVerify(VerifyCapability::Any),
        )
        .expect("compaction fixture checks out")
        .apply();
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.into_work() else {
        panic!("continuous permit returns resolve work");
    };

    let input_data = Bytes::from(vec![0x51; 128]);
    let dependency_data = Bytes::from(vec![0x52; 128]);
    let dependency_info = TransactionInfo::new(
        1,
        EpochNumberWithFraction::new(1, 0, 1),
        Byte32::new([0xb3; 32]),
        0,
    );
    let resolved = Arc::new(ResolvedTransaction {
        transaction: transaction.clone(),
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(CellOutput::default(), input_data.clone())
                .out_point(input)
                .build(),
        ],
        resolved_cell_deps: vec![
            CellMetaBuilder::from_cell_output(CellOutput::default(), dependency_data)
                .out_point(dependency.clone())
                .transaction_info(dependency_info.clone())
                .build(),
        ],
        resolved_dep_groups: Vec::new(),
    });
    let tx_size = transaction.data().serialized_size_in_block();
    let resident_bytes = resolved_transaction_charge_bytes(tx_size, &resolved);
    let ContinuousResolution::Verify(verify) = resolve
        .resolved(ResolutionEvidence::for_foundation(
            resolved,
            Capacity::shannons(1),
            resident_bytes,
            VerifyCycleClass::Small,
        ))
        .expect("exact resolved evidence is valid")
    else {
        panic!("the resolved payload fits its continuous grant");
    };
    apply_plan(
        authority
            .apply_settlement(verify.verified(0))
            .expect("compacted verification settles"),
    );

    let Some(OwnedTx::PreAccepted(entry)) = authority.entry(&hash) else {
        panic!("the verified owner remains PreAccepted");
    };
    let PreAcceptedPhase::Ready(verified) = &entry.phase else {
        panic!("successful verification produces Ready");
    };
    let payload = verified.payload().resolved_transaction();
    assert_eq!(
        payload.resolved_inputs[0]
            .mem_cell_data
            .as_ref()
            .map(Bytes::len),
        Some(input_data.len()),
        "DAO-relevant input data remains resident"
    );
    let dep = &payload.resolved_cell_deps[0];
    assert_eq!(dep.out_point, dependency);
    assert_eq!(dep.transaction_info, Some(dependency_info));
    assert_eq!(dep.cell_output, CellOutput::default());
    assert_eq!(dep.data_bytes, 0);
    assert!(dep.mem_cell_data.is_none());
    assert!(dep.mem_cell_data_hash.is_none());
    assert_eq!(
        verified.metrics().cost.resident_bytes,
        accepted_transaction_charge_bytes(tx_size, payload)
    );
    assert_resource_reference(&authority);
}

#[test]
fn uak_resolve_to_verify_continuation_changes_no_authority_state() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 28, 48);
    let version = owner_version(&authority, &hash);
    let checkout = authority
        .plan_checkout_for_foundation(
            &hash,
            version,
            WorkPermit::ResolveThenVerify(VerifyCapability::Any),
        )
        .expect("continuous checkout plans")
        .apply();
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.into_work() else {
        panic!("continuous permit returns continuous resolve work");
    };
    let before = authority.normalized_snapshot();
    let payload = resolved_payload(resolve.transaction());
    let (verify, _) = continue_fixture_verify(resolve, payload);
    assert_eq!(authority.normalized_snapshot(), before);
    apply_plan(
        authority
            .apply_settlement(verify.internal_failure())
            .expect("continuous lease remains current"),
    );
    assert_resource_reference(&authority);
}

#[test]
fn uak_verified_settlement_has_one_ready_projection() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 29, 49);
    let version = owner_version(&authority, &hash);
    let checkout = authority
        .plan_checkout_for_foundation(
            &hash,
            version,
            WorkPermit::ResolveThenVerify(VerifyCapability::Any),
        )
        .expect("continuous checkout plans")
        .apply();
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.into_work() else {
        panic!("continuous permit returns continuous resolve work");
    };
    let payload = resolved_payload(resolve.transaction());
    let (verify, _) = continue_fixture_verify(resolve, payload);
    apply_plan(
        authority
            .apply_settlement(verify.verified(0))
            .expect("verified settlement plans"),
    );
    assert_eq!(authority.owner_count(), 1);
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Ready(_))
    ));
    assert_resource_reference(&authority);
}

#[test]
fn uak_foundation_state_command_table_rejects_illegal_rows_without_mutation() {
    let mut queued = TxPoolAuthority::for_foundation(limits());
    let queued_hash = admit_remote(&mut queued, 30, 50);
    let queued_version = owner_version(&queued, &queued_hash);
    let before = queued.normalized_snapshot();
    assert_eq!(
        queued
            .plan_checkout_for_foundation(
                &queued_hash,
                queued_version,
                WorkPermit::VerifyOnly(VerifyCapability::Any)
            )
            .err(),
        Some(PlanError::Stale(StalePlan::Phase))
    );
    assert_eq!(queued.normalized_snapshot(), before);
    assert_eq!(
        queued
            .plan_accept_for_foundation(&queued_hash, queued_version, AcceptedStatus::Pending,)
            .err(),
        Some(PlanError::Stale(StalePlan::Phase))
    );
    assert_eq!(queued.normalized_snapshot(), before);

    let checkout = queued
        .plan_checkout_for_foundation(&queued_hash, queued_version, WorkPermit::ResolveOnly)
        .expect("resolve checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.into_work() else {
        panic!("resolve-only permit returns resolve work");
    };
    apply_plan(
        queued
            .apply_settlement(
                resolve
                    .missing(missing_keys())
                    .expect("fixture missing keys are non-empty and bounded"),
            )
            .expect("missing settlement plans"),
    );
    let waiting_version = owner_version(&queued, &queued_hash);
    let before = queued.normalized_snapshot();
    assert_eq!(
        queued
            .plan_checkout_for_foundation(&queued_hash, waiting_version, WorkPermit::ResolveOnly)
            .err(),
        Some(PlanError::Stale(StalePlan::Phase))
    );
    assert_eq!(queued.normalized_snapshot(), before);

    let mut rejected = TxPoolAuthority::for_foundation(limits());
    let rejected_hash = admit_remote(&mut rejected, 31, 51);
    let version = owner_version(&rejected, &rejected_hash);
    let checkout = rejected
        .plan_checkout_for_foundation(&rejected_hash, version, WorkPermit::ResolveOnly)
        .expect("resolve checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.into_work() else {
        panic!("resolve-only permit returns resolve work");
    };
    apply_plan(
        rejected
            .apply_settlement(resolve.rejected(RejectionKind::Policy))
            .expect("rejection settlement plans"),
    );
    assert!(rejected.entry(&rejected_hash).is_none());
    let before = rejected.normalized_snapshot();
    assert_eq!(
        rejected
            .plan_accept_for_foundation(&rejected_hash, version, AcceptedStatus::Pending,)
            .err(),
        Some(PlanError::Stale(StalePlan::Missing))
    );
    assert_eq!(rejected.normalized_snapshot(), before);
    assert_resource_reference(&queued);
    assert_resource_reference(&rejected);
}

#[test]
fn uak_missing_settlement_registers_exact_level_wait() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 13, 32);
    let version = owner_version(&authority, &hash);
    let checkout = authority
        .plan_checkout_for_foundation(&hash, version, WorkPermit::ResolveOnly)
        .expect("resolve checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.into_work() else {
        panic!("resolve-only permit returns resolve work");
    };
    assert_eq!(resolve.transaction().hash(), hash.0);
    apply_plan(
        authority
            .apply_settlement(
                resolve
                    .missing(missing_keys())
                    .expect("fixture missing keys are non-empty and bounded"),
            )
            .expect("missing settlement plans"),
    );
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(
                &entry.phase,
                PreAcceptedPhase::Waiting(deps) if deps.len() == 1
            )
    ));
    assert_eq!(authority.resources().preaccepted().active_work, 0);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_continuation_yield_returns_one_queued_owner() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 14, 33);
    let version = owner_version(&authority, &hash);
    let checkout = authority
        .plan_checkout_for_foundation(&hash, version, WorkPermit::ResolveOnly)
        .expect("resolve checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.into_work() else {
        panic!("resolve-only permit returns resolve work");
    };
    let resident_bytes = resolve.transaction().data().total_size();
    assert_eq!(resolve.resolution_grant().max_edges(), 16);
    let evidence = resolution_evidence(
        resolve.transaction(),
        Capacity::shannons(1),
        resident_bytes,
        VerifyCycleClass::Small,
    );
    apply_plan(
        authority
            .apply_settlement(
                resolve
                    .resolved(evidence)
                    .expect("fixture resolution evidence is valid"),
            )
            .expect("yielded resolve settles as queued verify"),
    );
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Verify(_)))
    ));

    let version = owner_version(&authority, &hash);
    let verify_checkout = authority
        .plan_checkout_for_foundation(
            &hash,
            version,
            WorkPermit::VerifyOnly(VerifyCapability::Any),
        )
        .expect("queued verify accepts verify-only permit")
        .apply();
    let CheckedOutWork::Verify(verify) = verify_checkout.into_work() else {
        panic!("verify-only permit returns verify work");
    };
    apply_plan(
        authority
            .apply_settlement(verify.rejected(RejectionKind::Verification))
            .expect("verification rejection settles"),
    );
    assert!(authority.entry(&hash).is_none());
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_active_compute_capability_survives_chain_view_change() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let second_hash = admit_remote(&mut authority, 16, 35);
    let version = owner_version(&authority, &second_hash);
    let second_checkout = authority
        .plan_checkout_for_foundation(
            &second_hash,
            version,
            WorkPermit::ResolveThenVerify(VerifyCapability::Any),
        )
        .expect("second checkout plans")
        .apply();
    let CheckedOutWork::ContinuousResolve(second) = second_checkout.into_work() else {
        panic!("continuous permit returns continuous resolve work");
    };
    let payload = resolved_payload(second.transaction());
    let (verify, _) = continue_fixture_verify(second, payload);
    let settlement = verify.verified(0);
    authority.force_chain_view(ChainViewId::new(ChainRevision(1), Byte32::new([16; 32])));
    apply_plan(
        authority
            .apply_settlement(settlement)
            .expect("the active capability remains settleable after a chain-view change"),
    );
    assert_resource_reference(&authority);
}

#[test]
fn uak_every_resolve_and_verify_terminal_shape_is_typed() {
    let mut authority = TxPoolAuthority::for_foundation(limits());

    let resolve_reject_hash = admit_remote(&mut authority, 17, 36);
    let version = owner_version(&authority, &resolve_reject_hash);
    let resolve_checkout = authority
        .plan_checkout_for_foundation(&resolve_reject_hash, version, WorkPermit::ResolveOnly)
        .expect("resolve checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = resolve_checkout.into_work() else {
        panic!("resolve-only permit returns resolve work");
    };
    apply_plan(
        authority
            .apply_settlement(resolve.rejected(RejectionKind::Policy))
            .expect("resolve rejection settles"),
    );

    let resolve_failure_hash = admit_remote(&mut authority, 625, 73);
    let version = owner_version(&authority, &resolve_failure_hash);
    let checkout = authority
        .plan_checkout_for_foundation(&resolve_failure_hash, version, WorkPermit::ResolveOnly)
        .expect("resolve checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.into_work() else {
        panic!("resolve-only permit returns resolve work");
    };
    apply_plan(
        authority
            .apply_settlement(resolve.internal_failure())
            .expect("resolve worker failure settles"),
    );

    let continuous_missing_hash = admit_remote(&mut authority, 18, 37);
    let version = owner_version(&authority, &continuous_missing_hash);
    let continuous_checkout = authority
        .plan_checkout_for_foundation(
            &continuous_missing_hash,
            version,
            WorkPermit::ResolveThenVerify(VerifyCapability::Any),
        )
        .expect("continuous checkout plans")
        .apply();
    let CheckedOutWork::ContinuousResolve(continuous) = continuous_checkout.into_work() else {
        panic!("continuous permit returns continuous resolve work");
    };
    apply_plan(
        authority
            .apply_settlement(
                continuous
                    .missing(missing_keys())
                    .expect("fixture missing keys are non-empty and bounded"),
            )
            .expect("continuous missing settles"),
    );

    let verify_success_hash = admit_remote(&mut authority, 19, 38);
    let version = owner_version(&authority, &verify_success_hash);
    let first = authority
        .plan_checkout_for_foundation(&verify_success_hash, version, WorkPermit::ResolveOnly)
        .expect("resolve checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = first.into_work() else {
        panic!("resolve-only permit returns resolve work");
    };
    let payload = resolved_payload(resolve.transaction());
    apply_plan(
        authority
            .apply_settlement(
                resolve
                    .yield_verify(payload)
                    .expect("fixture payload belongs to the checked-out transaction"),
            )
            .expect("resolve yield settles"),
    );
    let version = owner_version(&authority, &verify_success_hash);
    let second = authority
        .plan_checkout_for_foundation(
            &verify_success_hash,
            version,
            WorkPermit::VerifyOnly(VerifyCapability::Any),
        )
        .expect("verify checkout plans")
        .apply();
    let CheckedOutWork::Verify(verify) = second.into_work() else {
        panic!("verify-only permit returns verify work");
    };
    apply_plan(
        authority
            .apply_settlement(verify.verified(0))
            .expect("verify success settles"),
    );

    assert!(authority.primary_projection_consistent());
    assert!(authority.entry(&resolve_reject_hash).is_none());
    assert!(matches!(
        authority.entry(&continuous_missing_hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Waiting(_))
    ));
    assert!(matches!(
        authority.entry(&verify_success_hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Ready(_))
    ));

    let mut authority = TxPoolAuthority::for_foundation(limits());
    let continuous_reject_hash = admit_remote(&mut authority, 20, 39);
    let version = owner_version(&authority, &continuous_reject_hash);
    let checkout = authority
        .plan_checkout_for_foundation(
            &continuous_reject_hash,
            version,
            WorkPermit::ResolveThenVerify(VerifyCapability::Any),
        )
        .expect("continuous checkout plans")
        .apply();
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.into_work() else {
        panic!("continuous permit returns continuous resolve work");
    };
    apply_plan(
        authority
            .apply_settlement(resolve.internal_failure())
            .expect("continuous resolve worker failure settles"),
    );

    let verify_failure_hash = admit_remote(&mut authority, 21, 40);
    let version = owner_version(&authority, &verify_failure_hash);
    let checkout = authority
        .plan_checkout_for_foundation(&verify_failure_hash, version, WorkPermit::ResolveOnly)
        .expect("resolve checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.into_work() else {
        panic!("resolve-only permit returns resolve work");
    };
    let payload = resolved_payload(resolve.transaction());
    apply_plan(
        authority
            .apply_settlement(
                resolve
                    .yield_verify(payload)
                    .expect("fixture payload belongs to the checked-out transaction"),
            )
            .expect("resolve yield settles"),
    );
    let version = owner_version(&authority, &verify_failure_hash);
    let checkout = authority
        .plan_checkout_for_foundation(
            &verify_failure_hash,
            version,
            WorkPermit::VerifyOnly(VerifyCapability::Any),
        )
        .expect("verify checkout plans")
        .apply();
    let CheckedOutWork::Verify(verify) = checkout.into_work() else {
        panic!("verify-only permit returns verify work");
    };
    apply_plan(
        authority
            .apply_settlement(verify.internal_failure())
            .expect("verify worker failure settles"),
    );

    let continuous_verify_reject_hash = admit_remote(&mut authority, 22, 41);
    let version = owner_version(&authority, &continuous_verify_reject_hash);
    let checkout = authority
        .plan_checkout_for_foundation(
            &continuous_verify_reject_hash,
            version,
            WorkPermit::ResolveThenVerify(VerifyCapability::Any),
        )
        .expect("continuous checkout plans")
        .apply();
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.into_work() else {
        panic!("continuous permit returns continuous resolve work");
    };
    let payload = resolved_payload(resolve.transaction());
    let (verify, _) = continue_fixture_verify(resolve, payload);
    apply_plan(
        authority
            .apply_settlement(verify.rejected(RejectionKind::Verification))
            .expect("continuous verification rejection settles"),
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_fair_frontier_is_a_derived_non_owning_projection() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 600, 60);
    assert!(authority.primary_projection_consistent());

    let before = authority.normalized_snapshot();
    let prepared = authority
        .plan_checkout_next(WorkPermit::ResolveOnly)
        .expect("frontier selection is valid")
        .expect("queued owner is selectable");
    drop(prepared);
    assert!(
        authority
            .normalized_snapshot()
            .equivalent_committed_state_with_exact_reservations(&before, 1, 0, 1),
        "the dropped checkout burns exactly its replacement version and Apply stamp"
    );

    let committed = authority
        .plan_checkout_next(WorkPermit::ResolveOnly)
        .expect("frontier selection is valid")
        .expect("dropped plan did not consume the queue slot")
        .apply();
    let (selected, work) = take_resolve_work(committed);
    assert_eq!(selected, hash);
    assert!(authority.primary_projection_consistent());

    apply_plan(
        authority
            .apply_settlement(work.rejected(RejectionKind::Policy))
            .expect("selected lease settles"),
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_fair_frontier_round_robins_owners_only_after_apply() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let peer_a_first = admit_remote(&mut authority, 601, 61);
    let peer_a_second = admit_remote(&mut authority, 602, 61);
    let peer_b = admit_remote(&mut authority, 603, 62);
    let trusted_admission =
        ValidatedAdmission::proposal(tx(604)).expect("fixture proposal admission is valid");
    let trusted = trusted_admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(trusted_admission)
            .expect("trusted admission plans"),
    );

    for expected in [&trusted, &peer_a_first, &peer_b, &peer_a_second] {
        let committed = authority
            .plan_checkout_next(WorkPermit::ResolveOnly)
            .expect("frontier selection is valid")
            .expect("one owner remains selectable")
            .apply();
        let (selected, work) = take_resolve_work(committed);
        assert_eq!(&selected, expected);
        apply_plan(
            authority
                .apply_settlement(work.rejected(RejectionKind::Policy))
                .expect("selected lease settles"),
        );
    }
    assert!(
        authority
            .plan_checkout_next(WorkPermit::ResolveOnly)
            .expect("empty frontier is valid")
            .is_none()
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_trusted_frontier_preserves_recovery_over_proposal_priority() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let proposal_admission =
        ValidatedAdmission::proposal(tx(611)).expect("fixture proposal admission is valid");
    let proposal = proposal_admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(proposal_admission)
            .expect("proposal admission plans"),
    );
    let recovery_admission = ValidatedAdmission::recovery(tx(612), PoolGeneration(0))
        .expect("fixture recovery admission is valid");
    let recovery = recovery_admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(recovery_admission)
            .expect("recovery admission plans"),
    );

    for expected in [&recovery, &proposal] {
        let (selected, work) = take_resolve_work(
            authority
                .plan_checkout_next(WorkPermit::ResolveOnly)
                .expect("trusted frontier selection is valid")
                .expect("trusted work remains")
                .apply(),
        );
        assert_eq!(&selected, expected);
        apply_plan(
            authority
                .apply_settlement(work.rejected(RejectionKind::Policy))
                .expect("trusted work settles"),
        );
    }
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_new_trusted_owner_joins_the_existing_owner_ring_without_starving_remote() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let peer_a_first = admit_remote(&mut authority, 617, 68);
    let _peer_a_second = admit_remote(&mut authority, 618, 68);
    let peer_b = admit_remote(&mut authority, 619, 69);

    let (selected, peer_a_work) = take_resolve_work(
        authority
            .plan_checkout_next(WorkPermit::ResolveOnly)
            .expect("initial remote selection is valid")
            .expect("peer A is the first remote owner")
            .apply(),
    );
    assert_eq!(selected, peer_a_first);
    apply_plan(
        authority
            .apply_settlement(peer_a_work.rejected(RejectionKind::Policy))
            .expect("peer A work settles"),
    );

    let trusted_admission =
        ValidatedAdmission::proposal(tx(620)).expect("fixture proposal admission is valid");
    let trusted = trusted_admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(trusted_admission)
            .expect("trusted admission plans"),
    );
    let (selected, peer_b_work) = take_resolve_work(
        authority
            .plan_checkout_next(WorkPermit::ResolveOnly)
            .expect("the next owner-ring selection is valid")
            .expect("peer B remains ahead of the newly arrived owner")
            .apply(),
    );
    assert_eq!(selected, peer_b);
    apply_plan(
        authority
            .apply_settlement(peer_b_work.rejected(RejectionKind::Policy))
            .expect("peer B work settles"),
    );

    let (selected, trusted_work) = take_resolve_work(
        authority
            .plan_checkout_next(WorkPermit::ResolveOnly)
            .expect("the joined trusted owner remains visible")
            .expect("trusted work is served within the same owner ring")
            .apply(),
    );
    assert_eq!(selected, trusted);
    apply_plan(
        authority
            .apply_settlement(trusted_work.rejected(RejectionKind::Policy))
            .expect("trusted work settles"),
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_verify_frontier_preserves_the_configured_arrival_or_fee_order() {
    for order in [VerifyOrder::Arrival, VerifyOrder::FeeRate] {
        let mut authority = TxPoolAuthority::for_foundation_with_order(limits(), order);
        let earlier = queue_remote_for_verify(&mut authority, tx(621), 70, Capacity::shannons(1));
        let later = queue_remote_for_verify(&mut authority, tx(622), 70, Capacity::shannons(1_000));
        let expected = match order {
            VerifyOrder::Arrival => &earlier,
            VerifyOrder::FeeRate => &later,
        };

        let committed = authority
            .plan_checkout_next(WorkPermit::VerifyOnly(VerifyCapability::Any))
            .expect("configured verify selection is valid")
            .expect("verify work is queued")
            .apply();
        let CheckedOutWork::Verify(work) = committed.into_work() else {
            panic!("verify permit returns verify work");
        };
        assert_eq!(
            &TxIdentity::from_transaction(work.transaction()).raw,
            expected
        );
        apply_plan(
            authority
                .apply_settlement(work.rejected(RejectionKind::Policy))
                .expect("selected verify work settles"),
        );
        assert!(authority.primary_projection_consistent());
    }
}

#[test]
fn uak_fair_frontier_skips_saturated_peer_without_blocking_new_peer() {
    let constrained = ResourceLimits::new(
        ResourceVector::new(8, 64 * 1024, 64, 8),
        ResourceVector::new(6, 48 * 1024, 48, 6),
        ResourceVector::new(3, 24 * 1024, 24, 1),
        AcceptedResources::new(8, 64 * 1024, 64 * 1024, 64),
        ComputeLimits::new(4 * 1024, 4 * 1024, 16),
    )
    .expect("fixture limits admit one indivisible grant");
    let mut authority = TxPoolAuthority::for_foundation(constrained);
    let peer_a = admit_remote(&mut authority, 605, 63);
    let peer_b_active = admit_remote(&mut authority, 606, 64);
    let _peer_b_waiting = admit_remote(&mut authority, 607, 64);
    let peer_c = admit_remote(&mut authority, 608, 65);

    let (selected, peer_a_work) = take_resolve_work(
        authority
            .plan_checkout_next(WorkPermit::ResolveOnly)
            .expect("first fair checkout plans")
            .expect("peer A is selectable")
            .apply(),
    );
    assert_eq!(selected, peer_a);

    let peer_b_version = owner_version(&authority, &peer_b_active);
    let (_, peer_b_work) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(&peer_b_active, peer_b_version, WorkPermit::ResolveOnly)
            .expect("manual foundation checkout saturates peer B")
            .apply(),
    );

    let (selected, peer_c_work) = take_resolve_work(
        authority
            .plan_checkout_next(WorkPermit::ResolveOnly)
            .expect("saturated peer is an ordinary unavailable owner")
            .expect("next peer remains selectable")
            .apply(),
    );
    assert_eq!(selected, peer_c);

    for work in [peer_a_work, peer_b_work, peer_c_work] {
        apply_plan(
            authority
                .apply_settlement(work.rejected(RejectionKind::Policy))
                .expect("checked-out work settles"),
        );
    }
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_full_retained_budget_cannot_hide_the_trusted_owner() {
    const COMPUTE_BYTES: usize = 256;
    let proposal =
        ValidatedAdmission::proposal(tx(613)).expect("fixture proposal admission is valid");
    let recovery = ValidatedAdmission::recovery(tx(614), PoolGeneration(0))
        .expect("fixture recovery admission is valid");
    let remote = ValidatedAdmission::remote(tx(615), PeerIndex::from(66))
        .expect("fixture remote admission is valid");
    let remote_hash = remote.identity.raw.clone();
    let remote_charge = remote.charge_for_foundation();
    let total = [
        proposal.charge_for_foundation(),
        recovery.charge_for_foundation(),
        remote_charge,
    ]
    .into_iter()
    .reduce(add_resources)
    .expect("fixture has admissions");
    let constrained = ResourceLimits::new(
        ResourceVector::new(total.entries, total.bytes, total.edges, 3),
        ResourceVector::new(
            remote_charge.entries,
            remote_charge.bytes,
            remote_charge.edges,
            1,
        ),
        ResourceVector::new(
            remote_charge.entries,
            remote_charge.bytes,
            remote_charge.edges,
            1,
        ),
        AcceptedResources::new(8, 64 * 1024, 64 * 1024, 64),
        ComputeLimits::new(COMPUTE_BYTES, COMPUTE_BYTES, 0),
    )
    .expect("retained and transient partitions have a checked combined ceiling");
    let mut authority = TxPoolAuthority::for_foundation(constrained);
    let proposal_hash = proposal.identity.raw.clone();
    let recovery_hash = recovery.identity.raw.clone();
    for admission in [proposal, recovery, remote] {
        apply_plan(
            authority
                .plan_admission(admission)
                .expect("the retained partition fills exactly"),
        );
    }

    let (first, first_probes) = authority
        .plan_checkout_next_with_probe_count_for_foundation(WorkPermit::ResolveOnly)
        .expect("a free transient slot makes the trusted head runnable");
    assert_eq!(first_probes, 1);
    let (selected, first_work) = take_resolve_work(first.expect("trusted work exists").apply());
    assert_eq!(selected, recovery_hash);

    let (second, second_probes) = authority
        .plan_checkout_next_with_probe_count_for_foundation(WorkPermit::ResolveOnly)
        .expect("the remote owner receives its bounded turn");
    assert_eq!(second_probes, 1);
    let (selected, second_work) = take_resolve_work(second.expect("remote work exists").apply());
    assert_eq!(selected, remote_hash);

    let (third, third_probes) = authority
        .plan_checkout_next_with_probe_count_for_foundation(WorkPermit::ResolveOnly)
        .expect("the remaining trusted transaction stays visible");
    assert_eq!(third_probes, 1);
    let (selected, third_work) =
        take_resolve_work(third.expect("second trusted work exists").apply());
    assert_eq!(selected, proposal_hash);

    for work in [first_work, second_work, third_work] {
        apply_plan(
            authority
                .apply_settlement(work.rejected(RejectionKind::Policy))
                .expect("every trusted lease settles"),
        );
    }
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_proposal_promotion_separates_peer_residency_from_trusted_compute() {
    let peer = PeerIndex::from(68);
    let active_admission =
        ValidatedAdmission::remote(tx(617), peer).expect("active remote fixture is valid");
    let promoted_admission =
        ValidatedAdmission::remote(tx(618), peer).expect("promoted remote fixture is valid");
    let trusted_admission =
        ValidatedAdmission::proposal(tx(619)).expect("trusted proposal fixture is valid");
    let remote = add_resources(
        active_admission.charge_for_foundation(),
        promoted_admission.charge_for_foundation(),
    );
    let total = add_resources(remote, trusted_admission.charge_for_foundation());
    let constrained = ResourceLimits::new(
        ResourceVector::new(total.entries, total.bytes, total.edges, 3),
        ResourceVector::new(remote.entries, remote.bytes, remote.edges, 1),
        ResourceVector::new(remote.entries, remote.bytes, remote.edges, 1),
        AcceptedResources::new(8, 64 * 1024, 64 * 1024, 64),
        ComputeLimits::new(4 * 1024, 4 * 1024, 16),
    )
    .expect("peer residency and transient compute have independent hard bounds");
    let mut authority = TxPoolAuthority::for_foundation(constrained);
    let active_hash = active_admission.identity.raw.clone();
    let promoted_hash = promoted_admission.identity.raw.clone();
    for admission in [active_admission, promoted_admission, trusted_admission] {
        apply_plan(
            authority
                .plan_admission(admission)
                .expect("fixture admission fits"),
        );
    }
    apply_plan(
        authority
            .plan_admission(
                ValidatedAdmission::proposal(tx(618))
                    .expect("proposal promotion carries chain provenance"),
            )
            .expect("promotion changes policy without erasing ingress"),
    );

    let active_version = owner_version(&authority, &active_hash);
    let (_, remote_work) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(&active_hash, active_version, WorkPermit::ResolveOnly)
            .expect("the peer consumes its only remote compute slot")
            .apply(),
    );
    assert_eq!(authority.resources().remote().active_work, 1);
    assert_eq!(authority.resources().peer(peer).active_work, 1);

    let (plan, probes) = authority
        .plan_checkout_next_with_probe_count_for_foundation(WorkPermit::ResolveOnly)
        .expect("proposal compute is authorized by its current priority lease");
    assert_eq!(probes, 1);
    let (selected, promoted_work) =
        take_resolve_work(plan.expect("promoted trusted work is runnable").apply());
    assert_eq!(selected, promoted_hash);
    assert_eq!(authority.resources().preaccepted().active_work, 2);
    assert_eq!(authority.resources().remote().active_work, 1);
    assert_eq!(authority.resources().peer(peer).active_work, 1);

    for work in [remote_work, promoted_work] {
        apply_plan(
            authority
                .apply_settlement(work.rejected(RejectionKind::Policy))
                .expect("both capability attributions settle exactly once"),
        );
    }
    assert_resource_reference(&authority);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_checkout_attack_work_is_bounded_by_owner_heads_and_active_slots() {
    const BLOCKED_PEERS: usize = 8;
    let constrained = ResourceLimits::new(
        ResourceVector::new(32, 512 * 1024, 512, BLOCKED_PEERS + 1),
        ResourceVector::new(32, 512 * 1024, 512, BLOCKED_PEERS + 1),
        ResourceVector::new(4, 32 * 1024, 32, 1),
        AcceptedResources::new(32, 512 * 1024, 512 * 1024, 512),
        ComputeLimits::new(4 * 1024, 4 * 1024, 16),
    )
    .expect("fixture admits every indivisible worker grant");
    let mut authority = TxPoolAuthority::for_foundation(constrained);
    let mut active_hashes = Vec::new();
    for offset in 0..BLOCKED_PEERS {
        let peer = 700 + offset;
        active_hashes.push(admit_remote(&mut authority, 800 + offset as u64 * 2, peer));
        let _queued = admit_remote(&mut authority, 801 + offset as u64 * 2, peer);
    }
    let cursor_peer = 700 + BLOCKED_PEERS;
    let cursor_seed = admit_remote(&mut authority, 899, cursor_peer);

    let mut active_work = Vec::new();
    for _ in 0..BLOCKED_PEERS {
        let (plan, probes) = authority
            .plan_checkout_next_with_probe_count_for_foundation(WorkPermit::ResolveOnly)
            .expect("the production scheduler advances across distinct peer owners");
        assert_eq!(probes, 1);
        let (selected, work) = take_resolve_work(
            plan.expect("one head per unsaturated peer remains runnable")
                .apply(),
        );
        assert!(active_hashes.contains(&selected));
        active_work.push(work);
    }

    // Seed the cursor through the real scheduler path, then release that
    // peer's compute charge. The next search must wrap across every blocked
    // owner before reaching a newly queued entry for this peer.
    let (plan, probes) = authority
        .plan_checkout_next_with_probe_count_for_foundation(WorkPermit::ResolveOnly)
        .expect("the cursor seed is schedulable");
    assert_eq!(probes, 1);
    let (selected, cursor_work) = take_resolve_work(
        plan.expect("the final unsaturated owner holds the cursor seed")
            .apply(),
    );
    assert_eq!(selected, cursor_seed);
    apply_plan(
        authority
            .apply_settlement(cursor_work.rejected(RejectionKind::Policy))
            .expect("cursor seeding releases the peer slot without rewinding the cursor"),
    );
    let final_peer = admit_remote(&mut authority, 900, cursor_peer);

    let (plan, probes) = authority
        .plan_checkout_next_with_probe_count_for_foundation(WorkPermit::ResolveOnly)
        .expect("bounded owner-head search plans");
    assert_eq!(probes, BLOCKED_PEERS + 1);
    let (selected, final_work) = take_resolve_work(
        plan.expect("the final unsaturated peer remains runnable")
            .apply(),
    );
    assert_eq!(selected, final_peer);
    let active_usage = authority.resources().preaccepted();
    assert_eq!(active_usage.active_work, BLOCKED_PEERS + 1);
    assert_eq!(
        active_usage.compute_bytes(),
        (BLOCKED_PEERS + 1)
            .checked_mul(4 * 1024)
            .expect("fixture compute partition is finite")
    );

    let (plan, probes) = authority
        .plan_checkout_next_with_probe_count_for_foundation(WorkPermit::ResolveOnly)
        .expect("global active-work exhaustion is ordinary backpressure");
    assert!(plan.is_none());
    assert_eq!(
        probes, 0,
        "the authoritative global slot gate stops before owner enumeration"
    );

    active_work.push(final_work);
    for work in active_work {
        apply_plan(
            authority
                .apply_settlement(work.internal_failure())
                .expect("every checked-out work value settles exactly once"),
        );
    }
    assert_eq!(authority.resources().preaccepted().compute_bytes(), 0);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_stale_dependency_head_cannot_abort_unrelated_checkout() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = TransactionBuilder::default()
        .version(900u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent_admission = ValidatedAdmission::remote(parent_tx.clone(), PeerIndex::from(900usize))
        .expect("fixture parent admission is valid");
    let parent = parent_admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(parent_admission)
            .expect("fixture parent enters PreAccepted ownership"),
    );
    let stale_input = OutPoint::new(parent_tx.hash(), 0);
    let stale_tx = TransactionBuilder::default()
        .version(901u32)
        .input(CellInput::new(stale_input, 0))
        .build();
    let fresh_input = OutPoint::new(Byte32::new([0xd2; 32]), 0);
    let fresh_tx = TransactionBuilder::default()
        .version(902u32)
        .input(CellInput::new(fresh_input, 0))
        .build();
    let stale =
        queue_remote_for_verify(&mut authority, stale_tx.clone(), 901, Capacity::shannons(1));
    let fresh = queue_remote_for_verify(&mut authority, fresh_tx, 902, Capacity::shannons(1));
    apply_plan(
        authority
            .plan_admission(
                ValidatedAdmission::proposal(stale_tx)
                    .expect("the queued remote owner can gain trusted proposal priority"),
            )
            .expect("promotion moves the same queue slot to the trusted owner"),
    );

    apply_plan(
        authority
            .plan_terminalize_for_foundation(&parent, owner_version(&authority, &parent))
            .expect("definitive parent loss publishes a new dependency cut"),
    );
    let (plan, probes) = authority
        .plan_checkout_next_with_probe_count_for_foundation(WorkPermit::VerifyOnly(
            VerifyCapability::Any,
        ))
        .expect("a stale owner head is local ineligibility, not a failed round");
    assert_eq!(probes, 2);
    let committed = plan.expect("the unrelated owner remains runnable").apply();
    let CheckedOutWork::Verify(work) = committed.into_work() else {
        panic!("verify-only capability returns verify work");
    };
    assert_eq!(RawTxHash(work.transaction().hash()), fresh);
    assert!(matches!(
        authority.entry(&stale),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Verify(_)))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_retained_growth_denial_atomically_settles_checked_out_work() {
    let admission = ValidatedAdmission::remote(tx(1_000), PeerIndex::from(900))
        .expect("capacity fixture admission is valid");
    let raw_charge = admission.charge_for_foundation();
    let compute_bytes = raw_charge
        .bytes
        .checked_add(64)
        .expect("fixture compute envelope is finite");
    let constrained = ResourceLimits::new(
        ResourceVector::new(1, raw_charge.bytes, raw_charge.edges, 1),
        ResourceVector::new(1, raw_charge.bytes, raw_charge.edges, 1),
        ResourceVector::new(1, raw_charge.bytes, raw_charge.edges, 1),
        AcceptedResources::new(1, compute_bytes, compute_bytes, 1),
        ComputeLimits::new(compute_bytes, compute_bytes, raw_charge.edges),
    )
    .expect("the full retained partition has one separate transient slot");
    let mut authority = TxPoolAuthority::for_foundation(constrained);
    let hash = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("raw residency fills the retained partition exactly"),
    );
    let (_, work) = take_resolve_work(
        authority
            .plan_checkout_next(WorkPermit::ResolveOnly)
            .expect("checkout is independent of retained byte fullness")
            .expect("the active slot is free")
            .apply(),
    );
    let evidence = resolution_evidence(
        work.transaction(),
        Capacity::shannons(1),
        raw_charge.bytes + 1,
        VerifyCycleClass::Small,
    );
    apply_plan(
        authority
            .apply_settlement(
                work.resolved(evidence)
                    .expect("the result fits its transient grant"),
            )
            .expect("retained growth denial is a total settlement"),
    );
    assert!(authority.entry(&hash).is_none());
    assert_eq!(
        authority.resources().preaccepted(),
        ResourceVector::default()
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_idle_peer_cardinality_does_not_expand_checkout_probe_work() {
    const OWNERS: usize = 64;
    let admissions = (0..OWNERS)
        .map(|offset| {
            ValidatedAdmission::remote(tx(1_100 + offset as u64), PeerIndex::from(1_000 + offset))
                .expect("owner-cardinality fixture admission is valid")
        })
        .collect::<Vec<_>>();
    let total = admissions
        .iter()
        .map(|admission| admission.charge_for_foundation())
        .reduce(add_resources)
        .expect("fixture has owners");
    let one = admissions[0].charge_for_foundation();
    let constrained = ResourceLimits::new(
        ResourceVector::new(total.entries, total.bytes, total.edges, 1),
        ResourceVector::new(total.entries, total.bytes, total.edges, 1),
        ResourceVector::new(one.entries, one.bytes, one.edges, 1),
        AcceptedResources::new(OWNERS, total.bytes, total.bytes, 1),
        ComputeLimits::new(4 * 1024, 4 * 1024, 16),
    )
    .expect("idle owners share one statically bounded active slot");
    let mut authority = TxPoolAuthority::for_foundation(constrained);
    for admission in admissions {
        apply_plan(
            authority
                .plan_admission(admission)
                .expect("every owner fits the retained partition"),
        );
    }
    let (plan, probes) = authority
        .plan_checkout_next_with_probe_count_for_foundation(WorkPermit::ResolveOnly)
        .expect("an idle owner head is constructively runnable");
    assert_eq!(probes, 1);
    let (_, work) = take_resolve_work(plan.expect("one owner is selected").apply());
    apply_plan(
        authority
            .apply_settlement(work.rejected(RejectionKind::Policy))
            .expect("selected lease settles"),
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_ready_frontier_and_independent_settlement_share_one_order() {
    let (mut authority, hashes) = independent_fixture(3);
    assert_eq!(
        authority.ready_for_reference(),
        vec![
            (hashes[2].clone(), owner_version(&authority, &hashes[2])),
            (hashes[1].clone(), owner_version(&authority, &hashes[1])),
            (hashes[0].clone(), owner_version(&authority, &hashes[0])),
        ]
    );

    let batch = independent_batch(
        &authority,
        &[hashes[0].clone(), hashes[2].clone(), hashes[1].clone()],
    );
    let SettlementPlan::IndependentRun(plan) = authority
        .plan_settlement(&batch)
        .expect("independent ready owners classify")
    else {
        panic!("chain-only candidates are independent");
    };
    assert_eq!(
        plan.independent_order_for_foundation()
            .expect("Ready and settlement expose one sealed test order"),
        vec![hashes[2].clone(), hashes[1].clone(), hashes[0].clone()]
    );
    let committed = plan.apply();
    assert!(authority.ready_for_reference().is_empty());
    assert!(authority.primary_projection_consistent());
    drop(committed);
}

#[test]
fn uak_small_cycle_capability_never_checks_out_large_verify_work() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 609, 65);
    let checkout = authority
        .plan_checkout_next(WorkPermit::ResolveThenVerify(
            VerifyCapability::SmallCycleOnly,
        ))
        .expect("resolve frontier is valid")
        .expect("resolve work is available")
        .apply();
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.into_work() else {
        panic!("continuous permit returns continuous resolve work");
    };
    let payload = resolved_payload(resolve.transaction());
    let before_continuation = authority.normalized_snapshot();
    let ContinuousResolution::Settle(yielded) = resolve
        .into_verify_as(payload, VerifyCycleClass::Large)
        .expect("fixture payload belongs to the checked-out transaction")
    else {
        panic!("small-cycle capability cannot continue large verification");
    };
    assert_eq!(authority.normalized_snapshot(), before_continuation);
    apply_plan(
        authority
            .apply_settlement(yielded)
            .expect("large verification yields one queued owner"),
    );

    let before_small_checkout = authority.normalized_snapshot();
    assert!(
        authority
            .plan_checkout_next(WorkPermit::VerifyOnly(VerifyCapability::SmallCycleOnly,))
            .expect("small frontier lookup is valid")
            .is_none()
    );
    assert_eq!(authority.normalized_snapshot(), before_small_checkout);

    let checkout = authority
        .plan_checkout_next(WorkPermit::VerifyOnly(VerifyCapability::Any))
        .expect("general frontier lookup is valid")
        .expect("general worker can consume large verification")
        .apply();
    let CheckedOutWork::Verify(verify) = checkout.into_work() else {
        panic!("verify-only permit returns verify work");
    };
    apply_plan(
        authority
            .apply_settlement(verify.rejected(RejectionKind::Verification))
            .expect("large verification lease settles"),
    );
    assert!(authority.entry(&hash).is_none());
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_small_cycle_frontier_finds_work_behind_same_owner_large_head() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let large_hash = admit_remote(&mut authority, 610, 66);
    let small_hash = admit_remote(&mut authority, 611, 66);

    let large_version = owner_version(&authority, &large_hash);
    let (_, large_resolve) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(&large_hash, large_version, WorkPermit::ResolveOnly)
            .expect("large fixture resolve plans")
            .apply(),
    );
    let large_payload = resolved_payload_with_facts(
        large_resolve.transaction(),
        Vec::new(),
        Vec::new(),
        Capacity::shannons(10_000),
    );
    apply_plan(
        authority
            .apply_settlement(
                large_resolve
                    .yield_verify_as(large_payload, VerifyCycleClass::Large)
                    .expect("large fixture payload matches"),
            )
            .expect("large fixture yields"),
    );

    let small_version = owner_version(&authority, &small_hash);
    let (_, small_resolve) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(&small_hash, small_version, WorkPermit::ResolveOnly)
            .expect("small fixture resolve plans")
            .apply(),
    );
    let small_payload = resolved_payload_with_facts(
        small_resolve.transaction(),
        Vec::new(),
        Vec::new(),
        Capacity::shannons(1),
    );
    apply_plan(
        authority
            .apply_settlement(
                small_resolve
                    .yield_verify(small_payload)
                    .expect("small fixture payload matches"),
            )
            .expect("small fixture yields"),
    );

    let (selected, small_verify) = {
        let committed = authority
            .plan_checkout_next(WorkPermit::VerifyOnly(VerifyCapability::SmallCycleOnly))
            .expect("small frontier lookup is valid")
            .expect("small work is not hidden by the large head")
            .apply();
        let CheckedOutWork::Verify(work) = committed.into_work() else {
            panic!("verify-only permit returns verify work");
        };
        (TxIdentity::from_transaction(work.transaction()).raw, work)
    };
    assert_eq!(selected, small_hash);
    apply_plan(
        authority
            .apply_settlement(small_verify.rejected(RejectionKind::Verification))
            .expect("small lease settles"),
    );

    let committed = authority
        .plan_checkout_next(WorkPermit::VerifyOnly(VerifyCapability::Any))
        .expect("general frontier lookup is valid")
        .expect("large work remains")
        .apply();
    let CheckedOutWork::Verify(large_verify) = committed.into_work() else {
        panic!("verify-only permit returns verify work");
    };
    assert_eq!(
        TxIdentity::from_transaction(large_verify.transaction()).raw,
        large_hash
    );
    apply_plan(
        authority
            .apply_settlement(large_verify.rejected(RejectionKind::Verification))
            .expect("large lease settles"),
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_runner_cancellation_settles_one_exact_work_capability_before_exit() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 612, 67);
    let checkout = authority
        .plan_checkout_next(WorkPermit::ResolveThenVerify(VerifyCapability::Any))
        .expect("frontier lookup is valid")
        .expect("work is available")
        .apply();
    assert_eq!(authority.resources().preaccepted().active_work, 1);
    let cancellation = checkout.into_work().cancelled();
    apply_plan(
        authority
            .apply_settlement(cancellation)
            .expect("current cancellation receipt settles"),
    );
    assert_eq!(authority.resources().preaccepted().active_work, 0);
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert!(authority.primary_projection_consistent());
}
