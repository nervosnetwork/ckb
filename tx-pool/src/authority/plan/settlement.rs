use super::{
    ApplyClockReservation, ApplyOwnerBatchReservationError, AuthorityFault,
    CompiledSharedIndependent, IndependentDelta, IndependentOwnerAction, IndependentOwnerCut,
    IndependentUpdate, OwnerPrestate, PlanError, StalePlan, TxPoolAuthority,
};
#[cfg(test)]
use super::{CandidateDispositionPlan, CommittedDelta, PreparedIndependentApply};
use crate::authority::{
    chain::{FinalAdmissionReceipt, ReadyPayloadRelation},
    effect::{EffectPolicy, EffectWakeTransition},
    plan::membership::{
        IndependentMembershipChange, IndependentMembershipOutcome, PreparedIndependentMembership,
        has_membership_relation_coupling, prepare_classified_ordinary_membership,
        prepare_independent_membership,
    },
    scheduler::{ReadyKey, ReadyReservation},
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
#[cfg(test)]
#[expect(
    clippy::large_enum_variant,
    reason = "both variants linearly own already-prepared semantic plans; boxing would add an unaccounted infallible allocation after planning reserved every fallible resource"
)]
pub(in crate::authority) enum SettlementPlan<'authority> {
    /// Every member commutes, so all prepared membership deltas share one
    /// mechanical Apply.
    IndependentRun(PreparedIndependentApply<'authority>),
    /// The canonical strongest member is fully planned against the same
    /// authority. Remaining cohort members retain their Ready owner and
    /// are reclassified after this single coupled component commits.
    CoupledComponent(CoupledSettlementPlan<'authority>),
}

#[expect(
    clippy::large_enum_variant,
    reason = "the independent compiler result owns its fallibly preallocated delta; heap indirection would reintroduce an unplanned allocation on the commit preparation path"
)]
enum SettlementCompilation {
    Independent(IndependentDelta),
    Coupled {
        strongest_receipt: FinalAdmissionReceipt,
        continuation: Vec<IndependentCandidate>,
    },
    ClockContended(SharedReadyClockContention),
}

/// Source-owned result of capturing and ordering one Ready cohort once, then
/// materializing each independently linearizable job through the same
/// settlement compiler without re-reading or re-sorting its policy facts.
pub(in crate::authority) enum SharedReadyWaveCompilation {
    Complete(Vec<CompiledSharedIndependent>),
    Fallback(Vec<CompiledSharedIndependent>),
    ClockContended {
        compiled: Vec<CompiledSharedIndependent>,
        contention: SharedReadyClockContention,
    },
    Error {
        compiled: Vec<CompiledSharedIndependent>,
        error: PlanError,
    },
}

/// Exact rollback edge produced when an independently synchronized clock
/// commit wins after Ready has staged, but not published, its effect suffix.
/// Runtime must consume the wake before reporting ordinary stale progress.
#[must_use = "clock contention must publish its staged-effect rollback wake"]
pub(in crate::authority) struct SharedReadyClockContention {
    effect_wake: Option<EffectWakeTransition>,
}

impl SharedReadyClockContention {
    pub(in crate::authority) fn into_effect_wake(self) -> Option<EffectWakeTransition> {
        self.effect_wake
    }
}

/// Closed result of compiling the aggregate shared Ready path. Coupled work
/// returns to the canonical exact-cut head compiler; clock contention is
/// distinct because it owns a wake that cannot be embedded in `PlanError`.
#[expect(
    clippy::large_enum_variant,
    reason = "the compiled arm linearly owns fully preallocated semantic plans; boxing after those reservations would add a new fallible allocation to the Ready boundary"
)]
pub(in crate::authority) enum SharedIndependentSettlementCompilation {
    Compiled(CompiledSharedIndependent),
    RequiresCanonical(Option<CoupledSettlementContinuation>),
    ClockContended(SharedReadyClockContention),
}

/// One exactly compiled strongest candidate and only the same-policy tail
/// whose scheduler reservations may remain live across that commit.  The
/// tail is never preplanned: every later head is reclassified against the
/// authority produced by the preceding shared Apply.
pub(in crate::authority) struct CompiledSharedCanonicalReadyHead {
    compiled: CompiledSharedIndependent,
    continuation: Option<CoupledSettlementContinuation>,
}

impl CompiledSharedCanonicalReadyHead {
    pub(in crate::authority) fn into_parts(
        self,
    ) -> (
        CompiledSharedIndependent,
        Option<CoupledSettlementContinuation>,
    ) {
        (self.compiled, self.continuation)
    }
}

#[cfg(test)]
impl SharedIndependentSettlementCompilation {
    pub(in crate::authority) fn into_option_for_foundation(
        self,
    ) -> Option<CompiledSharedIndependent> {
        match self {
            Self::Compiled(compiled) => Some(compiled),
            Self::RequiresCanonical(_) => None,
            Self::ClockContended(contention) => {
                let _ = contention.into_effect_wake();
                panic!("a single-threaded foundation compiler cannot lose its clock base")
            }
        }
    }
}

fn ready_wave_defers_to_canonical(error: &PlanError) -> bool {
    !matches!(error, PlanError::Fault(_) | PlanError::EffectClosed)
}

/// One canonical coupled candidate plus the already validated, same-policy
/// Ready receipts that may be re-planned after it commits. Every continuation
/// head is reclassified against the newly committed authority; a now
/// independent tail returns to the ordinary batch planner. This is scheduling
/// reuse, not a second membership engine.
#[must_use = "a coupled settlement must be applied or explicitly discarded"]
#[cfg(test)]
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

impl CoupledSettlementContinuation {
    pub(in crate::authority) fn batch(&self) -> &SettlementBatch {
        &self.batch
    }
}

/// A continuation Plan that could not commit still owns the exact validated
/// Ready tail. Only effect-capacity pressure may retain this value across the
/// existing publisher wait; every other caller must consume the error and let
/// the level-triggered Ready frontier recapture current work.
#[derive(Debug, PartialEq, Eq)]
#[must_use = "a failed coupled Plan still owns its validated Ready continuation"]
#[cfg(test)]
pub(in crate::authority) struct CoupledSettlementPlanFailure {
    error: PlanError,
    continuation: Box<CoupledSettlementContinuation>,
}

#[cfg(test)]
impl CoupledSettlementPlanFailure {
    fn new(error: impl Into<PlanError>, batch: SettlementBatch) -> Self {
        Self {
            error: error.into(),
            continuation: Box::new(CoupledSettlementContinuation { batch }),
        }
    }
}

#[cfg(test)]
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
    #[cfg(test)]
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
        let before = {
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
            before.clone()
        };
        // Dependency evidence owns a separately routed shard cut. Clone the
        // bounded Ready owner first so no point owner guard survives into the
        // dependency read and creates an owner->dependency lock-order edge.
        self.validate_acceptance_evidence(&before, &request.receipt)?;
        Ok(CandidateFact {
            receipt: request.receipt.clone(),
            rank: ReadyKey::from_ready(&before)?,
            before,
        })
    }

    /// Continue an already ranked, single-policy coupled tail without
    /// rebuilding and sorting every weaker fact on every step. The current
    /// head alone is revalidated and classified against the authority produced
    /// by the preceding Apply. If it is no longer relation-coupled, return to
    /// the ordinary full batch planner so a newly independent tail keeps its
    /// one mechanical Apply and exact effect/clock observations.
    #[cfg(test)]
    pub(in crate::authority) fn plan_coupled_continuation(
        &mut self,
        continuation: CoupledSettlementContinuation,
    ) -> Result<SettlementPlan<'_>, CoupledSettlementPlanFailure> {
        let batch = continuation.batch;
        if let Err(error) = self.effects.lock().ensure_open() {
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

    fn capture_ready_facts(
        &self,
        batch: &SettlementBatch,
    ) -> Result<Vec<CandidateFact>, PlanError> {
        self.effects.lock().ensure_open()?;
        let mut facts = Vec::new();
        facts
            .try_reserve(batch.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        for request in batch.candidates() {
            facts.push(self.ready_candidate_fact(request)?);
        }
        facts.sort_unstable_by(|left, right| right.rank.cmp(&left.rank));
        Ok(facts)
    }

    fn compile_settlement(
        &self,
        batch: &SettlementBatch,
    ) -> Result<SettlementCompilation, PlanError> {
        let facts = self.capture_ready_facts(batch)?;
        self.compile_captured_settlement(facts)
    }

    fn compile_captured_settlement(
        &self,
        mut facts: Vec<CandidateFact>,
    ) -> Result<SettlementCompilation, PlanError> {
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
            return Ok(SettlementCompilation::Coupled {
                strongest_receipt,
                continuation,
            });
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

        let prepared = match prepare_independent_membership(self, &changes) {
            Ok(IndependentMembershipOutcome::Prepared(prepared)) => prepared,
            Ok(IndependentMembershipOutcome::Coupled) => {
                // Coupling changes only the membership planner. Preserve the
                // exact proof issued by final validation instead of creating
                // a second proof-construction path at the handoff boundary.
                return Ok(SettlementCompilation::Coupled {
                    strongest_receipt,
                    continuation,
                });
            }
            Err(error) => return Err(error),
        };
        self.compile_prepared_independent(
            strongest_receipt,
            continuation,
            policy,
            changes,
            async_process_starts,
            prepared,
        )
    }

    fn compile_prepared_independent(
        &self,
        strongest_receipt: FinalAdmissionReceipt,
        continuation: Vec<IndependentCandidate>,
        policy: EffectPolicy,
        changes: Vec<IndependentMembershipChange>,
        async_process_starts: Vec<crate::authority::state::AsyncProcessStart>,
        prepared: PreparedIndependentMembership,
    ) -> Result<SettlementCompilation, PlanError> {
        let PreparedIndependentMembership {
            resource,
            projection,
            removals,
        } = prepared;
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
                .lock()
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
            let scheduler = self.scheduler.lock().plan_batch(
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
                .with_control(dependency_control.into(), &self.dependencies)?;
            let sources = self.source_versions.plan_replacements(
                updates
                    .iter()
                    .zip(&before_owners)
                    .map(|(update, before)| (before.as_ref(), update.after.as_ref())),
                prospective_sequence,
            );
            let template_sources =
                self.plan_owner_sources(updates.iter().zip(&before_owners).map(
                    |(update, before)| (&update.key, before.as_ref(), update.after.as_ref()),
                ))?;
            let (_entries, indexes) = self.entries_and_indexes_for_plan();
            let indexes =
                indexes.plan_replacements(updates.iter().zip(&before_owners).map(
                    |(update, before)| (&update.key, before.as_ref(), update.after.as_ref()),
                ))?;
            let owners = super::DerivedOwnerDelta {
                indexes,
                sources,
                template_sources,
            };
            let retired = super::retired_buffer(before_owners.len())?;
            let mut owner_cuts = Vec::new();
            owner_cuts
                .try_reserve_exact(updates.len())
                .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
            for (update, before) in updates.into_iter().zip(&before_owners) {
                let expected = before
                    .as_ref()
                    .map(OwnerPrestate::from_owner)
                    .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
                owner_cuts.push(IndependentOwnerCut {
                    key: update.key,
                    expected,
                    action: IndependentOwnerAction::Replace(update.after),
                });
            }
            Ok((
                owner_cuts,
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
            owner_cuts,
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
                return Ok(SettlementCompilation::Coupled {
                    strongest_receipt,
                    continuation,
                });
            }
            Err(error) => return Err(error),
        };

        // Exact-base OCC makes the irreversible clock advance the final
        // fallible operation. Every version-bearing derived delta above was
        // compiled from these previewed identities; a changed base or
        // exhausted range fails atomically without advancing the bank.
        #[cfg(test)]
        self.entries.enter_ready_clock_commit_probe();
        let clocks = match ApplyClockReservation::commit_owner_batch(
            std::sync::Arc::clone(&self.clocks),
            clock_base,
            identity_count,
            history_count,
        ) {
            Ok(clocks) => clocks,
            Err(ApplyOwnerBatchReservationError::StaleBase) => {
                let effect_wake = effect
                    .rollback_staged_with_wake()
                    .map_err(|_| PlanError::Fault(AuthorityFault::EffectProjection))?;
                return Ok(SettlementCompilation::ClockContended(
                    SharedReadyClockContention { effect_wake },
                ));
            }
            Err(ApplyOwnerBatchReservationError::Exhausted) => {
                return Err(PlanError::Fault(AuthorityFault::CounterExhausted));
            }
        };
        debug_assert_eq!(clocks.sequence(), prospective_sequence);
        Ok(SettlementCompilation::Independent(IndependentDelta {
            owner_cuts,
            owners,
            resource: Some(resource),
            projection,
            scheduler,
            dependency,
            effect,
            clocks: clocks.finish(),
            async_process_starts,
            removals: committed_removals,
            retired,
        }))
    }

    pub(in crate::authority::plan) fn seal_shared_independent(
        &self,
        mut delta: IndependentDelta,
    ) -> Result<CompiledSharedIndependent, PlanError> {
        let support = delta.physical_support(self);
        let staged_effect = super::super::effect::EffectLog::stage_publication(
            &self.effects,
            std::mem::take(&mut delta.effect),
        )
        .map_err(PlanError::from)?;
        Ok(CompiledSharedIndependent {
            generation: self.generation,
            chain_view: self.chain_view.clone(),
            delta,
            support,
            staged_effect,
        })
    }

    /// Compile under the coherent outer read cut, but return no live mutation
    /// authority. Runtime binds the result later under a shared generation
    /// barrier so owner Apply can overlap without weakening Plan coherence.
    pub(in crate::authority) fn compile_shared_independent_settlement(
        &self,
        batch: &SettlementBatch,
    ) -> Result<SharedIndependentSettlementCompilation, PlanError> {
        if batch.len() == 1 {
            let receipt = batch.head.receipt.clone();
            let delta = match self.prepare_shared_accept_delta(receipt) {
                Ok(delta) => delta,
                Err(
                    PlanError::Membership(_) | PlanError::Stale(StalePlan::AcceptedObservation),
                ) => {
                    return Ok(SharedIndependentSettlementCompilation::RequiresCanonical(
                        None,
                    ));
                }
                Err(error) => return Err(error),
            };
            let delta = match delta.into_shared_exact() {
                Ok(delta) => delta,
                Err(PlanError::Stale(StalePlan::AcceptedObservation)) => {
                    return Ok(SharedIndependentSettlementCompilation::RequiresCanonical(
                        None,
                    ));
                }
                Err(error) => return Err(error),
            };
            return self
                .seal_shared_independent(delta)
                .map(SharedIndependentSettlementCompilation::Compiled);
        }
        match self.compile_settlement(batch)? {
            SettlementCompilation::Independent(delta) if delta.is_pure_accepted() => self
                .seal_shared_independent(delta)
                .map(SharedIndependentSettlementCompilation::Compiled),
            SettlementCompilation::Independent(_) => Ok(
                SharedIndependentSettlementCompilation::RequiresCanonical(None),
            ),
            SettlementCompilation::Coupled {
                strongest_receipt,
                continuation,
            } => Ok(SharedIndependentSettlementCompilation::RequiresCanonical(
                Some(CoupledSettlementContinuation {
                    batch: SettlementBatch::from_validated_ready(
                        IndependentCandidate::new(strongest_receipt),
                        continuation,
                    ),
                }),
            )),
            SettlementCompilation::ClockContended(contention) => Ok(
                SharedIndependentSettlementCompilation::ClockContended(contention),
            ),
        }
    }

    /// Compile one strongest Ready candidate through the canonical policy and
    /// the existing exact-cut shared Apply.  This is not a second membership
    /// engine: it calls the same single-candidate compiler used by the
    /// established shared Ready route. Capacity decisions bind one coherent
    /// 64-shard order revision vector, while policy rejection binds the exact
    /// facts that keep the rejection true through final Apply.
    pub(in crate::authority) fn compile_shared_canonical_ready_head(
        &self,
        batch: &SettlementBatch,
    ) -> Result<CompiledSharedCanonicalReadyHead, PlanError> {
        let mut facts = self.capture_ready_facts(batch)?;
        let strongest = facts
            .first()
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        if strongest.receipt.key() != batch.head.receipt.key()
            || strongest.receipt.expected() != batch.head.receipt.expected()
        {
            return Err(PlanError::Fault(AuthorityFault::SchedulerProjection));
        }
        let receipt = strongest.receipt.clone();
        let policy = EffectPolicy::for_preaccepted_source(strongest.before.source);
        facts.retain(|fact| EffectPolicy::for_preaccepted_source(fact.before.source) == policy);
        let mut tail = Vec::new();
        tail.try_reserve_exact(facts.len().saturating_sub(1))
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        tail.extend(
            facts
                .into_iter()
                .skip(1)
                .map(|fact| IndependentCandidate::new(fact.receipt)),
        );
        let continuation = SettlementBatch::from_candidates(tail)
            .map(|batch| CoupledSettlementContinuation { batch });
        let delta = self.compile_shared_candidate_disposition_delta(receipt)?;
        self.seal_shared_independent(delta)
            .map(|compiled| CompiledSharedCanonicalReadyHead {
                compiled,
                continuation,
            })
    }

    /// Recheck that the captured head is still the scheduler's strongest
    /// visible Ready member before turning its reservation into a committing
    /// slot.  A later stronger insertion is progress and forces recapture.
    pub(in crate::authority) fn shared_ready_head_is_current(
        &self,
        reservation: &ReadyReservation,
        batch: &SettlementBatch,
    ) -> bool {
        reservation.current_prefix_len(
            &self.scheduler,
            std::iter::once((batch.head.receipt.key(), batch.head.receipt.expected())),
        ) == 1
    }

    /// Capture, rank and classify one bounded Ready cohort exactly once. Only
    /// the already-classified ordinary-leaf shape is materialized per job; a
    /// refreshed payload, cross-candidate membership relation, capacity
    /// coupling or non-pure delta returns the complete cohort to the canonical
    /// aggregate planner before any owner mutation.
    pub(in crate::authority) fn compile_shared_ready_wave(
        &self,
        batch: &SettlementBatch,
    ) -> SharedReadyWaveCompilation {
        let mut facts = match self.capture_ready_facts(batch) {
            Ok(facts) => facts,
            Err(error) => {
                return SharedReadyWaveCompilation::Error {
                    compiled: Vec::new(),
                    error,
                };
            }
        };
        let Some(strongest) = facts.first() else {
            return SharedReadyWaveCompilation::Error {
                compiled: Vec::new(),
                error: PlanError::Fault(AuthorityFault::MembershipProjection),
            };
        };
        let policy = EffectPolicy::for_preaccepted_source(strongest.before.source);
        // Preserve the canonical trust boundary: a weaker Remote region must
        // never veto trusted Ready progress, and owner/template visibility
        // must not overtake the strongest source class. Ready ordering makes
        // this retained class one contiguous prefix of the captured cohort.
        facts.retain(|fact| EffectPolicy::for_preaccepted_source(fact.before.source) == policy);
        if facts
            .iter()
            .any(|fact| fact.receipt.payload_relation() == ReadyPayloadRelation::LocationRefreshed)
        {
            return SharedReadyWaveCompilation::Fallback(Vec::new());
        }

        let mut changes = Vec::new();
        let mut metadata = Vec::new();
        if changes.try_reserve_exact(facts.len()).is_err()
            || metadata.try_reserve_exact(facts.len()).is_err()
        {
            return SharedReadyWaveCompilation::Fallback(Vec::new());
        }
        for fact in facts {
            let receipt = fact.receipt.clone();
            let policy = EffectPolicy::for_preaccepted_source(fact.before.source);
            let (change, async_process_start) = match fact.into_membership_change() {
                Ok(change) => change,
                Err(error) if ready_wave_defers_to_canonical(&error) => {
                    return SharedReadyWaveCompilation::Fallback(Vec::new());
                }
                Err(error) => {
                    return SharedReadyWaveCompilation::Error {
                        compiled: Vec::new(),
                        error,
                    };
                }
            };
            changes.push(change);
            metadata.push((receipt, policy, async_process_start));
        }
        match has_membership_relation_coupling(self, &changes) {
            Ok(false) => {}
            Ok(true) => return SharedReadyWaveCompilation::Fallback(Vec::new()),
            Err(error) if ready_wave_defers_to_canonical(&error) => {
                return SharedReadyWaveCompilation::Fallback(Vec::new());
            }
            Err(error) => {
                return SharedReadyWaveCompilation::Error {
                    compiled: Vec::new(),
                    error,
                };
            }
        }

        let mut compiled = Vec::new();
        if compiled.try_reserve_exact(changes.len()).is_err() {
            return SharedReadyWaveCompilation::Fallback(compiled);
        }
        for (change, (receipt, policy, async_process_start)) in changes.into_iter().zip(metadata) {
            let mut job_changes = Vec::new();
            if job_changes.try_reserve_exact(1).is_err() {
                return SharedReadyWaveCompilation::Fallback(compiled);
            }
            job_changes.push(change);
            let prepared = match prepare_classified_ordinary_membership(self, &job_changes) {
                Ok(IndependentMembershipOutcome::Prepared(prepared)) => prepared,
                Ok(IndependentMembershipOutcome::Coupled) => {
                    return SharedReadyWaveCompilation::Fallback(compiled);
                }
                Err(error) if ready_wave_defers_to_canonical(&error) => {
                    return SharedReadyWaveCompilation::Fallback(compiled);
                }
                Err(error) => {
                    return SharedReadyWaveCompilation::Error { compiled, error };
                }
            };
            let mut async_process_starts = Vec::new();
            if async_process_start.is_some() && async_process_starts.try_reserve_exact(1).is_err() {
                return SharedReadyWaveCompilation::Fallback(compiled);
            }
            async_process_starts.extend(async_process_start);
            let delta = match self.compile_prepared_independent(
                receipt,
                Vec::new(),
                policy,
                job_changes,
                async_process_starts,
                prepared,
            ) {
                Ok(SettlementCompilation::Independent(delta)) if delta.is_pure_accepted() => delta,
                Ok(
                    SettlementCompilation::Independent(_) | SettlementCompilation::Coupled { .. },
                ) => {
                    return SharedReadyWaveCompilation::Fallback(compiled);
                }
                Ok(SettlementCompilation::ClockContended(contention)) => {
                    return SharedReadyWaveCompilation::ClockContended {
                        compiled,
                        contention,
                    };
                }
                Err(error) if ready_wave_defers_to_canonical(&error) => {
                    return SharedReadyWaveCompilation::Fallback(compiled);
                }
                Err(error) => {
                    return SharedReadyWaveCompilation::Error { compiled, error };
                }
            };
            match self.seal_shared_independent(delta) {
                Ok(candidate) => compiled.push(candidate),
                Err(error) if ready_wave_defers_to_canonical(&error) => {
                    return SharedReadyWaveCompilation::Fallback(compiled);
                }
                Err(error) => {
                    return SharedReadyWaveCompilation::Error { compiled, error };
                }
            }
        }
        SharedReadyWaveCompilation::Complete(compiled)
    }

    #[cfg(test)]
    pub(in crate::authority) fn plan_settlement(
        &mut self,
        batch: &SettlementBatch,
    ) -> Result<SettlementPlan<'_>, PlanError> {
        match self.compile_settlement(batch)? {
            SettlementCompilation::Independent(delta) => {
                if delta.is_pure_accepted() {
                    let compiled = self.seal_shared_independent(delta)?;
                    compiled
                        .bind(self)
                        .map(SettlementPlan::IndependentRun)
                        .map_err(|_| PlanError::Stale(StalePlan::Generation))
                } else {
                    Ok(SettlementPlan::IndependentRun(
                        PreparedIndependentApply::Exclusive {
                            authority: self,
                            delta,
                        },
                    ))
                }
            }
            SettlementCompilation::Coupled {
                strongest_receipt,
                continuation,
            } => self.plan_coupled_component(strongest_receipt, continuation),
            SettlementCompilation::ClockContended(contention) => {
                // A caller holding `&mut TxPoolAuthority` excludes every
                // shared compiler that can advance this clock bank. Preserve
                // the closed algebra without pretending ordinary contention
                // is counter exhaustion if that invariant is ever refuted.
                let _ = contention.into_effect_wake();
                Err(PlanError::Stale(StalePlan::ClockBase))
            }
        }
    }
}
