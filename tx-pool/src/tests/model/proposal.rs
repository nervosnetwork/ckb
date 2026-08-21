//! Proposal protocol semantics and the exact chain/tx-pool projection.
//!
//! `ProposalContext` is the normative chain environment: tip height,
//! consensus window and main/uncle proposals at every retained height.
//! `ProposalView` is its sole tx-pool boundary projection. Accepted owners
//! cache a sealed `ProposalStatusReceipt`; one exact view delta is sufficient
//! and pointwise necessary to keep that sparse cache equal to full protocol
//! recomputation.

use super::state::{AcceptedStatus, ProposalId};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Weak},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct ProposalWindow {
    closest: u16,
    farthest: u16,
}

impl ProposalWindow {
    pub(super) const fn new(closest: u16, farthest: u16) -> Option<Self> {
        if closest == 0 || closest > farthest {
            None
        } else {
            Some(Self { closest, farthest })
        }
    }

    pub(super) const fn closest(self) -> u16 {
        self.closest
    }

    pub(super) const fn farthest(self) -> u16 {
        self.farthest
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct ProposalBlock {
    height: u16,
    main: BTreeSet<ProposalId>,
    uncles: BTreeSet<ProposalId>,
}

impl ProposalBlock {
    pub(super) fn new(
        height: u16,
        main: BTreeSet<ProposalId>,
        uncles: BTreeSet<ProposalId>,
    ) -> Self {
        Self {
            height,
            main,
            uncles,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ProposalWindowPosition {
    Proposed,
    Gap,
    Outside,
}

/// Names the authority that admitted primitive proposal history.
///
/// Normal peer, sync and assume-valid processing retains consensus proposal
/// and uncle bounds.  The explicit `ckb import --skip-all-verify` operation is
/// a trusted operator bypass: its finite bytes still project exactly, but they
/// are not evidence that consensus verification ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ProposalHistoryAdmission {
    ConsensusVerified,
    OperatorTrustedBypass,
}

impl ProposalHistoryAdmission {
    pub(super) const fn proves_consensus_verification(self) -> bool {
        matches!(self, Self::ConsensusVerified)
    }
}

/// A sealed result: sibling model modules can consume it but cannot fabricate
/// a status without a `ProposalContext` derivation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ProposalStatusReceipt(AcceptedStatus);

impl ProposalStatusReceipt {
    pub(super) const fn value(self) -> AcceptedStatus {
        self.0
    }

    pub(super) const fn is_inside(self) -> bool {
        !matches!(self.0, AcceptedStatus::Pending)
    }
}

/// The unique finite cache-maintenance cut between two proposal views.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProposalTransitionDelta {
    changed: BTreeSet<ProposalId>,
}

impl ProposalTransitionDelta {
    pub(super) fn changed(&self) -> &BTreeSet<ProposalId> {
        &self.changed
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ProposalBandCounts {
    proposed: u16,
    gap: u16,
}

impl ProposalBandCounts {
    const fn position(self) -> ProposalWindowPosition {
        if self.proposed != 0 {
            ProposalWindowPosition::Proposed
        } else if self.gap != 0 {
            ProposalWindowPosition::Gap
        } else {
            ProposalWindowPosition::Outside
        }
    }

    const fn is_empty(self) -> bool {
        self.proposed == 0 && self.gap == 0
    }
}

/// Sparse evidence names the exact predecessor allocation. `Weak` retains the
/// allocation identity without retaining the predecessor view itself.
#[derive(Clone, Debug)]
struct AuthenticatedProposalTransition {
    predecessor: Weak<()>,
    changed: BTreeSet<ProposalId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProposalDeltaPath {
    AuthenticatedSparse,
    ExactFallback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProposalProjectionError {
    NotSuccessor,
    HistoryDiscontinuity,
    StaleProjection,
    CountOverflow,
    CountUnderflow,
}

/// Cost-refining realization of `ProposalView`. Counts preserve repeated ids
/// across heights and bands; the map stands for a persistent deterministic-
/// bound index, not for this model's concrete `BTreeMap` allocation behavior.
#[derive(Clone, Debug)]
struct CountedProposalProjection {
    identity: Arc<()>,
    counts: BTreeMap<ProposalId, ProposalBandCounts>,
    receipt: Option<AuthenticatedProposalTransition>,
}

/// The exact immutable projection installed atomically with a chain view.
/// Repeated proposal ids may occur in both sets; Proposed has protocol
/// precedence over Gap, matching `ckb_proposal_table::ProposalView`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(super) struct ProposalView {
    proposed: BTreeSet<ProposalId>,
    gap: BTreeSet<ProposalId>,
}

impl ProposalView {
    pub(super) fn empty() -> Self {
        Self::default()
    }

    pub(super) fn position(&self, proposal: ProposalId) -> ProposalWindowPosition {
        if self.proposed.contains(&proposal) {
            ProposalWindowPosition::Proposed
        } else if self.gap.contains(&proposal) {
            ProposalWindowPosition::Gap
        } else {
            ProposalWindowPosition::Outside
        }
    }

    pub(super) fn status(&self, proposal: ProposalId) -> ProposalStatusReceipt {
        ProposalStatusReceipt(match self.position(proposal) {
            ProposalWindowPosition::Proposed => AcceptedStatus::Proposed,
            ProposalWindowPosition::Gap => AcceptedStatus::Gap,
            ProposalWindowPosition::Outside => AcceptedStatus::Pending,
        })
    }

    /// Symmetric membership changes are the only possible status changes.
    /// The final position filter is required because repeated ids may occupy
    /// both bands and a membership change can therefore be observationally
    /// silent.
    pub(super) fn transition_to(&self, next: &Self) -> ProposalTransitionDelta {
        let candidates = self
            .proposed
            .symmetric_difference(&next.proposed)
            .chain(self.gap.symmetric_difference(&next.gap))
            .copied()
            .collect::<BTreeSet<_>>();
        let changed = candidates
            .into_iter()
            .filter(|proposal| self.position(*proposal) != next.position(*proposal))
            .collect();
        ProposalTransitionDelta { changed }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProposalContextError {
    TipHeightOverflow,
    FutureBlock,
    DuplicateHeight,
    OverlappingStatusWitness,
    ConsensusVerificationBypassed,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct ProposalContext {
    tip_height: u16,
    window: ProposalWindow,
    blocks: BTreeMap<u16, ProposalBlock>,
    admission: ProposalHistoryAdmission,
}

impl ProposalContext {
    pub(super) fn new(
        tip_height: u16,
        window: ProposalWindow,
        blocks: impl IntoIterator<Item = ProposalBlock>,
    ) -> Result<Self, ProposalContextError> {
        Self::with_admission(
            tip_height,
            window,
            blocks,
            ProposalHistoryAdmission::ConsensusVerified,
        )
    }

    pub(super) fn with_admission(
        tip_height: u16,
        window: ProposalWindow,
        blocks: impl IntoIterator<Item = ProposalBlock>,
        admission: ProposalHistoryAdmission,
    ) -> Result<Self, ProposalContextError> {
        let candidate = tip_height
            .checked_add(1)
            .ok_or(ProposalContextError::TipHeightOverflow)?;
        let retained_start = candidate.saturating_sub(window.farthest());
        let mut retained = BTreeMap::new();
        for block in blocks {
            if block.height > tip_height {
                return Err(ProposalContextError::FutureBlock);
            }
            if block.height < retained_start {
                continue;
            }
            if retained.insert(block.height, block).is_some() {
                return Err(ProposalContextError::DuplicateHeight);
            }
        }
        Ok(Self {
            tip_height,
            window,
            blocks: retained,
            admission,
        })
    }

    pub(super) fn empty() -> Self {
        Self::initial(ProposalWindow {
            closest: 2,
            farthest: 10,
        })
    }

    pub(super) fn initial(window: ProposalWindow) -> Self {
        Self {
            tip_height: 0,
            window,
            blocks: BTreeMap::new(),
            admission: ProposalHistoryAdmission::ConsensusVerified,
        }
    }

    pub(super) const fn tip_height(&self) -> u16 {
        self.tip_height
    }

    pub(super) const fn window(&self) -> ProposalWindow {
        self.window
    }

    pub(super) const fn admission(&self) -> ProposalHistoryAdmission {
        self.admission
    }

    pub(super) fn verified_view(&self) -> Result<ProposalView, ProposalContextError> {
        if self.admission.proves_consensus_verification() {
            Ok(self.view())
        } else {
            Err(ProposalContextError::ConsensusVerificationBypassed)
        }
    }

    /// Advance one canonical height from primitive block content. This is the
    /// only transition used by the two-phase progress machine; reorg remains
    /// an atomic replacement by another validated `ProposalContext`.
    pub(super) fn advance(
        &self,
        main: BTreeSet<ProposalId>,
        uncles: BTreeSet<ProposalId>,
    ) -> Result<Self, ProposalContextError> {
        let height = self
            .tip_height
            .checked_add(1)
            .ok_or(ProposalContextError::TipHeightOverflow)?;
        Self::with_admission(
            height,
            self.window,
            self.blocks
                .values()
                .cloned()
                .chain(std::iter::once(ProposalBlock::new(height, main, uncles))),
            self.admission,
        )
    }

    pub(super) fn position(&self, proposal: ProposalId) -> ProposalWindowPosition {
        self.view().position(proposal)
    }

    /// Project the chain history onto the one immutable value consumed by the
    /// tx-pool. The two raw unions intentionally retain overlap so this model
    /// also checks the production precedence rule instead of assuming it.
    pub(super) fn view(&self) -> ProposalView {
        let candidate = self.tip_height + 1;
        let mut proposed = BTreeSet::new();
        let mut gap = BTreeSet::new();
        if candidate > self.window.closest() {
            let start = candidate.saturating_sub(self.window.farthest()).max(1);
            let end = candidate.saturating_sub(self.window.closest());
            if start <= end {
                for block in self.blocks.range(start..=end).map(|(_, block)| block) {
                    proposed.extend(block.main.iter().chain(&block.uncles).copied());
                }
            }
            let gap_start = end.saturating_add(1);
            if gap_start <= self.tip_height {
                for block in self
                    .blocks
                    .range(gap_start..=self.tip_height)
                    .map(|(_, block)| block)
                {
                    gap.extend(block.main.iter().chain(&block.uncles).copied());
                }
            }
        } else if self.tip_height >= 1 {
            for block in self
                .blocks
                .range(1..=self.tip_height)
                .map(|(_, block)| block)
            {
                gap.extend(block.main.iter().chain(&block.uncles).copied());
            }
        }
        ProposalView { proposed, gap }
    }

    pub(super) fn status(&self, proposal: ProposalId) -> ProposalStatusReceipt {
        self.view().status(proposal)
    }

    /// Derive the exact finite delta required to refine a cache from `self` to
    /// `next`. An id outside both retained histories is Outside in both states,
    /// so the union of their primitive proposal ids is a complete universe.
    pub(super) fn transition_to(&self, next: &Self) -> ProposalTransitionDelta {
        self.view().transition_to(&next.view())
    }

    fn height_position(&self, height: u16) -> ProposalWindowPosition {
        if height == 0 {
            return ProposalWindowPosition::Outside;
        }
        let candidate = self.tip_height + 1;
        if candidate <= self.window.closest() {
            return if height <= self.tip_height {
                ProposalWindowPosition::Gap
            } else {
                ProposalWindowPosition::Outside
            };
        }
        let start = candidate.saturating_sub(self.window.farthest());
        let end = candidate.saturating_sub(self.window.closest());
        if (start..=end).contains(&height) {
            ProposalWindowPosition::Proposed
        } else if height > end && height <= self.tip_height {
            ProposalWindowPosition::Gap
        } else {
            ProposalWindowPosition::Outside
        }
    }

    fn is_exact_successor_of(&self, predecessor: &Self) -> Result<(), ProposalProjectionError> {
        if predecessor.tip_height.checked_add(1) != Some(self.tip_height)
            || predecessor.window != self.window
        {
            return Err(ProposalProjectionError::NotSuccessor);
        }
        let heights = predecessor
            .blocks
            .keys()
            .chain(self.blocks.keys())
            .copied()
            .filter(|height| *height <= predecessor.tip_height)
            .collect::<BTreeSet<_>>();
        for height in heights {
            let retained_by_successor =
                height >= (self.tip_height + 1).saturating_sub(self.window.farthest());
            if retained_by_successor && predecessor.blocks.get(&height) != self.blocks.get(&height)
            {
                return Err(ProposalProjectionError::HistoryDiscontinuity);
            }
        }
        Ok(())
    }

    /// Construct a primitive history witness for tests that need a particular
    /// pair of disjoint derived sets. The resulting `ChainTransition` still
    /// carries only primitive block history; no status enters the kernel.
    pub(super) fn status_witness(
        proposed: BTreeSet<ProposalId>,
        gap: BTreeSet<ProposalId>,
    ) -> Result<Self, ProposalContextError> {
        if !proposed.is_disjoint(&gap) {
            return Err(ProposalContextError::OverlappingStatusWitness);
        }
        let window = ProposalWindow::new(2, 10).expect("fixed model window is valid");
        Self::new(
            10,
            window,
            [
                ProposalBlock::new(9, proposed, BTreeSet::new()),
                ProposalBlock::new(10, BTreeSet::new(), gap),
            ],
        )
    }
}

impl CountedProposalProjection {
    fn rebuild(context: &ProposalContext) -> Result<Self, ProposalProjectionError> {
        let mut counts = BTreeMap::new();
        for block in context.blocks.values() {
            let position = context.height_position(block.height);
            for proposal in block.main.union(&block.uncles).copied() {
                Self::adjust(&mut counts, proposal, position, true)?;
            }
        }
        Ok(Self {
            identity: Arc::new(()),
            counts,
            receipt: None,
        })
    }

    fn position(&self, proposal: ProposalId) -> ProposalWindowPosition {
        self.counts
            .get(&proposal)
            .copied()
            .unwrap_or_default()
            .position()
    }

    fn view(&self) -> ProposalView {
        let mut proposed = BTreeSet::new();
        let mut gap = BTreeSet::new();
        for (&proposal, &counts) in &self.counts {
            if counts.proposed != 0 {
                proposed.insert(proposal);
            }
            if counts.gap != 0 {
                gap.insert(proposal);
            }
        }
        ProposalView { proposed, gap }
    }

    fn adjust(
        counts: &mut BTreeMap<ProposalId, ProposalBandCounts>,
        proposal: ProposalId,
        position: ProposalWindowPosition,
        add: bool,
    ) -> Result<(), ProposalProjectionError> {
        if matches!(position, ProposalWindowPosition::Outside) {
            return Ok(());
        }
        let mut next = counts.get(&proposal).copied().unwrap_or_default();
        let coordinate = match position {
            ProposalWindowPosition::Proposed => &mut next.proposed,
            ProposalWindowPosition::Gap => &mut next.gap,
            ProposalWindowPosition::Outside => unreachable!("outside returned above"),
        };
        *coordinate = if add {
            coordinate
                .checked_add(1)
                .ok_or(ProposalProjectionError::CountOverflow)?
        } else {
            coordinate
                .checked_sub(1)
                .ok_or(ProposalProjectionError::CountUnderflow)?
        };
        if next.is_empty() {
            counts.remove(&proposal);
        } else {
            counts.insert(proposal, next);
        }
        Ok(())
    }

    fn advance_successor(
        &self,
        old_context: &ProposalContext,
        new_context: &ProposalContext,
    ) -> Result<Self, ProposalProjectionError> {
        new_context.is_exact_successor_of(old_context)?;
        if self.counts != Self::rebuild(old_context)?.counts {
            return Err(ProposalProjectionError::StaleProjection);
        }
        let heights = old_context
            .blocks
            .keys()
            .chain(new_context.blocks.keys())
            .copied()
            .collect::<BTreeSet<_>>();
        let mut counts = self.counts.clone();
        let mut touched = BTreeSet::new();
        for height in heights {
            let old_block = old_context.blocks.get(&height);
            let new_block = new_context.blocks.get(&height);
            let old_position = old_context.height_position(height);
            let new_position = new_context.height_position(height);
            if old_block == new_block && old_position == new_position {
                continue;
            }
            if let Some(block) = old_block {
                for proposal in block.main.union(&block.uncles).copied() {
                    touched.insert(proposal);
                    Self::adjust(&mut counts, proposal, old_position, false)?;
                }
            }
            if let Some(block) = new_block {
                for proposal in block.main.union(&block.uncles).copied() {
                    touched.insert(proposal);
                    Self::adjust(&mut counts, proposal, new_position, true)?;
                }
            }
        }
        let changed = touched
            .into_iter()
            .filter(|proposal| {
                self.position(*proposal)
                    != counts.get(proposal).copied().unwrap_or_default().position()
            })
            .collect();
        Ok(Self {
            identity: Arc::new(()),
            counts,
            receipt: Some(AuthenticatedProposalTransition {
                predecessor: Arc::downgrade(&self.identity),
                changed,
            }),
        })
    }

    fn delta_from(&self, predecessor: &Self) -> (ProposalDeltaPath, ProposalTransitionDelta) {
        if let Some(receipt) = &self.receipt
            && receipt.predecessor.as_ptr() == Arc::as_ptr(&predecessor.identity)
        {
            return (
                ProposalDeltaPath::AuthenticatedSparse,
                ProposalTransitionDelta {
                    changed: receipt.changed.clone(),
                },
            );
        }
        (
            ProposalDeltaPath::ExactFallback,
            predecessor.view().transition_to(&self.view()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::two_phase::{CausalCandidate, causally_eligible};
    use super::*;
    use ckb_chain_spec::consensus::ProposalWindow as ProductionProposalWindow;
    use ckb_proposal_table::{
        ProposalTable, ProposalTransitionSource, ProposalView as ProductionProposalView,
    };
    use ckb_types::packed::ProposalShortId;
    use std::collections::HashSet;

    fn window(closest: u16, farthest: u16) -> ProposalWindow {
        ProposalWindow::new(closest, farthest).expect("test window is valid")
    }

    fn production_id(proposal: ProposalId) -> ProposalShortId {
        let mut bytes = [0; 10];
        bytes[9] = proposal.0;
        ProposalShortId::new(bytes)
    }

    fn production_position(
        view: &ProductionProposalView,
        proposal: &ProposalShortId,
    ) -> ProposalWindowPosition {
        if view.contains_proposed(proposal) {
            ProposalWindowPosition::Proposed
        } else if view.contains_gap(proposal) {
            ProposalWindowPosition::Gap
        } else {
            ProposalWindowPosition::Outside
        }
    }

    fn status_view(statuses: [AcceptedStatus; 3]) -> ProposalView {
        let mut proposed = BTreeSet::new();
        let mut gap = BTreeSet::new();
        for (index, status) in statuses.into_iter().enumerate() {
            let proposal = ProposalId(index as u8);
            match status {
                AcceptedStatus::Pending => {}
                AcceptedStatus::Gap => {
                    gap.insert(proposal);
                }
                AcceptedStatus::Proposed => {
                    proposed.insert(proposal);
                }
            }
        }
        ProposalView { proposed, gap }
    }

    fn decoded_statuses(mut encoding: u8) -> [AcceptedStatus; 3] {
        std::array::from_fn(|_| {
            let status = match encoding % 3 {
                0 => AcceptedStatus::Pending,
                1 => AcceptedStatus::Gap,
                2 => AcceptedStatus::Proposed,
                _ => unreachable!("base-three digit is total"),
            };
            encoding /= 3;
            status
        })
    }

    fn causal_candidates(parents: &[BTreeSet<usize>; 3]) -> Vec<CausalCandidate> {
        parents
            .iter()
            .enumerate()
            .map(|(candidate, parents)| {
                CausalCandidate::new(
                    ProposalId(candidate as u8),
                    parents
                        .iter()
                        .map(|parent| ProposalId(*parent as u8))
                        .collect(),
                    candidate as u16,
                )
            })
            .collect()
    }

    #[test]
    fn model_proposal_position_matches_the_window_distance_algebra() {
        let proposal = ProposalId(1);
        for closest in 1..=4 {
            for farthest in closest..=6 {
                for tip_height in 0..=10 {
                    for proposal_height in 0..=tip_height {
                        let distance = tip_height + 1 - proposal_height;
                        let expected = if proposal_height == 0 {
                            ProposalWindowPosition::Outside
                        } else if tip_height < closest || distance < closest {
                            ProposalWindowPosition::Gap
                        } else if distance <= farthest {
                            ProposalWindowPosition::Proposed
                        } else {
                            ProposalWindowPosition::Outside
                        };
                        for in_uncle in [false, true] {
                            let (main, uncles) = if in_uncle {
                                (BTreeSet::new(), BTreeSet::from([proposal]))
                            } else {
                                (BTreeSet::from([proposal]), BTreeSet::new())
                            };
                            let context = ProposalContext::new(
                                tip_height,
                                window(closest, farthest),
                                [ProposalBlock::new(proposal_height, main, uncles)],
                            )
                            .expect("the enumerated history is canonical");
                            assert_eq!(context.position(proposal), expected);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn model_history_projection_matches_production_proposal_table_exhaustively() {
        let proposal = ProposalId(1);
        let packed = production_id(proposal);
        for closest in 1..=4u16 {
            for farthest in closest..=6 {
                for tip_height in 0..=7u16 {
                    let history_count = 1u16 << (tip_height + 1);
                    for history in 0..history_count {
                        // Main and uncle membership are distinct protocol
                        // producers but one ProposalTable coordinate.
                        for use_uncles in [false, true] {
                            let mut blocks = Vec::new();
                            let mut table = ProposalTable::new(ProductionProposalWindow(
                                u64::from(closest),
                                u64::from(farthest),
                            ))
                            .expect("the enumerated production window is valid");
                            for height in 0..=tip_height {
                                let present = history & (1 << height) != 0;
                                let ids = if present {
                                    HashSet::from([packed.clone()])
                                } else {
                                    HashSet::new()
                                };
                                table.insert(u64::from(height), ids);
                                if present {
                                    let (main, uncles) = if use_uncles {
                                        (BTreeSet::new(), BTreeSet::from([proposal]))
                                    } else {
                                        (BTreeSet::from([proposal]), BTreeSet::new())
                                    };
                                    blocks.push(ProposalBlock::new(height, main, uncles));
                                }
                            }
                            let context =
                                ProposalContext::new(tip_height, window(closest, farthest), blocks)
                                    .expect("the enumerated history is canonical");
                            let production = table.finalize(
                                &ProductionProposalView::default(),
                                u64::from(tip_height),
                            );
                            assert_eq!(
                                context.position(proposal),
                                production_position(&production, &packed),
                                "closest={closest} farthest={farthest} tip={tip_height} history={history} uncles={use_uncles}",
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn model_production_sparse_successor_matches_full_rebuild_exhaustively() {
        let proposal = ProposalId(1);
        let packed = production_id(proposal);
        for closest in 1..=3u16 {
            for farthest in closest..=4 {
                for tip_height in 0..=5u16 {
                    for history in 0..(1u16 << (tip_height + 2)) {
                        let ids_at = |height: u16| {
                            if history & (1 << height) != 0 {
                                HashSet::from([packed.clone()])
                            } else {
                                HashSet::new()
                            }
                        };
                        let production_window =
                            ProductionProposalWindow(u64::from(closest), u64::from(farthest));
                        let mut incremental = ProposalTable::new(production_window)
                            .expect("the enumerated production window is valid");
                        for height in 0..=tip_height {
                            incremental.insert(u64::from(height), ids_at(height));
                        }
                        let old = incremental
                            .finalize(&ProductionProposalView::default(), u64::from(tip_height));
                        let next_height = tip_height + 1;
                        incremental.insert(u64::from(next_height), ids_at(next_height));
                        let next = incremental.finalize(&old, u64::from(next_height));

                        let mut sparse_changed = Vec::new();
                        let sparse_source = next
                            .try_for_each_changed_from(&old, |id| {
                                sparse_changed.push(id);
                                Ok::<_, std::convert::Infallible>(())
                            })
                            .expect("the visitor is infallible");
                        assert_eq!(sparse_source, ProposalTransitionSource::AuthenticatedSparse);

                        let mut rebuilt = ProposalTable::new(production_window)
                            .expect("the enumerated production window is valid");
                        for height in 0..=next_height {
                            rebuilt.insert(u64::from(height), ids_at(height));
                        }
                        let rebuilt_view = rebuilt.finalize(&old, u64::from(next_height));
                        assert_eq!(
                            production_position(&next, &packed),
                            production_position(&rebuilt_view, &packed),
                            "closest={closest} farthest={farthest} tip={tip_height} history={history}",
                        );

                        let mut fallback_changed = Vec::new();
                        let fallback_source = rebuilt_view
                            .try_for_each_changed_from(&old, |id| {
                                fallback_changed.push(id);
                                Ok::<_, std::convert::Infallible>(())
                            })
                            .expect("the visitor is infallible");
                        assert_eq!(fallback_source, ProposalTransitionSource::ExactFallback);
                        assert_eq!(sparse_changed, fallback_changed);
                    }
                }
            }
        }
    }

    #[test]
    fn model_main_and_uncle_proposals_have_one_membership_semantics() {
        let proposal = ProposalId(1);
        let main = ProposalContext::new(
            10,
            window(2, 10),
            [ProposalBlock::new(
                9,
                BTreeSet::from([proposal]),
                BTreeSet::new(),
            )],
        )
        .expect("main history is canonical");
        let uncle = ProposalContext::new(
            10,
            window(2, 10),
            [ProposalBlock::new(
                9,
                BTreeSet::new(),
                BTreeSet::from([proposal]),
            )],
        )
        .expect("uncle history is canonical");
        assert_eq!(main.status(proposal), uncle.status(proposal));
        assert_eq!(main.status(proposal).value(), AcceptedStatus::Proposed);
    }

    #[test]
    fn model_earlier_proposed_occurrence_dominates_a_later_gap_occurrence() {
        let proposal = ProposalId(1);
        let context = ProposalContext::new(
            10,
            window(2, 10),
            [
                ProposalBlock::new(9, BTreeSet::from([proposal]), BTreeSet::new()),
                ProposalBlock::new(10, BTreeSet::new(), BTreeSet::from([proposal])),
            ],
        )
        .expect("the repeated proposal history is canonical");
        assert_eq!(context.position(proposal), ProposalWindowPosition::Proposed);
    }

    #[test]
    fn model_plain_extension_expires_to_the_reproposal_frontier() {
        let proposal = ProposalId(1);
        let statuses = (5..=9)
            .map(|tip_height| {
                ProposalContext::new(
                    tip_height,
                    window(2, 4),
                    [ProposalBlock::new(
                        5,
                        BTreeSet::from([proposal]),
                        BTreeSet::new(),
                    )],
                )
                .expect("plain extension history is canonical")
                .status(proposal)
                .value()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            statuses,
            vec![
                AcceptedStatus::Gap,
                AcceptedStatus::Proposed,
                AcceptedStatus::Proposed,
                AcceptedStatus::Proposed,
                AcceptedStatus::Pending,
            ]
        );
    }

    #[test]
    fn model_exact_proposal_delta_is_cache_complete_and_pointwise_minimal() {
        let proposals = [ProposalId(0), ProposalId(1), ProposalId(2)];
        let contexts = (0u16..27)
            .map(|encoding| {
                let mut proposed = BTreeSet::new();
                let mut gap = BTreeSet::new();
                let mut digits = encoding;
                for proposal in proposals {
                    match digits % 3 {
                        0 => {}
                        1 => {
                            gap.insert(proposal);
                        }
                        2 => {
                            proposed.insert(proposal);
                        }
                        _ => unreachable!("base-three digit is total"),
                    }
                    digits /= 3;
                }
                ProposalContext::status_witness(proposed, gap)
                    .expect("the base-three partition is disjoint")
            })
            .collect::<Vec<_>>();

        for old in &contexts {
            for new in &contexts {
                let delta = old.transition_to(new);
                let mut refined = proposals
                    .into_iter()
                    .map(|proposal| (proposal, old.status(proposal).value()))
                    .collect::<BTreeMap<_, _>>();
                for proposal in delta.changed() {
                    refined.insert(*proposal, new.status(*proposal).value());
                }
                let canonical = proposals
                    .into_iter()
                    .map(|proposal| (proposal, new.status(proposal).value()))
                    .collect::<BTreeMap<_, _>>();
                assert_eq!(refined, canonical);

                for omitted in delta.changed() {
                    let mut incomplete = proposals
                        .into_iter()
                        .map(|proposal| (proposal, old.status(proposal).value()))
                        .collect::<BTreeMap<_, _>>();
                    for proposal in delta.changed().iter().filter(|item| *item != omitted) {
                        incomplete.insert(*proposal, new.status(*proposal).value());
                    }
                    assert_ne!(
                        incomplete, canonical,
                        "every selected proposal is a necessary cache coordinate"
                    );
                }
            }
        }
    }

    #[test]
    fn model_exact_delta_ignores_overlap_changes_that_preserve_position() {
        let proposal = ProposalId(1);
        let old = ProposalContext::new(
            10,
            window(2, 10),
            [
                ProposalBlock::new(9, BTreeSet::from([proposal]), BTreeSet::new()),
                ProposalBlock::new(10, BTreeSet::new(), BTreeSet::from([proposal])),
            ],
        )
        .expect("the repeated proposal history is canonical");
        let next = ProposalContext::new(
            10,
            window(2, 10),
            [ProposalBlock::new(
                9,
                BTreeSet::from([proposal]),
                BTreeSet::new(),
            )],
        )
        .expect("the proposed-only history is canonical");

        assert_eq!(old.position(proposal), ProposalWindowPosition::Proposed);
        assert_eq!(next.position(proposal), ProposalWindowPosition::Proposed);
        assert!(old.transition_to(&next).changed().is_empty());
    }

    #[test]
    fn model_persistent_count_projection_refines_every_small_successor_history() {
        let proposal = ProposalId(1);
        for closest in 1..=3u16 {
            for farthest in closest..=4 {
                for tip_height in 0..=5u16 {
                    for history in 0..(1u16 << (tip_height + 2)) {
                        for use_uncles in [false, true] {
                            let contexts = [tip_height, tip_height + 1].map(|tip| {
                                let blocks = (0..=tip)
                                    .filter(|height| history & (1 << height) != 0)
                                    .map(|height| {
                                        let (main, uncles) = if use_uncles {
                                            (BTreeSet::new(), BTreeSet::from([proposal]))
                                        } else {
                                            (BTreeSet::from([proposal]), BTreeSet::new())
                                        };
                                        ProposalBlock::new(height, main, uncles)
                                    });
                                ProposalContext::new(tip, window(closest, farthest), blocks)
                                    .expect("the enumerated history is canonical")
                            });
                            let old = CountedProposalProjection::rebuild(&contexts[0])
                                .expect("the bounded canonical history has finite counts");
                            let next = old
                                .advance_successor(&contexts[0], &contexts[1])
                                .expect("the second context is the exact successor");
                            let canonical = CountedProposalProjection::rebuild(&contexts[1])
                                .expect("the bounded canonical history has finite counts");
                            assert_eq!(next.counts, canonical.counts);
                            assert_eq!(next.view(), contexts[1].view());

                            let (path, delta) = next.delta_from(&old);
                            assert_eq!(path, ProposalDeltaPath::AuthenticatedSparse);
                            assert_eq!(
                                delta,
                                contexts[0].view().transition_to(&contexts[1].view())
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn model_sparse_receipt_requires_exact_predecessor_and_reorg_falls_back() {
        let proposal = ProposalId(1);
        let old_context = ProposalContext::new(
            3,
            window(2, 4),
            [ProposalBlock::new(
                3,
                BTreeSet::from([proposal]),
                BTreeSet::new(),
            )],
        )
        .expect("old history is canonical");
        let next_context = old_context
            .advance(BTreeSet::new(), BTreeSet::new())
            .expect("successor height is representable");
        let old =
            CountedProposalProjection::rebuild(&old_context).expect("old projection is bounded");
        let next = old
            .advance_successor(&old_context, &next_context)
            .expect("the successor projection is exact");
        let unrelated_equal = CountedProposalProjection::rebuild(&old_context)
            .expect("equal semantics may have an unrelated identity");

        let (wrong_path, wrong_delta) = next.delta_from(&unrelated_equal);
        assert_eq!(wrong_path, ProposalDeltaPath::ExactFallback);
        assert_eq!(
            wrong_delta,
            unrelated_equal.view().transition_to(&next.view())
        );

        let reorg_context = ProposalContext::new(
            next_context.tip_height(),
            next_context.window(),
            [ProposalBlock::new(
                next_context.tip_height(),
                BTreeSet::new(),
                BTreeSet::from([ProposalId(2)]),
            )],
        )
        .expect("replacement history is canonical");
        let rebuilt = CountedProposalProjection::rebuild(&reorg_context)
            .expect("reorg projection is rebuilt from primitives");
        let (reorg_path, reorg_delta) = rebuilt.delta_from(&next);
        assert_eq!(reorg_path, ProposalDeltaPath::ExactFallback);
        assert_eq!(reorg_delta, next.view().transition_to(&rebuilt.view()));
    }

    #[test]
    fn model_projection_identity_has_no_counter_exhaustion_state() {
        let old_context = ProposalContext::initial(window(2, 4));
        let next_context = old_context
            .advance(BTreeSet::new(), BTreeSet::new())
            .expect("successor height is representable");
        let old =
            CountedProposalProjection::rebuild(&old_context).expect("empty projection is bounded");
        let next = old
            .advance_successor(&old_context, &next_context)
            .expect("object identity has no integer exhaustion state");
        assert_eq!(
            next.delta_from(&old).0,
            ProposalDeltaPath::AuthenticatedSparse
        );
    }

    #[test]
    fn model_successor_rejects_rewritten_retained_history_and_stale_counts() {
        let proposal = ProposalId(1);
        let old_context = ProposalContext::new(
            3,
            window(2, 4),
            [ProposalBlock::new(
                3,
                BTreeSet::from([proposal]),
                BTreeSet::new(),
            )],
        )
        .expect("old history is canonical");
        let rewritten = ProposalContext::new(
            4,
            window(2, 4),
            [ProposalBlock::new(
                3,
                BTreeSet::new(),
                BTreeSet::from([ProposalId(2)]),
            )],
        )
        .expect("replacement history is canonical but not a successor");
        let old =
            CountedProposalProjection::rebuild(&old_context).expect("old projection is bounded");
        assert!(matches!(
            old.advance_successor(&old_context, &rewritten),
            Err(ProposalProjectionError::HistoryDiscontinuity)
        ));

        let next_context = old_context
            .advance(BTreeSet::new(), BTreeSet::new())
            .expect("successor height is representable");
        let stale = CountedProposalProjection::rebuild(&ProposalContext::empty())
            .expect("empty projection is bounded");
        assert!(matches!(
            stale.advance_successor(&old_context, &next_context),
            Err(ProposalProjectionError::StaleProjection)
        ));
    }

    #[test]
    fn model_direct_status_delta_is_complete_for_causal_template_eligibility() {
        // Every edge points from a lower to a higher index, so these eight
        // masks are the complete three-owner DAG normal forms.
        let possible_edges = [(0usize, 1usize), (0, 2), (1, 2)];
        for edge_mask in 0u8..8 {
            let mut parents: [BTreeSet<usize>; 3] = Default::default();
            for (edge_index, (parent, child)) in possible_edges.into_iter().enumerate() {
                if edge_mask & (1 << edge_index) != 0 {
                    parents[child].insert(parent);
                }
            }
            for old_encoding in 0u8..27 {
                let old_statuses = decoded_statuses(old_encoding);
                let old_view = status_view(old_statuses);
                for new_encoding in 0u8..27 {
                    let new_statuses = decoded_statuses(new_encoding);
                    let new_view = status_view(new_statuses);
                    let delta = old_view.transition_to(&new_view);
                    let candidates = causal_candidates(&parents);

                    let mut sparse = old_statuses;
                    for proposal in delta.changed() {
                        sparse[usize::from(proposal.0)] = new_view.status(*proposal).value();
                    }
                    assert_eq!(sparse, new_statuses);
                    assert_eq!(
                        causally_eligible(&status_view(sparse), &candidates),
                        causally_eligible(&new_view, &candidates),
                    );

                    // A descendant whose own proposal position did not change
                    // needs no owner mutation. Its eligibility changes by
                    // reading the directly updated ancestor at template time.
                    for (parent, child) in possible_edges {
                        if !parents[child].contains(&parent)
                            || old_statuses[parent] != AcceptedStatus::Proposed
                            || new_statuses[parent] == AcceptedStatus::Proposed
                        {
                            continue;
                        }
                        if !delta.changed().contains(&ProposalId(child as u8)) {
                            assert_eq!(old_statuses[child], new_statuses[child]);
                        }
                    }

                    // Every transaction selected from the causal relation is
                    // individually in the exact consensus-eligible set.
                    let selected: BTreeSet<_> = causally_eligible(&new_view, &candidates)
                        .expect("the enumerated graph is acyclic")
                        .into_iter()
                        .collect();
                    assert!(selected.iter().all(|proposal| {
                        new_view.position(*proposal) == ProposalWindowPosition::Proposed
                    }));
                }
            }
        }
    }

    #[test]
    fn model_proposal_context_excludes_invalid_structure() {
        assert_eq!(ProposalWindow::new(0, 1), None);
        assert_eq!(ProposalWindow::new(3, 2), None);
        assert_eq!(
            ProposalContext::new(u16::MAX, window(2, 10), std::iter::empty::<ProposalBlock>(),),
            Err(ProposalContextError::TipHeightOverflow)
        );
        assert_eq!(
            ProposalContext::new(
                5,
                window(2, 10),
                [ProposalBlock::new(6, BTreeSet::new(), BTreeSet::new())],
            ),
            Err(ProposalContextError::FutureBlock)
        );
        assert_eq!(
            ProposalContext::new(
                5,
                window(2, 10),
                [
                    ProposalBlock::new(5, BTreeSet::new(), BTreeSet::new()),
                    ProposalBlock::new(5, BTreeSet::new(), BTreeSet::new()),
                ],
            ),
            Err(ProposalContextError::DuplicateHeight)
        );
        assert_eq!(
            ProposalContext::status_witness(
                BTreeSet::from([ProposalId(1)]),
                BTreeSet::from([ProposalId(1)]),
            ),
            Err(ProposalContextError::OverlappingStatusWitness)
        );
    }

    #[test]
    fn model_operator_verification_bypass_is_typed_and_does_not_fabricate_consensus_proof() {
        let proposal = ProposalId(1);
        let blocks = [ProposalBlock::new(
            5,
            BTreeSet::from([proposal]),
            BTreeSet::new(),
        )];
        let verified = ProposalContext::new(5, window(2, 10), blocks.clone())
            .expect("the verified history is structurally canonical");
        let operator = ProposalContext::with_admission(
            5,
            window(2, 10),
            blocks,
            ProposalHistoryAdmission::OperatorTrustedBypass,
        )
        .expect("the operator-owned history is structurally representable");

        assert_eq!(verified.view(), operator.view());
        assert!(verified.admission().proves_consensus_verification());
        assert!(!operator.admission().proves_consensus_verification());
        assert_eq!(verified.verified_view(), Ok(verified.view()));
        assert_eq!(
            operator.verified_view(),
            Err(ProposalContextError::ConsensusVerificationBypassed)
        );
    }
}
