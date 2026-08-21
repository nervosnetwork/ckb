use super::{
    dependency_progress::{ModelDependencyCut, ModelDependencyKey},
    evidence_transition::{
        ModelDependencyLevel, ModelEvidenceFrontier, ModelEvidenceIdentity, ModelEvidenceView,
        ModelKnownDependencies, ModelRawTransaction, ModelUnindexedDependencyLevel,
    },
    kernel::{
        Admission, ChainTransition, Completion, KernelCommand, KernelDisposition, KernelStep,
        WorkResult,
    },
    proposal::ProposalContext,
    settlement_transition::{
        ModelMissingSettlement, ModelPayloadPolicy, ModelPayloadPolicyEvolution,
        ModelSettlementCut, ModelSettlementEvidence, ModelSettlementFault, ModelSettlementNext,
        ModelSettlementObservation, ModelSettlementOrigin, ModelSettlementRejection,
    },
    state::{
        CellId, ModelLimits, MonotonicTick, Omega, OwnerLocation, PeerId, RemoteDeadline,
        RemoteResidency, ResolvedEvidence, RetainedOwner, RetainedPhase, RetainedSource, RulesId,
        Transaction, ViewId, WorkCapability, WorkPayloadPolicy, WorkStage,
    },
};
use std::collections::BTreeSet;

fn dependencies(values: &[u8]) -> ModelKnownDependencies {
    values
        .iter()
        .copied()
        .map(ModelDependencyKey::cell)
        .collect()
}

fn frontier(loss: Option<u16>) -> ModelEvidenceFrontier {
    let last_change = loss.unwrap_or(1);
    ModelEvidenceFrontier::new(
        [(
            ModelDependencyKey::cell(1),
            ModelDependencyLevel::new(
                ModelDependencyCut(last_change),
                loss.map(ModelDependencyCut),
            )
            .expect("the loss is bounded by the level"),
        )],
        ModelUnindexedDependencyLevel::new(Some(ModelDependencyCut(last_change)), None)
            .expect("the unindexed level is legal"),
    )
    .expect("the dependency level key is unique")
}

fn settlement_cut() -> ModelSettlementCut {
    ModelSettlementCut {
        authority_view: ModelEvidenceView(1),
        owner_identity: ModelEvidenceIdentity {
            raw: ModelRawTransaction(1),
            witness: 2,
        },
        baseline_dependencies: dependencies(&[1]),
        current_policy: ModelPayloadPolicy::Trusted,
        active: ModelSettlementOrigin {
            payload_identity: ModelEvidenceIdentity {
                raw: ModelRawTransaction(1),
                witness: 2,
            },
            view: ModelEvidenceView(1),
            dependency_cut: ModelDependencyCut(1),
            payload_policy: ModelPayloadPolicy::Trusted,
        },
        frontier: frontier(None),
    }
}

fn evidence() -> ModelSettlementEvidence {
    let cut = settlement_cut();
    cut.active.evidence(cut.baseline_dependencies)
}

#[test]
fn model_checkout_origin_is_the_exact_settlement_evidence_equivalence_premise() {
    for raw in [1, 2] {
        for witness in [3, 4] {
            for view in [ModelEvidenceView(1), ModelEvidenceView(2)] {
                for dependency_cut in [ModelDependencyCut(1), ModelDependencyCut(2)] {
                    for payload_policy in [
                        ModelPayloadPolicy::RemoteDeclaredCycles(5),
                        ModelPayloadPolicy::Trusted,
                    ] {
                        let origin = ModelSettlementOrigin {
                            payload_identity: ModelEvidenceIdentity {
                                raw: ModelRawTransaction(raw),
                                witness,
                            },
                            view,
                            dependency_cut,
                            payload_policy,
                        };
                        let dependencies = dependencies(&[1]);
                        let evidence = origin.evidence(dependencies.clone());
                        assert_eq!(evidence.payload_identity, origin.payload_identity);
                        assert_eq!(evidence.view, origin.view);
                        assert_eq!(evidence.dependency_cut, origin.dependency_cut);
                        assert_eq!(evidence.dependencies, dependencies);
                    }
                }
            }
        }
    }
}

#[test]
fn model_queued_verify_requires_exact_identity_view_cut_chain_and_dependencies() {
    let cut = settlement_cut();
    let resolved = evidence();
    assert_eq!(
        cut.classify(&ModelSettlementNext::QueuedVerify(resolved.clone())),
        ModelSettlementObservation::QueuedVerify
    );

    let mut changed = resolved.clone();
    changed.payload_identity.raw = ModelRawTransaction(2);
    assert_eq!(
        cut.classify(&ModelSettlementNext::QueuedVerify(changed)),
        ModelSettlementObservation::Fault(ModelSettlementFault::MembershipProjection)
    );
    changed = resolved.clone();
    changed.view = ModelEvidenceView(2);
    assert_eq!(
        cut.classify(&ModelSettlementNext::QueuedVerify(changed)),
        ModelSettlementObservation::Fault(ModelSettlementFault::MembershipProjection)
    );
    changed = resolved.clone();
    changed.dependency_cut = ModelDependencyCut(2);
    assert_eq!(
        cut.classify(&ModelSettlementNext::QueuedVerify(changed)),
        ModelSettlementObservation::Fault(ModelSettlementFault::DependencyProjection)
    );

    assert_eq!(
        ModelSettlementCut {
            authority_view: ModelEvidenceView(2),
            ..cut.clone()
        }
        .classify(&ModelSettlementNext::QueuedVerify(resolved.clone())),
        ModelSettlementObservation::QueuedResolve
    );
    assert_eq!(
        ModelSettlementCut {
            frontier: frontier(Some(2)),
            ..cut
        }
        .classify(&ModelSettlementNext::QueuedVerify(resolved)),
        ModelSettlementObservation::QueuedResolve
    );
}

#[test]
fn model_waiting_is_retained_only_at_the_same_chain_and_current_missing_cut() {
    let cut = settlement_cut();
    let waiting = ModelSettlementNext::Waiting(ModelMissingSettlement {
        dependencies: dependencies(&[1]),
        missing: dependencies(&[1]),
    });
    assert_eq!(cut.classify(&waiting), ModelSettlementObservation::Waiting);
    assert_eq!(
        ModelSettlementCut {
            authority_view: ModelEvidenceView(2),
            ..cut.clone()
        }
        .classify(&waiting),
        ModelSettlementObservation::QueuedResolve
    );
    assert_eq!(
        ModelSettlementCut {
            frontier: frontier(Some(2)),
            ..cut
        }
        .classify(&waiting),
        ModelSettlementObservation::QueuedResolve
    );
}

#[test]
fn model_ready_requires_exact_payload_identity_from_checkout_origin() {
    let cut = settlement_cut();
    let verified = evidence();
    assert_eq!(
        cut.classify(&ModelSettlementNext::Ready(verified.clone())),
        ModelSettlementObservation::Ready
    );
    let mut changed = verified.clone();
    changed.payload_identity.raw = ModelRawTransaction(2);
    assert_eq!(
        cut.classify(&ModelSettlementNext::Ready(changed)),
        ModelSettlementObservation::Fault(ModelSettlementFault::MembershipProjection)
    );
    changed = verified;
    changed.payload_identity.witness = 3;
    assert_eq!(
        cut.classify(&ModelSettlementNext::Ready(changed)),
        ModelSettlementObservation::Fault(ModelSettlementFault::MembershipProjection)
    );
}

#[test]
fn model_rejection_validity_is_chain_or_resource_bound() {
    for chain_current in [false, true] {
        for rejection in [
            ModelSettlementRejection::ChainBound,
            ModelSettlementRejection::ResourceBound,
        ] {
            let cut = ModelSettlementCut {
                authority_view: ModelEvidenceView(if chain_current { 1 } else { 2 }),
                ..settlement_cut()
            };
            assert_eq!(
                cut.classify(&ModelSettlementNext::Rejected(rejection)),
                if chain_current || rejection == ModelSettlementRejection::ResourceBound {
                    ModelSettlementObservation::Rejected
                } else {
                    ModelSettlementObservation::QueuedResolve
                }
            );
        }
    }
}

#[test]
fn model_every_settlement_result_requires_the_active_baseline_to_remain_current() {
    let stale = ModelSettlementCut {
        frontier: frontier(Some(2)),
        ..settlement_cut()
    };
    let evidence = evidence();
    let outcomes = [
        ModelSettlementNext::QueuedVerify(evidence.clone()),
        ModelSettlementNext::Waiting(ModelMissingSettlement {
            dependencies: dependencies(&[1]),
            missing: dependencies(&[1]),
        }),
        ModelSettlementNext::Ready(evidence.clone()),
        ModelSettlementNext::Rejected(ModelSettlementRejection::ChainBound),
        ModelSettlementNext::Rejected(ModelSettlementRejection::ResourceBound),
        ModelSettlementNext::VerificationRejected(evidence),
        ModelSettlementNext::Retry,
    ];
    for outcome in outcomes {
        assert_eq!(
            stale.classify(&outcome),
            ModelSettlementObservation::QueuedResolve,
            "a completion cannot retain facts after its baseline dependency loss: {outcome:?}"
        );
    }
}

#[test]
fn model_verification_rejection_policy_transition_is_a_closed_truth_table() {
    let resolved = evidence();
    for active_policy in [
        ModelPayloadPolicy::RemoteDeclaredCycles(1),
        ModelPayloadPolicy::RemoteDeclaredCycles(2),
        ModelPayloadPolicy::Trusted,
    ] {
        for current_policy in [
            ModelPayloadPolicy::RemoteDeclaredCycles(1),
            ModelPayloadPolicy::RemoteDeclaredCycles(2),
            ModelPayloadPolicy::Trusted,
        ] {
            for chain_current in [false, true] {
                let baseline = settlement_cut();
                let cut = ModelSettlementCut {
                    authority_view: ModelEvidenceView(if chain_current { 1 } else { 2 }),
                    current_policy,
                    active: ModelSettlementOrigin {
                        payload_policy: active_policy,
                        ..baseline.active.clone()
                    },
                    ..baseline
                };
                let expected = match active_policy.evolution_to(current_policy) {
                    ModelPayloadPolicyEvolution::Unchanged => {
                        if chain_current {
                            ModelSettlementObservation::Rejected
                        } else {
                            ModelSettlementObservation::QueuedResolve
                        }
                    }
                    ModelPayloadPolicyEvolution::RemoteToTrusted => {
                        if chain_current {
                            ModelSettlementObservation::QueuedVerify
                        } else {
                            ModelSettlementObservation::QueuedResolve
                        }
                    }
                    ModelPayloadPolicyEvolution::Invalid => ModelSettlementObservation::Fault(
                        ModelSettlementFault::MembershipProjection,
                    ),
                };
                assert_eq!(
                    cut.classify(&ModelSettlementNext::VerificationRejected(resolved.clone())),
                    expected,
                    "active={active_policy:?}, current={current_policy:?}, chain={chain_current}"
                );
            }
        }
    }
    assert_eq!(
        settlement_cut().classify(&ModelSettlementNext::Retry),
        ModelSettlementObservation::QueuedResolve
    );
}

#[derive(Clone, Copy, Debug)]
enum KernelSettlementKind {
    QueuedVerify,
    Waiting,
    Ready,
    ChainRejected,
    ResourceRejected,
    VerificationRejected,
    Retry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KernelSettlementScenario {
    Current,
    StaleChain,
    TrustedPromotion,
}

fn kernel_model() -> Omega {
    Omega::new(
        ModelLimits::small()
            .validate()
            .expect("the settlement quotient uses valid bounds"),
        ViewId(1),
        RulesId(1),
    )
}

fn kernel_checkout(step: KernelStep) -> WorkCapability {
    match step {
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::CheckedOut(capability),
            ..
        } => capability,
        other => panic!("expected checkout, got {other:?}"),
    }
}

fn kernel_settlement_case(
    kind: KernelSettlementKind,
    scenario: KernelSettlementScenario,
) -> (ModelSettlementObservation, ModelSettlementObservation) {
    let transaction = Transaction::independent(1, 2, 10, 20);
    let mut omega = kernel_model();
    assert!(matches!(
        omega.kernel_step(KernelCommand::Admit(Admission {
            transaction: transaction.clone(),
            source: RetainedSource::Remote(RemoteResidency::new(
                PeerId(7),
                RemoteDeadline(u64::MAX),
            )),
            observed_at: MonotonicTick(0),
        })),
        KernelStep::AuthorityCommit { .. }
    ));
    let resolved = ResolvedEvidence::for_transaction(
        &transaction,
        omega.authority.chain,
        omega.authority.rules,
    )
    .expect("direct transaction has no dep-group expansion");
    let capability = match kind {
        KernelSettlementKind::Ready | KernelSettlementKind::VerificationRejected => {
            let resolve = kernel_checkout(omega.kernel_step(KernelCommand::Checkout));
            assert_eq!(
                omega
                    .kernel_step(KernelCommand::Complete(Completion {
                        capability: resolve.id,
                        result: WorkResult::Resolved(resolved.clone()),
                    }))
                    .disposition(),
                &KernelDisposition::Continued(transaction.id)
            );
            kernel_checkout(omega.kernel_step(KernelCommand::Checkout))
        }
        KernelSettlementKind::QueuedVerify
        | KernelSettlementKind::Waiting
        | KernelSettlementKind::ChainRejected
        | KernelSettlementKind::ResourceRejected
        | KernelSettlementKind::Retry => {
            kernel_checkout(omega.kernel_step(KernelCommand::Checkout))
        }
    };
    assert_eq!(capability.payload_policy(), WorkPayloadPolicy::Remote);
    if matches!(
        kind,
        KernelSettlementKind::Ready | KernelSettlementKind::VerificationRejected
    ) {
        let WorkStage::Verify(evidence) = capability.stage() else {
            panic!("a verification outcome requires the checked-out Verify evidence");
        };
        assert!(evidence.is_for(&transaction, capability.chain, capability.rules));
    }

    match scenario {
        KernelSettlementScenario::Current => {}
        KernelSettlementScenario::StaleChain => {
            let from = omega.authority.chain;
            assert!(matches!(
                omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
                    context: super::kernel::ChainContextTransition::from_primitives(
                        super::state::RulesId(1),
                        false,
                    ),
                    from,
                    to_tip: ViewId(2),
                    committed: BTreeSet::new(),
                    available_cells: BTreeSet::new(),
                    available_headers: BTreeSet::new(),
                    lost_cells: BTreeSet::new(),
                    lost_headers: BTreeSet::new(),
                    conflicting_cells: BTreeSet::new(),
                    recovered: Vec::new(),
                    proposals: ProposalContext::empty().view(),
                })),
                KernelStep::AuthorityCommit { .. }
            ));
        }
        KernelSettlementScenario::TrustedPromotion => {
            assert_eq!(
                omega
                    .kernel_step(KernelCommand::Admit(Admission {
                        transaction: transaction.clone(),
                        source: RetainedSource::Proposal,
                        observed_at: MonotonicTick(0),
                    }))
                    .disposition(),
                &KernelDisposition::Promoted(transaction.id)
            );
        }
    }

    let result = match kind {
        KernelSettlementKind::QueuedVerify => WorkResult::Resolved(resolved),
        KernelSettlementKind::Waiting => WorkResult::Missing(
            super::state::MissingDependencies::for_transaction(
                &transaction,
                BTreeSet::from([CellId(10)]),
            )
            .expect("the transaction declares the missing cell"),
        ),
        KernelSettlementKind::Ready => WorkResult::Verified,
        KernelSettlementKind::ChainRejected => WorkResult::resolve_rejected(),
        KernelSettlementKind::ResourceRejected => WorkResult::resource_rejected(),
        KernelSettlementKind::VerificationRejected => WorkResult::VerificationRejected,
        KernelSettlementKind::Retry => WorkResult::Retry,
    };
    let step = omega.kernel_step(KernelCommand::Complete(Completion {
        capability: capability.id,
        result,
    }));
    let kernel = match step.disposition() {
        KernelDisposition::Continued(id) if *id == transaction.id => {
            match &omega.authority.owners[id].location {
                OwnerLocation::Retained(RetainedOwner {
                    phase: RetainedPhase::Queued(WorkStage::Verify(_)),
                    ..
                }) => ModelSettlementObservation::QueuedVerify,
                OwnerLocation::Retained(RetainedOwner {
                    phase: RetainedPhase::Queued(WorkStage::Resolve),
                    ..
                }) => ModelSettlementObservation::QueuedResolve,
                other => panic!("continued into an unmodeled phase: {other:?}"),
            }
        }
        KernelDisposition::Waiting(id) if *id == transaction.id => {
            ModelSettlementObservation::Waiting
        }
        KernelDisposition::Ready(id) if *id == transaction.id => ModelSettlementObservation::Ready,
        KernelDisposition::Rejected(id) if *id == transaction.id => {
            ModelSettlementObservation::Rejected
        }
        other => panic!("unexpected kernel settlement observation: {other:?}"),
    };

    let mut cut = settlement_cut();
    cut.active.payload_policy = ModelPayloadPolicy::RemoteDeclaredCycles(9);
    cut.current_policy = if scenario == KernelSettlementScenario::TrustedPromotion {
        ModelPayloadPolicy::Trusted
    } else {
        ModelPayloadPolicy::RemoteDeclaredCycles(9)
    };
    if scenario == KernelSettlementScenario::StaleChain {
        cut.authority_view = ModelEvidenceView(2);
    }
    let model_evidence = cut.active.evidence(cut.baseline_dependencies.clone());
    let next = match kind {
        KernelSettlementKind::QueuedVerify => ModelSettlementNext::QueuedVerify(model_evidence),
        KernelSettlementKind::Waiting => ModelSettlementNext::Waiting(ModelMissingSettlement {
            dependencies: cut.baseline_dependencies.clone(),
            missing: cut.baseline_dependencies.clone(),
        }),
        KernelSettlementKind::Ready => ModelSettlementNext::Ready(model_evidence),
        KernelSettlementKind::ChainRejected => {
            ModelSettlementNext::Rejected(ModelSettlementRejection::ChainBound)
        }
        KernelSettlementKind::ResourceRejected => {
            ModelSettlementNext::Rejected(ModelSettlementRejection::ResourceBound)
        }
        KernelSettlementKind::VerificationRejected => {
            ModelSettlementNext::VerificationRejected(model_evidence)
        }
        KernelSettlementKind::Retry => ModelSettlementNext::Retry,
    };
    (kernel, cut.classify(&next))
}

#[test]
fn model_kernel_completion_with_current_baseline_is_the_exact_settlement_quotient() {
    for kind in [
        KernelSettlementKind::QueuedVerify,
        KernelSettlementKind::Waiting,
        KernelSettlementKind::Ready,
        KernelSettlementKind::ChainRejected,
        KernelSettlementKind::ResourceRejected,
        KernelSettlementKind::VerificationRejected,
        KernelSettlementKind::Retry,
    ] {
        for scenario in [
            KernelSettlementScenario::Current,
            KernelSettlementScenario::StaleChain,
        ] {
            let (kernel, exact) = kernel_settlement_case(kind, scenario);
            assert_eq!(kernel, exact, "kind={kind:?}, scenario={scenario:?}");
        }
    }
    let (kernel, exact) = kernel_settlement_case(
        KernelSettlementKind::VerificationRejected,
        KernelSettlementScenario::TrustedPromotion,
    );
    assert_eq!(kernel, exact, "trusted promotion reuses exact resolution");
}
