//! M2 adversarial shapes and checked quantitative proof terms.
//!
//! These types are audit instruments, not runtime scheduler state. They make
//! hostile count, byte, key, edge, slot, wake and repeated-work bounds
//! executable without fitting constants to benchmark results.

use super::{
    composition::CompositionCost,
    kernel::{
        Admission, ChainTransition, Completion, KernelCommand, KernelStep, ResolveContinuation,
        WorkResult,
    },
    state::{
        ApplyStamp, CapabilityId, CellId, ChainView, EvidenceContext, HeaderId, MonotonicTick,
        Omega, OwnerLocation, PeerBanDeadline, PeerId, ProposalId, RemoteDeadline, RemoteResidency,
        ResolvedEvidence, RetainedSource, RulesId, Transaction, TxId, VerifyCapability,
        VerifyCycleClass, ViewId, WitnessId, WorkKind,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
    num::NonZeroU16,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AdversarialShape {
    Independent(u8),
    SharedInput(u8),
    SharedHeaderRead(u8),
    DeepChain(u8),
    ReadFanout(u8),
    RbfReplacement,
    ConditionalReadWrite,
    ProposalCollision,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScenarioError {
    Empty,
    IdentifierBound,
    ArithmeticBound,
}

/// The irreducible M2 premises. Each variant has one typed, minimum-cardinality
/// witness below; adding a premise without a witness is therefore a compile-time
/// incomplete match rather than a prose-only obligation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum M2RootPremise {
    FullIdentityIsNotProposalKey,
    InputWritesAreExclusive,
    ProducedCellsImposeCausalOrder,
    ReadsOrderAgainstSpends,
    PoolOriginIsEvidence,
    PlanUsesExactAuthorityCut,
    WorkerLocationsAreSlotBounded,
    ResourceArithmeticIsChecked,
    PermitOrderIsFair,
    RepeatedWorkNeedsNewEvidence,
    DetachedEndpointWorkIsBounded,
}

impl M2RootPremise {
    pub(super) const ALL: [Self; 11] = [
        Self::FullIdentityIsNotProposalKey,
        Self::InputWritesAreExclusive,
        Self::ProducedCellsImposeCausalOrder,
        Self::ReadsOrderAgainstSpends,
        Self::PoolOriginIsEvidence,
        Self::PlanUsesExactAuthorityCut,
        Self::WorkerLocationsAreSlotBounded,
        Self::ResourceArithmeticIsChecked,
        Self::PermitOrderIsFair,
        Self::RepeatedWorkNeedsNewEvidence,
        Self::DetachedEndpointWorkIsBounded,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PremiseViolation {
    ProposalAlias {
        proposal: ProposalId,
        first: TxId,
        second: TxId,
    },
    SharedInput {
        cell: CellId,
        first: TxId,
        second: TxId,
    },
    CausalCell {
        cell: CellId,
        producer: TxId,
        consumer: TxId,
    },
    ReadSpend {
        cell: CellId,
        reader: TxId,
        spender: TxId,
    },
    PoolOrigin {
        cell: CellId,
        parent: TxId,
        child: TxId,
    },
    StaleCut {
        planned: ApplyStamp,
        current: ApplyStamp,
    },
    WorkerSlotOverrun {
        slots: u8,
        executing: u8,
        finished: u8,
    },
    ResourceOverrun {
        limit: u8,
        observed: u8,
    },
    FairnessInversion {
        older_ticket: u8,
        selected_newer_ticket: u8,
    },
    SameEvidenceCut {
        transaction: TxId,
        context: EvidenceContext,
        kind: WorkKind,
    },
    DetachedCallOverrun {
        limit: u8,
        observed: u8,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PremiseCounterexample {
    pub(super) premise: M2RootPremise,
    pub(super) semantic_members: u8,
    pub(super) violation: PremiseViolation,
}

/// Construct the minimum witness over the symbolic M2 domain. Binary alias,
/// ordering and fairness laws require two distinct members; single-cut and
/// single-resource laws use the first positive difference or overrun.
pub(super) fn shortest_premise_counterexample(
    premise: M2RootPremise,
) -> Option<PremiseCounterexample> {
    let (semantic_members, violation) = match premise {
        M2RootPremise::FullIdentityIsNotProposalKey => {
            let transactions = adversarial_cohort(AdversarialShape::ProposalCollision).ok()?;
            let [first, second] = transactions.as_slice() else {
                return None;
            };
            (
                2,
                PremiseViolation::ProposalAlias {
                    proposal: (first.proposal == second.proposal).then_some(first.proposal)?,
                    first: first.id,
                    second: (first.id != second.id).then_some(second.id)?,
                },
            )
        }
        M2RootPremise::InputWritesAreExclusive => {
            let transactions = adversarial_cohort(AdversarialShape::SharedInput(2)).ok()?;
            let [first, second] = transactions.as_slice() else {
                return None;
            };
            let cell = first.inputs.intersection(&second.inputs).next().copied()?;
            (
                2,
                PremiseViolation::SharedInput {
                    cell,
                    first: first.id,
                    second: second.id,
                },
            )
        }
        M2RootPremise::ProducedCellsImposeCausalOrder => {
            let transactions = adversarial_cohort(AdversarialShape::DeepChain(2)).ok()?;
            let [producer, consumer] = transactions.as_slice() else {
                return None;
            };
            let cell = producer
                .outputs
                .intersection(&consumer.inputs)
                .next()
                .copied()?;
            (
                2,
                PremiseViolation::CausalCell {
                    cell,
                    producer: producer.id,
                    consumer: consumer.id,
                },
            )
        }
        M2RootPremise::ReadsOrderAgainstSpends => {
            let transactions = adversarial_cohort(AdversarialShape::ConditionalReadWrite).ok()?;
            let [reader, spender] = transactions.as_slice() else {
                return None;
            };
            let cell = reader.deps.intersection(&spender.inputs).next().copied()?;
            (
                2,
                PremiseViolation::ReadSpend {
                    cell,
                    reader: reader.id,
                    spender: spender.id,
                },
            )
        }
        M2RootPremise::PoolOriginIsEvidence => (
            2,
            PremiseViolation::PoolOrigin {
                cell: CellId(20),
                parent: TxId(1),
                child: TxId(2),
            },
        ),
        M2RootPremise::PlanUsesExactAuthorityCut => (
            1,
            PremiseViolation::StaleCut {
                planned: ApplyStamp(0),
                current: ApplyStamp(1),
            },
        ),
        M2RootPremise::WorkerLocationsAreSlotBounded => (
            2,
            PremiseViolation::WorkerSlotOverrun {
                slots: 1,
                executing: 1,
                finished: 1,
            },
        ),
        M2RootPremise::ResourceArithmeticIsChecked => (
            1,
            PremiseViolation::ResourceOverrun {
                limit: 0,
                observed: 1,
            },
        ),
        M2RootPremise::PermitOrderIsFair => (
            2,
            PremiseViolation::FairnessInversion {
                older_ticket: 1,
                selected_newer_ticket: 2,
            },
        ),
        M2RootPremise::RepeatedWorkNeedsNewEvidence => (
            1,
            PremiseViolation::SameEvidenceCut {
                transaction: TxId(1),
                context: EvidenceContext {
                    chain: ChainView::initial(ViewId(1)),
                    rules: RulesId(1),
                    witness: WitnessId(1),
                },
                kind: WorkKind::Resolve,
            },
        ),
        M2RootPremise::DetachedEndpointWorkIsBounded => (
            2,
            PremiseViolation::DetachedCallOverrun {
                limit: 1,
                observed: 2,
            },
        ),
    };
    Some(PremiseCounterexample {
        premise,
        semantic_members,
        violation,
    })
}

pub(super) fn adversarial_cohort(
    shape: AdversarialShape,
) -> Result<Vec<Transaction>, ScenarioError> {
    match shape {
        AdversarialShape::Independent(width) => build_width(width, |index| {
            Ok(Transaction::independent(
                index,
                index,
                checked_id(10, index)?,
                checked_id(100, index)?,
            ))
        }),
        AdversarialShape::SharedInput(width) => build_width(width, |index| {
            Ok(Transaction::independent(
                index,
                index,
                10,
                checked_id(100, index)?,
            ))
        }),
        AdversarialShape::SharedHeaderRead(width) => build_width(width, |index| {
            let mut transaction = Transaction::independent(
                index,
                index,
                checked_id(10, index)?,
                checked_id(100, index)?,
            );
            transaction.header_deps.insert(HeaderId(7));
            Ok(transaction)
        }),
        AdversarialShape::DeepChain(depth) => {
            if depth == 0 {
                return Err(ScenarioError::Empty);
            }
            let mut transactions = Vec::with_capacity(usize::from(depth));
            let mut input = 10u8;
            for index in 1..=depth {
                let output = checked_id(20, index)?;
                transactions.push(Transaction::independent(index, index, input, output));
                input = output;
            }
            Ok(transactions)
        }
        AdversarialShape::ReadFanout(width) => {
            if width == 0 {
                return Err(ScenarioError::Empty);
            }
            let parent = Transaction::independent(1, 1, 1, 2);
            let capacity = usize::from(width)
                .checked_add(1)
                .ok_or(ScenarioError::ArithmeticBound)?;
            let mut transactions = Vec::with_capacity(capacity);
            transactions.push(parent);
            for offset in 0..width {
                let id = offset
                    .checked_add(2)
                    .ok_or(ScenarioError::IdentifierBound)?;
                let mut reader = Transaction::independent(
                    id,
                    id,
                    checked_id(10, offset)?,
                    checked_id(120, offset)?,
                );
                reader.deps.insert(CellId(2));
                transactions.push(reader);
            }
            Ok(transactions)
        }
        AdversarialShape::RbfReplacement => {
            let original = Transaction::independent(1, 1, 10, 20);
            let mut replacement = Transaction::independent(2, 2, 10, 21);
            replacement.fee = original
                .fee
                .checked_add(10)
                .ok_or(ScenarioError::ArithmeticBound)?;
            Ok(vec![original, replacement])
        }
        AdversarialShape::ConditionalReadWrite => {
            let mut reader = Transaction::independent(1, 1, 11, 20);
            reader.deps.insert(CellId(10));
            Ok(vec![reader, Transaction::independent(2, 2, 10, 21)])
        }
        AdversarialShape::ProposalCollision => {
            let mut first = Transaction::independent(1, 1, 10, 20);
            let mut second = Transaction::independent(2, 2, 11, 21);
            first.proposal = ProposalId(7);
            second.proposal = ProposalId(7);
            Ok(vec![first, second])
        }
    }
}

/// An independently enumerated hostile schedule action. The action stores only
/// semantic identities; exact receipts and evidence are reconstructed from the
/// state cut where the action is generated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct HostileTxKey {
    pub(super) raw: TxId,
    pub(super) witness: WitnessId,
}

impl HostileTxKey {
    fn for_transaction(transaction: &Transaction) -> Self {
        Self {
            raw: transaction.id,
            witness: transaction.witness,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum HostileAction {
    AdmitRemote {
        transaction: HostileTxKey,
        peer: PeerId,
    },
    AdmitProposal {
        transaction: HostileTxKey,
    },
    Checkout,
    CheckoutContinuous(VerifyCapability),
    ContinueCurrent(CapabilityId),
    CompleteCurrent(CapabilityId),
    CompleteRejected(CapabilityId),
    FinishCurrent(CapabilityId),
    FinishRejected(CapabilityId),
    SettleFinished(CapabilityId),
    CancelCapability(CapabilityId),
    FinalizeNext,
    BanPeer(PeerId),
    ExpireRemote,
    AdvanceWallClock {
        to: u64,
    },
    AdvanceMonotonic {
        to: MonotonicTick,
    },
    AdvanceChain,
    ClaimEffect,
    SettleEffect {
        source: super::state::EffectClaimSource,
        stamp: ApplyStamp,
        ordinal: u16,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct HostileState {
    omega: Omega,
    wall_clock: u64,
    monotonic_clock: MonotonicTick,
}

impl HostileState {
    pub(super) fn omega(&self) -> &Omega {
        &self.omega
    }

    pub(super) const fn wall_clock(&self) -> u64 {
        self.wall_clock
    }

    pub(super) const fn monotonic_clock(&self) -> MonotonicTick {
        self.monotonic_clock
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HostileTransition {
    // The hostile explorer is test-only, but keeping the environment variants
    // small avoids multiplying the largest command payload across every
    // generated action. Production cost equations do not include this box.
    Kernel(Box<KernelCommand>),
    WallClock(u64),
    MonotonicClock(MonotonicTick),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct HostileTraceLimits {
    pub(super) depth: usize,
    pub(super) states: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct HostileTraceReport {
    pub(super) unique_states: usize,
    pub(super) transitions: usize,
    pub(super) deepest_trace: usize,
    pub(super) authority_commits: usize,
    pub(super) no_authority_commits: usize,
    pub(super) environment_steps: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum HostileTraceError {
    EmptyUniverse,
    InvalidLimits,
    DuplicateTransaction(HostileTxKey),
    ConflictingRawIdentity(TxId),
    InvalidInitial(String),
    InvalidTransition {
        error: String,
        trace: Vec<HostileAction>,
    },
    NoCommitChangedAuthority(Vec<HostileAction>),
    CommitStampMismatch(Vec<HostileAction>),
    StateBound {
        maximum: usize,
        trace: Vec<HostileAction>,
    },
    CounterBound(Vec<HostileAction>),
}

/// A finite hostile scheduler that is deliberately separate from the M1
/// transition explorer. It derives actions from public model state and never
/// calls the happy-path command generator in `properties.rs`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HostileTraceGenerator {
    transactions: BTreeMap<HostileTxKey, Transaction>,
    peers: BTreeSet<PeerId>,
    limits: HostileTraceLimits,
}

impl HostileTraceGenerator {
    pub(super) fn new(
        transactions: impl IntoIterator<Item = Transaction>,
        peers: impl IntoIterator<Item = PeerId>,
        limits: HostileTraceLimits,
    ) -> Result<Self, HostileTraceError> {
        if limits.depth == 0 || limits.states == 0 {
            return Err(HostileTraceError::InvalidLimits);
        }
        let mut indexed: BTreeMap<HostileTxKey, Transaction> = BTreeMap::new();
        for transaction in transactions {
            let key = HostileTxKey::for_transaction(&transaction);
            if indexed.values().any(|existing| {
                existing.id == transaction.id && !same_raw_transaction(existing, &transaction)
            }) {
                return Err(HostileTraceError::ConflictingRawIdentity(transaction.id));
            }
            if indexed.insert(key, transaction).is_some() {
                return Err(HostileTraceError::DuplicateTransaction(key));
            }
        }
        let transactions = indexed;
        if transactions.is_empty() {
            return Err(HostileTraceError::EmptyUniverse);
        }
        Ok(Self {
            transactions,
            peers: peers.into_iter().collect(),
            limits,
        })
    }

    pub(super) fn transaction_keys(&self) -> BTreeSet<HostileTxKey> {
        self.transactions.keys().copied().collect()
    }

    pub(super) fn explore(&self, initial: Omega) -> Result<HostileTraceReport, HostileTraceError> {
        self.search(initial, |_| false).map(|(report, _)| report)
    }

    pub(super) fn shortest_trace_to(
        &self,
        initial: Omega,
        predicate: impl Fn(&HostileState) -> bool,
    ) -> Result<Option<Vec<HostileAction>>, HostileTraceError> {
        self.search(initial, predicate).map(|(_, trace)| trace)
    }

    fn search(
        &self,
        initial: Omega,
        predicate: impl Fn(&HostileState) -> bool,
    ) -> Result<(HostileTraceReport, Option<Vec<HostileAction>>), HostileTraceError> {
        initial
            .check_invariants()
            .map_err(|error| HostileTraceError::InvalidInitial(format!("{error:?}")))?;
        let initial = HostileState {
            omega: initial,
            wall_clock: 1,
            monotonic_clock: MonotonicTick(1),
        };
        let mut report = HostileTraceReport {
            unique_states: 1,
            ..HostileTraceReport::default()
        };
        if predicate(&initial) {
            return Ok((report, Some(Vec::new())));
        }
        let mut frontier = VecDeque::from([(initial.clone(), Vec::new())]);
        let mut seen = HashSet::from([initial.clone()]);
        while let Some((state, trace)) = frontier.pop_front() {
            report.deepest_trace = report.deepest_trace.max(trace.len());
            if trace.len() == self.limits.depth {
                continue;
            }
            for (action, command) in self.actions(&state) {
                let mut next = state.clone();
                let before_authority = next.omega.authority.clone();
                let before_stamp = before_authority.last_apply;
                let step = match command {
                    HostileTransition::Kernel(command) => Some(next.omega.kernel_step(*command)),
                    HostileTransition::WallClock(to) => {
                        next.wall_clock = to;
                        None
                    }
                    HostileTransition::MonotonicClock(to) if to > next.monotonic_clock => {
                        next.monotonic_clock = to;
                        None
                    }
                    HostileTransition::MonotonicClock(_) => continue,
                };
                let mut next_trace = trace.clone();
                next_trace.push(action);
                report.transitions = report
                    .transitions
                    .checked_add(1)
                    .ok_or_else(|| HostileTraceError::CounterBound(next_trace.clone()))?;
                next.omega.check_invariants().map_err(|error| {
                    HostileTraceError::InvalidTransition {
                        error: format!("{error:?}"),
                        trace: next_trace.clone(),
                    }
                })?;
                match step {
                    None => {
                        report.environment_steps = report
                            .environment_steps
                            .checked_add(1)
                            .ok_or_else(|| HostileTraceError::CounterBound(next_trace.clone()))?;
                        if next.omega.authority != before_authority {
                            return Err(HostileTraceError::NoCommitChangedAuthority(next_trace));
                        }
                    }
                    Some(KernelStep::NoAuthorityCommit(_)) => {
                        report.no_authority_commits = report
                            .no_authority_commits
                            .checked_add(1)
                            .ok_or_else(|| HostileTraceError::CounterBound(next_trace.clone()))?;
                        if next.omega.authority != before_authority {
                            return Err(HostileTraceError::NoCommitChangedAuthority(next_trace));
                        }
                    }
                    Some(KernelStep::AuthorityCommit { stamp, .. }) => {
                        report.authority_commits = report
                            .authority_commits
                            .checked_add(1)
                            .ok_or_else(|| HostileTraceError::CounterBound(next_trace.clone()))?;
                        if before_stamp
                            .0
                            .checked_add(1)
                            .is_none_or(|expected| stamp != ApplyStamp(expected))
                            || next.omega.authority.last_apply != stamp
                        {
                            return Err(HostileTraceError::CommitStampMismatch(next_trace));
                        }
                    }
                }
                if predicate(&next) {
                    return Ok((report, Some(next_trace)));
                }
                if seen.insert(next.clone()) {
                    if seen.len() > self.limits.states {
                        return Err(HostileTraceError::StateBound {
                            maximum: self.limits.states,
                            trace: next_trace,
                        });
                    }
                    report.unique_states = seen.len();
                    frontier.push_back((next, next_trace));
                }
            }
        }
        Ok((report, None))
    }

    fn actions(&self, state: &HostileState) -> Vec<(HostileAction, HostileTransition)> {
        let omega = &state.omega;
        let mut actions = Vec::new();
        {
            let mut push_kernel = |action, command| {
                actions.push((action, HostileTransition::Kernel(Box::new(command))));
            };
            for (transaction_key, transaction) in &self.transactions {
                for peer in &self.peers {
                    push_kernel(
                        HostileAction::AdmitRemote {
                            transaction: *transaction_key,
                            peer: *peer,
                        },
                        KernelCommand::Admit(Admission {
                            transaction: transaction.clone(),
                            source: RetainedSource::Remote(RemoteResidency::new(
                                *peer,
                                RemoteDeadline(100),
                            )),
                            observed_at: state.monotonic_clock,
                        }),
                    );
                }
                push_kernel(
                    HostileAction::AdmitProposal {
                        transaction: *transaction_key,
                    },
                    KernelCommand::Admit(Admission {
                        transaction: transaction.clone(),
                        source: RetainedSource::Proposal,
                        observed_at: state.monotonic_clock,
                    }),
                );
            }
            push_kernel(HostileAction::Checkout, KernelCommand::Checkout);
            // Any and SmallCycleOnly induce the same transition relation when
            // every transaction is Small. Enumerate both only when Large work
            // makes the verifier capability observable, avoiding a duplicate
            // finite-state branch without weakening capability coverage.
            let continuous_capabilities: &[_] = if self
                .transactions
                .values()
                .any(|transaction| transaction.verify_class == VerifyCycleClass::Large)
            {
                &[VerifyCapability::Any, VerifyCapability::SmallCycleOnly]
            } else {
                &[VerifyCapability::Any]
            };
            for capability in continuous_capabilities.iter().copied() {
                push_kernel(
                    HostileAction::CheckoutContinuous(capability),
                    KernelCommand::CheckoutContinuous(capability),
                );
            }
            for capability in omega.linear.work.values() {
                if matches!(capability.stage(), super::state::WorkStage::Resolve)
                    && matches!(
                        capability.permit(),
                        super::state::WorkPermit::ResolveThenVerify(_)
                    )
                    && let Some(owner) = omega.authority.owners.get(&capability.transaction)
                {
                    push_kernel(
                        HostileAction::ContinueCurrent(capability.id),
                        KernelCommand::ContinueResolveThenVerify(ResolveContinuation {
                            capability: capability.id,
                            evidence: ResolvedEvidence::for_transaction(
                                &owner.transaction,
                                omega.authority.chain,
                                omega.authority.rules,
                            ),
                        }),
                    );
                }
                push_kernel(
                    HostileAction::CompleteRejected(capability.id),
                    KernelCommand::Complete(Completion {
                        capability: capability.id,
                        result: WorkResult::Rejected,
                    }),
                );
                push_kernel(
                    HostileAction::FinishRejected(capability.id),
                    KernelCommand::FinishExecution(Completion {
                        capability: capability.id,
                        result: WorkResult::Rejected,
                    }),
                );
                if let Some(result) = self.current_result(omega, capability.id) {
                    push_kernel(
                        HostileAction::CompleteCurrent(capability.id),
                        KernelCommand::Complete(Completion {
                            capability: capability.id,
                            result: result.clone(),
                        }),
                    );
                    push_kernel(
                        HostileAction::FinishCurrent(capability.id),
                        KernelCommand::FinishExecution(Completion {
                            capability: capability.id,
                            result,
                        }),
                    );
                }
                push_kernel(
                    HostileAction::CancelCapability(capability.id),
                    KernelCommand::CancelCapability(capability.id),
                );
            }
            for finished in omega.linear.finished_work.values() {
                push_kernel(
                    HostileAction::SettleFinished(finished.capability.id),
                    KernelCommand::SettleFinished(finished.capability.id),
                );
                push_kernel(
                    HostileAction::CancelCapability(finished.capability.id),
                    KernelCommand::CancelCapability(finished.capability.id),
                );
            }
            push_kernel(
                HostileAction::FinalizeNext,
                KernelCommand::FinalizeNext {
                    wall_time: state.wall_clock,
                },
            );
            for peer in &self.peers {
                push_kernel(
                    HostileAction::BanPeer(*peer),
                    KernelCommand::BanPeer {
                        peer: *peer,
                        observed_at: state.monotonic_clock,
                    },
                );
            }
            push_kernel(
                HostileAction::ExpireRemote,
                KernelCommand::ExpireRemote {
                    wall_time: state.wall_clock,
                    limit: NonZeroU16::new(2).expect("two is statically non-zero"),
                },
            );
            let next_tip = if omega.authority.chain.tip == ViewId(1) {
                ViewId(2)
            } else {
                ViewId(1)
            };
            push_kernel(
                HostileAction::AdvanceChain,
                KernelCommand::ReconcileChain(ChainTransition {
                    from: omega.authority.chain,
                    to_tip: next_tip,
                    committed: BTreeSet::new(),
                    available_cells: BTreeSet::new(),
                    available_headers: BTreeSet::new(),
                    lost_cells: BTreeSet::new(),
                    lost_headers: BTreeSet::new(),
                    conflicting_cells: BTreeSet::new(),
                    recovered: Vec::new(),
                    proposed: BTreeSet::new(),
                    gap: BTreeSet::new(),
                }),
            );
            push_kernel(HostileAction::ClaimEffect, KernelCommand::ClaimEffect);
            if let Some(claim) = omega.linear.effect_claim {
                push_kernel(
                    HostileAction::SettleEffect {
                        source: claim.source,
                        stamp: claim.stamp,
                        ordinal: claim.ordinal,
                    },
                    KernelCommand::SettleEffect(claim),
                );
            }
        }

        for to in self.wall_clock_candidates(state) {
            actions.push((
                HostileAction::AdvanceWallClock { to },
                HostileTransition::WallClock(to),
            ));
        }
        for to in self.monotonic_clock_candidates(state) {
            actions.push((
                HostileAction::AdvanceMonotonic { to },
                HostileTransition::MonotonicClock(to),
            ));
        }
        actions
    }

    fn wall_clock_candidates(&self, state: &HostileState) -> BTreeSet<u64> {
        let mut boundaries = BTreeSet::new();
        if state.wall_clock != 0 {
            boundaries.insert(0);
        }
        let mut next_deadline = None;
        for owner in state.omega.authority.owners.values() {
            let OwnerLocation::Retained(retained) = &owner.location else {
                continue;
            };
            if let Some(deadline) = retained.source.active_remote_deadline()
                && deadline.0 > state.wall_clock
            {
                next_deadline =
                    Some(next_deadline.map_or(deadline.0, |current: u64| current.min(deadline.0)));
            }
        }
        boundaries.extend(next_deadline);
        boundaries
    }

    fn monotonic_clock_candidates(&self, state: &HostileState) -> BTreeSet<MonotonicTick> {
        let mut candidates = BTreeSet::new();
        if let Some(next) = state.monotonic_clock.0.checked_add(1) {
            candidates.insert(MonotonicTick(next));
        }
        let mut next_deadline = None;
        for ban in state.omega.authority.peer_bans.values() {
            if let PeerBanDeadline::At(deadline) = ban.deadline
                && deadline > state.monotonic_clock
            {
                next_deadline = Some(
                    next_deadline.map_or(deadline, |current: MonotonicTick| current.min(deadline)),
                );
            }
        }
        candidates.extend(next_deadline);
        candidates
    }

    fn current_result(&self, state: &Omega, capability: CapabilityId) -> Option<WorkResult> {
        let capability = state.linear.work.get(&capability)?;
        let owner = state.authority.owners.get(&capability.transaction)?;
        Some(match capability.kind() {
            WorkKind::Resolve => WorkResult::Resolved(ResolvedEvidence::for_transaction(
                &owner.transaction,
                state.authority.chain,
                state.authority.rules,
            )),
            WorkKind::Verify => WorkResult::Verified,
        })
    }
}

fn same_raw_transaction(left: &Transaction, right: &Transaction) -> bool {
    left.id == right.id
        && left.proposal == right.proposal
        && left.inputs == right.inputs
        && left.deps == right.deps
        && left.header_deps == right.header_deps
        && left.outputs == right.outputs
        && left.bytes == right.bytes
        && left.fee == right.fee
}

fn build_width(
    width: u8,
    mut build: impl FnMut(u8) -> Result<Transaction, ScenarioError>,
) -> Result<Vec<Transaction>, ScenarioError> {
    if width == 0 {
        return Err(ScenarioError::Empty);
    }
    (1..=width).map(&mut build).collect()
}

fn checked_id(base: u8, offset: u8) -> Result<u8, ScenarioError> {
    base.checked_add(offset)
        .ok_or(ScenarioError::IdentifierBound)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct QuantitativeLimits {
    pub(super) mutation_batch: u32,
    pub(super) worker_slots: u32,
    pub(super) external_records: u32,
    pub(super) external_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct QuantitativeInput {
    pub(super) ingress_items: u32,
    pub(super) completions: u32,
    pub(super) grants: u32,
    pub(super) ready_items: u32,
    pub(super) accepted_owners_scanned: u32,
    pub(super) accepted_edges_scanned: u32,
    pub(super) cell_keys: u32,
    pub(super) header_keys: u32,
    pub(super) pool_edges: u32,
    pub(super) index_operations: u32,
    pub(super) candidate_scratch_entries: u32,
    pub(super) coupled_members: u32,
    pub(super) coupled_edges: u32,
    pub(super) wake_edges: u32,
    pub(super) stale_capabilities: u32,
    /// Logical endpoint records retained by immutable effect batches.
    pub(super) effect_records: u32,
    /// Resident effect batches. Production settles one complete batch with
    /// one authority Apply even when that batch contains several records.
    pub(super) effect_batches: u32,
    pub(super) effect_bytes: u64,
    pub(super) relay_records: u32,
    pub(super) relay_bytes: u64,
    pub(super) detached_endpoint_calls: u32,
    pub(super) detached_endpoint_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct QuantitativeBound {
    pub(super) transient_items: u64,
    pub(super) key_edge_operations: u64,
    pub(super) component_operations: u64,
    pub(super) scratch_entries: u64,
    pub(super) linear_drain_steps: u32,
    pub(super) completion_order_items: u32,
    pub(super) completion_pair_space: u64,
    pub(super) core_authority_applies: u32,
    pub(super) effect_settlement_applies: u32,
    pub(super) authority_apply_upper: u32,
    pub(super) wake_operations: u32,
    pub(super) stale_retirements: u32,
    pub(super) external_backlog_records: u64,
    pub(super) external_backlog_bytes: u64,
}

impl QuantitativeInput {
    pub(super) fn with_ready_composition(mut self, cost: CompositionCost) -> Self {
        self.ready_items = cost.candidates;
        self.accepted_owners_scanned = cost.accepted_owners_scanned;
        self.accepted_edges_scanned = cost.accepted_edges_scanned;
        self.cell_keys = cost.cell_keys;
        self.header_keys = cost.header_keys;
        self.pool_edges = cost.pool_edges;
        self.index_operations = cost.index_operations;
        self.candidate_scratch_entries = cost.scratch_entries;
        self
    }

    pub(super) fn compile(self, limits: QuantitativeLimits) -> Option<QuantitativeBound> {
        if limits.mutation_batch == 0
            || limits.worker_slots == 0
            || self.ingress_items > limits.mutation_batch
            || self.completions > limits.worker_slots
            || self.grants > limits.worker_slots
            || self.ready_items > limits.mutation_batch
            || self.stale_capabilities > limits.worker_slots
            || (self.effect_records == 0) != (self.effect_batches == 0)
            || self.effect_batches > self.effect_records
        {
            return None;
        }
        let external_backlog_records = u64::from(self.effect_records)
            .checked_add(u64::from(self.relay_records))?
            .checked_add(u64::from(self.detached_endpoint_calls))?;
        let external_backlog_bytes = self
            .effect_bytes
            .checked_add(self.relay_bytes)?
            .checked_add(self.detached_endpoint_bytes)?;
        if external_backlog_records > u64::from(limits.external_records)
            || external_backlog_bytes > limits.external_bytes
        {
            return None;
        }
        let transient_items = [
            self.ingress_items,
            self.completions,
            self.grants,
            self.ready_items,
        ]
        .into_iter()
        .try_fold(0u64, |sum, value| sum.checked_add(u64::from(value)))?;
        let key_edge_operations = [
            self.accepted_edges_scanned,
            self.cell_keys,
            self.header_keys,
            self.pool_edges,
            self.index_operations,
        ]
        .into_iter()
        .try_fold(0u64, |sum, value| sum.checked_add(u64::from(value)))?;
        let component_operations =
            u64::from(self.coupled_members).checked_add(u64::from(self.coupled_edges))?;
        let scratch_entries = u64::from(self.accepted_owners_scanned)
            .checked_add(u64::from(self.accepted_edges_scanned))?
            .checked_add(u64::from(self.candidate_scratch_entries))?
            .checked_add(component_operations)?
            .checked_add(transient_items)?;
        let linear_drain_steps = self.completions;
        let completion_order_items = self.completions;
        let completion_predecessors = match self.completions {
            0 => 0,
            completions => completions - 1,
        };
        let completion_pair_space =
            u64::from(self.completions).checked_mul(u64::from(completion_predecessors))? / 2;
        let core_authority_applies = [
            self.ingress_items != 0,
            self.completions != 0 || self.grants != 0,
            self.ready_items != 0,
        ]
        .into_iter()
        .map(u32::from)
        .try_fold(0u32, u32::checked_add)?;
        let effect_settlement_applies = self.effect_batches;
        let authority_apply_upper =
            core_authority_applies.checked_add(effect_settlement_applies)?;
        Some(QuantitativeBound {
            transient_items,
            key_edge_operations,
            component_operations,
            scratch_entries,
            linear_drain_steps,
            completion_order_items,
            completion_pair_space,
            core_authority_applies,
            effect_settlement_applies,
            authority_apply_upper,
            wake_operations: self.wake_edges,
            stale_retirements: self.stale_capabilities,
            external_backlog_records,
            external_backlog_bytes,
        })
    }
}

/// Static authority-Apply cost for the current ordinary retained path.
///
/// Scope is deliberately narrow: homogeneous chain-backed independent owners,
/// continuous Resolve-to-Verify execution, no stale work, no pressure and one
/// non-empty acceptance-effect batch per Ready Apply. The equation describes
/// current topology; it is neither a semantic lower bound nor a timing model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CurrentRetainedPathInput {
    pub(super) items: u32,
    pub(super) ready_applies: u32,
    pub(super) ready_batch_limit: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CurrentRetainedPathCost {
    pub(super) admission_applies: u32,
    pub(super) checkout_applies: u32,
    pub(super) completion_applies: u32,
    pub(super) membership_applies: u32,
    pub(super) effect_settlement_applies: u32,
    pub(super) total_applies: u32,
}

impl CurrentRetainedPathInput {
    pub(super) fn compile(self) -> Option<CurrentRetainedPathCost> {
        if self.ready_batch_limit == 0 {
            return None;
        }
        if self.items == 0 {
            return (self.ready_applies == 0).then_some(CurrentRetainedPathCost {
                admission_applies: 0,
                checkout_applies: 0,
                completion_applies: 0,
                membership_applies: 0,
                effect_settlement_applies: 0,
                total_applies: 0,
            });
        }
        let minimum_ready_applies = self.items.div_ceil(self.ready_batch_limit);
        if !(minimum_ready_applies..=self.items).contains(&self.ready_applies) {
            return None;
        }
        let per_item_applies = self.items.checked_mul(3)?;
        let batched_applies = self.ready_applies.checked_mul(2)?;
        Some(CurrentRetainedPathCost {
            admission_applies: self.items,
            checkout_applies: self.items,
            completion_applies: self.items,
            membership_applies: self.ready_applies,
            effect_settlement_applies: self.ready_applies,
            total_applies: per_item_applies.checked_add(batched_applies)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct WorkAttemptKey {
    transaction: TxId,
    chain: ChainView,
    rules: RulesId,
    witness: WitnessId,
    kind: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkRecordDisposition {
    Recorded,
    DuplicateCut,
    TransactionBound,
    EvidenceCutBound,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct WorkAmplificationAudit {
    max_transactions: u16,
    max_attempts_per_transaction: u16,
    attempts: BTreeSet<WorkAttemptKey>,
    transactions: BTreeMap<TxId, u16>,
}

impl WorkAmplificationAudit {
    pub(super) fn new(max_transactions: u16, max_attempts_per_transaction: u16) -> Option<Self> {
        (max_transactions != 0 && max_attempts_per_transaction != 0).then_some(Self {
            max_transactions,
            max_attempts_per_transaction,
            attempts: BTreeSet::new(),
            transactions: BTreeMap::new(),
        })
    }

    pub(super) fn record(
        &mut self,
        transaction: TxId,
        context: EvidenceContext,
        kind: WorkKind,
    ) -> WorkRecordDisposition {
        let key = WorkAttemptKey {
            transaction,
            chain: context.chain,
            rules: context.rules,
            witness: context.witness,
            kind: match kind {
                WorkKind::Resolve => 0,
                WorkKind::Verify => 1,
            },
        };
        if self.attempts.contains(&key) {
            return WorkRecordDisposition::DuplicateCut;
        }
        let known_transaction = self.transactions.contains_key(&transaction);
        if !known_transaction
            && u16::try_from(self.transactions.len())
                .ok()
                .is_none_or(|count| count >= self.max_transactions)
        {
            return WorkRecordDisposition::TransactionBound;
        }
        let cuts = self.transactions.get(&transaction).copied().unwrap_or(0);
        if cuts >= self.max_attempts_per_transaction {
            return WorkRecordDisposition::EvidenceCutBound;
        }
        self.attempts.insert(key);
        self.transactions.insert(transaction, cuts + 1);
        WorkRecordDisposition::Recorded
    }

    pub(super) fn total_attempts(&self) -> usize {
        self.attempts.len()
    }
}

pub(super) fn bounded_permutations<T: Clone>(items: &[T], maximum: usize) -> Option<Vec<Vec<T>>> {
    if items.len() > maximum {
        return None;
    }
    fn visit<T: Clone>(remaining: &mut Vec<T>, prefix: &mut Vec<T>, output: &mut Vec<Vec<T>>) {
        if remaining.is_empty() {
            output.push(prefix.clone());
            return;
        }
        for index in 0..remaining.len() {
            let item = remaining.remove(index);
            prefix.push(item);
            visit(remaining, prefix, output);
            let item = prefix.pop();
            if let Some(item) = item {
                remaining.insert(index, item);
            }
        }
    }
    let mut output = Vec::new();
    visit(&mut items.to_vec(), &mut Vec::new(), &mut output);
    Some(output)
}

pub(super) fn expanded_key_count(transactions: &[Transaction]) -> Option<u32> {
    transactions.iter().try_fold(0u32, |count, transaction| {
        let keys = transaction
            .inputs
            .len()
            .checked_add(transaction.outputs.len())?
            .checked_add(transaction.deps.len())?
            .checked_add(transaction.header_deps.len())?;
        count.checked_add(u32::try_from(keys).ok()?)
    })
}

pub(super) fn canonical_proposals(transactions: &[Transaction]) -> BTreeSet<ProposalId> {
    transactions
        .iter()
        .map(|transaction| transaction.proposal)
        .collect()
}

pub(super) fn canonical_headers(transactions: &[Transaction]) -> BTreeSet<HeaderId> {
    transactions
        .iter()
        .flat_map(|transaction| transaction.header_deps.iter().copied())
        .collect()
}
