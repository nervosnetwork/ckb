use super::state::{
    AcceptedStatus, ModelFeeRate, Omega, OwnerLocation, ProposalId, RetainedOwner, RetainedPhase,
    RulesId, Source, Transaction, TxId, WitnessId, WorkPermit, WorkStage,
};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct TemplateSources {
    pub(super) replacement: u16,
    pub(super) proposals: u16,
    pub(super) transactions: u16,
    pub(super) uncles: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TemplateLane {
    Full,
    Reset,
    Proposals,
    Transactions,
    Uncles,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TemplateReceipt {
    pub(super) lane: TemplateLane,
    pub(super) sources: TemplateSources,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TemplateDisposition {
    Captured(TemplateReceipt),
    ReplacementBusy,
    FullPreemptedReset(TemplateReceipt),
    Published(TemplateLane),
    Stale(TemplateLane),
}

/// Equality is the complete observation of a failed template attempt's
/// component-specific source cut.  The concrete production cut is a tuple of
/// monotonic revisions; this finite quotient deliberately forgets their
/// magnitudes because only `same` versus `changed` can authorize another
/// attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TemplateFailureCut(pub(super) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TemplateFailureProgress {
    Parked(TemplateFailureCut),
    RetryAfterChange(TemplateFailureCut),
}

pub(super) const fn template_failure_progress(
    failed: TemplateFailureCut,
    observed: TemplateFailureCut,
) -> TemplateFailureProgress {
    if failed.0 == observed.0 {
        TemplateFailureProgress::Parked(observed)
    } else {
        TemplateFailureProgress::RetryAfterChange(observed)
    }
}

/// Complete externally observable retained-ingress result after protocol-size
/// sealing, non-contextual validation and fallible dependency materialization.
/// None of these outcomes carries structural authority evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModelRetainedIngressOutcome {
    Validated,
    Rejected,
    ProposalUnavailable,
}

impl ModelRetainedIngressOutcome {
    pub(super) const fn service_failure(self) -> Option<ModelServiceFailure> {
        match self {
            Self::Validated | Self::Rejected | Self::ProposalUnavailable => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModelOperationalFailure {
    Cancelled,
    BlockAssemblerDisabled,
    TemplateUnavailable,
    ResourceUnavailable,
    EffectCapacity,
    LifecycleClosed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModelStructuralFault {
    InvalidChainEvidence,
    CounterExhausted,
    EffectLifecycleClosed,
    ResourceProjection,
    MembershipProjection,
    IndexProjection,
    SchedulerProjection,
    DependencyProjection,
    EffectProjection,
}

/// Sealed proof that a structural premise was established by its producer.
/// The private field prevents tests or model callers from pairing an ordinary
/// ingress result with a structural label.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ModelStructuralEvidence {
    fault: ModelStructuralFault,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ModelServiceFailure {
    Operational(ModelOperationalFailure),
    Integrity(ModelStructuralEvidence),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModelFailureDisposition {
    Ordinary(ModelOperationalFailure),
    Integrity(ModelStructuralFault),
}

pub(super) const fn authority_structural_failure(
    fault: ModelStructuralFault,
) -> Option<ModelServiceFailure> {
    match fault {
        ModelStructuralFault::InvalidChainEvidence
        | ModelStructuralFault::EffectLifecycleClosed => None,
        ModelStructuralFault::CounterExhausted
        | ModelStructuralFault::ResourceProjection
        | ModelStructuralFault::MembershipProjection
        | ModelStructuralFault::IndexProjection
        | ModelStructuralFault::SchedulerProjection
        | ModelStructuralFault::DependencyProjection
        | ModelStructuralFault::EffectProjection => {
            Some(ModelServiceFailure::Integrity(ModelStructuralEvidence {
                fault,
            }))
        }
    }
}

pub(super) const fn chain_structural_failure(fault: ModelStructuralFault) -> ModelServiceFailure {
    ModelServiceFailure::Integrity(ModelStructuralEvidence { fault })
}

pub(super) const fn service_failure_disposition(
    failure: ModelServiceFailure,
) -> ModelFailureDisposition {
    match failure {
        ModelServiceFailure::Operational(failure) => ModelFailureDisposition::Ordinary(failure),
        ModelServiceFailure::Integrity(evidence) => {
            ModelFailureDisposition::Integrity(evidence.fault)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModelRecoveryAdmissionFailure {
    InvalidTransaction,
    ResourceUnavailable,
}

pub(super) const fn recovery_admission_disposition(
    failure: ModelRecoveryAdmissionFailure,
) -> ModelFailureDisposition {
    match failure {
        ModelRecoveryAdmissionFailure::InvalidTransaction => {
            ModelFailureDisposition::Integrity(ModelStructuralFault::InvalidChainEvidence)
        }
        ModelRecoveryAdmissionFailure::ResourceUnavailable => {
            ModelFailureDisposition::Ordinary(ModelOperationalFailure::ResourceUnavailable)
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct TemplateProtocol {
    pub(super) current: TemplateSources,
    pub(super) published: TemplateSources,
    replacement_claim: Option<TemplateLane>,
}

impl TemplateProtocol {
    pub(super) fn advance(&mut self, lane: TemplateLane) -> bool {
        let source = match lane {
            TemplateLane::Full | TemplateLane::Reset => &mut self.current.replacement,
            TemplateLane::Proposals => &mut self.current.proposals,
            TemplateLane::Transactions => &mut self.current.transactions,
            TemplateLane::Uncles => &mut self.current.uncles,
        };
        let Some(next) = source.checked_add(1) else {
            return false;
        };
        *source = next;
        true
    }

    pub(super) fn capture(&mut self, lane: TemplateLane) -> TemplateDisposition {
        match lane {
            TemplateLane::Full | TemplateLane::Reset => match self.replacement_claim {
                None => {
                    if !self.bump_replacement() {
                        return TemplateDisposition::ReplacementBusy;
                    }
                    self.replacement_claim = Some(lane);
                    TemplateDisposition::Captured(TemplateReceipt {
                        lane,
                        sources: self.current,
                    })
                }
                Some(TemplateLane::Reset) if lane == TemplateLane::Full => {
                    if !self.bump_replacement() {
                        return TemplateDisposition::ReplacementBusy;
                    }
                    self.replacement_claim = Some(TemplateLane::Full);
                    TemplateDisposition::FullPreemptedReset(TemplateReceipt {
                        lane,
                        sources: self.current,
                    })
                }
                Some(_) => TemplateDisposition::ReplacementBusy,
            },
            TemplateLane::Proposals | TemplateLane::Transactions | TemplateLane::Uncles => {
                TemplateDisposition::Captured(TemplateReceipt {
                    lane,
                    sources: self.current,
                })
            }
        }
    }

    pub(super) fn publish(&mut self, receipt: TemplateReceipt) -> TemplateDisposition {
        let current = match receipt.lane {
            TemplateLane::Full | TemplateLane::Reset => {
                self.replacement_claim == Some(receipt.lane) && receipt.sources == self.current
            }
            TemplateLane::Proposals => {
                receipt.sources.replacement == self.current.replacement
                    && receipt.sources.proposals == self.current.proposals
            }
            TemplateLane::Transactions => {
                receipt.sources.replacement == self.current.replacement
                    && receipt.sources.transactions == self.current.transactions
            }
            TemplateLane::Uncles => {
                receipt.sources.replacement == self.current.replacement
                    && receipt.sources.uncles == self.current.uncles
            }
        };
        if !current {
            if matches!(receipt.lane, TemplateLane::Full | TemplateLane::Reset)
                && self.replacement_claim == Some(receipt.lane)
            {
                self.replacement_claim = None;
            }
            return TemplateDisposition::Stale(receipt.lane);
        }
        match receipt.lane {
            TemplateLane::Full | TemplateLane::Reset => {
                self.published = receipt.sources;
                self.replacement_claim = None;
            }
            TemplateLane::Proposals => self.published.proposals = receipt.sources.proposals,
            TemplateLane::Transactions => {
                self.published.transactions = receipt.sources.transactions;
            }
            TemplateLane::Uncles => self.published.uncles = receipt.sources.uncles,
        }
        TemplateDisposition::Published(receipt.lane)
    }

    fn bump_replacement(&mut self) -> bool {
        let Some(next) = self.current.replacement.checked_add(1) else {
            return false;
        };
        self.current.replacement = next;
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CandidateUncleInput {
    pub(super) id: u8,
    pub(super) proposals: BTreeSet<ProposalId>,
    pub(super) serialized_bytes: usize,
}

impl CandidateUncleInput {
    pub(super) const fn new(
        id: u8,
        proposals: BTreeSet<ProposalId>,
        serialized_bytes: usize,
    ) -> Self {
        Self {
            id,
            proposals,
            serialized_bytes,
        }
    }
}

pub(super) fn filter_uncles_conflicting_with_proposals(
    uncles: impl IntoIterator<Item = CandidateUncleInput>,
    proposals: &BTreeSet<ProposalId>,
) -> Vec<CandidateUncleInput> {
    uncles
        .into_iter()
        .filter(|uncle| uncle.proposals.is_disjoint(proposals))
        .collect()
}

pub(super) fn persistence_projection(omega: &Omega) -> Vec<TxId> {
    omega
        .authority
        .owners
        .iter()
        .filter_map(|(id, owner)| {
            (matches!(owner.location, OwnerLocation::Accepted { .. })
                || matches!(owner.retained_source(), Some(Source::Recovery(_))))
            .then_some(*id)
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct VerificationKey {
    pub(super) witness: WitnessId,
    pub(super) rules: RulesId,
}

impl VerificationKey {
    pub(super) const fn new(witness: WitnessId, rules: RulesId) -> Self {
        Self { witness, rules }
    }
}

/// The deterministic script observation selected by one verification key.
///
/// A function `VerificationKey -> ModelScriptBehavior` is the finite-model
/// counterpart of the trusted CKB premise that witness identity plus the
/// active script-rule generation fixes script validity and consumed cycles.
/// The current cycle limit deliberately is not an input to this behavior: it
/// can only truncate an otherwise deterministic execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ModelScriptBehavior {
    valid: bool,
    cycles: u64,
}

impl ModelScriptBehavior {
    pub(super) const fn valid(cycles: u64) -> Self {
        Self {
            valid: true,
            cycles,
        }
    }

    pub(super) const fn invalid() -> Self {
        Self {
            valid: false,
            cycles: 0,
        }
    }
}

pub(super) type ModelScriptSemantics = fn(VerificationKey) -> ModelScriptBehavior;

/// Private construction witness for a successful VM execution. Neither cache
/// lookup nor a caller-supplied cycle count can construct this seal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VmSuccessSeal(());

/// The complete cacheable quotient of successful script verification.
///
/// Fee, capacity, time, DAO and the current cycle limit are intentionally
/// absent. The key is retained inside the proof as well as in the cache map so
/// a malformed key/value pairing remains observable instead of silently
/// becoming verification evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ModelScriptProof {
    key: VerificationKey,
    cycles: u64,
    _success: VmSuccessSeal,
}

impl ModelScriptProof {
    pub(super) const fn key(self) -> VerificationKey {
        self.key
    }

    pub(super) const fn cycles(self) -> u64 {
        self.cycles
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModelScriptRejection {
    InvalidScript,
    ExceededMaximumCycles(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModelScriptExecution {
    Verified(ModelScriptProof),
    Rejected(ModelScriptRejection),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModelScriptReuse {
    Miss,
    Verified(ModelScriptProof),
    Rejected(ModelScriptRejection),
}

/// Execute the canonical cold script path. This is the only constructor for a
/// `ModelScriptProof`, making successful VM provenance a static model fact.
pub(super) fn execute_model_script_vm(
    key: VerificationKey,
    current_max: u64,
    semantics: ModelScriptSemantics,
) -> ModelScriptExecution {
    let behavior = semantics(key);
    if !behavior.valid {
        ModelScriptExecution::Rejected(ModelScriptRejection::InvalidScript)
    } else if behavior.cycles > current_max {
        ModelScriptExecution::Rejected(ModelScriptRejection::ExceededMaximumCycles(current_max))
    } else {
        ModelScriptExecution::Verified(ModelScriptProof {
            key,
            cycles: behavior.cycles,
            _success: VmSuccessSeal(()),
        })
    }
}

/// Reuse one successful script proof under the exact requested identity and
/// current limit. Keeping `current_max` outside `VerificationKey` maximizes
/// reuse; this pointwise comparison is the necessary and sufficient guard.
pub(super) fn reuse_model_script_proof(
    proof: ModelScriptProof,
    requested: VerificationKey,
    current_max: u64,
) -> ModelScriptReuse {
    if proof.key != requested {
        ModelScriptReuse::Miss
    } else if proof.cycles > current_max {
        ModelScriptReuse::Rejected(ModelScriptRejection::ExceededMaximumCycles(current_max))
    } else {
        ModelScriptReuse::Verified(proof)
    }
}

/// DAO-aware fee/minimum-fee evidence and the accepted-residency projection
/// produced from one tx-pool resolution cut. Final-admission location refresh
/// happens later and is modeled separately: the current production path
/// carries these values across that refresh, which must remain observable
/// until X0-to-X3 adjudicates whether the producer cut is invariant or stale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ModelTxPoolResolutionReceipt {
    valid: bool,
    fee: u64,
    accepted_resident_bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelMinimumFeeObservation {
    Accepted { actual: u64, required: u64 },
    Rejected { actual: u64, required: u64 },
}

/// Exact production minimum-fee arithmetic. Other resolution failures remain
/// separate; this relation owns only the configured fee-rate observation.
pub(crate) const fn minimum_fee_observation(
    actual: u64,
    serialized_bytes: u64,
    minimum_rate: ModelFeeRate,
) -> ModelMinimumFeeObservation {
    let required = minimum_rate.fee(serialized_bytes);
    if actual < required {
        ModelMinimumFeeObservation::Rejected { actual, required }
    } else {
        ModelMinimumFeeObservation::Accepted { actual, required }
    }
}

impl ModelTxPoolResolutionReceipt {
    pub(super) const fn current(valid: bool, fee: u64, accepted_resident_bytes: usize) -> Self {
        Self {
            valid,
            fee,
            accepted_resident_bytes,
        }
    }
}

/// Fresh occupied/full-capacity evidence for the exact resolved payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ModelFreshCapacityReceipt {
    valid: bool,
}

impl ModelFreshCapacityReceipt {
    pub(super) const fn new(valid: bool) -> Self {
        Self { valid }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ModelFreshTimeReceipt {
    eligible: bool,
}

impl ModelFreshTimeReceipt {
    pub(super) const fn new(eligible: bool) -> Self {
        Self { eligible }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ModelFreshDaoReceipt {
    valid: bool,
}

impl ModelFreshDaoReceipt {
    pub(super) const fn new(valid: bool) -> Self {
        Self { valid }
    }
}

/// Non-script tx-pool evidence is supplied independently of an optional cached
/// script proof. Fee remains in the resolution receipt and cannot enter the
/// script-cache quotient.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ModelFreshVerificationReceipts {
    capacity: ModelFreshCapacityReceipt,
    time: ModelFreshTimeReceipt,
    dao: ModelFreshDaoReceipt,
}

impl ModelFreshVerificationReceipts {
    pub(super) const fn new(
        capacity: ModelFreshCapacityReceipt,
        time: ModelFreshTimeReceipt,
        dao: ModelFreshDaoReceipt,
    ) -> Self {
        Self {
            capacity,
            time,
            dao,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModelVerificationRejection {
    Capacity,
    Fee,
    Time,
    Script(ModelScriptRejection),
    Dao,
    DeclaredWrongCycles { declared: u64, actual: u64 },
}

/// A peer cycle claim admitted only when it is inside the consensus work
/// envelope. The private coordinates make `declared > consensus_max`
/// unrepresentable after ingress, while retaining `declared` as the tighter
/// VM limit that bounds work before exact-cycle comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ModelRemoteCycleLimit {
    declared: u64,
    consensus_max: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModelRemoteCycleLimitError {
    ExceedsConsensusMax { declared: u64, consensus_max: u64 },
}

/// The two production entry routes for a peer-declared cycle value. Both must
/// refine the same checked tx-pool ingress relation; the relay check is a peer
/// policy fast path, not the authority boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModelRemoteIngressRoute {
    NetworkRelay,
    DirectTxPool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModelRemoteCycleObservation {
    Sealed(ModelRemoteCycleLimit),
    Rejected(ModelRemoteCycleLimitError),
}

/// Exact route quotient: both relay and direct-controller paths terminate at
/// the same checked tx-pool boundary before any ownership or VM work.
pub(super) const fn remote_cycle_observation(
    _route: ModelRemoteIngressRoute,
    declared: u64,
    consensus_max: u64,
) -> ModelRemoteCycleObservation {
    match ModelRemoteCycleLimit::checked(declared, consensus_max) {
        Ok(limit) => ModelRemoteCycleObservation::Sealed(limit),
        Err(error) => ModelRemoteCycleObservation::Rejected(error),
    }
}

impl ModelRemoteCycleLimit {
    pub(super) const fn checked(
        declared: u64,
        consensus_max: u64,
    ) -> Result<Self, ModelRemoteCycleLimitError> {
        if declared <= consensus_max {
            Ok(Self {
                declared,
                consensus_max,
            })
        } else {
            Err(ModelRemoteCycleLimitError::ExceedsConsensusMax {
                declared,
                consensus_max,
            })
        }
    }

    pub(super) const fn declared(self) -> u64 {
        self.declared
    }

    pub(super) const fn consensus_max(self) -> u64 {
        self.consensus_max
    }

    pub(super) const fn bounds_vm_work(self, vm_work: u64) -> bool {
        vm_work <= self.declared && self.declared <= self.consensus_max
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModelTxPoolCyclePolicy {
    Trusted { current_max: u64 },
    RemoteDeclared(ModelRemoteCycleLimit),
}

impl ModelTxPoolCyclePolicy {
    const fn current_max(self) -> u64 {
        match self {
            Self::Trusted { current_max } => current_max,
            Self::RemoteDeclared(limit) => limit.declared(),
        }
    }
}

/// Tx-pool verification has a fee-producing resolution prelude and may carry
/// a remote exact-cycle claim. Cache presence can replace only the script step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ModelTxPoolVerificationContext {
    resolution: ModelTxPoolResolutionReceipt,
    cycles: ModelTxPoolCyclePolicy,
    cached: Option<ModelScriptProof>,
}

impl ModelTxPoolVerificationContext {
    pub(super) const fn trusted(
        resolution: ModelTxPoolResolutionReceipt,
        current_max: u64,
        cached: Option<ModelScriptProof>,
    ) -> Self {
        Self {
            resolution,
            cycles: ModelTxPoolCyclePolicy::Trusted { current_max },
            cached,
        }
    }

    pub(super) const fn remote(
        resolution: ModelTxPoolResolutionReceipt,
        limit: ModelRemoteCycleLimit,
        cached: Option<ModelScriptProof>,
    ) -> Self {
        Self {
            resolution,
            cycles: ModelTxPoolCyclePolicy::RemoteDeclared(limit),
            cached,
        }
    }
}

/// A successful tx-pool receipt keeps fee and script authorities separate. The
/// script proof is the only part eligible for shared cache publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ModelTxPoolVerificationReceipt {
    script: ModelScriptProof,
    fee: u64,
    accepted_resident_bytes: usize,
}

impl ModelTxPoolVerificationReceipt {
    pub(super) const fn script_proof(self) -> ModelScriptProof {
        self.script
    }

    pub(super) const fn cycles(self) -> u64 {
        self.script.cycles
    }

    pub(super) const fn fee(self) -> u64 {
        self.fee
    }

    pub(super) const fn accepted_resident_bytes(self) -> usize {
        self.accepted_resident_bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModelVerificationObservation {
    TxPoolVerified(ModelTxPoolVerificationReceipt),
    Rejected(ModelVerificationRejection),
}

fn execute_or_reuse_model_script(
    key: VerificationKey,
    current_max: u64,
    cached: Option<ModelScriptProof>,
    semantics: ModelScriptSemantics,
) -> ModelScriptExecution {
    if let Some(proof) = cached {
        match reuse_model_script_proof(proof, key, current_max) {
            ModelScriptReuse::Miss => {}
            ModelScriptReuse::Verified(proof) => {
                return ModelScriptExecution::Verified(proof);
            }
            ModelScriptReuse::Rejected(rejection) => {
                return ModelScriptExecution::Rejected(rejection);
            }
        }
    }
    execute_model_script_vm(key, current_max, semantics)
}

fn observe_tx_pool_resolution(
    receipt: ModelTxPoolResolutionReceipt,
) -> Result<(u64, usize), ModelVerificationObservation> {
    match receipt {
        ModelTxPoolResolutionReceipt {
            valid: true,
            fee,
            accepted_resident_bytes,
        } => Ok((fee, accepted_resident_bytes)),
        ModelTxPoolResolutionReceipt { valid: false, .. } => Err(
            ModelVerificationObservation::Rejected(ModelVerificationRejection::Fee),
        ),
    }
}

fn observe_script(
    key: VerificationKey,
    current_max: u64,
    cached: Option<ModelScriptProof>,
    semantics: ModelScriptSemantics,
) -> Result<ModelScriptProof, ModelVerificationObservation> {
    match execute_or_reuse_model_script(key, current_max, cached, semantics) {
        ModelScriptExecution::Verified(proof) => Ok(proof),
        ModelScriptExecution::Rejected(rejection) => Err(ModelVerificationObservation::Rejected(
            ModelVerificationRejection::Script(rejection),
        )),
    }
}

/// Exact tx-pool orchestration: resolution fee -> time -> capacity ->
/// script/current limit -> DAO -> remote-declared equality. Cache presence can
/// replace only the script VM step. Downstream block verification is a shared-
/// API compatibility observer, not part of the tx-pool architecture model.
pub(super) fn model_tx_pool_verification(
    key: VerificationKey,
    semantics: ModelScriptSemantics,
    context: ModelTxPoolVerificationContext,
    fresh: ModelFreshVerificationReceipts,
) -> ModelVerificationObservation {
    let (fee, accepted_resident_bytes) = match observe_tx_pool_resolution(context.resolution) {
        Ok(receipt) => receipt,
        Err(observation) => return observation,
    };
    if !fresh.time.eligible {
        return ModelVerificationObservation::Rejected(ModelVerificationRejection::Time);
    }
    if !fresh.capacity.valid {
        return ModelVerificationObservation::Rejected(ModelVerificationRejection::Capacity);
    }
    let script = match observe_script(key, context.cycles.current_max(), context.cached, semantics)
    {
        Ok(proof) => proof,
        Err(observation) => return observation,
    };
    if !fresh.dao.valid {
        return ModelVerificationObservation::Rejected(ModelVerificationRejection::Dao);
    }
    if let ModelTxPoolCyclePolicy::RemoteDeclared(limit) = context.cycles
        && limit.declared() != script.cycles
    {
        return ModelVerificationObservation::Rejected(
            ModelVerificationRejection::DeclaredWrongCycles {
                declared: limit.declared(),
                actual: script.cycles,
            },
        );
    }
    ModelVerificationObservation::TxPoolVerified(ModelTxPoolVerificationReceipt {
        script,
        fee,
        accepted_resident_bytes,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QueryStatus {
    Pending,
    Proposed,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QuerySubject {
    Accepted(AcceptedStatus),
    PreAcceptedPending,
    PreAcceptedProposalAware(AcceptedStatus),
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct QueryProjection {
    pub(super) status: QueryStatus,
    pub(super) minimum_replacement_fee: Option<u64>,
}

/// The two finite relay lookup normal forms. The wire protocol supplies the
/// complete raw hash; `ProposalShort` exists only as the minimum collision
/// counterexample against incorrectly reusing the compact-block index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RelayLookupIdentity {
    FullRaw,
    ProposalShort,
}

pub(super) fn relay_query_owner(
    omega: &Omega,
    requested: &Transaction,
    identity: RelayLookupIdentity,
) -> Option<TxId> {
    match identity {
        RelayLookupIdentity::FullRaw => {
            omega.authority.owners.get(&requested.id).and_then(|owner| {
                matches!(owner.location, OwnerLocation::Accepted { .. })
                    .then_some(owner.transaction.id)
            })
        }
        RelayLookupIdentity::ProposalShort => omega.authority.owners.values().find_map(|owner| {
            (owner.transaction.proposal == requested.proposal
                && matches!(owner.location, OwnerLocation::Accepted { .. }))
            .then_some(owner.transaction.id)
        }),
    }
}

pub(super) fn query_projection(
    subject: QuerySubject,
    descendant_fee: u64,
    minimum_rate: Option<ModelFeeRate>,
    transaction_size: u64,
) -> QueryProjection {
    let status = match subject {
        QuerySubject::Accepted(AcceptedStatus::Proposed)
        | QuerySubject::PreAcceptedProposalAware(AcceptedStatus::Proposed) => QueryStatus::Proposed,
        QuerySubject::Accepted(AcceptedStatus::Pending | AcceptedStatus::Gap)
        | QuerySubject::PreAcceptedPending
        | QuerySubject::PreAcceptedProposalAware(AcceptedStatus::Pending | AcceptedStatus::Gap) => {
            QueryStatus::Pending
        }
        QuerySubject::Hidden => QueryStatus::Unknown,
    };
    let minimum_replacement_fee = matches!(
        subject,
        QuerySubject::Accepted(AcceptedStatus::Pending | AcceptedStatus::Gap)
    )
    .then(|| minimum_rate.and_then(|rate| descendant_fee.checked_add(rate.fee(transaction_size))))
    .flatten();
    QueryProjection {
        status,
        minimum_replacement_fee,
    }
}

pub(super) fn query_subject(omega: &Omega, transaction: TxId) -> QuerySubject {
    let Some(owner) = omega.authority.owners.get(&transaction) else {
        return QuerySubject::Hidden;
    };
    match &owner.location {
        OwnerLocation::Accepted { .. } => {
            QuerySubject::Accepted(omega.proposal_status(&owner.transaction))
        }
        OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Queued(WorkStage::Verify(_)) | RetainedPhase::Ready(_),
            ..
        }) => QuerySubject::PreAcceptedProposalAware(omega.proposal_status(&owner.transaction)),
        OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Computing(active),
            ..
        }) if matches!(active.permit, WorkPermit::VerifyOnly(_)) => {
            QuerySubject::PreAcceptedProposalAware(omega.proposal_status(&owner.transaction))
        }
        OwnerLocation::Retained(_) => QuerySubject::PreAcceptedPending,
        OwnerLocation::ReplacementHistory { .. } => QuerySubject::Hidden,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CallbackAccess {
    AuthorityMutation,
    CoherentRead,
    NonblockingDerivedControl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CallbackDisposition {
    Allowed,
    ReentrantMutationRejected,
}

pub(super) fn callback_disposition(
    callback_active: bool,
    access: CallbackAccess,
) -> CallbackDisposition {
    if callback_active && access == CallbackAccess::AuthorityMutation {
        CallbackDisposition::ReentrantMutationRejected
    } else {
        CallbackDisposition::Allowed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VerificationControl {
    Running,
    Suspended,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ActiveVerificationAction {
    Continue,
    ReturnCapability,
}

impl VerificationControl {
    pub(super) fn suspend(self) -> Self {
        match self {
            Self::Running => Self::Suspended,
            state => state,
        }
    }

    pub(super) fn resume(self) -> Self {
        match self {
            Self::Suspended => Self::Running,
            state => state,
        }
    }

    pub(super) const fn stop(self) -> Self {
        Self::Stopped
    }

    pub(super) const fn checkout_allowed(self) -> bool {
        matches!(self, Self::Running)
    }

    pub(super) const fn active_action(self) -> ActiveVerificationAction {
        match self {
            Self::Running | Self::Suspended => ActiveVerificationAction::Continue,
            Self::Stopped => ActiveVerificationAction::ReturnCapability,
        }
    }
}
