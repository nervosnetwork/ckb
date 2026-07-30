use super::{
    AuthorityClocks, AuthorityDelta, AuthorityFault, CommittedChange, IndependentDelta,
    IndependentUpdate, PlanError, PreparedApply, StalePlan, TxPoolAuthority, next_sequence,
    next_version,
};
use crate::authority::{
    plan::membership::{
        IndependentCoupling, IndependentMembershipChange, IndependentMembershipOutcome,
        PreparedIndependentMembership, prepare_independent_membership,
    },
    scheduler::{MAX_READY_BATCH, ReadyKey},
    state::{AcceptedEntry, AcceptedStatus, EntryVersion, OwnedTx, PreAcceptedPhase, RawTxHash},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct IndependentCandidate {
    key: RawTxHash,
    expected: EntryVersion,
    status: AcceptedStatus,
}

impl IndependentCandidate {
    pub(in crate::authority) fn new(
        key: RawTxHash,
        expected: EntryVersion,
        status: AcceptedStatus,
    ) -> Self {
        Self {
            key,
            expected,
            status,
        }
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
                .any(|other| other.key == candidate.key)
            {
                return Err(CandidateBatchError::Duplicate(candidate.key.clone()));
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
    request: IndependentCandidate,
    before: crate::authority::state::PreAcceptedEntry,
    rank: ReadyKey,
}

impl TxPoolAuthority {
    pub(in crate::authority) fn plan_settlement_for_foundation(
        &mut self,
        batch: &SettlementBatch,
    ) -> Result<SettlementPlan<'_>, PlanError> {
        let mut facts = Vec::new();
        facts
            .try_reserve(batch.0.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        for request in &batch.0 {
            let owner = self
                .entries
                .get(&request.key)
                .ok_or(PlanError::Stale(StalePlan::Missing))?;
            if owner.record().version != request.expected {
                return Err(PlanError::Stale(StalePlan::Version));
            }
            let OwnedTx::PreAccepted(before) = owner else {
                return Err(PlanError::Stale(StalePlan::Phase));
            };
            let PreAcceptedPhase::Computed(super::super::state::ComputedOutcome::Verified(
                verified,
            )) = &before.phase
            else {
                return Err(PlanError::Stale(StalePlan::Phase));
            };
            if verified.chain_epoch() != self.chain_epoch {
                return Err(PlanError::Stale(StalePlan::ChainEpoch));
            }
            facts.push(CandidateFact {
                request: request.clone(),
                before: before.clone(),
                rank: ReadyKey::from_computed(before)?,
            });
        }
        facts.sort_unstable_by(|left, right| right.rank.cmp(&left.rank));

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
            let PreAcceptedPhase::Computed(super::super::state::ComputedOutcome::Verified(
                verified,
            )) = &fact.before.phase
            else {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            };
            let version = clocks.next_version;
            let sequence = clocks.next_sequence;
            clocks = AuthorityClocks {
                next_version: next_version(version)?,
                next_sequence: next_sequence(sequence)?,
                ..clocks
            };
            let mut record = fact.before.record.clone();
            record.version = version;
            let after = AcceptedEntry {
                record,
                status: fact.request.status,
                verified: verified.clone(),
            };
            committed.push(CommittedChange {
                sequence,
                changed: fact.request.key.clone(),
            });
            changes.push(IndependentMembershipChange {
                key: fact.request.key,
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
                let first = changes
                    .first()
                    .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
                let key = first.key.clone();
                let expected = first.before.record.version;
                let status = first.after.status;
                let plan = self.plan_accept_for_foundation(&key, expected, status)?;
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
        let scheduler = self.scheduler.plan_batch(
            updates
                .iter()
                .map(|update| (self.entries.get(&update.key), Some(&update.after))),
        )?;
        Ok(SettlementPlan::IndependentRun(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Independent(IndependentDelta {
                updates,
                resource,
                projection,
                scheduler,
                clocks,
                committed,
            }),
            work: None,
        }))
    }
}
