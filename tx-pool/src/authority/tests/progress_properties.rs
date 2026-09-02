//! Finite production refinement for effect publication and post-commit wake.
//!
//! Effect expectations are derived from a direct stored-log observation. Wake
//! expectations use the independent executable relation over the exact
//! before/after values carried by production.

use super::claim_relations::progress::{
    AuthorityProgressCut, EffectHead, EffectLogCut,
    EffectPublicationObservation as ClaimEffectPublicationObservation,
    EffectReceiptSource as ClaimEffectReceiptSource, EffectUsageCut, ProgressVersion,
    SchedulerProgressCut, WakeObservation as ClaimWakeObservation,
};
use super::foundation::{apply_plan, limits, tx};
use crate::authority::{
    effect::{
        CommittedEffect, CommittedRejection, EffectPolicy, RejectionAudience,
        test_support::{
            EffectPublicationObservationSnapshot, EffectPublisherLevelInput,
            EffectReceiptSourceObservation, EffectWakeProjectionInput,
        },
    },
    plan::{
        AuthorityWakeTransition, TxPoolAuthority,
        test_support::{WakeObservation as ProductionWakeObservation, WakeProjectionInput},
    },
    state::{ApplySequence, test_support::RejectionKind},
};
use std::sync::Arc;

fn wake_input() -> WakeProjectionInput {
    WakeProjectionInput {
        scheduler: [None; 4],
        active_work: 0,
        dependency_maintenance: false,
        effects: EffectWakeProjectionInput::default(),
        template_sources: [0; 3],
    }
}

fn claim_cut(input: WakeProjectionInput) -> AuthorityProgressCut {
    let [resolve, verify_small, verify_any, ready] = input.scheduler;
    let publication = match input.effects.publisher {
        EffectPublisherLevelInput::Idle => EffectLogCut::default(),
        EffectPublisherLevelInput::Available => EffectLogCut {
            queued: Some(EffectHead {
                sequence: 1,
                ordinal: 0,
            }),
            ..EffectLogCut::default()
        },
        EffectPublisherLevelInput::ClosedAndDrained => EffectLogCut {
            closed: true,
            ..EffectLogCut::default()
        },
    };
    let [
        remote_batches,
        remote_bytes,
        ordinary_batches,
        ordinary_bytes,
        total_batches,
        total_bytes,
    ] = input.effects.usage;
    AuthorityProgressCut {
        scheduler: SchedulerProgressCut {
            resolve: resolve.map(ProgressVersion),
            verify_small: verify_small.map(ProgressVersion),
            verify_any: verify_any.map(ProgressVersion),
            ready: ready.map(ProgressVersion),
        },
        active_work: input.active_work,
        dependency_maintenance: input.dependency_maintenance,
        effects: EffectLogCut {
            usage: EffectUsageCut {
                remote_batches,
                remote_bytes,
                ordinary_batches,
                ordinary_bytes,
                total_batches,
                total_bytes,
            },
            ..publication
        },
        template_sources: input.template_sources,
    }
}

fn assert_wake_refines(before: WakeProjectionInput, after: WakeProjectionInput) {
    let production = AuthorityWakeTransition::observe_for_foundation(before, after);
    let relation = ClaimWakeObservation::between(claim_cut(before), claim_cut(after));
    assert_eq!(
        production,
        ProductionWakeObservation {
            compute: relation.compute,
            ready: relation.ready,
            dependency_maintenance: relation.dependency_maintenance,
            effect_publisher: relation.effect_publisher,
            effect_capacity: relation.effect_capacity,
            template: relation.template,
        },
        "production wake must equal the reference before/after relation"
    );
}

#[test]
fn uak_wake_transition_refines_every_finite_projection_law() {
    let heads = [None, Some(1), Some(2)];
    for before_resolve in heads {
        for after_resolve in heads {
            for before_small in heads {
                for after_small in heads {
                    for before_any in heads {
                        for after_any in heads {
                            for before_active in 0..=2 {
                                for after_active in 0..=2 {
                                    let mut before = wake_input();
                                    before.scheduler =
                                        [before_resolve, before_small, before_any, None];
                                    before.active_work = before_active;
                                    let mut after = before;
                                    after.scheduler = [after_resolve, after_small, after_any, None];
                                    after.active_work = after_active;
                                    assert_wake_refines(before, after);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    for before_ready in heads {
        for after_ready in heads {
            let mut before = wake_input();
            before.scheduler[3] = before_ready;
            let mut after = before;
            after.scheduler[3] = after_ready;
            assert_wake_refines(before, after);
        }
    }
    for before_dependency in [false, true] {
        for after_dependency in [false, true] {
            let mut before = wake_input();
            before.dependency_maintenance = before_dependency;
            let mut after = before;
            after.dependency_maintenance = after_dependency;
            assert_wake_refines(before, after);
        }
    }
    for before_level in [
        EffectPublisherLevelInput::Idle,
        EffectPublisherLevelInput::Available,
        EffectPublisherLevelInput::ClosedAndDrained,
    ] {
        for after_level in [
            EffectPublisherLevelInput::Idle,
            EffectPublisherLevelInput::Available,
            EffectPublisherLevelInput::ClosedAndDrained,
        ] {
            let mut before = wake_input();
            before.effects.publisher = before_level;
            let mut after = before;
            after.effects.publisher = after_level;
            assert_wake_refines(before, after);
        }
    }
    for field in 0..6 {
        for before_value in 0..=2 {
            for after_value in 0..=2 {
                let mut before = wake_input();
                before.effects.usage[field] = before_value;
                let mut after = before;
                after.effects.usage[field] = after_value;
                assert_wake_refines(before, after);
            }
        }
    }
    for field in 0..3 {
        for before_value in 0..=2 {
            for after_value in 0..=2 {
                let mut before = wake_input();
                before.template_sources[field] = before_value;
                let mut after = before;
                after.template_sources[field] = after_value;
                assert_wake_refines(before, after);
            }
        }
    }

    let mut before = wake_input();
    before.active_work = 2;
    before.effects.usage = [2; 6];
    let mut after = before;
    after.scheduler = [Some(4), None, None, Some(5)];
    after.active_work = 1;
    after.dependency_maintenance = true;
    after.effects.publisher = EffectPublisherLevelInput::Available;
    after.effects.usage = [1; 6];
    after.template_sources[1] = 1;
    assert_wake_refines(before, after);
}

fn claim_effect_observation(
    observation: &crate::authority::effect::test_support::EffectObservation,
) -> EffectPublicationObservationSnapshot {
    let queued = observation
        .queued
        .first()
        .zip(observation.queued_processed_steps.first())
        .map(|(sequence, processed)| EffectHead {
            sequence: sequence.0,
            ordinal: *processed,
        });
    let generation_reset = observation
        .latest_generation_reset
        .zip(observation.generation_reset_processed_steps)
        .map(|(sequence, processed)| EffectHead {
            sequence: sequence.0,
            ordinal: processed,
        });
    let blocking_staged_head = observation.blocking_staged_head.map(|sequence| EffectHead {
        sequence: sequence.0,
        ordinal: 0,
    });
    let cut = EffectLogCut {
        queued,
        generation_reset,
        blocking_staged_head,
        closed: observation.closed,
        pending_recent_rejects: observation.pending_recent_rejects,
        usage: EffectUsageCut {
            remote_batches: observation.remote_usage.batches,
            remote_bytes: observation.remote_usage.bytes,
            ordinary_batches: observation.ordinary_usage.batches,
            ordinary_bytes: observation.ordinary_usage.bytes,
            total_batches: observation.total_usage.batches,
            total_bytes: observation.total_usage.bytes,
        },
    };
    match cut.publication_observation() {
        ClaimEffectPublicationObservation::Receipt { source, head } => {
            EffectPublicationObservationSnapshot::Receipt {
                source: match source {
                    ClaimEffectReceiptSource::Queued => EffectReceiptSourceObservation::Queued,
                    ClaimEffectReceiptSource::GenerationReset => {
                        EffectReceiptSourceObservation::GenerationReset
                    }
                },
                sequence: ApplySequence(head.sequence),
                processed_steps: head.ordinal,
            }
        }
        ClaimEffectPublicationObservation::Idle => EffectPublicationObservationSnapshot::Idle,
        ClaimEffectPublicationObservation::ClosedAndDrained => {
            EffectPublicationObservationSnapshot::ClosedAndDrained
        }
    }
}

#[test]
fn effect_relation_blocks_a_later_reset_behind_an_earlier_staged_sequence() {
    let cut = EffectLogCut {
        generation_reset: Some(EffectHead {
            sequence: 51,
            ordinal: 0,
        }),
        blocking_staged_head: Some(EffectHead {
            sequence: 50,
            ordinal: 0,
        }),
        usage: EffectUsageCut {
            remote_batches: 1,
            remote_bytes: 1,
            ordinary_batches: 1,
            ordinary_bytes: 1,
            total_batches: 1,
            total_bytes: 1,
        },
        ..EffectLogCut::default()
    };
    assert_eq!(
        cut.publication_observation(),
        ClaimEffectPublicationObservation::Idle
    );
}

fn assert_effect_observation_refines(authority: &TxPoolAuthority) {
    let stored = authority.effect_observation_for_foundation();
    assert_eq!(
        authority.effect_publication_observation_for_foundation(),
        claim_effect_observation(&stored)
    );
}

fn rejected_effect(
    authority: &TxPoolAuthority,
    nonce: u64,
) -> crate::authority::effect::EffectPublication {
    authority
        .effect_publication_for_foundation(
            EffectPolicy::Remote,
            vec![CommittedEffect::Rejected(
                CommittedRejection::for_foundation(
                    Arc::new(tx(nonce)),
                    RejectionAudience::foundation(),
                    RejectionKind::Policy,
                ),
            )],
        )
        .expect("the finite effect fixture fits")
}

#[test]
fn uak_effect_publication_is_one_log_owned_three_way_observation() {
    let mut reset_first = TxPoolAuthority::for_foundation(limits());
    assert_effect_observation_refines(&reset_first);
    apply_plan(
        reset_first
            .plan_generation_reset_for_foundation()
            .expect("the first generation reset plans"),
    );
    assert_effect_observation_refines(&reset_first);
    let later = rejected_effect(&reset_first, 42_001);
    apply_plan(
        reset_first
            .plan_effect_publication_for_foundation(&later)
            .expect("the later queued record plans"),
    );
    assert_effect_observation_refines(&reset_first);

    let mut queued_first = TxPoolAuthority::for_foundation(limits());
    let earlier = rejected_effect(&queued_first, 42_002);
    apply_plan(
        queued_first
            .plan_effect_publication_for_foundation(&earlier)
            .expect("the first queued record plans"),
    );
    apply_plan(
        queued_first
            .plan_generation_reset_for_foundation()
            .expect("the later generation reset plans"),
    );
    assert_effect_observation_refines(&queued_first);
    apply_plan(
        queued_first
            .plan_effect_close_for_foundation()
            .expect("a journal with queued work closes"),
    );
    assert_effect_observation_refines(&queued_first);

    while let Some(receipt) = queued_first.effect_publication_receipt_for_foundation() {
        apply_plan(
            queued_first
                .apply_effect_settlement_for_foundation(
                    receipt.complete_for_foundation().published(),
                )
                .expect("the selected receipt settles"),
        );
        assert_effect_observation_refines(&queued_first);
    }
    assert!(queued_first.effects_closed_and_drained_for_foundation());
    assert_effect_observation_refines(&queued_first);

    let mut empty_close = TxPoolAuthority::for_foundation(limits());
    apply_plan(
        empty_close
            .plan_effect_close_for_foundation()
            .expect("an empty journal closes"),
    );
    assert_effect_observation_refines(&empty_close);
}
