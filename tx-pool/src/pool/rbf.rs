//! Replace-by-fee rule checks.
//!
//! These [`TxPool`] methods validate an RBF replacement against the pool's
//! rules and compute the minimum replacement fee. Split out of `pool.rs`;
//! [`TxPool::check_rbf`] is the entry point.

use super::TxPool;
use crate::component::TxEntry;
use crate::component::pool_map::{
    ConflictClosure, PoolEntry, PoolMutationFault, PoolMutationPlanningError,
};
use crate::constants::MAX_POOL_MUTATION_CANDIDATES;
use crate::error::Reject;
use ckb_logger::error;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::{
    core::Capacity,
    packed::{OutPoint, ProposalShortId},
};
use std::collections::{HashMap, HashSet};

/// Upper bound on the number of RBF replacement candidates evaluated in one replacement.
///
/// Prevents an O(n) scan of the mempool when a large transaction conflicts with many
/// existing entries. 100 is the same order of magnitude as Bitcoin Core's replacement
/// candidate limit.
/// Outcome of a successful [`TxPool::check_rbf`]: all fields come from a
/// single [`PoolMap::conflict_closure`] traversal so that the commit
/// pre-validation and `process_rbf` can reuse them without re-walking the
/// descendants.
pub(crate) struct RbfCheck {
    /// Conflicts + descendants, post-ordered for removal.
    pub removal: Vec<ProposalShortId>,
    /// The same set as `removal`, for membership/count checks.
    pub removal_set: HashSet<ProposalShortId>,
}

impl TxPool {
    /// min_replace_fee = sum(replaced_txs.fee) + extra_rbf_fee
    ///
    /// `size` is the replacement transaction's serialized size in bytes. It is
    /// intentionally not a weight: the replacement threshold must use the same
    /// unit as `min_fee_rate` (shannons per kilo-bytes), which is calculated from
    /// size because cycles are not available at submission time. The
    /// kernel's verified-conflict gate also uses the size-based fee rate
    /// for provisional ordering: peer-declared cycles are not trusted before
    /// verification. That gate only affects scheduling and never lowers the
    /// replacement fee floor computed here.
    pub(super) fn calculate_min_replace_fee(
        &self,
        conflicts: &[&PoolEntry],
        size: u64,
    ) -> Option<Capacity> {
        let extra_rbf_fee = self.config.min_rbf_rate.fee(size);
        // don't account for duplicate txs
        let replaced_sum_fee = conflicts
            .iter()
            .map(|c| (c.id.clone(), c.inner.fee))
            .collect::<HashMap<_, _>>()
            .into_values()
            .try_fold(Capacity::zero(), |acc, x| acc.safe_add(x));
        let total_fee = replaced_sum_fee.and_then(|sum| sum.safe_add(extra_rbf_fee));
        match total_fee {
            Ok(res) => Some(res),
            Err(_) => {
                let fees = conflicts.iter().map(|c| c.inner.fee).collect::<Vec<_>>();
                error!(
                    "conflicts: {:?} replaced_sum_fee {:?} overflow by add {}",
                    conflicts.iter().map(|e| e.id.clone()).collect::<Vec<_>>(),
                    fees,
                    extra_rbf_fee
                );
                None
            }
        }
    }

    pub(crate) fn check_rbf(
        &self,
        snapshot: &Snapshot,
        entry: &TxEntry,
    ) -> Result<RbfCheck, PoolMutationPlanningError> {
        if !self.enable_rbf() {
            return Err(Reject::RBFRejected("RBF is disabled".to_string()).into());
        }
        let tx_inputs: Vec<OutPoint> = entry.transaction().input_pts_iter().collect();
        let conflict_ids = self.pool_map.find_conflict_tx(entry.transaction());

        if conflict_ids.is_empty() {
            return Ok(RbfCheck {
                removal: Vec::new(),
                removal_set: HashSet::new(),
            });
        }

        // Rule #1, the node has enabled RBF, which is checked by caller
        let mut conflicts = Vec::with_capacity(conflict_ids.len());
        for id in &conflict_ids {
            conflicts.push(
                self.get_pool_entry(id)
                    .ok_or(PoolMutationFault::MissingEntry("RBF conflict index"))?,
            );
        }

        // Rule #2, new tx don't contain any new unconfirmed inputs
        self.check_rbf_no_new_unconfirmed_inputs(&conflicts, &tx_inputs, snapshot)?;

        // Compute the conflict closure after rule #2 has passed, so a
        // rule #2 rejection does not pay for it. The candidate cap (rule
        // #5) is enforced inside the traversal itself, so an oversized
        // union costs at most `MAX_POOL_MUTATION_CANDIDATES` visited entries
        // regardless of pool population.
        let (removal, removal_set) = match self
            .pool_map
            .conflict_closure(&conflict_ids, MAX_POOL_MUTATION_CANDIDATES)
        {
            ConflictClosure::Exceeded { count_lower_bound } => {
                return Err(Reject::RBFRejected(format!(
                    "Tx conflict with too many txs, conflict txs count: >= {}, expect <= {}",
                    count_lower_bound, MAX_POOL_MUTATION_CANDIDATES,
                ))
                .into());
            }
            ConflictClosure::Complete {
                removal,
                removal_set,
            } => (removal, removal_set),
        };

        // Rule #5, ancestor-descendant overlap and no inputs from
        // descendants (the candidate count limit was just enforced by the
        // closure traversal above).
        let all_conflicted = self.check_rbf_descendants(&conflicts, &tx_inputs, &removal_set)?;

        // Check new tx does not use cell deps from conflicted txs
        self.check_rbf_no_conflict_cell_deps(&all_conflicted, entry)?;

        // Rule #3 & #4, new tx's fee must be higher than both conflicts and min_rbf_fee
        self.check_rbf_fee(&all_conflicted, entry)?;

        Ok(RbfCheck {
            removal,
            removal_set,
        })
    }

    /// RBF Rule #2: new tx must not contain any new unconfirmed inputs
    /// (all inputs must either be from the conflicted txs or already confirmed on-chain).
    fn check_rbf_no_new_unconfirmed_inputs(
        &self,
        conflicts: &[&PoolEntry],
        tx_inputs: &[OutPoint],
        snapshot: &Snapshot,
    ) -> Result<(), Reject> {
        let inputs_capacity = conflicts
            .iter()
            .map(|c| c.inner.transaction().inputs().len())
            .sum();
        let mut inputs = HashSet::with_capacity(inputs_capacity);
        for c in conflicts.iter() {
            inputs.extend(c.inner.transaction().input_pts_iter());
        }
        if tx_inputs
            .iter()
            .any(|pt| !inputs.contains(pt) && !snapshot.transaction_exists(&pt.tx_hash()))
        {
            return Err(Reject::RBFRejected(
                "new Tx contains unconfirmed inputs".to_string(),
            ));
        }
        Ok(())
    }

    /// RBF Rule #5: check that the number of replaced txs (conflicts + descendants)
    /// does not exceed MAX_POOL_MUTATION_CANDIDATES, that the new tx does not
    /// reference outputs of descendant txs as inputs, and that the new tx's
    /// ancestors do not overlap with the conflicted txs' descendants.
    ///
    /// `removal_set` is the complete conflict closure (conflicts + all
    /// descendants): the candidate cap was already enforced inside
    /// [`PoolMap::conflict_closure`] before this runs (the union's size is
    /// exactly what the previous per-conflict `calc_descendants`
    /// accumulation counted, shared descendants deduplicated either way),
    /// and the descendant-overlap condition over the union is equivalent to
    /// the per-conflict condition — `(∪Dᵢ) ∩ A ≠ ∅` holds iff some `Dᵢ ∩ A
    /// ≠ ∅`.
    ///
    /// Returns the full set of conflicted entries (direct conflicts + their descendants).
    fn check_rbf_descendants<'a>(
        &'a self,
        conflicts: &[&'a PoolEntry],
        tx_inputs: &[OutPoint],
        removal_set: &HashSet<ProposalShortId>,
    ) -> Result<Vec<&'a PoolEntry>, Reject> {
        let mut all_conflicted = conflicts.to_vec();
        let mut seen_ids: HashSet<ProposalShortId> =
            conflicts.iter().map(|c| c.id.clone()).collect();
        let mut ancestors: HashSet<ProposalShortId> =
            HashSet::with_capacity(tx_inputs.len().saturating_mul(2));
        // Include inputs in ancestor set. Kept separate from
        // `PoolMap::get_tx_ancestors`: that one also walks cell-dep parents,
        // which would broaden the disjointness check below and reject
        // replacements that are valid today.
        for input in tx_inputs {
            let parent_hash = input.tx_hash();
            let parent_id = ProposalShortId::from_tx_hash(&parent_hash);
            if self.pool_map.get_by_hash(&parent_hash).is_some() {
                ancestors.insert(parent_id.clone());
                ancestors.extend(self.pool_map.calc_ancestors(&parent_id));
            }
        }

        // Descendants only (excluding the direct conflicts): inputs that
        // spend a direct conflict's *own* output are already rejected by
        // rule #2 (which runs before this check), so excluding the roots
        // here preserves the exact semantics of the previous per-conflict
        // descendant walk, including when one conflict's root is itself a
        // descendant of another conflict.
        let mut descendants: HashSet<ProposalShortId> = HashSet::new();
        for id in removal_set {
            if seen_ids.contains(id) {
                continue;
            }
            descendants.insert(id.clone());
            let Some(entry) = self.get_pool_entry(id) else {
                continue;
            };
            // Check the more specific error first: the new tx is spending an
            // output that belongs to a descendant of the to-be-replaced tx.
            let hash = entry.inner.transaction().hash();
            if tx_inputs.iter().any(|pt| pt.tx_hash() == hash) {
                return Err(Reject::RBFRejected(
                    "new Tx contains inputs in descendants of to be replaced Tx".to_string(),
                ));
            }
            seen_ids.insert(id.clone());
            all_conflicted.push(entry);
        }

        // Then check the broader ancestor/descendant overlap.
        if !descendants.is_disjoint(&ancestors) {
            return Err(Reject::RBFRejected(
                "Tx ancestors have common with conflict Tx descendants".to_string(),
            ));
        }

        Ok(all_conflicted)
    }

    /// Check that the new tx does not reference any conflicted tx as a direct
    /// or dep-group-expanded cell dependency.
    pub(crate) fn check_rbf_no_conflict_cell_deps(
        &self,
        all_conflicted: &[&PoolEntry],
        entry: &TxEntry,
    ) -> Result<(), Reject> {
        let conflicted_hashes = all_conflicted
            .iter()
            .map(|conflicted| conflicted.inner.transaction().hash())
            .collect::<HashSet<_>>();
        if entry
            .related_dep_out_points()
            .any(|dep| conflicted_hashes.contains(&dep.tx_hash()))
        {
            return Err(Reject::RBFRejected(
                "new Tx contains cell deps from conflicts".to_string(),
            ));
        }
        Ok(())
    }

    /// RBF Rule #3 & #4: the new tx's fee must be higher than the total fee of
    /// all conflicted txs and must meet the minimum replacement fee rate.
    ///
    /// The minimum replacement fee uses the replacement tx's serialized size,
    /// consistent with `min_fee_rate`. See [`calculate_min_replace_fee`].
    fn check_rbf_fee(&self, all_conflicted: &[&PoolEntry], entry: &TxEntry) -> Result<(), Reject> {
        let fee = entry.fee;
        if let Some(min_replace_fee) =
            self.calculate_min_replace_fee(all_conflicted, entry.size as u64)
        {
            if fee < min_replace_fee {
                return Err(Reject::RBFRejected(format!(
                    "Tx's current fee is {}, expect it to >= {} to replace old txs",
                    fee, min_replace_fee,
                )));
            }
        } else {
            return Err(Reject::RBFRejected(
                "calculate_min_replace_fee failed".to_string(),
            ));
        }
        Ok(())
    }
}
