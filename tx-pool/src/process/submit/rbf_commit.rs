//! The write-locked accepted-pool transaction family.
//!
//! RBF, final liveness, ancestry and capacity policy are compiled into one
//! immutable [`PoolMutationPlan`]. The pre-pool handoff is completed while
//! accepted membership is unchanged; only then does total PoolMap Apply move
//! the prevalidated entries. Stable effects are journaled before the lock is
//! released. Entry and verification orchestration lives in `super`.

use crate::component::entry::TxEntry;
use crate::component::pool_map::{
    AppliedPoolMutation, PoolMutationPlan, RemovalCause, RemovedPoolEntry, Status,
};
use crate::component::pre_pool::{DependencyKey, PipelineRawTx, PrePoolKernel, PrePoolSource};
use crate::error::Reject;
use crate::pool::TxPool;
use crate::pool::rbf::RbfCheck;
use crate::service::TxPoolService;
use crate::service::effects::{EffectBatch, TxPoolEffect};
use crate::util::time_relative_verify;
use ckb_logger::debug;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::core::cell::ResolvedTransaction;
use ckb_types::core::error::OutPointError;
use ckb_types::packed::{Byte32, ProposalShortId};
use std::collections::HashSet;
use std::sync::Arc;

use crate::process::{get_tx_status, status_to_verify_env};

#[cfg(test)]
#[path = "../tests/rbf_commit_seam.rs"]
mod test_seam;

/// Outcome of `try_submit_entry`, carried as one side-effect envelope across
/// the tx-pool write-lock boundary.
pub(crate) struct SubmitEntryOutcome {
    pub(crate) result: Result<(), Reject>,
    /// Terminal removals whose reject callbacks run outside the lock.
    pub(crate) reject_events: Vec<(TxEntry, Reject)>,
    /// Successful accepted callback, also dispatched outside the lock.
    pub(crate) accept_event: Option<(TxEntry, Status)>,
    /// Exact accepted entries moved by the immutable plan.
    removals: Vec<RemovedPoolEntry>,
}

impl SubmitEntryOutcome {
    /// Minimal assembler refresh set for this committed pool transaction.
    /// A replacement can remove a Proposed entry while inserting a Pending
    /// one, so the new entry alone is not a complete template delta.
    pub(crate) fn block_assembler_statuses(&self) -> HashSet<Status> {
        if self.result.is_err() {
            return HashSet::new();
        }
        let mut statuses = self
            .accept_event
            .iter()
            .map(|(_, status)| *status)
            .collect::<HashSet<_>>();
        statuses.extend(self.removals.iter().map(|item| item.status));
        statuses
    }
}

impl TxPoolService {
    /// Conflict history is optional under transaction/capacity pressure, but
    /// a structural transition error belongs to the enclosing authoritative
    /// defect boundary and must never masquerade as a best-effort drop.
    pub(crate) fn retain_optional_conflict(
        &self,
        kernel: &mut PrePoolKernel,
        raw: PipelineRawTx,
        owner: PrePoolSource,
        keys: std::collections::BTreeSet<DependencyKey>,
        expires_at: Option<u64>,
        context: &'static str,
    ) {
        let hash = raw.tx.hash();
        match kernel.retain_conflict(raw, owner, keys, expires_at) {
            Ok(_) => {}
            Err(error) if error.is_capacity_rejection() || error.is_transaction_rejection() => {
                debug!("dropping optional conflict history for {hash} in {context}: {error:?}");
            }
            Err(error) => panic!("{context}: {error:?}"),
        }
    }

    pub(crate) fn planned_unavailable_parent_hashes(
        &self,
        plan: &PoolMutationPlan,
        snapshot: &Snapshot,
    ) -> HashSet<Byte32> {
        plan.removals
            .iter()
            .map(|removal| removal.hash.clone())
            .filter(|hash| !snapshot.transaction_exists(hash))
            .collect()
    }

    /// Apply the pre-pool side of an accepted-pool plan while the caller owns
    /// `TxPool -> PrePoolKernel`. This completes before total PoolMap Apply,
    /// so no fallible coordinator operation follows an accepted mutation.
    pub(crate) fn settle_kernel_for_pool_plan(
        &self,
        kernel: &mut crate::component::pre_pool::PrePoolKernel,
        tx_pool: &TxPool,
        entry: &TxEntry,
        plan: &PoolMutationPlan,
    ) {
        let removed_ids = plan
            .removals
            .iter()
            .map(|removal| removal.id.clone())
            .collect::<HashSet<_>>();
        let candidate_inputs = entry
            .transaction()
            .input_pts_iter()
            .map(|out_point| crate::util::compact_packed(&out_point))
            .collect::<HashSet<_>>();
        let released = plan
            .removals
            .iter()
            .filter_map(|removal| tx_pool.pool_map.get_by_id(&removal.id))
            .flat_map(|removed| removed.inner.transaction().input_pts_iter())
            .chain(entry.transaction().output_pts())
            .map(|out_point| crate::util::compact_packed(&out_point))
            .filter(|out_point| {
                if candidate_inputs.contains(out_point) {
                    return false;
                }
                if tx_pool
                    .pool_map
                    .out_point_index
                    .get_input_ref(out_point)
                    .is_some_and(|owner| !removed_ids.contains(owner))
                {
                    return false;
                }
                if out_point.tx_hash() == entry.transaction().hash() {
                    return entry
                        .transaction()
                        .output(out_point.index().into())
                        .is_some();
                }
                tx_pool
                    .pool_map
                    .get_by_hash(&out_point.tx_hash())
                    .is_some_and(|producer| !removed_ids.contains(&producer.id))
                    || tx_pool.snapshot().get_cell(out_point).is_some()
            })
            .map(crate::component::pre_pool::DependencyKey::Cell)
            .collect::<HashSet<_>>();

        kernel
            .remove_conflict_hash(&entry.transaction().hash())
            .unwrap_or_else(|error| panic!("planned conflict removal failed: {error:?}"));
        kernel.note_available(released);
        let epoch = self.pipeline.epoch.current().unwrap_or(0);
        for victim in plan
            .removals
            .iter()
            .filter(|removal| removal.cause == RemovalCause::Replacement)
        {
            let victim = tx_pool
                .pool_map
                .get_by_id(&victim.id)
                .expect("planned replacement victim remains present");
            let tx = victim.inner.transaction().clone();
            let keys = crate::component::pre_pool::conflict_dependency_keys(
                &tx,
                victim.inner.related_dep_out_points().cloned(),
            );
            let raw = crate::component::pre_pool::PipelineRawTx::new(
                tx,
                crate::tx_source::TxSource::Local,
                epoch,
            );
            let owner =
                crate::component::pre_pool::historical_source(crate::tx_source::TxSource::Local);
            self.retain_optional_conflict(
                kernel,
                raw,
                owner,
                keys,
                crate::component::pre_pool::historical_deadline(owner),
                "planned RBF history transition failed",
            );
        }
    }

    /// Build the complete read-only accepted-pool decision. RBF, final
    /// role-aware liveness, causal ancestry and both capacity budgets are
    /// decided before the first physical removal.
    pub(crate) fn plan_pool_mutation(
        &self,
        tx_pool: &TxPool,
        snapshot: &Arc<Snapshot>,
        pre_resolve_tip: Byte32,
        entry: &TxEntry,
    ) -> Result<PoolMutationPlan, Reject> {
        // check_rbf must be invoked in `write` lock to avoid concurrent issues.
        // It returns the direct conflicts plus their shared conflict closure
        // (post-ordered removal plan + membership set), computed in one
        // traversal.
        let RbfCheck {
            removal,
            removal_set,
        } = if tx_pool.enable_rbf() {
            tx_pool.check_rbf(snapshot, entry)?
        } else {
            // RBF is disabled but we found conflicts, return error here
            // after_process will put this tx into conflicts_pool
            let conflicted_outpoint = tx_pool.pool_map.find_conflict_outpoint(entry.transaction());
            if let Some(outpoint) = conflicted_outpoint {
                return Err(Reject::Resolve(OutPointError::Dead(outpoint)));
            }
            RbfCheck {
                removal: Vec::new(),
                removal_set: HashSet::new(),
            }
        };

        // Final liveness always uses the virtual post-RBF pool. This is both
        // the stale-verification check and the role-aware reader/spender rule.
        let status = check_rtx(tx_pool, snapshot, &entry.rtx, &removal_set)?;

        // If snapshot changed by context switch redo time-relative verify.
        let tip_hash = snapshot.tip_hash();
        if pre_resolve_tip != tip_hash {
            debug!(
                "submit_entry {} context changed. previous:{} now:{}",
                entry.proposal_short_id(),
                pre_resolve_tip,
                tip_hash
            );
            let tip_header = snapshot.tip_header();
            let tx_env = status_to_verify_env(status, tip_header);
            time_relative_verify(Arc::clone(snapshot), Arc::clone(&entry.rtx), tx_env)?;
        }

        tx_pool.pool_map.plan_mutation(
            entry.clone(),
            status,
            &removal,
            tx_pool.config.max_tx_pool_size,
            tx_pool.config.tx_pool_resident_size_budget(),
        )
    }

    /// Total application of one immutable pool plan. Effect payloads are
    /// derived from the applied plan, never progressively while deciding.
    pub(crate) fn apply_pool_mutation(
        &self,
        tx_pool: &mut TxPool,
        entry: &TxEntry,
        plan: PoolMutationPlan,
    ) -> (Vec<(TxEntry, Reject)>, Vec<RemovedPoolEntry>) {
        let AppliedPoolMutation { removals } = tx_pool.pool_map.apply_mutation(plan);
        let mut reject_events = Vec::with_capacity(removals.len());
        let mut removed_entries = Vec::with_capacity(removals.len());
        for applied in removals {
            let reject = match applied.cause {
                RemovalCause::Replacement => {
                    Reject::RBFRejected(format!("replaced by tx {}", entry.transaction().hash()))
                }
                RemovalCause::SizeLimit => crate::error::Reject::Full(format!(
                    "the fee_rate for this transaction is: {}",
                    applied.removed.entry.fee_rate()
                )),
            };
            reject_events.push((applied.removed.entry.clone(), reject));
            removed_entries.push(applied.removed);
        }
        (reject_events, removed_entries)
    }
    pub(crate) fn try_submit_entry_with_handoff<T>(
        &self,
        tx_pool: &mut TxPool,
        snapshot: Arc<Snapshot>,
        pre_resolve_tip: Byte32,
        entry: TxEntry,
        before_apply: impl FnOnce(&TxPool, &PoolMutationPlan) -> T,
    ) -> (SubmitEntryOutcome, Option<T>) {
        match self.plan_pool_mutation(tx_pool, &snapshot, pre_resolve_tip, &entry) {
            Err(reject) => (
                SubmitEntryOutcome {
                    result: Err(reject),
                    reject_events: Vec::new(),
                    accept_event: None,
                    removals: Vec::new(),
                },
                None,
            ),
            Ok(plan) => {
                let value = before_apply(tx_pool, &plan);
                let final_status = plan.status;
                let (reject_events, removals) = self.apply_pool_mutation(tx_pool, &entry, plan);
                (
                    SubmitEntryOutcome {
                        result: Ok(()),
                        reject_events,
                        accept_event: Some((entry, final_status)),
                        removals,
                    },
                    Some(value),
                )
            }
        }
    }

    /// Materialize the complete stable-state publication batch from the
    /// already validated/applied plan. The caller runs this inside the
    /// journal's bounded `try_apply_bounded` closure so state Apply and append
    /// remain one critical section without a carried reservation.
    pub(crate) fn prepare_submit_effects(
        &self,
        outcome: &mut SubmitEntryOutcome,
        mut extra_effects: Vec<TxPoolEffect>,
    ) -> Option<EffectBatch> {
        let mut effects = Vec::new();
        if let Some((entry, status)) = outcome.accept_event.take()
            && let Some(effect) = self.accepted_effect(entry, status)
        {
            effects.push(effect);
        }
        for (entry, reject) in std::mem::take(&mut outcome.reject_events) {
            effects.extend(self.rejected_effects(entry, reject));
        }
        effects.append(&mut extra_effects);
        EffectBatch::new(effects)
    }
}

fn check_rtx(
    tx_pool: &TxPool,
    snapshot: &Snapshot,
    rtx: &ResolvedTransaction,
    excluded: &HashSet<ProposalShortId>,
) -> Result<Status, Reject> {
    let short_id = rtx.transaction.proposal_short_id();
    let tx_status = get_tx_status(snapshot, &short_id);
    tx_pool
        .check_rtx_from_pool_excluding(rtx, excluded)
        .map(|_| tx_status)
}
