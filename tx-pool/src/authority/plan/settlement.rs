use super::{
    AuthorityClocks, AuthorityDelta, AuthorityFault, CandidateDispositionPlan, IndependentDelta,
    IndependentUpdate, PlanError, PrepareSettlementError, PreparedApply, StalePlan,
    TxPoolAuthority, next_sequence, next_version,
};
use crate::authority::{
    chain::{FinalAdmissionReceipt, ReadyPayloadRelation, TxPoolComputeAdmissionReceipt},
    effect::{CommittedAcceptance, CommittedEffect, EffectPolicy},
    plan::membership::{
        AncestorAggregate, DescendantAggregate, IndependentMembershipChange,
        IndependentMembershipOutcome, PreparedIndependentMembership,
        prepare_independent_membership,
    },
    scheduler::ReadyKey,
    state::{AcceptedEntry, ApplySequence, AsyncProcessStart, OwnedTx, PreAcceptedPhase},
    work::{SettlementNext, SettlementToken},
};

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
    CoupledComponent(CandidateDispositionPlan<'authority>),
}

struct CandidateFact {
    receipt: FinalAdmissionReceipt,
    before: crate::authority::state::PreAcceptedEntry,
    rank: ReadyKey,
}

impl TxPoolAuthority {
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
            let PreAcceptedPhase::Ready(_) = &before.phase else {
                return Err(PlanError::Stale(StalePlan::Phase));
            };
            self.validate_acceptance_evidence(before, &request.receipt)?;
            facts.push(CandidateFact {
                receipt: request.receipt.clone(),
                before: before.clone(),
                rank: ReadyKey::from_ready(before)?,
            });
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

        if facts
            .iter()
            .any(|fact| fact.receipt.payload_relation() == ReadyPayloadRelation::LocationRefreshed)
        {
            // Independent Apply drops replaced Ready shells inline. A
            // refreshed payload does not share that shell's resolved-cell
            // allocation, so use the single-candidate compiler that reserves
            // an outside-guard retirement carrier.
            let disposition = self.plan_candidate_disposition(strongest_receipt)?;
            return Ok(SettlementPlan::CoupledComponent(disposition));
        }

        let mut clocks = self.clocks;
        let mut changes = Vec::new();
        let mut async_process_starts = Vec::new();
        let mut source_sequence = None;
        changes
            .try_reserve(facts.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        async_process_starts
            .try_reserve(facts.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        for fact in facts {
            if !matches!(&fact.before.phase, PreAcceptedPhase::Ready(_)) {
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
            let (proof, proposal, accepted_at, async_process_start) =
                fact.receipt.into_membership_parts();
            let after = AcceptedEntry {
                record,
                provenance: fact.before.source.accepted_provenance(),
                proof,
                proposal,
                accepted_at,
            };
            if let Some(started_at) = async_process_start {
                async_process_starts.push(started_at);
            }
            source_sequence = Some(sequence);
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
            IndependentMembershipOutcome::Coupled => {
                // Coupling changes only the membership planner. Preserve the
                // exact proof issued by final validation instead of creating
                // a second proof-construction path at the handoff boundary.
                let disposition = self.plan_candidate_disposition(strongest_receipt)?;
                return Ok(SettlementPlan::CoupledComponent(disposition));
            }
        };
        let source_sequence =
            source_sequence.ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        let delta = self.compile_independent_delta(
            changes,
            PreparedIndependentMembership {
                resource,
                projection,
            },
            clocks,
            source_sequence,
            async_process_starts,
            policy,
        )?;
        Ok(SettlementPlan::IndependentRun(PreparedApply {
            authority: self,
            delta,
        }))
    }
}

impl TxPoolAuthority {
    /// Plan one successful verification through the sole settlement Apply.
    /// The common independent case reaches Accepted directly; every ordinary
    /// miss delegates to the established Computing-to-Ready planner. Runtime
    /// therefore owns neither a second Apply protocol nor a publication point.
    #[expect(
        clippy::result_large_err,
        reason = "a structural failure retains the exact unboxed verified facts with the linear settlement capability"
    )]
    pub(super) fn prepare_verified_compute_settlement<'a>(
        &'a mut self,
        token: &SettlementToken,
        receipt: TxPoolComputeAdmissionReceipt,
    ) -> Result<PreparedApply<'a>, PrepareSettlementError> {
        // Ready remains the sole source/economic ordering authority. Even a
        // weaker existing head wins this boundary; the next Ready batch can
        // classify both owners together without an alternate rank compiler.
        if self.scheduler.has_ready() {
            return self
                .prepare_settlement(token, SettlementNext::Ready(receipt.into_ready_facts()));
        }
        if let Err(error) = self.effects.ensure_open() {
            return Err(preserve_settlement_error(
                error.into(),
                SettlementNext::FinalAdmission(receipt),
            ));
        }

        let key = token.hash.clone();
        let existing = match self.entries.get(&key).cloned() {
            Some(existing) if existing.record().version == token.version => existing,
            Some(_) | None => {
                return self
                    .prepare_settlement(token, SettlementNext::Ready(receipt.into_ready_facts()));
            }
        };
        let OwnedTx::PreAccepted(before) = existing else {
            return self
                .prepare_settlement(token, SettlementNext::Ready(receipt.into_ready_facts()));
        };
        if !matches!(&before.phase, PreAcceptedPhase::Computing(_)) {
            return self
                .prepare_settlement(token, SettlementNext::Ready(receipt.into_ready_facts()));
        }
        if let Err(error) = self.validate_compute_acceptance_evidence(&before, &receipt) {
            return match error {
                PlanError::Stale(_) => self
                    .prepare_settlement(token, SettlementNext::Ready(receipt.into_ready_facts())),
                error => Err(preserve_settlement_error(
                    error,
                    SettlementNext::FinalAdmission(receipt),
                )),
            };
        }

        let version = self.clocks.next_version;
        let sequence = self.clocks.next_sequence;
        let clocks = match next_version(version).and_then(|next_version| {
            Ok(AuthorityClocks {
                next_version,
                next_sequence: next_sequence(sequence)?,
                ..self.clocks
            })
        }) {
            Ok(clocks) => clocks,
            Err(error) => {
                return Err(preserve_settlement_error(
                    error,
                    SettlementNext::FinalAdmission(receipt),
                ));
            }
        };
        // From here the receipt is consumed into the candidate Accepted owner.
        // Keep one Arc-only shell for the ordinary fallback; direct success
        // drops it only after the prepared delta owns the same payload.
        let verified = receipt.ready_facts();
        let mut record = before.record.clone();
        record.version = version;
        let (proof, proposal, accepted_at, async_process_start) = receipt.into_membership_parts();
        let after = AcceptedEntry {
            record,
            provenance: before.source.accepted_provenance(),
            proof,
            proposal,
            accepted_at,
        };
        let policy = EffectPolicy::for_preaccepted_source(before.source);
        let mut changes = Vec::new();
        if changes.try_reserve_exact(1).is_err() {
            return self.prepare_settlement(token, SettlementNext::Ready(verified));
        }
        changes.push(IndependentMembershipChange { key, before, after });
        let prepared = match prepare_independent_membership(self, &changes) {
            Ok(IndependentMembershipOutcome::Prepared(prepared)) => prepared,
            Ok(IndependentMembershipOutcome::Coupled) => {
                return self.prepare_settlement(token, SettlementNext::Ready(verified));
            }
            Err(error) if direct_fallback_error(&error) => {
                return self.prepare_settlement(token, SettlementNext::Ready(verified));
            }
            Err(error) => {
                return Err(preserve_settlement_error(
                    error,
                    SettlementNext::Ready(verified),
                ));
            }
        };
        let mut async_process_starts = Vec::new();
        if let Some(started_at) = async_process_start {
            if async_process_starts.try_reserve_exact(1).is_err() {
                return self.prepare_settlement(token, SettlementNext::Ready(verified));
            }
            async_process_starts.push(started_at);
        }
        match self.compile_independent_delta(
            changes,
            prepared,
            clocks,
            sequence,
            async_process_starts,
            policy,
        ) {
            // `receipt` moved the shared payload Arc into the prepared
            // Accepted owner, so dropping the remaining `verified` shell while
            // this borrow is live cannot destroy resolved payload storage.
            Ok(delta) => Ok(PreparedApply {
                authority: self,
                delta,
            }),
            Err(error) if direct_fallback_error(&error) => {
                self.prepare_settlement(token, SettlementNext::Ready(verified))
            }
            Err(error) => Err(preserve_settlement_error(
                error,
                SettlementNext::Ready(verified),
            )),
        }
    }

    /// Compile the mechanical half of independent membership exactly once for
    /// both a Ready batch and the one-member Computing fast path.
    fn compile_independent_delta(
        &mut self,
        changes: Vec<IndependentMembershipChange>,
        prepared: PreparedIndependentMembership,
        clocks: AuthorityClocks,
        source_sequence: ApplySequence,
        async_process_starts: Vec<AsyncProcessStart>,
        policy: EffectPolicy,
    ) -> Result<AuthorityDelta, PlanError> {
        let PreparedIndependentMembership {
            resource,
            projection,
        } = prepared;
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
            .effects
            .plan_publication(&publication, source_sequence)?;
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
        Ok(AuthorityDelta::Independent(IndependentDelta {
            updates,
            owners,
            resource,
            projection,
            scheduler,
            dependency,
            effect,
            clocks,
            async_process_starts,
        }))
    }
}

fn direct_fallback_error(error: &PlanError) -> bool {
    matches!(error, PlanError::Stale(_) | PlanError::Backpressure(_))
}

fn preserve_settlement_error(error: PlanError, next: SettlementNext) -> PrepareSettlementError {
    PrepareSettlementError::Preserve { error, next }
}
