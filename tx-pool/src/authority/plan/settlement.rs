use super::{
    AuthorityClocks, AuthorityDelta, AuthorityFault, CommittedChange, CommittedHandoff,
    IndependentDelta, IndependentUpdate, PlanError, PreparedApply, StalePlan, TxPoolAuthority,
    next_sequence, next_version,
};
use crate::authority::{
    chain::FinalAdmissionReceipt,
    effect::EffectDelta,
    plan::membership::{
        IndependentCoupling, IndependentMembershipChange, IndependentMembershipOutcome,
        PreparedIndependentMembership, prepare_independent_membership,
    },
    scheduler::{MAX_READY_BATCH, ReadyKey},
    state::{AcceptedEntry, OwnedTx, PreAcceptedPhase, RawTxHash},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct IndependentCandidate {
    receipt: FinalAdmissionReceipt,
}

impl IndependentCandidate {
    pub(in crate::authority) fn new(receipt: FinalAdmissionReceipt) -> Self {
        Self { receipt }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) enum CandidateBatchError {
    Empty,
    TooLarge { limit: usize },
    Duplicate(RawTxHash),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct SettlementBatch(Vec<IndependentCandidate>);

impl SettlementBatch {
    pub(in crate::authority) fn new(
        candidates: Vec<IndependentCandidate>,
    ) -> Result<Self, CandidateBatchError> {
        if candidates.is_empty() {
            return Err(CandidateBatchError::Empty);
        }
        if candidates.len() > MAX_READY_BATCH {
            return Err(CandidateBatchError::TooLarge {
                limit: MAX_READY_BATCH,
            });
        }
        for (index, candidate) in candidates.iter().enumerate() {
            if candidates
                .iter()
                .skip(index + 1)
                .any(|other| other.receipt.key() == candidate.receipt.key())
            {
                return Err(CandidateBatchError::Duplicate(
                    candidate.receipt.key().clone(),
                ));
            }
        }
        Ok(Self(candidates))
    }
}

#[must_use = "the settlement classification must be applied or routed through the coupled planner"]
pub(in crate::authority) enum SettlementPlan<'authority> {
    /// Every member commutes, so all prepared membership deltas share one
    /// mechanical Apply.
    IndependentRun(PreparedApply<'authority>),
    /// The canonical strongest member is fully planned against the same
    /// authority. Remaining cohort members retain their Computed owner and
    /// are reclassified after this single coupled component commits.
    CoupledComponent {
        reason: IndependentCoupling,
        plan: PreparedApply<'authority>,
    },
}

struct CandidateFact {
    receipt: FinalAdmissionReceipt,
    before: crate::authority::state::PreAcceptedEntry,
    rank: ReadyKey,
}

impl TxPoolAuthority {
    pub(in crate::authority) fn plan_settlement_for_foundation(
        &mut self,
        batch: &SettlementBatch,
    ) -> Result<SettlementPlan<'_>, PlanError> {
        self.effects.ensure_open()?;
        let mut facts = Vec::new();
        facts
            .try_reserve(batch.0.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        for request in &batch.0 {
            let key = request.receipt.key().clone();
            let expected = request.receipt.expected();
            let owner = self
                .entries
                .get(&key)
                .ok_or(PlanError::Stale(StalePlan::Missing))?;
            if owner.record().version != expected {
                return Err(PlanError::Stale(StalePlan::Version));
            }
            let OwnedTx::PreAccepted(before) = owner else {
                return Err(PlanError::Stale(StalePlan::Phase));
            };
            let PreAcceptedPhase::Computed(super::super::state::ComputedOutcome::Verified(_)) =
                &before.phase
            else {
                return Err(PlanError::Stale(StalePlan::Phase));
            };
            self.validate_acceptance_evidence(before, &request.receipt)?;
            facts.push(CandidateFact {
                receipt: request.receipt.clone(),
                before: before.clone(),
                rank: ReadyKey::from_computed(before)?,
            });
        }
        facts.sort_unstable_by(|left, right| right.rank.cmp(&left.rank));
        let strongest_receipt = facts
            .first()
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?
            .receipt
            .clone();

        let mut clocks = self.clocks;
        let mut changes = Vec::new();
        let mut committed = Vec::new();
        changes
            .try_reserve(facts.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        committed
            .try_reserve(facts.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        for fact in facts {
            if !matches!(
                &fact.before.phase,
                PreAcceptedPhase::Computed(super::super::state::ComputedOutcome::Verified(_))
            ) {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            }
            let version = clocks.next_version;
            let sequence = clocks.next_sequence;
            clocks = AuthorityClocks {
                next_version: next_version(version)?,
                next_sequence: next_sequence(sequence)?,
                ..clocks
            };
            let mut record = fact.before.record.clone();
            record.version = version;
            let (proof, proposal, accepted_at) = fact.receipt.into_membership_parts();
            let after = AcceptedEntry {
                record,
                provenance: fact.before.source.accepted_provenance(),
                proof,
                proposal,
                accepted_at,
            };
            committed.push(CommittedChange {
                sequence,
                changed: fact.before.record.identity.raw.clone(),
            });
            changes.push(IndependentMembershipChange {
                key: fact.before.record.identity.raw.clone(),
                before: fact.before,
                after,
            });
        }

        let PreparedIndependentMembership {
            resource,
            projection,
        } = match prepare_independent_membership(self, &changes)? {
            IndependentMembershipOutcome::Prepared(prepared) => prepared,
            IndependentMembershipOutcome::Coupled(reason) => {
                // Coupling changes only the membership planner. Preserve the
                // exact proof issued by final validation instead of creating
                // a second proof-construction path at the handoff boundary.
                let plan = self.plan_accept(strongest_receipt)?;
                return Ok(SettlementPlan::CoupledComponent { reason, plan });
            }
        };
        let mut updates = Vec::new();
        updates
            .try_reserve(changes.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        updates.extend(changes.into_iter().map(|change| IndependentUpdate {
            key: change.key,
            after: OwnedTx::Accepted(change.after),
        }));
        let source_sequence = committed
            .last()
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?
            .sequence;
        let scheduler = self.scheduler.plan_batch(
            updates
                .iter()
                .map(|update| (self.entries.get(&update.key), Some(&update.after))),
        )?;
        let dependency_control = self
            .dependencies
            .plan_events(
                self.collect_dependency_loss_keys(updates.iter().map(|update| &update.after))?
                    .keys,
                Vec::new(),
                super::super::state::DependencyCut(source_sequence),
            )?
            .unwrap_or_default();
        let dependency = self
            .dependencies
            .plan_replacements(
                updates
                    .iter()
                    .map(|update| (self.entries.get(&update.key), Some(&update.after))),
            )?
            .with_control(dependency_control);
        let entries = &self.entries;
        let sources = self.source_versions.plan_replacements(
            updates
                .iter()
                .map(|update| (entries.get(&update.key), Some(&update.after))),
            source_sequence,
        );
        let indexes = self.indexes.plan_replacements(
            updates
                .iter()
                .map(|update| (&update.key, entries.get(&update.key), Some(&update.after))),
        )?;
        let owners = super::DerivedOwnerDelta { indexes, sources };
        Ok(SettlementPlan::IndependentRun(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Independent(IndependentDelta {
                updates,
                owners,
                resource,
                projection,
                scheduler,
                dependency,
                effect: EffectDelta::default(),
                clocks,
                committed,
            }),
            handoff: CommittedHandoff::None,
        }))
    }
}
