use super::{
    AuthorityFault, CompiledSharedIndependent, IndependentDelta, PlanError, StalePlan,
    TxPoolAuthority,
};
use crate::authority::{
    chain::FinalAdmissionReceipt,
    effect::EffectPolicy,
    scheduler::ReadyKey,
    state::{OwnedTx, PreAcceptedPhase},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct SettlementBatch {
    head: FinalAdmissionReceipt,
    tail: Vec<FinalAdmissionReceipt>,
}

impl SettlementBatch {
    /// Construct one non-empty bounded Ready cut. The scheduler is the sole
    /// production producer; storing the head separately makes an empty cut
    /// unrepresentable at the policy boundary.
    pub(in crate::authority) fn from_validated_ready(
        head: FinalAdmissionReceipt,
        tail: Vec<FinalAdmissionReceipt>,
    ) -> Self {
        Self { head, tail }
    }

    pub(in crate::authority) fn len(&self) -> usize {
        self.tail.len().saturating_add(1)
    }

    fn candidates(&self) -> impl Iterator<Item = &FinalAdmissionReceipt> {
        std::iter::once(&self.head).chain(&self.tail)
    }
}

#[cfg(test)]
#[path = "../tests/support/plan_settlement.rs"]
pub(in crate::authority) mod test_support;

/// Closed Plan result for one strongest contiguous canonical-singleton wave.
/// `Prefix` owns the first incompatible sealed member so runtime can cancel it
/// only after releasing the authority read guard. No weaker suffix is planned.
pub(in crate::authority) enum SharedReadyWaveCompilation {
    Complete(Vec<CompiledSharedIndependent>),
    Prefix(SharedReadyWavePrefix),
    Retry,
    EffectCapacity,
    Error {
        compiled: Vec<CompiledSharedIndependent>,
        error: PlanError,
    },
}

/// Non-empty sealed prefix whose final member is the incompatible boundary.
/// Keeping that member in the already-sized vector avoids a large inline enum
/// and a second allocation on the Ready hot path.
pub(in crate::authority) struct SharedReadyWavePrefix(Vec<CompiledSharedIndependent>);

impl SharedReadyWavePrefix {
    fn new(
        mut compiled: Vec<CompiledSharedIndependent>,
        boundary: CompiledSharedIndependent,
    ) -> Self {
        compiled.push(boundary);
        Self(compiled)
    }

    #[expect(
        clippy::expect_used,
        reason = "the private constructor appends the boundary before this value can exist"
    )]
    pub(in crate::authority) fn into_parts(
        mut self,
    ) -> (Vec<CompiledSharedIndependent>, CompiledSharedIndependent) {
        let boundary = self
            .0
            .pop()
            .expect("SharedReadyWavePrefix::new always appends one boundary");
        (self.0, boundary)
    }
}

struct CandidateFact {
    receipt: FinalAdmissionReceipt,
    policy: EffectPolicy,
    rank: ReadyKey,
}

impl TxPoolAuthority {
    fn ready_candidate_fact(
        &self,
        request: &FinalAdmissionReceipt,
    ) -> Result<CandidateFact, PlanError> {
        let key = request.key().clone();
        let expected = request.expected();
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
        // Clone the bounded owner before reading dependency evidence so no
        // point owner guard can create an owner -> dependency lock edge.
        self.validate_acceptance_evidence(&before, request)?;
        Ok(CandidateFact {
            receipt: request.clone(),
            policy: EffectPolicy::for_preaccepted_source(before.source),
            rank: ReadyKey::from_ready(&before)?,
        })
    }

    fn capture_ready_facts(
        &self,
        batch: &SettlementBatch,
    ) -> Result<Vec<CandidateFact>, PlanError> {
        self.effects.lock().ensure_open()?;
        let mut facts = Vec::with_capacity(batch.len());
        for request in batch.candidates() {
            facts.push(self.ready_candidate_fact(request)?);
        }
        facts.sort_unstable_by(|left, right| right.rank.cmp(&left.rank));
        Ok(facts)
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

    /// Compile the strongest contiguous Ready prefix through the sole
    /// canonical singleton policy/compiler. A wave contains only pairwise
    /// compatible complete physical supports. The first conflict is cancelled
    /// and becomes the stop boundary; no weaker suffix is planned across it.
    pub(in crate::authority) fn compile_shared_ready_wave(
        &self,
        batch: &SettlementBatch,
    ) -> SharedReadyWaveCompilation {
        let mut facts = match self.capture_ready_facts(batch) {
            Ok(facts) => facts,
            Err(PlanError::Stale(_)) => return SharedReadyWaveCompilation::Retry,
            Err(PlanError::Backpressure(super::Backpressure::EffectCapacity)) => {
                return SharedReadyWaveCompilation::EffectCapacity;
            }
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
        let policy = strongest.policy;
        // ReadyKey keeps trusted work ahead of Remote. Only the strongest
        // contiguous effect class participates in this wave, so peer pressure
        // cannot delay a stronger class or merge their effect capacity.
        facts.retain(|fact| fact.policy == policy);

        let mut compiled = Vec::with_capacity(facts.len());
        for fact in facts {
            let candidate = match self
                .compile_shared_candidate_disposition_delta(fact.receipt)
                .and_then(|delta| self.seal_shared_independent(delta))
            {
                Ok(candidate) => candidate,
                Err(PlanError::Stale(_)) if compiled.is_empty() => {
                    return SharedReadyWaveCompilation::Retry;
                }
                Err(PlanError::Backpressure(super::Backpressure::EffectCapacity))
                    if compiled.is_empty() =>
                {
                    return SharedReadyWaveCompilation::EffectCapacity;
                }
                Err(
                    PlanError::Stale(_)
                    | PlanError::Backpressure(super::Backpressure::EffectCapacity),
                ) => return SharedReadyWaveCompilation::Complete(compiled),
                Err(error) => return SharedReadyWaveCompilation::Error { compiled, error },
            };
            if compiled
                .iter()
                .all(|prior| prior.is_compatible_with(&candidate))
            {
                compiled.push(candidate);
                continue;
            }
            return SharedReadyWaveCompilation::Prefix(SharedReadyWavePrefix::new(
                compiled, candidate,
            ));
        }
        SharedReadyWaveCompilation::Complete(compiled)
    }
}
