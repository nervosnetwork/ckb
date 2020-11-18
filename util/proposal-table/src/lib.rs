//! TODO(doc): @zhangsoledad
use ckb_chain_spec::consensus::ProposalWindow;
use ckb_types::{core::BlockNumber, packed::ProposalShortId};
use std::collections::{BTreeMap, HashSet};
use std::ops::Bound;

/// TODO(doc): @zhangsoledad
#[derive(Default, Clone, Debug)]
pub struct ProposalView {
    pub(crate) gap: HashSet<ProposalShortId>,
    pub(crate) set: HashSet<ProposalShortId>,
}

impl ProposalView {
    /// TODO(doc): @zhangsoledad
    pub fn new(gap: HashSet<ProposalShortId>, set: HashSet<ProposalShortId>) -> ProposalView {
        ProposalView { gap, set }
    }

    /// TODO(doc): @zhangsoledad
    pub fn gap(&self) -> &HashSet<ProposalShortId> {
        &self.gap
    }

    /// TODO(doc): @zhangsoledad
    pub fn set(&self) -> &HashSet<ProposalShortId> {
        &self.set
    }

    /// TODO(doc): @zhangsoledad
    pub fn contains_proposed(&self, id: &ProposalShortId) -> bool {
        self.set.contains(id)
    }

    /// TODO(doc): @zhangsoledad
    pub fn contains_gap(&self, id: &ProposalShortId) -> bool {
        self.gap.contains(id)
    }
}

/// TODO(doc): @zhangsoledad
#[derive(Debug, PartialEq, Clone, Eq)]
pub struct ProposalTable {
    pub(crate) table: BTreeMap<BlockNumber, HashSet<ProposalShortId>>,
    pub(crate) proposal_window: ProposalWindow,
}

impl ProposalTable {
    /// TODO(doc): @zhangsoledad
    pub fn new(proposal_window: ProposalWindow) -> Self {
        ProposalTable {
            proposal_window,
            table: BTreeMap::default(),
        }
    }

    /// TODO(doc): @zhangsoledad
    // If the TABLE did not have this value present, true is returned.
    // If the TABLE did have this value present, false is returned
    pub fn insert(&mut self, number: BlockNumber, ids: HashSet<ProposalShortId>) -> bool {
        self.table.insert(number, ids).is_none()
    }

    /// TODO(doc): @zhangsoledad
    pub fn remove(&mut self, number: BlockNumber) -> Option<HashSet<ProposalShortId>> {
        self.table.remove(&number)
    }

    /// TODO(doc): @zhangsoledad
    pub fn all(&self) -> &BTreeMap<BlockNumber, HashSet<ProposalShortId>> {
        &self.table
    }

    /// TODO(doc): @zhangsoledad
    pub fn finalize(
        &mut self,
        origin: &ProposalView,
        number: BlockNumber,
    ) -> (HashSet<ProposalShortId>, ProposalView) {
        let candidate_number = number + 1;
        let proposal_start = candidate_number.saturating_sub(self.proposal_window.farthest());
        let proposal_end = candidate_number.saturating_sub(self.proposal_window.closest());

        if proposal_start > 1 {
            self.table = self.table.split_off(&proposal_start);
        }

        ckb_logger::trace!("[proposal_finalize] table {:?}", self.table);

        // - if candidate_number <= self.proposal_window.closest()
        //      new_ids = []
        //      gap = [1..candidate_number]
        // - else
        //      new_ids = [candidate_number- farthest..= candidate_number- closest]
        //      gap = [candidate_number- closest + 1..candidate_number]
        // - end
        let (new_ids, gap) = if candidate_number <= self.proposal_window.closest() {
            (
                HashSet::new(),
                self.table
                    .range((Bound::Unbounded, Bound::Included(&number)))
                    .map(|pair| pair.1)
                    .cloned()
                    .flatten()
                    .collect(),
            )
        } else {
            (
                self.table
                    .range((
                        Bound::Included(&proposal_start),
                        Bound::Included(&proposal_end),
                    ))
                    .map(|pair| pair.1)
                    .cloned()
                    .flatten()
                    .collect(),
                self.table
                    .range((Bound::Excluded(&proposal_end), Bound::Included(&number)))
                    .map(|pair| pair.1)
                    .cloned()
                    .flatten()
                    .collect(),
            )
        };

        let removed_ids: HashSet<ProposalShortId> =
            origin.set().difference(&new_ids).cloned().collect();
        ckb_logger::trace!(
            "[proposal_finalize] number {} proposal_start {}----proposal_end {}",
            number,
            proposal_start,
            proposal_end
        );
        ckb_logger::trace!(
            "[proposal_finalize] number {} new_ids {:?}----removed_ids {:?}",
            number,
            new_ids,
            removed_ids
        );
        (removed_ids, ProposalView::new(gap, new_ids))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::iter::{self, FromIterator};

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
        assert_eq!(
            view.gap(),
            &HashSet::from_iter(iter::once(proposals[1].clone()))
        );

        // in window
        for i in 2..=10usize {
            let (removed_ids, new_view) = table.finalize(&view, i as u64);
            let c = i + 1;
            assert_eq!(
                new_view.gap(),
                &HashSet::from_iter(proposals[(c - 2 + 1)..=i].iter().cloned())
            );

            let s = ::std::cmp::max(1, c.saturating_sub(10));
            assert_eq!(
                new_view.set(),
                &HashSet::from_iter(proposals[s..=(c - 2)].iter().cloned())
            );

            assert!(removed_ids.is_empty());
            view = new_view;
        }

        // finalize 11
        let (removed_ids, new_view) = table.finalize(&view, 11);
        assert_eq!(
            removed_ids,
            HashSet::from_iter(iter::once(proposals[1].clone()))
        );
        assert_eq!(
            new_view.set(),
            &HashSet::from_iter(proposals[2..=10].iter().cloned())
        );
        assert!(new_view.gap().is_empty());

        view = new_view;

        // finalize 12
        let (removed_ids, new_view) = table.finalize(&view, 12);
        assert_eq!(
            removed_ids,
            HashSet::from_iter(iter::once(proposals[2].clone()))
        );
        assert_eq!(
            new_view.set(),
            &HashSet::from_iter(proposals[3..=10].iter().cloned())
        );
        assert!(new_view.gap().is_empty());
    }
}
