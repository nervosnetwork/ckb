use super::super::chain::{AcceptedProof, ProposalContextReceipt};
use super::super::effect::{
    CommittedAcceptance, CommittedEffect, CommittedRejection, EffectPolicy, RejectionAudience,
};
use super::super::plan::{
    AcceptedOrderKey, AdminCause, AncestorAggregate, AuthorityFault, Backpressure,
    CandidateBatchError, CandidateDispositionPlan, CommittedChange, CommittedChanges,
    CommittedDelta, ComponentLimitKind, ComputeSettlementFailure, ComputeSettlementRecovery,
    DescendantAggregate, DirectAdmissionDisposition, EvictionOrderKey, IndependentCoupling,
    MembershipReject, MembershipSnapshot, PlanError, PreparedApply, RemovalCause, SettlementBatch,
    SettlementPlan, StalePlan, StatusCounts, TxPoolAuthority,
};
use super::super::resources::{
    AcceptedCost, AcceptedResources, ChargeRecord, ComputeLimits, ComputeReleaseError,
    ResourceConfigError, ResourceLedger, ResourceLimits, ResourceSnapshot, ResourceVector,
};
use super::super::runtime::{AuthorityMaintenanceOutcome, AuthorityRuntime};
use super::super::scheduler::VerifyOrder;
use super::super::state::{
    AcceptedAtMillis, AcceptedEntry, AcceptedProvenance, AcceptedStatus, ActiveWork, ApplySequence,
    CandidateMetrics, ChainRevision, ChainViewId, ComputeAttribution, ComputeGrant, ComputeLeaseId,
    DependencyCut, DependencyKey, EntryVersion, ExpandedFootprint, FootprintError,
    FoundationResolution, InputEvidenceError, KnownDependencies, ObservedDependencies, OwnedTx,
    PayloadPolicy, PoolGeneration, PreAcceptedPhase, PreAcceptedSource, ProposalBase, QueuedWork,
    RawTxHash, RejectionKind, RemoteDeadline, RemoteResidencyLease, ResolvedPayload, TxIdentity,
    ValidatedAdmission, VerifiedFacts, VerifyCapability, VerifyCycleClass, WorkPermit,
};
use super::super::work::{
    CheckedOutWork, ComputeSettlement, ContinuousResolution, ContinuousResolveWork,
    ContinuousVerifyWork, ResolutionEvidence, ResolutionReceiptError, ResolveWork, SettlementNext,
    SettlementToken, VerifyWork,
};
use crate::{
    component::entry::{accepted_transaction_charge_bytes, resolved_transaction_charge_bytes},
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
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::num::NonZeroUsize;
use std::sync::Arc;

pub(super) fn runtime_config() -> TxPoolConfig {
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

pub(super) fn missing_keys() -> Vec<DependencyKey> {
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
    apply_without_work(
        authority
            .plan_admission(admission)
            .expect("fixture admission plans"),
    );
    hash
}

pub(super) fn admit_remote_until(
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
    apply_without_work(
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
    apply_without_work(
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
    apply_without_work(
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

fn checkout_remote_for_verify_with_claim(
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
    apply_without_work(
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
    apply_without_work(
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
    let CheckedOutWork::Verify(verify) = checkout.into_work().expect("verify work exists") else {
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

impl FixtureCommit for CommittedDelta {
    fn into_committed(self) -> CommittedDelta {
        self
    }
}

pub(super) fn apply_without_work(commit: impl FixtureCommit) {
    let _ = apply_committed_without_work(commit);
}

fn apply_committed_without_work(commit: impl FixtureCommit) -> CommittedDelta {
    let committed = commit.into_committed();
    assert!(
        committed.handoff_is_none(),
        "transition unexpectedly issued work"
    );
    committed
}

fn drain_fixture_effects(authority: &mut TxPoolAuthority) {
    loop {
        let Some(checkout) = authority
            .plan_effect_checkout_for_foundation()
            .expect("fixture effect checkout plans")
        else {
            break;
        };
        let lease = checkout
            .apply()
            .into_effect_lease()
            .expect("fixture checkout returns one effect lease");
        let committed = authority
            .apply_effect_settlement_for_foundation(lease.complete_for_foundation().published())
            .expect("fixture effect publication settles");
        assert!(committed.handoff_is_none());
    }
}

pub(super) fn take_resolve_work(committed: impl Into<CommittedDelta>) -> (RawTxHash, ResolveWork) {
    let committed = committed.into();
    let CheckedOutWork::Resolve(work) = committed.into_work().expect("resolve work exists") else {
        panic!("resolve-only checkout returns resolve work");
    };
    let hash = TxIdentity::from_transaction(work.transaction()).raw;
    (hash, work)
}

fn only_committed_change(committed: &CommittedDelta) -> &CommittedChange {
    let CommittedChanges::One(change) = &committed.changes else {
        panic!("fixture expected one committed change");
    };
    change
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
    let payload = Arc::new(
        resolved_payload_with_facts(transaction, expanded_dependencies, chain_inputs, fee)
            .into_payload(),
    );
    let serialized_bytes = payload.serialized_bytes();
    let resident_bytes =
        accepted_transaction_charge_bytes(serialized_bytes, payload.resolved_transaction());
    VerifiedFacts::for_foundation_view(
        chain_view,
        DependencyCut(ApplySequence(0)),
        payload,
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
    apply_without_work(
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
    apply_without_work(
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
    apply_without_work(
        authority
            .plan_accept_at_for_foundation(&hash, version, status, accepted_at)
            .expect("timestamped fixture membership plans"),
    );
    drain_fixture_effects(authority);
    hash
}

pub(super) fn accepted_parent_child_at(
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
    apply_without_work(
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
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.into_work().expect("work exists")
    else {
        panic!("continuous permit returns continuous resolve work");
    };
    let (verify, _) = continue_fixture_verify(resolve, payload);
    apply_without_work(
        authority
            .apply_settlement(verify.verified_under(cycles, rules))
            .expect("fixture verification settles"),
    );
    hash
}

fn independent_fixture(count: usize) -> (TxPoolAuthority, Vec<RawTxHash>) {
    let mut authority = TxPoolAuthority::for_foundation(limits());
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

pub(super) fn independent_batch(
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

fn coupled_reason_and_drop(plan: SettlementPlan<'_>) -> IndependentCoupling {
    let SettlementPlan::CoupledComponent {
        reason,
        disposition,
    } = plan
    else {
        panic!("fixture expected a coupled settlement");
    };
    drop(disposition);
    reason
}

fn accepted_disposition(disposition: CandidateDispositionPlan<'_>) -> PreparedApply<'_> {
    let CandidateDispositionPlan::Accepted(plan) = disposition else {
        panic!("fixture candidate must be accepted");
    };
    plan
}

fn rejected_coupled_reason_and_drop(plan: SettlementPlan<'_>) -> MembershipReject {
    let SettlementPlan::CoupledComponent { disposition, .. } = plan else {
        panic!("fixture expected a coupled settlement");
    };
    let CandidateDispositionPlan::Rejected(rejection) = disposition else {
        panic!("fixture candidate must be rejected");
    };
    rejection.reason().clone()
}

pub(super) fn assert_resource_reference(authority: &TxPoolAuthority) {
    let mut charges = HashMap::new();
    let mut preaccepted = ResourceVector::default();
    let mut remote = ResourceVector::default();
    let mut peers = HashMap::new();
    let mut replacement_history = ResourceVector::default();
    let mut accepted = AcceptedResources::default();
    for (hash, owner) in authority.entries_for_reference() {
        let charge = owner.charge_record();
        assert!(charges.insert(hash.clone(), charge).is_none());
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
            charges,
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
    let accepted = authority
        .entries_for_reference()
        .iter()
        .filter_map(|(hash, owner)| match owner {
            OwnedTx::Accepted(entry) => Some((hash, entry)),
            OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_) => None,
        })
        .collect::<HashMap<_, _>>();
    let mut spenders = HashMap::new();
    let mut dependency_readers = HashMap::<OutPoint, HashSet<_>>::new();
    let mut parents = accepted
        .keys()
        .map(|hash| ((*hash).clone(), HashSet::new()))
        .collect::<HashMap<_, _>>();
    let mut children = parents.clone();
    let mut counts = StatusCounts::default();

    for (hash, entry) in &accepted {
        match entry.status() {
            AcceptedStatus::Pending => {
                counts.pending = counts.pending.checked_add(1).expect("fixture count fits")
            }
            AcceptedStatus::Gap => {
                counts.gap = counts.gap.checked_add(1).expect("fixture count fits")
            }
            AcceptedStatus::Proposed => {
                counts.proposed = counts.proposed.checked_add(1).expect("fixture count fits")
            }
        }
        for input in entry.proof.payload().footprint.inputs() {
            assert!(
                spenders.insert(input.clone(), (*hash).clone()).is_none(),
                "accepted input has one spender"
            );
        }
        for dependency in entry.proof.payload().footprint.dependencies() {
            dependency_readers
                .entry(dependency.clone())
                .or_default()
                .insert((*hash).clone());
        }
        for out_point in entry
            .proof
            .payload()
            .footprint
            .inputs()
            .iter()
            .chain(entry.proof.payload().footprint.dependencies())
        {
            let parent = super::super::state::RawTxHash(out_point.tx_hash());
            if !accepted.contains_key(&parent) {
                continue;
            }
            parents
                .get_mut(*hash)
                .expect("accepted candidate has a parent row")
                .insert(parent.clone());
            children
                .get_mut(&parent)
                .expect("accepted parent has a child row")
                .insert((*hash).clone());
        }
    }

    let mut ancestor_aggregates = HashMap::new();
    let mut descendant_aggregates = HashMap::new();
    let mut accepted_order = BTreeSet::new();
    let mut eviction_order = BTreeSet::new();
    for (root, root_entry) in &accepted {
        let mut ancestor_aggregate = AncestorAggregate::default();
        let mut visited = HashSet::new();
        let mut frontier = VecDeque::from([(*root).clone()]);
        while let Some(ancestor) = frontier.pop_front() {
            if !visited.insert(ancestor.clone()) {
                continue;
            }
            let entry = accepted
                .get(&ancestor)
                .expect("accepted ancestor has a primary entry");
            let cost = entry.proof.metrics().cost;
            ancestor_aggregate.entries = ancestor_aggregate
                .entries
                .checked_add(1)
                .expect("fixture ancestor count fits");
            ancestor_aggregate.serialized_bytes = ancestor_aggregate
                .serialized_bytes
                .checked_add(cost.serialized_bytes)
                .expect("fixture ancestor size fits");
            ancestor_aggregate.cycles = ancestor_aggregate
                .cycles
                .checked_add(cost.cycles)
                .expect("fixture ancestor cycles fit");
            ancestor_aggregate.fee = ancestor_aggregate
                .fee
                .safe_add(entry.proof.metrics().fee)
                .expect("fixture ancestor fee fits");
            frontier.extend(
                parents
                    .get(&ancestor)
                    .expect("accepted ancestor has a parent row")
                    .iter()
                    .cloned(),
            );
        }
        ancestor_aggregates.insert((*root).clone(), ancestor_aggregate);
        accepted_order.insert(AcceptedOrderKey::new(root_entry, ancestor_aggregate));

        let mut aggregate = DescendantAggregate::default();
        let mut visited = HashSet::new();
        let mut frontier = VecDeque::from([(*root).clone()]);
        while let Some(descendant) = frontier.pop_front() {
            if !visited.insert(descendant.clone()) {
                continue;
            }
            let entry = accepted
                .get(&descendant)
                .expect("accepted descendant has a primary entry");
            let cost = entry.proof.metrics().cost;
            aggregate.entries = aggregate
                .entries
                .checked_add(1)
                .expect("fixture aggregate count fits");
            aggregate.serialized_bytes = aggregate
                .serialized_bytes
                .checked_add(cost.serialized_bytes)
                .expect("fixture aggregate size fits");
            aggregate.cycles = aggregate
                .cycles
                .checked_add(cost.cycles)
                .expect("fixture aggregate cycles fit");
            aggregate.fee = aggregate
                .fee
                .safe_add(entry.proof.metrics().fee)
                .expect("fixture aggregate fee fits");
            frontier.extend(
                children
                    .get(&descendant)
                    .expect("accepted descendant has a child row")
                    .iter()
                    .cloned(),
            );
        }
        descendant_aggregates.insert((*root).clone(), aggregate);
        let cost = root_entry.proof.metrics().cost;
        let self_rate = FeeRate::calculate(
            root_entry.proof.metrics().fee,
            get_transaction_weight(cost.serialized_bytes, cost.cycles),
        );
        let descendants_rate = FeeRate::calculate(
            aggregate.fee,
            get_transaction_weight(aggregate.serialized_bytes, aggregate.cycles),
        );
        eviction_order.insert(EvictionOrderKey {
            status: root_entry.status(),
            fee_rate: self_rate.max(descendants_rate),
            descendants_count: aggregate.entries,
            arrival: root_entry.record.arrival,
            hash: (*root).clone(),
        });
    }

    assert_eq!(
        authority.membership_snapshot_for_reference(),
        MembershipSnapshot {
            spenders,
            dependency_readers,
            parents,
            children,
            ancestor_aggregates,
            descendant_aggregates,
            accepted_order,
            eviction_order,
            counts,
        }
    );
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
    let delta = authority
        .plan_admission(admission)
        .expect("bounded first admission plans")
        .apply();

    assert_eq!(only_committed_change(&delta).changed, hash);
    assert!(delta.handoff_is_none());
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
    apply_without_work(
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
fn uak_accepted_source_ignores_preaccepted_work_and_status_only_changes() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    assert_eq!(authority.accepted_source_for_reference(), ApplySequence(0));

    let disposable = admit_remote(&mut authority, 1_704, 704);
    assert_eq!(
        authority.accepted_source_for_reference(),
        ApplySequence(0),
        "pre-acceptance owner changes cannot stale a ChainPlan"
    );
    let version = owner_version(&authority, &disposable);
    apply_without_work(
        authority
            .plan_terminalize_for_foundation(&disposable, version)
            .expect("pre-accepted terminalization plans"),
    );
    assert_eq!(authority.accepted_source_for_reference(), ApplySequence(0));

    let accepted = accept_remote_transaction(
        &mut authority,
        tx(1_705),
        705,
        AcceptedStatus::Gap,
        Vec::new(),
    );
    let accepted_source = authority.accepted_source_for_reference();
    assert_ne!(accepted_source, ApplySequence(0));

    let version = owner_version(&authority, &accepted);
    let status_change = apply_committed_without_work(
        authority
            .plan_status_for_foundation(&accepted, version, AcceptedStatus::Pending)
            .expect("status-only transition plans"),
    );
    let _status_sequence = only_committed_change(&status_change).sequence;
    assert_eq!(
        authority.accepted_source_for_reference(),
        accepted_source,
        "status-only mutation must not invalidate accepted-content work"
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_accepted_timestamp_is_part_of_the_immutable_source_cut() {
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
        .cloned()
        .expect("fixture accepted owner exists");
    let OwnedTx::Accepted(mut changed) = before.clone() else {
        panic!("fixture owner is accepted");
    };
    changed.accepted_at = AcceptedAtMillis(1);
    assert!(
        super::super::source::replacement_changes_accepted_source_for_foundation(
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
    assert_eq!(authority.normalized_snapshot(), before);

    let old_clocks = authority.clocks();
    let old_chain = authority.chain_view().clone();
    let old_accepted_source = authority.accepted_source_for_reference();
    let committed = authority
        .plan_clear_pipeline()
        .expect("clear replans")
        .apply();
    let CommittedChanges::ClearPipelineControl { changed_owners, .. } = committed.changes else {
        panic!("fixture expected one pipeline-clear commit");
    };
    assert_eq!(changed_owners, 1);
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
    assert_eq!(
        authority.accepted_source_for_reference(),
        old_accepted_source
    );
    assert_eq!(authority.clocks().next_version, old_clocks.next_version);
    assert_eq!(authority.clocks().next_lease, old_clocks.next_lease);
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
        .plan_effect_checkout_for_foundation()
        .expect("reset checkout plans")
        .expect("generation swap commits one reset")
        .apply()
        .into_effect_lease()
        .expect("reset has one publisher lease");
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
    assert!(matches!(
        committed.changes,
        CommittedChanges::ClearPoolControl(_)
    ));
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

#[test]
fn uak_runtime_clear_scopes_and_snapshot_pairing_are_indivisible() {
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
        AuthorityMaintenanceOutcome::Applied { owners: 1 }
    );
    assert_eq!(
        runtime
            .expire_accepted_due()
            .expect("runtime derives the accepted cutoff from expiry_hours"),
        AuthorityMaintenanceOutcome::Applied { owners: 2 }
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

    let committed = authority
        .plan_local_removal(&parent)
        .expect("the complete descendant closure plans")
        .expect("the root is present")
        .apply();
    assert!(matches!(
        committed.changes,
        CommittedChanges::AdminControl {
            cause: AdminCause::LocalRemoval { ref root },
            changed_owners: 2,
            ..
        } if root == &parent
    ));
    assert_eq!(committed.retired_len(), 2);
    assert!(authority.entry(&parent).is_none());
    assert!(authority.entry(&child).is_none());
    assert_eq!(authority.owner_count(), 0);
    assert_eq!(authority.charged_count(), 0);
    assert!(
        authority
            .plan_effect_checkout_for_foundation()
            .expect("effect lookup remains valid")
            .is_none(),
        "trusted local removal must not invent an Accepted rejection"
    );
    assert!(
        authority
            .plan_local_removal(&parent)
            .expect("an absent lookup is not a structural error")
            .is_none()
    );
    assert_resource_reference(&authority);
    assert!(authority.primary_projection_consistent());
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
        .plan_local_removal(&hash)
        .expect("active removal plans without a drain")
        .expect("the owner is present")
        .apply();
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
        .plan_effect_checkout_for_foundation()
        .expect("release checkout plans")
        .expect("removal commits relay cleanup")
        .apply()
        .into_effect_lease()
        .expect("cleanup has one publisher lease");
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
    apply_without_work(
        authority
            .plan_admission(proposal)
            .expect("trusted proposal admission plans"),
    );
    let recovery = ValidatedAdmission::recovery(tx(1_727), PoolGeneration(0))
        .expect("fixture recovery admission is valid");
    let recovery_hash = recovery.identity.raw.clone();
    apply_without_work(
        authority
            .plan_admission(recovery)
            .expect("recovery admission plans"),
    );

    for hash in [&proposal_hash, &recovery_hash] {
        drop(
            authority
                .plan_local_removal(hash)
                .expect("local removal plans")
                .expect("the owner exists")
                .apply(),
        );
    }

    assert!(
        authority
            .plan_effect_checkout_for_foundation()
            .expect("an empty effect log is valid")
            .is_none(),
        "owners without remote ingress attribution must not mutate relay projections"
    );
    assert_resource_reference(&authority);
    assert!(authority.primary_projection_consistent());
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
            .plan_accepted_expiry(AcceptedAtMillis(9))
            .expect("pre-deadline lookup is valid")
            .is_none()
    );

    let version = owner_version(&authority, &parent);
    apply_without_work(
        authority
            .plan_status_for_foundation(&parent, version, AcceptedStatus::Gap)
            .expect("status-only version churn plans"),
    );
    drain_fixture_effects(&mut authority);

    let committed = authority
        .plan_accepted_expiry(AcceptedAtMillis(10))
        .expect("the stable accepted deadline remains indexed")
        .expect("the oldest root is due")
        .apply();
    assert!(matches!(
        committed.changes,
        CommittedChanges::AdminControl {
            cause: AdminCause::AcceptedExpiry {
                cutoff: AcceptedAtMillis(10)
            },
            changed_owners: 2,
            ..
        }
    ));
    assert_eq!(committed.retired_len(), 2);
    assert!(authority.entry(&parent).is_none());
    assert!(authority.entry(&child).is_none());

    let effect = authority
        .plan_effect_checkout_for_foundation()
        .expect("expiry checkout plans")
        .expect("the atomic removal publishes every exact outcome")
        .apply()
        .into_effect_lease()
        .expect("expiry has one publisher lease");
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
    apply_without_work(
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
    assert!(matches!(
        &revoked.changes,
        CommittedChanges::AdminControl {
            cause: AdminCause::PeerRevocation(peer),
            ..
        } if *peer == banned
    ));
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
        .plan_effect_checkout_for_foundation()
        .expect("revocation effect checkout plans")
        .expect("revocation committed one cleanup batch")
        .apply()
        .into_effect_lease()
        .expect("cleanup has one publisher lease");
    assert!(matches!(
        effect.effects(),
        [CommittedEffect::PeerCohortRevoked(revocation)]
            if revocation.peer() == banned && revocation.culprit().is_none()
    ));

    let blocked = ValidatedAdmission::remote(queued_tx.clone(), banned)
        .expect("same-peer retry remains structurally valid");
    assert_eq!(
        authority.plan_admission(blocked).err(),
        Some(PlanError::IngressRevoked(banned))
    );

    let resubmitted = ValidatedAdmission::remote(queued_tx, survivor_peer)
        .expect("another peer may provide the same raw transaction");
    apply_without_work(
        authority
            .plan_admission(resubmitted)
            .expect("peer cleanup does not install a raw-hash tombstone"),
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_peer_revocation_removes_active_owner_and_makes_its_lease_stale() {
    let peer = PeerIndex::from(710);
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(1_712);
    let hash = admit_remote(&mut authority, 1_712, 710);
    apply_without_work(
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
    let transaction = tx(1_715);
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

    let retry = ValidatedAdmission::remote(transaction, peer)
        .expect("the retry remains structurally valid");
    assert_eq!(
        authority.plan_admission(retry).err(),
        Some(PlanError::IngressRevoked(peer)),
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

    let queued_before_ban = ValidatedAdmission::remote(transaction.clone(), peer)
        .expect("an already queued controller message remains structurally valid");
    assert_eq!(
        authority.plan_admission(queued_before_ban).err(),
        Some(PlanError::IngressRevoked(peer)),
        "the authority marker linearizes the ban against delayed ingress"
    );

    apply_without_work(
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
        .apply();
    assert!(matches!(
        committed.changes,
        CommittedChanges::AdminControl {
            cause: AdminCause::RemoteExpiry {
                cutoff: RemoteDeadline(10)
            },
            changed_owners: 1,
            ..
        }
    ));
    assert_eq!(committed.retired_len(), 1);
    assert!(authority.entry(&expired).is_none());
    assert!(authority.entry(&future).is_some());

    let effect = authority
        .plan_effect_checkout_for_foundation()
        .expect("expiry effect checkout plans")
        .expect("expiry committed one cleanup batch")
        .apply()
        .into_effect_lease()
        .expect("cleanup has one publisher lease");
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
    apply_without_work(
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
    apply_without_work(
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
    let CheckedOutWork::Resolve(resolve) = checkout.into_work().expect("resolve work exists")
    else {
        panic!("resolve-only permit returns resolve work");
    };
    apply_without_work(
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

    apply_without_work(
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
    let CheckedOutWork::Resolve(resolve) = checkout.into_work().expect("resolve work exists")
    else {
        panic!("resolve-only permit returns resolve work");
    };
    apply_without_work(
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
        .apply();
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
        .plan_effect_checkout_for_foundation()
        .expect("expiry effect checkout plans")
        .expect("the exact due prefix committed one batch")
        .apply()
        .into_effect_lease()
        .expect("expiry has one publisher lease");
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
    apply_without_work(
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
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.into_work().expect("work exists")
    else {
        panic!("continuous permit returns continuous resolve work");
    };

    let duplicate = ValidatedAdmission::proposal(transaction).expect("fixture promotion is valid");
    apply_without_work(
        authority
            .plan_admission(duplicate)
            .expect("proposal promotes the existing owner"),
    );
    assert_eq!(authority.owner_count(), 1);
    assert_eq!(authority.charged_count(), 1);
    let owner = authority.entry(&hash).expect("promoted owner exists");
    assert!(matches!(
        owner,
        OwnedTx::PreAccepted(entry)
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
        OwnedTx::PreAccepted(entry)
            if matches!(entry.phase, PreAcceptedPhase::Computing(_))
    ));
    assert!(authority.primary_projection_consistent());
    assert_resource_reference(&authority);

    apply_without_work(
        authority
            .apply_settlement(resolve.rejected(RejectionKind::Policy))
            .expect("promotion does not invalidate the active compute lease"),
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
    apply_without_work(
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
    apply_without_work(
        authority
            .plan_admission(initial)
            .expect("remote variant enters ownership"),
    );
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if entry.source.payload_policy()
                == PayloadPolicy::RemoteDeclaredCycles(declared_cycles)
    ));

    let replacement =
        ValidatedAdmission::proposal(trusted.clone()).expect("trusted replacement is valid");
    apply_without_work(
        authority
            .plan_admission(replacement)
            .expect("trusted witness replaces the inactive remote payload"),
    );

    let owner = authority.entry(&hash).expect("replacement owner exists");
    assert_eq!(owner.record().tx.witness_hash(), trusted.witness_hash());
    assert_eq!(owner.ingress_peer(), Some(peer));
    assert!(matches!(
        owner,
        OwnedTx::PreAccepted(entry)
            if entry.source.payload_policy() == PayloadPolicy::Trusted
    ));
    assert!(owner.record().version > initial_version);
    assert!(matches!(
        owner,
        OwnedTx::PreAccepted(entry)
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
        PayloadPolicy::RemoteDeclaredCycles(declared_cycles)
    );
    let stale_rejection = verify.verified(declared_cycles + 1);

    apply_without_work(
        authority
            .plan_admission(
                ValidatedAdmission::proposal(transaction)
                    .expect("same-witness proposal is trusted"),
            )
            .expect("proposal promotion preserves the active compute lease"),
    );
    apply_without_work(
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
            .plan_effect_checkout_for_foundation()
            .expect("effect projection remains coherent")
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
    let CheckedOutWork::Verify(verify) = checkout.into_work().expect("verify work exists") else {
        panic!("verify-only checkout returns verify work");
    };
    assert_eq!(verify.payload_policy(), PayloadPolicy::Trusted);
    apply_without_work(
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
        PayloadPolicy::RemoteDeclaredCycles(declared_cycles)
    );
    let stale_rejection = verify.rejected(RejectionKind::Verification);

    apply_without_work(
        authority
            .plan_admission(
                ValidatedAdmission::proposal(transaction)
                    .expect("same-witness proposal is trusted"),
            )
            .expect("proposal promotion preserves the active compute lease"),
    );
    apply_without_work(
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
            .plan_effect_checkout_for_foundation()
            .expect("effect projection remains coherent")
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
    apply_without_work(
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

    let checkout = authority
        .plan_effect_checkout_for_foundation()
        .expect("rejection effect checkout plans")
        .expect("rejection is committed with terminalization")
        .apply();
    let lease = checkout
        .into_effect_lease()
        .expect("rejection checkout returns one effect lease");
    assert!(matches!(
        lease.effects(),
        [CommittedEffect::PeerCohortRevoked(revocation)]
            if revocation.peer() == peer
                && revocation.culprit().is_some_and(|culprit|
                    culprit.tx_hash() == &hash
                        && matches!(culprit.reason().reject(), Reject::DeclaredWrongCycles(200, 201)))
    ));

    let blocked = ValidatedAdmission::remote(transaction.clone(), peer)
        .expect("same-peer retry remains structurally valid");
    assert_eq!(
        authority.plan_admission(blocked).err(),
        Some(PlanError::IngressRevoked(peer))
    );
    let other_peer = PeerIndex::from(6_271);
    apply_without_work(
        authority
            .plan_admission(
                ValidatedAdmission::remote(transaction, other_peer)
                    .expect("another peer may provide the same transaction"),
            )
            .expect("the ban marker is peer-scoped, not a tx tombstone"),
    );
    assert_resource_reference(&authority);
}

#[test]
fn uak_direct_local_admission_moves_from_absent_to_accepted_in_one_apply() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = Arc::new(tx(270));
    let hash = RawTxHash(transaction.hash());
    let verified = direct_verified_facts(
        &transaction,
        Vec::new(),
        Vec::new(),
        Capacity::shannons(1_000),
    );
    let disposition = authority
        .plan_direct_admission_for_foundation(
            Arc::clone(&transaction),
            verified,
            AcceptedStatus::Pending,
        )
        .expect("a validated local transaction has one direct disposition");
    let DirectAdmissionDisposition::Accepted(plan) = disposition else {
        panic!("vacant local admission must acquire Accepted ownership");
    };
    let committed = apply_committed_without_work(plan);
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

    let checkout = authority
        .plan_effect_checkout_for_foundation()
        .expect("direct admission effect checkout plans")
        .expect("direct admission publishes one outcome")
        .apply();
    let lease = checkout
        .into_effect_lease()
        .expect("direct admission checkout returns its effect lease");
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
fn uak_dropped_direct_local_plan_is_semantically_mutation_free() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = Arc::new(tx(271));
    let verified = direct_verified_facts(
        &transaction,
        Vec::new(),
        Vec::new(),
        Capacity::shannons(1_000),
    );
    let before = authority.normalized_snapshot();
    let disposition = authority
        .plan_direct_admission_for_foundation(transaction, verified, AcceptedStatus::Pending)
        .expect("direct admission plans without mutating authority state");
    drop(disposition);
    assert_eq!(authority.normalized_snapshot(), before);
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
    apply_without_work(
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
        .plan_direct_admission_for_foundation(
            Arc::new(local.clone()),
            verified,
            AcceptedStatus::Pending,
        )
        .expect("local payload supersedes the inactive same-raw owner");
    let DirectAdmissionDisposition::Accepted(plan) = disposition else {
        panic!("same-raw inactive PreAccepted owner is settled by local acceptance");
    };
    let committed = apply_committed_without_work(plan);
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
        }) if *ingress == peer
    ));
    assert_eq!(
        authority.resources().preaccepted(),
        ResourceVector::default()
    );
    assert_eq!(authority.resources().remote(), ResourceVector::default());
    assert_eq!(authority.resources().peer(peer), ResourceVector::default());
    assert_resource_reference(&authority);

    let checkout = authority
        .plan_effect_checkout_for_foundation()
        .expect("direct replacement effect checkout plans")
        .expect("direct replacement publishes one outcome")
        .apply();
    let lease = checkout
        .into_effect_lease()
        .expect("direct replacement returns its effect lease");
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
    apply_without_work(
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
        .plan_direct_admission_for_foundation(
            Arc::new(local.clone()),
            verified,
            AcceptedStatus::Pending,
        )
        .expect("validated Local acceptance replaces obsolete active work");
    let DirectAdmissionDisposition::Accepted(plan) = disposition else {
        panic!("the direct result must install Accepted ownership");
    };
    let committed = apply_committed_without_work(plan);
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
        .plan_direct_admission_for_foundation(
            Arc::new(transaction.clone()),
            verified,
            AcceptedStatus::Pending,
        )
        .expect("an accepted raw hash has a deterministic duplicate outcome");
    let DirectAdmissionDisposition::Duplicate(duplicate) = disposition else {
        panic!("Accepted ownership dominates a racing direct receipt");
    };
    assert_eq!(duplicate.key(), &hash);
    let (duplicate_hash, committed) = duplicate.apply();
    assert_eq!(duplicate_hash, hash);
    assert!(committed.handoff_is_none());
    assert_eq!(owner_version(&authority, &hash), version);
    assert_eq!(authority.resources().accepted(), accepted_resources);

    let checkout = authority
        .plan_effect_checkout_for_foundation()
        .expect("duplicate effect checkout plans")
        .expect("duplicate commits one accepted relay outcome")
        .apply();
    let lease = checkout
        .into_effect_lease()
        .expect("duplicate checkout returns its effect lease");
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
        .plan_direct_admission_for_foundation(
            Arc::new(replacement.clone()),
            verified,
            AcceptedStatus::Pending,
        )
        .expect("under-fee RBF is a transaction outcome, not an authority fault");
    let DirectAdmissionDisposition::Rejected(rejected) = disposition else {
        panic!("replacement must pay the victim fee plus the configured increment");
    };
    assert!(matches!(
        rejected.reason(),
        MembershipReject::InsufficientReplacementFee { .. }
    ));
    let (reason, committed) = rejected.apply();
    assert!(committed.handoff_is_none());
    assert!(matches!(
        reason,
        MembershipReject::InsufficientReplacementFee { .. }
    ));
    assert!(authority.entry(&replacement_hash).is_none());
    assert_eq!(owner_version(&authority, &victim), victim_version);
    assert_eq!(authority.resources().accepted(), accepted_resources);
    assert_eq!(authority.owner_count(), 1);

    let checkout = authority
        .plan_effect_checkout_for_foundation()
        .expect("direct reject effect checkout plans")
        .expect("direct reject commits one exact outcome")
        .apply();
    let lease = checkout
        .into_effect_lease()
        .expect("direct reject checkout returns its effect lease");
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
    apply_without_work(
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
    apply_without_work(
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
    apply_without_work(
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
    apply_without_work(
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
    apply_without_work(
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
            vec![CommittedEffect::Rejected(CommittedRejection::Foundation {
                tx: Arc::clone(&retained_tx),
                audience: RejectionAudience::foundation(),
                reason: RejectionKind::Policy,
            })],
        )
        .expect("fixture effect is bounded");
    let terminal = authority
        .plan_terminalize_with_effect_for_foundation(&hash, version, &publication)
        .expect("terminal plan is complete")
        .apply();

    assert_eq!(only_committed_change(&terminal).changed, hash);
    assert!(terminal.handoff_is_none());
    assert_eq!(authority.owner_count(), 0);
    assert_eq!(authority.charged_count(), 0);
    assert!(authority.primary_projection_consistent());
    assert_eq!(terminal.retired_len(), 1);
    assert_eq!(terminal.retired_effect_len(), 0);
    assert_eq!(Arc::strong_count(&retained_tx), 3);
    drop(terminal);
    drop(publication);
    assert_eq!(Arc::strong_count(&retained_tx), 2);

    let checkout = authority
        .plan_effect_checkout_for_foundation()
        .expect("effect checkout plans")
        .expect("committed effect is available")
        .apply();
    let lease = checkout
        .into_effect_lease()
        .expect("effect checkout returns the only lease");
    assert_eq!(lease.effects().len(), 1);
    assert!(matches!(
        &lease.effects()[0],
        CommittedEffect::Rejected(CommittedRejection::Foundation { tx, reason, .. })
            if Arc::ptr_eq(tx, &retained_tx)
                && *reason == RejectionKind::Policy
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
            lease: ComputeLeaseId(1),
            chain_view: ChainViewId::initial(),
            permit: WorkPermit::ResolveThenVerify(VerifyCapability::Any),
            grant: ComputeGrant {
                max_resident_bytes: bytes,
                max_edges: 0,
            },
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
    apply_without_work(
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
        OwnedTx::PreAccepted(entry)
            if entry.source.payload_blame_peer() == Some(PeerIndex::from(17))
    ));
    assert_eq!(record.arrival.0, 0);
    assert_eq!(authority.chain_revision(), ChainRevision(0));
    assert_eq!(authority.chain_view(), &ChainViewId::initial());
    assert_eq!(authority.generation(), PoolGeneration(0));
    assert_eq!(authority.resources().remote().entries, 1);
    assert_eq!(authority.clocks().next_lease, ComputeLeaseId(1));
    let declared_dependencies = match owner {
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
            lease: ComputeLeaseId(2),
            chain_view: ChainViewId::initial(),
            permit: WorkPermit::ResolveOnly,
            grant: ComputeGrant {
                max_resident_bytes: 1,
                max_edges: 1,
            },
            attribution: ComputeAttribution::Peer(PeerIndex::from(17)),
            payload_policy: PayloadPolicy::RemoteDeclaredCycles(0),
            dependency_cut: DependencyCut(ApplySequence(1)),
            dependencies: declared_dependencies.clone(),
        }),
        PreAcceptedPhase::Computing(ActiveWork {
            lease: ComputeLeaseId(3),
            chain_view: ChainViewId::initial(),
            permit: WorkPermit::VerifyOnly(VerifyCapability::Any),
            grant: ComputeGrant {
                max_resident_bytes: 1,
                max_edges: 1,
            },
            attribution: ComputeAttribution::Peer(PeerIndex::from(17)),
            payload_policy: PayloadPolicy::RemoteDeclaredCycles(0),
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
            proposal: ProposalContextReceipt::from_validation(AcceptedStatus::Gap),
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
        Err(InputEvidenceError::NotAnInput)
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
        Some(&child_hash)
    );
    assert_eq!(
        authority
            .accepted_parents(&child_hash)
            .expect("accepted child has a graph row"),
        &HashSet::from([parent_hash.clone()])
    );
    assert_eq!(
        authority
            .accepted_children(&parent_hash)
            .expect("accepted parent has a graph row"),
        &HashSet::from([child_hash])
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
        &HashSet::from([left.clone(), right.clone()])
    );
    assert_eq!(
        authority
            .accepted_children(&left)
            .expect("left parent has one child row"),
        &HashSet::from([child.clone()])
    );
    assert_eq!(
        authority
            .accepted_children(&right)
            .expect("right parent has one child row"),
        &HashSet::from([child])
    );
    assert_resource_reference(&authority);
}

#[test]
fn uak_status_reconcile_updates_count_and_eviction_projection_once() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash =
        accept_remote_transaction(&mut authority, tx(70), 70, AcceptedStatus::Gap, Vec::new());
    let version = owner_version(&authority, &hash);
    let demotion = apply_committed_without_work(
        authority
            .plan_status_for_foundation(&hash, version, AcceptedStatus::Pending)
            .expect("Gap demotion is one membership transition"),
    );
    assert_eq!(demotion.retired_len(), 0);
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

    apply_without_work(
        authority
            .plan_status_for_foundation(&hash, version, AcceptedStatus::Proposed)
            .expect("Pending promotion is one membership transition"),
    );
    let counts = authority.membership_counts();
    assert_eq!((counts.pending, counts.gap, counts.proposed), (0, 0, 1));
    assert_resource_reference(&authority);
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
        let batch = independent_batch(&aggregate, &hashes);
        let SettlementPlan::IndependentRun(plan) = aggregate
            .plan_settlement(&batch)
            .expect("independent cohort classification is total")
        else {
            panic!("chain-backed disjoint cohort must remain independent");
        };
        let aggregate_committed = apply_committed_without_work(plan);
        let CommittedChanges::IndependentRun(committed) = &aggregate_committed.changes else {
            panic!("aggregate Apply preserves the independent committed order");
        };
        assert_eq!(committed.len(), count);
        assert_eq!(
            committed
                .iter()
                .map(|change| change.changed.clone())
                .collect::<Vec<_>>(),
            hashes.iter().rev().cloned().collect::<Vec<_>>()
        );
        assert!(aggregate_committed.removals.is_empty());
        assert_eq!(aggregate_committed.retired_len(), 0);

        let (mut reference, reference_hashes) = independent_fixture(count);
        assert_eq!(reference_hashes, hashes);
        for expected in committed {
            let version = owner_version(&reference, &expected.changed);
            let single = apply_committed_without_work(
                reference
                    .plan_accept_for_foundation(&expected.changed, version, AcceptedStatus::Pending)
                    .expect("canonical single reference accepts the same candidate"),
            );
            let actual = only_committed_change(&single);
            // Timing is post-commit observability, not membership semantics;
            // independently constructed equivalent executions need not share
            // the same monotonic start instant.
            assert_eq!(actual.sequence, expected.sequence);
            assert_eq!(actual.changed, expected.changed);
        }

        assert!(
            aggregate
                .normalized_snapshot()
                .equivalent_modulo_effect_batching(&reference.normalized_snapshot()),
            "commuting Apply must equal its canonical single sequence apart from journal batching"
        );
        assert_resource_reference(&aggregate);
        assert_membership_reference(&aggregate);
    }
}

#[test]
fn uak_popular_dependency_appends_sparse_reader_edges() {
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

    let snapshot = authority.membership_snapshot_for_reference();
    assert_eq!(
        snapshot.dependency_readers.get(&shared_dependency),
        Some(&expected_readers)
    );
    assert_membership_reference(&authority);
    assert_resource_reference(&authority);
}

#[test]
fn uak_resource_batch_is_a_commutative_set_transition() {
    let bound = usize::MAX / 8;
    let unbounded = ResourceVector::new(bound, bound, bound, 1);
    let mut ledger = ResourceLedger::new(
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
            (first.clone(), Some(first_before), Some(first_after)),
            (second.clone(), Some(second_before), None),
        ])
        .expect("net-neutral batch does not depend on caller order");
    ledger.apply_batch(plan);
    let snapshot = ledger.snapshot();
    assert_eq!(snapshot.preaccepted, ResourceVector::new(1, bound, 0, 0));
    assert_eq!(snapshot.charges.len(), 1);
    assert_eq!(snapshot.charges.get(&first), Some(&first_after));
    assert!(!snapshot.charges.contains_key(&second));
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
        let committed = apply_committed_without_work(plan);
        let CommittedChanges::IndependentRun(order) = committed.changes else {
            panic!("cohort commits with one canonical order");
        };
        let order = order
            .into_iter()
            .map(|change| change.changed)
            .collect::<Vec<_>>();
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
    apply_without_work(
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
        apply_without_work(
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
    let committed = apply_committed_without_work(plan);
    assert_eq!(committed.changed_owner_count(), 1);
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
    let committed = apply_committed_without_work(plan);
    assert_eq!(committed.changed_owner_count(), 1);
    assert!(matches!(
        authority.entry(&remote),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_independent_plan_drop_and_mid_batch_counter_failure_are_mutation_free() {
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
    assert_eq!(dropped.normalized_snapshot(), before);

    let (mut exhausted, hashes) = independent_fixture(2);
    exhausted.force_next_sequence(ApplySequence(u128::MAX - 1));
    let before = exhausted.normalized_snapshot();
    let batch = independent_batch(&exhausted, &hashes);
    assert_eq!(
        exhausted.plan_settlement(&batch).err(),
        Some(PlanError::Fault(AuthorityFault::CounterExhausted))
    );
    assert_eq!(exhausted.normalized_snapshot(), before);
    assert_resource_reference(&exhausted);
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
            vec![shared_input.clone()],
            Capacity::shannons(2_000),
        ),
    );
    let before = conflicts.normalized_snapshot();
    let batch = independent_batch(&conflicts, &[left, right]);
    let reason = coupled_reason_and_drop(
        conflicts
            .plan_settlement(&batch)
            .expect("classification itself is valid"),
    );
    assert!(matches!(
        reason,
        IndependentCoupling::CohortInputConflict(input) if input == shared_input
    ));
    assert_eq!(conflicts.normalized_snapshot(), before);

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
            vec![spent.clone()],
            vec![independent_input],
            Capacity::shannons(2_000),
        ),
    );
    let before = conditional.normalized_snapshot();
    let batch = independent_batch(&conditional, &[spender, reader]);
    let reason = coupled_reason_and_drop(
        conditional
            .plan_settlement(&batch)
            .expect("classification itself is valid"),
    );
    assert!(matches!(
        reason,
        IndependentCoupling::CohortConditionalEdge(edge) if edge == spent
    ));
    assert_eq!(conditional.normalized_snapshot(), before);
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

    let reason = coupled_reason_and_drop(
        authority
            .plan_settlement(&batch)
            .expect("capacity coupling is a normal classification"),
    );
    assert_eq!(reason, IndependentCoupling::AcceptedCapacity);
    assert_eq!(authority.normalized_snapshot(), before);
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
    assert_eq!(conflict.normalized_snapshot(), before);

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
            vec![conditional_cell.clone()],
            Capacity::shannons(2_000),
        ),
    );
    let before = conditional.normalized_snapshot();
    let batch = independent_batch(&conditional, &[spender]);
    let reason = coupled_reason_and_drop(
        conditional
            .plan_settlement(&batch)
            .expect("accepted conditional edge routes normally"),
    );
    assert!(matches!(
        reason,
        IndependentCoupling::AcceptedConditionalEdge(edge) if edge == conditional_cell
    ));
    assert_eq!(conditional.normalized_snapshot(), before);

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
    let reason = coupled_reason_and_drop(
        causal
            .plan_settlement(&batch)
            .expect("pool parent routes normally"),
    );
    assert!(matches!(
        reason,
        IndependentCoupling::PoolParent(hash) if hash == parent
    ));
    assert_eq!(causal.normalized_snapshot(), before);

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
    let reason = coupled_reason_and_drop(
        late.plan_settlement(&batch)
            .expect("accepted child routes normally"),
    );
    assert!(matches!(
        reason,
        IndependentCoupling::AcceptedChild(hash) if hash == late_child
    ));
    assert_eq!(late.normalized_snapshot(), before);
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
    assert_eq!(authority.normalized_snapshot(), before);

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
    assert_eq!(authority.normalized_snapshot(), before);
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
    assert_eq!(authority.normalized_snapshot(), before);
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
    apply_without_work(
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
    assert_eq!(authority.normalized_snapshot(), before);
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
    let SettlementPlan::CoupledComponent {
        reason,
        disposition,
    } = authority
        .plan_settlement(&batch)
        .expect("late parent has one bounded coupled Plan")
    else {
        panic!("late parent must not use IndependentRun");
    };
    assert_eq!(reason, IndependentCoupling::AcceptedChild(child.clone()));
    let _ = accepted_disposition(disposition).apply();
    assert_eq!(
        authority.accepted_children(&parent),
        Some(&HashSet::from([child.clone()]))
    );
    assert_eq!(
        authority.accepted_parents(&child),
        Some(&HashSet::from([parent.clone()]))
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
    let SettlementPlan::CoupledComponent {
        reason,
        disposition,
    } = authority
        .plan_settlement(&batch)
        .expect("late grandparent has one bounded coupled Plan")
    else {
        panic!("late grandparent must not use IndependentRun");
    };
    assert_eq!(reason, IndependentCoupling::AcceptedChild(parent.clone()));
    let _ = accepted_disposition(disposition).apply();
    assert_eq!(
        authority.accepted_children(&grandparent),
        Some(&HashSet::from([parent.clone()]))
    );
    assert_eq!(
        authority.accepted_parents(&parent),
        Some(&HashSet::from([grandparent]))
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
    let SettlementPlan::CoupledComponent {
        reason,
        disposition,
    } = authority
        .plan_settlement(&batch)
        .expect("shared descendant path has one bounded coupled Plan")
    else {
        panic!("accepted child must route through the coupled planner");
    };
    assert_eq!(reason, IndependentCoupling::PoolParent(ancestor.clone()));
    let _ = accepted_disposition(disposition).apply();

    assert_eq!(
        authority.accepted_parents(&child),
        Some(&HashSet::from([ancestor.clone(), parent.clone()]))
    );
    assert_eq!(
        authority.accepted_children(&ancestor),
        Some(&HashSet::from([child.clone(), parent.clone()]))
    );
    assert_eq!(
        authority.accepted_children(&parent),
        Some(&HashSet::from([child]))
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
    let SettlementPlan::CoupledComponent { disposition, .. } = authority
        .plan_settlement(&batch)
        .expect("late parent capacity is planned on the projected graph")
    else {
        panic!("accepted child must route through the coupled planner");
    };
    let committed = accepted_disposition(disposition).apply();

    assert_eq!(committed.removals.len(), 1);
    assert_eq!(committed.retired_len(), committed.removals.len());
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
    let SettlementPlan::CoupledComponent { disposition, .. } = authority
        .plan_settlement(&batch)
        .expect("late-child eviction is compiled before Apply")
    else {
        panic!("accepted child must route through the coupled planner");
    };
    let committed = accepted_disposition(disposition).apply();

    assert_eq!(committed.removals.len(), 1);
    assert_eq!(committed.retired_len(), committed.removals.len());
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
    assert_eq!(authority.accepted_children(&parent), Some(&HashSet::new()));
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
    assert_eq!(authority.normalized_snapshot(), before);
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
    assert_eq!(authority.normalized_snapshot(), before);
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
    assert_eq!(authority.normalized_snapshot(), before);
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
    let committed = apply_committed_without_work(
        authority
            .plan_accept_for_foundation(&candidate, version, AcceptedStatus::Pending)
            .expect("higher-fee candidate atomically evicts a closed component"),
    );
    assert_eq!(committed.removals.len(), 2);
    assert_eq!(committed.retired_len(), committed.removals.len());
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

    let checkout = authority
        .plan_effect_checkout_for_foundation()
        .expect("capacity outcome checkout plans")
        .expect("membership Apply commits one complete outcome batch")
        .apply();
    let lease = checkout
        .into_effect_lease()
        .expect("capacity outcome checkout returns one lease");
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
    let committed = apply_committed_without_work(plan);
    assert_eq!(committed.removals.len(), 2);
    assert_eq!(committed.retired_len(), committed.removals.len());
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
    assert_eq!(authority.accepted_spender(&chain_input), Some(&replacement));
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
    assert_resource_reference(&authority);

    let checkout = authority
        .plan_effect_checkout_for_foundation()
        .expect("replacement outcome checkout plans")
        .expect("replacement Apply commits one complete outcome batch")
        .apply();
    let lease = checkout
        .into_effect_lease()
        .expect("replacement outcome checkout returns one lease");
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
    let expected_victims = HashSet::from([victim.clone(), child.clone()]);
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
        apply_without_work(
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
            vec![released_input.clone(), retained_input.clone()],
            Capacity::shannons(10_000),
        ),
    );
    let middle_version = owner_version(&authority, &middle);
    apply_without_work(
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
    apply_without_work(
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
    while let Some(plan) = authority
        .plan_dependency_maintenance()
        .expect("continuous reservation makes wakeup capacity-invariant")
    {
        apply_without_work(plan);
    }
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
    apply_without_work(
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
        apply_without_work(unrelated);
    }
    while let Some(plan) = authority
        .plan_dependency_maintenance()
        .expect("unrelated maintenance remains bounded")
    {
        apply_without_work(plan);
    }
    assert!(matches!(
        authority.entry(&victim),
        Some(OwnedTx::ReplacementHistory(_))
    ));

    let released = authority
        .plan_dependency_availability_for_foundation(vec![DependencyKey::Cell(conflicting_input)])
        .expect("the conflicting input availability plans")
        .expect("the exact history trigger has an indexed waiter");
    apply_without_work(released);
    while let Some(plan) = authority
        .plan_dependency_maintenance()
        .expect("the exact history trigger remains coherent")
    {
        apply_without_work(plan);
    }
    assert!(matches!(
        authority.entry(&victim),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.source, PreAcceptedSource::Recovery(_))
                && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));
    assert_resource_reference(&authority);
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
    apply_without_work(
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
    apply_without_work(first_release);
    while let Some(plan) = authority
        .plan_dependency_maintenance()
        .expect("partial availability maintenance remains coherent")
    {
        apply_without_work(plan);
    }
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
    apply_without_work(second_release);
    while let Some(plan) = authority
        .plan_dependency_maintenance()
        .expect("complete availability maintenance remains coherent")
    {
        apply_without_work(plan);
    }
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
            vec![released_input.clone(), retained_input.clone()],
            Capacity::shannons(10_000),
        ),
    );
    let middle_version = owner_version(&authority, &middle);
    apply_without_work(
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
    apply_without_work(
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

    let mut rounds = 0usize;
    while let Some(plan) = authority
        .plan_dependency_maintenance()
        .expect("dependency maintenance remains coherent")
    {
        apply_without_work(plan);
        rounds += 1;
        assert!(
            rounds <= 3,
            "one key with two waiters has bounded maintenance"
        );
    }
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
    apply_without_work(
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
    apply_without_work(
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
    let committed = apply_committed_without_work(
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
    let committed = apply_committed_without_work(
        authority
            .plan_accept_for_foundation(&replacement, version, AcceptedStatus::Pending)
            .expect("one virtual component unions both direct-conflict trees"),
    );

    assert_eq!(committed.removals.len(), 5);
    assert_eq!(committed.retired_len(), committed.removals.len());
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
    assert!(committed.handoff_is_none());
    assert!(committed.removals.is_empty());
    assert!(matches!(
        authority.entry(&victim),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(authority.entry(&replacement).is_none());
    assert_eq!(authority.resources().replacement_history().entries, 0);
    assert_resource_reference(&authority);

    let checkout = authority
        .plan_effect_checkout_for_foundation()
        .expect("effect checkout plans")
        .expect("the terminal Apply committed its rejection")
        .apply();
    let lease = checkout
        .into_effect_lease()
        .expect("effect checkout returns the rejection lease");
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
    apply_without_work(
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
        Some(&replacement)
    );
    assert_eq!(
        authority.accepted_spender(&confirmed_input),
        Some(&replacement)
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
        apply_without_work(plan);
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
fn uak_resource_reference_rejects_ghost_overcharge() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 626, 74);
    let entries = authority
        .entries_for_reference()
        .iter()
        .map(|(hash, owner)| (hash.clone(), owner.clone()))
        .collect::<HashMap<_, _>>();
    let exact = entries
        .get(&hash)
        .expect("fixture owner exists")
        .charge_record();
    let ChargeRecord::PreAccepted {
        resources,
        residency_peer,
        compute_peer,
    } = exact
    else {
        panic!("fixture owner is preaccepted");
    };
    let inflated = ChargeRecord::PreAccepted {
        resources: ResourceVector::new(
            resources.entries,
            resources.bytes.checked_add(1).expect("fixture fits"),
            resources.edges,
            resources.active_work,
        ),
        residency_peer,
        compute_peer,
    };
    let mut ledger = ResourceLedger::new(limits());
    let plan = ledger
        .plan_replace(hash, None, Some(inflated))
        .expect("inflated fixture still fits the configured ceiling");
    ledger.apply(plan);

    assert!(
        !ledger.semantically_matches(&entries),
        "a normalized rebuild must reject both undercharge and ghost overcharge"
    );
}

#[test]
fn uak_compute_release_requires_the_exact_non_compute_charge() {
    let key = RawTxHash(Byte32::new([127; 32]));
    let peer = PeerIndex::from(127);
    let retained = ResourceVector::new(1, 128, 2, 0);
    let computing = retained
        .reserve_compute(ComputeGrant {
            max_resident_bytes: 256,
            max_edges: 4,
        })
        .expect("the fixture reserves one compute envelope");
    let before = ChargeRecord::PreAccepted {
        resources: computing,
        residency_peer: Some(peer),
        compute_peer: Some(peer),
    };
    let mut ledger = ResourceLedger::new(limits());
    let insertion = ledger
        .plan_replace(key.clone(), None, Some(before))
        .expect("the fixture charge fits every resource partition");
    ledger.apply(insertion);

    let shrunk = ChargeRecord::PreAccepted {
        resources: ResourceVector::new(1, 127, 2, 0),
        residency_peer: Some(peer),
        compute_peer: None,
    };
    assert_eq!(
        ledger.plan_compute_release(key, before, shrunk).err(),
        Some(ComputeReleaseError::Projection),
        "compute cancellation cannot disguise retained-charge drift as release"
    );
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
    apply_without_work(
        authority
            .apply_settlement(exhausted.into_settlement())
            .expect("returned capability commits the original rejection"),
    );
    assert_eq!(authority.resources().preaccepted().active_work, 0);
    assert!(authority.entry(&hash).is_none());
    let checkout = authority
        .plan_effect_checkout_for_foundation()
        .expect("effect checkout plans")
        .expect("terminalization and rejection publish in the same Apply")
        .apply();
    let lease = checkout
        .into_effect_lease()
        .expect("rejection effect owns one publication lease");
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
    apply_without_work(
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
    assert_eq!(authority.normalized_snapshot(), before);
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
    let CheckedOutWork::Resolve(resolve) = checkout.into_work().expect("resolve work exists")
    else {
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

    apply_without_work(
        authority
            .apply_settlement(resolve.rejected(RejectionKind::Policy))
            .expect("live lease still settles after peer backpressure"),
    );
}

#[test]
fn uak_stale_lease_is_mutation_free_across_aba() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(27);
    let first = ValidatedAdmission::remote(transaction.clone(), PeerIndex::from(46))
        .expect("fixture admission is valid");
    let hash = first.identity.raw.clone();
    apply_without_work(
        authority
            .plan_admission(first)
            .expect("first incarnation plans"),
    );
    let version = owner_version(&authority, &hash);
    let checkout = authority
        .plan_checkout_for_foundation(&hash, version, WorkPermit::ResolveOnly)
        .expect("first incarnation checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.into_work().expect("resolve work exists")
    else {
        panic!("resolve-only permit returns resolve work");
    };

    let settlement = resolve.rejected(RejectionKind::Policy);
    let stale_token = SettlementToken {
        hash: settlement.token.hash.clone(),
        version: settlement.token.version,
        lease: settlement.token.lease,
    };
    apply_without_work(
        authority
            .plan_terminalize_for_foundation(&hash, owner_version(&authority, &hash))
            .expect("active terminalization invalidates the exact owner"),
    );
    assert!(authority.entry(&hash).is_none());
    apply_without_work(
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
        .and_then(OwnedTx::preaccepted_charge)
        .expect("queued owner has an exact retained charge");
    let version = owner_version(&authority, &hash);
    let checkout = authority
        .plan_checkout_for_foundation(
            &hash,
            version,
            WorkPermit::ResolveThenVerify(VerifyCapability::Any),
        )
        .expect("queued resolve accepts a continuous permit")
        .apply();
    assert_eq!(
        only_committed_change(checkout.committed_delta_for_foundation()).sequence,
        ApplySequence(2)
    );
    let (compute_bytes, compute_edges) = authority
        .resources()
        .compute_limits()
        .reservation_for(WorkPermit::ResolveThenVerify(VerifyCapability::Any));
    let expected_charge = queued_charge
        .reserve_compute(ComputeGrant {
            max_resident_bytes: compute_bytes,
            max_edges: compute_edges,
        })
        .expect("fixture charge accepts exactly one compute reservation");
    assert_eq!(authority.resources().preaccepted(), expected_charge);
    assert!(authority.primary_projection_consistent());
    let before_local_continuation = authority.normalized_snapshot();
    let CheckedOutWork::ContinuousResolve(resolve) = checkout
        .into_work()
        .expect("checkout returns one work capability")
    else {
        panic!("continuous permit returns continuous resolve work");
    };
    let payload = resolved_payload(resolve.transaction());
    let (verify, accepted_resident_bytes) = continue_fixture_verify(resolve, payload);
    assert_eq!(authority.normalized_snapshot(), before_local_continuation);
    let settlement = verify.verified(0);
    apply_without_work(
        authority
            .apply_settlement(settlement)
            .expect("current continuous lease settles"),
    );
    let retained = authority
        .entry(&hash)
        .and_then(OwnedTx::preaccepted_charge)
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
    apply_without_work(accepted);
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
fn uak_allocation_failure_discards_result_without_retaining_compute_capability() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote_until(&mut authority, 1_733, 733, 10);
    let original_charge = authority
        .entry(&hash)
        .and_then(OwnedTx::preaccepted_charge)
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

    let committed = authority
        .apply_compute_cancellation(failure.discard_result_for_cancellation())
        .expect("the narrow cancellation plan cannot acquire allocator backpressure");
    assert_eq!(committed.retired_len(), 0);
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
                && entry.charge == original_charge
    ));
    assert_resource_reference(&authority);
    assert!(authority.primary_projection_consistent());

    let expired = authority
        .plan_remote_expiry(
            RemoteDeadline(10),
            NonZeroUsize::new(1).expect("the fixture limit is non-zero"),
        )
        .expect("the immutable deadline remains valid across compute cancellation")
        .expect("the cancelled owner remains due")
        .apply();
    assert_eq!(expired.changed_owner_count(), 1);
    assert!(authority.entry(&hash).is_none());
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
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.into_work().expect("work exists")
    else {
        panic!("continuous permit returns continuous resolve work");
    };
    assert_eq!(
        resolve.resolution_grant(),
        ComputeGrant {
            max_resident_bytes: 4 * 1024,
            max_edges: 16,
        }
    );
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
    apply_without_work(
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
    apply_without_work(
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
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.into_work().expect("work exists")
    else {
        panic!("continuous permit returns continuous resolve work");
    };
    let payload = resolved_payload(resolve.transaction());
    let (verify, _) = continue_fixture_verify(resolve, payload);
    let denied = verify.verified(0);
    apply_without_work(
        verify_authority
            .apply_settlement(denied)
            .expect("verified budget denial releases the active grant"),
    );
    assert!(verify_authority.entry(&verify_hash).is_none());
    assert_resource_reference(&verify_authority);
}

#[test]
fn uak_invalid_resolution_receipt_retains_the_only_lease_settlement() {
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
    apply_without_work(
        authority
            .apply_settlement(failure.into_settlement())
            .expect("invalid resolve receipt settles its exact lease"),
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
    apply_without_work(
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
    let CheckedOutWork::Verify(verify) = committed.into_work().expect("verify work exists") else {
        panic!("verify permit returns verify work");
    };
    let expected_resident_bytes = accepted_transaction_charge_bytes(
        transaction.data().serialized_size_in_block(),
        verify.resolved_transaction(),
    );
    let settlement = verify.verified(0);
    apply_without_work(
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
    apply_without_work(
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
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.into_work().expect("work exists")
    else {
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
    apply_without_work(
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
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.into_work().expect("work exists")
    else {
        panic!("continuous permit returns continuous resolve work");
    };
    let before = authority.normalized_snapshot();
    let payload = resolved_payload(resolve.transaction());
    let (verify, _) = continue_fixture_verify(resolve, payload);
    assert_eq!(authority.normalized_snapshot(), before);
    apply_without_work(
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
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.into_work().expect("work exists")
    else {
        panic!("continuous permit returns continuous resolve work");
    };
    let payload = resolved_payload(resolve.transaction());
    let (verify, _) = continue_fixture_verify(resolve, payload);
    apply_without_work(
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
    let CheckedOutWork::Resolve(resolve) = checkout.into_work().expect("work exists") else {
        panic!("resolve-only permit returns resolve work");
    };
    apply_without_work(
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
    let CheckedOutWork::Resolve(resolve) = checkout.into_work().expect("work exists") else {
        panic!("resolve-only permit returns resolve work");
    };
    apply_without_work(
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
    let CheckedOutWork::Resolve(resolve) = checkout.into_work().expect("resolve work exists")
    else {
        panic!("resolve-only permit returns resolve work");
    };
    assert_eq!(resolve.transaction().hash(), hash.0);
    apply_without_work(
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
    let CheckedOutWork::Resolve(resolve) = checkout.into_work().expect("resolve work exists")
    else {
        panic!("resolve-only permit returns resolve work");
    };
    let resident_bytes = resolve.transaction().data().total_size();
    assert_eq!(resolve.resolution_grant().max_edges, 16);
    let evidence = resolution_evidence(
        resolve.transaction(),
        Capacity::shannons(1),
        resident_bytes,
        VerifyCycleClass::Small,
    );
    apply_without_work(
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
    let CheckedOutWork::Verify(verify) = verify_checkout.into_work().expect("verify work exists")
    else {
        panic!("verify-only permit returns verify work");
    };
    apply_without_work(
        authority
            .apply_settlement(verify.rejected(RejectionKind::Verification))
            .expect("verification rejection settles"),
    );
    assert!(authority.entry(&hash).is_none());
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_stale_lease_is_mutation_free_after_chain_view_change() {
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
    let CheckedOutWork::ContinuousResolve(second) =
        second_checkout.into_work().expect("second work exists")
    else {
        panic!("continuous permit returns continuous resolve work");
    };
    let payload = resolved_payload(second.transaction());
    let (verify, _) = continue_fixture_verify(second, payload);
    let settlement = verify.verified(0);
    authority.force_chain_view(ChainViewId::new(ChainRevision(1), Byte32::new([16; 32])));
    let forged = ComputeSettlement {
        token: SettlementToken {
            hash: settlement.token.hash.clone(),
            version: settlement.token.version,
            lease: ComputeLeaseId(u128::MAX),
        },
        next: SettlementNext::Retry,
    };
    let before_forged = authority.normalized_snapshot();
    let stale_lease = authority
        .apply_settlement(forged)
        .expect_err("a forged compute lease is stale");
    assert_eq!(
        stale_lease.recovery(),
        &ComputeSettlementRecovery::Obsolete(StalePlan::Lease)
    );
    assert_eq!(authority.normalized_snapshot(), before_forged);
    apply_without_work(
        authority
            .apply_settlement(settlement)
            .expect("the genuine lease remains available after the forged token"),
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
    let CheckedOutWork::Resolve(resolve) =
        resolve_checkout.into_work().expect("resolve work exists")
    else {
        panic!("resolve-only permit returns resolve work");
    };
    apply_without_work(
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
    let CheckedOutWork::Resolve(resolve) = checkout.into_work().expect("resolve work exists")
    else {
        panic!("resolve-only permit returns resolve work");
    };
    apply_without_work(
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
    let CheckedOutWork::ContinuousResolve(continuous) = continuous_checkout
        .into_work()
        .expect("continuous work exists")
    else {
        panic!("continuous permit returns continuous resolve work");
    };
    apply_without_work(
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
    let CheckedOutWork::Resolve(resolve) = first.into_work().expect("resolve work exists") else {
        panic!("resolve-only permit returns resolve work");
    };
    let payload = resolved_payload(resolve.transaction());
    apply_without_work(
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
    let CheckedOutWork::Verify(verify) = second.into_work().expect("verify work exists") else {
        panic!("verify-only permit returns verify work");
    };
    apply_without_work(
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
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.into_work().expect("work exists")
    else {
        panic!("continuous permit returns continuous resolve work");
    };
    apply_without_work(
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
    let CheckedOutWork::Resolve(resolve) = checkout.into_work().expect("work exists") else {
        panic!("resolve-only permit returns resolve work");
    };
    let payload = resolved_payload(resolve.transaction());
    apply_without_work(
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
    let CheckedOutWork::Verify(verify) = checkout.into_work().expect("work exists") else {
        panic!("verify-only permit returns verify work");
    };
    apply_without_work(
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
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.into_work().expect("work exists")
    else {
        panic!("continuous permit returns continuous resolve work");
    };
    let payload = resolved_payload(resolve.transaction());
    let (verify, _) = continue_fixture_verify(resolve, payload);
    apply_without_work(
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
    assert_eq!(authority.normalized_snapshot(), before);

    let committed = authority
        .plan_checkout_next(WorkPermit::ResolveOnly)
        .expect("frontier selection is valid")
        .expect("dropped plan did not consume the queue slot")
        .apply();
    let (selected, work) = take_resolve_work(committed);
    assert_eq!(selected, hash);
    assert!(authority.primary_projection_consistent());

    apply_without_work(
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
    apply_without_work(
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
        apply_without_work(
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
    apply_without_work(
        authority
            .plan_admission(proposal_admission)
            .expect("proposal admission plans"),
    );
    let recovery_admission = ValidatedAdmission::recovery(tx(612), PoolGeneration(0))
        .expect("fixture recovery admission is valid");
    let recovery = recovery_admission.identity.raw.clone();
    apply_without_work(
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
        apply_without_work(
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
    apply_without_work(
        authority
            .apply_settlement(peer_a_work.rejected(RejectionKind::Policy))
            .expect("peer A work settles"),
    );

    let trusted_admission =
        ValidatedAdmission::proposal(tx(620)).expect("fixture proposal admission is valid");
    let trusted = trusted_admission.identity.raw.clone();
    apply_without_work(
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
    apply_without_work(
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
    apply_without_work(
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
        let CheckedOutWork::Verify(work) = committed.into_work().expect("verify work exists")
        else {
            panic!("verify permit returns verify work");
        };
        assert_eq!(
            &TxIdentity::from_transaction(work.transaction()).raw,
            expected
        );
        apply_without_work(
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
        apply_without_work(
            authority
                .apply_settlement(work.rejected(RejectionKind::Policy))
                .expect("active lease settles"),
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
        apply_without_work(
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
        apply_without_work(
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
        apply_without_work(
            authority
                .plan_admission(admission)
                .expect("fixture admission fits"),
        );
    }
    apply_without_work(
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
        apply_without_work(
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
    let final_peer = admit_remote(&mut authority, 900, 700 + BLOCKED_PEERS);

    let mut active_work = Vec::new();
    for hash in active_hashes {
        let version = owner_version(&authority, &hash);
        let (_, work) = take_resolve_work(
            authority
                .plan_checkout_for_foundation(&hash, version, WorkPermit::ResolveOnly)
                .expect("manual checkout consumes the peer's one active slot")
                .apply(),
        );
        active_work.push(work);
    }

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
        apply_without_work(
            authority
                .apply_settlement(work.internal_failure())
                .expect("every active lease settles exactly once"),
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
    apply_without_work(
        authority
            .plan_admission(parent_admission)
            .expect("fixture parent enters PreAccepted ownership"),
    );
    let stale_input = OutPoint::new(parent_tx.hash(), 0);
    let stale_tx = TransactionBuilder::default()
        .version(901u32)
        .input(CellInput::new(stale_input.clone(), 0))
        .build();
    let fresh_input = OutPoint::new(Byte32::new([0xd2; 32]), 0);
    let fresh_tx = TransactionBuilder::default()
        .version(902u32)
        .input(CellInput::new(fresh_input, 0))
        .build();
    let stale =
        queue_remote_for_verify(&mut authority, stale_tx.clone(), 901, Capacity::shannons(1));
    let fresh = queue_remote_for_verify(&mut authority, fresh_tx, 902, Capacity::shannons(1));
    apply_without_work(
        authority
            .plan_admission(
                ValidatedAdmission::proposal(stale_tx)
                    .expect("the queued remote owner can gain trusted proposal priority"),
            )
            .expect("promotion moves the same queue slot to the trusted owner"),
    );

    apply_without_work(
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
    let CheckedOutWork::Verify(work) = committed.into_work().expect("verify work exists") else {
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
fn uak_retained_growth_denial_atomically_releases_the_compute_lease() {
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
    apply_without_work(
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
    apply_without_work(
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
        apply_without_work(
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
    apply_without_work(
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
    let committed = plan.apply();
    assert!(authority.ready_for_reference().is_empty());
    assert!(authority.primary_projection_consistent());
    assert!(matches!(
        committed.changes,
        CommittedChanges::IndependentRun(changes)
            if changes.iter().map(|change| &change.changed).eq([
                &hashes[2],
                &hashes[1],
                &hashes[0],
            ])
    ));
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
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.into_work().expect("work exists")
    else {
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
    apply_without_work(
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
    let CheckedOutWork::Verify(verify) = checkout.into_work().expect("verify work exists") else {
        panic!("verify-only permit returns verify work");
    };
    apply_without_work(
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
    apply_without_work(
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
    apply_without_work(
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
        let CheckedOutWork::Verify(work) = committed.into_work().expect("verify work exists")
        else {
            panic!("verify-only permit returns verify work");
        };
        (TxIdentity::from_transaction(work.transaction()).raw, work)
    };
    assert_eq!(selected, small_hash);
    apply_without_work(
        authority
            .apply_settlement(small_verify.rejected(RejectionKind::Verification))
            .expect("small lease settles"),
    );

    let committed = authority
        .plan_checkout_next(WorkPermit::VerifyOnly(VerifyCapability::Any))
        .expect("general frontier lookup is valid")
        .expect("large work remains")
        .apply();
    let CheckedOutWork::Verify(large_verify) = committed.into_work().expect("verify work exists")
    else {
        panic!("verify-only permit returns verify work");
    };
    assert_eq!(
        TxIdentity::from_transaction(large_verify.transaction()).raw,
        large_hash
    );
    apply_without_work(
        authority
            .apply_settlement(large_verify.rejected(RejectionKind::Verification))
            .expect("large lease settles"),
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_runner_cancellation_settles_one_exact_lease_before_exit() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let hash = admit_remote(&mut authority, 612, 67);
    let checkout = authority
        .plan_checkout_next(WorkPermit::ResolveThenVerify(VerifyCapability::Any))
        .expect("frontier lookup is valid")
        .expect("work is available")
        .apply();
    assert_eq!(authority.resources().preaccepted().active_work, 1);
    let cancellation = checkout
        .into_work()
        .expect("checked-out capability exists")
        .cancelled();
    apply_without_work(
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
