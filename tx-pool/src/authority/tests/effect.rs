use super::super::{
    effect::{
        CommittedEffect, EffectBatchBounds, EffectBuildError, EffectCapacity, EffectConfigError,
        EffectLease, EffectLimits, EffectPolicy, EffectPublication,
    },
    plan::{
        AuthorityFault, Backpressure, CommittedChanges, CommittedDelta, PlanError, StalePlan,
        TxPoolAuthority,
    },
    state::{
        AcceptedStatus, ApplySequence, ComputedOutcome, OwnedTx, PreAcceptedPhase, RejectionKind,
        RemoteDeadline, ValidatedAdmission, WorkPermit,
    },
};
use super::foundation::{
    FixtureCommit, admit_remote, admit_remote_until, limits, owner_version, take_resolve_work, tx,
    verify_remote_transaction,
};
use ckb_network::PeerIndex;
use ckb_types::core::TransactionView;
use std::{num::NonZeroUsize, sync::Arc};

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
            max_effects,
            EFFECT_BYTES,
            EFFECT_BYTES * 2,
            EFFECT_BYTES * 3,
        ),
    )
    .expect("fixture effect regions admit every indivisible batch")
}

fn authority_with_effect_limits(effect_limits: EffectLimits) -> TxPoolAuthority {
    TxPoolAuthority::for_foundation_with_effect_limits(limits(), effect_limits)
        .expect("fixture effect storage reserves its bounded queue")
}

#[test]
fn uak_peer_revocation_over_detail_bound_commits_a_constant_reset() {
    let peer = PeerIndex::from(711);
    let mut authority = authority_with_effect_limits(effect_limits(8, 2, 2, 1));
    let first = admit_remote(&mut authority, 1_713, 711);
    let second = admit_remote(&mut authority, 1_714, 711);

    let committed = authority
        .plan_peer_revocation_for_foundation(peer)
        .expect("rebuildable cleanup cannot be blocked by its detail bound")
        .expect("peer owns a bounded cohort")
        .apply();
    assert_eq!(committed.retired_len(), 2);
    assert!(authority.entry(&first).is_none());
    assert!(authority.entry(&second).is_none());

    let lease = checkout(&mut authority);
    assert_eq!(lease.effects(), &[CommittedEffect::GenerationReset]);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_remote_expiry_effect_backpressure_is_zero_mutation() {
    let mut authority = authority_with_effect_limits(effect_limits(1, 1, 1, 1));
    let occupied = rejected_publication(&authority, EffectPolicy::Remote, Arc::new(tx(1_720)));
    drop(publish(&mut authority, &occupied));
    let due = admit_remote_until(&mut authority, 1_721, 717, 10);
    let before = authority.normalized_snapshot();

    assert_eq!(
        authority
            .plan_remote_expiry_for_foundation(
                RemoteDeadline(10),
                NonZeroUsize::new(1).expect("fixture slice is non-zero"),
            )
            .err(),
        Some(PlanError::Backpressure(Backpressure::EffectCapacity))
    );
    assert_eq!(authority.normalized_snapshot(), before);
    assert!(authority.entry(&due).is_some());
    assert!(authority.primary_projection_consistent());
}

fn rejected_publication(
    authority: &TxPoolAuthority,
    policy: EffectPolicy,
    transaction: Arc<TransactionView>,
) -> EffectPublication {
    authority
        .effect_publication_for_foundation(
            policy,
            vec![CommittedEffect::Rejected {
                tx: transaction,
                reason: RejectionKind::Policy,
            }],
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
            vec![CommittedEffect::Accepted {
                tx: transaction,
                status: AcceptedStatus::Pending,
            }],
        )
        .expect("fixture effect is bounded")
}

fn apply_without_handoff(commit: impl FixtureCommit) -> CommittedDelta {
    let committed = commit.into_committed();
    assert!(committed.handoff_is_none());
    committed
}

fn publish(authority: &mut TxPoolAuthority, publication: &EffectPublication) -> CommittedDelta {
    let plan = authority
        .plan_effect_publication_for_foundation(publication)
        .expect("fixture publication fits");
    apply_without_handoff(plan)
}

fn checkout(authority: &mut TxPoolAuthority) -> EffectLease {
    authority
        .plan_effect_checkout_for_foundation()
        .expect("effect checkout plans")
        .expect("one effect is pending")
        .apply()
        .into_effect_lease()
        .expect("effect checkout returns exactly one lease")
}

fn effect_control_sequence(committed: &CommittedDelta) -> ApplySequence {
    let CommittedChanges::EffectControl(sequence) = &committed.changes else {
        panic!("fixture expected an effect-only authority commit");
    };
    *sequence
}

#[test]
fn uak_effect_configuration_and_publication_are_authority_bounded() {
    assert_eq!(
        EffectLimits::partitioned(
            EffectCapacity::new(0, EFFECT_BYTES),
            EffectCapacity::new(1, EFFECT_BYTES),
            EffectCapacity::new(1, EFFECT_BYTES),
            EffectBatchBounds::new(1, EFFECT_BYTES, EFFECT_BYTES, EFFECT_BYTES),
        ),
        Err(EffectConfigError::EmptyRemoteRegion)
    );
    assert_eq!(
        EffectLimits::partitioned(
            EffectCapacity::new(1, 64),
            EffectCapacity::new(0, 0),
            EffectCapacity::new(0, 0),
            EffectBatchBounds::new(1, 65, 64, 64),
        ),
        Err(EffectConfigError::IndivisibleBatch)
    );

    let broad = authority_with_effect_limits(effect_limits(2, 1, 1, 2));
    let oversized = broad
        .effect_publication_for_foundation(
            EffectPolicy::Remote,
            vec![
                CommittedEffect::Rejected {
                    tx: Arc::new(tx(700)),
                    reason: RejectionKind::Policy,
                },
                CommittedEffect::Rejected {
                    tx: Arc::new(tx(701)),
                    reason: RejectionKind::Policy,
                },
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
fn uak_compute_outcome_survives_effect_backpressure() {
    let mut authority = authority_with_effect_limits(effect_limits(1, 1, 1, 1));
    let occupied = rejected_publication(&authority, EffectPolicy::Remote, Arc::new(tx(710)));
    drop(publish(&mut authority, &occupied));

    let transaction = tx(711);
    let hash = verify_remote_transaction(&mut authority, transaction, 71, Vec::new());
    let Some(OwnedTx::PreAccepted(entry)) = authority.entry(&hash) else {
        panic!("verified transaction retains one pre-accepted owner");
    };
    assert!(matches!(
        entry.phase,
        PreAcceptedPhase::Computed(ComputedOutcome::Verified(_))
    ));
    assert_eq!(authority.owner_count(), 1);
    assert_eq!(authority.charged_count(), 1);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_effect_full_preserves_computed_owner_and_charge() {
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
            if matches!(entry.phase, PreAcceptedPhase::Computed(ComputedOutcome::Verified(_)))
    ));
    assert_eq!(authority.owner_count(), 1);
    assert_eq!(authority.charged_count(), 1);
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_effect_lease_preserves_sequence_and_charge() {
    let mut authority = authority_with_effect_limits(effect_limits(2, 1, 1, 1));
    let publication = rejected_publication(&authority, EffectPolicy::Remote, Arc::new(tx(730)));
    drop(publish(&mut authority, &publication));
    let queued = authority.effect_observation_for_foundation();
    let expected_sequence = queued.queued[0];
    let expected_charge = queued.total_usage;

    let before_dropped_plan = authority.normalized_snapshot();
    let checkout_plan = authority
        .plan_effect_checkout_for_foundation()
        .expect("effect checkout plans")
        .expect("one effect is pending");
    drop(checkout_plan);
    assert_eq!(authority.normalized_snapshot(), before_dropped_plan);

    let lease = checkout(&mut authority);
    assert_eq!(lease.sequence(), expected_sequence);
    assert_eq!(lease.charge_bytes(), expected_charge.bytes);
    let active = authority.effect_observation_for_foundation();
    assert_eq!(active.active, Some(expected_sequence));
    assert_eq!(active.total_usage, expected_charge);

    let mut unrelated_authority = authority_with_effect_limits(effect_limits(2, 1, 1, 1));
    let unrelated_publication = rejected_publication(
        &unrelated_authority,
        EffectPolicy::Remote,
        Arc::new(tx(732)),
    );
    drop(publish(&mut unrelated_authority, &unrelated_publication));
    let unrelated_lease = checkout(&mut unrelated_authority);
    assert_eq!(unrelated_lease.sequence(), expected_sequence);
    let before_stale = authority.normalized_snapshot();
    let stale = authority
        .apply_effect_settlement_for_foundation(unrelated_lease.retain())
        .expect_err("an unrelated effect lease is stale");
    assert_eq!(stale.error(), &PlanError::Stale(StalePlan::EffectLease));
    assert_eq!(authority.normalized_snapshot(), before_stale);

    let resumable_sequence = authority.clocks().next_sequence;
    authority.force_next_sequence(ApplySequence(u128::MAX));
    let before_exhaustion = authority.normalized_snapshot();
    let exhausted = authority
        .apply_effect_settlement_for_foundation(lease.retain())
        .expect_err("counter exhaustion cannot consume the publisher capability");
    assert_eq!(
        exhausted.error(),
        &PlanError::Fault(AuthorityFault::CounterExhausted)
    );
    assert_eq!(authority.normalized_snapshot(), before_exhaustion);
    authority.force_next_sequence(resumable_sequence);

    let retained = apply_without_handoff(
        authority
            .apply_effect_settlement_for_foundation(exhausted.into_settlement())
            .expect("the exact lease can be retained"),
    );
    assert_eq!(retained.retired_effect_len(), 0);
    let requeued = authority.effect_observation_for_foundation();
    assert_eq!(requeued.queued, vec![expected_sequence]);
    assert_eq!(requeued.active, None);
    assert_eq!(requeued.total_usage, expected_charge);

    let lease = checkout(&mut authority);
    let published = apply_without_handoff(
        authority
            .apply_effect_settlement_for_foundation(lease.published())
            .expect("the exact lease publishes"),
    );
    assert_eq!(published.retired_effect_len(), 1);
    let empty = authority.effect_observation_for_foundation();
    assert!(empty.queued.is_empty());
    assert_eq!(empty.active, None);
    assert_eq!(empty.total_usage.batches, 0);
    assert_eq!(empty.total_usage.bytes, 0);
    drop(published);

    let accepted = accepted_publication(&authority, EffectPolicy::Trusted, Arc::new(tx(731)));
    drop(publish(&mut authority, &accepted));
    let lease = checkout(&mut authority);
    let disposed = apply_without_handoff(
        authority
            .apply_effect_settlement_for_foundation(lease.circuit_disposed())
            .expect("the endpoint circuit can dispose committed detail"),
    );
    assert_eq!(disposed.retired_effect_len(), 1);
    drop(disposed);
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
    let first = apply_without_handoff(
        authority
            .plan_generation_reset_for_foundation()
            .expect("first generation reset plans"),
    );
    let first_sequence = effect_control_sequence(&first);
    assert_eq!(first.retired_effect_len(), 0);

    let second = apply_without_handoff(
        authority
            .plan_generation_reset_for_foundation()
            .expect("newer generation reset plans"),
    );
    let second_sequence = effect_control_sequence(&second);
    assert!(second_sequence > first_sequence);
    assert_eq!(second.retired_effect_len(), 1);
    assert_eq!(
        authority
            .effect_observation_for_foundation()
            .latest_generation_reset,
        Some(second_sequence)
    );

    let old_active = checkout(&mut authority);
    assert_eq!(old_active.sequence(), second_sequence);
    let third = apply_without_handoff(
        authority
            .plan_generation_reset_for_foundation()
            .expect("reset can advance while an older reset is active"),
    );
    let third_sequence = effect_control_sequence(&third);
    assert!(third_sequence > second_sequence);
    let retained = apply_without_handoff(
        authority
            .apply_effect_settlement_for_foundation(old_active.retain())
            .expect("old reset lease settles"),
    );
    assert_eq!(retained.retired_effect_len(), 1);
    assert_eq!(
        authority
            .effect_observation_for_foundation()
            .latest_generation_reset,
        Some(third_sequence)
    );

    let newest = checkout(&mut authority);
    assert_eq!(newest.sequence(), third_sequence);
    assert!(matches!(
        newest.effects(),
        [CommittedEffect::GenerationReset]
    ));
    drop(apply_without_handoff(
        authority
            .apply_effect_settlement_for_foundation(newest.published())
            .expect("latest reset publishes"),
    ));
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_closed_authority_freezes_new_state_and_drains_committed_effects() {
    let mut authority = authority_with_effect_limits(effect_limits(2, 1, 1, 1));
    let publication = rejected_publication(&authority, EffectPolicy::Remote, Arc::new(tx(750)));
    drop(publish(&mut authority, &publication));
    drop(apply_without_handoff(
        authority
            .plan_generation_reset_for_foundation()
            .expect("generation reset plans"),
    ));
    drop(apply_without_handoff(
        authority
            .plan_effect_close_for_foundation()
            .expect("authority close plans"),
    ));
    assert!(authority.effect_observation_for_foundation().closed);

    let before_rejected_admission = authority.normalized_snapshot();
    let admission = ValidatedAdmission::remote(tx(751), PeerIndex::from(75))
        .expect("fixture admission is valid");
    assert_eq!(
        authority.plan_admission(admission).err(),
        Some(PlanError::EffectClosed)
    );
    assert_eq!(authority.normalized_snapshot(), before_rejected_admission);
    assert_eq!(
        authority
            .plan_effect_publication_for_foundation(&publication)
            .err(),
        Some(PlanError::EffectClosed)
    );
    assert_eq!(
        authority.plan_effect_close_for_foundation().err(),
        Some(PlanError::EffectClosed)
    );

    for _ in 0..2 {
        let lease = checkout(&mut authority);
        drop(apply_without_handoff(
            authority
                .apply_effect_settlement_for_foundation(lease.published())
                .expect("already committed effects drain after close"),
        ));
    }
    assert!(authority.effects_closed_and_drained_for_foundation());
    assert!(authority.primary_projection_consistent());
}

#[test]
fn uak_effect_close_requires_every_compute_capability_to_settle() {
    let mut authority = authority_with_effect_limits(effect_limits(1, 1, 1, 1));
    let admission = ValidatedAdmission::remote(tx(752), PeerIndex::from(76))
        .expect("fixture admission is valid");
    let hash = admission.identity.raw.clone();
    drop(apply_without_handoff(
        authority
            .plan_admission(admission)
            .expect("fixture admission plans"),
    ));
    let version = owner_version(&authority, &hash);
    let (_, work) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(&hash, version, WorkPermit::ResolveOnly)
            .expect("compute checkout plans")
            .apply(),
    );

    let before = authority.normalized_snapshot();
    assert_eq!(
        authority.plan_effect_close_for_foundation().err(),
        Some(PlanError::Backpressure(Backpressure::ActiveWorkDrain))
    );
    assert_eq!(authority.normalized_snapshot(), before);

    drop(apply_without_handoff(
        authority
            .apply_settlement(work.rejected(RejectionKind::Policy))
            .expect("the unique live lease settles before close"),
    ));
    drop(apply_without_handoff(
        authority
            .plan_effect_close_for_foundation()
            .expect("drained compute permits close"),
    ));
    assert!(authority.effect_observation_for_foundation().closed);
    assert!(authority.primary_projection_consistent());
}
