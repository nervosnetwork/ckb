//! Neutral stable-cut trace vocabulary and the independent reference replay.
//!
//! Production refinement tests may share the symbolic inputs and normalized
//! observations exported here. They must not call this adapter's transition or
//! normalization implementation when constructing production observations.

use super::{
    kernel::{
        Admission, Completion, KernelCommand, KernelDisposition, KernelStep, ResolveContinuation,
        WorkResult,
    },
    state::{
        AcceptanceEffect, AcceptedProvenance, AcceptedStatus, Arrival, CapabilityId,
        EffectClaimSource, EffectClass, EntryVersion, LogicalEffect, ModelLimits, Omega,
        OwnerLocation, PeerId, ProposalBase, ProposalId, RemoteDeadline, RemoteResidency,
        ResolvedEvidence, RetainedOwner, RetainedPhase, RetainedSource, RulesId, Source,
        Transaction, TxId, VerifyCapability, VerifyCycleClass, ViewId, WitnessId, WorkPermit,
        WorkStage,
    },
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct TraceTxId(pub(crate) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct TracePeerId(pub(crate) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceVerifyClass {
    Small,
    Large,
}

impl TraceVerifyClass {
    pub(crate) const ALL: [Self; 2] = [Self::Small, Self::Large];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceVerifyCapability {
    Any,
    SmallCycleOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceWorkPermit {
    ResolveOnly,
    VerifyOnly(TraceVerifyCapability),
    ResolveThenVerify(TraceVerifyCapability),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceLifecycleRoute {
    Split,
    Continuous(TraceVerifyCapability),
}

impl TraceLifecycleRoute {
    pub(crate) const ALL: [Self; 3] = [
        Self::Split,
        Self::Continuous(TraceVerifyCapability::Any),
        Self::Continuous(TraceVerifyCapability::SmallCycleOnly),
    ];

    const fn initial_permit(self) -> TraceWorkPermit {
        match self {
            Self::Split => TraceWorkPermit::ResolveOnly,
            Self::Continuous(capability) => TraceWorkPermit::ResolveThenVerify(capability),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceAction {
    AdmitRemote {
        transaction: TraceTxId,
        peer: TracePeerId,
        deadline: u64,
    },
    Checkout(TraceWorkPermit),
    Resolve(TraceTxId),
    Verify(TraceTxId),
    FinalizeReady,
    ClaimEffect,
    SettleEffect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TraceTransaction {
    pub(crate) id: TraceTxId,
    pub(crate) verify_class: TraceVerifyClass,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TraceScenario {
    pub(crate) transaction: TraceTransaction,
    pub(crate) peer: TracePeerId,
    pub(crate) route: TraceLifecycleRoute,
}

impl TraceScenario {
    pub(crate) fn lifecycle(
        transaction: TraceTxId,
        verify_class: TraceVerifyClass,
        route: TraceLifecycleRoute,
    ) -> Self {
        Self {
            transaction: TraceTransaction {
                id: transaction,
                verify_class,
            },
            peer: TracePeerId(1),
            route,
        }
    }

    /// The action shape is intentionally independent of route/class outcome.
    /// A compatible continuous route observes an idle Verify checkout, while a
    /// split or incompatible route observes a real checkout at the same cut.
    pub(crate) fn actions(&self) -> Vec<TraceAction> {
        vec![
            TraceAction::AdmitRemote {
                transaction: self.transaction.id,
                peer: self.peer,
                deadline: 100,
            },
            TraceAction::Checkout(self.route.initial_permit()),
            TraceAction::Resolve(self.transaction.id),
            TraceAction::Checkout(TraceWorkPermit::VerifyOnly(TraceVerifyCapability::Any)),
            TraceAction::Verify(self.transaction.id),
            TraceAction::FinalizeReady,
            TraceAction::ClaimEffect,
            TraceAction::SettleEffect,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceWorkStage {
    Resolve,
    Verify(TraceVerifyClass),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceAcceptedStatus {
    Pending,
    Gap,
    Proposed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceRetainedSource {
    Remote(TracePeerId),
    Proposal { ingress_peer: Option<TracePeerId> },
    Recovery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceAcceptedProvenance {
    Trusted,
    Peer(TracePeerId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceRetainedPhase {
    QueuedResolve,
    QueuedVerify(TraceVerifyClass),
    Computing(TraceWorkPermit),
    Waiting,
    Ready,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceOwnerLocation {
    Retained {
        source: TraceRetainedSource,
        phase: TraceRetainedPhase,
    },
    Accepted {
        provenance: TraceAcceptedProvenance,
        status: TraceAcceptedStatus,
    },
    ReplacementHistory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct TraceOwnerObservation {
    pub(crate) transaction: TraceTxId,
    pub(crate) version_rank: u16,
    pub(crate) arrival_rank: u16,
    pub(crate) location: TraceOwnerLocation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceWorkLocation {
    Executing,
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct TraceWorkObservation {
    pub(crate) capability_rank: u16,
    pub(crate) transaction: TraceTxId,
    pub(crate) permit: TraceWorkPermit,
    pub(crate) stage: TraceWorkStage,
    pub(crate) location: TraceWorkLocation,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct TraceResourceCounts {
    pub(crate) owners: u16,
    pub(crate) charged_owners: u16,
    pub(crate) retained: u16,
    pub(crate) remote: u16,
    pub(crate) accepted: u16,
    pub(crate) replacement_history: u16,
    pub(crate) active_work: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceEffectClass {
    Remote,
    Trusted,
    Critical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceEffect {
    Accepted {
        transaction: TraceTxId,
        status: TraceAcceptedStatus,
        ingress_peer: Option<TracePeerId>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct TraceEffectObservation {
    pub(crate) sequence: u64,
    pub(crate) ordinal: u16,
    pub(crate) class: TraceEffectClass,
    pub(crate) effect: TraceEffect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct TraceEffectClaim {
    pub(crate) sequence: u64,
    pub(crate) ordinal: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TraceObservation {
    pub(crate) last_apply: u64,
    pub(crate) generation: u64,
    pub(crate) chain_revision: u64,
    pub(crate) owners: Vec<TraceOwnerObservation>,
    pub(crate) work: Vec<TraceWorkObservation>,
    pub(crate) resources: TraceResourceCounts,
    pub(crate) effects: Vec<TraceEffectObservation>,
    pub(crate) effect_claim: Option<TraceEffectClaim>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TraceDisposition {
    Retained(TraceTxId),
    CheckedOut {
        transaction: TraceTxId,
        permit: TraceWorkPermit,
        stage: TraceWorkStage,
    },
    ResolveContinued(TraceTxId),
    QueuedVerify(TraceTxId),
    Ready(TraceTxId),
    Accepted(TraceTxId),
    EffectClaimed(TraceEffectClaim),
    EffectSettled(TraceEffectClaim),
    Idle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TraceCut {
    pub(crate) action: TraceAction,
    pub(crate) disposition: TraceDisposition,
    pub(crate) observation: TraceObservation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReferenceTraceError {
    InvalidConfiguration,
    UnknownTransaction(TraceTxId),
    MissingCapability(TraceTxId),
    DuplicateCapability(TraceTxId),
    UnsupportedCheckout(TraceWorkPermit),
    UnexpectedDisposition,
    UnsupportedEffect,
    CounterOverflow,
    Invariant,
}

pub(crate) fn replay_reference_trace(
    scenario: &TraceScenario,
) -> Result<Vec<TraceCut>, ReferenceTraceError> {
    let mut replay = ReferenceTrace::new(scenario)?;
    scenario
        .actions()
        .into_iter()
        .map(|action| {
            let disposition = replay.step(action)?;
            let observation = replay.observe()?;
            Ok(TraceCut {
                action,
                disposition,
                observation,
            })
        })
        .collect()
}

struct ReferenceTrace {
    omega: Omega,
    transactions: BTreeMap<TraceTxId, Transaction>,
    capability_ranks: BTreeMap<CapabilityId, u16>,
    version_ranks: BTreeMap<EntryVersion, u16>,
    arrival_ranks: BTreeMap<Arrival, u16>,
}

impl ReferenceTrace {
    fn new(scenario: &TraceScenario) -> Result<Self, ReferenceTraceError> {
        let limits = ModelLimits::small()
            .validate()
            .map_err(|_| ReferenceTraceError::InvalidConfiguration)?;
        let transaction = Transaction {
            id: TxId(scenario.transaction.id.0),
            witness: WitnessId(scenario.transaction.id.0),
            proposal: ProposalId(scenario.transaction.id.0),
            inputs: BTreeSet::new(),
            deps: BTreeSet::new(),
            header_deps: BTreeSet::new(),
            outputs: BTreeSet::new(),
            bytes: 4,
            cycles: 0,
            fee: 10,
            verify_class: model_verify_class(scenario.transaction.verify_class),
        };
        Ok(Self {
            omega: Omega::new(limits, ViewId(1), RulesId(1)),
            transactions: BTreeMap::from([(scenario.transaction.id, transaction)]),
            capability_ranks: BTreeMap::new(),
            version_ranks: BTreeMap::new(),
            arrival_ranks: BTreeMap::new(),
        })
    }

    fn step(&mut self, action: TraceAction) -> Result<TraceDisposition, ReferenceTraceError> {
        let disposition = match action {
            TraceAction::AdmitRemote {
                transaction,
                peer,
                deadline,
            } => {
                let transaction = self.transaction(transaction)?.clone();
                match self.omega.kernel_step(KernelCommand::Admit(Admission {
                    transaction,
                    source: RetainedSource::Remote(RemoteResidency::new(
                        PeerId(peer.0),
                        RemoteDeadline(deadline),
                    )),
                    observed_at: super::state::MonotonicTick(0),
                })) {
                    KernelStep::AuthorityCommit {
                        disposition: KernelDisposition::Retained(transaction),
                        ..
                    } => TraceDisposition::Retained(TraceTxId(transaction.0)),
                    _ => return Err(ReferenceTraceError::UnexpectedDisposition),
                }
            }
            TraceAction::Checkout(permit) => {
                let step =
                    match permit {
                        TraceWorkPermit::ResolveOnly
                        | TraceWorkPermit::VerifyOnly(TraceVerifyCapability::Any) => {
                            self.omega.kernel_step(KernelCommand::Checkout)
                        }
                        TraceWorkPermit::ResolveThenVerify(capability) => self.omega.kernel_step(
                            KernelCommand::CheckoutContinuous(model_capability(capability)),
                        ),
                        TraceWorkPermit::VerifyOnly(TraceVerifyCapability::SmallCycleOnly) => {
                            return Err(ReferenceTraceError::UnsupportedCheckout(permit));
                        }
                    };
                match step {
                    KernelStep::NoAuthorityCommit(KernelDisposition::Idle) => {
                        TraceDisposition::Idle
                    }
                    KernelStep::AuthorityCommit {
                        disposition: KernelDisposition::CheckedOut(capability),
                        ..
                    } => {
                        let actual_permit = trace_permit(capability.permit());
                        if actual_permit != permit {
                            return Err(ReferenceTraceError::UnexpectedDisposition);
                        }
                        self.register_capability(capability.id)?;
                        TraceDisposition::CheckedOut {
                            transaction: TraceTxId(capability.transaction.0),
                            permit: actual_permit,
                            stage: trace_stage(capability.stage()),
                        }
                    }
                    _ => return Err(ReferenceTraceError::UnexpectedDisposition),
                }
            }
            TraceAction::Resolve(transaction) => {
                let capability = self.executing_capability(transaction)?;
                let owner = self.transaction(transaction)?;
                let evidence = ResolvedEvidence::for_transaction(
                    owner,
                    self.omega.authority.chain,
                    self.omega.authority.rules,
                );
                let continuous = matches!(
                    capability.permit(),
                    WorkPermit::ResolveThenVerify(verify)
                        if verify.permits(evidence.verify_class)
                );
                let step = if continuous {
                    self.omega
                        .kernel_step(KernelCommand::ContinueResolveThenVerify(
                            ResolveContinuation {
                                capability: capability.id,
                                evidence,
                            },
                        ))
                } else {
                    self.omega.kernel_step(KernelCommand::Complete(Completion {
                        capability: capability.id,
                        result: WorkResult::Resolved(evidence),
                    }))
                };
                match step {
                    KernelStep::NoAuthorityCommit(KernelDisposition::ResolveContinued(_)) => {
                        TraceDisposition::ResolveContinued(transaction)
                    }
                    KernelStep::AuthorityCommit {
                        disposition: KernelDisposition::Continued(id),
                        ..
                    } if id == TxId(transaction.0) => TraceDisposition::QueuedVerify(transaction),
                    _ => return Err(ReferenceTraceError::UnexpectedDisposition),
                }
            }
            TraceAction::Verify(transaction) => {
                let capability = self.executing_capability(transaction)?;
                if !matches!(capability.stage(), WorkStage::Verify(_)) {
                    return Err(ReferenceTraceError::UnexpectedDisposition);
                }
                match self.omega.kernel_step(KernelCommand::Complete(Completion {
                    capability: capability.id,
                    result: WorkResult::Verified,
                })) {
                    KernelStep::AuthorityCommit {
                        disposition: KernelDisposition::Ready(id),
                        ..
                    } if id == TxId(transaction.0) => TraceDisposition::Ready(transaction),
                    _ => return Err(ReferenceTraceError::UnexpectedDisposition),
                }
            }
            TraceAction::FinalizeReady => {
                match self
                    .omega
                    .kernel_step(KernelCommand::FinalizeNext { wall_time: 10 })
                {
                    KernelStep::AuthorityCommit {
                        disposition: KernelDisposition::Accepted(transaction),
                        ..
                    } => TraceDisposition::Accepted(TraceTxId(transaction.0)),
                    _ => return Err(ReferenceTraceError::UnexpectedDisposition),
                }
            }
            TraceAction::ClaimEffect => match self.omega.kernel_step(KernelCommand::ClaimEffect) {
                KernelStep::NoAuthorityCommit(KernelDisposition::EffectClaimed(claim)) => {
                    TraceDisposition::EffectClaimed(trace_claim(claim)?)
                }
                _ => return Err(ReferenceTraceError::UnexpectedDisposition),
            },
            TraceAction::SettleEffect => {
                let claim = self
                    .omega
                    .linear
                    .effect_claim
                    .ok_or(ReferenceTraceError::UnexpectedDisposition)?;
                match self.omega.kernel_step(KernelCommand::SettleEffect(claim)) {
                    KernelStep::AuthorityCommit {
                        disposition: KernelDisposition::EffectSettled(settled),
                        ..
                    } if settled == claim => TraceDisposition::EffectSettled(trace_claim(claim)?),
                    _ => return Err(ReferenceTraceError::UnexpectedDisposition),
                }
            }
        };
        if self.omega.check_invariants() != Ok(()) {
            return Err(ReferenceTraceError::Invariant);
        }
        Ok(disposition)
    }

    fn observe(&mut self) -> Result<TraceObservation, ReferenceTraceError> {
        self.register_owner_ranks()?;
        let mut owners = self
            .omega
            .authority
            .owners
            .iter()
            .map(|(id, owner)| {
                let location = match &owner.location {
                    OwnerLocation::Retained(RetainedOwner { source, phase }) => {
                        TraceOwnerLocation::Retained {
                            source: trace_source(*source),
                            phase: trace_retained_phase(phase),
                        }
                    }
                    OwnerLocation::Accepted {
                        provenance,
                        evidence,
                        ..
                    } => TraceOwnerLocation::Accepted {
                        provenance: trace_provenance(*provenance),
                        status: trace_status(evidence.proposal_status),
                    },
                    OwnerLocation::ReplacementHistory { .. } => {
                        TraceOwnerLocation::ReplacementHistory
                    }
                };
                Ok(TraceOwnerObservation {
                    transaction: TraceTxId(id.0),
                    version_rank: *self
                        .version_ranks
                        .get(&owner.version)
                        .ok_or(ReferenceTraceError::CounterOverflow)?,
                    arrival_rank: *self
                        .arrival_ranks
                        .get(&owner.arrival)
                        .ok_or(ReferenceTraceError::CounterOverflow)?,
                    location,
                })
            })
            .collect::<Result<Vec<_>, ReferenceTraceError>>()?;
        owners.sort_unstable();

        let mut work = self
            .omega
            .linear
            .work
            .values()
            .map(|capability| {
                Ok(TraceWorkObservation {
                    capability_rank: *self
                        .capability_ranks
                        .get(&capability.id)
                        .ok_or(ReferenceTraceError::CounterOverflow)?,
                    transaction: TraceTxId(capability.transaction.0),
                    permit: trace_permit(capability.permit()),
                    stage: trace_stage(capability.stage()),
                    location: TraceWorkLocation::Executing,
                })
            })
            .chain(self.omega.linear.finished_work.values().map(|finished| {
                let capability = &finished.capability;
                Ok(TraceWorkObservation {
                    capability_rank: *self
                        .capability_ranks
                        .get(&capability.id)
                        .ok_or(ReferenceTraceError::CounterOverflow)?,
                    transaction: TraceTxId(capability.transaction.0),
                    permit: trace_permit(capability.permit()),
                    stage: trace_stage(capability.stage()),
                    location: TraceWorkLocation::Finished,
                })
            }))
            .collect::<Result<Vec<_>, ReferenceTraceError>>()?;
        work.sort_unstable();

        let resources = reference_resource_counts(&self.omega)?;
        let effects = reference_effects(&self.omega)?;
        let effect_claim = self
            .omega
            .linear
            .effect_claim
            .map(trace_claim)
            .transpose()?;
        Ok(TraceObservation {
            last_apply: u64::from(self.omega.authority.last_apply.0),
            generation: u64::from(self.omega.authority.generation.0),
            chain_revision: u64::from(self.omega.authority.chain.revision.0),
            owners,
            work,
            resources,
            effects,
            effect_claim,
        })
    }

    fn transaction(&self, id: TraceTxId) -> Result<&Transaction, ReferenceTraceError> {
        self.transactions
            .get(&id)
            .ok_or(ReferenceTraceError::UnknownTransaction(id))
    }

    fn executing_capability(
        &self,
        transaction: TraceTxId,
    ) -> Result<super::state::WorkCapability, ReferenceTraceError> {
        let mut matches = self
            .omega
            .linear
            .work
            .values()
            .filter(|capability| capability.transaction == TxId(transaction.0));
        let capability = matches
            .next()
            .cloned()
            .ok_or(ReferenceTraceError::MissingCapability(transaction))?;
        if matches.next().is_some() {
            return Err(ReferenceTraceError::DuplicateCapability(transaction));
        }
        Ok(capability)
    }

    fn register_capability(&mut self, capability: CapabilityId) -> Result<(), ReferenceTraceError> {
        if self.capability_ranks.contains_key(&capability) {
            return Ok(());
        }
        let rank = u16::try_from(self.capability_ranks.len())
            .map_err(|_| ReferenceTraceError::CounterOverflow)?;
        self.capability_ranks.insert(capability, rank);
        Ok(())
    }

    fn register_owner_ranks(&mut self) -> Result<(), ReferenceTraceError> {
        let mut versions = self
            .omega
            .authority
            .owners
            .values()
            .map(|owner| owner.version)
            .filter(|version| !self.version_ranks.contains_key(version))
            .collect::<Vec<_>>();
        versions.sort_unstable();
        versions.dedup();
        for version in versions {
            let rank = u16::try_from(self.version_ranks.len())
                .map_err(|_| ReferenceTraceError::CounterOverflow)?;
            self.version_ranks.insert(version, rank);
        }
        let mut arrivals = self
            .omega
            .authority
            .owners
            .values()
            .map(|owner| owner.arrival)
            .filter(|arrival| !self.arrival_ranks.contains_key(arrival))
            .collect::<Vec<_>>();
        arrivals.sort_unstable();
        arrivals.dedup();
        for arrival in arrivals {
            let rank = u16::try_from(self.arrival_ranks.len())
                .map_err(|_| ReferenceTraceError::CounterOverflow)?;
            self.arrival_ranks.insert(arrival, rank);
        }
        Ok(())
    }
}

fn reference_resource_counts(omega: &Omega) -> Result<TraceResourceCounts, ReferenceTraceError> {
    let count = |predicate: &dyn Fn(&OwnerLocation) -> bool| {
        u16::try_from(
            omega
                .authority
                .owners
                .values()
                .filter(|owner| predicate(&owner.location))
                .count(),
        )
        .map_err(|_| ReferenceTraceError::CounterOverflow)
    };
    let owners = u16::try_from(omega.authority.owners.len())
        .map_err(|_| ReferenceTraceError::CounterOverflow)?;
    let retained = count(&|location| matches!(location, OwnerLocation::Retained(_)))?;
    let remote = count(&|location| {
        matches!(
            location,
            OwnerLocation::Retained(RetainedOwner {
                source: Source::Remote(_)
                    | Source::Proposal {
                        base: ProposalBase::Remote(_),
                    },
                ..
            })
        )
    })?;
    let accepted = count(&|location| matches!(location, OwnerLocation::Accepted { .. }))?;
    let replacement_history =
        count(&|location| matches!(location, OwnerLocation::ReplacementHistory { .. }))?;
    let active_work = u16::try_from(
        omega
            .linear
            .work
            .len()
            .checked_add(omega.linear.finished_work.len())
            .ok_or(ReferenceTraceError::CounterOverflow)?,
    )
    .map_err(|_| ReferenceTraceError::CounterOverflow)?;
    Ok(TraceResourceCounts {
        owners,
        charged_owners: owners,
        retained,
        remote,
        accepted,
        replacement_history,
        active_work,
    })
}

fn reference_effects(omega: &Omega) -> Result<Vec<TraceEffectObservation>, ReferenceTraceError> {
    if omega.authority.latest_generation_reset.is_some() {
        return Err(ReferenceTraceError::UnsupportedEffect);
    }
    omega
        .authority
        .effects
        .iter()
        .map(|record| {
            let LogicalEffect::Accepted {
                transaction,
                cause:
                    AcceptanceEffect::Admission {
                        status,
                        ingress_peer,
                    },
                ..
            } = record.logical
            else {
                return Err(ReferenceTraceError::UnsupportedEffect);
            };
            Ok(TraceEffectObservation {
                sequence: u64::from(record.stamp.0),
                ordinal: record.ordinal,
                class: trace_effect_class(record.class),
                effect: TraceEffect::Accepted {
                    transaction: TraceTxId(transaction.0),
                    status: trace_status(status),
                    ingress_peer: ingress_peer.map(|peer| TracePeerId(peer.0)),
                },
            })
        })
        .collect()
}

fn trace_claim(claim: super::state::EffectClaim) -> Result<TraceEffectClaim, ReferenceTraceError> {
    if claim.source != EffectClaimSource::Queued {
        return Err(ReferenceTraceError::UnsupportedEffect);
    }
    Ok(TraceEffectClaim {
        sequence: u64::from(claim.stamp.0),
        ordinal: claim.ordinal,
    })
}

const fn model_verify_class(class: TraceVerifyClass) -> VerifyCycleClass {
    match class {
        TraceVerifyClass::Small => VerifyCycleClass::Small,
        TraceVerifyClass::Large => VerifyCycleClass::Large,
    }
}

const fn model_capability(capability: TraceVerifyCapability) -> VerifyCapability {
    match capability {
        TraceVerifyCapability::Any => VerifyCapability::Any,
        TraceVerifyCapability::SmallCycleOnly => VerifyCapability::SmallCycleOnly,
    }
}

const fn trace_verify_class(class: VerifyCycleClass) -> TraceVerifyClass {
    match class {
        VerifyCycleClass::Small => TraceVerifyClass::Small,
        VerifyCycleClass::Large => TraceVerifyClass::Large,
    }
}

const fn trace_capability(capability: VerifyCapability) -> TraceVerifyCapability {
    match capability {
        VerifyCapability::Any => TraceVerifyCapability::Any,
        VerifyCapability::SmallCycleOnly => TraceVerifyCapability::SmallCycleOnly,
    }
}

const fn trace_permit(permit: WorkPermit) -> TraceWorkPermit {
    match permit {
        WorkPermit::ResolveOnly => TraceWorkPermit::ResolveOnly,
        WorkPermit::VerifyOnly(capability) => {
            TraceWorkPermit::VerifyOnly(trace_capability(capability))
        }
        WorkPermit::ResolveThenVerify(capability) => {
            TraceWorkPermit::ResolveThenVerify(trace_capability(capability))
        }
    }
}

fn trace_stage(stage: &WorkStage) -> TraceWorkStage {
    match stage {
        WorkStage::Resolve => TraceWorkStage::Resolve,
        WorkStage::Verify(evidence) => {
            TraceWorkStage::Verify(trace_verify_class(evidence.verify_class))
        }
    }
}

const fn trace_status(status: AcceptedStatus) -> TraceAcceptedStatus {
    match status {
        AcceptedStatus::Pending => TraceAcceptedStatus::Pending,
        AcceptedStatus::Gap => TraceAcceptedStatus::Gap,
        AcceptedStatus::Proposed => TraceAcceptedStatus::Proposed,
    }
}

const fn trace_source(source: Source) -> TraceRetainedSource {
    match source {
        Source::Remote(residency) => TraceRetainedSource::Remote(TracePeerId(residency.peer.0)),
        Source::Proposal {
            base: ProposalBase::Trusted,
        } => TraceRetainedSource::Proposal { ingress_peer: None },
        Source::Proposal {
            base: ProposalBase::Remote(residency),
        } => TraceRetainedSource::Proposal {
            ingress_peer: Some(TracePeerId(residency.peer.0)),
        },
        Source::Recovery(_) => TraceRetainedSource::Recovery,
    }
}

const fn trace_provenance(provenance: AcceptedProvenance) -> TraceAcceptedProvenance {
    match provenance {
        AcceptedProvenance::Trusted => TraceAcceptedProvenance::Trusted,
        AcceptedProvenance::Peer(peer) => TraceAcceptedProvenance::Peer(TracePeerId(peer.0)),
    }
}

fn trace_retained_phase(phase: &RetainedPhase) -> TraceRetainedPhase {
    match phase {
        RetainedPhase::Queued(WorkStage::Resolve) => TraceRetainedPhase::QueuedResolve,
        RetainedPhase::Queued(WorkStage::Verify(evidence)) => {
            TraceRetainedPhase::QueuedVerify(trace_verify_class(evidence.verify_class))
        }
        RetainedPhase::Computing(permit) => TraceRetainedPhase::Computing(trace_permit(*permit)),
        RetainedPhase::Waiting { .. } => TraceRetainedPhase::Waiting,
        RetainedPhase::Ready(_) => TraceRetainedPhase::Ready,
    }
}

const fn trace_effect_class(class: EffectClass) -> TraceEffectClass {
    match class {
        EffectClass::Remote => TraceEffectClass::Remote,
        EffectClass::Trusted => TraceEffectClass::Trusted,
        EffectClass::Critical => TraceEffectClass::Critical,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_lifecycle_domain_is_the_complete_class_route_product() {
        let scenarios = TraceVerifyClass::ALL
            .into_iter()
            .flat_map(|class| {
                TraceLifecycleRoute::ALL
                    .into_iter()
                    .map(move |route| TraceScenario::lifecycle(TraceTxId(1), class, route))
            })
            .collect::<Vec<_>>();
        assert_eq!(scenarios.len(), 6);
        let action_shapes = scenarios
            .iter()
            .map(TraceScenario::actions)
            .collect::<Vec<_>>();
        assert!(action_shapes.iter().all(|actions| actions.len() == 8));
        assert!(action_shapes.iter().all(|actions| {
            matches!(
                actions.as_slice(),
                [
                    TraceAction::AdmitRemote { .. },
                    TraceAction::Checkout(_),
                    TraceAction::Resolve(_),
                    TraceAction::Checkout(TraceWorkPermit::VerifyOnly(TraceVerifyCapability::Any)),
                    TraceAction::Verify(_),
                    TraceAction::FinalizeReady,
                    TraceAction::ClaimEffect,
                    TraceAction::SettleEffect,
                ]
            )
        }));
    }

    #[test]
    fn reference_lifecycle_domain_preserves_every_stable_cut() {
        for class in TraceVerifyClass::ALL {
            for route in TraceLifecycleRoute::ALL {
                let scenario = TraceScenario::lifecycle(TraceTxId(1), class, route);
                let cuts = replay_reference_trace(&scenario)
                    .expect("every finite lifecycle trace is legal in the reference kernel");
                assert_eq!(cuts.len(), scenario.actions().len());
                assert_eq!(
                    cuts.last().map(|cut| cut.observation.effects.len()),
                    Some(0)
                );
                assert_eq!(
                    cuts.last().and_then(|cut| cut.observation.effect_claim),
                    None
                );
            }
        }
    }
}
