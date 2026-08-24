use super::{
    ApplyClockReservation, AuthorityDelta, AuthorityFault, CandidateDispositionPlan,
    CommittedDelta, IndependentDelta, IndependentUpdate, PlanError, PreparedApply, StalePlan,
    TxPoolAuthority,
};
use crate::authority::{
    chain::{FinalAdmissionReceipt, ReadyPayloadRelation},
    effect::{CommittedAcceptance, CommittedEffect, EffectPolicy},
    plan::membership::{
        AncestorAggregate, DescendantAggregate, IndependentMembershipChange,
        IndependentMembershipOutcome, PreparedIndependentMembership,
        has_membership_relation_coupling, prepare_independent_membership,
    },
    scheduler::ReadyKey,
    state::{AcceptedEntry, OwnedTx, PreAcceptedPhase},
};
use std::num::NonZeroUsize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct IndependentCandidate {
    receipt: FinalAdmissionReceipt,
}

impl IndependentCandidate {
    pub(in crate::authority) fn new(receipt: FinalAdmissionReceipt) -> Self {
        Self { receipt }
    }

    pub(in crate::authority) fn into_receipt(self) -> FinalAdmissionReceipt {
        self.receipt
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct SettlementBatch {
    head: IndependentCandidate,
    tail: Vec<IndependentCandidate>,
}

impl SettlementBatch {
    /// Construct the production batch from one non-empty Ready validation
    /// cut. The scheduler is the sole producer of that bounded, unique cut;
    /// retaining `head` separately makes an empty settlement unrepresentable.
    pub(in crate::authority) fn from_validated_ready(
        head: IndependentCandidate,
        tail: Vec<IndependentCandidate>,
    ) -> Self {
        Self { head, tail }
    }

    pub(in crate::authority) fn len(&self) -> usize {
        self.tail.len().saturating_add(1)
    }

    fn candidates(&self) -> impl Iterator<Item = &IndependentCandidate> {
        std::iter::once(&self.head).chain(&self.tail)
    }

    fn from_candidates(mut candidates: Vec<IndependentCandidate>) -> Option<Self> {
        if candidates.is_empty() {
            return None;
        }
        let head = candidates.remove(0);
        Some(Self {
            head,
            tail: candidates,
        })
    }
}

#[cfg(test)]
#[path = "../tests/support/plan_settlement.rs"]
pub(in crate::authority) mod test_support;

#[must_use = "the settlement classification must be applied or routed through the coupled planner"]
pub(in crate::authority) enum SettlementPlan<'authority> {
    /// Every member commutes, so all prepared membership deltas share one
    /// mechanical Apply.
    IndependentRun(PreparedApply<'authority>),
    /// The canonical strongest member is fully planned against the same
    /// authority. Remaining cohort members retain their Ready owner and
    /// are reclassified after this single coupled component commits.
    CoupledComponent(CoupledSettlementPlan<'authority>),
}

/// One canonical coupled candidate plus the already validated, same-policy
/// Ready receipts that may be re-planned after it commits. Every continuation
/// head is reclassified against the newly committed authority; a now
/// independent tail returns to the ordinary batch planner. This is scheduling
/// reuse, not a second membership engine.
#[must_use = "a coupled settlement must be applied or explicitly discarded"]
pub(in crate::authority) struct CoupledSettlementPlan<'authority> {
    disposition: CandidateDispositionPlan<'authority>,
    continuation: Vec<IndependentCandidate>,
}

/// The already ranked, single-policy tail emitted only by a coupled Plan.
/// Runtime can therefore use the bounded coupled-head classifier without
/// accidentally bypassing the ordinary first-pass batch planner.
#[must_use = "a coupled continuation must be planned or returned to the Ready level"]
pub(in crate::authority) struct CoupledSettlementContinuation {
    batch: SettlementBatch,
}

impl<'authority> CoupledSettlementPlan<'authority> {
    pub(in crate::authority) fn apply(
        self,
    ) -> (CommittedDelta, Option<CoupledSettlementContinuation>) {
        let committed = match self.disposition {
            CandidateDispositionPlan::Accepted(plan) => plan.apply(),
            CandidateDispositionPlan::Rejected(plan) => plan.apply().1,
        };
        (
            committed,
            SettlementBatch::from_candidates(self.continuation)
                .map(|batch| CoupledSettlementContinuation { batch }),
        )
    }

    #[cfg(test)]
    pub(in crate::authority) fn into_disposition(self) -> CandidateDispositionPlan<'authority> {
        self.disposition
    }
}

struct CandidateFact {
    receipt: FinalAdmissionReceipt,
    before: crate::authority::state::PreAcceptedEntry,
    rank: ReadyKey,
}

impl CandidateFact {
    fn into_membership_change(
        self,
    ) -> Result<
        (
            IndependentMembershipChange,
            Option<crate::authority::state::AsyncProcessStart>,
        ),
        PlanError,
    > {
        if !matches!(&self.before.phase, PreAcceptedPhase::Ready(_)) {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        }
        let (proof, proposal, accepted_at, async_process_start) =
            self.receipt.into_membership_parts();
        let after = AcceptedEntry {
            // Classification, graph projection and resource admission do not
            // depend on the fresh committed version. The planner allocates it
            // only after the complete cohort is proven independent.
            record: self.before.record.clone(),
            provenance: self.before.source.accepted_provenance(),
            proof,
            proposal,
            accepted_at,
        };
        Ok((
            IndependentMembershipChange {
                key: self.before.record.identity.raw.clone(),
                before: self.before,
                after,
            },
            async_process_start,
        ))
    }
}

impl TxPoolAuthority {
    fn ready_candidate_fact(
        &self,
        request: &IndependentCandidate,
    ) -> Result<CandidateFact, PlanError> {
        let key = request.receipt.key().clone();
        let expected = request.receipt.expected();
        let owner = self
            .entries
            .get(&key)
            .ok_or(PlanError::Stale(StalePlan::Missing))?;
        if owner.record().version != expected {
            return Err(PlanError::Stale(StalePlan::Version));
        }
        let OwnedTx::PreAccepted(before) = &*owner else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        let PreAcceptedPhase::Ready(_) = &before.phase else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        self.validate_acceptance_evidence(before, &request.receipt)?;
        Ok(CandidateFact {
            receipt: request.receipt.clone(),
            before: before.clone(),
            rank: ReadyKey::from_ready(before)?,
        })
    }

    /// Continue an already ranked, single-policy coupled tail without
    /// rebuilding and sorting every weaker fact on every step. The current
    /// head alone is revalidated and classified against the authority produced
    /// by the preceding Apply. If it is no longer relation-coupled, return to
    /// the ordinary full batch planner so a newly independent tail keeps its
    /// one mechanical Apply and exact effect/clock observations.
    pub(in crate::authority) fn plan_coupled_continuation(
        &mut self,
        continuation: CoupledSettlementContinuation,
    ) -> Result<SettlementPlan<'_>, PlanError> {
        self.effects.ensure_open()?;
        let batch = continuation.batch;
        let fact = self.ready_candidate_fact(&batch.head)?;
        let strongest_receipt = fact.receipt.clone();
        let coupled = if fact.receipt.payload_relation() == ReadyPayloadRelation::LocationRefreshed
        {
            true
        } else {
            let (change, _async_process_start) = fact.into_membership_change()?;
            has_membership_relation_coupling(self, std::slice::from_ref(&change))?
        };
        if !coupled {
            return self.plan_settlement(&batch);
        }

        let disposition = self.plan_candidate_disposition(strongest_receipt)?;
        Ok(SettlementPlan::CoupledComponent(CoupledSettlementPlan {
            disposition,
            continuation: batch.tail,
        }))
    }

    pub(in crate::authority) fn plan_settlement(
        &mut self,
        batch: &SettlementBatch,
    ) -> Result<SettlementPlan<'_>, PlanError> {
        self.effects.ensure_open()?;
        let mut facts = Vec::new();
        facts
            .try_reserve(batch.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        for request in batch.candidates() {
            facts.push(self.ready_candidate_fact(request)?);
        }
        facts.sort_unstable_by(|left, right| right.rank.cmp(&left.rank));
        let strongest = facts
            .first()
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        let strongest_receipt = strongest.receipt.clone();
        let policy = EffectPolicy::for_preaccepted_source(strongest.before.source);

        // One immutable effect batch occupies one capacity region. Keep the
        // strongest Ready owner's control class and leave the other class in
        // Ready for the next level-triggered round. ReadyKey orders Proposal
        // and Recovery before Remote, so peer-controlled saturation cannot
        // hide trusted progress; this filter repeats the semantic boundary at
        // the sole membership planner instead of trusting a caller convention.
        facts.retain(|fact| EffectPolicy::for_preaccepted_source(fact.before.source) == policy);

        let mut continuation = Vec::new();
        continuation
            .try_reserve_exact(facts.len().saturating_sub(1))
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        continuation.extend(
            facts
                .iter()
                .skip(1)
                .map(|fact| IndependentCandidate::new(fact.receipt.clone())),
        );

        if facts
            .iter()
            .any(|fact| fact.receipt.payload_relation() == ReadyPayloadRelation::LocationRefreshed)
        {
            // Independent Apply drops replaced Ready shells inline. A
            // refreshed payload does not share that shell's resolved-cell
            // allocation, so use the single-candidate compiler that reserves
            // an outside-guard retirement carrier.
            let disposition = self.plan_candidate_disposition(strongest_receipt)?;
            return Ok(SettlementPlan::CoupledComponent(CoupledSettlementPlan {
                disposition,
                continuation,
            }));
        }

        let mut changes = Vec::new();
        let mut async_process_starts = Vec::new();
        changes
            .try_reserve(facts.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        async_process_starts
            .try_reserve(facts.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        let member_count = NonZeroUsize::new(facts.len())
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        for fact in facts {
            let (change, async_process_start) = fact.into_membership_change()?;
            if let Some(started_at) = async_process_start {
                async_process_starts.push(started_at);
            }
            changes.push(change);
        }

        let PreparedIndependentMembership {
            resource,
            projection,
        } = match prepare_independent_membership(self, &changes)? {
            IndependentMembershipOutcome::Prepared(prepared) => prepared,
            IndependentMembershipOutcome::Coupled => {
                // Coupling changes only the membership planner. Preserve the
                // exact proof issued by final validation instead of creating
                // a second proof-construction path at the handoff boundary.
                let disposition = self.plan_candidate_disposition(strongest_receipt)?;
                return Ok(SettlementPlan::CoupledComponent(CoupledSettlementPlan {
                    disposition,
                    continuation,
                }));
            }
        };
        let (versions, clocks) = ApplyClockReservation::begin_replacements(
            std::sync::Arc::clone(&self.clocks),
            member_count,
        )?;
        let source_sequence = clocks.sequence();
        for (change, version) in changes.iter_mut().zip(versions) {
            change.after.record.version = version;
        }
        let mut effects = Vec::new();
        effects
            .try_reserve(changes.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        for change in &changes {
            let ingress_peer = change.before.source.ingress_peer();
            effects.push(CommittedEffect::Accepted(CommittedAcceptance::Admission {
                entry: Self::committed_entry_snapshot(
                    &change.after,
                    AncestorAggregate::one(&change.after),
                    DescendantAggregate::one(&change.after),
                ),
                status: change.after.status(),
                ingress_peer,
            }));
        }
        let publication = self
            .effects
            .build_publication(policy, effects)
            .map_err(|_| PlanError::Fault(AuthorityFault::EffectProjection))?;
        let effect = self
            .effects_for_plan()
            .plan_publication(&publication, source_sequence)?;
        let mut updates = Vec::new();
        updates
            .try_reserve(changes.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        updates.extend(changes.into_iter().map(|change| IndependentUpdate {
            key: change.key,
            after: OwnedTx::Accepted(change.after),
        }));
        let mut before_owners = Vec::new();
        before_owners
            .try_reserve(updates.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        before_owners.extend(
            updates
                .iter()
                .map(|update| self.entries.get(&update.key).as_deref().cloned()),
        );
        let scheduler = self.scheduler.plan_batch(
            updates
                .iter()
                .zip(&before_owners)
                .map(|(update, before)| (before.as_ref(), Some(&update.after))),
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
                    .zip(&before_owners)
                    .map(|(update, before)| (before.as_ref(), Some(&update.after))),
            )?
            .with_control(dependency_control);
        let sources = self.source_versions.plan_replacements(
            updates
                .iter()
                .zip(&before_owners)
                .map(|(update, before)| (before.as_ref(), Some(&update.after))),
            source_sequence,
        );
        let (_entries, indexes) = self.entries_and_indexes_for_plan();
        let indexes = indexes.plan_replacements(
            updates
                .iter()
                .zip(&before_owners)
                .map(|(update, before)| (&update.key, before.as_ref(), Some(&update.after))),
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
                effect,
                clocks: clocks.finish(),
                async_process_starts,
                retired: super::retired_buffer(before_owners.len())?,
            }),
        }))
    }
}
