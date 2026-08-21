//! Stable-cut trace refinement for the authority core.
//!
//! The symbolic alphabet and observation value types are shared with the
//! reference claim. All production transitions and normalization below are
//! independently implemented from real authority state and move-only receipts.

use super::claim_relations::{
    TraceAcceptedProvenance, TraceAcceptedStatus, TraceAction, TraceDisposition, TraceEffect,
    TraceEffectClaim, TraceEffectClass, TraceEffectObservation, TraceLifecycleRoute,
    TraceObservation, TraceOwnerLocation, TraceOwnerObservation, TracePeerId, TraceResourceCounts,
    TraceRetainedPhase, TraceRetainedSource, TraceScenario, TraceTxId, TraceVerifyCapability,
    TraceVerifyClass, TraceWorkLocation, TraceWorkObservation, TraceWorkPermit, TraceWorkStage,
};
use super::foundation::{apply_plan, limits, resolved_payload_with_facts, tx};
use crate::authority::{
    effect::{CommittedAcceptance, CommittedEffect, EffectReceipt, test_support::EffectTraceClass},
    plan::TxPoolAuthority,
    state::{
        AcceptedProvenance, AcceptedStatus, Arrival, EntryVersion, OwnedTx, PreAcceptedPhase,
        PreAcceptedSource, ProposalBase, RawTxHash, RemoteDeadline, RemoteResidencyLease,
        TxIdentity, ValidatedAdmission, VerifyCapability, VerifyCycleClass, WorkPermit,
    },
    work::{
        CheckedOutWork, ContinuousResolution, ContinuousResolveWork, ContinuousVerifyWork,
        ResolveWork, VerifyWork,
    },
};
use ckb_network::PeerIndex;
use ckb_types::core::{Capacity, TransactionView};
use std::{collections::BTreeMap, fmt};

#[derive(Debug)]
enum ProductionTraceError {
    UnknownTransaction(TraceTxId),
    UnknownHash,
    DuplicateWork(TraceTxId),
    MissingWork(TraceTxId),
    UnexpectedWork(TraceTxId),
    Plan(String),
    Receipt(String),
    UnsupportedEffect,
    CounterOverflow,
    Projection,
}

impl fmt::Display for ProductionTraceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTransaction(transaction) => {
                write!(formatter, "unknown transaction {transaction:?}")
            }
            Self::UnknownHash => formatter.write_str("unknown transaction hash"),
            Self::DuplicateWork(transaction) => {
                write!(formatter, "duplicate work for {transaction:?}")
            }
            Self::MissingWork(transaction) => {
                write!(formatter, "missing work for {transaction:?}")
            }
            Self::UnexpectedWork(transaction) => {
                write!(formatter, "unexpected work for {transaction:?}")
            }
            Self::Plan(error) => write!(formatter, "plan failed: {error}"),
            Self::Receipt(error) => write!(formatter, "receipt failed: {error}"),
            Self::UnsupportedEffect => formatter.write_str("unsupported effect"),
            Self::CounterOverflow => formatter.write_str("trace counter overflow"),
            Self::Projection => formatter.write_str("production projection mismatch"),
        }
    }
}

struct ProductionWorkSlot {
    rank: u16,
    work: ProductionWork,
}

enum ProductionWork {
    Resolve(ResolveWork),
    ContinuousResolve(ContinuousResolveWork),
    Verify(VerifyWork),
    ContinuousVerify(ContinuousVerifyWork),
}

impl ProductionWork {
    fn stage(&self) -> TraceWorkStage {
        match self {
            Self::Resolve(_) | Self::ContinuousResolve(_) => TraceWorkStage::Resolve,
            Self::Verify(work) => {
                TraceWorkStage::Verify(trace_verify_class(work.verify_class_for_foundation()))
            }
            Self::ContinuousVerify(work) => {
                TraceWorkStage::Verify(trace_verify_class(work.verify_class_for_foundation()))
            }
        }
    }
}

struct ProductionTrace {
    authority: TxPoolAuthority,
    scenario: TraceScenario,
    transactions: BTreeMap<TraceTxId, TransactionView>,
    hashes: BTreeMap<TraceTxId, RawTxHash>,
    work: BTreeMap<TraceTxId, ProductionWorkSlot>,
    next_work_rank: u16,
    effect_claim: Option<EffectReceipt>,
    version_ranks: BTreeMap<EntryVersion, u16>,
    arrival_ranks: BTreeMap<Arrival, u16>,
}

impl ProductionTrace {
    fn new(scenario: TraceScenario) -> Self {
        let transaction = tx(u64::from(scenario.transaction.id.0));
        let identity = TxIdentity::from_transaction(&transaction);
        Self {
            authority: TxPoolAuthority::for_foundation(limits()),
            transactions: BTreeMap::from([(scenario.transaction.id, transaction)]),
            hashes: BTreeMap::from([(scenario.transaction.id, identity.raw)]),
            scenario,
            work: BTreeMap::new(),
            next_work_rank: 0,
            effect_claim: None,
            version_ranks: BTreeMap::new(),
            arrival_ranks: BTreeMap::new(),
        }
    }

    fn step(&mut self, action: TraceAction) -> Result<TraceDisposition, ProductionTraceError> {
        match action {
            TraceAction::AdmitRemote {
                transaction,
                peer,
                deadline,
            } => {
                let tx = self.transaction(transaction)?.clone();
                let admission = ValidatedAdmission::remote_with_lease(
                    tx,
                    RemoteResidencyLease::new(
                        PeerIndex::from(usize::from(peer.0)),
                        RemoteDeadline(deadline),
                    ),
                    declared_cycles(self.scenario.transaction.verify_class),
                )
                .map_err(|error| ProductionTraceError::Receipt(format!("{error:?}")))?;
                apply_plan(
                    self.authority
                        .plan_admission(admission)
                        .map_err(|error| ProductionTraceError::Plan(format!("{error:?}")))?,
                );
                Ok(TraceDisposition::Retained(transaction))
            }
            TraceAction::Checkout(permit) => {
                let production_permit = production_permit(permit);
                let Some(committed) = self
                    .authority
                    .plan_checkout_next(production_permit)
                    .map_err(|error| ProductionTraceError::Plan(format!("{error:?}")))?
                else {
                    return Ok(TraceDisposition::Idle);
                };
                let checked_out = committed.apply().into_work();
                let transaction = self.trace_transaction(checked_out_transaction(&checked_out))?;
                if self.work.contains_key(&transaction) {
                    return Err(ProductionTraceError::DuplicateWork(transaction));
                }
                let stage = checked_out_stage(&checked_out);
                let actual_permit = self.owner_computing_permit(transaction)?;
                if actual_permit != permit {
                    return Err(ProductionTraceError::Projection);
                }
                let rank = self.next_work_rank;
                self.next_work_rank = self
                    .next_work_rank
                    .checked_add(1)
                    .ok_or(ProductionTraceError::CounterOverflow)?;
                self.work.insert(
                    transaction,
                    ProductionWorkSlot {
                        rank,
                        work: production_work(checked_out),
                    },
                );
                Ok(TraceDisposition::CheckedOut {
                    transaction,
                    permit: actual_permit,
                    stage,
                })
            }
            TraceAction::Resolve(transaction) => {
                let slot = self
                    .work
                    .remove(&transaction)
                    .ok_or(ProductionTraceError::MissingWork(transaction))?;
                let verify_class = production_verify_class(self.transaction_class(transaction)?);
                match slot.work {
                    ProductionWork::Resolve(work) => {
                        let resolution = resolved_payload_with_facts(
                            work.transaction(),
                            Vec::new(),
                            Vec::new(),
                            Capacity::shannons(10),
                        );
                        let settlement = work
                            .yield_verify_as(resolution, verify_class)
                            .map_err(|error| ProductionTraceError::Receipt(format!("{error:?}")))?;
                        apply_plan(
                            self.authority
                                .apply_settlement(settlement)
                                .map_err(|error| {
                                    ProductionTraceError::Plan(format!("{error:?}"))
                                })?,
                        );
                        Ok(TraceDisposition::QueuedVerify(transaction))
                    }
                    ProductionWork::ContinuousResolve(work) => {
                        let resolution = resolved_payload_with_facts(
                            work.transaction(),
                            Vec::new(),
                            Vec::new(),
                            Capacity::shannons(10),
                        );
                        match work
                            .into_verify_as(resolution, verify_class)
                            .map_err(|error| ProductionTraceError::Receipt(format!("{error:?}")))?
                        {
                            ContinuousResolution::Verify(work) => {
                                self.work.insert(
                                    transaction,
                                    ProductionWorkSlot {
                                        rank: slot.rank,
                                        work: ProductionWork::ContinuousVerify(work),
                                    },
                                );
                                Ok(TraceDisposition::ResolveContinued(transaction))
                            }
                            ContinuousResolution::Settle(settlement) => {
                                apply_plan(self.authority.apply_settlement(settlement).map_err(
                                    |error| ProductionTraceError::Plan(format!("{error:?}")),
                                )?);
                                Ok(TraceDisposition::QueuedVerify(transaction))
                            }
                        }
                    }
                    ProductionWork::Verify(_) | ProductionWork::ContinuousVerify(_) => {
                        Err(ProductionTraceError::UnexpectedWork(transaction))
                    }
                }
            }
            TraceAction::Verify(transaction) => {
                let slot = self
                    .work
                    .remove(&transaction)
                    .ok_or(ProductionTraceError::MissingWork(transaction))?;
                let cycles = declared_cycles(self.transaction_class(transaction)?);
                let settlement = match slot.work {
                    ProductionWork::Verify(work) => work.verified(cycles),
                    ProductionWork::ContinuousVerify(work) => work.verified(cycles),
                    ProductionWork::Resolve(_) | ProductionWork::ContinuousResolve(_) => {
                        return Err(ProductionTraceError::UnexpectedWork(transaction));
                    }
                };
                apply_plan(
                    self.authority
                        .apply_settlement(settlement)
                        .map_err(|error| ProductionTraceError::Plan(format!("{error:?}")))?,
                );
                Ok(TraceDisposition::Ready(transaction))
            }
            TraceAction::FinalizeReady => {
                let ready = self.authority.ready_for_reference();
                let [(hash, version)] = ready.as_slice() else {
                    return Err(ProductionTraceError::Projection);
                };
                let transaction = self.trace_transaction_for_hash(hash)?;
                apply_plan(
                    self.authority
                        .plan_accept_for_foundation(hash, *version, AcceptedStatus::Pending)
                        .map_err(|error| ProductionTraceError::Plan(format!("{error:?}")))?,
                );
                Ok(TraceDisposition::Accepted(transaction))
            }
            TraceAction::ClaimEffect => {
                if let Some(receipt) = &self.effect_claim {
                    return Ok(TraceDisposition::EffectClaimed(TraceEffectClaim {
                        sequence: trace_sequence(receipt.sequence())?,
                        ordinal: 0,
                    }));
                }
                let receipt = self
                    .authority
                    .effect_publication_receipt_for_foundation()
                    .ok_or(ProductionTraceError::Projection)?;
                let claim = TraceEffectClaim {
                    sequence: trace_sequence(receipt.sequence())?,
                    ordinal: 0,
                };
                self.effect_claim = Some(receipt);
                Ok(TraceDisposition::EffectClaimed(claim))
            }
            TraceAction::SettleEffect => {
                let receipt = self
                    .effect_claim
                    .take()
                    .ok_or(ProductionTraceError::Projection)?;
                let claim = TraceEffectClaim {
                    sequence: trace_sequence(receipt.sequence())?,
                    ordinal: 0,
                };
                drop(
                    self.authority
                        .apply_effect_settlement_for_foundation(
                            receipt.complete_for_foundation().published(),
                        )
                        .map_err(|error| ProductionTraceError::Plan(format!("{error:?}")))?,
                );
                Ok(TraceDisposition::EffectSettled(claim))
            }
        }
    }

    fn observe(&mut self) -> Result<TraceObservation, ProductionTraceError> {
        self.register_owner_ranks()?;
        let mut owners = self
            .authority
            .entries_for_reference()
            .iter()
            .map(|(hash, owner)| {
                let transaction = self.trace_transaction_for_hash(hash)?;
                let record = owner.record();
                let location = match owner {
                    OwnedTx::PreAccepted(entry) => TraceOwnerLocation::Retained {
                        source: trace_source(entry.source),
                        phase: trace_retained_phase(&entry.phase),
                    },
                    OwnedTx::Accepted(entry) => TraceOwnerLocation::Accepted {
                        provenance: trace_provenance(entry.provenance),
                        status: trace_status(entry.status()),
                    },
                    OwnedTx::ReplacementHistory(_) => TraceOwnerLocation::ReplacementHistory,
                };
                Ok(TraceOwnerObservation {
                    transaction,
                    version_rank: *self
                        .version_ranks
                        .get(&record.version)
                        .ok_or(ProductionTraceError::CounterOverflow)?,
                    arrival_rank: *self
                        .arrival_ranks
                        .get(&record.arrival)
                        .ok_or(ProductionTraceError::CounterOverflow)?,
                    location,
                })
            })
            .collect::<Result<Vec<_>, ProductionTraceError>>()?;
        owners.sort_unstable();

        let mut work = self
            .work
            .iter()
            .map(|(transaction, slot)| {
                let permit = self.owner_computing_permit(*transaction)?;
                Ok(TraceWorkObservation {
                    capability_rank: slot.rank,
                    transaction: *transaction,
                    permit,
                    stage: slot.work.stage(),
                    location: TraceWorkLocation::Executing,
                })
            })
            .collect::<Result<Vec<_>, ProductionTraceError>>()?;
        work.sort_unstable();

        let clocks = self.authority.clocks();
        let last_apply = clocks
            .next_sequence
            .0
            .checked_sub(1)
            .ok_or(ProductionTraceError::CounterOverflow)?;
        let resources = self.authority.resources().snapshot();
        let effects = self.production_effects()?;
        let effect_claim = self
            .effect_claim
            .as_ref()
            .map(|receipt| {
                Ok(TraceEffectClaim {
                    sequence: trace_sequence(receipt.sequence())?,
                    ordinal: 0,
                })
            })
            .transpose()?;
        Ok(TraceObservation {
            last_apply: u64::try_from(last_apply)
                .map_err(|_| ProductionTraceError::CounterOverflow)?,
            generation: self.authority.generation().0,
            chain_revision: self.authority.chain_view_for_reference().revision().0,
            owners,
            work,
            resources: TraceResourceCounts {
                owners: trace_count(self.authority.owner_count())?,
                charged_owners: trace_count(self.authority.charged_count())?,
                retained: trace_count(resources.preaccepted.entries)?,
                remote: trace_count(resources.remote.entries)?,
                accepted: trace_count(resources.accepted.entries)?,
                replacement_history: trace_count(resources.replacement_history.entries)?,
                active_work: trace_count(resources.preaccepted.active_work)?,
            },
            effects,
            effect_claim,
        })
    }

    fn production_effects(&self) -> Result<Vec<TraceEffectObservation>, ProductionTraceError> {
        let mut observations = Vec::new();
        for batch in self.authority.effect_trace_for_reference() {
            if batch.processed_steps != 0 {
                return Err(ProductionTraceError::UnsupportedEffect);
            }
            let class = batch
                .class
                .map(trace_effect_class)
                .ok_or(ProductionTraceError::UnsupportedEffect)?;
            let sequence = trace_sequence(batch.sequence)?;
            for (ordinal, effect) in batch.effects.into_iter().enumerate() {
                let ordinal =
                    u16::try_from(ordinal).map_err(|_| ProductionTraceError::CounterOverflow)?;
                let effect = match effect {
                    CommittedEffect::Accepted(CommittedAcceptance::Admission {
                        entry,
                        status,
                        ingress_peer,
                    }) => TraceEffect::Accepted {
                        transaction: self.trace_transaction(&entry.tx)?,
                        status: trace_status(status),
                        ingress_peer: ingress_peer.map(trace_peer),
                    },
                    _ => return Err(ProductionTraceError::UnsupportedEffect),
                };
                observations.push(TraceEffectObservation {
                    sequence,
                    ordinal,
                    class,
                    effect,
                });
            }
        }
        observations.sort_unstable();
        Ok(observations)
    }

    fn transaction(&self, id: TraceTxId) -> Result<&TransactionView, ProductionTraceError> {
        self.transactions
            .get(&id)
            .ok_or(ProductionTraceError::UnknownTransaction(id))
    }

    fn transaction_class(&self, id: TraceTxId) -> Result<TraceVerifyClass, ProductionTraceError> {
        (self.scenario.transaction.id == id)
            .then_some(self.scenario.transaction.verify_class)
            .ok_or(ProductionTraceError::UnknownTransaction(id))
    }

    fn trace_transaction(
        &self,
        transaction: &TransactionView,
    ) -> Result<TraceTxId, ProductionTraceError> {
        self.trace_transaction_for_hash(&RawTxHash(transaction.hash()))
    }

    fn trace_transaction_for_hash(
        &self,
        hash: &RawTxHash,
    ) -> Result<TraceTxId, ProductionTraceError> {
        self.hashes
            .iter()
            .find_map(|(id, candidate)| (candidate == hash).then_some(*id))
            .ok_or(ProductionTraceError::UnknownHash)
    }

    fn owner_computing_permit(
        &self,
        transaction: TraceTxId,
    ) -> Result<TraceWorkPermit, ProductionTraceError> {
        let hash = self
            .hashes
            .get(&transaction)
            .ok_or(ProductionTraceError::UnknownTransaction(transaction))?;
        let Some(OwnedTx::PreAccepted(entry)) = self.authority.entry(hash) else {
            return Err(ProductionTraceError::Projection);
        };
        let PreAcceptedPhase::Computing(active) = &entry.phase else {
            return Err(ProductionTraceError::Projection);
        };
        Ok(trace_permit(active.permit))
    }

    fn register_owner_ranks(&mut self) -> Result<(), ProductionTraceError> {
        let mut versions = self
            .authority
            .entries_for_reference()
            .values()
            .map(|owner| owner.record().version)
            .filter(|version| !self.version_ranks.contains_key(version))
            .collect::<Vec<_>>();
        versions.sort_unstable();
        versions.dedup();
        for version in versions {
            let rank = trace_count(self.version_ranks.len())?;
            self.version_ranks.insert(version, rank);
        }
        let mut arrivals = self
            .authority
            .entries_for_reference()
            .values()
            .map(|owner| owner.record().arrival)
            .filter(|arrival| !self.arrival_ranks.contains_key(arrival))
            .collect::<Vec<_>>();
        arrivals.sort_unstable();
        arrivals.dedup();
        for arrival in arrivals {
            let rank = trace_count(self.arrival_ranks.len())?;
            self.arrival_ranks.insert(arrival, rank);
        }
        Ok(())
    }
}

fn checked_out_transaction(work: &CheckedOutWork) -> &TransactionView {
    match work {
        CheckedOutWork::Resolve(work) => work.transaction(),
        CheckedOutWork::ContinuousResolve(work) => work.transaction(),
        CheckedOutWork::Verify(work) => work.transaction(),
    }
}

fn checked_out_stage(work: &CheckedOutWork) -> TraceWorkStage {
    match work {
        CheckedOutWork::Resolve(_) | CheckedOutWork::ContinuousResolve(_) => {
            TraceWorkStage::Resolve
        }
        CheckedOutWork::Verify(work) => {
            TraceWorkStage::Verify(trace_verify_class(work.verify_class_for_foundation()))
        }
    }
}

fn production_work(work: CheckedOutWork) -> ProductionWork {
    match work {
        CheckedOutWork::Resolve(work) => ProductionWork::Resolve(work),
        CheckedOutWork::ContinuousResolve(work) => ProductionWork::ContinuousResolve(work),
        CheckedOutWork::Verify(work) => ProductionWork::Verify(work),
    }
}

fn production_permit(permit: TraceWorkPermit) -> WorkPermit {
    match permit {
        TraceWorkPermit::ResolveOnly => WorkPermit::ResolveOnly,
        TraceWorkPermit::VerifyOnly(capability) => {
            WorkPermit::VerifyOnly(production_capability(capability))
        }
        TraceWorkPermit::ResolveThenVerify(capability) => {
            WorkPermit::ResolveThenVerify(production_capability(capability))
        }
    }
}

fn production_capability(capability: TraceVerifyCapability) -> VerifyCapability {
    match capability {
        TraceVerifyCapability::Any => VerifyCapability::Any,
        TraceVerifyCapability::SmallCycleOnly => VerifyCapability::SmallCycleOnly,
    }
}

fn production_verify_class(class: TraceVerifyClass) -> VerifyCycleClass {
    match class {
        TraceVerifyClass::Small => VerifyCycleClass::Small,
        TraceVerifyClass::Large => VerifyCycleClass::Large,
    }
}

fn trace_verify_class(class: VerifyCycleClass) -> TraceVerifyClass {
    match class {
        VerifyCycleClass::Small => TraceVerifyClass::Small,
        VerifyCycleClass::Large => TraceVerifyClass::Large,
    }
}

fn trace_capability(capability: VerifyCapability) -> TraceVerifyCapability {
    match capability {
        VerifyCapability::Any => TraceVerifyCapability::Any,
        VerifyCapability::SmallCycleOnly => TraceVerifyCapability::SmallCycleOnly,
    }
}

fn trace_permit(permit: WorkPermit) -> TraceWorkPermit {
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

fn trace_source(source: PreAcceptedSource) -> TraceRetainedSource {
    match source {
        PreAcceptedSource::Remote(remote) => {
            TraceRetainedSource::Remote(trace_peer(remote.residency.peer))
        }
        PreAcceptedSource::Proposal {
            base: ProposalBase::Trusted,
        } => TraceRetainedSource::Proposal { ingress_peer: None },
        PreAcceptedSource::Proposal {
            base: ProposalBase::Remote(residency),
        } => TraceRetainedSource::Proposal {
            ingress_peer: Some(trace_peer(residency.peer)),
        },
        PreAcceptedSource::Recovery(_) => TraceRetainedSource::Recovery,
    }
}

fn trace_provenance(provenance: AcceptedProvenance) -> TraceAcceptedProvenance {
    match provenance {
        AcceptedProvenance::Trusted => TraceAcceptedProvenance::Trusted,
        AcceptedProvenance::Peer { ingress } => TraceAcceptedProvenance::Peer(trace_peer(ingress)),
    }
}

fn trace_retained_phase(phase: &PreAcceptedPhase) -> TraceRetainedPhase {
    match phase {
        PreAcceptedPhase::Queued(crate::authority::state::QueuedWork::Resolve) => {
            TraceRetainedPhase::QueuedResolve
        }
        PreAcceptedPhase::Queued(crate::authority::state::QueuedWork::Verify(resolved)) => {
            TraceRetainedPhase::QueuedVerify(trace_verify_class(resolved.verify_class()))
        }
        PreAcceptedPhase::Computing(active) => {
            TraceRetainedPhase::Computing(trace_permit(active.permit))
        }
        PreAcceptedPhase::Waiting(_) => TraceRetainedPhase::Waiting,
        PreAcceptedPhase::Ready(_) => TraceRetainedPhase::Ready,
    }
}

fn trace_status(status: AcceptedStatus) -> TraceAcceptedStatus {
    match status {
        AcceptedStatus::Pending => TraceAcceptedStatus::Pending,
        AcceptedStatus::Gap => TraceAcceptedStatus::Gap,
        AcceptedStatus::Proposed => TraceAcceptedStatus::Proposed,
    }
}

fn trace_effect_class(class: EffectTraceClass) -> TraceEffectClass {
    match class {
        EffectTraceClass::Remote => TraceEffectClass::Remote,
        EffectTraceClass::Trusted => TraceEffectClass::Trusted,
        EffectTraceClass::Critical => TraceEffectClass::Critical,
    }
}

fn trace_peer(peer: PeerIndex) -> TracePeerId {
    TracePeerId(u8::try_from(peer.value()).expect("the finite trace peer fits u8"))
}

fn trace_count(count: usize) -> Result<u16, ProductionTraceError> {
    u16::try_from(count).map_err(|_| ProductionTraceError::CounterOverflow)
}

fn trace_sequence(
    sequence: crate::authority::state::ApplySequence,
) -> Result<u64, ProductionTraceError> {
    u64::try_from(sequence.0).map_err(|_| ProductionTraceError::CounterOverflow)
}

fn declared_cycles(class: TraceVerifyClass) -> u64 {
    // The authority consumes a sealed class from Resolve; the resolver owns
    // the configurable threshold. A threshold of one makes both finite-trace
    // witnesses production-reachable without turning class coverage into an
    // unrelated accepted-capacity eviction test.
    match class {
        TraceVerifyClass::Small => 1,
        TraceVerifyClass::Large => 2,
    }
}

fn route_continues(class: TraceVerifyClass, route: TraceLifecycleRoute) -> bool {
    matches!(
        route,
        TraceLifecycleRoute::Continuous(TraceVerifyCapability::Any)
    ) || matches!(
        (class, route),
        (
            TraceVerifyClass::Small,
            TraceLifecycleRoute::Continuous(TraceVerifyCapability::SmallCycleOnly)
        )
    )
}

fn initial_permit(route: TraceLifecycleRoute) -> TraceWorkPermit {
    match route {
        TraceLifecycleRoute::Split => TraceWorkPermit::ResolveOnly,
        TraceLifecycleRoute::Continuous(capability) => {
            TraceWorkPermit::ResolveThenVerify(capability)
        }
    }
}

fn assert_observation_invariants(observation: &TraceObservation) {
    assert_eq!(
        usize::from(observation.resources.owners),
        observation.owners.len()
    );
    assert_eq!(
        observation.resources.owners,
        observation.resources.charged_owners
    );
    assert_eq!(
        observation.resources.owners,
        observation.resources.retained
            + observation.resources.accepted
            + observation.resources.replacement_history
    );
    assert_eq!(
        usize::from(observation.resources.active_work),
        observation.work.len()
    );
    assert!(
        observation
            .owners
            .array_windows::<2>()
            .all(|[left, right]| { left.transaction != right.transaction })
    );
    assert!(observation.work.iter().all(|work| {
        observation
            .owners
            .iter()
            .any(|owner| owner.transaction == work.transaction)
    }));
    if let Some(claim) = observation.effect_claim {
        assert!(observation.effects.iter().any(|effect| {
            effect.sequence == claim.sequence && effect.ordinal == claim.ordinal
        }));
    }
}

#[test]
fn uak_authority_lifecycle_obeys_every_stable_cut_property() {
    for class in TraceVerifyClass::ALL {
        for route in TraceLifecycleRoute::ALL {
            let scenario = TraceScenario::lifecycle(TraceTxId(1), class, route);
            let actions = scenario.actions();
            let mut production = ProductionTrace::new(scenario.clone());
            let mut previous_apply = None;
            let mut effect_claim = None;
            for (index, action) in actions.iter().copied().enumerate() {
                let actual_disposition = production.step(action).unwrap_or_else(|error| {
                    panic!(
                        "production trace failed at {class:?}/{route:?} cut {index}: {error}; prefix={:#?}",
                        &actions[..=index]
                    )
                });
                match index {
                    0 => assert_eq!(actual_disposition, TraceDisposition::Retained(TraceTxId(1))),
                    1 => assert_eq!(
                        actual_disposition,
                        TraceDisposition::CheckedOut {
                            transaction: TraceTxId(1),
                            permit: initial_permit(route),
                            stage: TraceWorkStage::Resolve,
                        }
                    ),
                    2 if route_continues(class, route) => assert_eq!(
                        actual_disposition,
                        TraceDisposition::ResolveContinued(TraceTxId(1))
                    ),
                    2 => assert_eq!(
                        actual_disposition,
                        TraceDisposition::QueuedVerify(TraceTxId(1))
                    ),
                    3 if route_continues(class, route) => {
                        assert_eq!(actual_disposition, TraceDisposition::Idle)
                    }
                    3 => assert_eq!(
                        actual_disposition,
                        TraceDisposition::CheckedOut {
                            transaction: TraceTxId(1),
                            permit: TraceWorkPermit::VerifyOnly(TraceVerifyCapability::Any),
                            stage: TraceWorkStage::Verify(class),
                        }
                    ),
                    4 => assert_eq!(actual_disposition, TraceDisposition::Ready(TraceTxId(1))),
                    5 => assert_eq!(actual_disposition, TraceDisposition::Accepted(TraceTxId(1))),
                    6 => {
                        let TraceDisposition::EffectClaimed(claim) = actual_disposition else {
                            panic!("the committed acceptance must expose one effect claim")
                        };
                        effect_claim = Some(claim);
                    }
                    7 => assert_eq!(
                        actual_disposition,
                        TraceDisposition::EffectSettled(
                            effect_claim.expect("the prior cut claimed the exact effect")
                        )
                    ),
                    _ => unreachable!("the lifecycle action vector has eight cuts"),
                }
                let actual_observation = production.observe().unwrap_or_else(|error| {
                    panic!(
                        "production observation failed at {class:?}/{route:?} cut {index}: {error}; prefix={:#?}",
                        &actions[..=index]
                    )
                });
                assert_observation_invariants(&actual_observation);
                if let Some(previous) = previous_apply {
                    assert!(actual_observation.last_apply >= previous);
                }
                previous_apply = Some(actual_observation.last_apply);
                if index == 7 {
                    assert!(actual_observation.effect_claim.is_none());
                }
            }
        }
    }
}
