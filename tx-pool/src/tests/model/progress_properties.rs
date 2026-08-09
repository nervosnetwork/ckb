use super::progress::{
    AuthorityProgressCut, EffectHead, EffectLogCut, EffectPublicationObservation,
    EffectReceiptSource, EffectUsageCut, EffectWaitDisposition, ProgressVersion,
    SchedulerProgressCut, WakeObservation,
};

fn empty_cut() -> AuthorityProgressCut {
    AuthorityProgressCut {
        scheduler: SchedulerProgressCut::default(),
        active_work: 0,
        dependency_maintenance: false,
        effects: EffectLogCut::default(),
        template_sources: [0; 3],
    }
}

#[test]
fn model_effect_publication_observation_is_one_total_three_way_cut() {
    let queued = EffectHead {
        sequence: 3,
        ordinal: 1,
    };
    let older_reset = EffectHead {
        sequence: 2,
        ordinal: 0,
    };
    let newer_reset = EffectHead {
        sequence: 4,
        ordinal: 0,
    };
    let cut = |queued, generation_reset, closed, pending_recent_rejects, usage| EffectLogCut {
        queued,
        generation_reset,
        closed,
        pending_recent_rejects,
        usage,
    };

    assert_eq!(
        cut(
            Some(queued),
            Some(older_reset),
            false,
            0,
            EffectUsageCut::default()
        )
        .publication_observation(),
        EffectPublicationObservation::Receipt {
            source: EffectReceiptSource::GenerationReset,
            head: older_reset,
        }
    );
    assert_eq!(
        cut(
            Some(queued),
            Some(newer_reset),
            true,
            0,
            EffectUsageCut::default()
        )
        .publication_observation(),
        EffectPublicationObservation::Receipt {
            source: EffectReceiptSource::Queued,
            head: queued,
        }
    );
    assert_eq!(
        cut(None, None, false, 0, EffectUsageCut::default()).publication_observation(),
        EffectPublicationObservation::Idle
    );
    assert_eq!(
        cut(None, None, true, 0, EffectUsageCut::default()).publication_observation(),
        EffectPublicationObservation::ClosedAndDrained
    );
    assert_eq!(
        cut(None, None, true, 1, EffectUsageCut::default()).publication_observation(),
        EffectPublicationObservation::Idle
    );
    assert_eq!(
        cut(
            None,
            None,
            true,
            0,
            EffectUsageCut {
                total_bytes: 1,
                ..EffectUsageCut::default()
            },
        )
        .publication_observation(),
        EffectPublicationObservation::Idle
    );
}

#[test]
fn model_effect_wait_names_the_only_releaser_and_terminal_observation() {
    let head = EffectHead {
        sequence: 1,
        ordinal: 0,
    };
    assert_eq!(
        EffectPublicationObservation::Receipt {
            source: EffectReceiptSource::Queued,
            head,
        }
        .wait_disposition(),
        EffectWaitDisposition::Publish {
            source: EffectReceiptSource::Queued,
            head,
        }
    );
    assert_eq!(
        EffectPublicationObservation::Idle.wait_disposition(),
        EffectWaitDisposition::WaitForProducerCommit
    );
    assert_eq!(
        EffectPublicationObservation::ClosedAndDrained.wait_disposition(),
        EffectWaitDisposition::Terminate
    );
}

#[test]
fn model_compute_wake_is_exact_over_the_finite_head_and_release_algebra() {
    let heads = [None, Some(ProgressVersion(1)), Some(ProgressVersion(2))];
    for before_resolve in heads {
        for after_resolve in heads {
            for before_small in heads {
                for after_small in heads {
                    for before_any in heads {
                        for after_any in heads {
                            for before_active in 0..=2 {
                                for after_active in 0..=2 {
                                    let mut before = empty_cut();
                                    before.scheduler.resolve = before_resolve;
                                    before.scheduler.verify_small = before_small;
                                    before.scheduler.verify_any = before_any;
                                    before.active_work = before_active;
                                    let mut after = before;
                                    after.scheduler.resolve = after_resolve;
                                    after.scheduler.verify_small = after_small;
                                    after.scheduler.verify_any = after_any;
                                    after.active_work = after_active;

                                    let head_advanced = [
                                        (before_resolve, after_resolve),
                                        (before_small, after_small),
                                        (before_any, after_any),
                                    ]
                                    .into_iter()
                                    .any(|(old, new)| new.is_some() && old != new);
                                    let released_to_runnable = after_active < before_active
                                        && [after_resolve, after_small, after_any]
                                            .into_iter()
                                            .any(|head| head.is_some());
                                    assert_eq!(
                                        WakeObservation::between(before, after).compute,
                                        head_advanced || released_to_runnable
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn model_wake_relation_separates_suppression_from_spurious_cost() {
    let before = empty_cut();
    assert_eq!(
        WakeObservation::between(before, before),
        WakeObservation::default()
    );

    let mut after = before;
    after.scheduler.ready = Some(ProgressVersion(1));
    let ready = WakeObservation::between(before, after);
    assert!(ready.ready);
    assert_eq!(ready.notification_operations(), 1);

    let before = after;
    after.dependency_maintenance = true;
    after.effects.generation_reset = Some(EffectHead {
        sequence: 2,
        ordinal: 0,
    });
    after.template_sources[2] = 1;
    let three = WakeObservation::between(before, after);
    assert!(three.dependency_maintenance);
    assert!(three.effect_publisher);
    assert!(three.template);
    assert_eq!(three.notification_operations(), 3);
    assert!(three.notification_operations() <= 6);
}

#[test]
fn model_effect_capacity_wake_observes_any_release_but_not_stable_or_growth() {
    let before_usage = EffectUsageCut {
        remote_batches: 2,
        remote_bytes: 20,
        ordinary_batches: 3,
        ordinary_bytes: 30,
        total_batches: 4,
        total_bytes: 40,
    };
    let mut before = empty_cut();
    before.effects.usage = before_usage;

    for field in 0..6 {
        let mut after = before;
        match field {
            0 => after.effects.usage.remote_batches -= 1,
            1 => after.effects.usage.remote_bytes -= 1,
            2 => after.effects.usage.ordinary_batches -= 1,
            3 => after.effects.usage.ordinary_bytes -= 1,
            4 => after.effects.usage.total_batches -= 1,
            5 => after.effects.usage.total_bytes -= 1,
            _ => unreachable!("the finite field index is closed"),
        }
        assert!(WakeObservation::between(before, after).effect_capacity);
    }

    assert!(!WakeObservation::between(before, before).effect_capacity);
    let mut growth = before;
    growth.effects.usage.total_bytes += 1;
    assert!(!WakeObservation::between(before, growth).effect_capacity);
}
