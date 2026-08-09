use super::atomic_transition::{
    ClockCommit, ClockCommitError, ClockDemand, ClockPlan, ModelAuthorityClocks,
    ModelDependencyControl, ModelEffectControl, TransitionControlCommit, TransitionControlDemand,
    TransitionControlError,
};

#[test]
fn model_clock_commit_advances_one_sequence_and_the_exact_identity_ranges() {
    let before = ModelAuthorityClocks {
        next_version: 10,
        next_arrival: 20,
        next_sequence: 30,
    };
    for versions in 0..=4 {
        for arrivals in 0..=versions {
            let demand = ClockDemand::new(versions, arrivals)
                .expect("the finite demand never asks for an arrival without a version");
            let commit = ClockCommit::reserve(before, demand)
                .expect("the bounded fixture is below every counter limit");
            assert_eq!(commit.before(), before);
            assert_eq!(commit.sequence(), before.next_sequence);
            assert_eq!(
                commit.after(),
                ModelAuthorityClocks {
                    next_version: before.next_version + demand.version_count(),
                    next_arrival: before.next_arrival + demand.arrival_count(),
                    next_sequence: before.next_sequence + 1,
                }
            );
            for index in 0..versions {
                assert_eq!(
                    commit.version(index),
                    Ok(before.next_version + index as u128)
                );
            }
            for index in 0..arrivals {
                assert_eq!(
                    commit.arrival(index),
                    Ok(before.next_arrival + index as u128)
                );
            }
            assert_eq!(
                commit.version(versions),
                Err(ClockCommitError::IndexOutOfBounds)
            );
            assert_eq!(
                commit.arrival(arrivals),
                Err(ClockCommitError::IndexOutOfBounds)
            );
        }
    }
}

#[test]
fn model_clock_commit_is_total_or_mutation_free_at_every_counter_boundary() {
    let one_version = ClockDemand::new(1, 0).expect("one replacement is a legal demand");
    assert_eq!(
        ClockCommit::reserve(
            ModelAuthorityClocks {
                next_version: u128::MAX,
                next_arrival: 0,
                next_sequence: 0,
            },
            one_version,
        ),
        Err(ClockCommitError::VersionOverflow)
    );
    let one_insertion = ClockDemand::new(1, 1).expect("one insertion is a legal demand");
    assert_eq!(
        ClockCommit::reserve(
            ModelAuthorityClocks {
                next_version: 0,
                next_arrival: u128::MAX,
                next_sequence: 0,
            },
            one_insertion,
        ),
        Err(ClockCommitError::ArrivalOverflow)
    );
    assert_eq!(
        ClockCommit::reserve(
            ModelAuthorityClocks {
                next_version: 0,
                next_arrival: 0,
                next_sequence: u128::MAX,
            },
            ClockDemand::new(0, 0).expect("an effect-only Apply needs no owner identity"),
        ),
        Err(ClockCommitError::SequenceOverflow)
    );
    assert_eq!(
        ClockDemand::new(0, 1),
        Err(ClockCommitError::ArrivalWithoutVersion)
    );
}

#[test]
fn model_discardable_clock_plan_does_not_require_apply_sequence_capacity() {
    let before = ModelAuthorityClocks {
        next_version: 7,
        next_arrival: 11,
        next_sequence: u128::MAX,
    };
    let plan = ClockPlan::reserve(
        before,
        ClockDemand::new(1, 1).expect("one prospective insertion is legal"),
    )
    .expect("a discardable owner Plan does not reserve an Apply sequence");

    assert_eq!(plan.before(), before);
    assert_eq!(plan.version(0), Ok(7));
    assert_eq!(plan.arrival(0), Ok(11));
    assert_eq!(
        plan.owner_after(),
        ModelAuthorityClocks {
            next_version: 8,
            next_arrival: 12,
            next_sequence: u128::MAX,
        }
    );
    assert_eq!(plan.commit(), Err(ClockCommitError::SequenceOverflow));
}

#[test]
fn model_batch_clock_commit_uses_one_apply_sequence_for_every_member() {
    let before = ModelAuthorityClocks {
        next_version: 7,
        next_arrival: 11,
        next_sequence: 13,
    };
    for members in 1..=16 {
        let commit = ClockCommit::reserve(
            before,
            ClockDemand::new(members, 0).expect("the batch changes existing owners"),
        )
        .expect("the finite batch fits every counter");
        assert_eq!(commit.sequence(), 13);
        assert_eq!(commit.after().next_sequence, 14);
        assert_eq!(commit.after().next_version, 7 + members as u128);
    }
}

#[test]
fn model_transition_controls_make_every_required_projection_structural() {
    let dependency = ModelDependencyControl(1);
    let effect = ModelEffectControl(2);
    let cases = [
        (TransitionControlDemand::None, None, None),
        (TransitionControlDemand::Dependency, Some(dependency), None),
        (TransitionControlDemand::Effect, None, Some(effect)),
        (
            TransitionControlDemand::DependencyAndEffect,
            Some(dependency),
            Some(effect),
        ),
    ];
    for (demand, dependency, effect) in cases {
        let commit = TransitionControlCommit::seal(demand, dependency, effect)
            .expect("the exact required controls form one closed capability");
        assert_eq!(commit.dependency(), dependency);
        assert_eq!(commit.effect(), effect);
    }

    assert_eq!(
        TransitionControlCommit::seal(TransitionControlDemand::Dependency, None, None),
        Err(TransitionControlError::MissingDependency)
    );
    assert_eq!(
        TransitionControlCommit::seal(TransitionControlDemand::Effect, None, None),
        Err(TransitionControlError::MissingEffect)
    );
    assert_eq!(
        TransitionControlCommit::seal(TransitionControlDemand::None, Some(dependency), None,),
        Err(TransitionControlError::UnexpectedDependency)
    );
    assert_eq!(
        TransitionControlCommit::seal(TransitionControlDemand::None, None, Some(effect)),
        Err(TransitionControlError::UnexpectedEffect)
    );
}
