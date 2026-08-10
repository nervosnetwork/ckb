use super::{
    dependency_progress::{ModelDependencyCut, ModelDependencyKey},
    evidence_transition::{
        ModelAcceptedPoolOutput, ModelAdmissionReceipt, ModelCellLocation, ModelDependencyLevel,
        ModelDependencyMaintenanceAction, ModelDependencyMaintenanceError,
        ModelDependencyMaintenanceLocation, ModelDependencyMaintenanceOwner,
        ModelDependencyMaintenanceScope, ModelDependencyMaintenanceTicket,
        ModelDirectRejectionObservation, ModelDirectRejectionValidity, ModelEvidenceFrontier,
        ModelEvidenceIdentity, ModelEvidenceProof, ModelEvidenceValidation, ModelEvidenceView,
        ModelFinalAdmissionSubject, ModelKnownDependencies, ModelMissingDisposition,
        ModelMissingFact, ModelPoolParent, ModelPreAcceptedMaintenancePhase,
        ModelPreAcceptedSource, ModelRawTransaction, ModelReadyOwner, ModelReadyPayloadRelation,
        ModelReleasedInputContext, ModelReleasedInputCut, ModelReleasedInputDisposition,
        ModelReplacementReference, ModelSubjectValidation, ModelUnindexedDependencyLevel,
        dependency_maintenance_action, missing_resolution_disposition, released_input_disposition,
        replacement_history_trigger, validate_direct_acceptance, validate_direct_rejection,
        validate_final_acceptance, validate_final_subject, validated_location_transition,
    },
};
use std::collections::{BTreeMap, BTreeSet};

fn key(value: u8) -> ModelDependencyKey {
    ModelDependencyKey(value)
}

fn dependencies(values: &[u8]) -> ModelKnownDependencies {
    values.iter().copied().map(key).collect()
}

fn level(change: u16, loss: Option<u16>) -> ModelDependencyLevel {
    ModelDependencyLevel::new(ModelDependencyCut(change), loss.map(ModelDependencyCut))
        .expect("the definitive loss cannot follow its level change")
}

fn unindexed(change: Option<u16>, loss: Option<u16>) -> ModelUnindexedDependencyLevel {
    ModelUnindexedDependencyLevel::new(change.map(ModelDependencyCut), loss.map(ModelDependencyCut))
        .expect("the unindexed loss cannot follow its level change")
}

fn frontier(
    levels: &[(u8, ModelDependencyLevel)],
    unindexed: ModelUnindexedDependencyLevel,
) -> ModelEvidenceFrontier {
    ModelEvidenceFrontier::new(
        levels
            .iter()
            .map(|(key, level)| (ModelDependencyKey(*key), *level)),
        unindexed,
    )
    .expect("the dependency level keys are unique")
}

#[test]
fn model_dependency_proof_currentness_is_the_exact_loss_cut_order() {
    let observed = dependencies(&[1]);
    for cut in 0..=3 {
        for loss in [None, Some(0), Some(1), Some(2), Some(3)] {
            let model = frontier(&[(1, level(3, loss))], unindexed(None, None));
            assert_eq!(
                model.proof_is_current(&observed, ModelDependencyCut(cut)),
                loss.is_none_or(|loss| loss <= cut),
                "cut={cut}, loss={loss:?}"
            );
        }
    }
    let absent_level = frontier(&[], unindexed(None, None));
    assert!(absent_level.proof_is_current(&observed, ModelDependencyCut(0)));
}

fn maintenance_ticket(
    scope: ModelDependencyMaintenanceScope,
    target: u16,
    loss: Option<u16>,
) -> ModelDependencyMaintenanceTicket {
    ModelDependencyMaintenanceTicket {
        key: key(1),
        has_owner_edge: true,
        target: ModelDependencyCut(target),
        scope,
        last_definitive_loss: loss.map(ModelDependencyCut),
    }
}

fn maintenance_owner(
    location: ModelDependencyMaintenanceLocation,
) -> ModelDependencyMaintenanceOwner {
    ModelDependencyMaintenanceOwner {
        identity_matches: true,
        dependencies: dependencies(&[1]),
        location,
    }
}

#[test]
fn model_dependency_maintenance_action_is_the_total_phase_cut_relation() {
    let model = frontier(&[], unindexed(None, None));
    let loss = ModelDependencyCut(2);
    for cut in 0..=4 {
        let dependency_cut = ModelDependencyCut(cut);
        let stale = dependency_cut < loss;
        let all_consumers = maintenance_ticket(
            ModelDependencyMaintenanceScope::AllConsumers,
            loss.0,
            Some(loss.0),
        );

        let accepted =
            maintenance_owner(ModelDependencyMaintenanceLocation::Accepted { dependency_cut });
        assert_eq!(
            dependency_maintenance_action(&model, all_consumers, Some(&accepted)),
            if stale {
                Err(ModelDependencyMaintenanceError::SurvivingAcceptedConsumer)
            } else {
                Ok(ModelDependencyMaintenanceAction::Advance)
            },
            "Accepted cut={cut}"
        );

        for phase in [
            ModelPreAcceptedMaintenancePhase::QueuedVerify { dependency_cut },
            ModelPreAcceptedMaintenancePhase::Ready { dependency_cut },
        ] {
            let owner = maintenance_owner(ModelDependencyMaintenanceLocation::PreAccepted(phase));
            assert_eq!(
                dependency_maintenance_action(&model, all_consumers, Some(&owner)),
                Ok(if stale {
                    ModelDependencyMaintenanceAction::Requeue
                } else {
                    ModelDependencyMaintenanceAction::Advance
                }),
                "positive-evidence cut={cut}"
            );
        }

        let waiting = maintenance_owner(ModelDependencyMaintenanceLocation::PreAccepted(
            ModelPreAcceptedMaintenancePhase::Waiting {
                observed: dependencies(&[1]),
                dependency_cut,
            },
        ));
        for scope in [
            ModelDependencyMaintenanceScope::ExistingWaiters,
            ModelDependencyMaintenanceScope::AllConsumers,
        ] {
            let ticket = maintenance_ticket(scope, loss.0, Some(loss.0));
            assert_eq!(
                dependency_maintenance_action(&model, ticket, Some(&waiting)),
                Ok(if stale {
                    ModelDependencyMaintenanceAction::Requeue
                } else {
                    ModelDependencyMaintenanceAction::Advance
                }),
                "Waiting scope={scope:?}, cut={cut}"
            );
        }
    }

    for phase in [
        ModelPreAcceptedMaintenancePhase::QueuedResolve,
        ModelPreAcceptedMaintenancePhase::Computing,
    ] {
        let owner = maintenance_owner(ModelDependencyMaintenanceLocation::PreAccepted(phase));
        assert_eq!(
            dependency_maintenance_action(
                &model,
                maintenance_ticket(
                    ModelDependencyMaintenanceScope::AllConsumers,
                    loss.0,
                    Some(loss.0),
                ),
                Some(&owner),
            ),
            Ok(ModelDependencyMaintenanceAction::Advance)
        );
    }
}

#[test]
fn model_replacement_history_requires_strictly_newer_final_availability_for_every_key() {
    let ticket = maintenance_ticket(ModelDependencyMaintenanceScope::AllConsumers, 3, Some(3));
    let history = ModelDependencyMaintenanceOwner {
        identity_matches: true,
        dependencies: dependencies(&[1, 2]),
        location: ModelDependencyMaintenanceLocation::ReplacementHistory {
            observed: dependencies(&[1, 2]),
            dependency_cut: ModelDependencyCut(2),
        },
    };
    let cases = [
        (
            frontier(
                &[(1, level(2, None)), (2, level(3, Some(2)))],
                unindexed(None, None),
            ),
            ModelDependencyMaintenanceAction::Advance,
            "same-cut change is not a later availability",
        ),
        (
            frontier(
                &[(1, level(3, Some(3))), (2, level(3, Some(2)))],
                unindexed(None, None),
            ),
            ModelDependencyMaintenanceAction::Advance,
            "a newer definitive loss is not availability",
        ),
        (
            frontier(&[(1, level(3, Some(2)))], unindexed(None, None)),
            ModelDependencyMaintenanceAction::Advance,
            "every observed key needs a current level",
        ),
        (
            frontier(
                &[(1, level(3, Some(2))), (2, level(4, Some(3)))],
                unindexed(None, None),
            ),
            ModelDependencyMaintenanceAction::Requeue,
            "every key has a strictly newer final availability",
        ),
    ];
    for (model, expected, reason) in cases {
        assert_eq!(
            dependency_maintenance_action(&model, ticket, Some(&history)),
            Ok(expected),
            "{reason}"
        );
    }
}

#[test]
fn model_dependency_maintenance_rejects_projection_faults_before_deciding_progress() {
    let model = frontier(&[], unindexed(None, None));
    let ticket = maintenance_ticket(ModelDependencyMaintenanceScope::AllConsumers, 2, Some(2));
    assert_eq!(
        dependency_maintenance_action(&model, ticket, None),
        Err(ModelDependencyMaintenanceError::Projection)
    );

    let mut mismatched = maintenance_owner(ModelDependencyMaintenanceLocation::PreAccepted(
        ModelPreAcceptedMaintenancePhase::QueuedResolve,
    ));
    mismatched.identity_matches = false;
    assert_eq!(
        dependency_maintenance_action(&model, ticket, Some(&mismatched)),
        Err(ModelDependencyMaintenanceError::Projection)
    );

    let mut absent_key = maintenance_owner(ModelDependencyMaintenanceLocation::PreAccepted(
        ModelPreAcceptedMaintenancePhase::QueuedResolve,
    ));
    absent_key.dependencies.clear();
    assert_eq!(
        dependency_maintenance_action(&model, ticket, Some(&absent_key)),
        Err(ModelDependencyMaintenanceError::Projection)
    );

    let marker = ModelDependencyMaintenanceTicket {
        has_owner_edge: false,
        ..ticket
    };
    assert_eq!(
        dependency_maintenance_action(&model, marker, None),
        Ok(ModelDependencyMaintenanceAction::Advance)
    );

    let missing_loss = ModelDependencyMaintenanceTicket {
        last_definitive_loss: None,
        ..ticket
    };
    let queued = maintenance_owner(ModelDependencyMaintenanceLocation::PreAccepted(
        ModelPreAcceptedMaintenancePhase::QueuedResolve,
    ));
    assert_eq!(
        dependency_maintenance_action(&model, missing_loss, Some(&queued)),
        Err(ModelDependencyMaintenanceError::Projection)
    );
}

#[test]
fn model_owner_free_resolution_and_missing_evidence_use_the_global_cut_exactly_once() {
    let baseline = dependencies(&[1]);
    let same = dependencies(&[1]);
    let expanded = dependencies(&[1, 2]);
    let cut = ModelDependencyCut(1);
    let stale_global_loss = frontier(
        &[(1, level(2, None)), (2, level(2, None))],
        unindexed(Some(2), Some(2)),
    );
    assert!(stale_global_loss.proof_is_current(&baseline, cut));
    assert!(!stale_global_loss.owner_free_proof_is_current(&baseline, cut));
    assert!(stale_global_loss.resolution_is_current(&baseline, &same, cut));
    assert!(!stale_global_loss.resolution_is_current(&baseline, &expanded, cut));

    let stale_new_missing_change = frontier(
        &[(1, level(1, None)), (2, level(2, None))],
        unindexed(Some(2), None),
    );
    assert!(!stale_new_missing_change.missing_result_is_current(
        &baseline,
        &same,
        &dependencies(&[2]),
        cut,
    ));
    let current = frontier(
        &[(1, level(1, None)), (2, level(1, None))],
        unindexed(Some(1), None),
    );
    assert!(current.missing_result_is_current(&baseline, &expanded, &dependencies(&[2]), cut,));
}

fn valid_receipt() -> (ModelEvidenceIdentity, ModelAdmissionReceipt) {
    let identity = ModelEvidenceIdentity {
        raw: ModelRawTransaction(1),
        witness: 2,
    };
    let receipt = ModelAdmissionReceipt {
        proof: ModelEvidenceProof {
            view: ModelEvidenceView(1),
            identity,
            dependencies: dependencies(&[1]),
            dependency_cut: ModelDependencyCut(1),
        },
    };
    (identity, receipt)
}

#[test]
fn model_acceptance_receipt_requires_chain_key_identity_proof_view_and_dependency_cut() {
    let (identity, receipt) = valid_receipt();
    let current = frontier(&[(1, level(1, None))], unindexed(Some(1), None));
    let stale = frontier(&[(1, level(2, Some(2)))], unindexed(Some(2), Some(2)));
    assert_eq!(receipt.view(), receipt.proof.view);
    assert_eq!(receipt.key(), receipt.proof.identity.raw);

    for authority_view in [ModelEvidenceView(1), ModelEvidenceView(2)] {
        for proof_view in [ModelEvidenceView(1), ModelEvidenceView(2)] {
            for owner_identity in [
                identity,
                ModelEvidenceIdentity {
                    raw: ModelRawTransaction(2),
                    ..identity
                },
                ModelEvidenceIdentity {
                    witness: 3,
                    ..identity
                },
            ] {
                for (frontier, dependency_is_current) in [(&current, true), (&stale, false)] {
                    let mut candidate = receipt.clone();
                    candidate.proof.view = proof_view;
                    let expected_final = if proof_view != authority_view {
                        ModelEvidenceValidation::StaleChain
                    } else if candidate.proof.identity != owner_identity {
                        ModelEvidenceValidation::StructuralFault
                    } else if !dependency_is_current {
                        ModelEvidenceValidation::StaleDependency
                    } else {
                        ModelEvidenceValidation::Current
                    };
                    assert_eq!(
                        validate_final_acceptance(
                            authority_view,
                            owner_identity,
                            frontier,
                            &candidate,
                        ),
                        expected_final
                    );

                    let expected_direct = if proof_view != authority_view {
                        ModelEvidenceValidation::StaleChain
                    } else if !dependency_is_current {
                        ModelEvidenceValidation::StaleDependency
                    } else {
                        ModelEvidenceValidation::Current
                    };
                    assert_eq!(
                        validate_direct_acceptance(authority_view, frontier, &candidate),
                        expected_direct
                    );
                }
            }
        }
    }
}

#[test]
fn model_final_subject_and_direct_rejection_have_closed_currentness_outcomes() {
    let current = frontier(&[(1, level(1, None))], unindexed(Some(1), None));
    let owner = ModelReadyOwner {
        version: 4,
        ready: true,
        dependencies: dependencies(&[1]),
        dependency_cut: ModelDependencyCut(1),
    };
    let owners = BTreeMap::from([(ModelRawTransaction(1), owner.clone())]);
    let subject = ModelFinalAdmissionSubject {
        view: ModelEvidenceView(1),
        key: ModelRawTransaction(1),
        version: 4,
        dependency_cut: ModelDependencyCut(1),
    };
    assert_eq!(
        validate_final_subject(ModelEvidenceView(1), &owners, &current, subject),
        ModelSubjectValidation::Current
    );
    assert_eq!(
        validate_final_subject(
            ModelEvidenceView(1),
            &owners,
            &current,
            ModelFinalAdmissionSubject {
                dependency_cut: ModelDependencyCut(0),
                ..subject
            },
        ),
        ModelSubjectValidation::StaleDependency
    );
    let not_ready = BTreeMap::from([(
        ModelRawTransaction(1),
        ModelReadyOwner {
            ready: false,
            ..owner
        },
    )]);
    assert_eq!(
        validate_final_subject(ModelEvidenceView(1), &not_ready, &current, subject),
        ModelSubjectValidation::StalePhase
    );

    assert_eq!(
        validate_direct_rejection(
            ModelEvidenceView(1),
            7,
            ModelDirectRejectionValidity::Stable,
        ),
        ModelDirectRejectionObservation::Current
    );
    for (view, source, expected) in [
        (1, 7, ModelDirectRejectionObservation::Current),
        (2, 7, ModelDirectRejectionObservation::StaleChain),
        (1, 8, ModelDirectRejectionObservation::StaleSource),
    ] {
        assert_eq!(
            validate_direct_rejection(
                ModelEvidenceView(1),
                7,
                ModelDirectRejectionValidity::AcceptedCut {
                    view: ModelEvidenceView(view),
                    accepted_source: source,
                },
            ),
            expected
        );
    }
}

#[test]
fn model_missing_source_policy_is_remote_wait_or_trusted_definitive_rejection() {
    let known_cell = ModelMissingFact::Cell {
        key: key(1),
        parent_is_preaccepted: true,
    };
    let unknown_cell = ModelMissingFact::Cell {
        key: key(2),
        parent_is_preaccepted: false,
    };
    let header = ModelMissingFact::Header { key: key(3) };
    for facts in [
        BTreeSet::from([known_cell]),
        BTreeSet::from([unknown_cell]),
        BTreeSet::from([header]),
        BTreeSet::from([known_cell, unknown_cell, header]),
    ] {
        assert_eq!(
            missing_resolution_disposition(ModelPreAcceptedSource::Remote, &facts),
            ModelMissingDisposition::Wait
        );
    }
    assert_eq!(
        missing_resolution_disposition(
            ModelPreAcceptedSource::Proposal,
            &BTreeSet::from([known_cell]),
        ),
        ModelMissingDisposition::Wait
    );
    assert_eq!(
        missing_resolution_disposition(
            ModelPreAcceptedSource::Recovery,
            &BTreeSet::from([unknown_cell]),
        ),
        ModelMissingDisposition::RejectUnknownCell(key(2))
    );
    assert_eq!(
        missing_resolution_disposition(ModelPreAcceptedSource::Proposal, &BTreeSet::from([header]),),
        ModelMissingDisposition::RejectInvalidHeader(key(3))
    );
}

#[test]
fn model_released_input_is_derived_from_the_projected_final_owner_set() {
    let victim = ModelRawTransaction(1);
    let removed_spender = ModelRawTransaction(2);
    let retained_spender = ModelRawTransaction(3);
    let removed = BTreeSet::from([victim, removed_spender]);
    for candidate_uses_input in [false, true] {
        for current_spender in [None, Some(removed_spender), Some(retained_spender)] {
            for chain_backed in [false, true] {
                for parent in [
                    ModelPoolParent::Removed,
                    ModelPoolParent::Other,
                    ModelPoolParent::SurvivingAccepted { output_count: 1 },
                    ModelPoolParent::SurvivingAccepted { output_count: 2 },
                ] {
                    let cut = ModelReleasedInputCut {
                        context: ModelReleasedInputContext::Replacement {
                            candidate_uses_input,
                        },
                        current_spender,
                        removed: removed.clone(),
                        chain_backed,
                        parent,
                        output_index: 1,
                    };
                    let expected = if candidate_uses_input {
                        ModelReleasedInputDisposition::Retained
                    } else if current_spender.is_none() {
                        ModelReleasedInputDisposition::StructuralFault
                    } else if current_spender.is_none_or(|spender| !removed.contains(&spender)) {
                        ModelReleasedInputDisposition::Retained
                    } else if chain_backed
                        || matches!(
                            parent,
                            ModelPoolParent::SurvivingAccepted { output_count } if 1 < output_count
                        )
                    {
                        ModelReleasedInputDisposition::Released
                    } else {
                        ModelReleasedInputDisposition::Retained
                    };
                    assert_eq!(released_input_disposition(&cut), expected);
                }
            }
        }
    }

    let administrative = ModelReleasedInputCut {
        context: ModelReleasedInputContext::Administrative { victim },
        current_spender: Some(victim),
        removed,
        chain_backed: false,
        parent: ModelPoolParent::SurvivingAccepted { output_count: 1 },
        output_index: 1,
    };
    assert_eq!(
        released_input_disposition(&administrative),
        ModelReleasedInputDisposition::Retained
    );
    assert_eq!(
        released_input_disposition(&ModelReleasedInputCut {
            parent: ModelPoolParent::SurvivingAccepted { output_count: 2 },
            ..administrative.clone()
        }),
        ModelReleasedInputDisposition::Released
    );
    assert_eq!(
        released_input_disposition(&ModelReleasedInputCut {
            current_spender: Some(retained_spender),
            ..administrative
        }),
        ModelReleasedInputDisposition::StructuralFault
    );
}

#[test]
fn model_accepted_pool_output_construction_excludes_the_weak_bound_counterexample() {
    for output_count in 0..=4 {
        for output_index in 0..=5 {
            let reference = ModelAcceptedPoolOutput::new(output_index, output_count);
            assert_eq!(reference.is_some(), output_index < output_count);
            if let Some(reference) = reference {
                assert!(reference.output_index() < reference.output_count());
                assert!(reference.output_index() <= reference.output_count());
            }
        }
    }
}

#[test]
fn model_replacement_history_trigger_is_exactly_conflict_or_removed_pool_producer() {
    for producer_removed in [false, true] {
        for chain_backed in [false, true] {
            for candidate_uses_input in [false, true] {
                assert_eq!(
                    replacement_history_trigger(
                        ModelReplacementReference::Input {
                            candidate_uses_input,
                        },
                        producer_removed,
                        chain_backed,
                    ),
                    candidate_uses_input || (producer_removed && !chain_backed)
                );
            }
            assert_eq!(
                replacement_history_trigger(
                    ModelReplacementReference::CellDependency,
                    producer_removed,
                    chain_backed,
                ),
                producer_removed && !chain_backed
            );
        }
    }
}

#[test]
fn model_final_location_refresh_updates_payload_and_context_atomically() {
    let locations = [
        ModelCellLocation::Pool,
        ModelCellLocation::Chain(1),
        ModelCellLocation::Chain(2),
    ];
    for previous in locations {
        for authoritative in locations {
            let transition = validated_location_transition(previous, authoritative);
            assert_eq!(transition.payload_location, authoritative);
            assert_eq!(transition.context_location, authoritative);
            assert_eq!(
                transition.relation,
                if previous == authoritative {
                    ModelReadyPayloadRelation::Shared
                } else {
                    ModelReadyPayloadRelation::LocationRefreshed
                }
            );
        }
    }
}
