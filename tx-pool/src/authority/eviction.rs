//! Unified capacity eviction over `AuthorityState`'s global lower-bound index.
//!
//! `EvictionRank::lower_bound` is maintained incrementally in `AuthorityState` for
//! every accepted transaction. Because an entry's actual eviction rank (using
//! its true proposal status and complete descendant closure) is always `>=` its
//! singleton lower bound, scanning ascending lower bounds and stopping as soon
//! as `bound >= winner` finds the exact minimum eviction rank in sub-linear time
//! for both leaf admissions and late-parent admissions.

use super::{
    budget::Amount,
    membership::Aggregate,
    model::{Accepted, Entry, Error, Status},
    state::{AuthorityState, FastHashMap},
};
use crate::{constants::MAX_POOL_MUTATION_CANDIDATES, error::Reject};
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{FeeRate, tx_pool::get_transaction_weight},
    packed::Byte32,
    prelude::Unpack,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Smaller ranks are evicted first: pending before gap before proposed, then
/// lower fee rate, fewer descendants, earlier arrival and smaller hash.
/// Capacity trimming selects the minimum; packing's cycle fallback retains the
/// maximum among eligible package roots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct EvictionRank {
    pub(super) status: Status,
    pub(super) fee: FeeRate,
    pub(super) descendants: usize,
    pub(super) arrival: u64,
    pub(super) hash: [u8; 32],
}

pub(super) fn own_fee_rate(value: &Accepted) -> FeeRate {
    FeeRate::calculate(
        value.fee,
        get_transaction_weight(value.size, value.scripts.cycles()),
    )
}

impl EvictionRank {
    /// Mathematically guaranteed lower bound for an accepted entry's true rank.
    /// Proposal transitions and descendant growth can only raise this value.
    pub(super) fn lower_bound(owner: &Entry) -> Option<Self> {
        Some(Self {
            status: Status::Pending,
            fee: own_fee_rate(owner.accepted()?),
            descendants: 1,
            arrival: owner.arrival,
            hash: owner.hash_bytes(),
        })
    }

    pub(super) fn for_entry(
        entry: &Entry,
        descendants: Aggregate,
        snapshot: &Snapshot,
    ) -> Result<Self, Error> {
        let value = entry.accepted().ok_or(Error::Stale)?;
        Ok(Self::for_accepted(
            value,
            value.status(snapshot),
            descendants,
            entry.arrival,
            entry.transaction.hash_ref(),
        ))
    }

    pub(super) fn for_accepted(
        value: &Accepted,
        status: Status,
        descendants: Aggregate,
        arrival: u64,
        hash: &Byte32,
    ) -> Self {
        let descendants_rate = FeeRate::calculate(
            descendants.fee(),
            get_transaction_weight(descendants.bytes, descendants.cycles),
        );
        Self {
            status,
            fee: own_fee_rate(value).max(descendants_rate),
            descendants: descendants.count,
            arrival,
            hash: hash.unpack(),
        }
    }
}

fn protected_eviction(candidate_rate: FeeRate) -> Error {
    Reject::Full(format!(
        "the fee_rate for this transaction is: {candidate_rate}"
    ))
    .into()
}

/// Select complete victim closures directly from `AuthorityState` until every
/// over-limit coordinate in `excess` is released.
///
/// `removed` initially contains RBF victims and is extended in place with all
/// capacity-evicted victims. Protected ancestors (including `candidate`) are
/// never bypassed to evict a higher-ranked transaction; if a protected ancestor
/// has the lowest rank, admission is rejected with `Reject::Full`.
pub(super) fn select_victims(
    state: &AuthorityState,
    candidate: &Entry,
    candidate_ancestors: &HashSet<[u8; 32]>,
    late_entries: &HashMap<[u8; 32], Arc<Entry>>,
    late_descendants: &HashSet<[u8; 32]>,
    removed: &mut HashSet<[u8; 32]>,
    mut excess: Amount,
) -> Result<(), Error> {
    if excess == Amount::default() {
        return Ok(());
    }
    let snapshot = state.snapshot.as_ref();
    let candidate_hash = candidate.hash_bytes();
    let candidate_accepted = candidate.accepted().ok_or(Error::Stale)?;
    let candidate_own = Aggregate::one(candidate_accepted);
    let candidate_own_rate = own_fee_rate(candidate_accepted);

    // Roots whose closure cannot fit in MAX_POOL_MUTATION_CANDIDATES stay
    // unaffordable across later rounds: evicting a descendant moves it into
    // `removed`, so `|removed ∪ late_descendants ∪ closure|` never shrinks.
    let mut skipped = HashSet::<[u8; 32]>::new();

    // Base count of `|removed ∪ late_descendants|`, updated as `removed` grows.
    let mut base_touched = removed.len().saturating_add(
        late_descendants
            .iter()
            .filter(|hash| !removed.contains(*hash))
            .count(),
    );

    let add_owner = |total: Aggregate, hash: &[u8; 32]| -> Result<Aggregate, Error> {
        let entry = state.owners.get(hash).ok_or(Error::Stale)?;
        total.add(Aggregate::one(entry.accepted().ok_or(Error::Stale)?))
    };

    let compute_candidate_rank = |removed: &HashSet<[u8; 32]>| -> Result<EvictionRank, Error> {
        let total = late_descendants
            .iter()
            .filter(|hash| !removed.contains(*hash))
            .try_fold(candidate_own, add_owner)?;
        EvictionRank::for_entry(candidate, total, snapshot)
    };

    let mut candidate_rank = compute_candidate_rank(removed)?;

    // Track exact virtual descendant aggregates for every protected ancestor in
    // O(1) per eviction round without walking unbounded descendant subtrees.
    let mut protected_totals: FastHashMap<[u8; 32], Aggregate> =
        FastHashMap::with_capacity_and_hasher(candidate_ancestors.len(), Default::default());
    if !candidate_ancestors.is_empty() {
        for &anc in candidate_ancestors {
            if anc == candidate_hash {
                continue;
            }
            let total = state.descendant_aggregate(&anc)?.add(candidate_own)?;
            protected_totals.insert(anc, total);
        }
        if !protected_totals.is_empty() {
            if !removed.contains(&candidate_hash)
                && let Some(prev_accepted) =
                    state.owners.get(&candidate_hash).and_then(|e| e.accepted())
            {
                let prev_own = Aggregate::one(prev_accepted);
                for anc in state.strict_ancestors(&candidate_hash) {
                    if let Some(total) = protected_totals.get_mut(&anc) {
                        *total = total.checked_sub(prev_own)?;
                    }
                }
            }
            for victim in removed.iter() {
                let Some(victim_accepted) = state.owners.get(victim).and_then(|e| e.accepted())
                else {
                    continue;
                };
                let victim_own = Aggregate::one(victim_accepted);
                for anc in state.strict_ancestors(victim) {
                    if let Some(total) = protected_totals.get_mut(&anc) {
                        *total = total.checked_sub(victim_own)?;
                    }
                }
            }
            for desc in late_descendants {
                if removed.contains(desc) || *desc == candidate_hash {
                    continue;
                }
                let desc_own = Aggregate::one(
                    state
                        .owners
                        .get(desc)
                        .ok_or(Error::Stale)?
                        .accepted()
                        .ok_or(Error::Stale)?,
                );
                let old_anc = state.strict_ancestors(desc);
                for (&anc, total) in &mut protected_totals {
                    if anc != *desc && !old_anc.contains(&anc) {
                        *total = total.add(desc_own)?;
                    }
                }
            }
        }
    }

    enum SelectedVictim {
        Leaf([u8; 32]),
        Family(Vec<[u8; 32]>),
    }

    while excess != Amount::default() {
        let mut winner = candidate_rank;
        let mut selected: Option<SelectedVictim> = None;
        let remaining_budget = MAX_POOL_MUTATION_CANDIDATES.saturating_sub(removed.len());

        for bound in &state.eviction_bounds {
            if bound >= &winner {
                break;
            }
            if bound.hash == candidate_hash
                || removed.contains(&bound.hash)
                || skipped.contains(&bound.hash)
            {
                continue;
            }
            let Some(root) = state.owners.get(&bound.hash) else {
                return Err(Error::Fault("eviction bound missing owner"));
            };
            let root_accepted = root.accepted().ok_or(Error::Stale)?;
            // `status` is the leading field of `EvictionRank`: an owner with a
            // strictly higher proposal status than the current winner cannot
            // improve it, so skip descendant traversal and allocation altogether.
            if root_accepted.status(snapshot) > winner.status {
                continue;
            }
            if let Some(&total) = protected_totals.get(&bound.hash) {
                let rank = EvictionRank::for_entry(root, total, snapshot)?;
                if rank < winner {
                    winner = rank;
                    selected = None;
                }
                continue;
            }

            if !state.children.contains_key(&bound.hash) {
                let extra_touched = usize::from(!late_descendants.contains(&bound.hash));
                if remaining_budget == 0
                    || base_touched.saturating_add(extra_touched) > MAX_POOL_MUTATION_CANDIDATES
                {
                    skipped.insert(bound.hash);
                    continue;
                }
                let total = Aggregate::one(root_accepted);
                let rank = EvictionRank::for_entry(root, total, snapshot)?;
                if rank < winner {
                    winner = rank;
                    selected = Some(SelectedVictim::Leaf(bound.hash));
                }
                continue;
            }

            let live_agg = state.descendant_aggregate(&bound.hash)?;
            if live_agg.count.saturating_sub(removed.len()) > remaining_budget {
                skipped.insert(bound.hash);
                continue;
            }
            if removed.is_empty() {
                if late_descendants.is_empty()
                    && base_touched.saturating_add(live_agg.count) > MAX_POOL_MUTATION_CANDIDATES
                {
                    skipped.insert(bound.hash);
                    continue;
                }
                let rank = EvictionRank::for_entry(root, live_agg, snapshot)?;
                if rank >= winner {
                    continue;
                }
            }

            let Some(family) = state.descendants_bounded(&bound.hash, removed, remaining_budget)?
            else {
                skipped.insert(bound.hash);
                continue;
            };
            let extra_touched = if late_descendants.is_empty() {
                family.len()
            } else {
                family
                    .iter()
                    .filter(|hash| !late_descendants.contains(*hash))
                    .count()
            };
            if base_touched.saturating_add(extra_touched) > MAX_POOL_MUTATION_CANDIDATES {
                skipped.insert(bound.hash);
                continue;
            }

            let total = if removed.is_empty() {
                live_agg
            } else {
                family.iter().try_fold(Aggregate::default(), add_owner)?
            };
            let rank = EvictionRank::for_entry(root, total, snapshot)?;
            if rank < winner {
                winner = rank;
                selected = Some(SelectedVictim::Family(family));
            }
        }

        let Some(victim) = selected else {
            return Err(protected_eviction(candidate_own_rate));
        };
        let mut late_changed = false;
        let mut mark_removed = |hash: [u8; 32]| -> Result<(), Error> {
            // Refund the exact projected charge that fed `excess`: a late child
            // counted its updated parent-edge charge in `evaluate_admission` and
            // will not be installed once evicted.
            let entry = late_entries
                .get(&hash)
                .or_else(|| state.owners.get(&hash))
                .ok_or(Error::Stale)?;
            excess = excess.positive_difference(entry.charge());
            if removed.insert(hash) {
                let is_late = late_descendants.contains(&hash);
                if is_late {
                    late_changed = true;
                } else {
                    base_touched = base_touched.saturating_add(1);
                }
                if !protected_totals.is_empty() {
                    let victim_own = Aggregate::one(
                        state
                            .owners
                            .get(&hash)
                            .ok_or(Error::Stale)?
                            .accepted()
                            .ok_or(Error::Stale)?,
                    );
                    if is_late {
                        for total in protected_totals.values_mut() {
                            *total = total.checked_sub(victim_own)?;
                        }
                    } else {
                        for anc in state.strict_ancestors(&hash) {
                            if let Some(total) = protected_totals.get_mut(&anc) {
                                *total = total.checked_sub(victim_own)?;
                            }
                        }
                    }
                }
            }
            Ok(())
        };
        match victim {
            SelectedVictim::Leaf(hash) => mark_removed(hash)?,
            SelectedVictim::Family(family) => {
                for hash in family {
                    mark_removed(hash)?;
                }
            }
        }
        if late_changed {
            candidate_rank = compute_candidate_rank(removed)?;
        }
    }

    Ok(())
}
