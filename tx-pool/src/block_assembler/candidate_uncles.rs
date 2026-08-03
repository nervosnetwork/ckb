use ckb_types::core::{BlockNumber, EpochExt, UncleBlockView};
use std::collections::{BTreeMap, HashSet, btree_map::Entry};

use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;

pub(crate) const MAX_CANDIDATE_UNCLES: usize = 128;
pub(crate) const MAX_PER_HEIGHT: usize = 10;

/// Monotonic version of candidate changes that can alter uncle selection.
///
/// Stale-candidate pruning is deliberately version-neutral: the paired chain
/// cut already proves that those candidates cannot occur in the template, and
/// every later chain change advances the independent chain source. Accepted
/// insertions and explicit removals do advance this version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct CandidateUncleVersion(u64);

impl CandidateUncleVersion {
    const INITIAL: Self = Self(0);

    fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CandidateUncleMutationError {
    SourceVersionExhausted,
}

/// Opaque read token for the exact candidate source observed by one build.
/// Callers can compare or join receipts but cannot manufacture a version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CandidateUncleSourceReceipt {
    version: CandidateUncleVersion,
}

/// Candidate uncles container
#[derive(Clone)]
pub struct CandidateUncles {
    map: BTreeMap<BlockNumber, HashSet<UncleBlockView>>,
    source_version: CandidateUncleVersion,
}

/// Read-only uncle selection produced from one candidate-cache snapshot.
///
/// Selection must not mutate the live cache: an optimistic template build may
/// later lose its publication token. Stale candidates are pruned only by the
/// matching successful template Apply.
pub(crate) struct PreparedUncles {
    selected: Vec<UncleBlockView>,
    prune: CandidateUnclePrune,
    source: CandidateUncleSourceReceipt,
}

/// Sealed cleanup capability produced only by uncle preparation.
///
/// It contains candidates proven stale by the same chain facts used for the
/// read receipt. Applying it cannot remove a candidate selected by that read.
pub(crate) struct CandidateUnclePrune {
    stale: Vec<UncleBlockView>,
}

impl PreparedUncles {
    pub(crate) fn into_parts(
        self,
    ) -> (
        Vec<UncleBlockView>,
        CandidateUnclePrune,
        CandidateUncleSourceReceipt,
    ) {
        (self.selected, self.prune, self.source)
    }
}

impl CandidateUncles {
    /// Construct new candidate uncles container
    pub fn new() -> CandidateUncles {
        CandidateUncles {
            map: BTreeMap::new(),
            source_version: CandidateUncleVersion::INITIAL,
        }
    }

    /// insert new candidate uncles
    /// If the map did not have this value present, true is returned.
    /// If the map did have this value present, false is returned.
    pub fn insert(&mut self, uncle: UncleBlockView) -> bool {
        // Compatibility adapter for the historical public container API. The
        // production topology consumes `try_insert` and therefore preserves
        // the typed counter-exhaustion outcome.
        self.try_insert(uncle).is_ok_and(|inserted| inserted)
    }

    pub(crate) fn try_insert(
        &mut self,
        uncle: UncleBlockView,
    ) -> Result<bool, CandidateUncleMutationError> {
        let number: BlockNumber = uncle.header().number();
        // Validate the target bucket before changing global capacity. The old
        // order evicted the lowest-height set first, then discovered that a
        // duplicate higher uncle (or a full target height) could not be
        // inserted. Replaying one rejected candidate could therefore erase
        // unrelated valid uncles at no residency cost.
        if self
            .map
            .get(&number)
            .is_some_and(|set| set.contains(&uncle) || set.len() >= MAX_PER_HEIGHT)
        {
            return Ok(false);
        }
        let mut evict = None;
        if self.len() >= MAX_CANDIDATE_UNCLES {
            let Some(first_key) = self.map.keys().next().copied() else {
                // `len` is derived from `map`, so this branch is
                // unconstructible through the type's private mutation API.
                return Ok(false);
            };
            if number > first_key {
                evict = Some(first_key);
            } else {
                return Ok(false);
            }
        }

        let next_version = self
            .source_version
            .next()
            .ok_or(CandidateUncleMutationError::SourceVersionExhausted)?;

        let set = self.map.entry(number).or_default();
        if set.len() < MAX_PER_HEIGHT {
            // `BlockView::uncles()` yields a slice into the complete block.
            // Copy only after all rejection checks so retained uncle bytes
            // are charged independently without taxing duplicate floods.
            if !set.insert(uncle.into_compact()) {
                return Ok(false);
            }
            if let Some(first_key) = evict {
                self.map.remove(&first_key);
            }
            self.source_version = next_version;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Returns the number of elements in the container.
    pub fn len(&self) -> usize {
        self.map.values().map(HashSet::len).sum()
    }

    /// Returns true if the container contains no elements.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns true if the container contains a value.
    pub fn contains(&self, uncle: &UncleBlockView) -> bool {
        let number: BlockNumber = uncle.header().number();
        self.map
            .get(&number)
            .map(|set| set.contains(uncle))
            .unwrap_or(false)
    }

    /// Gets an iterator over the values of the map, in order by block_number.
    pub fn values(&self) -> impl Iterator<Item = &UncleBlockView> {
        self.map.values().flat_map(HashSet::iter)
    }

    /// Consume the bounded container into compact candidate values. Reorg
    /// phase handoff uses this to release transaction-bearing detached blocks
    /// before retained recovery awaits.
    pub(crate) fn into_values(self) -> Vec<UncleBlockView> {
        self.map.into_values().flatten().collect()
    }

    /// Removes uncles from the container by specified uncle's number
    pub fn remove_by_number(&mut self, uncle: &UncleBlockView) -> bool {
        self.try_remove_by_number(uncle)
            .is_ok_and(|removed| removed)
    }

    pub(crate) fn try_remove_by_number(
        &mut self,
        uncle: &UncleBlockView,
    ) -> Result<bool, CandidateUncleMutationError> {
        let number: BlockNumber = uncle.header().number();

        if !self.map.get(&number).is_some_and(|set| set.contains(uncle)) {
            return Ok(false);
        }
        let next_version = self
            .source_version
            .next()
            .ok_or(CandidateUncleMutationError::SourceVersionExhausted)?;
        let removed = self.remove_without_version(uncle);
        if removed {
            self.source_version = next_version;
        }
        Ok(removed)
    }

    fn remove_without_version(&mut self, uncle: &UncleBlockView) -> bool {
        let number: BlockNumber = uncle.header().number();
        if let Entry::Occupied(mut entry) = self.map.entry(number) {
            let set = entry.get_mut();
            if set.remove(uncle) {
                if set.is_empty() {
                    entry.remove();
                }
                return true;
            }
        }
        false
    }

    /// Get uncles from snapshot and current states.
    // A block B1 is considered to be the uncle of another block B2 if all of the following conditions are met:
    // (1) they are in the same epoch, sharing the same difficulty;
    // (2) height(B2) > height(B1);
    // (3) B1's parent is either B2's ancestor or embedded in B2 or its ancestors as an uncle;
    // and (4) B2 is the first block in its chain to refer to B1.
    pub(crate) fn prepare_uncles(
        &self,
        snapshot: &Snapshot,
        current_epoch_ext: &EpochExt,
    ) -> PreparedUncles {
        let Some(candidate_number) = snapshot.tip_number().checked_add(1) else {
            return PreparedUncles {
                selected: Vec::new(),
                prune: CandidateUnclePrune { stale: Vec::new() },
                source: CandidateUncleSourceReceipt {
                    version: self.source_version,
                },
            };
        };
        let epoch_number = current_epoch_ext.number();
        let max_uncles_num = snapshot.consensus().max_uncles_num();
        let mut uncles: Vec<UncleBlockView> = Vec::with_capacity(max_uncles_num);
        let mut removed = Vec::new();

        for uncle in self.values() {
            if uncles.len() == max_uncles_num {
                break;
            }
            let parent_hash = uncle.header().parent_hash();
            let hash = uncle.hash();
            // we should keep candidate util next epoch
            if uncle.compact_target() != current_epoch_ext.compact_target()
                || uncle.epoch().number() != epoch_number
                || snapshot.is_main_chain(&hash)
                || snapshot.is_uncle(&hash)
            {
                // Wrong epoch/target, or already embedded/on the main
                // chain: stale — drop it so it stops occupying the
                // candidate budget.
                removed.push(uncle.clone());
            } else if uncle.number() < candidate_number
                && (uncles.iter().any(|u| u.hash() == parent_hash)
                    || snapshot.is_main_chain(&parent_hash)
                    || snapshot.is_uncle(&parent_hash))
            {
                uncles.push(uncle.clone());
            }
        }

        PreparedUncles {
            selected: uncles,
            prune: CandidateUnclePrune { stale: removed },
            source: CandidateUncleSourceReceipt {
                version: self.source_version,
            },
        }
    }

    /// Apply stale-candidate cleanup from a plan whose template publication
    /// token is still current. Removal is idempotent because another committed
    /// update may already have pruned the same bounded candidates.
    pub(crate) fn prune(&mut self, plan: CandidateUnclePrune) {
        for uncle in plan.stale {
            self.remove_without_version(&uncle);
        }
    }

    /// Exercise the complete successful-publication behavior from a
    /// cross-crate test without exposing the internal read-only Plan or its
    /// token-sensitive Apply primitives in production builds.
    #[cfg(feature = "internal")]
    #[doc(hidden)]
    pub fn prepare_and_commit_for_test(
        &mut self,
        snapshot: &Snapshot,
        current_epoch_ext: &EpochExt,
    ) -> Vec<UncleBlockView> {
        let (selected, prune, _source) = self
            .prepare_uncles(snapshot, current_epoch_ext)
            .into_parts();
        self.prune(prune);
        selected
    }
}

impl Default for CandidateUncles {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "tests/candidate_uncles.rs"]
mod tests;
