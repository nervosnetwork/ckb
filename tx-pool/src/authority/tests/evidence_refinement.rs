//! Production refinement for dependency and admission evidence currentness.
//!
//! The immutable frontier relation is exhausted over a finite two-key domain.
//! Admission receipts and subjects are produced by the real lifecycle; sealed
//! structural mismatches are owned by the production-constructor audit rather
//! than by forged fixtures.

use super::foundation::{
    accept_remote_transaction, apply_plan, direct_verified_facts, limits, owner_version,
    resolved_payload_with_facts, tx, verify_remote_transaction_with_payload,
};
use crate::{
    authority::{
        chain::{DirectAdmissionWork, FinalAdmissionSubject},
        dependency::{
            DependencyFrontier,
            test_support::{DependencyEvidenceLevelInput, UnindexedDependencyLevelInput},
        },
        ingress::DirectCommand,
        plan::{
            AuthorityFault, PlanError, StalePlan, TxPoolAuthority,
            test_support::MissingResolutionObservationForFoundation,
        },
        rejection::DirectTransactionRejection,
        state::{
            AcceptedStatus, ApplySequence, ChainRevision, ChainViewId, DependencyCut,
            DependencyKey, KnownDependencies, MissingDependencies, PoolGeneration,
            PreAcceptedSource, RawTxHash, ValidatedAdmission,
        },
    },
    error::Reject,
    mathematical_model::{
        ModelAdmissionReceipt, ModelDependencyCut, ModelDependencyKey, ModelDependencyLevel,
        ModelDirectRejectionObservation, ModelDirectRejectionValidity, ModelEvidenceFrontier,
        ModelEvidenceIdentity, ModelEvidenceProof, ModelEvidenceValidation, ModelEvidenceView,
        ModelFinalAdmissionSubject, ModelKnownDependencies, ModelMissingDisposition,
        ModelMissingFact, ModelPreAcceptedSource, ModelRawTransaction, ModelReadyOwner,
        ModelSubjectValidation, ModelUnindexedDependencyLevel, missing_resolution_disposition,
        validate_direct_acceptance, validate_direct_rejection, validate_final_acceptance,
        validate_final_subject,
    },
};
use ckb_network::PeerIndex;
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, TransactionBuilder, TransactionView},
    packed::{Byte32, CellInput, CellOutput, OutPoint},
    prelude::Pack,
};
use ckb_verification::cache::ScriptVerificationRules;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AbstractLevel {
    change: Option<u16>,
    loss: Option<u16>,
}

fn finite_levels() -> Vec<AbstractLevel> {
    let mut levels = vec![AbstractLevel {
        change: None,
        loss: None,
    }];
    for change in 0..=2 {
        levels.push(AbstractLevel {
            change: Some(change),
            loss: None,
        });
        for loss in 0..=change {
            levels.push(AbstractLevel {
                change: Some(change),
                loss: Some(loss),
            });
        }
    }
    levels
}

fn production_keys() -> [DependencyKey; 2] {
    [
        DependencyKey::Cell(OutPoint::new(Byte32::new([44; 32]), 0)),
        DependencyKey::Header(Byte32::new([45; 32])),
    ]
}

fn production_cut(value: u16) -> DependencyCut {
    DependencyCut(ApplySequence(u128::from(value)))
}

fn production_frontier(
    keys: &[DependencyKey; 2],
    levels: [AbstractLevel; 2],
    unindexed: AbstractLevel,
) -> DependencyFrontier {
    let levels = keys.iter().cloned().zip(levels).filter_map(|(key, level)| {
        level.change.map(|change| DependencyEvidenceLevelInput {
            key,
            last_change: production_cut(change),
            last_definitive_loss: level.loss.map(production_cut),
        })
    });
    DependencyFrontier::from_evidence_cut_for_foundation(
        levels,
        UnindexedDependencyLevelInput {
            last_change: unindexed.change.map(production_cut),
            last_definitive_loss: unindexed.loss.map(production_cut),
        },
    )
    .expect("the finite production evidence levels are legal and unique")
}

fn model_frontier(levels: [AbstractLevel; 2], unindexed: AbstractLevel) -> ModelEvidenceFrontier {
    let levels = levels.into_iter().enumerate().filter_map(|(index, level)| {
        level.change.map(|change| {
            (
                ModelDependencyKey(index as u8),
                ModelDependencyLevel::new(
                    ModelDependencyCut(change),
                    level.loss.map(ModelDependencyCut),
                )
                .expect("the finite model loss does not follow its change"),
            )
        })
    });
    ModelEvidenceFrontier::new(
        levels,
        ModelUnindexedDependencyLevel::new(
            unindexed.change.map(ModelDependencyCut),
            unindexed.loss.map(ModelDependencyCut),
        )
        .expect("the finite model unindexed level is legal"),
    )
    .expect("the finite model evidence keys are unique")
}

fn production_dependencies(keys: &[DependencyKey; 2], mask: u8) -> KnownDependencies {
    KnownDependencies::from_keys_for_foundation(
        keys.iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, key)| key.clone())
            .collect(),
    )
    .expect("the two-key production dependency set is bounded")
}

fn production_missing(keys: &[DependencyKey; 2], mask: u8) -> MissingDependencies {
    MissingDependencies::from_keys_for_foundation(
        keys.iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, key)| key.clone())
            .collect(),
    )
    .expect("the nonempty two-key missing set is bounded")
}

fn model_dependencies(mask: u8) -> ModelKnownDependencies {
    (0..2)
        .filter(|index| mask & (1 << index) != 0)
        .map(ModelDependencyKey)
        .collect()
}

#[test]
fn uak_dependency_evidence_currentness_refines_the_complete_finite_cut_relation() {
    let keys = production_keys();
    let finite = finite_levels();
    for first in &finite {
        for second in &finite {
            for unindexed in &finite {
                let level_cut = [*first, *second];
                let production = production_frontier(&keys, level_cut, *unindexed);
                let model = model_frontier(level_cut, *unindexed);
                for cut in 0..=2 {
                    let production_cut = production_cut(cut);
                    let model_cut = ModelDependencyCut(cut);
                    for dependency_mask in 0..4 {
                        let production_dependencies =
                            production_dependencies(&keys, dependency_mask);
                        let model_dependencies = model_dependencies(dependency_mask);
                        assert_eq!(
                            production.proof_is_current(&production_dependencies, production_cut,),
                            model.proof_is_current(&model_dependencies, model_cut),
                            "proof currentness differs: levels={level_cut:?}, unindexed={unindexed:?}, cut={cut}, dependencies={dependency_mask:02b}"
                        );
                        assert_eq!(
                            production.owner_free_proof_is_current(
                                &production_dependencies,
                                production_cut,
                            ),
                            model.owner_free_proof_is_current(&model_dependencies, model_cut),
                            "owner-free currentness differs: levels={level_cut:?}, unindexed={unindexed:?}, cut={cut}, dependencies={dependency_mask:02b}"
                        );
                    }
                    for baseline_mask in 0..4 {
                        let production_baseline = production_dependencies(&keys, baseline_mask);
                        let model_baseline = model_dependencies(baseline_mask);
                        for resolved_mask in 0..4 {
                            let production_resolved = production_dependencies(&keys, resolved_mask);
                            let model_resolved = model_dependencies(resolved_mask);
                            assert_eq!(
                                production.resolution_is_current(
                                    &production_baseline,
                                    &production_resolved,
                                    production_cut,
                                ),
                                model.resolution_is_current(
                                    &model_baseline,
                                    &model_resolved,
                                    model_cut,
                                ),
                                "resolution currentness differs: levels={level_cut:?}, unindexed={unindexed:?}, cut={cut}, baseline={baseline_mask:02b}, resolved={resolved_mask:02b}"
                            );
                            for missing_mask in 1..4 {
                                let production_missing = production_missing(&keys, missing_mask);
                                let model_missing = model_dependencies(missing_mask);
                                assert_eq!(
                                    production.missing_result_is_current(
                                        &production_baseline,
                                        &production_resolved,
                                        &production_missing,
                                        production_cut,
                                    ),
                                    model.missing_result_is_current(
                                        &model_baseline,
                                        &model_resolved,
                                        &model_missing,
                                        model_cut,
                                    ),
                                    "missing-result currentness differs: levels={level_cut:?}, unindexed={unindexed:?}, cut={cut}, baseline={baseline_mask:02b}, resolved={resolved_mask:02b}, missing={missing_mask:02b}"
                                );
                            }
                        }
                        for missing_mask in 1..4 {
                            let production_missing = production_missing(&keys, missing_mask);
                            let model_missing = model_dependencies(missing_mask);
                            assert_eq!(
                                production.missing_observation_is_current(
                                    &production_baseline,
                                    &production_missing,
                                    production_cut,
                                ),
                                model.missing_observation_is_current(
                                    &model_baseline,
                                    &model_missing,
                                    model_cut,
                                ),
                                "missing-observation currentness differs: levels={level_cut:?}, unindexed={unindexed:?}, cut={cut}, baseline={baseline_mask:02b}, missing={missing_mask:02b}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EvidenceScenario {
    Current,
    StaleChain,
    DefinitiveLoss,
}

fn ready_receipt_fixture(
    nonce: u8,
) -> (
    TxPoolAuthority,
    TransactionView,
    RawTxHash,
    DependencyKey,
    crate::authority::chain::FinalAdmissionReceipt,
) {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let input = OutPoint::new(Byte32::new([nonce; 32]), 0);
    let transaction = TransactionBuilder::default()
        .version(u32::from(nonce))
        .input(CellInput::new(input.clone(), 0))
        .build();
    let payload = resolved_payload_with_facts(
        &transaction,
        Vec::new(),
        vec![input.clone()],
        Capacity::shannons(1),
    );
    let key = verify_remote_transaction_with_payload(
        &mut authority,
        transaction.clone(),
        usize::from(nonce),
        payload,
    );
    let receipt = authority
        .final_admission_work(&key, owner_version(&authority, &key))
        .expect("the real Ready owner issues final-admission work")
        .validate_for_foundation(AcceptedStatus::Pending, ScriptVerificationRules::V0)
        .expect("the fixture final evidence is sealed for the current view");
    (
        authority,
        transaction,
        key,
        DependencyKey::Cell(input),
        receipt,
    )
}

fn apply_evidence_scenario(
    authority: &mut TxPoolAuthority,
    dependency: &DependencyKey,
    scenario: EvidenceScenario,
) {
    match scenario {
        EvidenceScenario::Current => {}
        EvidenceScenario::StaleChain => {
            authority.force_chain_view(ChainViewId::new(ChainRevision(1), Byte32::new([90; 32])))
        }
        EvidenceScenario::DefinitiveLoss => apply_plan(
            authority
                .plan_dependency_loss_for_foundation(vec![dependency.clone()])
                .expect("the dependency loss event plans")
                .expect("the nonempty loss event has one Apply"),
        ),
    }
}

fn abstract_receipt() -> (ModelEvidenceIdentity, ModelAdmissionReceipt) {
    let identity = ModelEvidenceIdentity {
        raw: ModelRawTransaction(1),
        witness: 1,
    };
    (
        identity,
        ModelAdmissionReceipt {
            proof: ModelEvidenceProof {
                view: ModelEvidenceView(0),
                identity,
                dependencies: [ModelDependencyKey(0)].into_iter().collect(),
                dependency_cut: ModelDependencyCut(0),
            },
        },
    )
}

fn scenario_frontier(scenario: EvidenceScenario, owner_free: bool) -> ModelEvidenceFrontier {
    let loss = (scenario == EvidenceScenario::DefinitiveLoss).then_some(ModelDependencyCut(1));
    let levels = (!owner_free).then(|| {
        (
            ModelDependencyKey(0),
            ModelDependencyLevel::new(ModelDependencyCut(1), loss)
                .expect("the scenario loss shares its change cut"),
        )
    });
    ModelEvidenceFrontier::new(
        levels,
        ModelUnindexedDependencyLevel::new(
            owner_free.then_some(ModelDependencyCut(1)),
            owner_free.then_some(loss).flatten(),
        )
        .expect("the owner-free scenario has one legal global level"),
    )
    .expect("the scenario has at most one dependency level")
}

fn production_evidence_observation(result: Result<(), PlanError>) -> ModelEvidenceValidation {
    match result {
        Ok(()) => ModelEvidenceValidation::Current,
        Err(PlanError::Stale(StalePlan::ChainRevision)) => ModelEvidenceValidation::StaleChain,
        Err(PlanError::Stale(StalePlan::Dependency)) => ModelEvidenceValidation::StaleDependency,
        Err(PlanError::Fault(AuthorityFault::MembershipProjection)) => {
            ModelEvidenceValidation::StructuralFault
        }
        other => panic!("unexpected production evidence observation: {other:?}"),
    }
}

#[test]
fn uak_final_and_direct_acceptance_refine_current_chain_and_dependency_cuts() {
    for scenario in [
        EvidenceScenario::Current,
        EvidenceScenario::StaleChain,
        EvidenceScenario::DefinitiveLoss,
    ] {
        let (mut authority, _, key, dependency, receipt) =
            ready_receipt_fixture(46 + scenario as u8);
        apply_evidence_scenario(&mut authority, &dependency, scenario);
        let production = production_evidence_observation(
            authority.validate_final_acceptance_for_foundation(&key, &receipt),
        );
        let (identity, model_receipt) = abstract_receipt();
        let model = validate_final_acceptance(
            if scenario == EvidenceScenario::StaleChain {
                ModelEvidenceView(1)
            } else {
                ModelEvidenceView(0)
            },
            identity,
            &scenario_frontier(scenario, false),
            &model_receipt,
        );
        assert_eq!(production, model, "final evidence scenario={scenario:?}");

        let mut direct_authority = TxPoolAuthority::for_foundation(limits());
        let input = OutPoint::new(Byte32::new([56 + scenario as u8; 32]), 0);
        let transaction = Arc::new(
            TransactionBuilder::default()
                .version(5_600 + u32::from(scenario as u8))
                .input(CellInput::new(input.clone(), 0))
                .build(),
        );
        let verified = direct_verified_facts(
            &transaction,
            Vec::new(),
            vec![input.clone()],
            Capacity::shannons(1),
        );
        let direct_receipt = DirectAdmissionWork::new(Arc::clone(&transaction), verified)
            .expect("direct work binds the exact transaction identity")
            .validate_for_foundation(AcceptedStatus::Pending, ScriptVerificationRules::V0)
            .expect("direct evidence is sealed for the initial view");
        apply_evidence_scenario(&mut direct_authority, &DependencyKey::Cell(input), scenario);
        let production = production_evidence_observation(
            direct_authority.validate_direct_acceptance_for_foundation(&direct_receipt),
        );
        let (_, model_receipt) = abstract_receipt();
        let model = validate_direct_acceptance(
            if scenario == EvidenceScenario::StaleChain {
                ModelEvidenceView(1)
            } else {
                ModelEvidenceView(0)
            },
            &scenario_frontier(scenario, true),
            &model_receipt,
        );
        assert_eq!(production, model, "direct evidence scenario={scenario:?}");
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SubjectScenario {
    Current,
    StaleChain,
    Missing,
    StaleVersion,
    StaleDependency,
}

fn production_subject_observation(result: Result<(), PlanError>) -> ModelSubjectValidation {
    match result {
        Ok(()) => ModelSubjectValidation::Current,
        Err(PlanError::Stale(StalePlan::ChainRevision)) => ModelSubjectValidation::StaleChain,
        Err(PlanError::Stale(StalePlan::Missing)) => ModelSubjectValidation::Missing,
        Err(PlanError::Stale(StalePlan::Version)) => ModelSubjectValidation::StaleVersion,
        Err(PlanError::Stale(StalePlan::Phase)) => ModelSubjectValidation::StalePhase,
        Err(PlanError::Stale(StalePlan::Dependency)) => ModelSubjectValidation::StaleDependency,
        other => panic!("unexpected production subject observation: {other:?}"),
    }
}

#[test]
fn uak_final_subject_refines_every_reachable_currentness_outcome() {
    for scenario in [
        SubjectScenario::Current,
        SubjectScenario::StaleChain,
        SubjectScenario::Missing,
        SubjectScenario::StaleVersion,
        SubjectScenario::StaleDependency,
    ] {
        let (mut authority, transaction, key, dependency, receipt) =
            ready_receipt_fixture(60 + scenario as u8);
        let expected = owner_version(&authority, &key);
        let subject = FinalAdmissionSubject::for_foundation(
            key.clone(),
            expected,
            authority.chain_view().clone(),
            receipt.proof().dependency_cut(),
        );
        match scenario {
            SubjectScenario::Current => {}
            SubjectScenario::StaleChain => authority
                .force_chain_view(ChainViewId::new(ChainRevision(1), Byte32::new([91; 32]))),
            SubjectScenario::Missing => apply_plan(
                authority
                    .plan_terminalize_for_foundation(&key, expected)
                    .expect("the Ready owner terminalizes"),
            ),
            SubjectScenario::StaleVersion => {
                let changed_witness = transaction
                    .as_advanced_builder()
                    .set_witnesses(vec![Bytes::from_static(b"new-witness").pack()])
                    .build();
                apply_plan(
                    authority
                        .plan_admission(
                            ValidatedAdmission::proposal(changed_witness)
                                .expect("the same-raw trusted payload is valid"),
                        )
                        .expect("the different-witness proposal replaces the owner"),
                );
            }
            SubjectScenario::StaleDependency => apply_plan(
                authority
                    .plan_dependency_loss_for_foundation(vec![dependency])
                    .expect("the subject dependency loss plans")
                    .expect("the subject dependency loss has one Apply"),
            ),
        }
        let production = production_subject_observation(
            authority.validate_final_subject_for_foundation(&subject),
        );

        let owner = ModelReadyOwner {
            version: if scenario == SubjectScenario::StaleVersion {
                2
            } else {
                1
            },
            ready: true,
            dependencies: [ModelDependencyKey(0)].into_iter().collect(),
            dependency_cut: ModelDependencyCut(0),
        };
        let owners = if scenario == SubjectScenario::Missing {
            BTreeMap::new()
        } else {
            BTreeMap::from([(ModelRawTransaction(1), owner)])
        };
        let model = validate_final_subject(
            if scenario == SubjectScenario::StaleChain {
                ModelEvidenceView(1)
            } else {
                ModelEvidenceView(0)
            },
            &owners,
            &scenario_frontier(
                if scenario == SubjectScenario::StaleDependency {
                    EvidenceScenario::DefinitiveLoss
                } else {
                    EvidenceScenario::Current
                },
                false,
            ),
            ModelFinalAdmissionSubject {
                view: ModelEvidenceView(0),
                key: ModelRawTransaction(1),
                version: 1,
                dependency_cut: ModelDependencyCut(0),
            },
        );
        assert_eq!(production, model, "subject scenario={scenario:?}");
    }
}

fn production_direct_rejection_observation(
    result: Result<(), PlanError>,
) -> ModelDirectRejectionObservation {
    match result {
        Ok(()) => ModelDirectRejectionObservation::Current,
        Err(PlanError::Stale(StalePlan::ChainRevision)) => {
            ModelDirectRejectionObservation::StaleChain
        }
        Err(PlanError::Stale(StalePlan::SourceVersion)) => {
            ModelDirectRejectionObservation::StaleSource
        }
        other => panic!("unexpected direct-rejection observation: {other:?}"),
    }
}

fn chain_bound_rejection(authority: &TxPoolAuthority, nonce: u64) -> DirectTransactionRejection {
    DirectTransactionRejection::accepted_cut(
        Arc::new(tx(nonce)),
        DirectCommand::TestAccept,
        Reject::Invalidated("foundation chain-bound rejection".to_owned()),
        authority.chain_view().clone(),
        authority.accepted_source_for_reference(),
    )
}

#[test]
fn uak_direct_rejection_refines_the_closed_view_and_source_truth_table() {
    let mut current = TxPoolAuthority::for_foundation(limits());
    let stable = DirectTransactionRejection::stable(
        Arc::new(tx(6_700)),
        DirectCommand::TestAccept,
        Reject::Invalidated("foundation stable rejection".to_owned()),
    );
    assert_eq!(
        production_direct_rejection_observation(
            current.direct_rejection_is_current(stable.validity()),
        ),
        validate_direct_rejection(
            ModelEvidenceView(0),
            0,
            ModelDirectRejectionValidity::Stable,
        )
    );

    let current_cut = chain_bound_rejection(&current, 6_701);
    assert_eq!(
        production_direct_rejection_observation(
            current.direct_rejection_is_current(current_cut.validity()),
        ),
        validate_direct_rejection(
            ModelEvidenceView(0),
            0,
            ModelDirectRejectionValidity::AcceptedCut {
                view: ModelEvidenceView(0),
                accepted_source: 0,
            },
        )
    );

    let stale_view = chain_bound_rejection(&current, 6_702);
    current.force_chain_view(ChainViewId::new(ChainRevision(1), Byte32::new([92; 32])));
    assert_eq!(
        production_direct_rejection_observation(
            current.direct_rejection_is_current(stale_view.validity()),
        ),
        validate_direct_rejection(
            ModelEvidenceView(1),
            0,
            ModelDirectRejectionValidity::AcceptedCut {
                view: ModelEvidenceView(0),
                accepted_source: 0,
            },
        )
    );

    let mut stale_source = TxPoolAuthority::for_foundation(limits());
    let old_cut = chain_bound_rejection(&stale_source, 6_703);
    accept_remote_transaction(
        &mut stale_source,
        tx(6_704),
        6_704,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    assert_eq!(
        production_direct_rejection_observation(
            stale_source.direct_rejection_is_current(old_cut.validity()),
        ),
        validate_direct_rejection(
            ModelEvidenceView(0),
            1,
            ModelDirectRejectionValidity::AcceptedCut {
                view: ModelEvidenceView(0),
                accepted_source: 0,
            },
        )
    );
}

fn model_key_for_production(
    key: &DependencyKey,
    cell: &OutPoint,
    header: &Byte32,
) -> ModelDependencyKey {
    match key {
        DependencyKey::Cell(out_point) if out_point == cell => ModelDependencyKey(0),
        DependencyKey::Header(hash) if hash == header => ModelDependencyKey(1),
        other => panic!("unexpected finite missing key: {other:?}"),
    }
}

fn production_missing_observation(
    observation: MissingResolutionObservationForFoundation,
    cell: &OutPoint,
    header: &Byte32,
) -> ModelMissingDisposition {
    match observation {
        MissingResolutionObservationForFoundation::Wait => ModelMissingDisposition::Wait,
        MissingResolutionObservationForFoundation::RejectUnknownCell(out_point) => {
            ModelMissingDisposition::RejectUnknownCell(model_key_for_production(
                &DependencyKey::Cell(out_point),
                cell,
                header,
            ))
        }
        MissingResolutionObservationForFoundation::RejectInvalidHeader(hash) => {
            ModelMissingDisposition::RejectInvalidHeader(model_key_for_production(
                &DependencyKey::Header(hash),
                cell,
                header,
            ))
        }
        MissingResolutionObservationForFoundation::UnexpectedReject(rejection) => {
            panic!("missing policy produced an unmodeled rejection: {rejection:?}")
        }
    }
}

fn source_pair(
    source: ModelPreAcceptedSource,
    nonce: u64,
) -> (PreAcceptedSource, ModelPreAcceptedSource) {
    let admission = match source {
        ModelPreAcceptedSource::Remote => {
            ValidatedAdmission::remote(tx(nonce), PeerIndex::from(nonce as usize))
        }
        ModelPreAcceptedSource::Proposal => ValidatedAdmission::proposal(tx(nonce)),
        ModelPreAcceptedSource::Recovery => {
            ValidatedAdmission::recovery(tx(nonce), PoolGeneration(0))
        }
    }
    .expect("the source fixture admission is valid");
    (admission.source, source)
}

#[test]
fn uak_missing_dependency_policy_refines_source_and_parent_ownership() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = TransactionBuilder::default()
        .version(6_800u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent = ValidatedAdmission::proposal(parent_tx.clone())
        .expect("the known parent proposal is valid");
    apply_plan(
        authority
            .plan_admission(parent)
            .expect("the known parent enters PreAccepted ownership"),
    );
    let known_cell = OutPoint::new(parent_tx.hash(), 0);
    let unknown_cell = OutPoint::new(Byte32::new([93; 32]), 0);
    let header = Byte32::new([94; 32]);

    let cases = [
        (
            vec![DependencyKey::Cell(known_cell)],
            BTreeSet::from([ModelMissingFact::Cell {
                key: ModelDependencyKey(0),
                parent_is_preaccepted: true,
            }]),
        ),
        (
            vec![DependencyKey::Cell(unknown_cell.clone())],
            BTreeSet::from([ModelMissingFact::Cell {
                key: ModelDependencyKey(0),
                parent_is_preaccepted: false,
            }]),
        ),
        (
            vec![DependencyKey::Header(header.clone())],
            BTreeSet::from([ModelMissingFact::Header {
                key: ModelDependencyKey(1),
            }]),
        ),
        (
            vec![
                DependencyKey::Cell(unknown_cell.clone()),
                DependencyKey::Header(header.clone()),
            ],
            BTreeSet::from([
                ModelMissingFact::Cell {
                    key: ModelDependencyKey(0),
                    parent_is_preaccepted: false,
                },
                ModelMissingFact::Header {
                    key: ModelDependencyKey(1),
                },
            ]),
        ),
    ];
    for source in [
        ModelPreAcceptedSource::Remote,
        ModelPreAcceptedSource::Proposal,
        ModelPreAcceptedSource::Recovery,
    ] {
        let (production_source, model_source) = source_pair(source, 6_810 + source as u64);
        for (keys, facts) in &cases {
            let missing = MissingDependencies::from_keys_for_foundation(keys.clone())
                .expect("the finite missing set is bounded and nonempty");
            let production = production_missing_observation(
                authority
                    .missing_resolution_observation_for_foundation(production_source, &missing),
                &unknown_cell,
                &header,
            );
            let model = missing_resolution_disposition(model_source, facts);
            assert_eq!(
                production, model,
                "missing policy differs: source={source:?}, keys={keys:?}"
            );
        }
    }
}
