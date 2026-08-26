use super::{
    ApplyClockReservation, AuthorityDelta, AuthorityFault, CandidateDispositionPlan,
    CommittedDelta, IndependentDelta, IndependentUpdate, PlanError, PreparedApply, StalePlan,
    TxPoolAuthority,
};
use crate::authority::{
    chain::{FinalAdmissionReceipt, ReadyPayloadRelation},
    effect::EffectPolicy,
    plan::membership::{
        IndependentMembershipChange, IndependentMembershipOutcome, PreparedIndependentMembership,
        has_membership_relation_coupling, prepare_independent_membership,
    },
    scheduler::ReadyKey,
    state::{AcceptedEntry, EntryVersion, OwnedTx, PreAcceptedPhase},
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
#[derive(Debug, PartialEq, Eq)]
pub(in crate::authority) struct CoupledSettlementContinuation {
    batch: SettlementBatch,
}

/// A continuation Plan that could not commit still owns the exact validated
/// Ready tail. Only effect-capacity pressure may retain this value across the
/// existing publisher wait; every other caller must consume the error and let
/// the level-triggered Ready frontier recapture current work.
#[derive(Debug, PartialEq, Eq)]
#[must_use = "a failed coupled Plan still owns its validated Ready continuation"]
pub(in crate::authority) struct CoupledSettlementPlanFailure {
    error: PlanError,
    continuation: Box<CoupledSettlementContinuation>,
}

impl CoupledSettlementPlanFailure {
    fn new(error: impl Into<PlanError>, batch: SettlementBatch) -> Self {
        Self {
            error: error.into(),
            continuation: Box::new(CoupledSettlementContinuation { batch }),
        }
    }

    pub(in crate::authority) fn into_parts(self) -> (PlanError, CoupledSettlementContinuation) {
        (self.error, *self.continuation)
    }
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
    fn plan_coupled_component(
        &mut self,
        strongest_receipt: FinalAdmissionReceipt,
        continuation: Vec<IndependentCandidate>,
    ) -> Result<SettlementPlan<'_>, PlanError> {
        let disposition = self.plan_candidate_disposition(strongest_receipt)?;
        Ok(SettlementPlan::CoupledComponent(CoupledSettlementPlan {
            disposition,
            continuation,
        }))
    }

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
    ) -> Result<SettlementPlan<'_>, CoupledSettlementPlanFailure> {
        let batch = continuation.batch;
        if let Err(error) = self.effects.ensure_open() {
            return Err(CoupledSettlementPlanFailure::new(error, batch));
        }
        let fact = match self.ready_candidate_fact(&batch.head) {
            Ok(fact) => fact,
            Err(error) => {
                return Err(CoupledSettlementPlanFailure::new(error, batch));
            }
        };
        let strongest_receipt = fact.receipt.clone();
        let coupled = if fact.receipt.payload_relation() == ReadyPayloadRelation::LocationRefreshed
        {
            true
        } else {
            let (change, _async_process_start) = match fact.into_membership_change() {
                Ok(change) => change,
                Err(error) => {
                    return Err(CoupledSettlementPlanFailure::new(error, batch));
                }
            };
            match has_membership_relation_coupling(self, std::slice::from_ref(&change)) {
                Ok(coupled) => coupled,
                Err(error) => {
                    return Err(CoupledSettlementPlanFailure::new(error, batch));
                }
            }
        };
        if !coupled {
            return match self.plan_settlement(&batch) {
                Ok(plan) => Ok(plan),
                Err(error) => Err(CoupledSettlementPlanFailure::new(error, batch)),
            };
        }

        let disposition = match self.plan_candidate_disposition(strongest_receipt) {
            Ok(disposition) => disposition,
            Err(error) => {
                return Err(CoupledSettlementPlanFailure::new(error, batch));
            }
        };
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
            return self.plan_coupled_component(strongest_receipt, continuation);
        }

        let mut changes = Vec::new();
        let mut async_process_starts = Vec::new();
        changes
            .try_reserve(facts.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        async_process_starts
            .try_reserve(facts.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
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
            removals,
        } = match prepare_independent_membership(self, &changes)? {
            IndependentMembershipOutcome::Prepared(prepared) => prepared,
            IndependentMembershipOutcome::Coupled => {
                // Coupling changes only the membership planner. Preserve the
                // exact proof issued by final validation instead of creating
                // a second proof-construction path at the handoff boundary.
                return self.plan_coupled_component(strongest_receipt, continuation);
            }
        };
        let composite = !removals.is_empty();
        if composite && removals.len() != changes.len() {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        }
        let clock_base = self.clocks.snapshot();
        let prospective_sequence = clock_base.next_sequence;

        // All allocation, effect-capacity and derived-index work finishes
        // before the one atomic clock reservation. A cohort-only inability to
        // build the composite returns to the canonical strongest member;
        // after this closure succeeds, the remaining identity stamping and
        // delta assembly are total.
        let planned: Result<_, PlanError> = (|| {
            let removal_count = removals
                .iter()
                .try_fold(0usize, |total, member| total.checked_add(member.len()));
            let effect_count = removal_count
                .and_then(|count| count.checked_add(changes.len()))
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
            let mut effects = Vec::new();
            effects
                .try_reserve_exact(effect_count)
                .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
            for (index, change) in changes.iter().enumerate() {
                let member_removals = removals.get(index).map_or(&[][..], Vec::as_slice);
                self.append_admission_effects(
                    &mut effects,
                    &change.after,
                    member_removals,
                    &projection,
                )?;
            }
            let publication = self
                .effects
                .build_publication(policy, effects)
                .map_err(|_| {
                    if composite {
                        PlanError::Backpressure(super::Backpressure::EffectCapacity)
                    } else {
                        PlanError::Fault(AuthorityFault::EffectProjection)
                    }
                })?;
            let effect = self
                .effects_for_plan()
                .plan_publication(&publication, prospective_sequence)?;

            let update_count = changes
                .len()
                .checked_add(removal_count.unwrap_or_default())
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
            let mut released_available = Vec::new();
            released_available
                .try_reserve_exact(removal_count.unwrap_or_default())
                .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
            for (index, change) in changes.iter().enumerate() {
                let member_removals = removals.get(index).map_or(&[][..], Vec::as_slice);
                released_available.extend(
                    self.collect_released_replacement_inputs(&change.after, member_removals)?,
                );
            }

            let mut updates = Vec::new();
            updates
                .try_reserve_exact(update_count)
                .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
            let mut committed_removals = Vec::new();
            committed_removals
                .try_reserve_exact(removal_count.unwrap_or_default())
                .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
            let mut removal_members = removals.into_iter();
            for change in changes {
                updates.push(IndependentUpdate {
                    key: change.key,
                    after: Some(OwnedTx::Accepted(change.after)),
                });
                let member_removals = removal_members.next().unwrap_or_default();
                for mut removal in member_removals {
                    let after = removal.take_after();
                    updates.push(IndependentUpdate {
                        key: removal.hash.clone(),
                        after,
                    });
                    committed_removals.push(removal);
                }
            }

            let history_count = updates
                .iter()
                .filter(|update| matches!(update.after, Some(OwnedTx::ReplacementHistory(_))))
                .count();
            let identity_count = updates
                .iter()
                .filter(|update| update.after.is_some())
                .count();
            let identity_count = NonZeroUsize::new(identity_count)
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            let mut next_version = clock_base.next_version;
            let mut next_arrival = clock_base.next_arrival;
            for update in &mut updates {
                let Some(after) = update.after.as_mut() else {
                    continue;
                };
                let version = next_version;
                next_version = EntryVersion(
                    next_version
                        .0
                        .checked_add(1)
                        .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?,
                );
                match after {
                    OwnedTx::Accepted(entry) => entry.record.version = version,
                    OwnedTx::ReplacementHistory(history) => {
                        let arrival = next_arrival;
                        next_arrival = super::super::state::Arrival(
                            next_arrival
                                .0
                                .checked_add(1)
                                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?,
                        );
                        history.assign_planned_identity_and_dependency_cut(
                            version,
                            arrival,
                            super::super::state::DependencyCut(prospective_sequence),
                        );
                    }
                    OwnedTx::PreAccepted(_) => {
                        return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                    }
                }
            }
            let mut before_owners = Vec::new();
            before_owners
                .try_reserve_exact(updates.len())
                .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
            before_owners.extend(
                updates
                    .iter()
                    .map(|update| self.entries.get(&update.key).as_deref().cloned()),
            );
            if before_owners.iter().any(Option::is_none) {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            }

            let scheduler = self.scheduler.plan_batch(
                updates
                    .iter()
                    .zip(&before_owners)
                    .map(|(update, before)| (before.as_ref(), update.after.as_ref())),
            )?;
            let accepted_after = updates.iter().filter_map(|update| match &update.after {
                Some(after @ OwnedTx::Accepted(_)) => Some(after),
                Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None => None,
            });
            let mut available = self.collect_dependency_loss_keys(accepted_after)?.keys;
            available
                .try_reserve(released_available.len())
                .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
            available.extend(released_available);
            let lost = if composite {
                let lost_owners =
                    updates
                        .iter()
                        .zip(&before_owners)
                        .filter_map(|(update, before)| match (before.as_ref(), &update.after) {
                            (Some(before @ OwnedTx::Accepted(_)), None)
                            | (
                                Some(before @ OwnedTx::Accepted(_)),
                                Some(OwnedTx::ReplacementHistory(_)),
                            ) => Some(before),
                            (Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)), _)
                            | (Some(OwnedTx::Accepted(_)), Some(OwnedTx::Accepted(_)))
                            | (Some(OwnedTx::Accepted(_)), Some(OwnedTx::PreAccepted(_)))
                            | (None, _) => None,
                        });
                self.collect_dependency_loss_keys(lost_owners)?.keys
            } else {
                Vec::new()
            };
            let dependency_control = self
                .dependencies
                .plan_events(
                    available,
                    lost,
                    super::super::state::DependencyCut(prospective_sequence),
                )?
                .unwrap_or_default();
            let dependency = self
                .dependencies
                .plan_replacements(
                    updates
                        .iter()
                        .zip(&before_owners)
                        .map(|(update, before)| (before.as_ref(), update.after.as_ref())),
                )?
                .with_control(dependency_control);
            let sources = self.source_versions.plan_replacements(
                updates
                    .iter()
                    .zip(&before_owners)
                    .map(|(update, before)| (before.as_ref(), update.after.as_ref())),
                prospective_sequence,
            );
            let (_entries, indexes) = self.entries_and_indexes_for_plan();
            let indexes =
                indexes.plan_replacements(updates.iter().zip(&before_owners).map(
                    |(update, before)| (&update.key, before.as_ref(), update.after.as_ref()),
                ))?;
            let owners = super::DerivedOwnerDelta { indexes, sources };
            let retired = super::retired_buffer(before_owners.len())?;
            Ok((
                updates,
                owners,
                scheduler,
                dependency,
                effect,
                committed_removals,
                retired,
                identity_count,
                history_count,
            ))
        })();
        let (
            updates,
            owners,
            scheduler,
            dependency,
            effect,
            committed_removals,
            retired,
            identity_count,
            history_count,
        ) = match planned {
            Ok(planned) => planned,
            Err(PlanError::Backpressure(
                super::Backpressure::Allocation | super::Backpressure::EffectCapacity,
            )) if composite => {
                return self.plan_coupled_component(strongest_receipt, continuation);
            }
            Err(error) => return Err(error),
        };

        // Exact-base OCC makes the irreversible clock advance the final
        // fallible operation. Every version-bearing derived delta above was
        // compiled from these previewed identities; a changed base or
        // exhausted range fails atomically without advancing the bank.
        let clocks = ApplyClockReservation::commit_owner_batch(
            std::sync::Arc::clone(&self.clocks),
            clock_base,
            identity_count,
            history_count,
        )?;
        debug_assert_eq!(clocks.sequence(), prospective_sequence);
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
                removals: committed_removals,
                retired,
            }),
        }))
    }
}
