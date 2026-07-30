use super::super::plan::{
    AuthorityFault, Backpressure, PlanError, PreparedApply, StalePlan, TxPoolAuthority,
};
use super::super::resources::{ChargeRecord, ResourceLimits, ResourceSnapshot, ResourceVector};
use super::super::state::{
    AcceptedEntry, AcceptedStatus, ActiveWork, AdmissionClass, ApplySequence, ChainEpoch,
    ComputeLeaseId, ComputedOutcome, DependencyEpoch, DependencyKey, EntryVersion,
    IngressAttribution, ObservedDependencies, ObservedDependency, OwnedTx, PayloadBlame,
    PreAcceptedPhase, ProposalContextId, ProposalLease, QueuedWork, RejectionKind,
    ValidatedAdmission, VerifiedFacts, WaitCondition, WitnessTxHash, WorkPermit,
};
use super::super::work::CheckedOutWork;
use ckb_network::PeerIndex;
use ckb_types::{
    bytes::Bytes,
    core::TransactionBuilder,
    packed::{Byte32, OutPoint},
    prelude::Pack,
};
use std::collections::HashMap;

fn limits() -> ResourceLimits {
    ResourceLimits {
        total: ResourceVector::new(8, 64 * 1024, 64, 8),
        remote: ResourceVector::new(4, 32 * 1024, 32, 4),
        per_peer: ResourceVector::new(2, 16 * 1024, 16, 2),
    }
}

fn tx(nonce: u64) -> ckb_types::core::TransactionView {
    TransactionBuilder::default().version(nonce as u32).build()
}

fn observed(epoch: u64) -> ObservedDependencies {
    ObservedDependencies::new(vec![ObservedDependency {
        key: DependencyKey::Cell(OutPoint::default()),
        epoch: DependencyEpoch(epoch),
    }])
    .expect("fixture dependency set is non-empty")
}

fn admit_remote(
    authority: &mut TxPoolAuthority,
    nonce: u64,
    peer: usize,
) -> super::super::state::RawTxHash {
    let admission = ValidatedAdmission::remote(tx(nonce), PeerIndex::from(peer), 1)
        .expect("fixture admission is valid");
    let hash = admission.identity.raw.clone();
    apply_without_work(
        authority
            .plan_admission(admission)
            .expect("fixture admission plans"),
    );
    hash
}

fn owner_version(
    authority: &TxPoolAuthority,
    hash: &super::super::state::RawTxHash,
) -> EntryVersion {
    authority
        .entry(hash)
        .expect("owner exists")
        .record()
        .version
}

fn apply_without_work(plan: PreparedApply<'_>) {
    let committed = plan.apply();
    assert!(
        committed.work.is_none(),
        "transition unexpectedly issued work"
    );
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

fn assert_resource_reference(authority: &TxPoolAuthority) {
    let mut charges = HashMap::new();
    let mut total = ResourceVector::default();
    let mut remote = ResourceVector::default();
    let mut peers = HashMap::new();
    for (hash, owner) in authority.entries_for_reference() {
        let record = owner.record();
        let peer = record.ingress.peer();
        let charge = ChargeRecord {
            resources: record.charge,
            peer,
        };
        assert!(charges.insert(hash.clone(), charge).is_none());
        total = add_resources(total, record.charge);
        if let Some(peer) = peer {
            remote = add_resources(remote, record.charge);
            let usage = peers.entry(peer).or_default();
            *usage = add_resources(*usage, record.charge);
        }
    }
    assert_eq!(
        authority.resources().snapshot(),
        ResourceSnapshot {
            charges,
            total,
            remote,
            peers,
        }
    );
}

#[test]
fn uak_remote_admission_owns_and_charges_once() {
    let mut authority = TxPoolAuthority::new(limits());
    let admission = ValidatedAdmission::remote(tx(1), PeerIndex::from(7), 3)
        .expect("fixture admission is valid");
    let hash = admission.identity.raw.clone();
    let delta = authority
        .plan_admission(admission)
        .expect("bounded first admission plans")
        .apply();

    assert_eq!(delta.changed, hash);
    assert!(delta.work.is_none());
    assert_eq!(authority.owner_count(), 1);
    assert_eq!(authority.charged_count(), 1);
    assert!(authority.primary_projection_consistent());
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(_))
    ));
}

#[test]
fn uak_duplicate_and_promotion_never_create_second_owner() {
    let mut authority = TxPoolAuthority::new(limits());
    let transaction = tx(2);
    let first = ValidatedAdmission::remote(transaction.clone(), PeerIndex::from(9), 1)
        .expect("fixture admission is valid");
    let hash = first.identity.raw.clone();
    apply_without_work(
        authority
            .plan_admission(first)
            .expect("first admission plans"),
    );

    let version = owner_version(&authority, &hash);
    let checkout = authority
        .plan_checkout(&hash, version, WorkPermit::ResolveThenVerify)
        .expect("remote resolve checkout plans")
        .apply();
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.work.expect("work exists") else {
        panic!("continuous permit returns continuous resolve work");
    };

    let duplicate = ValidatedAdmission::proposal(transaction, ProposalContextId(3), 1)
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
        owner.record().charge
    );
    assert_eq!(authority.resources().remote(), owner.record().charge);
    assert!(matches!(
        owner,
        OwnedTx::PreAccepted(entry)
            if matches!(entry.phase, PreAcceptedPhase::Computing(_))
    ));
    assert!(authority.primary_projection_consistent());
    assert_resource_reference(&authority);

    apply_without_work(
        authority
            .plan_settlement(resolve.rejected(RejectionKind::Policy))
            .expect("promotion does not invalidate the active compute lease"),
    );
}

#[test]
fn uak_payload_variant_is_not_misclassified_as_duplicate() {
    let mut authority = TxPoolAuthority::new(limits());
    let raw = tx(23);
    let first = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"first").pack()])
        .build();
    let second = raw
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"second").pack()])
        .build();
    let admission = ValidatedAdmission::remote(first, PeerIndex::from(42), 1)
        .expect("fixture admission is valid");
    apply_without_work(
        authority
            .plan_admission(admission)
            .expect("first witness variant plans"),
    );
    let before = authority.normalized_snapshot();
    let variant = ValidatedAdmission::remote(second, PeerIndex::from(43), 1)
        .expect("second witness variant is structurally valid");
    assert_eq!(
        authority.plan_admission(variant).err(),
        Some(PlanError::PayloadVariant)
    );
    assert_eq!(authority.normalized_snapshot(), before);
}

#[test]
fn uak_short_id_collision_cannot_alias_primary_identity() {
    let mut authority = TxPoolAuthority::new(limits());
    let first = ValidatedAdmission::remote(tx(3), PeerIndex::from(11), 1)
        .expect("fixture admission is valid");
    let proposal = first.identity.proposal.clone();
    apply_without_work(
        authority
            .plan_admission(first)
            .expect("first admission plans"),
    );

    let mut second = ValidatedAdmission::remote(tx(4), PeerIndex::from(12), 1)
        .expect("fixture admission is valid");
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
fn uak_failed_membership_plan_is_byte_for_byte_mutation_free() {
    let mut authority = TxPoolAuthority::new(limits());
    let admission =
        ValidatedAdmission::recovery(tx(5), ChainEpoch(1), 1).expect("fixture admission is valid");
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
    let mut authority = TxPoolAuthority::new(limits());
    let admission = ValidatedAdmission::proposal(tx(6), ProposalContextId(5), 1)
        .expect("fixture admission is valid");
    let hash = admission.identity.raw.clone();
    apply_without_work(
        authority
            .plan_admission(admission)
            .expect("admission plans"),
    );
    let current = authority.entry(&hash).expect("owner exists").clone();
    let version = current.record().version;
    let terminal = authority
        .plan_terminalize_for_foundation(&hash, version)
        .expect("terminal plan is complete")
        .apply();

    assert_eq!(terminal.changed, hash);
    assert!(terminal.work.is_none());
    assert_eq!(authority.owner_count(), 0);
    assert_eq!(authority.charged_count(), 0);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_all_four_preaccepted_phases_are_closed_variants() {
    let witness = WitnessTxHash(Byte32::zero());
    let phases = [
        PreAcceptedPhase::Queued(QueuedWork::Resolve),
        PreAcceptedPhase::Computing(ActiveWork {
            lease: ComputeLeaseId(1),
            permit: WorkPermit::ResolveThenVerify,
        }),
        PreAcceptedPhase::Waiting(WaitCondition::Missing(observed(1))),
        PreAcceptedPhase::Computed(ComputedOutcome::Verified(VerifiedFacts {
            witness,
            chain_epoch: ChainEpoch(0),
        })),
    ];
    assert_eq!(phases.len(), 4);
    assert!(matches!(
        PreAcceptedPhase::Computed(ComputedOutcome::Rejected(RejectionKind::Policy)),
        PreAcceptedPhase::Computed(_)
    ));
}

#[test]
fn uak_foundation_types_preserve_distinct_domains_without_dead_state() {
    let mut authority = TxPoolAuthority::new(limits());
    let admission = ValidatedAdmission::remote(tx(7), PeerIndex::from(17), 2)
        .expect("fixture admission is valid");
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
    assert_eq!(authority.chain_epoch(), ChainEpoch(0));
    assert_eq!(authority.resources().remote().entries, 1);
    assert_eq!(authority.clocks().next_lease, ComputeLeaseId(1));

    let observed_values = vec![
        ObservedDependency {
            key: DependencyKey::Cell(OutPoint::default()),
            epoch: DependencyEpoch(1),
        },
        ObservedDependency {
            key: DependencyKey::Header(Byte32::zero()),
            epoch: DependencyEpoch(2),
        },
    ];
    let resolved = super::super::state::ResolvedFacts {
        chain_epoch: ChainEpoch(0),
        dependency_count: observed_values.len(),
    };
    let observed =
        ObservedDependencies::new(observed_values).expect("fixture dependency set is non-empty");
    let variants = [
        PreAcceptedPhase::Queued(QueuedWork::Verify(resolved)),
        PreAcceptedPhase::Computing(ActiveWork {
            lease: ComputeLeaseId(2),
            permit: WorkPermit::ResolveOnly,
        }),
        PreAcceptedPhase::Computing(ActiveWork {
            lease: ComputeLeaseId(3),
            permit: WorkPermit::VerifyOnly,
        }),
        PreAcceptedPhase::Waiting(WaitCondition::Conflict(observed)),
        PreAcceptedPhase::Computed(ComputedOutcome::Rejected(RejectionKind::Verification)),
        PreAcceptedPhase::Computed(ComputedOutcome::BudgetDenied),
        PreAcceptedPhase::Computed(ComputedOutcome::InternalFailure),
    ];
    assert_eq!(variants.len(), 7);

    let changed = owner
        .with_foundation_phase(
            PreAcceptedPhase::Computed(ComputedOutcome::Verified(VerifiedFacts {
                witness: WitnessTxHash(Byte32::zero()),
                chain_epoch: ChainEpoch(0),
            })),
            EntryVersion(9),
            record.charge,
        )
        .expect("preaccepted owner accepts a preaccepted phase");
    let accepted = match changed {
        OwnedTx::PreAccepted(entry) => OwnedTx::Accepted(AcceptedEntry {
            record: entry.record,
            status: AcceptedStatus::Gap,
        }),
        OwnedTx::Accepted(_) => unreachable!("fixture starts preaccepted"),
    };
    assert!(matches!(
        accepted,
        OwnedTx::Accepted(AcceptedEntry {
            status: AcceptedStatus::Gap,
            ..
        })
    ));
    assert_ne!(AcceptedStatus::Proposed, AcceptedStatus::Pending);
}

#[test]
fn uak_resource_limit_failure_preserves_every_observable_fact() {
    let mut authority = TxPoolAuthority::new(limits());
    for nonce in [8, 9] {
        let plan = authority
            .plan_admission(
                ValidatedAdmission::remote(tx(nonce), PeerIndex::from(21), 1)
                    .expect("fixture admission is valid"),
            )
            .expect("peer capacity holds two entries");
        apply_without_work(plan);
    }
    let before = authority.normalized_snapshot();
    let result = authority
        .plan_admission(
            ValidatedAdmission::remote(tx(10), PeerIndex::from(21), 1)
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
fn uak_counter_exhaustion_is_typed_and_mutation_free() {
    let mut authority = TxPoolAuthority::new(limits());
    authority.force_next_sequence(ApplySequence(u128::MAX));
    let before = authority.normalized_snapshot();
    let result = authority
        .plan_admission(
            ValidatedAdmission::remote(tx(11), PeerIndex::from(22), 1)
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
fn uak_dropped_prepared_apply_is_semantically_mutation_free() {
    let mut authority = TxPoolAuthority::new(limits());
    let before = authority.normalized_snapshot();
    {
        let prepared = authority
            .plan_admission(
                ValidatedAdmission::remote(tx(24), PeerIndex::from(44), 1)
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
    let limits = ResourceLimits {
        total: ResourceVector::new(4, 64 * 1024, 64, 4),
        remote: ResourceVector::new(4, 64 * 1024, 64, 4),
        per_peer: ResourceVector::new(4, 64 * 1024, 64, 1),
    };
    let mut authority = TxPoolAuthority::new(limits);
    let first = admit_remote(&mut authority, 25, 45);
    let second = admit_remote(&mut authority, 26, 45);
    let version = owner_version(&authority, &first);
    let checkout = authority
        .plan_checkout(&first, version, WorkPermit::ResolveOnly)
        .expect("first peer work grant fits")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.work.expect("resolve work exists") else {
        panic!("resolve-only permit returns resolve work");
    };

    let before = authority.normalized_snapshot();
    let version = owner_version(&authority, &second);
    assert_eq!(
        authority
            .plan_checkout(&second, version, WorkPermit::ResolveOnly)
            .err(),
        Some(PlanError::Backpressure(Backpressure::PeerResources))
    );
    assert_eq!(authority.normalized_snapshot(), before);
    assert_resource_reference(&authority);

    apply_without_work(
        authority
            .plan_settlement(resolve.rejected(RejectionKind::Policy))
            .expect("live lease still settles after peer backpressure"),
    );
}

#[test]
fn uak_stale_lease_is_mutation_free_across_aba() {
    let mut authority = TxPoolAuthority::new(limits());
    let transaction = tx(27);
    let first = ValidatedAdmission::remote(transaction.clone(), PeerIndex::from(46), 1)
        .expect("fixture admission is valid");
    let hash = first.identity.raw.clone();
    apply_without_work(
        authority
            .plan_admission(first)
            .expect("first incarnation plans"),
    );
    let version = owner_version(&authority, &hash);
    let checkout = authority
        .plan_checkout(&hash, version, WorkPermit::ResolveOnly)
        .expect("first incarnation checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.work.expect("resolve work exists") else {
        panic!("resolve-only permit returns resolve work");
    };

    let active_version = owner_version(&authority, &hash);
    apply_without_work(
        authority
            .plan_terminalize_for_foundation(&hash, active_version)
            .expect("first incarnation terminalizes"),
    );
    apply_without_work(
        authority
            .plan_admission(
                ValidatedAdmission::remote(transaction, PeerIndex::from(47), 1)
                    .expect("readmission is valid"),
            )
            .expect("same raw hash obtains a fresh incarnation"),
    );
    let before_stale = authority.normalized_snapshot();
    assert_eq!(
        authority
            .plan_settlement(resolve.rejected(RejectionKind::Policy))
            .err(),
        Some(PlanError::Stale(StalePlan::Version))
    );
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
    let mut authority = TxPoolAuthority::new(limits());
    let hash = admit_remote(&mut authority, 12, 31);
    let version = owner_version(&authority, &hash);
    let checkout = authority
        .plan_checkout(&hash, version, WorkPermit::ResolveThenVerify)
        .expect("queued resolve accepts a continuous permit")
        .apply();
    assert_eq!(checkout.sequence, ApplySequence(2));
    assert_eq!(authority.resources().total().active_work, 1);
    assert!(authority.primary_projection_consistent());
    let before_local_continuation = authority.normalized_snapshot();
    let CheckedOutWork::ContinuousResolve(resolve) =
        checkout.work.expect("checkout returns one work capability")
    else {
        panic!("continuous permit returns continuous resolve work");
    };
    let verify = resolve.into_verify(3);
    assert_eq!(authority.normalized_snapshot(), before_local_continuation);
    let settlement = verify.verified();
    apply_without_work(
        authority
            .plan_settlement(settlement)
            .expect("current continuous lease settles"),
    );
    assert_eq!(authority.resources().total().active_work, 0);
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
        Some(OwnedTx::Accepted(AcceptedEntry {
            status: AcceptedStatus::Proposed,
            ..
        }))
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_resolve_to_verify_continuation_changes_no_authority_state() {
    let mut authority = TxPoolAuthority::new(limits());
    let hash = admit_remote(&mut authority, 28, 48);
    let version = owner_version(&authority, &hash);
    let checkout = authority
        .plan_checkout(&hash, version, WorkPermit::ResolveThenVerify)
        .expect("continuous checkout plans")
        .apply();
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.work.expect("work exists") else {
        panic!("continuous permit returns continuous resolve work");
    };
    let before = authority.normalized_snapshot();
    let verify = resolve.into_verify(2);
    assert_eq!(authority.normalized_snapshot(), before);
    apply_without_work(
        authority
            .plan_settlement(verify.internal_failure())
            .expect("continuous lease remains current"),
    );
    assert_resource_reference(&authority);
}

#[test]
fn uak_verified_settlement_has_one_ready_projection() {
    let mut authority = TxPoolAuthority::new(limits());
    let hash = admit_remote(&mut authority, 29, 49);
    let version = owner_version(&authority, &hash);
    let checkout = authority
        .plan_checkout(&hash, version, WorkPermit::ResolveThenVerify)
        .expect("continuous checkout plans")
        .apply();
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.work.expect("work exists") else {
        panic!("continuous permit returns continuous resolve work");
    };
    apply_without_work(
        authority
            .plan_settlement(resolve.into_verify(2).verified())
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
    let mut queued = TxPoolAuthority::new(limits());
    let queued_hash = admit_remote(&mut queued, 30, 50);
    let queued_version = owner_version(&queued, &queued_hash);
    let before = queued.normalized_snapshot();
    assert_eq!(
        queued
            .plan_checkout(&queued_hash, queued_version, WorkPermit::VerifyOnly)
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
        .plan_checkout(&queued_hash, queued_version, WorkPermit::ResolveOnly)
        .expect("resolve checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.work.expect("work exists") else {
        panic!("resolve-only permit returns resolve work");
    };
    apply_without_work(
        queued
            .plan_settlement(resolve.missing(observed(11)))
            .expect("missing settlement plans"),
    );
    let waiting_version = owner_version(&queued, &queued_hash);
    let before = queued.normalized_snapshot();
    assert_eq!(
        queued
            .plan_checkout(&queued_hash, waiting_version, WorkPermit::ResolveOnly)
            .err(),
        Some(PlanError::Stale(StalePlan::Phase))
    );
    assert_eq!(queued.normalized_snapshot(), before);

    let mut rejected = TxPoolAuthority::new(limits());
    let rejected_hash = admit_remote(&mut rejected, 31, 51);
    let version = owner_version(&rejected, &rejected_hash);
    let checkout = rejected
        .plan_checkout(&rejected_hash, version, WorkPermit::ResolveOnly)
        .expect("resolve checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.work.expect("work exists") else {
        panic!("resolve-only permit returns resolve work");
    };
    apply_without_work(
        rejected
            .plan_settlement(resolve.rejected(RejectionKind::Policy))
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
    let mut authority = TxPoolAuthority::new(limits());
    let hash = admit_remote(&mut authority, 13, 32);
    let version = owner_version(&authority, &hash);
    let checkout = authority
        .plan_checkout(&hash, version, WorkPermit::ResolveOnly)
        .expect("resolve checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.work.expect("resolve work exists") else {
        panic!("resolve-only permit returns resolve work");
    };
    assert_eq!(resolve.transaction().hash(), hash.0);
    apply_without_work(
        authority
            .plan_settlement(resolve.missing(observed(4)))
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
    assert_eq!(authority.resources().total().active_work, 0);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_continuation_yield_returns_one_queued_owner() {
    let mut authority = TxPoolAuthority::new(limits());
    let hash = admit_remote(&mut authority, 14, 33);
    let version = owner_version(&authority, &hash);
    let checkout = authority
        .plan_checkout(&hash, version, WorkPermit::ResolveOnly)
        .expect("resolve checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.work.expect("resolve work exists") else {
        panic!("resolve-only permit returns resolve work");
    };
    apply_without_work(
        authority
            .plan_settlement(resolve.yield_verify(5))
            .expect("yielded resolve settles as queued verify"),
    );
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Verify(_)))
    ));

    let version = owner_version(&authority, &hash);
    let verify_checkout = authority
        .plan_checkout(&hash, version, WorkPermit::VerifyOnly)
        .expect("queued verify accepts verify-only permit")
        .apply();
    let CheckedOutWork::Verify(verify) = verify_checkout.work.expect("verify work exists") else {
        panic!("verify-only permit returns verify work");
    };
    apply_without_work(
        authority
            .plan_settlement(verify.rejected(RejectionKind::Verification))
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
fn uak_stale_lease_is_mutation_free_across_chain_epoch_and_token_mismatch() {
    let mut authority = TxPoolAuthority::new(limits());
    let hash = admit_remote(&mut authority, 15, 34);
    let version = owner_version(&authority, &hash);
    let checkout = authority
        .plan_checkout(&hash, version, WorkPermit::ResolveThenVerify)
        .expect("continuous checkout plans")
        .apply();
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.work.expect("work exists") else {
        panic!("continuous permit returns continuous resolve work");
    };
    let settlement = resolve.into_verify(2).internal_failure();
    authority.force_chain_epoch(ChainEpoch(1));
    let before = authority.normalized_snapshot();
    assert_eq!(
        authority.plan_settlement(settlement).err(),
        Some(PlanError::Stale(StalePlan::ChainEpoch))
    );
    assert_eq!(authority.normalized_snapshot(), before);

    let second_hash = admit_remote(&mut authority, 16, 35);
    let version = owner_version(&authority, &second_hash);
    let second_checkout = authority
        .plan_checkout(&second_hash, version, WorkPermit::ResolveThenVerify)
        .expect("second checkout plans")
        .apply();
    let CheckedOutWork::ContinuousResolve(second) =
        second_checkout.work.expect("second work exists")
    else {
        panic!("continuous permit returns continuous resolve work");
    };
    let mut forged = second.into_verify(1).verified();
    forged.token.lease = ComputeLeaseId(u128::MAX);
    let before_forged = authority.normalized_snapshot();
    assert_eq!(
        authority.plan_settlement(forged).err(),
        Some(PlanError::Stale(StalePlan::Lease))
    );
    assert_eq!(authority.normalized_snapshot(), before_forged);
}

#[test]
fn uak_every_resolve_and_verify_terminal_shape_is_typed() {
    let mut authority = TxPoolAuthority::new(limits());

    let resolve_reject_hash = admit_remote(&mut authority, 17, 36);
    let version = owner_version(&authority, &resolve_reject_hash);
    let resolve_checkout = authority
        .plan_checkout(&resolve_reject_hash, version, WorkPermit::ResolveOnly)
        .expect("resolve checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = resolve_checkout.work.expect("resolve work exists")
    else {
        panic!("resolve-only permit returns resolve work");
    };
    apply_without_work(
        authority
            .plan_settlement(resolve.rejected(RejectionKind::Policy))
            .expect("resolve rejection settles"),
    );

    let continuous_missing_hash = admit_remote(&mut authority, 18, 37);
    let version = owner_version(&authority, &continuous_missing_hash);
    let continuous_checkout = authority
        .plan_checkout(
            &continuous_missing_hash,
            version,
            WorkPermit::ResolveThenVerify,
        )
        .expect("continuous checkout plans")
        .apply();
    let CheckedOutWork::ContinuousResolve(continuous) =
        continuous_checkout.work.expect("continuous work exists")
    else {
        panic!("continuous permit returns continuous resolve work");
    };
    apply_without_work(
        authority
            .plan_settlement(continuous.missing(observed(9)))
            .expect("continuous missing settles"),
    );

    let verify_success_hash = admit_remote(&mut authority, 19, 38);
    let version = owner_version(&authority, &verify_success_hash);
    let first = authority
        .plan_checkout(&verify_success_hash, version, WorkPermit::ResolveOnly)
        .expect("resolve checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = first.work.expect("resolve work exists") else {
        panic!("resolve-only permit returns resolve work");
    };
    apply_without_work(
        authority
            .plan_settlement(resolve.yield_verify(1))
            .expect("resolve yield settles"),
    );
    let version = owner_version(&authority, &verify_success_hash);
    let second = authority
        .plan_checkout(&verify_success_hash, version, WorkPermit::VerifyOnly)
        .expect("verify checkout plans")
        .apply();
    let CheckedOutWork::Verify(verify) = second.work.expect("verify work exists") else {
        panic!("verify-only permit returns verify work");
    };
    apply_without_work(
        authority
            .plan_settlement(verify.verified())
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

    let mut authority = TxPoolAuthority::new(limits());
    let continuous_reject_hash = admit_remote(&mut authority, 20, 39);
    let version = owner_version(&authority, &continuous_reject_hash);
    let checkout = authority
        .plan_checkout(
            &continuous_reject_hash,
            version,
            WorkPermit::ResolveThenVerify,
        )
        .expect("continuous checkout plans")
        .apply();
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.work.expect("work exists") else {
        panic!("continuous permit returns continuous resolve work");
    };
    apply_without_work(
        authority
            .plan_settlement(resolve.rejected(RejectionKind::Policy))
            .expect("continuous resolve rejection settles"),
    );

    let verify_failure_hash = admit_remote(&mut authority, 21, 40);
    let version = owner_version(&authority, &verify_failure_hash);
    let checkout = authority
        .plan_checkout(&verify_failure_hash, version, WorkPermit::ResolveOnly)
        .expect("resolve checkout plans")
        .apply();
    let CheckedOutWork::Resolve(resolve) = checkout.work.expect("work exists") else {
        panic!("resolve-only permit returns resolve work");
    };
    apply_without_work(
        authority
            .plan_settlement(resolve.yield_verify(1))
            .expect("resolve yield settles"),
    );
    let version = owner_version(&authority, &verify_failure_hash);
    let checkout = authority
        .plan_checkout(&verify_failure_hash, version, WorkPermit::VerifyOnly)
        .expect("verify checkout plans")
        .apply();
    let CheckedOutWork::Verify(verify) = checkout.work.expect("work exists") else {
        panic!("verify-only permit returns verify work");
    };
    apply_without_work(
        authority
            .plan_settlement(verify.internal_failure())
            .expect("verify worker failure settles"),
    );

    let continuous_verify_reject_hash = admit_remote(&mut authority, 22, 41);
    let version = owner_version(&authority, &continuous_verify_reject_hash);
    let checkout = authority
        .plan_checkout(
            &continuous_verify_reject_hash,
            version,
            WorkPermit::ResolveThenVerify,
        )
        .expect("continuous checkout plans")
        .apply();
    let CheckedOutWork::ContinuousResolve(resolve) = checkout.work.expect("work exists") else {
        panic!("continuous permit returns continuous resolve work");
    };
    apply_without_work(
        authority
            .plan_settlement(resolve.into_verify(1).rejected(RejectionKind::Verification))
            .expect("continuous verification rejection settles"),
    );
    assert!(authority.primary_projection_consistent());
}
