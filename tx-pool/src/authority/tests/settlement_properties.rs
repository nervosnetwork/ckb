//! Production refinement for the total compute-settlement classifier.
//!
//! Legal outcomes are constructed through the real owner lifecycle. Forged
//! receipt fields remain outside this differential and are owned by the sealed
//! constructor audit.

use super::claim_relations::{
    ClaimDependencyCut, ClaimDependencyKey, ClaimDependencyLevel, ClaimEvidenceFrontier,
    ClaimEvidenceIdentity, ClaimEvidenceView, ClaimKnownDependencies, ClaimMissingSettlement,
    ClaimPayloadPolicy, ClaimPayloadPolicyEvolution, ClaimRawTransaction, ClaimSettlementCut,
    ClaimSettlementEvidence, ClaimSettlementFault, ClaimSettlementNext, ClaimSettlementObservation,
    ClaimSettlementOrigin, ClaimSettlementRejection, ClaimUnindexedDependencyLevel,
    ClaimVerifyCycleClass,
};
use super::foundation::{
    apply_plan, limits, owner_version, resolved_payload_with_facts, take_resolve_work,
};
use crate::authority::{
    plan::{
        AuthorityFault, PlanError, TxPoolAuthority,
        test_support::SettlementClassificationObservationForFoundation,
    },
    state::{
        OwnedTx, PayloadPolicy, PayloadPolicyEvolution, PreAcceptedPhase, QueuedWork,
        ValidatedAdmission, VerifyCapability, VerifyCycleClass, WorkPermit,
        test_support::RejectionKind,
    },
    work::{CheckedOutWork, ComputeSettlement},
};
use ckb_network::PeerIndex;
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, TransactionBuilder, TransactionView},
    packed::{Byte32, CellInput, CellOutput, OutPoint},
    prelude::Pack,
};

fn stale_reference_cut() -> ClaimSettlementCut {
    let key = ClaimDependencyKey::cell(1);
    let baseline: ClaimKnownDependencies = [key].into_iter().collect();
    ClaimSettlementCut {
        authority_view: ClaimEvidenceView(1),
        owner_identity: ClaimEvidenceIdentity {
            raw: ClaimRawTransaction(1),
            witness: 1,
        },
        baseline_dependencies: baseline,
        current_policy: ClaimPayloadPolicy::RemoteDeclaredCycles(7),
        active: ClaimSettlementOrigin {
            payload_identity: ClaimEvidenceIdentity {
                raw: ClaimRawTransaction(1),
                witness: 1,
            },
            view: ClaimEvidenceView(1),
            dependency_cut: ClaimDependencyCut(1),
            payload_policy: ClaimPayloadPolicy::RemoteDeclaredCycles(7),
        },
        frontier: ClaimEvidenceFrontier::new(
            [(
                key,
                ClaimDependencyLevel::new(ClaimDependencyCut(2), Some(ClaimDependencyCut(2)))
                    .expect("the definitive loss follows the active evidence cut"),
            )],
            ClaimUnindexedDependencyLevel::new(
                Some(ClaimDependencyCut(2)),
                Some(ClaimDependencyCut(2)),
            )
            .expect("the retired loss is a legal global evidence level"),
        )
        .expect("the finite dependency key is unique"),
    }
}

#[test]
fn uak_settlement_reference_retires_resource_rejection_after_baseline_loss() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = TransactionBuilder::default()
        .version(41_001u32)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let parent_admission =
        ValidatedAdmission::proposal(parent_tx.clone()).expect("the parent proposal is valid");
    let parent = parent_admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(parent_admission)
            .expect("the parent enters preacceptance"),
    );

    let parent_output = OutPoint::new(parent_tx.hash(), 0);
    let child_tx = TransactionBuilder::default()
        .version(41_002u32)
        .input(CellInput::new(parent_output, 0))
        .build();
    let child_admission = ValidatedAdmission::remote(child_tx, PeerIndex::from(41_002))
        .expect("the dependent child admission is valid");
    let child = child_admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(child_admission)
            .expect("the dependent child enters Resolve"),
    );
    let (_, work) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(
                &child,
                owner_version(&authority, &child),
                WorkPermit::ResolveOnly,
            )
            .expect("the child Resolve capability checks out")
            .apply(),
    );
    let resource_rejection = work.resource_denied();

    apply_plan(
        authority
            .plan_terminalize_for_foundation(&parent, owner_version(&authority, &parent))
            .expect("parent terminalization publishes definitive loss"),
    );
    apply_plan(
        authority
            .apply_settlement(resource_rejection)
            .expect("the exact active capability settles after loss"),
    );
    assert!(matches!(
        authority.entry(&child),
        Some(OwnedTx::PreAccepted(entry))
            if matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
    ));

    assert_eq!(
        stale_reference_cut().classify(&ClaimSettlementNext::Rejected(
            ClaimSettlementRejection::ResourceBound,
        )),
        ClaimSettlementObservation::QueuedResolve,
        "the reference must apply baseline currentness before result-specific validity"
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettlementKind {
    QueuedVerify,
    Waiting,
    Ready,
    ChainRejected,
    ResourceRejected,
    VerificationRejected,
    Retry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettlementScenario {
    Current,
    StaleChain,
    StaleBaseline,
    TrustedPromotion,
}

struct ProductionSettlementCase {
    authority: TxPoolAuthority,
    transaction: TransactionView,
    dependency: crate::authority::state::DependencyKey,
    settlement: ComputeSettlement,
}

fn production_settlement_case(kind: SettlementKind, nonce: u32) -> ProductionSettlementCase {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let input = OutPoint::new(Byte32::new([nonce as u8; 32]), 0);
    let dependency = crate::authority::state::DependencyKey::Cell(input.clone());
    let transaction = TransactionBuilder::default()
        .version(nonce)
        .input(CellInput::new(input.clone(), 0))
        .build();
    let admission =
        ValidatedAdmission::remote(transaction.clone(), PeerIndex::from(nonce as usize))
            .expect("the settlement transaction is a valid remote admission");
    let hash = admission.identity.raw.clone();
    apply_plan(
        authority
            .plan_admission(admission)
            .expect("the settlement owner enters Resolve"),
    );
    let (_, resolve) = take_resolve_work(
        authority
            .plan_checkout_for_foundation(
                &hash,
                owner_version(&authority, &hash),
                WorkPermit::ResolveOnly,
            )
            .expect("the settlement Resolve capability checks out")
            .apply(),
    );
    let payload = || {
        resolved_payload_with_facts(
            &transaction,
            Vec::new(),
            vec![input.clone()],
            Capacity::shannons(1),
        )
    };
    let settlement = match kind {
        SettlementKind::QueuedVerify => resolve
            .yield_verify(payload())
            .expect("the resolved payload belongs to the checked-out owner"),
        SettlementKind::Waiting => resolve
            .missing(vec![dependency.clone()])
            .expect("the bounded missing receipt is valid"),
        SettlementKind::ChainRejected => resolve.rejected(RejectionKind::Policy),
        SettlementKind::ResourceRejected => resolve.resource_denied(),
        SettlementKind::Retry => resolve.internal_failure(),
        SettlementKind::Ready | SettlementKind::VerificationRejected => {
            apply_plan(
                authority
                    .apply_settlement(
                        resolve
                            .yield_verify(payload())
                            .expect("the verification payload belongs to the Resolve work"),
                    )
                    .expect("the resolved result queues verification"),
            );
            let checkout = authority
                .plan_checkout_for_foundation(
                    &hash,
                    owner_version(&authority, &hash),
                    WorkPermit::VerifyOnly(VerifyCapability::Any),
                )
                .expect("the settlement Verify capability checks out")
                .apply();
            let CheckedOutWork::Verify(verify) = checkout.into_work() else {
                panic!("the Verify-only permit returns Verify work");
            };
            match kind {
                SettlementKind::Ready => verify.verified(0),
                SettlementKind::VerificationRejected => {
                    verify.rejected(RejectionKind::Verification)
                }
                SettlementKind::QueuedVerify
                | SettlementKind::Waiting
                | SettlementKind::ChainRejected
                | SettlementKind::ResourceRejected
                | SettlementKind::Retry => unreachable!("the outer match fixes Verify outcomes"),
            }
        }
    };
    ProductionSettlementCase {
        authority,
        transaction,
        dependency,
        settlement,
    }
}

fn apply_settlement_scenario(case: &mut ProductionSettlementCase, scenario: SettlementScenario) {
    match scenario {
        SettlementScenario::Current => {}
        SettlementScenario::StaleChain => {
            case.authority
                .force_chain_view(crate::authority::state::ChainViewId::new(
                    crate::authority::state::ChainRevision(1),
                    Byte32::new([95; 32]),
                ))
        }
        SettlementScenario::StaleBaseline => apply_plan(
            case.authority
                .plan_dependency_loss_for_foundation(vec![case.dependency.clone()])
                .expect("the active baseline loss plans")
                .expect("the active dependency has one loss Apply"),
        ),
        SettlementScenario::TrustedPromotion => apply_plan(
            case.authority
                .plan_admission(
                    ValidatedAdmission::proposal(case.transaction.clone())
                        .expect("the same-witness trusted promotion is valid"),
                )
                .expect("the trusted promotion preserves the active capability"),
        ),
    }
}

fn settlement_claim_frontier(stale_baseline: bool) -> ClaimEvidenceFrontier {
    ClaimEvidenceFrontier::new(
        [(
            ClaimDependencyKey::cell(1),
            ClaimDependencyLevel::new(
                ClaimDependencyCut(if stale_baseline { 2 } else { 1 }),
                stale_baseline.then_some(ClaimDependencyCut(2)),
            )
            .expect("the claim baseline loss shares its change cut"),
        )],
        ClaimUnindexedDependencyLevel::new(Some(ClaimDependencyCut(1)), None)
            .expect("the claim global level is legal"),
    )
    .expect("the claim settlement has one dependency level")
}

fn claim_settlement_cut(scenario: SettlementScenario) -> ClaimSettlementCut {
    let identity = ClaimEvidenceIdentity {
        raw: ClaimRawTransaction(1),
        witness: 2,
    };
    ClaimSettlementCut {
        authority_view: ClaimEvidenceView(if scenario == SettlementScenario::StaleChain {
            2
        } else {
            1
        }),
        owner_identity: identity,
        baseline_dependencies: [ClaimDependencyKey::cell(1)].into_iter().collect(),
        current_policy: if scenario == SettlementScenario::TrustedPromotion {
            ClaimPayloadPolicy::Trusted
        } else {
            ClaimPayloadPolicy::RemoteDeclaredCycles(9)
        },
        active: ClaimSettlementOrigin {
            payload_identity: identity,
            view: ClaimEvidenceView(1),
            dependency_cut: ClaimDependencyCut(1),
            payload_policy: ClaimPayloadPolicy::RemoteDeclaredCycles(9),
        },
        frontier: settlement_claim_frontier(scenario == SettlementScenario::StaleBaseline),
    }
}

fn claim_settlement_evidence(cut: &ClaimSettlementCut) -> ClaimSettlementEvidence {
    cut.active
        .evidence([ClaimDependencyKey::cell(1)].into_iter().collect())
}

fn claim_settlement_next(kind: SettlementKind, cut: &ClaimSettlementCut) -> ClaimSettlementNext {
    match kind {
        SettlementKind::QueuedVerify => {
            ClaimSettlementNext::QueuedVerify(claim_settlement_evidence(cut))
        }
        SettlementKind::Waiting => ClaimSettlementNext::Waiting(ClaimMissingSettlement {
            dependencies: [ClaimDependencyKey::cell(1)].into_iter().collect(),
            missing: [ClaimDependencyKey::cell(1)].into_iter().collect(),
        }),
        SettlementKind::Ready => ClaimSettlementNext::Ready(claim_settlement_evidence(cut)),
        SettlementKind::ChainRejected => {
            ClaimSettlementNext::Rejected(ClaimSettlementRejection::ChainBound)
        }
        SettlementKind::ResourceRejected => {
            ClaimSettlementNext::Rejected(ClaimSettlementRejection::ResourceBound)
        }
        SettlementKind::VerificationRejected => {
            ClaimSettlementNext::VerificationRejected(claim_settlement_evidence(cut))
        }
        SettlementKind::Retry => ClaimSettlementNext::Retry,
    }
}

fn production_settlement_observation(
    result: Result<SettlementClassificationObservationForFoundation, PlanError>,
) -> ClaimSettlementObservation {
    match result {
        Ok(SettlementClassificationObservationForFoundation::QueuedResolve) => {
            ClaimSettlementObservation::QueuedResolve
        }
        Ok(SettlementClassificationObservationForFoundation::QueuedVerify) => {
            ClaimSettlementObservation::QueuedVerify
        }
        Ok(SettlementClassificationObservationForFoundation::Waiting) => {
            ClaimSettlementObservation::Waiting
        }
        Ok(SettlementClassificationObservationForFoundation::Ready) => {
            ClaimSettlementObservation::Ready
        }
        Ok(SettlementClassificationObservationForFoundation::Rejected) => {
            ClaimSettlementObservation::Rejected
        }
        Ok(
            SettlementClassificationObservationForFoundation::UnexpectedOwnerLocalWaiting
            | SettlementClassificationObservationForFoundation::UnexpectedOwnerLocalComputing,
        ) => panic!("the classifier produced an unmodeled owner-local phase"),
        Err(PlanError::Fault(AuthorityFault::MembershipProjection)) => {
            ClaimSettlementObservation::Fault(ClaimSettlementFault::MembershipProjection)
        }
        Err(PlanError::Fault(AuthorityFault::DependencyProjection)) => {
            ClaimSettlementObservation::Fault(ClaimSettlementFault::DependencyProjection)
        }
        other => panic!("unexpected production settlement observation: {other:?}"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PayloadPolicyEvolutionObservation {
    Unchanged,
    RemoteToTrusted,
    Invalid,
}

impl From<PayloadPolicyEvolution> for PayloadPolicyEvolutionObservation {
    fn from(value: PayloadPolicyEvolution) -> Self {
        match value {
            PayloadPolicyEvolution::Unchanged => Self::Unchanged,
            PayloadPolicyEvolution::RemoteToTrusted => Self::RemoteToTrusted,
            PayloadPolicyEvolution::Invalid => Self::Invalid,
        }
    }
}

impl From<ClaimPayloadPolicyEvolution> for PayloadPolicyEvolutionObservation {
    fn from(value: ClaimPayloadPolicyEvolution) -> Self {
        match value {
            ClaimPayloadPolicyEvolution::Unchanged => Self::Unchanged,
            ClaimPayloadPolicyEvolution::RemoteToTrusted => Self::RemoteToTrusted,
            ClaimPayloadPolicyEvolution::Invalid => Self::Invalid,
        }
    }
}

#[test]
fn uak_payload_policy_evolution_refines_every_declared_cycle_pair() {
    let policies = [
        (
            PayloadPolicy::remote_for_foundation(1),
            ClaimPayloadPolicy::RemoteDeclaredCycles(1),
        ),
        (
            PayloadPolicy::remote_for_foundation(2),
            ClaimPayloadPolicy::RemoteDeclaredCycles(2),
        ),
        (PayloadPolicy::Trusted, ClaimPayloadPolicy::Trusted),
    ];
    for (production_active, claim_active) in policies {
        for (production_current, claim_current) in policies {
            assert_eq!(
                PayloadPolicyEvolutionObservation::from(
                    production_active.evolution_to(production_current),
                ),
                PayloadPolicyEvolutionObservation::from(claim_active.evolution_to(claim_current),),
                "payload policy differs: active={claim_active:?}, current={claim_current:?}"
            );
        }
    }
}

#[test]
fn uak_verify_cycle_class_refines_payload_policy_and_threshold() {
    for declared in 0u8..=u8::MAX {
        for threshold in 0u8..=u8::MAX {
            let production = PayloadPolicy::remote_for_foundation(u64::from(declared))
                .verify_cycle_class(u64::from(threshold));
            let claim =
                ClaimPayloadPolicy::RemoteDeclaredCycles(declared).verify_cycle_class(threshold);
            assert_eq!(
                matches!(production, VerifyCycleClass::Large),
                matches!(claim, ClaimVerifyCycleClass::Large),
                "declared={declared}, threshold={threshold}"
            );
        }
    }
    for threshold in [0, 1, u64::MAX] {
        assert_eq!(
            PayloadPolicy::Trusted.verify_cycle_class(threshold),
            VerifyCycleClass::Small
        );
    }
    assert_eq!(
        PayloadPolicy::remote_for_foundation(u64::MAX).verify_cycle_class(u64::MAX),
        VerifyCycleClass::Small
    );
    assert_eq!(
        PayloadPolicy::remote_for_foundation(u64::MAX).verify_cycle_class(u64::MAX - 1),
        VerifyCycleClass::Large
    );
}

#[test]
fn uak_settlement_classifier_refines_every_legal_result_staleness_and_policy_cut() {
    let kinds = [
        SettlementKind::QueuedVerify,
        SettlementKind::Waiting,
        SettlementKind::Ready,
        SettlementKind::ChainRejected,
        SettlementKind::ResourceRejected,
        SettlementKind::VerificationRejected,
        SettlementKind::Retry,
    ];
    let mut nonce = 44_100u32;
    for kind in kinds {
        for scenario in [
            SettlementScenario::Current,
            SettlementScenario::StaleChain,
            SettlementScenario::StaleBaseline,
        ] {
            nonce += 1;
            let mut production = production_settlement_case(kind, nonce);
            apply_settlement_scenario(&mut production, scenario);
            let observed = production_settlement_observation(
                production
                    .authority
                    .classify_settlement_for_foundation(production.settlement),
            );
            let claim_cut = claim_settlement_cut(scenario);
            let claim = claim_cut.classify(&claim_settlement_next(kind, &claim_cut));
            assert_eq!(
                observed, claim,
                "settlement differs: kind={kind:?}, scenario={scenario:?}"
            );
        }
    }

    let kind = SettlementKind::VerificationRejected;
    let scenario = SettlementScenario::TrustedPromotion;
    let mut production = production_settlement_case(kind, nonce + 1);
    apply_settlement_scenario(&mut production, scenario);
    let observed = production_settlement_observation(
        production
            .authority
            .classify_settlement_for_foundation(production.settlement),
    );
    let claim_cut = claim_settlement_cut(scenario);
    let claim = claim_cut.classify(&claim_settlement_next(kind, &claim_cut));
    assert_eq!(observed, claim, "trusted promotion must reuse resolution");
}
