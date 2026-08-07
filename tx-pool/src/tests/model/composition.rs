//! Executable M2 composition algebra built over the independent M1 oracle.
//!
//! A footprint is transient evidence for one coherent authority cut. It is
//! never stored in `Omega`, and this module owns no transaction lifecycle
//! state. Failure to construct a proof is an ordinary coupled disposition.

use super::kernel::{Completion, KernelCommand, KernelDisposition, KernelStep, ReadyCapture};
use super::permit::{
    FairPermitScheduler, ImmediatePermitDisposition, PermitClass, PermitGrant, PermitRequest,
    PermitRequestDisposition, PermitRequestId, RetainedPermitToken,
};
use super::state::{
    ApplyStamp, Arrival, CapabilityId, CellId, EntryVersion, EvidenceContext,
    FinishedWorkCapability, HeaderId, InputOrigin, LogicalEffect, ModelInvariantError, Omega,
    Owner, OwnerLocation, ProposalId, ResolvedEvidence, ResourceVector, RetainedOwner,
    RetainedPhase, Source, Transaction, TxId, WorkCapability,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RelationKind {
    SharedInput,
    CandidateProducesInput,
    CandidateProducesRead,
    CandidateSpendsRead,
    DuplicateOutput,
    ProposalCollision,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CouplingReason {
    InvalidInitial(ModelInvariantError),
    NotReady(TxId),
    StaleEvidence(TxId),
    PoolOrigin {
        transaction: TxId,
        parent: TxId,
    },
    CandidateRelation {
        first: TxId,
        second: TxId,
        cell: Option<CellId>,
        kind: RelationKind,
    },
    AcceptedRelation {
        candidate: TxId,
        accepted: TxId,
        cell: CellId,
        kind: RelationKind,
    },
    AcceptedCapacity(TxId),
    EffectCapacity(TxId),
    Arithmetic,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CohortClass {
    Empty,
    IndependentComposable,
    CanonicalOrdered,
    Coupled(CouplingReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OrderedBatchFamily {
    RetainedIngress,
    CompletionDrain,
    ComputeExchange,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum BatchPlanError {
    Empty,
    Coupled(CouplingReason),
    UnsupportedCommand,
    CounterExhausted,
    StampNormalization,
    UnexpectedDisposition,
    InvalidResult(ModelInvariantError),
    DuplicateCapability(CapabilityId),
    MissingFinishedCapability(CapabilityId),
    InvalidPermitToken(PermitRequestId),
    CompletionBatchBound,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CanonicalBatchPlan {
    expected: Omega,
    after: Omega,
    pub(super) class: CohortClass,
    pub(super) dispositions: Vec<KernelDisposition>,
    pub(super) sequential_apply_count: u16,
    pub(super) committed_stamp: Option<ApplyStamp>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum BatchApplyDisposition {
    Stale,
    Applied {
        stamp: Option<ApplyStamp>,
        dispositions: Vec<KernelDisposition>,
    },
}

#[must_use = "a retained permit batch owns linear scheduler grants"]
#[derive(Debug, PartialEq, Eq)]
pub(super) struct RetainedPermitGrant {
    tokens: Vec<RetainedPermitToken>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RetainedPermitGrantErrorKind {
    MixedDomains {
        expected: super::permit::PermitDomain,
        observed: super::permit::PermitDomain,
    },
    DuplicateIdentity {
        request: PermitRequestId,
    },
}

/// A rejected batch construction returns every move-only token. Mixed
/// scheduler domains and duplicate identities therefore cannot become a
/// `RetainedPermitGrant` or silently lose capacity.
#[must_use = "a rejected permit batch still owns every supplied token"]
#[derive(Debug, PartialEq, Eq)]
pub(super) struct RetainedPermitGrantError {
    pub(super) kind: RetainedPermitGrantErrorKind,
    tokens: Vec<RetainedPermitToken>,
}

impl RetainedPermitGrantError {
    pub(super) fn into_tokens(self) -> Vec<RetainedPermitToken> {
        self.tokens
    }
}

impl RetainedPermitGrant {
    pub(super) fn try_from_tokens(
        tokens: impl IntoIterator<Item = RetainedPermitToken>,
    ) -> Result<Self, RetainedPermitGrantError> {
        let mut tokens = tokens.into_iter().collect::<Vec<_>>();
        tokens.sort_unstable_by_key(RetainedPermitToken::identity);
        let Some(expected) = tokens.first().map(|token| token.identity().0) else {
            return Ok(Self { tokens });
        };
        if let Some(observed) = tokens
            .iter()
            .map(|token| token.identity().0)
            .find(|domain| *domain != expected)
        {
            return Err(RetainedPermitGrantError {
                kind: RetainedPermitGrantErrorKind::MixedDomains { expected, observed },
                tokens,
            });
        }
        if let Some(request) = tokens.windows(2).find_map(|pair| {
            (pair[0].identity() == pair[1].identity()).then(|| pair[0].request().id)
        }) {
            return Err(RetainedPermitGrantError {
                kind: RetainedPermitGrantErrorKind::DuplicateIdentity { request },
                tokens,
            });
        }
        Ok(Self { tokens })
    }

    pub(super) fn empty() -> Self {
        Self { tokens: Vec::new() }
    }

    pub(super) fn request_ids(&self) -> BTreeSet<PermitRequestId> {
        self.tokens.iter().map(|token| token.request().id).collect()
    }

    pub(super) fn into_tokens(self) -> Vec<RetainedPermitToken> {
        self.tokens
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RetainedAcquireStop {
    NoImmediatePermit(PermitRequestId),
    Duplicate(PermitRequestId),
    InvalidSchedulerState(PermitRequestId),
}

#[must_use = "an acquire disposition may contain linear scheduler grants"]
#[derive(Debug, PartialEq, Eq)]
pub(super) enum RetainedAcquireDisposition {
    Granted {
        grants: RetainedPermitGrant,
        stopped_by: Option<RetainedAcquireStop>,
    },
    Waiting(PermitRequestId),
    Busy(PermitRequestId),
    QueueFull(PermitRequestId),
    Duplicate(PermitRequestId),
    InvalidSchedulerState(PermitRequestId),
    NoWait,
    DeliveryPending(PermitRequestId),
    LostWait(PermitRequestId),
    UnexpectedGrant(PermitGrant),
    InvalidGrantBatch(RetainedPermitGrantError),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct RetainedPermitAcquirer {
    waiting: Option<PermitRequestId>,
}

impl RetainedPermitAcquirer {
    pub(super) fn acquire(
        &mut self,
        scheduler: &mut FairPermitScheduler,
        first: PermitRequestId,
        additional: impl IntoIterator<Item = PermitRequestId>,
    ) -> RetainedAcquireDisposition {
        if let Some(waiting) = self.waiting {
            return RetainedAcquireDisposition::Busy(waiting);
        }
        let request = PermitRequest {
            id: first,
            class: PermitClass::Retained,
        };
        match scheduler.request(request) {
            PermitRequestDisposition::Granted {
                grant: PermitGrant::Retained(token),
            } => Self::fill_immediate(scheduler, token, additional),
            PermitRequestDisposition::Granted { grant } => {
                RetainedAcquireDisposition::UnexpectedGrant(grant)
            }
            PermitRequestDisposition::Queued(request) => {
                self.waiting = Some(request);
                RetainedAcquireDisposition::Waiting(request)
            }
            PermitRequestDisposition::QueueFull(request) => {
                RetainedAcquireDisposition::QueueFull(request)
            }
            PermitRequestDisposition::Duplicate(request) => {
                RetainedAcquireDisposition::Duplicate(request)
            }
            PermitRequestDisposition::InvalidSchedulerState(request) => {
                RetainedAcquireDisposition::InvalidSchedulerState(request)
            }
        }
    }

    pub(super) fn resume(
        &mut self,
        scheduler: &mut FairPermitScheduler,
        delivered: Option<PermitGrant>,
        additional: impl IntoIterator<Item = PermitRequestId>,
    ) -> RetainedAcquireDisposition {
        let Some(waiting) = self.waiting else {
            return delivered.map_or(
                RetainedAcquireDisposition::NoWait,
                RetainedAcquireDisposition::UnexpectedGrant,
            );
        };
        if let Some(grant) = delivered {
            return match grant {
                PermitGrant::Retained(token) if token.request().id == waiting => {
                    self.waiting = None;
                    Self::fill_immediate(scheduler, token, additional)
                }
                grant => RetainedAcquireDisposition::UnexpectedGrant(grant),
            };
        }
        if scheduler.waiting_position(waiting).is_some() {
            return RetainedAcquireDisposition::Waiting(waiting);
        }
        if scheduler.is_active(waiting) {
            return RetainedAcquireDisposition::DeliveryPending(waiting);
        }
        self.waiting = None;
        RetainedAcquireDisposition::LostWait(waiting)
    }

    fn fill_immediate(
        scheduler: &mut FairPermitScheduler,
        first: RetainedPermitToken,
        additional: impl IntoIterator<Item = PermitRequestId>,
    ) -> RetainedAcquireDisposition {
        let mut grants = vec![first];
        let mut stopped_by = None;
        for request in additional {
            let request = PermitRequest {
                id: request,
                class: PermitClass::Retained,
            };
            match scheduler.try_request(request) {
                ImmediatePermitDisposition::Granted {
                    grant: PermitGrant::Retained(token),
                } => {
                    grants.push(token);
                }
                ImmediatePermitDisposition::Granted { grant } => {
                    return RetainedAcquireDisposition::UnexpectedGrant(grant);
                }
                ImmediatePermitDisposition::Unavailable(request) => {
                    stopped_by = Some(RetainedAcquireStop::NoImmediatePermit(request));
                    break;
                }
                ImmediatePermitDisposition::Duplicate(request) => {
                    stopped_by = Some(RetainedAcquireStop::Duplicate(request));
                    break;
                }
                ImmediatePermitDisposition::InvalidSchedulerState(request) => {
                    stopped_by = Some(RetainedAcquireStop::InvalidSchedulerState(request));
                    break;
                }
            }
        }
        match RetainedPermitGrant::try_from_tokens(grants) {
            Ok(grants) => RetainedAcquireDisposition::Granted { grants, stopped_by },
            Err(error) => RetainedAcquireDisposition::InvalidGrantBatch(error),
        }
    }
}

#[must_use = "a compute exchange plan owns linear scheduler grants"]
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ComputeExchangePlan {
    pub(super) batch: CanonicalBatchPlan,
    pub(super) attempted: Vec<CapabilityId>,
    pub(super) settled: Vec<CapabilityId>,
    pub(super) blocked: Vec<CapabilityId>,
    pub(super) assigned: Vec<(RetainedPermitToken, WorkCapability)>,
    pub(super) unused_grants: Vec<RetainedPermitToken>,
}

#[must_use = "an execution completion owns its permit until settlement"]
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ExecutionCompletion {
    pub(super) permit: RetainedPermitToken,
    pub(super) completion: Completion,
}

#[derive(Debug, PartialEq, Eq)]
enum CompletionDrainMember {
    Finished(ExecutionCompletion),
    Retired(ExecutionCompletion),
}

impl CompletionDrainMember {
    fn into_completion(self) -> ExecutionCompletion {
        match self {
            Self::Finished(completion) | Self::Retired(completion) => completion,
        }
    }
}

#[must_use = "a completion drain plan owns every submitted completion"]
#[derive(Debug, PartialEq, Eq)]
pub(super) struct CompletionDrainPlan {
    pub(super) batch: CanonicalBatchPlan,
    members: Vec<CompletionDrainMember>,
}

/// Planning is read-only. Any ordinary rejection returns every move-only
/// completion token to its caller; only a committed Apply may transfer it.
#[must_use = "a rejected completion plan returns every submitted completion"]
#[derive(Debug, PartialEq, Eq)]
pub(super) struct CompletionDrainPlanFailure {
    pub(super) error: BatchPlanError,
    pub(super) completions: Vec<ExecutionCompletion>,
}

/// A compute-exchange rejection likewise returns the exact requested work and
/// every fair-scheduler grant. This makes non-commit conservation structural
/// instead of depending on callers reconstructing a lease from scheduler
/// observation.
#[must_use = "a rejected exchange plan returns every grant and work identifier"]
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ComputeExchangePlanFailure {
    pub(super) error: BatchPlanError,
    pub(super) finished: Vec<CapabilityId>,
    pub(super) grants: RetainedPermitGrant,
}

#[must_use = "a stale completion apply returns every submitted completion"]
#[derive(Debug, PartialEq, Eq)]
pub(super) enum CompletionDrainApplyDisposition {
    Stale {
        completions: Vec<ExecutionCompletion>,
    },
    Applied {
        dispositions: Vec<KernelDisposition>,
        finished: Vec<CapabilityId>,
        retired: Vec<CapabilityId>,
        released: Vec<(RetainedPermitToken, CapabilityId)>,
    },
}

#[must_use = "an exchange apply disposition may return linear scheduler grants"]
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ComputeExchangeApplyDisposition {
    Stale {
        grants: RetainedPermitGrant,
    },
    Applied {
        stamp: Option<ApplyStamp>,
        dispositions: Vec<KernelDisposition>,
        attempted: Vec<CapabilityId>,
        settled: Vec<CapabilityId>,
        blocked: Vec<CapabilityId>,
        assignments: Vec<(RetainedPermitToken, WorkCapability)>,
        unused_grants: Vec<RetainedPermitToken>,
    },
    InvalidGrantBatch(RetainedPermitGrantError),
}

impl ComputeExchangePlan {
    pub(super) fn apply(self, current: &mut Omega) -> ComputeExchangeApplyDisposition {
        let Self {
            batch,
            attempted,
            settled,
            blocked,
            assigned,
            unused_grants,
        } = self;
        match batch.apply(current) {
            BatchApplyDisposition::Stale => {
                let grants = assigned
                    .into_iter()
                    .map(|(grant, _)| grant)
                    .chain(unused_grants)
                    .collect::<Vec<_>>();
                match RetainedPermitGrant::try_from_tokens(grants) {
                    Ok(grants) => ComputeExchangeApplyDisposition::Stale { grants },
                    Err(error) => ComputeExchangeApplyDisposition::InvalidGrantBatch(error),
                }
            }
            BatchApplyDisposition::Applied {
                stamp,
                dispositions,
            } => ComputeExchangeApplyDisposition::Applied {
                stamp,
                dispositions,
                attempted,
                settled,
                blocked,
                assignments: assigned,
                unused_grants,
            },
        }
    }
}

impl CompletionDrainPlan {
    pub(super) fn apply(self, current: &mut Omega) -> CompletionDrainApplyDisposition {
        let Self { batch, members } = self;
        match batch.apply(current) {
            BatchApplyDisposition::Stale => CompletionDrainApplyDisposition::Stale {
                completions: members
                    .into_iter()
                    .map(CompletionDrainMember::into_completion)
                    .collect(),
            },
            BatchApplyDisposition::Applied { dispositions, .. } => {
                let mut finished = Vec::new();
                let mut retired = Vec::new();
                let mut released = Vec::new();
                for member in members {
                    match member {
                        CompletionDrainMember::Finished(execution) => {
                            finished.push(execution.completion.capability);
                            released.push((execution.permit, execution.completion.capability));
                        }
                        CompletionDrainMember::Retired(execution) => {
                            retired.push(execution.completion.capability);
                            released.push((execution.permit, execution.completion.capability));
                        }
                    }
                }
                CompletionDrainApplyDisposition::Applied {
                    dispositions,
                    finished,
                    retired,
                    released,
                }
            }
        }
    }
}

impl CanonicalBatchPlan {
    pub(super) fn apply(self, current: &mut Omega) -> BatchApplyDisposition {
        if *current != self.expected {
            return BatchApplyDisposition::Stale;
        }
        *current = self.after;
        BatchApplyDisposition::Applied {
            stamp: self.committed_stamp,
            dispositions: self.dispositions,
        }
    }

    pub(super) fn planned_state(&self) -> &Omega {
        &self.after
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DynamicFootprint {
    pub(super) transaction: TxId,
    pub(super) proposal: ProposalId,
    pub(super) version: EntryVersion,
    pub(super) context: EvidenceContext,
    pub(super) source_priority: u8,
    pub(super) fee: u64,
    pub(super) arrival: Arrival,
    pub(super) consumes: BTreeSet<CellId>,
    pub(super) produces: BTreeSet<CellId>,
    pub(super) reads: BTreeSet<CellId>,
    pub(super) header_reads: BTreeSet<HeaderId>,
    pub(super) pool_inputs: BTreeMap<CellId, TxId>,
    pub(super) pool_reads: BTreeMap<CellId, TxId>,
    pub(super) charge: ResourceVector,
    pub(super) effect: LogicalEffect,
}

impl DynamicFootprint {
    fn from_ready(owner: &Owner, omega: &Omega) -> Result<Self, CouplingReason> {
        let OwnerLocation::Retained(RetainedOwner {
            source,
            phase: RetainedPhase::Ready(evidence),
        }) = &owner.location
        else {
            return Err(CouplingReason::NotReady(owner.transaction.id));
        };
        if !evidence.is_for(
            &owner.transaction,
            omega.authority.chain,
            omega.authority.rules,
        ) {
            return Err(CouplingReason::StaleEvidence(owner.transaction.id));
        }
        let pool_inputs = evidence
            .input_origins
            .iter()
            .filter_map(|(cell, origin)| match origin {
                InputOrigin::Chain => None,
                InputOrigin::Pool(parent) => Some((*cell, *parent)),
            })
            .collect::<BTreeMap<_, _>>();
        let pool_reads = evidence
            .dep_origins
            .iter()
            .filter_map(|(cell, origin)| match origin {
                InputOrigin::Chain => None,
                InputOrigin::Pool(parent) => Some((*cell, *parent)),
            })
            .collect::<BTreeMap<_, _>>();
        let charge = owner
            .transaction
            .charge()
            .ok_or(CouplingReason::Arithmetic)?;
        let effect = LogicalEffect::admitted(
            &owner.transaction,
            super::state::AcceptedStatus::Pending,
            source.ingress_peer(),
        );
        Ok(Self {
            transaction: owner.transaction.id,
            proposal: owner.transaction.proposal,
            version: owner.version,
            context: evidence.context,
            source_priority: source.priority(),
            fee: owner.transaction.fee,
            arrival: owner.arrival,
            consumes: owner.transaction.inputs.clone(),
            produces: owner.transaction.outputs.clone(),
            reads: owner.transaction.deps.clone(),
            header_reads: owner.transaction.header_deps.clone(),
            pool_inputs,
            pool_reads,
            charge,
            effect,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CompositionCost {
    pub(super) candidates: u32,
    pub(super) accepted_owners_scanned: u32,
    pub(super) accepted_edges_scanned: u32,
    pub(super) cell_keys: u32,
    pub(super) header_keys: u32,
    pub(super) pool_edges: u32,
    pub(super) index_operations: u32,
    pub(super) scratch_entries: u32,
}

impl CompositionCost {
    fn checked_add_count(target: u32, amount: usize) -> Result<u32, CouplingReason> {
        let amount = u32::try_from(amount).map_err(|_| CouplingReason::Arithmetic)?;
        target.checked_add(amount).ok_or(CouplingReason::Arithmetic)
    }

    fn add_candidate(&mut self, footprint: &DynamicFootprint) -> Result<(), CouplingReason> {
        self.candidates = self
            .candidates
            .checked_add(1)
            .ok_or(CouplingReason::Arithmetic)?;
        self.cell_keys = self
            .cell_keys
            .checked_add(
                u32::try_from(
                    footprint
                        .consumes
                        .len()
                        .checked_add(footprint.produces.len())
                        .and_then(|count| count.checked_add(footprint.reads.len()))
                        .ok_or(CouplingReason::Arithmetic)?,
                )
                .map_err(|_| CouplingReason::Arithmetic)?,
            )
            .ok_or(CouplingReason::Arithmetic)?;
        self.header_keys = Self::checked_add_count(self.header_keys, footprint.header_reads.len())?;
        self.pool_edges = Self::checked_add_count(
            self.pool_edges,
            footprint
                .pool_inputs
                .len()
                .checked_add(footprint.pool_reads.len())
                .ok_or(CouplingReason::Arithmetic)?,
        )?;
        Ok(())
    }

    pub(super) fn linear_key_bound(self) -> Option<u32> {
        self.accepted_edges_scanned
            .checked_add(self.cell_keys)?
            .checked_add(self.header_keys)?
            .checked_add(self.pool_edges)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ReadyComposition {
    pub(super) class: CohortClass,
    pub(super) prefix: Vec<DynamicFootprint>,
    pub(super) stopped_by: Option<CouplingReason>,
    pub(super) cost: CompositionCost,
}

#[derive(Default)]
struct AcceptedIndex {
    consumers: BTreeMap<CellId, TxId>,
    producers: BTreeMap<CellId, TxId>,
    readers: BTreeMap<CellId, BTreeSet<TxId>>,
}

impl AcceptedIndex {
    fn build(omega: &Omega, cost: &mut CompositionCost) -> Result<Self, CouplingReason> {
        let mut index = Self::default();
        for (id, owner) in &omega.authority.owners {
            let OwnerLocation::Accepted { .. } = owner.location else {
                continue;
            };
            cost.accepted_owners_scanned = cost
                .accepted_owners_scanned
                .checked_add(1)
                .ok_or(CouplingReason::Arithmetic)?;
            let edges = owner
                .transaction
                .inputs
                .len()
                .checked_add(owner.transaction.outputs.len())
                .and_then(|count| count.checked_add(owner.transaction.deps.len()))
                .ok_or(CouplingReason::Arithmetic)?;
            cost.accepted_edges_scanned =
                CompositionCost::checked_add_count(cost.accepted_edges_scanned, edges)?;
            for cell in &owner.transaction.inputs {
                index.consumers.insert(*cell, *id);
            }
            for cell in &owner.transaction.outputs {
                index.producers.insert(*cell, *id);
            }
            for cell in &owner.transaction.deps {
                index.readers.entry(*cell).or_default().insert(*id);
            }
        }
        Ok(index)
    }

    fn relation(&self, candidate: &DynamicFootprint) -> Option<CouplingReason> {
        for cell in &candidate.consumes {
            if let Some(accepted) = self.consumers.get(cell) {
                return Some(CouplingReason::AcceptedRelation {
                    candidate: candidate.transaction,
                    accepted: *accepted,
                    cell: *cell,
                    kind: RelationKind::SharedInput,
                });
            }
            if let Some(accepted) = self.producers.get(cell) {
                return Some(CouplingReason::AcceptedRelation {
                    candidate: candidate.transaction,
                    accepted: *accepted,
                    cell: *cell,
                    kind: RelationKind::CandidateProducesInput,
                });
            }
            if let Some(accepted) = self.readers.get(cell).and_then(|readers| readers.first()) {
                return Some(CouplingReason::AcceptedRelation {
                    candidate: candidate.transaction,
                    accepted: *accepted,
                    cell: *cell,
                    kind: RelationKind::CandidateSpendsRead,
                });
            }
        }
        for cell in &candidate.reads {
            if let Some(accepted) = self.consumers.get(cell) {
                return Some(CouplingReason::AcceptedRelation {
                    candidate: candidate.transaction,
                    accepted: *accepted,
                    cell: *cell,
                    kind: RelationKind::CandidateSpendsRead,
                });
            }
            if let Some(accepted) = self.producers.get(cell) {
                return Some(CouplingReason::AcceptedRelation {
                    candidate: candidate.transaction,
                    accepted: *accepted,
                    cell: *cell,
                    kind: RelationKind::CandidateProducesRead,
                });
            }
        }
        for cell in &candidate.produces {
            if let Some(accepted) = self.consumers.get(cell) {
                return Some(CouplingReason::AcceptedRelation {
                    candidate: candidate.transaction,
                    accepted: *accepted,
                    cell: *cell,
                    kind: RelationKind::CandidateProducesInput,
                });
            }
            if let Some(accepted) = self.producers.get(cell) {
                return Some(CouplingReason::AcceptedRelation {
                    candidate: candidate.transaction,
                    accepted: *accepted,
                    cell: *cell,
                    kind: RelationKind::DuplicateOutput,
                });
            }
            if let Some(accepted) = self.readers.get(cell).and_then(|readers| readers.first()) {
                return Some(CouplingReason::AcceptedRelation {
                    candidate: candidate.transaction,
                    accepted: *accepted,
                    cell: *cell,
                    kind: RelationKind::CandidateProducesRead,
                });
            }
        }
        None
    }
}

#[derive(Default)]
struct CandidateIndex {
    consumers: BTreeMap<CellId, TxId>,
    producers: BTreeMap<CellId, TxId>,
    readers: BTreeMap<CellId, BTreeSet<TxId>>,
    proposals: BTreeMap<ProposalId, TxId>,
}

impl CandidateIndex {
    fn first_reader(&self, cell: CellId) -> Option<TxId> {
        self.readers
            .get(&cell)
            .and_then(|readers| readers.first())
            .copied()
    }

    fn relation(&self, candidate: &DynamicFootprint) -> Option<CouplingReason> {
        if let Some(first) = self.proposals.get(&candidate.proposal) {
            return Some(CouplingReason::CandidateRelation {
                first: *first,
                second: candidate.transaction,
                cell: None,
                kind: RelationKind::ProposalCollision,
            });
        }
        for cell in &candidate.consumes {
            if let Some(first) = self.consumers.get(cell) {
                return Some(CouplingReason::CandidateRelation {
                    first: *first,
                    second: candidate.transaction,
                    cell: Some(*cell),
                    kind: RelationKind::SharedInput,
                });
            }
            if let Some(first) = self.producers.get(cell) {
                return Some(CouplingReason::CandidateRelation {
                    first: *first,
                    second: candidate.transaction,
                    cell: Some(*cell),
                    kind: RelationKind::CandidateProducesInput,
                });
            }
            if let Some(first) = self.first_reader(*cell) {
                return Some(CouplingReason::CandidateRelation {
                    first,
                    second: candidate.transaction,
                    cell: Some(*cell),
                    kind: RelationKind::CandidateSpendsRead,
                });
            }
        }
        for cell in &candidate.produces {
            if let Some(first) = self.consumers.get(cell) {
                return Some(CouplingReason::CandidateRelation {
                    first: *first,
                    second: candidate.transaction,
                    cell: Some(*cell),
                    kind: RelationKind::CandidateProducesInput,
                });
            }
            if let Some(first) = self.producers.get(cell) {
                return Some(CouplingReason::CandidateRelation {
                    first: *first,
                    second: candidate.transaction,
                    cell: Some(*cell),
                    kind: RelationKind::DuplicateOutput,
                });
            }
            if let Some(first) = self.first_reader(*cell) {
                return Some(CouplingReason::CandidateRelation {
                    first,
                    second: candidate.transaction,
                    cell: Some(*cell),
                    kind: RelationKind::CandidateProducesRead,
                });
            }
        }
        for cell in &candidate.reads {
            if let Some(first) = self.consumers.get(cell) {
                return Some(CouplingReason::CandidateRelation {
                    first: *first,
                    second: candidate.transaction,
                    cell: Some(*cell),
                    kind: RelationKind::CandidateSpendsRead,
                });
            }
            if let Some(first) = self.producers.get(cell) {
                return Some(CouplingReason::CandidateRelation {
                    first: *first,
                    second: candidate.transaction,
                    cell: Some(*cell),
                    kind: RelationKind::CandidateProducesRead,
                });
            }
        }
        None
    }

    fn insert(
        &mut self,
        candidate: &DynamicFootprint,
        cost: &mut CompositionCost,
    ) -> Result<(), CouplingReason> {
        self.proposals
            .insert(candidate.proposal, candidate.transaction);
        for cell in &candidate.consumes {
            self.consumers.insert(*cell, candidate.transaction);
        }
        for cell in &candidate.produces {
            self.producers.insert(*cell, candidate.transaction);
        }
        for cell in &candidate.reads {
            self.readers
                .entry(*cell)
                .or_default()
                .insert(candidate.transaction);
        }
        let operations = candidate
            .consumes
            .len()
            .checked_add(candidate.produces.len())
            .and_then(|count| count.checked_add(candidate.reads.len()))
            .and_then(|count| count.checked_add(1))
            .ok_or(CouplingReason::Arithmetic)?;
        cost.index_operations =
            CompositionCost::checked_add_count(cost.index_operations, operations)?;
        cost.scratch_entries = u32::try_from(
            self.consumers
                .len()
                .checked_add(self.producers.len())
                .and_then(|count| count.checked_add(self.readers.len()))
                .and_then(|count| count.checked_add(self.proposals.len()))
                .ok_or(CouplingReason::Arithmetic)?,
        )
        .map_err(|_| CouplingReason::Arithmetic)?;
        Ok(())
    }
}

pub(super) fn analyze_ready_prefix(omega: &Omega, limit: usize) -> ReadyComposition {
    if let Err(error) = omega.check_invariants() {
        return ReadyComposition {
            class: CohortClass::Coupled(CouplingReason::InvalidInitial(error.clone())),
            prefix: Vec::new(),
            stopped_by: Some(CouplingReason::InvalidInitial(error)),
            cost: CompositionCost::default(),
        };
    }
    let mut cost = CompositionCost::default();
    let accepted = match AcceptedIndex::build(omega, &mut cost) {
        Ok(index) => index,
        Err(reason) => {
            return ReadyComposition {
                class: CohortClass::Coupled(reason.clone()),
                prefix: Vec::new(),
                stopped_by: Some(reason),
                cost,
            };
        }
    };
    let accepted_usage = match omega.accepted_usage() {
        Ok(usage) => usage,
        Err(error) => {
            let reason = CouplingReason::InvalidInitial(error);
            return ReadyComposition {
                class: CohortClass::Coupled(reason.clone()),
                prefix: Vec::new(),
                stopped_by: Some(reason),
                cost,
            };
        }
    };
    let Some((effect_records, used_effect_bytes)) = omega.effect_usage() else {
        return ReadyComposition {
            class: CohortClass::Coupled(CouplingReason::Arithmetic),
            prefix: Vec::new(),
            stopped_by: Some(CouplingReason::Arithmetic),
            cost,
        };
    };

    let mut candidates = CandidateIndex::default();
    let mut prefix = Vec::new();
    let mut prefix_charge = ResourceVector::ZERO;
    let mut prefix_effect_records = 0u16;
    let mut prefix_effect_bytes = 0u32;
    let mut stopped_by = None;
    for id in omega.ready_order().into_iter().take(limit) {
        let Some(owner) = omega.authority.owners.get(&id) else {
            stopped_by = Some(CouplingReason::NotReady(id));
            break;
        };
        let footprint = match DynamicFootprint::from_ready(owner, omega) {
            Ok(footprint) => footprint,
            Err(reason) => {
                stopped_by = Some(reason);
                break;
            }
        };
        if let Some(parent) = footprint
            .pool_inputs
            .values()
            .chain(footprint.pool_reads.values())
            .next()
        {
            stopped_by = Some(CouplingReason::PoolOrigin {
                transaction: footprint.transaction,
                parent: *parent,
            });
            break;
        }
        if let Some(reason) = accepted.relation(&footprint) {
            stopped_by = Some(reason);
            break;
        }
        if let Some(reason) = candidates.relation(&footprint) {
            stopped_by = Some(reason);
            break;
        }
        let Some(next_charge) = prefix_charge.checked_add(footprint.charge) else {
            stopped_by = Some(CouplingReason::Arithmetic);
            break;
        };
        if accepted_usage
            .checked_add(next_charge)
            .is_none_or(|usage| !usage.fits(omega.authority.limits.accepted))
        {
            stopped_by = Some(CouplingReason::AcceptedCapacity(footprint.transaction));
            break;
        }
        let Some(next_records) = prefix_effect_records.checked_add(1) else {
            stopped_by = Some(CouplingReason::Arithmetic);
            break;
        };
        let Some(candidate_effect_bytes) = footprint.effect.charge_bytes() else {
            stopped_by = Some(CouplingReason::Arithmetic);
            break;
        };
        let Some(next_bytes) = prefix_effect_bytes.checked_add(candidate_effect_bytes) else {
            stopped_by = Some(CouplingReason::Arithmetic);
            break;
        };
        if effect_records
            .checked_add(next_records)
            .is_none_or(|records| records > omega.authority.limits.effect_records)
            || used_effect_bytes
                .checked_add(next_bytes)
                .is_none_or(|bytes| bytes > omega.authority.limits.effect_bytes)
        {
            stopped_by = Some(CouplingReason::EffectCapacity(footprint.transaction));
            break;
        }
        if let Err(reason) = cost.add_candidate(&footprint) {
            stopped_by = Some(reason);
            break;
        }
        if let Err(reason) = candidates.insert(&footprint, &mut cost) {
            stopped_by = Some(reason);
            break;
        }
        prefix_charge = next_charge;
        prefix_effect_records = next_records;
        prefix_effect_bytes = next_bytes;
        prefix.push(footprint);
    }

    let class = if prefix.is_empty() {
        stopped_by
            .clone()
            .map_or(CohortClass::Empty, CohortClass::Coupled)
    } else {
        CohortClass::IndependentComposable
    };
    ReadyComposition {
        class,
        prefix,
        stopped_by,
        cost,
    }
}

fn command_is_supported(family: OrderedBatchFamily, command: &KernelCommand) -> bool {
    match family {
        OrderedBatchFamily::RetainedIngress => matches!(command, KernelCommand::Admit(_)),
        OrderedBatchFamily::CompletionDrain => {
            matches!(command, KernelCommand::FinishExecution(_))
        }
        OrderedBatchFamily::ComputeExchange => {
            matches!(
                command,
                KernelCommand::SettleFinished(_) | KernelCommand::Checkout
            )
        }
    }
}

fn execution_completion_key(
    omega: &Omega,
    execution: &ExecutionCompletion,
) -> (u8, Arrival, TxId, CapabilityId) {
    let capability_id = execution.completion.capability;
    let Some(capability) = omega.linear.work.get(&capability_id) else {
        return (u8::MAX, Arrival(u16::MAX), TxId(u8::MAX), capability_id);
    };
    let Some(owner) = omega.authority.owners.get(&capability.transaction) else {
        return (
            u8::MAX,
            Arrival(u16::MAX),
            capability.transaction,
            capability_id,
        );
    };
    let source_priority = owner.retained_source().map_or(u8::MAX, Source::priority);
    (
        source_priority,
        owner.arrival,
        capability.transaction,
        capability_id,
    )
}

pub(super) fn plan_completion_drain(
    omega: &Omega,
    scheduler: &FairPermitScheduler,
    mut completions: Vec<ExecutionCompletion>,
) -> Result<CompletionDrainPlan, CompletionDrainPlanFailure> {
    if completions.is_empty() {
        return Err(CompletionDrainPlanFailure {
            error: BatchPlanError::Empty,
            completions,
        });
    }
    if completions.len() > usize::from(omega.authority.limits.compute_permits) {
        return Err(CompletionDrainPlanFailure {
            error: BatchPlanError::CompletionBatchBound,
            completions,
        });
    }
    let mut capabilities = BTreeSet::new();
    for execution in &completions {
        if !capabilities.insert(execution.completion.capability) {
            return Err(CompletionDrainPlanFailure {
                error: BatchPlanError::DuplicateCapability(execution.completion.capability),
                completions,
            });
        }
        let request = execution.permit.request().id;
        if !scheduler.owns_retained(&execution.permit) {
            return Err(CompletionDrainPlanFailure {
                error: BatchPlanError::InvalidPermitToken(request),
                completions,
            });
        }
    }
    completions.sort_unstable_by_key(|completion| execution_completion_key(omega, completion));
    let commands = completions
        .iter()
        .map(|execution| KernelCommand::FinishExecution(execution.completion.clone()))
        .collect::<Vec<_>>();
    let batch = match plan_ordered_batch(omega, OrderedBatchFamily::CompletionDrain, commands) {
        Ok(batch) => batch,
        Err(error) => {
            return Err(CompletionDrainPlanFailure { error, completions });
        }
    };
    if batch.sequential_apply_count != 0 || batch.committed_stamp.is_some() {
        return Err(CompletionDrainPlanFailure {
            error: BatchPlanError::UnexpectedDisposition,
            completions,
        });
    }
    let dispositions = batch.dispositions.clone();
    if completions
        .iter()
        .zip(&dispositions)
        .any(|(execution, disposition)| {
            let capability = execution.completion.capability;
            !matches!(
                disposition,
                KernelDisposition::Finished(observed)
                    | KernelDisposition::StaleCapabilityRetired(observed)
                    if *observed == capability
            )
        })
    {
        return Err(CompletionDrainPlanFailure {
            error: BatchPlanError::UnexpectedDisposition,
            completions,
        });
    }
    let mut members = Vec::with_capacity(completions.len());
    for (execution, disposition) in completions.into_iter().zip(dispositions) {
        let capability = execution.completion.capability;
        let member = match disposition {
            KernelDisposition::Finished(observed) if observed == capability => {
                CompletionDrainMember::Finished(execution)
            }
            KernelDisposition::StaleCapabilityRetired(observed) if observed == capability => {
                CompletionDrainMember::Retired(execution)
            }
            _ => continue,
        };
        members.push(member);
    }
    Ok(CompletionDrainPlan { batch, members })
}

fn compact_live_stamps(
    omega: &mut Omega,
) -> Result<BTreeMap<ApplyStamp, ApplyStamp>, BatchPlanError> {
    let mut live = omega
        .authority
        .effects
        .iter()
        .map(|effect| effect.stamp)
        .chain(omega.authority.peer_bans.values().map(|ban| ban.order))
        .chain(omega.linear.effect_claim.map(|claim| claim.stamp))
        .collect::<BTreeSet<_>>();
    live.remove(&ApplyStamp(0));
    let mut compact_to_original = BTreeMap::new();
    let mut original_to_compact = BTreeMap::new();
    for (index, original) in live.into_iter().enumerate() {
        let compact = ApplyStamp(
            u16::try_from(index)
                .ok()
                .and_then(|index| index.checked_add(1))
                .ok_or(BatchPlanError::StampNormalization)?,
        );
        compact_to_original.insert(compact, original);
        original_to_compact.insert(original, compact);
    }
    for effect in &mut omega.authority.effects {
        effect.stamp = *original_to_compact
            .get(&effect.stamp)
            .ok_or(BatchPlanError::StampNormalization)?;
    }
    for ban in omega.authority.peer_bans.values_mut() {
        ban.order = *original_to_compact
            .get(&ban.order)
            .ok_or(BatchPlanError::StampNormalization)?;
    }
    if let Some(claim) = &mut omega.linear.effect_claim {
        claim.stamp = *original_to_compact
            .get(&claim.stamp)
            .ok_or(BatchPlanError::StampNormalization)?;
    }
    omega.authority.last_apply = ApplyStamp(
        u16::try_from(compact_to_original.len()).map_err(|_| BatchPlanError::StampNormalization)?,
    );
    Ok(compact_to_original)
}

fn restore_collapsed_stamps(
    original: &Omega,
    mut planned: Omega,
    compact_to_original: &BTreeMap<ApplyStamp, ApplyStamp>,
    sequential_apply_count: u16,
) -> Result<(Omega, Option<ApplyStamp>), BatchPlanError> {
    let compact_cut = ApplyStamp(
        u16::try_from(compact_to_original.len()).map_err(|_| BatchPlanError::StampNormalization)?,
    );
    if sequential_apply_count == 0 {
        for effect in &mut planned.authority.effects {
            effect.stamp = *compact_to_original
                .get(&effect.stamp)
                .ok_or(BatchPlanError::StampNormalization)?;
        }
        for ban in planned.authority.peer_bans.values_mut() {
            ban.order = *compact_to_original
                .get(&ban.order)
                .ok_or(BatchPlanError::StampNormalization)?;
        }
        if let Some(claim) = &mut planned.linear.effect_claim {
            claim.stamp = *compact_to_original
                .get(&claim.stamp)
                .ok_or(BatchPlanError::StampNormalization)?;
        }
        planned.authority.last_apply = original.authority.last_apply;
        return Ok((planned, None));
    }

    let stamp = ApplyStamp(
        original
            .authority
            .last_apply
            .0
            .checked_add(1)
            .ok_or(BatchPlanError::CounterExhausted)?,
    );
    let mut next_ordinal = 0u16;
    for effect in &mut planned.authority.effects {
        if effect.stamp <= compact_cut {
            effect.stamp = *compact_to_original
                .get(&effect.stamp)
                .ok_or(BatchPlanError::StampNormalization)?;
        } else {
            effect.stamp = stamp;
            effect.ordinal = next_ordinal;
            next_ordinal = next_ordinal
                .checked_add(1)
                .ok_or(BatchPlanError::CounterExhausted)?;
        }
    }
    for ban in planned.authority.peer_bans.values_mut() {
        if ban.order <= compact_cut {
            ban.order = *compact_to_original
                .get(&ban.order)
                .ok_or(BatchPlanError::StampNormalization)?;
        } else {
            // A single Apply cannot encode two peer-fence order tokens with
            // one stamp. Current M2 batch families never produce a ban; keep
            // this as a mechanical fallback instead of inventing an ordinal.
            return Err(BatchPlanError::UnsupportedCommand);
        }
    }
    if let Some(claim) = &mut planned.linear.effect_claim {
        if claim.stamp <= compact_cut {
            claim.stamp = *compact_to_original
                .get(&claim.stamp)
                .ok_or(BatchPlanError::StampNormalization)?;
        } else {
            return Err(BatchPlanError::UnsupportedCommand);
        }
    }
    planned.authority.last_apply = stamp;
    planned
        .check_invariants()
        .map_err(BatchPlanError::InvalidResult)?;
    Ok((planned, Some(stamp)))
}

fn fold_canonical_commands(
    omega: &Omega,
    commands: &[KernelCommand],
) -> Result<(Omega, Vec<KernelDisposition>, u16, Option<ApplyStamp>), BatchPlanError> {
    if commands.is_empty() {
        return Err(BatchPlanError::Empty);
    }
    let mut planned = omega.clone();
    let compact_to_original = compact_live_stamps(&mut planned)?;
    let mut dispositions = Vec::with_capacity(commands.len());
    let mut sequential_apply_count = 0u16;
    for command in commands.iter().cloned() {
        let step = planned.kernel_step(command);
        if matches!(step.disposition(), KernelDisposition::CounterExhausted) {
            return Err(BatchPlanError::CounterExhausted);
        }
        if matches!(step, KernelStep::AuthorityCommit { .. }) {
            sequential_apply_count = sequential_apply_count
                .checked_add(1)
                .ok_or(BatchPlanError::CounterExhausted)?;
        }
        dispositions.push(step.disposition().clone());
    }
    let (planned, stamp) =
        restore_collapsed_stamps(omega, planned, &compact_to_original, sequential_apply_count)?;
    planned
        .check_invariants()
        .map_err(BatchPlanError::InvalidResult)?;
    Ok((planned, dispositions, sequential_apply_count, stamp))
}

pub(super) fn plan_ordered_batch(
    omega: &Omega,
    family: OrderedBatchFamily,
    commands: Vec<KernelCommand>,
) -> Result<CanonicalBatchPlan, BatchPlanError> {
    if commands
        .iter()
        .any(|command| !command_is_supported(family, command))
    {
        return Err(BatchPlanError::UnsupportedCommand);
    }
    let (after, dispositions, sequential_apply_count, committed_stamp) =
        fold_canonical_commands(omega, &commands)?;
    Ok(CanonicalBatchPlan {
        expected: omega.clone(),
        after,
        class: CohortClass::CanonicalOrdered,
        dispositions,
        sequential_apply_count,
        committed_stamp,
    })
}

pub(super) fn plan_ready_batch(
    omega: &Omega,
    limit: usize,
    wall_time: u64,
) -> Result<CanonicalBatchPlan, BatchPlanError> {
    let analysis = analyze_ready_prefix(omega, limit);
    if analysis.prefix.is_empty() {
        return Err(BatchPlanError::Coupled(
            analysis.stopped_by.unwrap_or(CouplingReason::Arithmetic),
        ));
    }
    let mut capture_source = omega.clone();
    let capture = match capture_source.kernel_step(KernelCommand::CaptureReady { limit }) {
        KernelStep::NoAuthorityCommit(KernelDisposition::ReadyCaptured(mut capture)) => {
            capture.keys.truncate(analysis.prefix.len());
            capture
        }
        _ => return Err(BatchPlanError::UnexpectedDisposition),
    };
    let sequential_commands = (0..analysis.prefix.len())
        .map(|_| KernelCommand::FinalizeNext { wall_time })
        .collect::<Vec<_>>();
    let (sequential_after, dispositions, sequential_apply_count, committed_stamp) =
        fold_canonical_commands(omega, &sequential_commands)?;

    let mut batch_after = omega.clone();
    let batch_step = batch_after.kernel_step(KernelCommand::FinalizeCaptured {
        capture: ReadyCapture {
            keys: capture.keys.clone(),
        },
        wall_time,
    });
    let KernelStep::AuthorityCommit { stamp, .. } = batch_step else {
        return Err(BatchPlanError::UnexpectedDisposition);
    };
    if Some(stamp) != committed_stamp || batch_after != sequential_after {
        return Err(BatchPlanError::UnexpectedDisposition);
    }
    Ok(CanonicalBatchPlan {
        expected: omega.clone(),
        after: batch_after,
        class: CohortClass::IndependentComposable,
        dispositions,
        sequential_apply_count,
        committed_stamp,
    })
}

fn finished_completion_key(
    omega: &Omega,
    finished: &FinishedWorkCapability,
) -> (u8, Arrival, TxId, CapabilityId) {
    let capability = finished.capability;
    let Some(owner) = omega.authority.owners.get(&capability.transaction) else {
        return (
            u8::MAX,
            Arrival(u16::MAX),
            capability.transaction,
            capability.id,
        );
    };
    let OwnerLocation::Retained(RetainedOwner {
        source,
        phase: RetainedPhase::Computing(_),
    }) = &owner.location
    else {
        return (
            u8::MAX,
            owner.arrival,
            capability.transaction,
            capability.id,
        );
    };
    if owner.version != capability.version
        || capability.chain != omega.authority.chain
        || capability.rules != omega.authority.rules
    {
        return (
            u8::MAX,
            owner.arrival,
            capability.transaction,
            capability.id,
        );
    }
    (
        source.priority(),
        owner.arrival,
        capability.transaction,
        capability.id,
    )
}

pub(super) fn plan_compute_exchange(
    omega: &Omega,
    finished: Vec<CapabilityId>,
    grants: RetainedPermitGrant,
) -> Result<ComputeExchangePlan, ComputeExchangePlanFailure> {
    if let Err(error) = omega.check_invariants() {
        return Err(ComputeExchangePlanFailure {
            error: BatchPlanError::InvalidResult(error),
            finished,
            grants,
        });
    }
    let mut seen = BTreeSet::new();
    for capability in &finished {
        if !seen.insert(*capability) {
            return Err(ComputeExchangePlanFailure {
                error: BatchPlanError::DuplicateCapability(*capability),
                finished,
                grants,
            });
        }
    }
    let mut eligible = Vec::new();
    for capability in finished.iter().copied() {
        let Some(finished) = omega.linear.finished_work.get(&capability) else {
            return Err(ComputeExchangePlanFailure {
                error: BatchPlanError::MissingFinishedCapability(capability),
                finished,
                grants,
            });
        };
        eligible.push((finished_completion_key(omega, finished), capability));
    }
    eligible.sort_unstable_by_key(|(key, _)| *key);
    let attempted = eligible
        .iter()
        .map(|(_, capability)| *capability)
        .collect::<Vec<_>>();
    let mut commands = eligible
        .into_iter()
        .map(|(_, capability)| KernelCommand::SettleFinished(capability))
        .collect::<Vec<_>>();
    let grant_count = grants.tokens.len();
    commands.extend((0..grant_count).map(|_| KernelCommand::Checkout));
    if commands.is_empty() {
        return Err(ComputeExchangePlanFailure {
            error: BatchPlanError::Empty,
            finished,
            grants,
        });
    }
    let batch = match plan_ordered_batch(omega, OrderedBatchFamily::ComputeExchange, commands) {
        Ok(batch) => batch,
        Err(error) => {
            return Err(ComputeExchangePlanFailure {
                error,
                finished,
                grants,
            });
        }
    };
    let checkout_dispositions = batch
        .dispositions
        .iter()
        .skip(attempted.len())
        .cloned()
        .collect::<Vec<_>>();
    if checkout_dispositions.len() != grant_count
        || checkout_dispositions.iter().any(|disposition| {
            !matches!(
                disposition,
                KernelDisposition::CheckedOut(_) | KernelDisposition::Idle
            )
        })
        || attempted
            .iter()
            .zip(batch.dispositions.iter())
            .any(|(_, disposition)| {
                !matches!(
                    disposition,
                    KernelDisposition::EffectCapacityWait(_)
                        | KernelDisposition::Continued(_)
                        | KernelDisposition::Ready(_)
                        | KernelDisposition::Waiting(_)
                        | KernelDisposition::Rejected(_)
                        | KernelDisposition::InvalidEvidenceRejected(_)
                        | KernelDisposition::StaleCapabilityRetired(_)
                )
            })
    {
        return Err(ComputeExchangePlanFailure {
            error: BatchPlanError::UnexpectedDisposition,
            finished,
            grants,
        });
    }
    let grant_tokens = grants.tokens;
    let mut assigned = Vec::new();
    let mut unused_grants = Vec::new();
    for (grant, disposition) in grant_tokens.into_iter().zip(checkout_dispositions) {
        match disposition {
            KernelDisposition::CheckedOut(capability) => assigned.push((grant, capability)),
            KernelDisposition::Idle => unused_grants.push(grant),
            _ => continue,
        }
    }
    let mut settled = Vec::new();
    let mut blocked = Vec::new();
    for (capability, disposition) in attempted.iter().copied().zip(batch.dispositions.iter()) {
        match disposition {
            KernelDisposition::EffectCapacityWait(_) => blocked.push(capability),
            KernelDisposition::Continued(_)
            | KernelDisposition::Ready(_)
            | KernelDisposition::Waiting(_)
            | KernelDisposition::Rejected(_)
            | KernelDisposition::InvalidEvidenceRejected(_)
            | KernelDisposition::StaleCapabilityRetired(_) => settled.push(capability),
            _ => continue,
        }
    }
    Ok(ComputeExchangePlan {
        batch,
        attempted,
        settled,
        blocked,
        assigned,
        unused_grants,
    })
}

pub(super) fn transaction_footprint(
    transaction: &Transaction,
    evidence: &ResolvedEvidence,
    version: EntryVersion,
    source: Source,
) -> Result<DynamicFootprint, CouplingReason> {
    let owner = Owner {
        version,
        arrival: super::state::Arrival(1),
        transaction: transaction.clone(),
        location: OwnerLocation::Retained(RetainedOwner {
            source,
            phase: RetainedPhase::Ready(evidence.clone()),
        }),
    };
    let limits = super::state::ModelLimits::small()
        .validate()
        .map_err(|_| CouplingReason::Arithmetic)?;
    let mut omega = Omega::new(limits, evidence.context.chain.tip, evidence.context.rules);
    omega.authority.chain = evidence.context.chain;
    DynamicFootprint::from_ready(&owner, &omega)
}
