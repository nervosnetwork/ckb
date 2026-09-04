use super::super::effect::{CommittedAcceptance, CommittedEffect};
use super::super::plan::{
    AdministrativeRemovalControl, CommittedDelta, ConcurrentRetainedIngressError, PreparedApply,
    PreparedIndependentApply, PreparedSharedDirectAdmissionDisposition, PreparedSharedOwnerRemoval,
    SettlementBatch, SharedDirectAdmissionCommitOutcome, TxPoolAuthority,
    test_support::CommittedCheckout,
};
use super::super::resources::{
    AcceptedCost, AcceptedResources, ChargeRecord, ComputeLimits, ResourceLimits, ResourceVector,
    test_support::ResourceSnapshot,
};
use super::super::runtime::AuthorityRuntime;
use super::super::shard::ConcurrentRemovalProbe;
use super::super::state::{
    AcceptedAtMillis, AcceptedStatus, ApplySequence, CandidateMetrics, ChainRevision, ChainViewId,
    DependencyCut, DependencyKey, EntryVersion, OwnedTx, RawTxHash, RemoteDeadline,
    RemoteResidencyLease, ResolvedPayload, TxIdentity, ValidatedAdmission, VerifiedFacts,
    VerifyCapability, WorkPermit, test_support::FoundationResolution,
};
use super::super::work::{
    CheckedOutWork, ContinuousResolution, ContinuousResolveWork, ContinuousVerifyWork, ResolveWork,
};
use crate::component::entry::accepted_transaction_charge_bytes;
use ckb_app_config::{TxPoolConfig, VerifyOrdering};
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_network::PeerIndex;
use ckb_snapshot::Snapshot;
use ckb_test_chain_utils::MockStore;
use ckb_types::{
    U256,
    bytes::Bytes,
    core::{Capacity, Cycle, FeeRate, TransactionBuilder, TransactionView},
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint},
    prelude::{Builder, Entity, Pack},
};
use ckb_verification::cache::ScriptVerificationRules;
use std::collections::{HashMap, HashSet};
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

pub(super) fn tx(nonce: u64) -> ckb_types::core::TransactionView {
    TransactionBuilder::default().version(nonce as u32).build()
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

impl FixtureCommit for PreparedSharedOwnerRemoval<'_, AdministrativeRemovalControl> {
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
            .apply_effect_settlement_for_foundation(lease.complete_for_foundation())
            .expect("fixture effect publication settles");
        drop(committed);
    }
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
        .checkout_for_foundation(
            &hash,
            version,
            WorkPermit::ResolveThenVerify(VerifyCapability::Any),
        )
        .expect("fixture checkout plans");
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

pub(in crate::authority) fn leaf_rbf_pair(
    authority: &mut TxPoolAuthority,
    marker: u8,
) -> (RawTxHash, RawTxHash) {
    let input = OutPoint::new(Byte32::new([marker; 32]), 0);
    let victim_tx = TransactionBuilder::default()
        .version(10_000 + u32::from(marker))
        .input(CellInput::new(input.clone(), 0))
        .build();
    let victim = accept_remote_transaction_with_payload(
        authority,
        victim_tx.clone(),
        10_000,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &victim_tx,
            Vec::new(),
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
        20_000,
        resolved_payload_with_facts(
            &candidate_tx,
            Vec::new(),
            vec![input],
            Capacity::shannons(30_000),
        ),
    );
    (victim, candidate)
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
fn uak_disjoint_accepted_local_removals_overlap_inside_the_real_runtime_cut() {
    const CUT_ENTRY_TIMEOUT: Duration = Duration::from_secs(5);
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the authority runtime fixture is valid");
    let (first, first_relation, second, second_relation, second_support) =
        runtime.with_authority_for_foundation(|authority| {
        let accept_independent = |authority: &mut TxPoolAuthority, seed: u8| {
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
                    vec![input.clone()],
                    Capacity::shannons(1_000),
                ),
            );
            (hash, DependencyKey::Cell(input))
        };
        let (first, first_relation) = accept_independent(authority, 200);
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
        let mut candidates = vec![(first, first_relation)];
        for seed in 201u8..=255 {
            let (candidate, relation) = accept_independent(authority, seed);
            candidates.push((candidate, relation));
        }
        let mut candidates_with_support = Vec::with_capacity(candidates.len());
        for (candidate, relation) in candidates {
            let support = match authority
                .shared_local_removal_support_for_foundation(&candidate)
                .expect("the frozen independent Accepted removal plans")
            {
                Some(support) => support,
                None => panic!("the frozen independent Accepted owner remains present"),
            };
            assert_ne!(
                support
                    .relation_apply()
                    .writes()
                    .mask_for_foundation(),
                0,
                "every candidate removal changes its real dependency relation"
            );
            candidates_with_support.push((candidate, relation, support));
        }
        (0..candidates_with_support.len())
            .find_map(|left| {
                ((left + 1)..candidates_with_support.len()).find_map(|right| {
                    let (left_hash, left_relation, left_support) =
                        &candidates_with_support[left];
                    let (right_hash, right_relation, right_support) =
                        &candidates_with_support[right];
                    (left_relation != right_relation
                        && left_support
                            .owner_writes()
                            .is_disjoint(right_support.owner_writes())
                        && left_support
                            .owner_apply()
                            .is_compatible(right_support.owner_apply())
                        && left_support
                            .relation_apply()
                            .is_compatible(right_support.relation_apply())
                        && left_support
                            .dependency_gates()
                            .is_compatible(right_support.dependency_gates())
                        && left_support
                            .relation_apply()
                            .writes()
                            .is_disjoint(right_support.relation_apply().writes())
                        && left_support
                            .relation_apply()
                            .writes()
                            .mask_for_foundation()
                            != 0
                        && right_support
                            .relation_apply()
                            .writes()
                            .mask_for_foundation()
                            != 0)
                        .then(|| {
                            (
                                left_hash.clone(),
                                left_relation.clone(),
                                right_hash.clone(),
                                right_relation.clone(),
                                *right_support,
                            )
                        })
                })
            })
            .expect(
                "the fixed layout admits two relation-changing removals with compatible owner, Apply, and dependency-gate support",
            )
    });
    assert_ne!(
        first_relation, second_relation,
        "the selected removals own different dependency relations"
    );
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
        assert!(
            first_entered.is_ok(),
            "the first relation-changing removal reaches its final owner cut"
        );
        assert!(
            shared_entries
                .try_dependency_gate_cut(second_support.dependency_gates())
                .is_some(),
            "the first live removal unexpectedly holds a conflicting dependency gate"
        );
        assert!(
            shared_entries
                .try_write_cut(second_support.owner_apply().writes())
                .is_some(),
            "the first live cut unexpectedly overlaps the frozen second support"
        );
        let (second_started_tx, second_started_rx) = std::sync::mpsc::channel();
        let runtime_ref = &runtime;
        let second_hash = second.0.clone();
        let second_remove = scope.spawn(move || {
            let _ = second_started_tx.send(());
            runtime_ref.remove_local_transaction(&second_hash)
        });
        assert!(
            second_started_rx.recv_timeout(CUT_ENTRY_TIMEOUT).is_ok(),
            "the second removal worker starts before the first cut is released"
        );
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
        assert!(
            second_entered.is_ok(),
            "the compatible relation-changing removal must reach its final owner cut while the first cut is live"
        );
        assert!(
            second_remove
                .join()
                .expect("second removal thread joins")
                .unwrap()
        );
    });
    runtime.with_authority_for_foundation(|authority| {
        authority
            .entries_for_reference()
            .set_shared_owner_commit_probe(None);
        assert!(authority.primary_projection_consistent());
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
    let prepared = compiled
        .bind(&authority)
        .expect("Bind owns projection preparation, not final owner freshness");
    let failure = match prepared.apply() {
        Ok(_) => panic!("the obsolete owner incarnation cannot be removed"),
        Err(failure) => failure,
    };
    let (error, _effect_wake) = failure.into_parts();
    assert!(matches!(error, ConcurrentRetainedIngressError::Stale));
    assert_eq!(authority.normalized_snapshot(), before);
    assert_eq!(authority.effect_observation_for_foundation(), before_effect);
    assert!(matches!(authority.entry(&hash), Some(OwnedTx::Accepted(_))));
    assert_resource_reference(&authority);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_disjoint_direct_commit_does_not_wait_for_prior_effect_activation() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let shared_dependency = OutPoint::new(Byte32::new([203; 32]), 0);
    let compile = |version: u64| {
        let transaction = Arc::new(
            TransactionBuilder::default()
                .version(version as u32)
                .cell_dep(
                    CellDep::new_builder()
                        .out_point(shared_dependency.clone())
                        .build(),
                )
                .build(),
        );
        let hash = RawTxHash(transaction.hash());
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
        (hash, compiled)
    };

    let (activation_probe, left_activating, release_left_activation) =
        ConcurrentRemovalProbe::new();
    authority.set_next_effect_activation_probe_for_foundation(Some(activation_probe));
    let (left_hash, left) = compile(303);
    let mut right = None;
    for version in 304..320 {
        let candidate = compile(version);
        if left.is_compatible_with_for_foundation(&authority, &candidate.1) {
            right = Some(candidate);
            break;
        }
        drop(candidate);
    }
    let (right_hash, right) =
        right.expect("the bounded fixture finds one physically disjoint Direct peer");
    assert!(
        left.dependency_primary_insertion_shape_for_foundation()
            && right.dependency_primary_insertion_shape_for_foundation(),
        "both absent Direct candidates must use the exact primary-insertion dependency shape"
    );
    let left_support = left
        .physical_apply_support_for_foundation()
        .expect("the accepted Direct candidate has exact shard support");
    let right_support = right
        .physical_apply_support_for_foundation()
        .expect("the accepted Direct peer has exact shard support");
    let left_dependency_writes = left
        .dependency_write_support_for_foundation(&authority)
        .expect("the accepted Direct candidate writes dependency relations");
    let right_dependency_writes = right
        .dependency_write_support_for_foundation(&authority)
        .expect("the accepted Direct peer writes dependency relations");
    assert_ne!(
        left_support.reads().mask_for_foundation() & right_support.reads().mask_for_foundation(),
        0,
        "both Direct candidates read the same immutable cell_dep shard"
    );
    assert!(
        left_support
            .reads()
            .is_disjoint_from_writes(right_support.writes())
            && right_support
                .reads()
                .is_disjoint_from_writes(left_support.writes()),
        "the shared cell_dep is read/read support and cannot serialize either final cut"
    );
    assert!(
        left_dependency_writes.is_disjoint(right_support.writes())
            && right_dependency_writes.is_disjoint(left_support.writes())
            && right_support
                .reads()
                .is_disjoint_from_writes(left_dependency_writes)
            && left_support
                .reads()
                .is_disjoint_from_writes(right_dependency_writes),
        "dependency writes must commute with the peer final cut: left_dependency_writes={:#018x}, right_dependency_writes={:#018x}, left_reads={:#018x}, right_reads={:#018x}, left_writes={:#018x}, right_writes={:#018x}",
        left_dependency_writes.mask_for_foundation(),
        right_dependency_writes.mask_for_foundation(),
        left_support.reads().mask_for_foundation(),
        right_support.reads().mask_for_foundation(),
        left_support.writes().mask_for_foundation(),
        right_support.writes().mask_for_foundation(),
    );

    let (left_terminal_tx, left_terminal_rx) = std::sync::mpsc::channel();
    let (right_terminal_tx, right_terminal_rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let (owner_probe, right_owner_committed, release_right_owner) =
            ConcurrentRemovalProbe::new();
        authority
            .entries_for_reference()
            .set_shared_owner_commit_probe(Some(owner_probe));
        let right_handle = scope.spawn(move || {
            let _ = right_terminal_tx.send(right.commit());
        });
        let right_owner_entered = right_owner_committed.recv_timeout(Duration::from_secs(2));
        authority
            .entries_for_reference()
            .set_shared_owner_commit_probe(None);

        let left_handle = scope.spawn(move || {
            let _ = left_terminal_tx.send(left.commit());
        });
        let left_activation_entered = left_activating.recv_timeout(Duration::from_secs(2));
        release_right_owner
            .send(())
            .expect("always release B's diagnostic owner-cut pause");

        let right_before_left_release = left_activation_entered
            .as_ref()
            .ok()
            .and_then(|_| right_terminal_rx.recv_timeout(Duration::from_secs(2)).ok());
        let later_effect_hidden = right_before_left_release.as_ref().is_some_and(|_| {
            authority
                .effect_publication_receipt_for_foundation()
                .is_none()
        });
        release_left_activation
            .send(())
            .expect("always release A's diagnostic activation pause");
        let left_result = left_terminal_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("A returns after every diagnostic pause is released");
        let right_completed_before_left_release = matches!(
            &right_before_left_release,
            Some(SharedDirectAdmissionCommitOutcome::Accepted(_))
        );
        let right_result = match right_before_left_release {
            Some(result) => result,
            None => right_terminal_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("B returns after every diagnostic pause is released"),
        };
        left_handle
            .join()
            .expect("the left Direct worker does not panic");
        right_handle
            .join()
            .expect("the right Direct worker does not panic");

        assert!(matches!(
            left_result,
            SharedDirectAdmissionCommitOutcome::Accepted(_)
        ));
        assert!(matches!(
            right_result,
            SharedDirectAdmissionCommitOutcome::Accepted(_)
        ));
        assert!(
            right_owner_entered.is_ok(),
            "B must reach its real owner cut before A starts"
        );
        assert!(
            left_activation_entered.is_ok(),
            "A's final cut must share the immutable cell_dep read without waiting for B's disjoint owner cut"
        );
        assert!(
            right_completed_before_left_release,
            "B's complete effectful commit must not wait for A's effect activation after B already crossed pre-row effect observation"
        );
        assert!(
            later_effect_hidden,
            "B may commit first but cannot publish across A's earlier pending sequence"
        );
    });

    for expected in [&left_hash, &right_hash] {
        let lease = authority
            .effect_publication_receipt_for_foundation()
            .expect("each committed Direct admission publishes one ordered effect batch");
        let [CommittedEffect::Accepted(CommittedAcceptance::Admission { entry, .. })] =
            lease.effects()
        else {
            panic!("the ordered Direct effect is the exact admission")
        };
        assert_eq!(&entry.tx.hash(), &expected.0);
        drop(
            authority
                .apply_effect_settlement_for_foundation(lease.complete_for_foundation())
                .expect("the ordered Direct effect settles"),
        );
    }
    assert!(authority.primary_projection_consistent());
}
