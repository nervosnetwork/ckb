use super::super::{
    plan::{ApplyClockReservation, ClockPlanReservation},
    state::{ApplySequence, Arrival, AuthorityClockBank, AuthorityClocks, EntryVersion},
};
use std::{num::NonZeroUsize, sync::Arc};

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

fn production_bank(clocks: ClockSnapshot) -> Arc<AuthorityClockBank> {
    Arc::new(AuthorityClockBank::from_snapshot(production_clocks(clocks)))
}

fn clock_snapshot(clocks: AuthorityClocks) -> ClockSnapshot {
    ClockSnapshot {
        next_version: clocks.next_version.0,
        next_arrival: clocks.next_arrival.0,
        next_sequence: clocks.next_sequence.0,
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
                let bank = production_bank(before);
                let mut reservation = ClockPlanReservation::begin(Arc::clone(&bank));

                for offset in 0..length {
                    if insertion_mask & (1usize << offset) == 0 {
                        let version = reservation
                            .replacement()
                            .expect("the finite replacement identity fits");
                        assert_eq!(version.0, before.next_version + offset as u128);
                    } else {
                        let arrival_index = (0..offset)
                            .filter(|prior| insertion_mask & (1usize << prior) != 0)
                            .count();
                        let (version, arrival) = reservation
                            .insertion()
                            .expect("the finite insertion identities fit");
                        assert_eq!(version.0, before.next_version + offset as u128);
                        assert_eq!(arrival.0, before.next_arrival + arrival_index as u128);
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
                        clock_snapshot(bank.snapshot()),
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
fn uak_apply_clock_reservation_rejects_each_counter_boundary() {
    let sequence_exhausted = ClockSnapshot {
        next_version: 0,
        next_arrival: 0,
        next_sequence: u128::MAX,
    };
    assert!(ApplyClockReservation::begin(production_bank(sequence_exhausted)).is_err());

    let version_exhausted = ClockSnapshot {
        next_version: u128::MAX,
        next_arrival: 0,
        next_sequence: 0,
    };
    let mut reservation = ApplyClockReservation::begin(production_bank(version_exhausted))
        .expect("the independent sequence range is available");
    assert!(reservation.replacement().is_err());

    let arrival_exhausted = ClockSnapshot {
        next_version: 0,
        next_arrival: u128::MAX,
        next_sequence: 0,
    };
    let mut reservation = ApplyClockReservation::begin(production_bank(arrival_exhausted))
        .expect("the independent sequence range is available");
    assert!(reservation.insertion().is_err());

    let batch_exhausted = ClockSnapshot {
        next_version: u128::MAX - 1,
        next_arrival: 0,
        next_sequence: 0,
    };
    let mut reservation = ClockPlanReservation::begin(production_bank(batch_exhausted));
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
            let bank = production_bank(before);
            let reservation = ApplyClockReservation::begin(Arc::clone(&bank))
                .expect("the Apply sequence is available");
            reservation
                .adopt_owner_progress(production_clocks(after).owner_progress())
                .expect("legal owner progress is adoptable");
            assert_eq!(clock_snapshot(bank.snapshot()), after);
        }
    }

    let reservation = ApplyClockReservation::begin(production_bank(before))
        .expect("the Apply sequence is available");
    assert!(
        reservation
            .adopt_owner_progress(
                production_clocks(ClockSnapshot {
                    next_version: before.next_version + 1,
                    next_arrival: before.next_arrival + 2,
                    next_sequence: before.next_sequence,
                })
                .owner_progress()
            )
            .is_err(),
        "an arrival without a matching version is not a legal demand"
    );
    let bank = production_bank(before);
    let reservation = ApplyClockReservation::begin(Arc::clone(&bank))
        .expect("the one external Apply sequence is available");
    reservation
        .adopt_owner_progress(
            production_clocks(ClockSnapshot {
                next_version: 19,
                next_arrival: 11,
                next_sequence: 1_000,
            })
            .owner_progress(),
        )
        .expect("two scratch owner insertions are legal");

    assert_eq!(
        clock_snapshot(bank.snapshot()),
        ClockSnapshot {
            next_version: 19,
            next_arrival: 11,
            next_sequence: 24,
        },
        "owner progress cannot import a scratch Apply sequence"
    );
}

#[test]
fn uak_dropped_clock_reservations_leave_nonreused_gaps() {
    let before = ClockSnapshot {
        next_version: 7,
        next_arrival: 11,
        next_sequence: 13,
    };
    let bank = production_bank(before);
    let mut first =
        ApplyClockReservation::begin(Arc::clone(&bank)).expect("the first sequence is available");
    let (first_version, first_arrival) = first
        .insertion()
        .expect("the first owner identity is available");
    assert_eq!(
        (first.sequence().0, first_version.0, first_arrival.0),
        (13, 7, 11)
    );
    drop(first);

    let mut second = ApplyClockReservation::begin(Arc::clone(&bank))
        .expect("the sequence after the abandoned gap is available");
    let (second_version, second_arrival) = second
        .insertion()
        .expect("the owner identity after the abandoned gap is available");
    assert_eq!(
        (second.sequence().0, second_version.0, second_arrival.0),
        (14, 8, 12)
    );
    assert_eq!(
        clock_snapshot(bank.snapshot()),
        ClockSnapshot {
            next_version: 9,
            next_arrival: 13,
            next_sequence: 15,
        }
    );
}

#[test]
fn uak_concurrent_clock_reservations_are_unique_without_an_apply_lock() {
    let bank = production_bank(ClockSnapshot {
        next_version: 1,
        next_arrival: 0,
        next_sequence: 1,
    });
    let mut workers = Vec::new();
    for _ in 0..8 {
        let bank = Arc::clone(&bank);
        workers.push(std::thread::spawn(move || {
            let mut reservation = ApplyClockReservation::begin(bank)
                .expect("the finite concurrent sequence range fits");
            let (version, arrival) = reservation
                .insertion()
                .expect("the finite concurrent owner range fits");
            (reservation.sequence(), version, arrival)
        }));
    }
    let mut identities = workers
        .into_iter()
        .map(|worker| {
            worker
                .join()
                .expect("clock reservation worker does not panic")
        })
        .collect::<Vec<_>>();
    identities.sort_unstable();
    identities.dedup();

    assert_eq!(identities.len(), 8);
    assert_eq!(
        clock_snapshot(bank.snapshot()),
        ClockSnapshot {
            next_version: 9,
            next_arrival: 8,
            next_sequence: 9,
        }
    );
}
