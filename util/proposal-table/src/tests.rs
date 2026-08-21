use ckb_types::packed::ProposalShortId;
use std::{
    collections::{BTreeMap, HashSet},
    convert::Infallible,
    iter,
};

use crate::{ProposalTable, ProposalTransitionSource, ProposalView, ProposalWindow};

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
fn sparse_receipt_accepts_only_the_exact_predecessor_object() {
    let mut table = ProposalTable::new(ProposalWindow(2, 10)).expect("window is valid");
    let proposal = ProposalShortId::new([3; 10]);
    table.insert(1, HashSet::from([proposal]));
    let origin = table.finalize(&ProposalView::default(), 1);
    let exact_clone = origin.clone();
    let unrelated_equal = ProposalView::new(origin.gap_ids(), origin.proposed_ids());

    table.insert(2, HashSet::new());
    let next = table.finalize(&origin, 2);
    let exact_source = next
        .try_for_each_changed_from(&exact_clone, |_| Ok::<_, Infallible>(()))
        .expect("the visitor is infallible");
    assert_eq!(exact_source, ProposalTransitionSource::AuthenticatedSparse);
    let unrelated_source = next
        .try_for_each_changed_from(&unrelated_equal, |_| Ok::<_, Infallible>(()))
        .expect("the visitor is infallible");
    assert_eq!(unrelated_source, ProposalTransitionSource::ExactFallback);
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
    let source = terminal
        .try_for_each_changed_from(&origin, |_| Ok::<_, Infallible>(()))
        .expect("the visitor is infallible");
    assert_eq!(source, ProposalTransitionSource::ExactFallback);
}
