use super::{
    dependency_progress::{ModelDependencyCut, ModelDependencyKey},
    evidence_transition::{
        ModelDependencyLevel, ModelEvidenceFrontier, ModelEvidenceIdentity, ModelEvidenceView,
        ModelKnownDependencies, ModelRawTransaction, ModelUnindexedDependencyLevel,
    },
    settlement_transition::{
        ModelMissingSettlement, ModelPayloadPolicy, ModelSettlementCut, ModelSettlementEvidence,
        ModelSettlementFault, ModelSettlementNext, ModelSettlementObservation,
        ModelSettlementRejection,
    },
};

fn dependencies(values: &[u8]) -> ModelKnownDependencies {
    values.iter().copied().map(ModelDependencyKey).collect()
}

fn frontier(loss: Option<u16>) -> ModelEvidenceFrontier {
    let last_change = loss.unwrap_or(1);
    ModelEvidenceFrontier::new(
        [(
            ModelDependencyKey(1),
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
        active_view: ModelEvidenceView(1),
        active_dependency_cut: ModelDependencyCut(1),
        active_policy: ModelPayloadPolicy::Trusted,
        frontier: frontier(None),
    }
}

fn evidence() -> ModelSettlementEvidence {
    let cut = settlement_cut();
    ModelSettlementEvidence {
        payload_identity: cut.owner_identity,
        sealed_witness: cut.owner_identity.witness,
        view: cut.active_view,
        dependency_cut: cut.active_dependency_cut,
        dependencies: cut.baseline_dependencies,
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
fn model_ready_requires_both_payload_identity_and_sealed_witness() {
    let cut = settlement_cut();
    let verified = evidence();
    assert_eq!(
        cut.classify(&ModelSettlementNext::Ready(verified.clone())),
        ModelSettlementObservation::Ready
    );
    let mut changed = verified.clone();
    changed.sealed_witness = 3;
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
        ModelPayloadPolicy::RemoteDeclaredCycles,
        ModelPayloadPolicy::Trusted,
    ] {
        for current_policy in [
            ModelPayloadPolicy::RemoteDeclaredCycles,
            ModelPayloadPolicy::Trusted,
        ] {
            for chain_current in [false, true] {
                let cut = ModelSettlementCut {
                    authority_view: ModelEvidenceView(if chain_current { 1 } else { 2 }),
                    active_policy,
                    current_policy,
                    ..settlement_cut()
                };
                let expected = if current_policy == active_policy {
                    if chain_current {
                        ModelSettlementObservation::Rejected
                    } else {
                        ModelSettlementObservation::QueuedResolve
                    }
                } else if active_policy == ModelPayloadPolicy::RemoteDeclaredCycles
                    && current_policy == ModelPayloadPolicy::Trusted
                {
                    if chain_current {
                        ModelSettlementObservation::QueuedVerify
                    } else {
                        ModelSettlementObservation::QueuedResolve
                    }
                } else {
                    ModelSettlementObservation::Fault(ModelSettlementFault::MembershipProjection)
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
