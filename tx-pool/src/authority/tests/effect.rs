use super::super::{
    effect::{
        CommittedAcceptance, CommittedConflictOwner, CommittedEffect, CommittedEntrySnapshot,
        CommittedRejection, CommittedRemoteIngressRelease, EffectBatchBound, EffectBatchBounds,
        EffectBuildError, EffectCapacity, EffectConfigError, EffectLimits, EffectPolicy,
        EffectPublication, EffectReceipt, EffectSettlementError, ParentTransactionRequest,
        RejectionAudience,
    },
    plan::{
        AuthorityFault, Backpressure, CommittedDelta, ComputeSettlementRecovery, EffectCloseError,
        MembershipReject, PlanError, TxPoolAuthority,
    },
    rejection::CommittedPublicReject,
    runtime::AuthorityRuntime,
    state::{
        AcceptedStatus, ApplySequence, DependencyKey, OwnedTx, PreAcceptedPhase, RawTxHash,
        ValidatedAdmission, WorkPermit, test_support::RejectionKind,
    },
};
use super::foundation::{
    FixtureCommit, admit_remote, genesis_snapshot, limits, owner_version, runtime_config,
    take_resolve_work, tx, verify_remote_transaction,
};
use ckb_network::PeerIndex;
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, FeeRate, TransactionBuilder, TransactionView},
    packed::{Byte32, OutPoint},
    prelude::Pack,
};
use std::sync::Arc;

const EFFECT_BYTES: usize = 1024 * 1024;

fn effect_limits(
    remote_batches: usize,
    trusted_headroom: usize,
    critical_headroom: usize,
    max_effects: usize,
) -> EffectLimits {
    EffectLimits::partitioned(
        EffectCapacity::new(remote_batches, EFFECT_BYTES),
        EffectCapacity::new(trusted_headroom, EFFECT_BYTES),
        EffectCapacity::new(critical_headroom, EFFECT_BYTES),
        EffectBatchBounds::new(
            EffectBatchBound::new(max_effects, EFFECT_BYTES),
            EffectBatchBound::new(max_effects, EFFECT_BYTES * 2),
            EffectBatchBound::new(max_effects, EFFECT_BYTES * 3),
        ),
    )
    .expect("fixture effect regions admit every indivisible batch")
}

fn authority_with_effect_limits(effect_limits: EffectLimits) -> TxPoolAuthority {
    TxPoolAuthority::for_foundation_with_effect_limits(limits(), effect_limits)
        .expect("fixture effect storage reserves its bounded queue")
}

fn rejected_publication(
    authority: &TxPoolAuthority,
    policy: EffectPolicy,
    transaction: Arc<TransactionView>,
) -> EffectPublication {
    authority
        .effect_publication_for_foundation(
            policy,
            vec![CommittedEffect::Rejected(
                CommittedRejection::for_foundation(
                    transaction,
                    RejectionAudience::foundation(),
                    RejectionKind::Policy,
                ),
            )],
        )
        .expect("fixture effect is bounded")
}

fn accepted_publication(
    authority: &TxPoolAuthority,
    policy: EffectPolicy,
    transaction: Arc<TransactionView>,
) -> EffectPublication {
    authority
        .effect_publication_for_foundation(
            policy,
            vec![CommittedEffect::Accepted(CommittedAcceptance::Duplicate {
                tx_hash: RawTxHash(transaction.hash()),
                requesting_peer: None,
            })],
        )
        .expect("fixture effect is bounded")
}

fn apply_plan(commit: impl FixtureCommit) -> CommittedDelta {
    commit.into_committed()
}

fn publish(authority: &mut TxPoolAuthority, publication: &EffectPublication) -> CommittedDelta {
    let plan = authority
        .plan_effect_publication_for_foundation(publication)
        .expect("fixture publication fits");
    apply_plan(plan)
}

fn publication_receipt(authority: &mut TxPoolAuthority) -> EffectReceipt {
    authority
        .effect_publication_receipt_for_foundation()
        .expect("one effect is pending")
}

#[tokio::test]
async fn uak_pending_recent_reject_is_an_exact_sequence_derived_projection() {
    let snapshot = genesis_snapshot();
    let runtime = AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        std::sync::Arc::clone(&snapshot),
    )
    .expect("the production runtime fixture is valid");
    let transaction = Arc::new(tx(698));
    let hash = transaction.hash();

    runtime
        .queue_effect_for_foundation(
            EffectPolicy::Remote,
            CommittedEffect::Rejected(CommittedRejection::for_foundation(
                Arc::clone(&transaction),
                RejectionAudience::foundation(),
                RejectionKind::Verification,
            )),
        )
        .expect("the first bounded rejection commits");
    let first = runtime
        .pending_recent_reject(&hash)
        .expect("the projection is structurally valid")
        .expect("a committed rejection is visible before persistence");
    assert_eq!(
        runtime
            .effect_observation_for_foundation()
            .pending_recent_rejects,
        1
    );

    runtime
        .queue_effect_for_foundation(
            EffectPolicy::Remote,
            CommittedEffect::Rejected(CommittedRejection::for_foundation(
                Arc::clone(&transaction),
                RejectionAudience::foundation(),
                RejectionKind::Policy,
            )),
        )
        .expect("a newer rejection for the same raw hash commits");
    let second = runtime
        .pending_recent_reject(&hash)
        .expect("the newer projection is structurally valid")
        .expect("the newer rejection replaces the read projection");
    assert_ne!(first, second);
    assert_eq!(
        runtime
            .effect_observation_for_foundation()
            .pending_recent_rejects,
        1,
        "one raw hash owns one latest pending projection"
    );

    let first_lease = runtime
        .wait_effect_publication_for_foundation()
        .await
        .expect("the effect log remains open");
    runtime
        .settle_effect_for_foundation(first_lease.complete_for_foundation())
        .expect("the older effect settles");
    assert_eq!(
        runtime
            .pending_recent_reject(&hash)
            .expect("older completion preserves a valid projection"),
        Some(second.clone()),
        "settling an older sequence must not erase the newer result"
    );

    let second_lease = runtime
        .wait_effect_publication_for_foundation()
        .await
        .expect("the effect log remains open");
    runtime
        .settle_effect_for_foundation(second_lease.complete_for_foundation())
        .expect("the latest effect settles");
    assert_eq!(
        runtime
            .pending_recent_reject(&hash)
            .expect("the empty projection remains valid"),
        None
    );

    runtime
        .queue_effect_for_foundation(
            EffectPolicy::Remote,
            CommittedEffect::Rejected(CommittedRejection::Membership {
                tx: transaction,
                audience: RejectionAudience::foundation(),
                reason: MembershipReject::CandidateEvicted {
                    fee_rate: FeeRate::from_u64(1_000),
                },
            }),
        )
        .expect("transient Full publication still commits its other endpoints");
    assert_eq!(
        runtime
            .pending_recent_reject(&hash)
            .expect("a non-recordable result cannot corrupt the projection"),
        None,
        "transient Full outcomes must not poison recent-reject reads"
    );
}

#[test]
fn uak_pending_recent_reject_uses_effect_position_within_one_batch() {
    let mut authority = authority_with_effect_limits(effect_limits(2, 1, 1, 2));
    let transaction = Arc::new(tx(699));
    let hash = RawTxHash(transaction.hash());

    let publication = authority
        .effect_publication_for_foundation(
            EffectPolicy::Remote,
            vec![
                CommittedEffect::Rejected(CommittedRejection::for_foundation(
                    Arc::clone(&transaction),
                    RejectionAudience::foundation(),
                    RejectionKind::Verification,
                )),
                CommittedEffect::Rejected(CommittedRejection::for_foundation(
                    transaction,
                    RejectionAudience::foundation(),
                    RejectionKind::Policy,
                )),
            ],
        )
        .expect("two bounded outcomes may share one raw hash and one batch");
    drop(publish(&mut authority, &publication));
    assert!(authority.primary_projection_consistent());
    let pending = authority
        .pending_recent_reject(&hash)
        .expect("the later effect position owns the projection")
        .public_reject()
        .expect("the projection is structurally valid");
    assert_eq!(pending, RejectionKind::Policy.into());

    let lease = publication_receipt(&mut authority);
    drop(apply_plan(
        authority
            .apply_effect_settlement_for_foundation(lease.complete_for_foundation())
            .expect("the complete batch settles"),
    ));
    assert!(authority.pending_recent_reject(&hash).is_none());
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_effect_configuration_and_publication_are_authority_bounded() {
    assert_eq!(
        EffectLimits::partitioned(
            EffectCapacity::new(0, EFFECT_BYTES),
            EffectCapacity::new(1, EFFECT_BYTES),
            EffectCapacity::new(1, EFFECT_BYTES),
            EffectBatchBounds::new(
                EffectBatchBound::new(1, EFFECT_BYTES),
                EffectBatchBound::new(1, EFFECT_BYTES),
                EffectBatchBound::new(1, EFFECT_BYTES),
            ),
        ),
        Err(EffectConfigError::EmptyRemoteRegion)
    );
    assert_eq!(
        EffectLimits::partitioned(
            EffectCapacity::new(1, 64),
            EffectCapacity::new(0, 0),
            EffectCapacity::new(0, 0),
            EffectBatchBounds::new(
                EffectBatchBound::new(1, 65),
                EffectBatchBound::new(1, 64),
                EffectBatchBound::new(1, 64),
            ),
        ),
        Err(EffectConfigError::IndivisibleBatch)
    );

    let broad = authority_with_effect_limits(effect_limits(2, 1, 1, 2));
    let oversized = broad
        .effect_publication_for_foundation(
            EffectPolicy::Remote,
            vec![
                CommittedEffect::Rejected(CommittedRejection::for_foundation(
                    Arc::new(tx(700)),
                    RejectionAudience::foundation(),
                    RejectionKind::Policy,
                )),
                CommittedEffect::Rejected(CommittedRejection::for_foundation(
                    Arc::new(tx(701)),
                    RejectionAudience::foundation(),
                    RejectionKind::Policy,
                )),
            ],
        )
        .expect("broad authority admits two effects");
    let mut narrow = authority_with_effect_limits(effect_limits(1, 1, 1, 1));
    let before = narrow.normalized_snapshot();
    assert_eq!(
        narrow
            .plan_effect_publication_for_foundation(&oversized)
            .err(),
        Some(PlanError::Fault(AuthorityFault::EffectProjection))
    );
    assert_eq!(narrow.normalized_snapshot(), before);
    assert!(narrow.primary_projection_consistent());

    assert!(matches!(
        narrow.effect_publication_for_foundation(EffectPolicy::Remote, Vec::new()),
        Err(EffectBuildError::Empty)
    ));
}

#[test]
fn uak_effect_shape_bounds_are_class_specific() {
    let limits = EffectLimits::partitioned(
        EffectCapacity::new(1, EFFECT_BYTES),
        EffectCapacity::new(1, EFFECT_BYTES),
        EffectCapacity::new(1, EFFECT_BYTES),
        EffectBatchBounds::new(
            EffectBatchBound::new(1, EFFECT_BYTES),
            EffectBatchBound::new(1, EFFECT_BYTES),
            EffectBatchBound::new(3, EFFECT_BYTES),
        ),
    )
    .expect("each region proves its own indivisible batch shape");
    let authority = authority_with_effect_limits(limits);
    let effects = vec![
        CommittedEffect::RemoteIngressReleased(
            CommittedRemoteIngressRelease::unretained_remote_submission(
                RawTxHash(Byte32::new([31; 32])),
                PeerIndex::from(31),
            ),
        ),
        CommittedEffect::RemoteIngressReleased(
            CommittedRemoteIngressRelease::unretained_remote_submission(
                RawTxHash(Byte32::new([32; 32])),
                PeerIndex::from(32),
            ),
        ),
    ];

    assert_eq!(
        authority
            .effect_publication_for_foundation(EffectPolicy::Trusted, effects.clone())
            .expect_err("trusted admission remains bounded to one outcome"),
        EffectBuildError::TooMany
    );
    authority
        .effect_publication_for_foundation(EffectPolicy::CriticalDetail, effects)
        .expect("critical detail may retain the larger proven all-owner shape");
}

fn sized_transaction(payload_bytes: usize) -> Arc<TransactionView> {
    Arc::new(
        TransactionBuilder::default()
            .witness(Bytes::from(vec![0x5a; payload_bytes]).pack())
            .build(),
    )
}

fn committed_entry(transaction: Arc<TransactionView>) -> CommittedEntrySnapshot {
    let size = transaction.data().total_size();
    CommittedEntrySnapshot {
        tx: transaction,
        cycles: u64::MAX,
        size,
        fee: Capacity::shannons(u64::MAX),
        ancestors_size: size,
        ancestors_fee: Capacity::shannons(u64::MAX),
        ancestors_cycles: u64::MAX,
        ancestors_count: crate::constants::MAX_POOL_MUTATION_CANDIDATES,
        descendants_fee: Capacity::shannons(u64::MAX),
        descendants_size: size,
        descendants_cycles: u64::MAX,
        descendants_count: crate::constants::MAX_POOL_MUTATION_CANDIDATES,
        timestamp: u64::MAX,
    }
}

/// Keep the sizing proof closed over the effect algebra. Adding a new effect
/// or rejection shape must update this classification and the constructive
/// bound tests below before the authority journal can compile its tests.
fn effect_sizing_family(effect: &CommittedEffect) -> &'static str {
    match effect {
        CommittedEffect::Accepted(acceptance) => match acceptance {
            CommittedAcceptance::Admission { .. } => "trusted-admission",
            CommittedAcceptance::Duplicate { .. } => "single-envelope",
            CommittedAcceptance::ChainStatusChange { .. } => "chain-rebuildable",
        },
        CommittedEffect::Rejected(rejection) => match rejection {
            CommittedRejection::Validation { .. } => "bounded-validation",
            CommittedRejection::Membership { .. } => "bounded-membership",
            CommittedRejection::Replaced { .. } => "trusted-admission",
            CommittedRejection::CapacityEvicted { .. } => "trusted-admission",
            CommittedRejection::Expired { .. } => "critical-detail",
            CommittedRejection::ChainConflict { .. } => "chain-rebuildable",
        },
        CommittedEffect::ChainCommitted { .. } => "chain-rebuildable",
        CommittedEffect::PeerCohortRevoked(_) => "critical-detail",
        CommittedEffect::RemoteExpired { .. } => "remote-prefix",
        CommittedEffect::RemoteIngressReleased(_) => "single-envelope",
        CommittedEffect::ParentTransactionsRequested(_) => "parent-request",
        CommittedEffect::GenerationReset => "reserved-reset",
    }
}

#[test]
fn uak_production_effect_sizing_constructively_covers_trusted_rbf_shape() {
    let transaction = sized_transaction(16 * 1024);
    let transaction_bytes = transaction.data().total_size();
    let victims = crate::constants::MAX_POOL_MUTATION_CANDIDATES;
    assert!(
        crate::constants::MAX_READY_BATCH <= victims + 1,
        "the independent settlement batch must remain a strict subset of the trusted shape proof"
    );
    let pool_bytes = victims
        .checked_mul(transaction_bytes)
        .expect("the bounded test shape fits usize");
    let limits = EffectLimits::production(pool_bytes, pool_bytes, transaction_bytes, 1)
        .expect("the production formula accepts the bounded pool shape");
    let entry = committed_entry(Arc::clone(&transaction));
    let winner = RawTxHash(Byte32::new([0xff; 32]));
    let mut effects = Vec::with_capacity(victims + 1);
    effects.push(CommittedEffect::Accepted(CommittedAcceptance::Admission {
        entry: entry.clone(),
        status: AcceptedStatus::Pending,
        ingress_peer: None,
    }));
    effects.extend((0..victims).map(|_| {
        CommittedEffect::Rejected(CommittedRejection::Replaced {
            entry: entry.clone(),
            winner: winner.clone(),
        })
    }));
    assert!(
        effects
            .iter()
            .all(|effect| { matches!(effect_sizing_family(effect), "trusted-admission") })
    );

    let publication = EffectPublication::new_for_foundation(EffectPolicy::Trusted, effects, limits)
        .expect("one winner plus the maximum victim closure fits by construction");
    assert!(
        publication.charge_bytes_for_foundation()
            <= limits.max_batch_bytes_for_foundation(EffectPolicy::Trusted)
    );
}

#[test]
fn uak_production_effect_sizing_constructively_covers_non_rebuildable_shapes() {
    let transaction = sized_transaction(16 * 1024);
    let transaction_bytes = transaction.data().total_size();
    let parent_count = 512;
    let limits = EffectLimits::production(
        transaction_bytes,
        transaction_bytes,
        transaction_bytes,
        parent_count,
    )
    .expect("the production formula accepts every single-effect shape");
    let validation = CommittedEffect::Rejected(CommittedRejection::Validation {
        tx: Arc::clone(&transaction),
        audience: RejectionAudience::foundation(),
        reason: CommittedPublicReject::new(ckb_types::core::tx_pool::Reject::Malformed(
            "transaction".to_owned(),
            "x".repeat(crate::constants::MAX_TX_POOL_REJECT_DESCRIPTION_BYTES * 2),
        )),
    });
    assert_eq!(effect_sizing_family(&validation), "bounded-validation");
    let remote =
        EffectPublication::new_for_foundation(EffectPolicy::Remote, vec![validation], limits)
            .expect("a maximal bounded validation rejection fits Remote shape bounds");
    assert!(
        remote.charge_bytes_for_foundation()
            <= limits.max_batch_bytes_for_foundation(EffectPolicy::Remote)
    );

    let parents = Arc::new(
        (0..parent_count)
            .map(|index| RawTxHash(Byte32::new([(index % 251) as u8; 32])))
            .collect::<Vec<_>>(),
    );
    let request = CommittedEffect::ParentTransactionsRequested(
        ParentTransactionRequest::new(PeerIndex::from(1), parents)
            .expect("the maximal parent request is non-empty"),
    );
    assert_eq!(effect_sizing_family(&request), "parent-request");
    let parent_publication =
        EffectPublication::new_for_foundation(EffectPolicy::Remote, vec![request], limits)
            .expect("the maximal parent frontier fits its exact production bound");
    assert!(
        parent_publication.charge_bytes_for_foundation()
            <= limits.max_batch_bytes_for_foundation(EffectPolicy::Remote)
    );

    let ban = CommittedEffect::PeerCohortRevoked(
        super::super::effect::CommittedPeerCohortRevocation::malformed_for_foundation(
            PeerIndex::from(2),
            RawTxHash(transaction.hash()),
            CommittedPublicReject::new(ckb_types::core::tx_pool::Reject::Malformed(
                "transaction".to_owned(),
                "x".repeat(crate::constants::MAX_TX_POOL_REJECT_DESCRIPTION_BYTES * 2),
            )),
        )
        .expect("a malformed rejection carries bounded ban evidence"),
    );
    assert_eq!(effect_sizing_family(&ban), "critical-detail");
    EffectPublication::new_for_foundation(EffectPolicy::CriticalDetail, vec![ban], limits)
        .expect("one bounded peer revocation fits non-rebuildable critical detail");

    // Exercise the remaining production rejection family in the exhaustive
    // classifier. Chain conflict/status/commit batches are intentionally
    // guarded by CriticalRebuildable and collapse to GenerationReset.
    let chain = CommittedEffect::Rejected(CommittedRejection::ChainConflict {
        owner: CommittedConflictOwner::PreAccepted {
            tx: transaction,
            audience: RejectionAudience::foundation(),
        },
        out_point: OutPoint::new(Byte32::new([3; 32]), u32::MAX),
    });
    assert_eq!(effect_sizing_family(&chain), "chain-rebuildable");
}

#[test]
fn uak_remote_missing_wait_and_parent_request_share_one_backpressured_apply() {
    let mut authority = authority_with_effect_limits(effect_limits(1, 1, 1, 1));
    let occupied = rejected_publication(&authority, EffectPolicy::Remote, Arc::new(tx(714)));
    drop(publish(&mut authority, &occupied));

    let peer = PeerIndex::from(74);
    let hash = admit_remote(&mut authority, 715, 74);
    let version = owner_version(&authority, &hash);
    let (_, work) = take_resolve_work(
        authority
            .checkout_for_foundation(&hash, version, WorkPermit::ResolveOnly)
            .expect("remote missing work checks out"),
    );
    let first_parent = Byte32::new([0x11; 32]);
    let second_parent = Byte32::new([0x22; 32]);
    let settlement = work
        .missing(vec![
            DependencyKey::Cell(OutPoint::new(second_parent.clone(), 0)),
            DependencyKey::Cell(OutPoint::new(first_parent.clone(), 1)),
            DependencyKey::Cell(OutPoint::new(first_parent.clone(), 0)),
            // The relayer requests transactions only. Production resolution
            // rejects an invalid header before this receipt is constructed.
            DependencyKey::Header(Byte32::new([0x33; 32])),
        ])
        .expect("the complete missing frontier fits the compute grant");
    let before = authority.normalized_snapshot();

    let blocked = authority
        .apply_settlement(settlement)
        .expect_err("the wait cannot commit without its parent request");
    assert_eq!(
        blocked.recovery(),
        &ComputeSettlementRecovery::WaitEffectCapacity
    );
    assert_eq!(authority.normalized_snapshot(), before);
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Computing(_))
                && entry.charge.active_work == 1
    ));

    let blocked = authority
        .apply_settlement(blocked.into_settlement())
        .expect_err("an unchanged full journal retains the exact missing result");
    assert_eq!(
        blocked.recovery(),
        &ComputeSettlementRecovery::WaitEffectCapacity
    );
    assert_eq!(authority.normalized_snapshot(), before);

    let occupied_lease = publication_receipt(&mut authority);
    drop(apply_plan(
        authority
            .apply_effect_settlement_for_foundation(occupied_lease.complete_for_foundation())
            .expect("the occupied publication settles"),
    ));
    drop(apply_plan(
        authority
            .apply_settlement(blocked.into_settlement())
            .expect("the same missing result commits after capacity returns"),
    ));
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Waiting(_))
                && entry.charge.active_work == 0
    ));

    let request = publication_receipt(&mut authority);
    let [CommittedEffect::ParentTransactionsRequested(request)] = request.effects() else {
        panic!("the wait commits exactly one parent-transaction request");
    };
    assert_eq!(request.peer(), peer);
    assert_eq!(
        request.parents(),
        &[RawTxHash(first_parent), RawTxHash(second_parent),]
    );
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_overtaken_effect_sequence_keeps_a_current_compute_settlement_retryable() {
    let mut authority = authority_with_effect_limits(effect_limits(1, 1, 1, 1));
    let hash = admit_remote(&mut authority, 716, 76);
    let version = owner_version(&authority, &hash);
    let (_, work) = take_resolve_work(
        authority
            .checkout_for_foundation(&hash, version, WorkPermit::ResolveOnly)
            .expect("the compute fixture checks out"),
    );
    let failure =
        authority.classify_overtaken_effect_settlement_for_foundation(work.internal_failure());
    assert!(matches!(
        failure.recovery(),
        ComputeSettlementRecovery::RetryExact(_)
    ));
}

#[test]
fn uak_effect_full_preserves_ready_owner_and_charge() {
    let mut authority = authority_with_effect_limits(effect_limits(1, 1, 1, 1));
    let occupied = rejected_publication(&authority, EffectPolicy::Remote, Arc::new(tx(720)));
    drop(publish(&mut authority, &occupied));

    let transaction = tx(721);
    let hash = verify_remote_transaction(&mut authority, transaction, 72, Vec::new());
    let version = owner_version(&authority, &hash);
    let retained = Arc::clone(&authority.entry(&hash).expect("owner exists").record().tx);
    let blocked = rejected_publication(&authority, EffectPolicy::Remote, retained);
    let before = authority.normalized_snapshot();

    assert_eq!(
        authority
            .plan_terminalize_with_effect_for_foundation(&hash, version, &blocked)
            .err(),
        Some(PlanError::Backpressure(Backpressure::EffectCapacity))
    );
    assert_eq!(authority.normalized_snapshot(), before);
    assert!(matches!(
        authority.entry(&hash),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Ready(_))
    ));
    assert_eq!(authority.owner_count(), 1);
    assert_eq!(authority.charged_count(), 1);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_effect_receipt_preserves_sequence_and_charge_without_an_apply_clock() {
    let mut authority = authority_with_effect_limits(effect_limits(2, 1, 1, 1));
    let publication = rejected_publication(&authority, EffectPolicy::Remote, Arc::new(tx(730)));
    drop(publish(&mut authority, &publication));
    let queued = authority.effect_observation_for_foundation();
    let expected_sequence = queued.queued[0];
    let expected_charge = queued.total_usage;

    let before_dropped_plan = authority.normalized_snapshot();
    let dropped_receipt = authority
        .effect_publication_receipt_for_foundation()
        .expect("one effect is pending");
    drop(dropped_receipt);
    assert_eq!(authority.normalized_snapshot(), before_dropped_plan);

    let lease = publication_receipt(&mut authority);
    assert_eq!(lease.sequence(), expected_sequence);
    assert_eq!(lease.charge_bytes(), expected_charge.bytes);
    let borrowed = authority.effect_observation_for_foundation();
    assert_eq!(borrowed.queued, vec![expected_sequence]);
    assert_eq!(borrowed.queued_processed_steps, vec![0]);
    assert_eq!(borrowed.total_usage, expected_charge);

    let mut unrelated_authority = authority_with_effect_limits(effect_limits(2, 1, 1, 1));
    let unrelated_publication = rejected_publication(
        &unrelated_authority,
        EffectPolicy::Remote,
        Arc::new(tx(732)),
    );
    drop(publish(&mut unrelated_authority, &unrelated_publication));
    let unrelated_lease = publication_receipt(&mut unrelated_authority);
    assert_eq!(unrelated_lease.sequence(), expected_sequence);
    let before_stale = authority.normalized_snapshot();
    let stale = authority
        .apply_effect_settlement_for_foundation(unrelated_lease)
        .expect_err("an unrelated effect receipt is stale");
    assert_eq!(stale.error(), EffectSettlementError::StaleLease);
    assert_eq!(authority.normalized_snapshot(), before_stale);

    let resumable_sequence = authority.clocks().next_sequence;
    drop(lease);
    authority.force_next_sequence(ApplySequence(u128::MAX));
    let lease = publication_receipt(&mut authority);
    assert_eq!(lease.sequence(), expected_sequence);
    let before_exhaustion = authority.normalized_snapshot();
    let retained = apply_plan(
        authority
            .apply_effect_settlement_for_foundation(lease)
            .expect("journal-local settlement does not reserve an Apply clock"),
    );
    assert_eq!(retained.retired_effect_len(), 0);
    assert_eq!(authority.normalized_snapshot(), before_exhaustion);
    assert_eq!(authority.clocks().next_sequence, ApplySequence(u128::MAX));
    authority.force_next_sequence(resumable_sequence);
    let requeued = authority.effect_observation_for_foundation();
    assert_eq!(requeued.queued, vec![expected_sequence]);
    assert_eq!(requeued.queued_processed_steps, vec![0]);
    assert_eq!(requeued.total_usage, expected_charge);

    let lease = publication_receipt(&mut authority);
    let published = apply_plan(
        authority
            .apply_effect_settlement_for_foundation(lease.complete_for_foundation())
            .expect("the exact receipt publishes"),
    );
    assert_eq!(published.retired_effect_len(), 1);
    let empty = authority.effect_observation_for_foundation();
    assert!(empty.queued.is_empty());
    assert!(empty.queued_processed_steps.is_empty());
    assert_eq!(empty.total_usage.batches, 0);
    assert_eq!(empty.total_usage.bytes, 0);
    drop(published);

    let accepted = accepted_publication(&authority, EffectPolicy::Trusted, Arc::new(tx(731)));
    drop(publish(&mut authority, &accepted));
    let lease = publication_receipt(&mut authority);
    let disposed = apply_plan(
        authority
            .apply_effect_settlement_for_foundation(lease.complete_for_foundation())
            .expect("the endpoint circuit can dispose committed detail"),
    );
    assert_eq!(disposed.retired_effect_len(), 1);
    drop(disposed);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_effect_settlement_rejects_forged_source_sequence_and_cursor_without_mutation() {
    let mut authority = authority_with_effect_limits(effect_limits(2, 1, 1, 1));
    let publication = rejected_publication(&authority, EffectPolicy::Remote, Arc::new(tx(735)));
    drop(publish(&mut authority, &publication));
    let exact_sequence = authority.effect_observation_for_foundation().queued[0];

    for forged in [
        publication_receipt(&mut authority).claim_generation_reset_source_for_foundation(),
        publication_receipt(&mut authority)
            .with_sequence_for_foundation(ApplySequence(exact_sequence.0 + 1)),
    ] {
        let before = authority.normalized_snapshot();
        let failure = authority
            .apply_effect_settlement_for_foundation(forged)
            .expect_err("forged receipt identity is stale");
        assert_eq!(failure.error(), EffectSettlementError::StaleLease);
        assert_eq!(authority.normalized_snapshot(), before);
        drop(failure);
    }

    let mut first = publication_receipt(&mut authority);
    first
        .mark_current_processed()
        .expect("the first endpoint advances tentatively");
    drop(apply_plan(
        authority
            .apply_effect_settlement_for_foundation(first)
            .expect("Retain commits the exact partial cursor"),
    ));
    assert_eq!(
        authority
            .effect_observation_for_foundation()
            .queued_processed_steps,
        vec![1]
    );

    for forged in [
        publication_receipt(&mut authority).with_processed_steps_for_foundation(0),
        publication_receipt(&mut authority).with_processed_steps_for_foundation(usize::MAX),
    ] {
        let before = authority.normalized_snapshot();
        let failure = authority
            .apply_effect_settlement_for_foundation(forged)
            .expect_err("a regressed or overrun cursor is a projection fault");
        assert_eq!(failure.error(), EffectSettlementError::Projection);
        assert_eq!(authority.normalized_snapshot(), before);
        drop(failure);
    }
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_effect_regions_are_cumulative_and_critical_full_resets() {
    let mut authority = authority_with_effect_limits(effect_limits(1, 1, 1, 1));
    let remote = rejected_publication(&authority, EffectPolicy::Remote, Arc::new(tx(740)));
    drop(publish(&mut authority, &remote));

    let second_remote = rejected_publication(&authority, EffectPolicy::Remote, Arc::new(tx(741)));
    assert_eq!(
        authority
            .plan_effect_publication_for_foundation(&second_remote)
            .err(),
        Some(PlanError::Backpressure(Backpressure::EffectCapacity))
    );

    let trusted = rejected_publication(&authority, EffectPolicy::Trusted, Arc::new(tx(742)));
    drop(publish(&mut authority, &trusted));
    let second_trusted = rejected_publication(&authority, EffectPolicy::Trusted, Arc::new(tx(743)));
    assert_eq!(
        authority
            .plan_effect_publication_for_foundation(&second_trusted)
            .err(),
        Some(PlanError::Backpressure(Backpressure::EffectCapacity))
    );

    let critical =
        rejected_publication(&authority, EffectPolicy::CriticalDetail, Arc::new(tx(744)));
    drop(publish(&mut authority, &critical));
    let essential =
        rejected_publication(&authority, EffectPolicy::CriticalDetail, Arc::new(tx(745)));
    assert_eq!(
        authority
            .plan_effect_publication_for_foundation(&essential)
            .err(),
        Some(PlanError::Backpressure(Backpressure::EffectCapacity))
    );
    assert_eq!(
        authority
            .effect_observation_for_foundation()
            .latest_generation_reset,
        None
    );
    let reset = rejected_publication(
        &authority,
        EffectPolicy::CriticalRebuildable,
        Arc::new(tx(746)),
    );
    let reset_commit = publish(&mut authority, &reset);
    assert_eq!(reset_commit.retired_effect_len(), 0);

    let observation = authority.effect_observation_for_foundation();
    assert_eq!(observation.remote_usage.batches, 1);
    assert_eq!(observation.ordinary_usage.batches, 2);
    assert_eq!(observation.total_usage.batches, 3);
    assert_eq!(observation.queued.len(), 3);
    assert!(observation.latest_generation_reset.is_some());
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_generation_reset_coalesces_and_retain_never_resurrects_an_old_reset() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let first = apply_plan(
        authority
            .plan_generation_reset_for_foundation()
            .expect("first generation reset plans"),
    );
    let first_sequence = authority
        .effect_observation_for_foundation()
        .latest_generation_reset
        .expect("the first reset is authoritative effect state");
    assert_eq!(first.retired_effect_len(), 0);

    let second = apply_plan(
        authority
            .plan_generation_reset_for_foundation()
            .expect("newer generation reset plans"),
    );
    let second_sequence = authority
        .effect_observation_for_foundation()
        .latest_generation_reset
        .expect("the second reset replaces the first");
    assert!(second_sequence > first_sequence);
    assert_eq!(second.retired_effect_len(), 1);
    assert_eq!(
        authority
            .effect_observation_for_foundation()
            .latest_generation_reset,
        Some(second_sequence)
    );

    let old_receipt = publication_receipt(&mut authority);
    assert_eq!(old_receipt.sequence(), second_sequence);
    let third = apply_plan(
        authority
            .plan_generation_reset_for_foundation()
            .expect("reset can advance while an older reset receipt is live"),
    );
    let third_sequence = authority
        .effect_observation_for_foundation()
        .latest_generation_reset
        .expect("the third reset remains authoritative while the old receipt is live");
    drop(third);
    assert!(third_sequence > second_sequence);
    let before_superseded = authority.normalized_snapshot();
    let superseded = authority
        .effect_settlement_for_foundation(old_receipt)
        .expect("a valid older reset receipt is subsumed");
    let (retired, _wake) = superseded.into_parts();
    assert!(retired.is_none());
    assert_eq!(authority.normalized_snapshot(), before_superseded);
    assert_eq!(
        authority
            .effect_observation_for_foundation()
            .latest_generation_reset,
        Some(third_sequence)
    );

    let newest = publication_receipt(&mut authority);
    assert_eq!(newest.sequence(), third_sequence);
    assert!(matches!(
        newest.effects(),
        [CommittedEffect::GenerationReset]
    ));
    drop(apply_plan(
        authority
            .apply_effect_settlement_for_foundation(newest.complete_for_foundation())
            .expect("latest reset publishes"),
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_effect_close_requires_every_compute_capability_to_settle() {
    let mut authority = authority_with_effect_limits(effect_limits(1, 1, 1, 1));
    let admission = ValidatedAdmission::remote(tx(752), PeerIndex::from(76))
        .expect("fixture admission is valid");
    let hash = admission.identity.raw.clone();
    drop(apply_plan(
        authority
            .plan_admission(admission)
            .expect("fixture admission plans"),
    ));
    let version = owner_version(&authority, &hash);
    let (_, work) = take_resolve_work(
        authority
            .checkout_for_foundation(&hash, version, WorkPermit::ResolveOnly)
            .expect("compute checkout plans"),
    );

    let before = authority.normalized_snapshot();
    assert_eq!(
        authority.close_effects_for_foundation().err(),
        Some(EffectCloseError::ActiveWork)
    );
    assert_eq!(authority.normalized_snapshot(), before);

    drop(apply_plan(
        authority
            .apply_settlement(work.rejected(RejectionKind::Policy))
            .expect("the unique live lease settles before close"),
    ));
    authority
        .close_effects_for_foundation()
        .expect("drained compute permits close");
    assert!(authority.effect_observation_for_foundation().closed);
    assert!(authority.primary_projection_consistent());
}
