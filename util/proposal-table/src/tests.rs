use ckb_types::packed::ProposalShortId;
use std::{
    collections::{BTreeMap, HashSet},
    iter,
};

use crate::{ProposalStatus, ProposalTable, ProposalView, ProposalWindow};

#[test]
fn phase_lookup_preserves_band_precedence_absence_and_snapshot_origin() {
    let id = |value: u16| {
        let [high, low] = value.to_be_bytes();
        ProposalShortId::new([high, low, 0, 0, 0, 0, 0, 0, 0, 0])
    };
    let gap: HashSet<_> = (0..257).filter(|value| value % 2 == 0).map(id).collect();
    let proposed: HashSet<_> = (0..257).filter(|value| value % 3 == 0).map(id).collect();
    let original = ProposalView::new(gap.iter().cloned(), proposed.iter().cloned());
    let changed = ProposalView::new(proposed.iter().cloned(), gap.iter().cloned());
    let retained = gap.union(&proposed).count();
    // Both sides of the materialization threshold, including a zero hint, must
    // implement the same phase projection for hits, misses and repeated ids.
    for hint in [0, 1, retained - 1, retained, retained + 1, 512] {
        let lookup = original.status_lookup(hint);
        let updated = changed.status_lookup(hint);
        for value in (0..512).chain((0..512).rev()) {
            let proposal = id(value);
            let expected = if proposed.contains(&proposal) {
                ProposalStatus::Proposed
            } else if gap.contains(&proposal) {
                ProposalStatus::Gap
            } else {
                ProposalStatus::Pending
            };
            let next = if gap.contains(&proposal) {
                ProposalStatus::Proposed
            } else if proposed.contains(&proposal) {
                ProposalStatus::Gap
            } else {
                ProposalStatus::Pending
            };
            assert_eq!(lookup(proposal.as_reader()), expected);
            assert_eq!(original.status(&proposal), expected);
            assert_eq!(original.status_reader(proposal.as_reader()), expected);
            assert_eq!(updated(proposal.as_reader()), next);
            assert_eq!(lookup(proposal.as_reader()), expected);
        }
    }
    let empty = ProposalView::default();
    assert_eq!(
        empty.status_lookup(512)(id(0).as_reader()),
        ProposalStatus::Pending
    );
}

#[test]
fn phase_changes_match_all_three_id_band_transitions_in_order() {
    let ids = [id(0), id(7), id(u8::MAX)];
    let phase = |bits| match bits {
        0 => ProposalStatus::Pending,
        1 => ProposalStatus::Gap,
        _ => ProposalStatus::Proposed,
    };
    let views: Vec<_> = (0_u32..64)
        .map(|bands| {
            let included = |band| {
                ids.iter()
                    .enumerate()
                    .filter(move |(index, _)| (bands >> (index * 2)) & band != 0)
                    .map(|(_, id)| id.clone())
            };
            ProposalView::new(included(1), included(2))
        })
        .collect();
    for (before, original) in views.iter().enumerate() {
        assert!(
            original
                .status_changes(&original.clone(), 0)
                .unwrap()
                .next()
                .is_none()
        );
        for (after, next) in views.iter().enumerate() {
            let expected: Vec<_> = ids
                .iter()
                .enumerate()
                .filter_map(|(index, id)| {
                    let old = phase((before >> (index * 2)) & 3);
                    let new = phase((after >> (index * 2)) & 3);
                    (old != new).then(|| (id.clone(), old, new))
                })
                .collect();
            assert_eq!(
                original
                    .status_changes(next, ids.len())
                    .unwrap()
                    .collect::<Vec<_>>(),
                expected,
                "materialized bands {before} -> {after}"
            );
        }
    }
}

#[test]
fn phase_change_scan_bounds_each_view_by_twice_the_population() {
    let original = ProposalView::new([id(1), id(2)], [id(3), id(4)]);
    let next = ProposalView::new([id(1)], [id(2), id(3), id(4)]);
    for population in 0..2 {
        assert!(original.status_changes(&next, population).is_none());
        assert!(next.status_changes(&original, population).is_none());
    }
    assert_eq!(
        original
            .status_changes(&next, 2)
            .unwrap()
            .collect::<Vec<_>>(),
        vec![(id(2), ProposalStatus::Gap, ProposalStatus::Proposed)]
    );
    assert!(
        original
            .status_changes(&original.clone(), 0)
            .unwrap()
            .next()
            .is_none()
    );
    let empty = ProposalView::default();
    assert!(original.status_changes(&empty, 1).is_none());
    assert!(empty.status_changes(&original, 1).is_none());
    assert_eq!(original.status_changes(&empty, 2).unwrap().count(), 4);
    assert_eq!(empty.status_changes(&original, 2).unwrap().count(), 4);
    let larger = ProposalView::new([], [id(1), id(2), id(3), id(4), id(5)]);
    assert!(original.status_changes(&larger, 2).is_none());
    assert!(larger.status_changes(&original, 2).is_none());
}

#[test]
fn phase_changes_cover_shared_tree_branches_during_full_windows() {
    let id = |value: u32| {
        let mut bytes = [0; 10];
        bytes[..4].copy_from_slice(&value.wrapping_mul(2_654_435_761).to_be_bytes());
        ProposalShortId::new(bytes)
    };
    let original = ProposalView::new(iter::empty(), (0..4_096).map(id));
    for value in 0..4_096 {
        let changed = id(value);
        let mut counts = original.counts.as_ref().clone();
        counts.insert(
            super::ProposalKey::from_packed(&changed),
            super::BandCounts {
                gap: 1,
                proposed: 0,
            },
        );
        let next = ProposalView::from_counts(counts);
        assert_eq!(
            original
                .status_changes(&next, 4_096)
                .unwrap()
                .collect::<Vec<_>>(),
            vec![(changed, ProposalStatus::Proposed, ProposalStatus::Gap)],
            "isolated change {value} among shared branches"
        );
    }
    let mut table = ProposalTable::new(ProposalWindow(2, 10)).unwrap();
    let mut view = ProposalView::default();
    for tip in 1..=30 {
        table.insert(
            tip,
            (0..1_500).map(|offset| id((tip as u32 * 997 + offset) % 20_000)),
        );
        let next = table.finalize(&view, tip);
        let expected: BTreeMap<_, _> = (0..20_000)
            .filter_map(|value| {
                let id = id(value);
                let before = view.status(&id);
                let after = next.status(&id);
                (before != after).then_some((id, (before, after)))
            })
            .collect();
        let actual: Vec<_> = view
            .status_changes(&next, 20_000)
            .unwrap()
            .map(|(id, before, after)| (id, (before, after)))
            .collect();
        assert_eq!(actual.len(), expected.len(), "transition at tip {tip}");
        assert_eq!(actual, expected.into_iter().collect::<Vec<_>>());
        view = next;
    }
}

fn proposed(view: &ProposalView) -> HashSet<ProposalShortId> {
    view.proposed_ids().collect()
}

fn gap(view: &ProposalView) -> HashSet<ProposalShortId> {
    view.gap_ids().collect()
}

#[test]
fn test_finalize() {
    let proposals = vec![
        ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
        ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 2]),
        ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 3]),
        ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 4]),
        ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 5]),
        ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 6]),
        ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 7]),
        ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 8]),
        ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 9]),
        ProposalShortId::new([0u8, 0, 0, 0, 0, 0, 0, 0, 0, 10]),
    ];

    let window = ProposalWindow(2, 10);
    let mut table = ProposalTable::new(window).expect("proposal window is valid");

    for (idx, id) in proposals.iter().skip(1).enumerate() {
        table.insert((idx + 1) as u64, iter::once(id.clone()));
    }

    let mut view = table.finalize(&ProposalView::default(), 1);
    assert!(proposed(&view).is_empty());
    assert_eq!(gap(&view), iter::once(proposals[1].clone()).collect());

    // in window
    for i in 2..=10usize {
        let new_view = table.finalize(&view, i as u64);
        let c = i + 1;
        assert_eq!(
            gap(&new_view),
            proposals[(c - 2 + 1)..=i].iter().cloned().collect()
        );

        let s = ::std::cmp::max(1, c.saturating_sub(10));
        assert_eq!(
            proposed(&new_view),
            proposals[s..=(c - 2)].iter().cloned().collect()
        );

        view = new_view;
    }

    // finalize 11
    let new_view = table.finalize(&view, 11);
    assert!(!new_view.contains_proposed(&proposals[1]));
    assert_eq!(
        proposed(&new_view),
        proposals[2..=10].iter().cloned().collect()
    );
    assert!(gap(&new_view).is_empty());

    view = new_view;

    // finalize 12
    let new_view = table.finalize(&view, 12);
    assert!(!new_view.contains_proposed(&proposals[2]));
    assert_eq!(
        proposed(&new_view),
        proposals[3..=10].iter().cloned().collect()
    );
    assert!(gap(&new_view).is_empty());
}

#[test]
fn invalid_window_is_rejected_before_history_ownership() {
    assert!(ProposalTable::new(ProposalWindow(0, 10)).is_err());
    assert!(ProposalTable::new(ProposalWindow(11, 10)).is_err());
}

#[test]
fn incremental_update_requires_the_exact_predecessor() {
    let mut table = ProposalTable::new(ProposalWindow(2, 10)).expect("window is valid");
    let proposal = id(3);
    table.insert(1, [proposal.clone()]);
    let origin = table.finalize(&ProposalView::default(), 1);
    let exact_clone = origin.clone();
    let unrelated_equal = ProposalView::new(origin.gap_ids(), origin.proposed_ids());
    table.insert(2, []);

    assert!(table.successor_view(&exact_clone, 2).is_some());
    assert!(table.successor_view(&unrelated_equal, 2).is_none());
    let rebuilt = table.finalize(&unrelated_equal, 2);
    assert_eq!(proposed(&rebuilt), HashSet::from([proposal]));
    assert!(gap(&rebuilt).is_empty());
}

#[test]
fn mutations_between_tips_preserve_the_exact_history_projection() {
    let mut table = ProposalTable::new(ProposalWindow(3, 10)).expect("window is valid");
    for height in [1, 3, 4, 5] {
        table.insert(height, [id(height as u8)]);
    }
    let origin = table.finalize(&ProposalView::default(), 5);

    // Removing an absent height preserves both the clean predecessor and an
    // already prepared ordinary successor.
    assert!(!table.remove(99));
    assert!(table.insert(6, [id(6)]));
    assert!(!table.remove(99));
    assert!(table.successor_view(&origin, 6).is_some());
    let view = table.finalize(&origin, 6);
    assert_eq!(proposed(&view), HashSet::from([1, 3, 4].map(id)));
    assert_eq!(gap(&view), HashSet::from([5, 6].map(id)));

    // A second insertion can add history inside an unchanged height band;
    // updating only the ordinary successor's boundaries would miss id(2).
    assert!(table.insert(7, [id(7)]));
    assert!(table.insert(2, [id(2)]));
    let view = table.finalize(&view, 7);
    assert_eq!(proposed(&view), HashSet::from([1, 2, 3, 4, 5].map(id)));
    assert_eq!(gap(&view), HashSet::from([6, 7].map(id)));

    assert!(table.insert(8, [id(8)]));
    assert!(!table.insert(3, [id(33)]));
    let replaced = table.finalize(&view, 8);
    assert_eq!(
        proposed(&replaced),
        HashSet::from([1, 2, 33, 4, 5, 6].map(id))
    );
    assert_eq!(gap(&replaced), HashSet::from([7, 8].map(id)));
    assert!(view.contains_proposed(&id(3)));
    assert!(!view.contains_proposed(&id(33)));

    assert!(table.insert(9, [id(9)]));
    assert!(table.remove(3));
    let removed = table.finalize(&replaced, 9);
    assert_eq!(
        proposed(&removed),
        HashSet::from([1, 2, 4, 5, 6, 7].map(id))
    );
    assert_eq!(gap(&removed), HashSet::from([8, 9].map(id)));
    assert!(replaced.contains_proposed(&id(33)));
}

#[test]
fn incremental_counts_preserve_repeated_ids_across_both_bands() {
    let window = ProposalWindow(2, 4);
    let mut history = BTreeMap::new();
    let mut table = ProposalTable::new(window).expect("window is valid");
    let mut view = ProposalView::default();
    for tip in 1..=12 {
        let ids = if tip <= 6 {
            HashSet::from([id(1), id(tip as u8)])
        } else {
            HashSet::new()
        };
        history.insert(tip, ids.clone());
        table.insert(tip, ids);
        let old = view.clone();
        view = table.finalize(&view, tip);
        let expected = finalize_history(window, &history, tip);
        assert_eq!(proposed(&view), proposed(&expected), "tip={tip}");
        assert_eq!(gap(&view), gap(&expected), "tip={tip}");
        let changes: Vec<_> = (0..=12)
            .filter_map(|byte| {
                let id = id(byte);
                let before = old.status(&id);
                let after = expected.status(&id);
                (before != after).then_some((id, before, after))
            })
            .collect();
        assert_eq!(
            old.status_changes(&view, 13).unwrap().collect::<Vec<_>>(),
            changes,
            "shared history at tip={tip}"
        );
        if tip == 2 {
            assert!(
                old.contains_gap(&id(1)),
                "the prior snapshot stays immutable"
            );
            assert!(view.contains_gap(&id(1)));
            assert!(view.contains_proposed(&id(1)));
        }
    }
    assert!(proposed(&view).is_empty());
    assert!(gap(&view).is_empty());
}

fn finalize_history(
    window: ProposalWindow,
    history: &BTreeMap<u64, HashSet<ProposalShortId>>,
    tip: u64,
) -> ProposalView {
    let mut table = ProposalTable::new(window).expect("window is valid");
    for (&height, ids) in history {
        table.insert(height, ids.iter().cloned());
    }
    table.finalize(&ProposalView::default(), tip)
}

fn id(byte: u8) -> ProposalShortId {
    ProposalShortId::new([byte; 10])
}

#[test]
fn reorg_rebuild_replaces_detached_proposals_at_genesis_boundary() {
    let window = ProposalWindow(2, 10);
    let genesis = id(9);
    let old = id(1);
    let new = id(4);
    let history = BTreeMap::from([
        (0, HashSet::from([genesis.clone()])),
        (1, HashSet::from([old.clone()])),
        (2, HashSet::from([id(2)])),
    ]);
    let mut table = ProposalTable::new(window).expect("window is valid");
    for (&height, ids) in &history {
        table.insert(height, ids.iter().cloned());
    }
    let origin = table.finalize(&ProposalView::default(), 2);

    table.remove(1);
    table.insert(1, HashSet::from([new.clone()]));
    let rebuilt = table.finalize(&origin, 2);
    assert!(!rebuilt.contains_proposed(&genesis));
    assert!(!rebuilt.contains_proposed(&old));
    assert!(rebuilt.contains_proposed(&new));
}

#[test]
fn gap_status_does_not_claim_an_exact_primitive_occurrence() {
    let window = ProposalWindow(3, 10);
    let shared = id(1);
    let extra = id(2);
    let history_a = BTreeMap::from([
        (5, HashSet::new()),
        (9, HashSet::from([shared.clone()])),
        (10, HashSet::new()),
    ]);
    let history_b = BTreeMap::from([
        (5, HashSet::from([extra.clone()])),
        (9, HashSet::from([shared.clone()])),
        (10, HashSet::new()),
    ]);
    let view_a = finalize_history(window, &history_a, 10);
    let view_b = finalize_history(window, &history_b, 10);

    assert!(view_a.contains_gap(&shared));
    assert!(view_b.contains_gap(&shared));
    assert_ne!(proposed(&view_a), proposed(&view_b));
    assert!(!view_a.contains_proposed(&extra));
    assert!(view_b.contains_proposed(&extra));
}

#[test]
fn maximum_tip_has_a_total_terminal_projection() {
    let id = ProposalShortId::new([7; 10]);
    let mut table = ProposalTable::new(ProposalWindow(2, 10)).expect("window is valid");
    table.insert(u64::MAX - 2, HashSet::from([id.clone()]));
    table.insert(u64::MAX - 1, HashSet::new());
    let origin = table.finalize(&ProposalView::default(), u64::MAX - 1);
    assert!(origin.contains_proposed(&id));

    table.insert(u64::MAX, HashSet::new());
    let terminal = table.finalize(&origin, u64::MAX);
    assert!(!terminal.contains_proposed(&id));
    assert!(!terminal.contains_gap(&id));
}
