use super::atomic_transition::{
    ClockBranchDecision, ClockCommit, ClockCommitError, ClockDemand, ClockPlan,
    ModelAuthorityClocks, ModelDependencyControl, ModelEffectControl, ModelOwnerTransition,
    ModelRetirementCarrier, TransitionControlCommit, TransitionControlDemand,
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
fn model_owner_clock_branch_adopts_exact_demand_or_discards_to_its_parent_cut() {
    for prefix_versions in 0..=3 {
        for prefix_arrivals in 0..=prefix_versions {
            for branch_versions in 0..=3 {
                for branch_arrivals in 0..=branch_versions {
                    let before = ModelAuthorityClocks {
                        next_version: 11,
                        next_arrival: 17,
                        next_sequence: 23,
                    };
                    let prefix = ClockPlan::reserve(
                        before,
                        ClockDemand::new(prefix_versions, prefix_arrivals)
                            .expect("the finite prefix demand is legal"),
                    )
                    .expect("the finite prefix fits every owner counter");
                    let parent = prefix.resolve(ClockBranchDecision::Adopt);
                    let branch_demand = ClockDemand::new(branch_versions, branch_arrivals)
                        .expect("the finite branch demand is legal");
                    let branch = ClockPlan::reserve(parent, branch_demand)
                        .expect("the finite branch fits every owner counter");

                    assert_eq!(
                        branch.resolve(ClockBranchDecision::Discard),
                        parent,
                        "discard must be the identity transition at every parent cut"
                    );
                    assert_eq!(
                        branch.resolve(ClockBranchDecision::Adopt),
                        ModelAuthorityClocks {
                            next_version: parent.next_version + branch_demand.version_count(),
                            next_arrival: parent.next_arrival + branch_demand.arrival_count(),
                            next_sequence: parent.next_sequence,
                        }
                    );
                }
            }
        }
    }

    let last_available = ModelAuthorityClocks {
        next_version: u128::MAX - 1,
        next_arrival: u128::MAX - 1,
        next_sequence: 31,
    };
    let speculative = ClockPlan::reserve(
        last_available,
        ClockDemand::new(1, 1).expect("one speculative insertion is legal"),
    )
    .expect("the final owner identities remain available");
    let one_insertion = ClockDemand::new(1, 1).expect("one later insertion is legal");

    assert!(
        ClockPlan::reserve(
            speculative.resolve(ClockBranchDecision::Discard),
            one_insertion,
        )
        .is_ok()
    );
    assert_eq!(
        ClockPlan::reserve(
            speculative.resolve(ClockBranchDecision::Adopt),
            one_insertion,
        ),
        Err(ClockCommitError::VersionOverflow),
        "burning a rejected branch changes the next legal disposition"
    );
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
fn model_distinct_nonempty_applies_exclude_an_equal_dependency_event_cut() {
    for next_sequence in 0..=8 {
        let before = ModelAuthorityClocks {
            next_version: 0,
            next_arrival: 0,
            next_sequence,
        };
        let no_owner = ClockDemand::new(0, 0).expect("a projection Apply needs no owner clock");
        let evidence = ClockCommit::reserve(before, no_owner)
            .expect("the finite evidence producer sequence is representable");
        let event = ClockCommit::reserve(evidence.after(), no_owner)
            .expect("the finite dependency event sequence is representable");
        let later_checkout = ClockCommit::reserve(event.after(), no_owner)
            .expect("the finite checkout sequence is representable");

        assert!(evidence.sequence() < event.sequence());
        assert!(later_checkout.sequence() > event.sequence());
        assert_ne!(evidence.sequence(), event.sequence());
        assert_ne!(later_checkout.sequence(), event.sequence());
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

#[test]
fn model_owner_transition_derives_insertion_and_retirement_without_a_noop_state() {
    let cases = [
        (ModelOwnerTransition::Insert, 1, false),
        (ModelOwnerTransition::ReplaceInline, 0, false),
        (ModelOwnerTransition::ReplaceOutside, 0, true),
        (ModelOwnerTransition::Remove, 0, true),
    ];
    for (transition, primary_insertions, outside) in cases {
        assert_eq!(transition.primary_insertions(), primary_insertions);
        for coupled_removals in [0, 1, 3] {
            let carrier = transition
                .retirement_carrier(coupled_removals)
                .expect("the finite retirement demand fits usize");
            assert_eq!(
                carrier.reserved_owners(),
                coupled_removals + usize::from(outside)
            );
            assert_eq!(
                matches!(carrier, ModelRetirementCarrier::Outside { .. }),
                outside
            );
        }
    }
    assert_eq!(
        ModelOwnerTransition::ReplaceOutside.retirement_carrier(usize::MAX),
        None
    );
    assert_eq!(
        ModelOwnerTransition::Remove.retirement_carrier(usize::MAX),
        None
    );
    assert_eq!(
        ModelOwnerTransition::Insert
            .retirement_carrier(usize::MAX)
            .map(ModelRetirementCarrier::reserved_owners),
        Some(usize::MAX)
    );
}
