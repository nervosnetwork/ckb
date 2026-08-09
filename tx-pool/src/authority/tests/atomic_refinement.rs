use super::super::{
    plan::{ApplyClockReservation, ClockPlanReservation},
    state::{ApplySequence, Arrival, AuthorityClocks, EntryVersion},
};
use crate::mathematical_model::{
    ClockBranchDecision, ClockCommit, ClockCommitError, ClockDemand, ClockPlan,
    ModelAuthorityClocks,
};
use std::num::NonZeroUsize;

fn production_clocks(clocks: ModelAuthorityClocks) -> AuthorityClocks {
    AuthorityClocks {
        next_version: EntryVersion(clocks.next_version),
        next_arrival: Arrival(clocks.next_arrival),
        next_sequence: ApplySequence(clocks.next_sequence),
    }
}

fn model_clocks(clocks: AuthorityClocks) -> ModelAuthorityClocks {
    ModelAuthorityClocks {
        next_version: clocks.next_version.0,
        next_arrival: clocks.next_arrival.0,
        next_sequence: clocks.next_sequence.0,
    }
}

#[test]
fn uak_apply_clock_reservation_refines_every_finite_model_demand() {
    for length in 0..=4 {
        for insertion_mask in 0..(1usize << length) {
            let arrival_count = (0..length)
                .filter(|offset| insertion_mask & (1usize << offset) != 0)
                .count();
            for before in [
                ModelAuthorityClocks {
                    next_version: 0,
                    next_arrival: 0,
                    next_sequence: 0,
                },
                ModelAuthorityClocks {
                    next_version: 7,
                    next_arrival: 11,
                    next_sequence: 13,
                },
                ModelAuthorityClocks {
                    next_version: u128::MAX - 4,
                    next_arrival: u128::MAX - 4,
                    next_sequence: u128::MAX - 1,
                },
            ] {
                let demand = ClockDemand::new(length, arrival_count)
                    .expect("the generated demand has at most one arrival per version");
                let model = ClockCommit::reserve(before, demand)
                    .expect("the finite boundary fixtures fit every counter");
                let mut reservation = ApplyClockReservation::begin(production_clocks(before))
                    .expect("the model accepted the Apply sequence");
                assert_eq!(reservation.sequence().0, model.sequence());

                let mut versions = Vec::new();
                let mut arrivals = Vec::new();
                for offset in 0..length {
                    if insertion_mask & (1usize << offset) == 0 {
                        let (version, next) = reservation
                            .replacement()
                            .expect("the model accepted the replacement range");
                        versions.push(version.0);
                        reservation = next;
                    } else {
                        let (version, arrival, next) = reservation
                            .insertion()
                            .expect("the model accepted the insertion ranges");
                        versions.push(version.0);
                        arrivals.push(arrival.0);
                        reservation = next;
                    }
                }

                assert_eq!(
                    versions,
                    (0..length)
                        .map(|index| model.version(index).expect("index is inside demand"))
                        .collect::<Vec<_>>()
                );
                assert_eq!(
                    arrivals,
                    (0..arrival_count)
                        .map(|index| model.arrival(index).expect("index is inside demand"))
                        .collect::<Vec<_>>()
                );
                assert_eq!(model_clocks(reservation.finish()), model.after());
            }
        }
    }
}

#[test]
fn uak_discardable_clock_plan_refines_owner_demand_before_apply_sealing() {
    for length in 0..=4 {
        for insertion_mask in 0..(1usize << length) {
            let arrival_count = (0..length)
                .filter(|offset| insertion_mask & (1usize << offset) != 0)
                .count();
            for before in [
                ModelAuthorityClocks {
                    next_version: 7,
                    next_arrival: 11,
                    next_sequence: 13,
                },
                ModelAuthorityClocks {
                    next_version: u128::MAX - 4,
                    next_arrival: u128::MAX - 4,
                    next_sequence: u128::MAX,
                },
            ] {
                let demand = ClockDemand::new(length, arrival_count)
                    .expect("the generated owner demand is legal");
                let model = ClockPlan::reserve(before, demand)
                    .expect("the finite owner demand fits without sequence capacity");
                let mut reservation = ClockPlanReservation::begin(production_clocks(before));

                for offset in 0..length {
                    if insertion_mask & (1usize << offset) == 0 {
                        let (version, next) = reservation
                            .replacement()
                            .expect("the model accepted the replacement identity");
                        assert_eq!(
                            version.0,
                            model.version(offset).expect("the index is inside demand")
                        );
                        reservation = next;
                    } else {
                        let arrival_index = (0..offset)
                            .filter(|prior| insertion_mask & (1usize << prior) != 0)
                            .count();
                        let (version, arrival, next) = reservation
                            .insertion()
                            .expect("the model accepted the insertion identities");
                        assert_eq!(
                            version.0,
                            model.version(offset).expect("the index is inside demand")
                        );
                        assert_eq!(
                            arrival.0,
                            model
                                .arrival(arrival_index)
                                .expect("the arrival index is inside demand")
                        );
                        reservation = next;
                    }
                }

                match (model.commit(), reservation.commit()) {
                    (Ok(model), Ok(production)) => {
                        assert_eq!(production.sequence().0, model.sequence());
                        assert_eq!(model_clocks(production.finish()), model.after());
                    }
                    (
                        Err(ClockCommitError::SequenceOverflow),
                        Err(super::super::plan::ClockReservationError),
                    ) => {}
                    (model, production) => {
                        panic!(
                            "model and production clock Plan dispositions differ: model={model:?}, production_error={}",
                            production.is_err()
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn uak_owner_clock_branch_refines_adopt_and_discard_for_plan_and_apply_parents() {
    for decision in [ClockBranchDecision::Discard, ClockBranchDecision::Adopt] {
        for before in [
            ModelAuthorityClocks {
                next_version: 7,
                next_arrival: 11,
                next_sequence: 13,
            },
            ModelAuthorityClocks {
                next_version: u128::MAX - 2,
                next_arrival: u128::MAX - 2,
                next_sequence: u128::MAX - 1,
            },
        ] {
            let prefix_demand = ClockDemand::new(1, 0).expect("the prefix demand is legal");
            let parent = ClockPlan::reserve(before, prefix_demand)
                .expect("the prefix fits the finite boundary")
                .resolve(ClockBranchDecision::Adopt);
            let branch_demand = ClockDemand::new(1, 1).expect("the branch demand is legal");
            let expected_owner = ClockPlan::reserve(parent, branch_demand)
                .expect("the branch fits the finite boundary")
                .resolve(decision);
            let expected = ModelAuthorityClocks {
                next_sequence: before.next_sequence + 1,
                ..expected_owner
            };

            let mut plan = ClockPlanReservation::begin(production_clocks(before));
            let (prefix, next) = plan
                .replacement()
                .expect("the model accepted the Plan prefix");
            assert_eq!(prefix.0, before.next_version);
            plan = next;
            let (version, arrival, branch) = plan
                .owner_branch()
                .insertion()
                .expect("the model accepted the Plan branch");
            assert_eq!(version.0, parent.next_version);
            assert_eq!(arrival.0, parent.next_arrival);
            match decision {
                ClockBranchDecision::Discard => {}
                ClockBranchDecision::Adopt => branch.adopt(),
            }
            let committed = plan.commit().expect("the model accepted the Plan sequence");
            assert_eq!(model_clocks(committed.finish()), expected);

            let mut apply = ApplyClockReservation::begin(production_clocks(before))
                .expect("the model accepted the Apply sequence");
            let (prefix, next) = apply
                .replacement()
                .expect("the model accepted the Apply prefix");
            assert_eq!(prefix.0, before.next_version);
            apply = next;
            let (version, arrival, branch) = apply
                .owner_branch()
                .insertion()
                .expect("the model accepted the Apply branch");
            assert_eq!(version.0, parent.next_version);
            assert_eq!(arrival.0, parent.next_arrival);
            match decision {
                ClockBranchDecision::Discard => {}
                ClockBranchDecision::Adopt => branch.adopt(),
            }
            assert_eq!(model_clocks(apply.finish()), expected);
        }
    }
}

#[test]
fn uak_apply_clock_reservation_refines_model_counter_boundaries() {
    let sequence_exhausted = ModelAuthorityClocks {
        next_version: 0,
        next_arrival: 0,
        next_sequence: u128::MAX,
    };
    assert_eq!(
        ClockCommit::reserve(
            sequence_exhausted,
            ClockDemand::new(0, 0).expect("effect-only demand is legal"),
        ),
        Err(ClockCommitError::SequenceOverflow)
    );
    assert!(ApplyClockReservation::begin(production_clocks(sequence_exhausted)).is_err());

    let version_exhausted = ModelAuthorityClocks {
        next_version: u128::MAX,
        next_arrival: 0,
        next_sequence: 0,
    };
    assert_eq!(
        ClockCommit::reserve(
            version_exhausted,
            ClockDemand::new(1, 0).expect("one replacement is legal"),
        ),
        Err(ClockCommitError::VersionOverflow)
    );
    let reservation = ApplyClockReservation::begin(production_clocks(version_exhausted))
        .expect("the independent sequence range is available");
    assert!(reservation.replacement().is_err());

    let arrival_exhausted = ModelAuthorityClocks {
        next_version: 0,
        next_arrival: u128::MAX,
        next_sequence: 0,
    };
    assert_eq!(
        ClockCommit::reserve(
            arrival_exhausted,
            ClockDemand::new(1, 1).expect("one insertion is legal"),
        ),
        Err(ClockCommitError::ArrivalOverflow)
    );
    let reservation = ApplyClockReservation::begin(production_clocks(arrival_exhausted))
        .expect("the independent sequence range is available");
    assert!(reservation.insertion().is_err());

    let batch_exhausted = ModelAuthorityClocks {
        next_version: u128::MAX - 1,
        next_arrival: 0,
        next_sequence: 0,
    };
    assert_eq!(
        ClockCommit::reserve(
            batch_exhausted,
            ClockDemand::new(2, 0).expect("two replacements are legal"),
        ),
        Err(ClockCommitError::VersionOverflow)
    );
    let reservation = ApplyClockReservation::begin(production_clocks(batch_exhausted))
        .expect("the independent sequence range is available");
    assert!(
        reservation
            .replacements(NonZeroUsize::new(2).expect("the batch is nonempty"))
            .is_err()
    );
}

#[test]
fn uak_apply_clock_reservation_adopts_only_model_legal_owner_progress() {
    let before = ModelAuthorityClocks {
        next_version: 17,
        next_arrival: 9,
        next_sequence: 23,
    };
    for versions in 0..=4 {
        for arrivals in 0..=versions {
            let model = ClockCommit::reserve(
                before,
                ClockDemand::new(versions, arrivals)
                    .expect("the owner prefix has at most one arrival per version"),
            )
            .expect("the finite owner prefix fits every counter");
            let reservation = ApplyClockReservation::begin(production_clocks(before))
                .expect("the model accepted the Apply sequence");
            let reservation = reservation
                .adopt_owner_progress(production_clocks(model.after()))
                .expect("model-legal owner progress is adoptable");
            assert_eq!(model_clocks(reservation.finish()), model.after());
        }
    }

    let reservation = ApplyClockReservation::begin(production_clocks(before))
        .expect("the Apply sequence is available");
    assert!(
        reservation
            .adopt_owner_progress(production_clocks(ModelAuthorityClocks {
                next_version: before.next_version + 1,
                next_arrival: before.next_arrival + 2,
                next_sequence: before.next_sequence,
            }))
            .is_err(),
        "an arrival without a matching version is not a model-legal demand"
    );
}
