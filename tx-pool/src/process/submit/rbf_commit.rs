//! The write-locked accepted-pool transaction family.
//!
//! RBF, final liveness, ancestry and capacity policy are compiled into one
//! immutable [`PoolMutationPlan`]. The pre-pool handoff is completed while
//! accepted membership is unchanged; only then does total PoolMap Apply move
//! the prevalidated entries. Stable effects are journaled before the lock is
//! released. Entry and verification orchestration lives in `super`.

use crate::component::entry::TxEntry;
use crate::component::pool_map::{
    PoolMap, PoolMutationFault, PoolMutationPlan, PoolMutationPlanningError, PreparedPoolMutation,
    RemovalCause, Status,
};
use crate::component::pre_pool::{
    ConflictRetention, DependencyKey, ExternalCommitPlan, PipelineRawTx, PrePoolError,
    PrePoolFault, PrePoolKernel, PrePoolSource, ReadyCommitPlan, ReadyCommitSession,
};
use crate::error::Reject;
use crate::pool::TxPool;
use crate::pool::rbf::RbfCheck;
use crate::service::TxPoolService;
use crate::service::effects::{
    EffectBatch, EffectBuildError, EffectClass, EffectJournalError, TxPoolEffect,
};
use crate::tx_source::TxSource;
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
#[path = "../tests/rbf_commit_test_support.rs"]
mod test_support;

enum AdmissionHandoff<'authority> {
    Ready(ReadyCommitPlan<'authority>),
    External(ExternalCommitPlan<'authority>),
}

/// Read-only admission planning has exactly two failure domains. Transaction
/// policy rejects are public outcomes; kernel errors describe an invalidated
/// or inconsistent authority proof and must never be returned as policy.
pub(crate) enum AdmissionPlanningError {
    Policy(Reject),
    Kernel(PrePoolFault),
    Pool(PoolMutationFault),
    Effect(EffectBuildError),
}

/// Ready planning has one additional administrative outcome that external
/// Local/Recovery admission cannot produce. Keeping it outside
/// `AdmissionPlanningError` makes direct submission exhaustively free of a
/// fictitious peer-revocation branch.
pub(crate) enum ReadyAdmissionPlanningError {
    IngressRevoked,
    Admission(AdmissionPlanningError),
}

impl From<AdmissionPlanningError> for ReadyAdmissionPlanningError {
    fn from(error: AdmissionPlanningError) -> Self {
        Self::Admission(error)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum UnacceptedReadyCause<'a> {
    Policy(&'a Reject),
    IngressRevoked,
}

impl From<Reject> for AdmissionPlanningError {
    fn from(reject: Reject) -> Self {
        Self::Policy(reject)
    }
}

impl From<PrePoolError> for AdmissionPlanningError {
    fn from(error: PrePoolError) -> Self {
        Self::Kernel(error.into_unexpected_fault())
    }
}

impl AdmissionPlanningError {
    /// A Ready handoff can still encounter a bounded scheduling-policy limit
    /// (for example the globally capped conflict cohort). That is ordinary
    /// backpressure on the candidate, not evidence that either authority is
    /// inconsistent. Every other failure at this already-owned boundary is a
    /// structural fault or a stale capability and remains generation-fatal.
    fn from_ready_handoff(error: PrePoolError) -> Self {
        match error {
            PrePoolError::Public(error) => {
                Self::Policy(crate::component::pre_pool::pre_pool_reject(error))
            }
            error => Self::Kernel(error.into_unexpected_fault()),
        }
    }
}

impl From<PoolMutationPlanningError> for AdmissionPlanningError {
    fn from(error: PoolMutationPlanningError) -> Self {
        match error {
            PoolMutationPlanningError::Policy(reject) => Self::Policy(reject),
            PoolMutationPlanningError::Fault(fault) => Self::Pool(fault),
        }
    }
}

impl From<EffectBuildError> for AdmissionPlanningError {
    fn from(error: EffectBuildError) -> Self {
        Self::Effect(error)
    }
}

#[derive(Debug)]
pub(crate) enum AdmissionApplyError {
    Journal(EffectJournalError),
    Pool(PoolMutationFault),
}

/// One complete ordinary transition from the logical PrePool/Absent arm to
/// Accepted.  Every transaction-shaped, capacity, identity and publication
/// decision is finished before this value exists.  `apply_admission_plan` is
/// its only consumer.
pub(crate) struct AdmissionPlan<'authority, 'pool> {
    pool: PreparedPoolMutation<'pool>,
    handoff: AdmissionHandoff<'authority>,
    effects: Option<EffectBatch>,
    effect_class: EffectClass,
    block_assembler_statuses: HashSet<Status>,
}

/// Complete kernel-only settlement when final accepted planning rejects a
/// Ready owner.  The terminal record and every effect are fixed before the
/// journal predicate; Apply cannot call `park_*`/`fail_*` or return a policy
/// error.
pub(crate) struct FailedAdmissionPlan<'authority> {
    terminal: crate::component::pre_pool::FailedCommitPlan<'authority>,
    effects: Option<EffectBatch>,
    effect_class: EffectClass,
    peer_ban: Option<(ckb_network::PeerIndex, std::time::Duration)>,
}

impl FailedAdmissionPlan<'_> {
    pub(crate) fn effect_bytes(&self) -> usize {
        self.effects.as_ref().map_or(0, EffectBatch::charge_bytes)
    }

    pub(crate) fn effect_class(&self) -> EffectClass {
        self.effect_class
    }
}

impl AdmissionPlan<'_, '_> {
    pub(crate) fn effect_bytes(&self) -> usize {
        self.effects.as_ref().map_or(0, EffectBatch::charge_bytes)
    }

    pub(crate) fn effect_class(&self) -> EffectClass {
        self.effect_class
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
    ) -> Result<(), PrePoolError> {
        let hash = raw.tx.hash();
        match kernel.retain_conflict(raw, owner, keys, expires_at) {
            Ok(_) => Ok(()),
            Err(error) if error.is_optional_retention_rejection() => {
                debug!("dropping optional conflict history for {hash} in {context}: {error:?}");
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn planned_unavailable_parent_hashes(
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

    fn planned_available_dependencies(
        &self,
        pool_map: &PoolMap,
        snapshot: &Snapshot,
        entry: &TxEntry,
        plan: &PoolMutationPlan,
    ) -> HashSet<DependencyKey> {
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
        plan.removals
            .iter()
            .flat_map(|removal| removal.entry.transaction().input_pts_iter())
            .chain(entry.transaction().output_pts())
            .map(|out_point| crate::util::compact_packed(&out_point))
            .filter(|out_point| {
                if candidate_inputs.contains(out_point) {
                    return false;
                }
                if pool_map
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
                pool_map
                    .get_by_hash(&out_point.tx_hash())
                    .is_some_and(|producer| !removed_ids.contains(&producer.id))
                    || snapshot.get_cell(out_point).is_some()
            })
            .map(crate::component::pre_pool::DependencyKey::Cell)
            .collect()
    }

    fn planned_replacement_history(
        &self,
        plan: &PoolMutationPlan,
        epoch: u64,
    ) -> Vec<ConflictRetention> {
        plan.removals
            .iter()
            .filter(|removal| removal.cause == RemovalCause::Replacement)
            .map(|removal| {
                let tx = removal.entry.transaction().clone();
                let keys = crate::component::pre_pool::conflict_dependency_keys(
                    &tx,
                    removal.entry.related_dep_out_points().cloned(),
                );
                let raw = crate::component::pre_pool::PipelineRawTx::new(
                    tx,
                    crate::tx_source::TxSource::Local,
                    epoch,
                );
                let owner = crate::component::pre_pool::historical_source(
                    crate::tx_source::TxSource::Local,
                );
                ConflictRetention::new(
                    raw,
                    owner,
                    keys,
                    crate::component::pre_pool::historical_deadline(owner),
                )
            })
            .collect()
    }

    /// Build the complete read-only accepted-pool decision. RBF, final
    /// role-aware liveness, causal ancestry and both capacity budgets are
    /// decided before the first physical removal.
    pub(crate) fn prepare_pool_mutation<'pool>(
        &self,
        tx_pool: &'pool mut TxPool,
        snapshot: &Arc<Snapshot>,
        pre_resolve_tip: Byte32,
        entry: &TxEntry,
    ) -> Result<PreparedPoolMutation<'pool>, AdmissionPlanningError> {
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
                return Err(Reject::Resolve(OutPointError::Dead(outpoint)).into());
            }
            RbfCheck {
                removal: Vec::new(),
                removal_set: HashSet::new(),
            }
        };

        // Final liveness always uses the virtual post-RBF pool. This is both
        // the stale-verification check and the role-aware reader/spender rule.
        let status = check_rtx(
            tx_pool,
            snapshot,
            &entry.rtx,
            &removal_set,
            &pre_resolve_tip,
        )?;

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

        let max_tx_pool_size = tx_pool.config.max_tx_pool_size;
        let max_resident_size = tx_pool.config.tx_pool_resident_size_budget();
        Ok(tx_pool.pool_map.prepare_mutation(
            entry.clone(),
            status,
            &removal,
            max_tx_pool_size,
            max_resident_size,
        )?)
    }

    /// Build one exact immutable effect/template receipt from the same stable
    /// accepted generation as `plan`.  The accepted callback uses the final
    /// candidate stored in `PoolMutationPlan`, not the pre-ancestry input
    /// entry; removed callbacks observe the one pre-Apply pool generation
    /// rather than order-dependent intermediate removal statistics.
    fn planned_publication(
        &self,
        plan: &PoolMutationPlan,
        mut extra_effects: Vec<TxPoolEffect>,
    ) -> Result<(Option<EffectBatch>, HashSet<Status>), EffectBuildError> {
        let mut effects = Vec::new();
        if let Some(effect) = self.accepted_effect(plan.candidate.clone(), plan.status) {
            effects.push(effect);
        }
        let mut statuses = HashSet::from([plan.status]);
        for planned in &plan.removals {
            let reject = match planned.cause {
                RemovalCause::Replacement => Reject::RBFRejected(format!(
                    "replaced by tx {}",
                    plan.candidate.transaction().hash()
                )),
                RemovalCause::SizeLimit => crate::error::Reject::Full(format!(
                    "the fee_rate for this transaction is: {}",
                    planned.entry.fee_rate()
                )),
            };
            effects.extend(self.rejected_effects(planned.entry.clone(), reject)?);
            statuses.insert(planned.status);
        }
        effects.append(&mut extra_effects);
        Ok((EffectBatch::new(effects), statuses))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn plan_external_admission<'authority, 'pool>(
        &self,
        tx_pool: &'pool mut TxPool,
        kernel: &'authority mut PrePoolKernel,
        snapshot: Arc<Snapshot>,
        pre_resolve_tip: Byte32,
        entry: TxEntry,
        source: TxSource,
        original_peer: Option<ckb_network::PeerIndex>,
        epoch: u64,
    ) -> Result<AdmissionPlan<'authority, 'pool>, AdmissionPlanningError> {
        let pool = self.prepare_pool_mutation(tx_pool, &snapshot, pre_resolve_tip, &entry)?;
        let plan = pool.decision();
        let unavailable = self.planned_unavailable_parent_hashes(plan, &snapshot);
        let available = self.planned_available_dependencies(pool.pool(), &snapshot, &entry, plan);
        let history = self.planned_replacement_history(plan, epoch);
        let committed = HashSet::from([entry.transaction().hash()]);
        let handoff = kernel.plan_external_commit(&committed, &unavailable, available, history)?;
        let committed_ingress_peer = handoff
            .records()
            .first()
            .and_then(|record| record.raw.ingress_peer())
            .or(original_peer);
        let extra_effects = vec![TxPoolEffect::Relay(
            crate::service::TxVerificationResult::Ok {
                original_peer: committed_ingress_peer,
                tx_hash: entry.transaction().hash(),
            },
        )];
        let (effects, block_assembler_statuses) = self
            .planned_publication(plan, extra_effects)
            .map_err(AdmissionPlanningError::from)?;
        Ok(AdmissionPlan {
            pool,
            handoff: AdmissionHandoff::External(handoff),
            effects,
            effect_class: if matches!(source, TxSource::Remote { .. }) {
                EffectClass::Remote
            } else {
                EffectClass::Trusted
            },
            block_assembler_statuses,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn plan_ready_admission<'authority, 'pool>(
        &self,
        tx_pool: &'pool mut TxPool,
        session: &'authority mut ReadyCommitSession<'_>,
        snapshot: Arc<Snapshot>,
        pre_resolve_tip: Byte32,
        entry: TxEntry,
        epoch: u64,
    ) -> Result<AdmissionPlan<'authority, 'pool>, ReadyAdmissionPlanningError> {
        if session
            .ingress_peer()
            .is_some_and(|peer| self.relay.banned_peers.contains(peer))
        {
            return Err(ReadyAdmissionPlanningError::IngressRevoked);
        }
        let pool = self.prepare_pool_mutation(tx_pool, &snapshot, pre_resolve_tip, &entry)?;
        let plan = pool.decision();
        let unavailable = self.planned_unavailable_parent_hashes(plan, &snapshot);
        let available = self.planned_available_dependencies(pool.pool(), &snapshot, &entry, plan);
        let history = self.planned_replacement_history(plan, epoch);
        let handoff = session
            .plan_ready(&unavailable, available, history)
            .map_err(AdmissionPlanningError::from_ready_handoff)?;
        let settlement = handoff.settlement();
        // Source promotion changes the authoritative pre-pool owner without
        // rewriting the already verified payload. Publication capacity must
        // therefore follow the handoff owner, never the stale payload source.
        let effect_class = if matches!(settlement.winner.source, PrePoolSource::Remote(_)) {
            EffectClass::Remote
        } else {
            EffectClass::Trusted
        };
        let mut extra_effects = vec![TxPoolEffect::Relay(
            crate::service::TxVerificationResult::Ok {
                original_peer: settlement.winner.raw.ingress_peer(),
                tx_hash: entry.transaction().hash(),
            },
        )];
        let superseded_reject =
            Reject::RBFRejected(Self::SUPERSEDED_BY_HIGHER_FEE_CANDIDATE.to_string());
        for record in &settlement.superseded {
            if let Some(effect) = self.recent_reject_effect(record.hash.clone(), &superseded_reject)
            {
                extra_effects.push(effect);
            }
            if record.raw.ingress_peer().is_some() && superseded_reject.is_allowed_relay() {
                extra_effects.push(TxPoolEffect::Relay(
                    crate::service::TxVerificationResult::Reject {
                        tx_hash: record.hash.clone(),
                    },
                ));
            }
        }
        let (effects, block_assembler_statuses) = self
            .planned_publication(plan, extra_effects)
            .map_err(AdmissionPlanningError::from)?;
        Ok(AdmissionPlan {
            pool,
            handoff: AdmissionHandoff::Ready(handoff),
            effects,
            effect_class,
            block_assembler_statuses,
        })
    }

    /// The sole ordinary cross-partition Apply.  Journal capacity is checked
    /// against the already materialized exact batch before either owner moves.
    pub(crate) fn apply_admission_plan(
        &self,
        plan: AdmissionPlan<'_, '_>,
    ) -> Result<(), AdmissionApplyError> {
        let AdmissionPlan {
            pool,
            handoff,
            effects,
            effect_class,
            block_assembler_statuses,
        } = plan;
        let applied = self
            .relay
            .effects
            .try_apply_checked(effects, effect_class, || {
                pool.apply()?;
                match handoff {
                    AdmissionHandoff::Ready(plan) => plan.apply(),
                    AdmissionHandoff::External(plan) => plan.apply(),
                }
                for status in block_assembler_statuses {
                    self.journal_block_assembler_update(status);
                }
                Ok(())
            })
            .map_err(AdmissionApplyError::Journal)?;
        applied.map_err(AdmissionApplyError::Pool)
    }

    pub(crate) fn plan_unaccepted_admission<'authority>(
        &self,
        tx_pool: &TxPool,
        session: &'authority mut ReadyCommitSession<'_>,
        entry: &TxEntry,
        cause: UnacceptedReadyCause<'_>,
    ) -> Result<FailedAdmissionPlan<'authority>, PrePoolFault> {
        let reject = match cause {
            UnacceptedReadyCause::Policy(reject) => Some(reject),
            UnacceptedReadyCause::IngressRevoked => None,
        };
        let disposition = if reject.is_some_and(|reject| {
            matches!(
                reject,
                Reject::RBFRejected(..) | Reject::Resolve(OutPointError::Dead(_))
            )
        }) && tx_pool
            .pool_map
            .find_conflict_outpoint(entry.transaction())
            .is_some()
        {
            crate::component::pre_pool::ConflictDisposition::Retain
        } else {
            crate::component::pre_pool::ConflictDisposition::Terminalize
        };
        let terminal = session
            .plan_failed(disposition)
            .map_err(PrePoolError::into_unexpected_fault)?;
        let record = terminal.record();
        let (effects, peer_ban) = self.pipeline_outcome_effects(record, reject);
        let effect_class = match cause {
            UnacceptedReadyCause::IngressRevoked => EffectClass::Remote,
            UnacceptedReadyCause::Policy(_)
                if matches!(record.source, PrePoolSource::Remote(_)) =>
            {
                EffectClass::Remote
            }
            UnacceptedReadyCause::Policy(_) => EffectClass::Trusted,
        };
        Ok(FailedAdmissionPlan {
            terminal,
            effects,
            effect_class,
            peer_ban,
        })
    }

    pub(crate) fn apply_failed_admission(
        &self,
        plan: FailedAdmissionPlan<'_>,
    ) -> Result<Option<ckb_network::PeerIndex>, EffectJournalError> {
        let FailedAdmissionPlan {
            terminal,
            effects,
            effect_class,
            peer_ban,
        } = plan;
        self.relay.effects.try_apply(effects, effect_class, || {
            terminal.apply();
            if let Some((peer, duration)) = peer_ban {
                self.record_peer_ban(peer, duration);
            }
            peer_ban.map(|(peer, _)| peer)
        })
    }
}

fn check_rtx(
    tx_pool: &TxPool,
    snapshot: &Snapshot,
    rtx: &ResolvedTransaction,
    excluded: &HashSet<ProposalShortId>,
    pre_resolve_tip: &Byte32,
) -> Result<Status, Reject> {
    let short_id = rtx.transaction.proposal_short_id();
    let tx_status = get_tx_status(snapshot, &short_id);
    tx_pool
        .check_rtx_from_pool_excluding(rtx, excluded, pre_resolve_tip)
        .map(|_| tx_status)
}
