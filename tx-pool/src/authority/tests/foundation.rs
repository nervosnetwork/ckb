use super::super::chain::{AcceptedProof, ProposalContextReceipt};
use super::super::effect::{CommittedEffect, EffectPolicy};
use super::super::plan::{
    AuthorityFault, Backpressure, CandidateBatchError, CommittedChange, CommittedChanges,
    CommittedDelta, DescendantAggregate, EvictionOrderKey, IndependentCoupling, MembershipReject,
    MembershipSnapshot, PlanError, PreparedApply, RemovalCause, SettlementBatch, SettlementPlan,
    StalePlan, StatusCounts, TxPoolAuthority,
};
use super::super::resources::{
    AcceptedCost, AcceptedResources, ChargeRecord, ComputeLimits, ResourceConfigError,
    ResourceLedger, ResourceLimits, ResourceSnapshot, ResourceVector,
};
use super::super::scheduler::VerifyOrder;
use super::super::state::{
    AcceptedEntry, AcceptedStatus, ActiveWork, AdmissionClass, ApplySequence, CandidateMetrics,
    ChainRevision, ChainViewId, ComputeAttribution, ComputeGrant, ComputeLeaseId, ComputedOutcome,
    DependencyCut, DependencyKey, EntryVersion, ExpandedFootprint, FootprintError,
    FoundationResolution, IngressAttribution, InputEvidenceError, KnownDependencies,
    ObservedDependencies, OwnedTx, PayloadBlame, PoolGeneration, PreAcceptedPhase,
    ProposalContextId, ProposalLease, QueuedWork, RawTxHash, RejectionKind, ResolvedPayload,
    TxIdentity, ValidatedAdmission, VerifiedFacts, VerifyCapability, VerifyCycleClass,
    WaitCondition, WorkPermit,
};
use super::super::work::{
    CheckedOutWork, ComputeSettlement, ContinuousResolution, ContinuousResolveWork,
    ContinuousVerifyWork, ResolutionEvidence, ResolutionReceiptError, ResolveWork, SettlementNext,
    SettlementToken, VerificationReceiptError,
};
use ckb_network::PeerIndex;
use ckb_types::{
    bytes::Bytes,
    core::{
        Capacity, FeeRate, TransactionBuilder, TransactionView, tx_pool::get_transaction_weight,
    },
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint},
    prelude::{Builder, Entity, Pack},
};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;

pub(super) fn limits() -> ResourceLimits {
    ResourceLimits::new(
        ResourceVector::new(8, 64 * 1024, 64, 8),
        ResourceVector::new(4, 32 * 1024, 32, 4),
        ResourceVector::new(2, 16 * 1024, 16, 2),
        AcceptedResources::new(8, 64 * 1024, 64 * 1024, 64),
        ComputeLimits::new(4 * 1024, 4 * 1024, 16),
    )
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

pub(super) fn take_resolve_work(committed: CommittedDelta) -> (RawTxHash, ResolveWork) {
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
    let accepted_resident_bytes = payload.serialized_bytes();
    let ContinuousResolution::Verify(verify) = resolve
        .into_verify(payload)
        .expect("fixture payload belongs to the checked-out transaction")
    else {
        panic!("fixture payload fits the reserved compute grant");
    };
    (verify, accepted_resident_bytes)
}

fn add_resources(left: ResourceVector, right: ResourceVector) -> ResourceVector {
    ResourceVector::new(
        left.entries
            .checked_add(right.entries)
            .expect("fixture fits"),
        left.bytes.checked_add(right.bytes).expect("fixture fits"),
        left.edges.checked_add(right.edges).expect("fixture fits"),
        left.active_work
            .checked_add(right.active_work)
            .expect("fixture fits"),
    )
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
    ResolvedPayload::for_foundation(tx, expanded_dependencies, 64, fee, bytes, chain_inputs)
        .expect("fixture chain evidence is a subset of inputs")
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
    hash
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

fn verify_remote_transaction_with_payload(
    authority: &mut TxPoolAuthority,
    transaction: TransactionView,
    peer: usize,
    payload: FoundationResolution,
) -> super::super::state::RawTxHash {
    let admission = ValidatedAdmission::remote(transaction, PeerIndex::from(peer))
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
    let (verify, accepted_resident_bytes) = continue_fixture_verify(resolve, payload);
    apply_without_work(
        authority
            .apply_settlement(
                verify
                    .verified(accepted_resident_bytes, 0)
                    .expect("fixture verification metrics are valid"),
            )
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
    let SettlementPlan::CoupledComponent { reason, plan } = plan else {
        panic!("fixture expected a coupled settlement");
    };
    drop(plan);
    reason
}

pub(super) fn assert_resource_reference(authority: &TxPoolAuthority) {
    let mut charges = HashMap::new();
    let mut preaccepted = ResourceVector::default();
    let mut remote = ResourceVector::default();
    let mut peers = HashMap::new();
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
                    let peer_resources = ResourceVector::new(
                        resources.entries,
                        resources.bytes,
                        resources.edges,
                        if compute_peer == Some(peer) {
                            resources.active_work
                        } else {
                            0
                        },
                    );
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
        }
    }
    assert_eq!(
        authority.resources().snapshot(),
        ResourceSnapshot {
            charges,
            preaccepted,
            remote,
            peers,
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
            OwnedTx::PreAccepted(_) => None,
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

    let mut descendant_aggregates = HashMap::new();
    let mut eviction_order = BTreeSet::new();
    for (root, root_entry) in &accepted {
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
            descendant_aggregates,
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
fn uak_source_versions_ignore_preaccepted_work_and_track_accepted_facts() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    assert_eq!(
        authority.source_versions_for_reference(),
        (ApplySequence(0), ApplySequence(0))
    );

    let disposable = admit_remote(&mut authority, 1_704, 704);
    assert_eq!(
        authority.source_versions_for_reference(),
        (ApplySequence(0), ApplySequence(0)),
        "pre-acceptance owner changes cannot stale a ChainPlan"
    );
    let version = owner_version(&authority, &disposable);
    apply_without_work(
        authority
            .plan_terminalize_for_foundation(&disposable, version)
            .expect("pre-accepted terminalization plans"),
    );
    assert_eq!(
        authority.source_versions_for_reference(),
        (ApplySequence(0), ApplySequence(0))
    );

    let accepted = accept_remote_transaction(
        &mut authority,
        tx(1_705),
        705,
        AcceptedStatus::Gap,
        Vec::new(),
    );
    let accepted_sources = authority.source_versions_for_reference();
    assert_eq!(accepted_sources.0, accepted_sources.1);
    assert_ne!(accepted_sources.0, ApplySequence(0));

    let version = owner_version(&authority, &accepted);
    let status_change = apply_committed_without_work(
        authority
            .plan_status_for_foundation(&accepted, version, AcceptedStatus::Pending)
            .expect("status-only transition plans"),
    );
    let status_sequence = only_committed_change(&status_change).sequence;
    assert_eq!(
        authority.source_versions_for_reference(),
        (accepted_sources.0, status_sequence),
        "status-only mutation must not invalidate accepted-content work"
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_generation_swap_requires_a_current_structured_drain() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let stale_empty = authority
        .drained_generation_receipt_for_foundation()
        .expect("an empty generation is drained");
    let active = admit_remote(&mut authority, 1_706, 706);
    assert_eq!(
        authority
            .plan_clear_generation_for_foundation(stale_empty)
            .err(),
        Some(PlanError::Stale(StalePlan::SourceVersion))
    );

    let version = owner_version(&authority, &active);
    let (_, work) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(&active, version, WorkPermit::ResolveOnly)
            .expect("active fixture checks out")
            .apply(),
    );
    assert_eq!(
        authority.drained_generation_receipt_for_foundation().err(),
        Some(PlanError::Backpressure(Backpressure::ActiveWorkDrain))
    );
    apply_without_work(
        authority
            .apply_settlement(work.rejected(RejectionKind::Policy))
            .expect("structured cancellation settles the only capability"),
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
    let receipt = authority
        .drained_generation_receipt_for_foundation()
        .expect("every compute capability has settled");
    drop(
        authority
            .plan_clear_generation_for_foundation(receipt)
            .expect("a complete generation replacement plans"),
    );
    assert_eq!(authority.normalized_snapshot(), before);

    let old_clocks = authority.clocks();
    let old_chain = authority.chain_view().clone();
    let receipt = authority
        .drained_generation_receipt_for_foundation()
        .expect("the dropped plan retained the drain proof conditions");
    let committed = authority
        .plan_clear_generation_for_foundation(receipt)
        .expect("clear replans")
        .apply();
    let CommittedChanges::GenerationControl(sequence) = &committed.changes else {
        panic!("fixture expected one generation commit");
    };
    let sequence = *sequence;
    assert_eq!(committed.retired_len(), 2);
    assert_eq!(authority.owner_count(), 0);
    assert_eq!(authority.charged_count(), 0);
    assert_eq!(authority.generation(), PoolGeneration(1));
    assert_eq!(authority.chain_view(), &old_chain);
    assert_eq!(
        authority.source_versions_for_reference(),
        (sequence, sequence)
    );
    assert_eq!(authority.clocks().next_version, old_clocks.next_version);
    assert_eq!(authority.clocks().next_lease, old_clocks.next_lease);
    assert_eq!(authority.clocks().next_arrival, old_clocks.next_arrival);
    assert!(authority.primary_projection_consistent());

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
fn uak_generation_chain_replacement_requires_the_next_revision() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let wrong = ChainViewId::new(ChainRevision(2), Byte32::new([72; 32]));
    let receipt = authority
        .drained_generation_receipt_for_foundation()
        .expect("empty generation is drained");
    assert_eq!(
        authority
            .plan_replace_generation_chain_for_foundation(receipt, wrong)
            .err(),
        Some(PlanError::Stale(StalePlan::ChainRevision))
    );

    let next = ChainViewId::new(ChainRevision(1), Byte32::new([73; 32]));
    let receipt = authority
        .drained_generation_receipt_for_foundation()
        .expect("failed planning did not consume authority state");
    let committed = authority
        .plan_replace_generation_chain_for_foundation(receipt, next.clone())
        .expect("the next chain revision can replace a drained generation")
        .apply();
    assert!(matches!(
        committed.changes,
        CommittedChanges::GenerationControl(_)
    ));
    assert_eq!(authority.chain_view(), &next);
    assert_eq!(authority.generation(), PoolGeneration(1));
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
    let promoted = admit_remote(&mut authority, 1_709, 708);
    apply_without_work(
        authority
            .plan_admission(
                ValidatedAdmission::proposal(promoted_tx, ProposalContextId(8))
                    .expect("promotion fixture is valid"),
            )
            .expect("promotion preserves immutable ingress"),
    );
    let survivor = admit_remote(&mut authority, 1_711, 709);

    let revoked = authority
        .plan_peer_revocation_for_foundation(banned)
        .expect("bounded peer cohort plans")
        .expect("two preaccepted owners are indexed")
        .apply();
    assert!(matches!(
        &revoked.changes,
        CommittedChanges::AdminControl { peer, .. } if *peer == banned
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
    let revoked_hashes = effect
        .effects()
        .iter()
        .map(|effect| match effect {
            CommittedEffect::PeerRevoked { tx, peer } if *peer == banned => tx.hash(),
            _ => panic!("fixture expected peer-specific cleanup detail"),
        })
        .collect::<HashSet<_>>();
    assert_eq!(
        revoked_hashes,
        HashSet::from([queued.0.clone(), promoted.0.clone()])
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
fn uak_peer_revocation_drains_promoted_active_work_before_delete() {
    let peer = PeerIndex::from(710);
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let transaction = tx(1_712);
    let hash = admit_remote(&mut authority, 1_712, 710);
    apply_without_work(
        authority
            .plan_admission(
                ValidatedAdmission::proposal(transaction, ProposalContextId(9))
                    .expect("promotion fixture is valid"),
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
    assert_eq!(
        authority.plan_peer_revocation_for_foundation(peer).err(),
        Some(PlanError::Backpressure(Backpressure::ActiveWorkDrain))
    );
    assert!(authority.entry(&hash).is_some());

    apply_without_work(
        authority
            .apply_settlement(work.rejected(RejectionKind::Policy))
            .expect("the worker returns its unique settlement capability"),
    );
    let revoked = authority
        .plan_peer_revocation_for_foundation(peer)
        .expect("drained cohort plans")
        .expect("owner remains until Apply")
        .apply();
    assert_eq!(revoked.retired_len(), 1);
    assert!(authority.entry(&hash).is_none());
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

    let duplicate = ValidatedAdmission::proposal(transaction, ProposalContextId(3))
        .expect("fixture promotion is valid");
    apply_without_work(
        authority
            .plan_admission(duplicate)
            .expect("proposal promotes the existing owner"),
    );
    assert_eq!(authority.owner_count(), 1);
    assert_eq!(authority.charged_count(), 1);
    let owner = authority.entry(&hash).expect("promoted owner exists");
    assert_eq!(
        owner.record().class,
        AdmissionClass::Proposal(ProposalLease {
            context: ProposalContextId(3),
        })
    );
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
        ValidatedAdmission::recovery(tx(5), PoolGeneration(1)).expect("fixture admission is valid");
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
    let admission = ValidatedAdmission::proposal(tx(6), ProposalContextId(5))
        .expect("fixture admission is valid");
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
            vec![CommittedEffect::Rejected {
                tx: Arc::clone(&retained_tx),
                reason: RejectionKind::Policy,
            }],
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
        CommittedEffect::Rejected { tx, reason }
            if Arc::ptr_eq(tx, &retained_tx) && *reason == RejectionKind::Policy
    ));
    let published = authority
        .apply_effect_settlement_for_foundation(lease.published())
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
            dependency_cut: DependencyCut(ApplySequence(1)),
            dependencies: KnownDependencies::default(),
        }),
        PreAcceptedPhase::Waiting(WaitCondition::Missing(observed(1))),
        PreAcceptedPhase::Computed(ComputedOutcome::Verified(VerifiedFacts::for_foundation(
            ChainRevision(0),
            DependencyCut(ApplySequence(1)),
            Arc::new(resolved_payload(&transaction).into_payload()),
            CandidateMetrics {
                fee: Capacity::shannons(1),
                cost: AcceptedCost::new(bytes, bytes, 0),
            },
        ))),
    ];
    assert_eq!(phases.len(), 4);
    assert!(matches!(
        PreAcceptedPhase::Computed(ComputedOutcome::Rejected(RejectionKind::Policy)),
        PreAcceptedPhase::Computed(_)
    ));
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
    assert_eq!(
        record.ingress,
        IngressAttribution::Peer(PeerIndex::from(17))
    );
    assert_eq!(record.blame, PayloadBlame::Peer(PeerIndex::from(17)));
    assert_eq!(record.arrival.0, 0);
    assert_eq!(authority.chain_revision(), ChainRevision(0));
    assert_eq!(authority.chain_view(), &ChainViewId::initial());
    assert_eq!(authority.generation(), PoolGeneration(0));
    assert_eq!(authority.resources().remote().entries, 1);
    assert_eq!(authority.clocks().next_lease, ComputeLeaseId(1));
    let declared_dependencies = match owner {
        OwnedTx::PreAccepted(entry) => entry.basis.dependencies().clone(),
        OwnedTx::Accepted(_) => unreachable!("fixture starts preaccepted"),
    };

    let observed_values = vec![
        DependencyKey::Cell(OutPoint::default()),
        DependencyKey::Header(Byte32::zero()),
    ];
    let resolved = super::super::state::ResolvedFacts::for_foundation(
        ChainRevision(0),
        DependencyCut(ApplySequence(1)),
        Arc::new(resolved_payload(&tx(0)).into_payload()),
        VerifyCycleClass::Small,
    );
    let observed =
        ObservedDependencies::for_foundation(observed_values, DependencyCut(ApplySequence(1)))
            .expect("fixture dependency set is non-empty");
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
            dependency_cut: DependencyCut(ApplySequence(1)),
            dependencies: declared_dependencies,
        }),
        PreAcceptedPhase::Waiting(WaitCondition::Conflict(observed)),
        PreAcceptedPhase::Computed(ComputedOutcome::Rejected(RejectionKind::Verification)),
        PreAcceptedPhase::Computed(ComputedOutcome::BudgetDenied),
        PreAcceptedPhase::Computed(ComputedOutcome::InternalFailure),
    ];
    assert_eq!(variants.len(), 7);

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
        .with_foundation_phase(
            PreAcceptedPhase::Computed(ComputedOutcome::Verified(verified.clone())),
            EntryVersion(9),
            owner.preaccepted_charge().expect("owner is preaccepted"),
        )
        .expect("preaccepted owner accepts a preaccepted phase");
    let accepted = match changed {
        OwnedTx::PreAccepted(entry) => OwnedTx::Accepted(AcceptedEntry {
            record: entry.record,
            proof: AcceptedProof::for_foundation(verified),
            proposal: ProposalContextReceipt::from_validation(AcceptedStatus::Gap),
        }),
        OwnedTx::Accepted(_) => unreachable!("fixture starts preaccepted"),
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
    )
    .expect("fixture chain evidence names one exact input");
    assert!(payload.is_chain_input(&input));
    assert!(matches!(
        ResolvedPayload::for_foundation(
            &transaction,
            vec![dependency],
            4,
            Capacity::shannons(1),
            resident_bytes,
            vec![OutPoint::new(Byte32::new([4; 32]), 0)],
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
            .plan_settlement_for_foundation(&batch)
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
            assert_eq!(only_committed_change(&single), expected);
        }

        assert_eq!(
            aggregate.normalized_snapshot(),
            reference.normalized_snapshot()
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
            .plan_settlement_for_foundation(&batch)
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
fn uak_independent_plan_drop_and_mid_batch_counter_failure_are_mutation_free() {
    let (mut dropped, hashes) = independent_fixture(3);
    let before = dropped.normalized_snapshot();
    let batch = independent_batch(&dropped, &hashes);
    let SettlementPlan::IndependentRun(plan) = dropped
        .plan_settlement_for_foundation(&batch)
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
        exhausted.plan_settlement_for_foundation(&batch).err(),
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
            .plan_settlement_for_foundation(&batch)
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
            .plan_settlement_for_foundation(&batch)
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
            .plan_settlement_for_foundation(&batch)
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
    assert!(matches!(
        conflict.plan_settlement_for_foundation(&batch),
        Err(PlanError::Membership(MembershipReject::InputConflict(input)))
            if input == conflicted_input
    ));
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
            .plan_settlement_for_foundation(&batch)
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
            .plan_settlement_for_foundation(&batch)
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
        late.plan_settlement_for_foundation(&batch)
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
    assert_eq!(
        authority.plan_settlement_for_foundation(&batch).err(),
        Some(PlanError::Membership(
            MembershipReject::MissingInputEvidence(missing)
        ))
    );
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
    assert_eq!(
        authority.plan_settlement_for_foundation(&batch).err(),
        Some(PlanError::Membership(MembershipReject::MissingPoolOutput(
            nonexistent_output
        )))
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
    assert_eq!(
        authority.plan_settlement_for_foundation(&batch).err(),
        Some(PlanError::Membership(MembershipReject::MissingPoolOutput(
            nonexistent_dependency
        )))
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
    let SettlementPlan::CoupledComponent { reason, plan } = authority
        .plan_settlement_for_foundation(&batch)
        .expect("late parent has one bounded coupled Plan")
    else {
        panic!("late parent must not use IndependentRun");
    };
    assert_eq!(reason, IndependentCoupling::AcceptedChild(child.clone()));
    let _ = plan.apply();
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
    let SettlementPlan::CoupledComponent { reason, plan } = authority
        .plan_settlement_for_foundation(&batch)
        .expect("late grandparent has one bounded coupled Plan")
    else {
        panic!("late grandparent must not use IndependentRun");
    };
    assert_eq!(reason, IndependentCoupling::AcceptedChild(parent.clone()));
    let _ = plan.apply();
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
    let SettlementPlan::CoupledComponent { reason, plan } = authority
        .plan_settlement_for_foundation(&batch)
        .expect("shared descendant path has one bounded coupled Plan")
    else {
        panic!("accepted child must route through the coupled planner");
    };
    assert_eq!(reason, IndependentCoupling::PoolParent(ancestor.clone()));
    let _ = plan.apply();

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
    let SettlementPlan::CoupledComponent { plan, .. } = authority
        .plan_settlement_for_foundation(&batch)
        .expect("late parent capacity is planned on the projected graph")
    else {
        panic!("accepted child must route through the coupled planner");
    };
    let committed = plan.apply();

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
    let SettlementPlan::CoupledComponent { plan, .. } = authority
        .plan_settlement_for_foundation(&batch)
        .expect("late-child eviction is compiled before Apply")
    else {
        panic!("accepted child must route through the coupled planner");
    };
    let committed = plan.apply();

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
    assert_eq!(
        authority.plan_settlement_for_foundation(&batch).err(),
        Some(PlanError::Membership(MembershipReject::ComponentLimit {
            limit: crate::constants::MAX_POOL_MUTATION_CANDIDATES,
        }))
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
    assert_eq!(
        authority.plan_settlement_for_foundation(&batch).err(),
        Some(PlanError::Membership(MembershipReject::ComponentLimit {
            limit: crate::constants::MAX_POOL_MUTATION_CANDIDATES,
        }))
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
    assert_eq!(
        authority.plan_settlement_for_foundation(&batch).err(),
        Some(PlanError::Membership(MembershipReject::TooManyAncestors))
    );
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

    assert_eq!(
        authority
            .plan_accept_for_foundation(&second, version, AcceptedStatus::Pending)
            .err(),
        Some(PlanError::Membership(MembershipReject::CandidateEvicted))
    );
    assert_eq!(authority.normalized_snapshot(), before);
    assert!(matches!(
        authority.entry(&first),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(matches!(
        authority.entry(&second),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computed(ComputedOutcome::Verified(_)))
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
            if matches!(entry.phase, PreAcceptedPhase::Computed(ComputedOutcome::Verified(_)))
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
    let committed = apply_committed_without_work(
        authority
            .plan_accept_for_foundation(&replacement, version, AcceptedStatus::Pending)
            .expect("complete replacement closure fits one membership plan"),
    );
    assert_eq!(committed.removals.len(), 2);
    assert_eq!(committed.retired_len(), committed.removals.len());
    assert!(
        committed
            .removals
            .iter()
            .all(|removal| removal.cause == RemovalCause::Replacement)
    );

    assert!(authority.entry(&victim).is_none());
    assert!(authority.entry(&child).is_none());
    assert!(matches!(
        authority.entry(&replacement),
        Some(OwnedTx::Accepted(_))
    ));
    assert_eq!(authority.accepted_spender(&chain_input), Some(&replacement));
    assert_eq!(authority.owner_count(), 1);
    assert_eq!(authority.resources().accepted().entries, 1);
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
    assert!(authority.entry(&victim).is_none());
    assert!(authority.entry(&child).is_none());
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
    assert_eq!(authority.owner_count(), 1);
    assert!(matches!(
        authority.entry(&replacement),
        Some(OwnedTx::Accepted(_))
    ));
    assert_resource_reference(&authority);
}

#[test]
fn uak_failed_rbf_fee_plan_preserves_candidate_and_victims() {
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
    assert!(matches!(
        authority.entry(&victim),
        Some(OwnedTx::Accepted(_))
    ));
    assert!(matches!(
        authority.entry(&replacement),
        Some(OwnedTx::PreAccepted(_))
    ));
    assert_resource_reference(&authority);
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

    assert!(authority.entry(&victim).is_none());
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

    assert_eq!(
        authority
            .plan_accept_for_foundation(&child, version, AcceptedStatus::Proposed)
            .err(),
        Some(PlanError::Membership(MembershipReject::CandidateEvicted))
    );
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
fn uak_settlement_failure_returns_a_cancellation_capability() {
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
        exhausted.error(),
        &PlanError::Fault(AuthorityFault::CounterExhausted)
    );
    assert_eq!(authority.normalized_snapshot(), before);

    authority.force_next_sequence(resumable_sequence);
    apply_without_work(
        authority
            .apply_settlement(exhausted.into_cancellation())
            .expect("returned capability deterministically releases the lease"),
    );
    assert_eq!(authority.resources().preaccepted().active_work, 0);
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(
                entry.phase,
                PreAcceptedPhase::Computed(ComputedOutcome::InternalFailure)
            )
    ));
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

    let active_version = owner_version(&authority, &hash);
    let before_active_removal = authority.normalized_snapshot();
    assert_eq!(
        authority
            .plan_terminalize_for_foundation(&hash, active_version)
            .err(),
        Some(PlanError::Backpressure(Backpressure::ActiveWorkDrain))
    );
    assert_eq!(authority.normalized_snapshot(), before_active_removal);

    let settlement = resolve.rejected(RejectionKind::Policy);
    let stale_token = SettlementToken {
        hash: settlement.token.hash.clone(),
        version: settlement.token.version,
        lease: settlement.token.lease,
    };
    apply_without_work(
        authority
            .apply_settlement(settlement)
            .expect("the active incarnation settles before retirement"),
    );
    let settled_version = owner_version(&authority, &hash);
    apply_without_work(
        authority
            .plan_terminalize_for_foundation(&hash, settled_version)
            .expect("the settled incarnation terminalizes"),
    );
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
            next: SettlementNext::Computed(ComputedOutcome::InternalFailure),
        })
        .expect_err("the retired incarnation cannot settle its successor");
    assert_eq!(stale.error(), &PlanError::Stale(StalePlan::Version));
    assert_eq!(authority.normalized_snapshot(), before_stale);
    assert_eq!(
        authority
            .entry(&hash)
            .expect("new incarnation exists")
            .record()
            .ingress,
        IngressAttribution::Peer(PeerIndex::from(47))
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
    assert_eq!(only_committed_change(&checkout).sequence, ApplySequence(2));
    assert_eq!(
        authority.resources().preaccepted(),
        ResourceVector::new(1, queued_charge.bytes, queued_charge.edges, 1)
    );
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
    let settlement = verify
        .verified(accepted_resident_bytes, 0)
        .expect("fixture verification metrics are valid");
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
            if matches!(entry.phase, PreAcceptedPhase::Computed(ComputedOutcome::Verified(_)))
    ));
    apply_without_work(
        authority
            .plan_accept_for_foundation(&hash, accepted_version, AcceptedStatus::Proposed)
            .expect("verified owner has one membership plan"),
    );
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
fn uak_compute_growth_requires_an_authority_issued_grant() {
    let mut resolve_authority = TxPoolAuthority::for_foundation(limits());
    let resolve_hash = admit_remote(&mut resolve_authority, 540, 54);
    let raw_charge = resolve_authority
        .entry(&resolve_hash)
        .and_then(OwnedTx::preaccepted_charge)
        .expect("queued raw owner is charged");
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
    let oversized = ResolutionEvidence::new(
        Vec::new(),
        Vec::new(),
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
    assert!(matches!(
        resolve_authority.entry(&resolve_hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computed(ComputedOutcome::BudgetDenied))
                && entry.charge == raw_charge
    ));
    assert_resource_reference(&resolve_authority);

    let mut verify_authority = TxPoolAuthority::for_foundation(limits());
    let verify_hash = admit_remote(&mut verify_authority, 541, 55);
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
    let denied = verify
        .verified(4 * 1024 + 1, 0)
        .expect("oversized verified residency is a typed budget outcome");
    apply_without_work(
        verify_authority
            .apply_settlement(denied)
            .expect("verified budget denial releases the active grant"),
    );
    assert!(matches!(
        verify_authority.entry(&verify_hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computed(ComputedOutcome::BudgetDenied))
                && entry.charge == entry.original_charge()
    ));
    assert_resource_reference(&verify_authority);
}

#[test]
fn uak_invalid_compute_receipt_retains_the_only_lease_settlement() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let resolve_hash = admit_remote(&mut authority, 623, 71);
    let resolve_version = owner_version(&authority, &resolve_hash);
    let (_, resolve) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(&resolve_hash, resolve_version, WorkPermit::ResolveOnly)
            .expect("resolve checkout plans")
            .apply(),
    );
    let evidence = ResolutionEvidence::new(
        Vec::new(),
        vec![OutPoint::default()],
        Capacity::shannons(1),
        resolve.transaction().data().total_size(),
        VerifyCycleClass::Small,
    );
    let failure = resolve
        .resolved(evidence)
        .expect_err("a non-input cannot be positive chain evidence");
    assert_eq!(
        failure.error(),
        &ResolutionReceiptError::InvalidEvidence(InputEvidenceError::NotAnInput)
    );
    apply_without_work(
        authority
            .apply_settlement(failure.into_settlement())
            .expect("invalid resolve receipt settles its exact lease"),
    );

    let verify_tx = tx(624);
    let verify_hash =
        queue_remote_for_verify(&mut authority, verify_tx.clone(), 72, Capacity::shannons(1));
    let verify_version = owner_version(&authority, &verify_hash);
    let committed = authority
        .plan_checkout_for_foundation(
            &verify_hash,
            verify_version,
            WorkPermit::VerifyOnly(VerifyCapability::Any),
        )
        .expect("verify checkout plans")
        .apply();
    let CheckedOutWork::Verify(verify) = committed.into_work().expect("verify work exists") else {
        panic!("verify permit returns verify work");
    };
    let underreported = verify_tx
        .data()
        .total_size()
        .checked_sub(1)
        .expect("fixture transaction is non-empty");
    let failure = verify
        .verified(underreported, 0)
        .expect_err("accepted residency cannot be smaller than serialization");
    assert_eq!(
        failure.error(),
        &VerificationReceiptError::ResidentBelowResolved
    );
    apply_without_work(
        authority
            .apply_settlement(failure.into_settlement())
            .expect("invalid verify receipt settles its exact lease"),
    );

    for hash in [&resolve_hash, &verify_hash] {
        assert!(matches!(
            authority.entry(hash),
            Some(OwnedTx::PreAccepted(entry))
                if matches!(
                    entry.phase,
                    PreAcceptedPhase::Computed(ComputedOutcome::InternalFailure)
                ) && entry.charge.active_work == 0
        ));
    }
    assert!(authority.primary_projection_consistent());
    assert_resource_reference(&authority);
}

#[test]
fn uak_verified_residency_cannot_undercharge_retained_resolution() {
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
    let resolved = resolve
        .resolved(ResolutionEvidence::new(
            Vec::new(),
            Vec::new(),
            Capacity::shannons(1),
            resolved_resident_bytes,
            VerifyCycleClass::Small,
        ))
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
    let failure = verify
        .verified(serialized_bytes, 0)
        .expect_err("accepted residency cannot undercharge retained resolution");
    assert_eq!(
        failure.error(),
        &VerificationReceiptError::ResidentBelowResolved
    );
    apply_without_work(
        authority
            .apply_settlement(failure.into_settlement())
            .expect("invalid verify receipt settles its exact lease"),
    );

    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(
                entry.phase,
                PreAcceptedPhase::Computed(ComputedOutcome::InternalFailure)
            ) && entry.charge == entry.original_charge()
    ));
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
    let (verify, accepted_resident_bytes) = continue_fixture_verify(resolve, payload);
    apply_without_work(
        authority
            .apply_settlement(
                verify
                    .verified(accepted_resident_bytes, 0)
                    .expect("fixture verification metrics are valid"),
            )
            .expect("verified settlement plans"),
    );
    assert_eq!(authority.owner_count(), 1);
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computed(ComputedOutcome::Verified(_)))
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
    let rejected_version = owner_version(&rejected, &rejected_hash);
    let before = rejected.normalized_snapshot();
    assert_eq!(
        rejected
            .plan_accept_for_foundation(&rejected_hash, rejected_version, AcceptedStatus::Pending,)
            .err(),
        Some(PlanError::Stale(StalePlan::Phase))
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
                PreAcceptedPhase::Waiting(WaitCondition::Missing(deps)) if deps.len() == 1
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
    let evidence = ResolutionEvidence::new(
        Vec::new(),
        Vec::new(),
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
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(
                entry.phase,
                PreAcceptedPhase::Computed(ComputedOutcome::Rejected(
                    RejectionKind::Verification
                ))
            )
    ));
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
    let (verify, accepted_resident_bytes) = continue_fixture_verify(second, payload);
    let settlement = verify
        .verified(accepted_resident_bytes, 0)
        .expect("fixture verification metrics are valid");
    authority.force_chain_view(ChainViewId::new(ChainRevision(1), Byte32::new([16; 32])));
    let forged = ComputeSettlement {
        token: SettlementToken {
            hash: settlement.token.hash.clone(),
            version: settlement.token.version,
            lease: ComputeLeaseId(u128::MAX),
        },
        next: SettlementNext::Computed(ComputedOutcome::InternalFailure),
    };
    let before_forged = authority.normalized_snapshot();
    let stale_lease = authority
        .apply_settlement(forged)
        .expect_err("a forged compute lease is stale");
    assert_eq!(stale_lease.error(), &PlanError::Stale(StalePlan::Lease));
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
    let accepted_resident_bytes = payload.serialized_bytes();
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
            .apply_settlement(
                verify
                    .verified(accepted_resident_bytes, 0)
                    .expect("fixture verification metrics are valid"),
            )
            .expect("verify success settles"),
    );

    assert!(authority.primary_projection_consistent());
    assert!(matches!(
        authority.entry(&resolve_reject_hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computed(ComputedOutcome::Rejected(_)))
    ));
    assert!(matches!(
        authority.entry(&continuous_missing_hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Waiting(WaitCondition::Missing(_)))
    ));
    assert!(matches!(
        authority.entry(&verify_success_hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computed(ComputedOutcome::Verified(_)))
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
    let trusted_admission = ValidatedAdmission::proposal(tx(604), ProposalContextId(1))
        .expect("fixture proposal admission is valid");
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
    let proposal_admission = ValidatedAdmission::proposal(tx(611), ProposalContextId(1))
        .expect("fixture proposal admission is valid");
    let proposal = proposal_admission.identity.raw.clone();
    apply_without_work(
        authority
            .plan_admission(proposal_admission)
            .expect("proposal admission plans"),
    );
    let recovery_admission = ValidatedAdmission::recovery(tx(612), PoolGeneration(1))
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
fn uak_trusted_checkout_does_not_reset_remote_fairness_progress() {
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

    let trusted_admission = ValidatedAdmission::proposal(tx(620), ProposalContextId(2))
        .expect("fixture proposal admission is valid");
    let trusted = trusted_admission.identity.raw.clone();
    apply_without_work(
        authority
            .plan_admission(trusted_admission)
            .expect("trusted admission plans"),
    );
    let (selected, trusted_work) = take_resolve_work(
        authority
            .plan_checkout_next(WorkPermit::ResolveOnly)
            .expect("trusted selection is valid")
            .expect("trusted work has priority")
            .apply(),
    );
    assert_eq!(selected, trusted);
    apply_without_work(
        authority
            .apply_settlement(trusted_work.rejected(RejectionKind::Policy))
            .expect("trusted work settles"),
    );

    let (selected, peer_b_work) = take_resolve_work(
        authority
            .plan_checkout_next(WorkPermit::ResolveOnly)
            .expect("remote fairness resumes")
            .expect("peer B remains selectable")
            .apply(),
    );
    assert_eq!(selected, peer_b);
    apply_without_work(
        authority
            .apply_settlement(peer_b_work.rejected(RejectionKind::Policy))
            .expect("peer B work settles"),
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
    let proposal = ValidatedAdmission::proposal(tx(613), ProposalContextId(1))
        .expect("fixture proposal admission is valid");
    let recovery = ValidatedAdmission::recovery(tx(614), PoolGeneration(0))
        .expect("fixture recovery admission is valid");
    let remote = ValidatedAdmission::remote(tx(615), PeerIndex::from(66))
        .expect("fixture remote admission is valid");
    let total = [proposal.charge, recovery.charge, remote.charge]
        .into_iter()
        .reduce(add_resources)
        .expect("fixture has admissions");
    let constrained = ResourceLimits::new(
        ResourceVector::new(total.entries, total.bytes, total.edges, 3),
        ResourceVector::new(
            remote.charge.entries,
            remote.charge.bytes,
            remote.charge.edges,
            1,
        ),
        ResourceVector::new(
            remote.charge.entries,
            remote.charge.bytes,
            remote.charge.edges,
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
        .expect("the next transaction in the same trusted owner remains visible");
    assert_eq!(second_probes, 1);
    let (selected, second_work) =
        take_resolve_work(second.expect("second trusted work exists").apply());
    assert_eq!(selected, proposal_hash);

    for work in [first_work, second_work] {
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
    let trusted_admission = ValidatedAdmission::proposal(tx(619), ProposalContextId(2))
        .expect("trusted proposal fixture is valid");
    let remote = add_resources(active_admission.charge, promoted_admission.charge);
    let total = add_resources(remote, trusted_admission.charge);
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
                ValidatedAdmission::proposal(tx(618), ProposalContextId(3))
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
                .apply_settlement(work.rejected(RejectionKind::Policy))
                .expect("every active lease settles exactly once"),
        );
    }
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_retained_growth_denial_atomically_releases_the_compute_lease() {
    let admission = ValidatedAdmission::remote(tx(1_000), PeerIndex::from(900))
        .expect("capacity fixture admission is valid");
    let raw_charge = admission.charge;
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
    let evidence = ResolutionEvidence::new(
        Vec::new(),
        Vec::new(),
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
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computed(ComputedOutcome::BudgetDenied))
                && entry.charge == raw_charge
    ));
    assert_eq!(authority.resources().preaccepted(), raw_charge);
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
        .map(|admission| admission.charge)
        .reduce(add_resources)
        .expect("fixture has owners");
    let one = admissions[0].charge;
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
        .plan_settlement_for_foundation(&batch)
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
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computed(ComputedOutcome::Rejected(_)))
    ));
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
            if matches!(entry.phase, PreAcceptedPhase::Computed(ComputedOutcome::InternalFailure))
    ));
    assert!(authority.primary_projection_consistent());
}
