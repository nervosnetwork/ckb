use super::super::{
    plan::{ApplyClockReservation, ClockPlanReservation},
    state::{ApplySequence, Arrival, AuthorityClocks, EntryVersion},
};
use std::num::NonZeroUsize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClockSnapshot {
    next_version: u128,
    next_arrival: u128,
    next_sequence: u128,
}

fn production_clocks(clocks: ClockSnapshot) -> AuthorityClocks {
    AuthorityClocks {
        next_version: EntryVersion(clocks.next_version),
        next_arrival: Arrival(clocks.next_arrival),
        next_sequence: ApplySequence(clocks.next_sequence),
    }
}

fn clock_snapshot(clocks: AuthorityClocks) -> ClockSnapshot {
    ClockSnapshot {
        next_version: clocks.next_version.0,
        next_arrival: clocks.next_arrival.0,
        next_sequence: clocks.next_sequence.0,
    }
}

#[test]
fn uak_apply_clock_reservation_allocates_every_finite_legal_demand_exactly() {
    for length in 0..=4 {
        for insertion_mask in 0..(1usize << length) {
            let arrival_count = (0..length)
                .filter(|offset| insertion_mask & (1usize << offset) != 0)
                .count();
            for before in [
                ClockSnapshot {
                    next_version: 0,
                    next_arrival: 0,
                    next_sequence: 0,
                },
                ClockSnapshot {
                    next_version: 7,
                    next_arrival: 11,
                    next_sequence: 13,
                },
                ClockSnapshot {
                    next_version: u128::MAX - 4,
                    next_arrival: u128::MAX - 4,
                    next_sequence: u128::MAX - 1,
                },
            ] {
                let mut reservation = ApplyClockReservation::begin(production_clocks(before))
                    .expect("the finite fixture has one Apply sequence");
                assert_eq!(reservation.sequence().0, before.next_sequence);

                let mut versions = Vec::new();
                let mut arrivals = Vec::new();
                for offset in 0..length {
                    if insertion_mask & (1usize << offset) == 0 {
                        let (version, next) = reservation
                            .replacement()
                            .expect("the finite replacement range fits");
                        versions.push(version.0);
                        reservation = next;
                    } else {
                        let (version, arrival, next) = reservation
                            .insertion()
                            .expect("the finite insertion ranges fit");
                        versions.push(version.0);
                        arrivals.push(arrival.0);
                        reservation = next;
                    }
                }

                assert_eq!(
                    versions,
                    (0..length)
                        .map(|index| before.next_version + index as u128)
                        .collect::<Vec<_>>()
                );
                assert_eq!(
                    arrivals,
                    (0..arrival_count)
                        .map(|index| before.next_arrival + index as u128)
                        .collect::<Vec<_>>()
                );
                assert_eq!(
                    clock_snapshot(reservation.finish()),
                    ClockSnapshot {
                        next_version: before.next_version + length as u128,
                        next_arrival: before.next_arrival + arrival_count as u128,
                        next_sequence: before.next_sequence + 1,
                    }
                );
            }
        }
    }
}

#[test]
fn uak_discardable_clock_plan_allocates_owner_demand_before_apply_sealing() {
    for length in 0..=4 {
        for insertion_mask in 0..(1usize << length) {
            let arrival_count = (0..length)
                .filter(|offset| insertion_mask & (1usize << offset) != 0)
                .count();
            for before in [
                ClockSnapshot {
                    next_version: 7,
                    next_arrival: 11,
                    next_sequence: 13,
                },
                ClockSnapshot {
                    next_version: u128::MAX - 4,
                    next_arrival: u128::MAX - 4,
                    next_sequence: u128::MAX,
                },
            ] {
                let mut reservation = ClockPlanReservation::begin(production_clocks(before));

                for offset in 0..length {
                    if insertion_mask & (1usize << offset) == 0 {
                        let (version, next) = reservation
                            .replacement()
                            .expect("the finite replacement identity fits");
                        assert_eq!(version.0, before.next_version + offset as u128);
                        reservation = next;
                    } else {
                        let arrival_index = (0..offset)
                            .filter(|prior| insertion_mask & (1usize << prior) != 0)
                            .count();
                        let (version, arrival, next) = reservation
                            .insertion()
                            .expect("the finite insertion identities fit");
                        assert_eq!(version.0, before.next_version + offset as u128);
                        assert_eq!(arrival.0, before.next_arrival + arrival_index as u128);
                        reservation = next;
                    }
                }

                if before.next_sequence == u128::MAX {
                    assert!(reservation.commit().is_err());
                } else {
                    let production = reservation
                        .commit()
                        .expect("the finite Plan sequence is available");
                    assert_eq!(production.sequence().0, before.next_sequence);
                    assert_eq!(
                        clock_snapshot(production.finish()),
                        ClockSnapshot {
                            next_version: before.next_version + length as u128,
                            next_arrival: before.next_arrival + arrival_count as u128,
                            next_sequence: before.next_sequence + 1,
                        }
                    );
                }
            }
        }
    }
}

#[test]
fn uak_owner_clock_branch_adopts_or_discards_for_plan_and_apply_parents() {
    for adopt in [false, true] {
        for before in [
            ClockSnapshot {
                next_version: 7,
                next_arrival: 11,
                next_sequence: 13,
            },
            ClockSnapshot {
                next_version: u128::MAX - 2,
                next_arrival: u128::MAX - 2,
                next_sequence: u128::MAX - 1,
            },
        ] {
            let parent = ClockSnapshot {
                next_version: before.next_version + 1,
                ..before
            };
            let expected = ClockSnapshot {
                next_version: parent.next_version + u128::from(adopt),
                next_arrival: parent.next_arrival + u128::from(adopt),
                next_sequence: before.next_sequence + 1,
            };

            let mut plan = ClockPlanReservation::begin(production_clocks(before));
            let (prefix, next) = plan.replacement().expect("the finite Plan prefix fits");
            assert_eq!(prefix.0, before.next_version);
            plan = next;
            let (version, arrival, branch) = plan
                .owner_branch()
                .insertion()
                .expect("the finite Plan branch fits");
            assert_eq!(version.0, parent.next_version);
            assert_eq!(arrival.0, parent.next_arrival);
            if adopt {
                branch.adopt();
            }
            let committed = plan.commit().expect("the Plan sequence is available");
            assert_eq!(clock_snapshot(committed.finish()), expected);

            let mut apply = ApplyClockReservation::begin(production_clocks(before))
                .expect("the Apply sequence is available");
            let (prefix, next) = apply.replacement().expect("the finite Apply prefix fits");
            assert_eq!(prefix.0, before.next_version);
            apply = next;
            let (version, arrival, branch) = apply
                .owner_branch()
                .insertion()
                .expect("the finite Apply branch fits");
            assert_eq!(version.0, parent.next_version);
            assert_eq!(arrival.0, parent.next_arrival);
            if adopt {
                branch.adopt();
            }
            assert_eq!(clock_snapshot(apply.finish()), expected);
        }
    }
}

#[test]
fn uak_apply_clock_reservation_rejects_each_counter_boundary() {
    let sequence_exhausted = ClockSnapshot {
        next_version: 0,
        next_arrival: 0,
        next_sequence: u128::MAX,
    };
    assert!(ApplyClockReservation::begin(production_clocks(sequence_exhausted)).is_err());

    let version_exhausted = ClockSnapshot {
        next_version: u128::MAX,
        next_arrival: 0,
        next_sequence: 0,
    };
    let reservation = ApplyClockReservation::begin(production_clocks(version_exhausted))
        .expect("the independent sequence range is available");
    assert!(reservation.replacement().is_err());

    let arrival_exhausted = ClockSnapshot {
        next_version: 0,
        next_arrival: u128::MAX,
        next_sequence: 0,
    };
    let reservation = ApplyClockReservation::begin(production_clocks(arrival_exhausted))
        .expect("the independent sequence range is available");
    assert!(reservation.insertion().is_err());

    let batch_exhausted = ClockSnapshot {
        next_version: u128::MAX - 1,
        next_arrival: 0,
        next_sequence: 0,
    };
    let reservation = ApplyClockReservation::begin(production_clocks(batch_exhausted))
        .expect("the independent sequence range is available");
    assert!(
        reservation
            .replacements(NonZeroUsize::new(2).expect("the batch is nonempty"))
            .is_err()
    );
}

#[test]
fn uak_apply_clock_reservation_adopts_only_legal_owner_progress() {
    let before = ClockSnapshot {
        next_version: 17,
        next_arrival: 9,
        next_sequence: 23,
    };
    for versions in 0..=4 {
        for arrivals in 0..=versions {
            let after = ClockSnapshot {
                next_version: before.next_version + versions as u128,
                next_arrival: before.next_arrival + arrivals as u128,
                next_sequence: before.next_sequence + 1,
            };
            let reservation = ApplyClockReservation::begin(production_clocks(before))
                .expect("the Apply sequence is available");
            let reservation = reservation
                .adopt_owner_progress(production_clocks(after))
                .expect("legal owner progress is adoptable");
            assert_eq!(clock_snapshot(reservation.finish()), after);
        }
    }

    let reservation = ApplyClockReservation::begin(production_clocks(before))
        .expect("the Apply sequence is available");
    assert!(
        reservation
            .adopt_owner_progress(production_clocks(ClockSnapshot {
                next_version: before.next_version + 1,
                next_arrival: before.next_arrival + 2,
                next_sequence: before.next_sequence,
            }))
            .is_err(),
        "an arrival without a matching version is not a legal demand"
    );
}
