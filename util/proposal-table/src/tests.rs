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
