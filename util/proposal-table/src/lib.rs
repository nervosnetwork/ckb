//! The CKB proposal history projection for two-step transaction confirmation.

use ckb_chain_spec::consensus::ProposalWindow;
use ckb_types::{core::BlockNumber, packed::ProposalShortId, prelude::Entity};
use imbl::OrdMap;
use std::{
    cmp::Ordering,
    collections::BTreeMap,
    fmt,
    sync::{Arc, Weak},
};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, Default)]
struct BandCounts {
    proposed: BlockNumber,
    gap: BlockNumber,
}

impl BandCounts {
    const fn is_empty(self) -> bool {
        self.proposed == 0 && self.gap == 0
    }
}

/// Inline protocol identity. Packed accessors may share the complete block or
/// uncle backing; retaining them as index keys would make the memory bound
/// depend on an unrelated envelope instead of the fixed 10-byte short id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct ProposalKey([u8; 10]);

impl ProposalKey {
    fn from_packed(id: &ProposalShortId) -> Self {
        let mut bytes = [0; 10];
        bytes.copy_from_slice(id.as_slice());
        Self(bytes)
    }

    fn sorted_unique(ids: impl IntoIterator<Item = ProposalShortId>) -> Vec<Self> {
        let mut ids = ids
            .into_iter()
            .map(|id| Self::from_packed(&id))
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    fn into_packed(self) -> ProposalShortId {
        ProposalShortId::new(self.0)
    }
}

#[derive(Debug)]
struct ProposalTransitionReceipt {
    predecessor: Weak<ProposalViewState>,
    changed: Vec<ProposalKey>,
}

#[derive(Debug)]
struct ProposalViewState {
    counts: OrdMap<ProposalKey, BandCounts>,
    receipt: Option<ProposalTransitionReceipt>,
}

/// Identifies whether a proposal-position delta used the sealed predecessor
/// receipt or was derived by an exact full comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProposalTransitionSource {
    /// The successor was produced directly from the supplied predecessor.
    AuthenticatedSparse,
    /// The views are unrelated or discontinuous, so every retained id was
    /// compared exactly.
    ExactFallback,
}

/// An immutable point-in-time projection of canonical proposal history.
///
/// Reference counts preserve repeated proposal ids across heights and across
/// the Gap/Proposed bands. `OrdMap` gives structural sharing, constant-time
/// snapshot cloning and deterministic logarithmic hostile-input work. The
/// consensus verifier does not consume this rebuildable projection.
#[derive(Clone, Debug)]
pub struct ProposalView {
    state: Arc<ProposalViewState>,
}

impl Default for ProposalView {
    fn default() -> Self {
        Self::from_counts(OrdMap::new(), None)
    }
}

impl ProposalView {
    fn from_counts(
        counts: OrdMap<ProposalKey, BandCounts>,
        receipt: Option<ProposalTransitionReceipt>,
    ) -> Self {
        Self {
            state: Arc::new(ProposalViewState { counts, receipt }),
        }
    }

    /// Constructs an unrelated exact view from materialized bands.
    ///
    /// This compatibility constructor intentionally creates no authenticated
    /// transition receipt; comparing it with another view therefore uses the
    /// exact fallback path.
    pub fn new(
        gap: impl IntoIterator<Item = ProposalShortId>,
        set: impl IntoIterator<Item = ProposalShortId>,
    ) -> Self {
        let mut counts: OrdMap<ProposalKey, BandCounts> = OrdMap::new();
        for id in ProposalKey::sorted_unique(set) {
            let mut next = counts.get(&id).copied().unwrap_or_default();
            next.proposed = 1;
            counts.insert(id, next);
        }
        for id in ProposalKey::sorted_unique(gap) {
            let mut next = counts.get(&id).copied().unwrap_or_default();
            next.gap = 1;
            counts.insert(id, next);
        }
        Self::from_counts(counts, None)
    }

    /// Iterates ids with at least one occurrence in the Gap band.
    pub fn gap_ids(&self) -> impl Iterator<Item = ProposalShortId> + '_ {
        self.state
            .counts
            .iter()
            .filter(|(_, counts)| counts.gap != 0)
            .map(|(id, _)| id.into_packed())
    }

    /// Iterates ids with at least one occurrence in the Proposed band.
    pub fn proposed_ids(&self) -> impl Iterator<Item = ProposalShortId> + '_ {
        self.state
            .counts
            .iter()
            .filter(|(_, counts)| counts.proposed != 0)
            .map(|(id, _)| id.into_packed())
    }

    /// Iterates the exact retained id universe in deterministic order.
    pub fn ids(&self) -> impl Iterator<Item = ProposalShortId> + '_ {
        self.state.counts.keys().map(|id| id.into_packed())
    }

    /// Returns true if an id has any occurrence in the Proposed band.
    pub fn contains_proposed(&self, id: &ProposalShortId) -> bool {
        self.state
            .counts
            .get(&ProposalKey::from_packed(id))
            .is_some_and(|counts| counts.proposed != 0)
    }

    /// Returns true if an id has any occurrence in the Gap band.
    pub fn contains_gap(&self, id: &ProposalShortId) -> bool {
        self.state
            .counts
            .get(&ProposalKey::from_packed(id))
            .is_some_and(|counts| counts.gap != 0)
    }

    fn same_identity(&self, state: &Weak<ProposalViewState>) -> bool {
        // The receipt's `Weak` keeps the predecessor control block allocated,
        // so an unrelated `Arc` cannot reuse this address while the receipt
        // exists.  Identity comparison therefore needs no fallible upgrade or
        // numeric generation counter.
        state.as_ptr() == Arc::as_ptr(&self.state)
    }

    fn key_position(&self, id: ProposalKey) -> HeightPosition {
        Self::position_in_counts(&self.state.counts, id)
    }

    fn position_in_counts(
        counts: &OrdMap<ProposalKey, BandCounts>,
        id: ProposalKey,
    ) -> HeightPosition {
        counts.get(&id).map_or(HeightPosition::Outside, |counts| {
            if counts.proposed != 0 {
                HeightPosition::Proposed
            } else if counts.gap != 0 {
                HeightPosition::Gap
            } else {
                HeightPosition::Outside
            }
        })
    }

    fn same_position(&self, other: &Self, id: ProposalKey) -> bool {
        self.key_position(id) == other.key_position(id)
    }

    fn try_for_each_changed_key_from<E>(
        &self,
        predecessor: &Self,
        mut visit: impl FnMut(ProposalKey) -> Result<(), E>,
    ) -> Result<ProposalTransitionSource, E> {
        if let Some(receipt) = &self.state.receipt
            && predecessor.same_identity(&receipt.predecessor)
        {
            for id in receipt.changed.iter() {
                visit(*id)?;
            }
            return Ok(ProposalTransitionSource::AuthenticatedSparse);
        }

        let mut old = predecessor.state.counts.iter().peekable();
        let mut new = self.state.counts.iter().peekable();
        loop {
            let old_key = old.peek().map(|(id, _)| *id);
            let new_key = new.peek().map(|(id, _)| *id);
            let candidate = match (old_key, new_key) {
                (Some(old_id), Some(new_id)) => match old_id.cmp(new_id) {
                    Ordering::Less => {
                        old.next();
                        old_id
                    }
                    Ordering::Equal => {
                        old.next();
                        new.next();
                        old_id
                    }
                    Ordering::Greater => {
                        new.next();
                        new_id
                    }
                },
                (Some(old_id), None) => {
                    old.next();
                    old_id
                }
                (None, Some(new_id)) => {
                    new.next();
                    new_id
                }
                (None, None) => break,
            };
            if !predecessor.same_position(self, *candidate) {
                visit(*candidate)?;
            }
        }
        Ok(ProposalTransitionSource::ExactFallback)
    }

    /// Visits every and only id whose externally observed position differs.
    ///
    /// A sparse receipt is accepted only for its exact predecessor identity.
    /// Otherwise two deterministic ordered key streams are merged, avoiding a
    /// second materialized full-window set. The visitor owns any fallible
    /// output allocation; each emitted packed id has an exact 10-byte backing.
    pub fn try_for_each_changed_from<E>(
        &self,
        predecessor: &Self,
        mut visit: impl FnMut(ProposalShortId) -> Result<(), E>,
    ) -> Result<ProposalTransitionSource, E> {
        self.try_for_each_changed_key_from(predecessor, |id| visit(id.into_packed()))
    }
}

/// Invalid trusted proposal-window configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProposalTableError {
    /// The closest distance is zero or exceeds the farthest distance.
    InvalidWindow {
        /// First legal proposal distance declared by consensus.
        closest: BlockNumber,
        /// Last legal proposal distance declared by consensus.
        farthest: BlockNumber,
    },
}

impl fmt::Display for ProposalTableError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidWindow { closest, farthest } => write!(
                formatter,
                "invalid proposal window: closest={closest}, farthest={farthest}"
            ),
        }
    }
}

impl std::error::Error for ProposalTableError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingTransition {
    Clean,
    SuccessorInsert(BlockNumber),
    Rebuild,
}

#[derive(Debug)]
struct FinalizedIdentity {
    number: BlockNumber,
    state: Weak<ProposalViewState>,
}

/// Canonical primitive proposal ids indexed by block height.
#[derive(Debug)]
pub struct ProposalTable {
    table: BTreeMap<BlockNumber, Box<[ProposalKey]>>,
    proposal_window: ProposalWindow,
    finalized: Option<FinalizedIdentity>,
    pending: PendingTransition,
}

impl ProposalTable {
    /// Creates a table from a validated consensus proposal window.
    pub fn new(proposal_window: ProposalWindow) -> Result<Self, ProposalTableError> {
        if proposal_window.closest() == 0 || proposal_window.closest() > proposal_window.farthest()
        {
            return Err(ProposalTableError::InvalidWindow {
                closest: proposal_window.closest(),
                farthest: proposal_window.farthest(),
            });
        }
        Ok(Self {
            proposal_window,
            table: BTreeMap::new(),
            finalized: None,
            pending: PendingTransition::Rebuild,
        })
    }

    /// Inserts or replaces primitive ids at one canonical height.
    pub fn insert(
        &mut self,
        number: BlockNumber,
        ids: impl IntoIterator<Item = ProposalShortId>,
    ) -> bool {
        let ids = ProposalKey::sorted_unique(ids);
        let absent = self.table.insert(number, ids.into_boxed_slice()).is_none();
        self.pending = match (&self.finalized, self.pending, absent) {
            (Some(finalized), PendingTransition::Clean, true)
                if finalized.number.checked_add(1) == Some(number) =>
            {
                PendingTransition::SuccessorInsert(number)
            }
            _ => PendingTransition::Rebuild,
        };
        absent
    }

    /// Removes primitive ids at one height and invalidates sparse succession.
    pub fn remove(&mut self, number: BlockNumber) -> bool {
        let removed = self.table.remove(&number).is_some();
        if removed {
            self.pending = PendingTransition::Rebuild;
        }
        removed
    }

    fn height_position(
        proposal_window: ProposalWindow,
        tip: BlockNumber,
        height: BlockNumber,
    ) -> HeightPosition {
        // TwoPhaseCommitVerifier reaches genesis but stops before collecting
        // that header's proposals. The rebuildable tx-pool projection must
        // therefore never turn height zero into commit evidence.
        if height == 0 {
            return HeightPosition::Outside;
        }
        let Some(candidate) = tip.checked_add(1) else {
            return HeightPosition::Outside;
        };
        if candidate <= proposal_window.closest() {
            return if height <= tip {
                HeightPosition::Gap
            } else {
                HeightPosition::Outside
            };
        }
        let start = candidate.saturating_sub(proposal_window.farthest());
        let end = candidate.saturating_sub(proposal_window.closest());
        if (start..=end).contains(&height) {
            HeightPosition::Proposed
        } else if height > end && height <= tip {
            HeightPosition::Gap
        } else {
            HeightPosition::Outside
        }
    }

    fn adjust(
        counts: &mut OrdMap<ProposalKey, BandCounts>,
        id: ProposalKey,
        position: HeightPosition,
        add: bool,
    ) -> bool {
        if matches!(position, HeightPosition::Outside) {
            return true;
        }
        let mut next = counts.get(&id).copied().unwrap_or_default();
        let coordinate = match position {
            HeightPosition::Proposed => &mut next.proposed,
            HeightPosition::Gap => &mut next.gap,
            HeightPosition::Outside => return true,
        };
        *coordinate = if add {
            let Some(value) = coordinate.checked_add(1) else {
                return false;
            };
            value
        } else {
            let Some(value) = coordinate.checked_sub(1) else {
                return false;
            };
            value
        };
        if next.is_empty() {
            counts.remove(&id);
        } else {
            counts.insert(id, next);
        }
        true
    }

    fn rebuild_view(&self, number: BlockNumber) -> ProposalView {
        let mut counts = OrdMap::new();
        for (&height, ids) in &self.table {
            let position = Self::height_position(self.proposal_window, number, height);
            for id in ids {
                // There cannot be more occurrences than distinct u64 heights;
                // an overflow would require u64::MAX + 1 table entries.
                let inserted = Self::adjust(&mut counts, *id, position, true);
                debug_assert!(inserted, "distinct height count is representable");
            }
        }
        ProposalView::from_counts(counts, None)
    }

    fn successor_view(&self, origin: &ProposalView, number: BlockNumber) -> Option<ProposalView> {
        let finalized = self.finalized.as_ref()?;
        if finalized.number.checked_add(1) != Some(number)
            || self.pending != PendingTransition::SuccessorInsert(number)
            || !origin.same_identity(&finalized.state)
        {
            return None;
        }
        let old_number = finalized.number;
        let old_candidate = old_number.checked_add(1)?;
        let new_candidate = number.checked_add(1)?;
        let mut heights = [
            old_number,
            number,
            old_candidate.saturating_sub(self.proposal_window.farthest()),
            old_candidate.saturating_sub(self.proposal_window.closest()),
            new_candidate.saturating_sub(self.proposal_window.farthest()),
            new_candidate.saturating_sub(self.proposal_window.closest()),
        ];
        heights.sort_unstable();

        let mut counts = origin.state.counts.clone();
        let mut touched = Vec::new();
        let mut previous_height = None;
        for height in heights {
            if previous_height == Some(height) {
                continue;
            }
            previous_height = Some(height);
            let old_position = Self::height_position(self.proposal_window, old_number, height);
            let new_position = Self::height_position(self.proposal_window, number, height);
            if old_position == new_position {
                continue;
            }
            if let Some(ids) = self.table.get(&height) {
                for id in ids {
                    touched.push(*id);
                    if !Self::adjust(&mut counts, *id, old_position, false)
                        || !Self::adjust(&mut counts, *id, new_position, true)
                    {
                        return None;
                    }
                }
            }
        }
        touched.sort_unstable();
        touched.dedup();

        touched.retain(|id| {
            origin.key_position(*id) != ProposalView::position_in_counts(&counts, *id)
        });
        Some(ProposalView::from_counts(
            counts,
            Some(ProposalTransitionReceipt {
                predecessor: Arc::downgrade(&origin.state),
                changed: touched,
            }),
        ))
    }

    fn prune(&mut self, number: BlockNumber) {
        let Some(candidate) = number.checked_add(1) else {
            self.table.clear();
            return;
        };
        let proposal_start = candidate.saturating_sub(self.proposal_window.farthest());
        if proposal_start > 0 {
            self.table = self.table.split_off(&proposal_start.max(1));
        }
    }

    /// Advances the exact immutable projection for an installed canonical tip.
    ///
    /// Ordinary successors update only changing height classes. Any mutation,
    /// identity mismatch, discontinuity or terminal height rebuilds exactly
    /// from the bounded primitive table and emits no sparse receipt.
    pub fn finalize(&mut self, origin: &ProposalView, number: BlockNumber) -> ProposalView {
        let next = self
            .successor_view(origin, number)
            .unwrap_or_else(|| self.rebuild_view(number));

        self.prune(number);
        self.finalized = Some(FinalizedIdentity {
            number,
            state: Arc::downgrade(&next.state),
        });
        self.pending = PendingTransition::Clean;
        ckb_logger::trace!(
            "[proposal_finalize] number {} retained heights {} distinct ids {}",
            number,
            self.table.len(),
            next.state.counts.len()
        );
        next
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeightPosition {
    Proposed,
    Gap,
    Outside,
}
