use ckb_types::packed::ProposalShortId;
use std::{collections::HashSet, iter};

use crate::{ProposalTable, ProposalView, ProposalWindow};

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
    let mut table = ProposalTable::new(window);

    for (idx, id) in proposals.iter().skip(1).enumerate() {
        let mut ids = HashSet::new();
        ids.insert(id.clone());
        table.insert((idx + 1) as u64, ids.clone());
    }

    let (removed_ids, mut view) = table.finalize(&ProposalView::default(), 1);
    assert!(removed_ids.is_empty());
    assert!(view.set().is_empty());
    assert_eq!(view.gap(), &iter::once(proposals[1].clone()).collect());

    // in window
    for i in 2..=10usize {
        let (removed_ids, new_view) = table.finalize(&view, i as u64);
        let c = i + 1;
        assert_eq!(
            new_view.gap(),
            &proposals[(c - 2 + 1)..=i].iter().cloned().collect()
        );

        let s = ::std::cmp::max(1, c.saturating_sub(10));
        assert_eq!(
            new_view.set(),
            &proposals[s..=(c - 2)].iter().cloned().collect()
        );

        assert!(removed_ids.is_empty());
        view = new_view;
    }

    // finalize 11
    let (removed_ids, new_view) = table.finalize(&view, 11);
    assert_eq!(removed_ids, iter::once(proposals[1].clone()).collect());
    assert_eq!(new_view.set(), &proposals[2..=10].iter().cloned().collect());
    assert!(new_view.gap().is_empty());

    view = new_view;

    // finalize 12
    let (removed_ids, new_view) = table.finalize(&view, 12);
    assert_eq!(removed_ids, iter::once(proposals[2].clone()).collect());
    assert_eq!(new_view.set(), &proposals[3..=10].iter().cloned().collect());
    assert!(new_view.gap().is_empty());
}
