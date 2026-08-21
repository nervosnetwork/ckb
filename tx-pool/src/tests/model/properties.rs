use super::{
    boundaries::{
        ActiveVerificationAction, CallbackAccess, CallbackDisposition, CandidateUncleInput,
        ModelFailureDisposition, ModelFreshCapacityReceipt, ModelFreshDaoReceipt,
        ModelFreshTimeReceipt, ModelFreshVerificationReceipts, ModelMinimumFeeObservation,
        ModelOperationalFailure, ModelRecoveryAdmissionFailure, ModelRemoteCycleLimit,
        ModelRemoteCycleLimitError, ModelRemoteCycleObservation, ModelRemoteIngressRoute,
        ModelRetainedIngressOutcome, ModelScriptBehavior, ModelScriptExecution,
        ModelScriptRejection, ModelScriptReuse, ModelServiceFailure, ModelStructuralFault,
        ModelTxPoolResolutionReceipt, ModelTxPoolVerificationContext, ModelVerificationObservation,
        ModelVerificationRejection, QueryProjection, QueryStatus, QuerySubject,
        RelayLookupIdentity, TemplateDisposition, TemplateFailureCut, TemplateFailureProgress,
        TemplateLane, TemplateProtocol, VerificationControl, VerificationKey,
        authority_structural_failure, callback_disposition, chain_structural_failure,
        execute_model_script_vm, minimum_fee_observation, model_tx_pool_verification,
        persistence_projection, query_projection, query_subject, recovery_admission_disposition,
        relay_query_owner, remote_cycle_observation, reuse_model_script_proof,
        service_failure_disposition, template_failure_progress,
    },
    eviction_quotient::EvictionRefinementMetrics,
    handoff::{
        CapabilityTransport, CapabilityTransportDisposition, EndpointCircuit, EndpointEvent,
        RelayDisposition, RelayHandoff, RelayInvariantError, RelayItem, RelayLimits, RelayLocation,
        RelaySource, RelayTerminal,
    },
    kernel::{
        Admission, ChainTransition, Completion, DirectCompletion, DirectNegativeReason,
        DirectWorkResult, KernelCommand, KernelDisposition, KernelStep, ReadyCapture,
        ResolveContinuation, WorkResult, invariant_after_each,
    },
    permit::{
        FairPermitScheduler, PermitClass, PermitDomain, PermitGrant, PermitReleaseDisposition,
        PermitRequest, PermitRequestDisposition, PermitRequestId,
    },
    proposal::{ProposalBlock, ProposalContext, ProposalWindow},
    protocol::{
        DerivedComponent, DerivedHealth, KernelAccess, Lifecycle, PayloadCost, PayloadLocation,
        ProtocolLimits, RequestId, RequestKind, ResponseResult, SystemDisposition, SystemEvent,
        SystemInvariantError, SystemState,
    },
    resource::{
        AllocationObligation, AllocationTerminal, ComputeAdmission, ComputeGrant,
        EdgeMetadataBytes, EntryMetadataBytes, PayloadBytes, QueryCostInputs, QueryCostUpperBound,
        ResolvedResidentBytes, RetainedChargeInputs, ScratchDisposition,
        SharedVariableResidencyCost, SharedVariableResidencyNormalForm, TotalRetainedBytes,
        prepare_bounded_scratch, terminal_allocation_disposition,
    },
    state::{
        AcceptedStatus, ApplyStamp, CapabilityId, CellId, ChainView, DirectKind, DirectRequestId,
        EffectBatchBound, EffectCapacity, EffectClaimSource, EffectUsage, EntryVersion, HeaderId,
        LogicalEffect, MembershipRejection, MissingDependencies, ModelFeeRate, ModelInvariantError,
        ModelLimits, ModelReplacementPolicy, MonotonicTick, Omega, OwnerLocation, PeerId,
        PoolGeneration, ProposalBase, ProposalId, RemoteDeadline, RemoteResidency,
        ResolvedEvidence, ResourceVector, RetainedOwner, RetainedPhase, RetainedSource, RulesId,
        Transaction, TxId, VerifyCapability, VerifyCycleClass, ViewId, WitnessId, WorkKind,
        WorkPermit, WorkStage,
    },
    two_phase::{
        CanonicalBlockService, CanonicalServicePremise, ConditionalSelectionInput,
        CurrentTemplateComposition, DependencyScanInput, ModelTemplatePackingLimits,
        StableAcceptedCohort, StableAcceptedCohortError, TEMPLATE_DESCENDANT_CACHE_MEMBER_BOUND,
        TEMPLATE_PACKING_FAILURE_BOUND, TemplatePackingInput, TemplateServicePremise,
        TemplateServiceSourceCut, TwoPhaseLiveness, complete_dependency_scan_refinement,
        conditional_template_selection_refinement, template_packing_refinement,
    },
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::num::NonZeroU16;

fn model() -> Omega {
    let limits = ModelLimits::small()
        .validate()
        .expect("the model fixture uses a valid startup configuration");
    Omega::new(limits, ViewId(1), RulesId(1))
}

fn model_with_accepted_limit(entries: u16, bytes: u32) -> Omega {
    let mut limits = ModelLimits::small();
    limits.accepted = ResourceVector {
        entries,
        bytes,
        edges: limits.accepted.edges,
    };
    Omega::new(
        limits
            .validate()
            .expect("the accepted-capacity fixture is internally bounded"),
        ViewId(1),
        RulesId(1),
    )
}

fn exact_model_template_premise(
    authority: &Omega,
    proposal_limit: usize,
    transaction_bytes: usize,
    candidate_uncles: Vec<CandidateUncleInput>,
) -> TemplateServicePremise {
    const PROPOSAL_BYTES: usize = 1_024;
    let accepted_count = authority
        .authority
        .owners
        .values()
        .filter(|owner| matches!(owner.location, OwnerLocation::Accepted { .. }))
        .count();
    let proposal_bytes = proposal_limit
        .min(accepted_count)
        .checked_mul(PROPOSAL_BYTES)
        .expect("the finite model proposal prefix fits usize");
    let uncle_bytes = candidate_uncles
        .iter()
        .try_fold(0usize, |total, uncle| {
            total.checked_add(uncle.serialized_bytes)
        })
        .expect("the finite model uncle prefix fits usize");
    let max_block_bytes = proposal_bytes
        .checked_add(uncle_bytes)
        .expect("the finite model optional content fits usize")
        .checked_add(transaction_bytes)
        .expect("the finite model template fits usize");
    let composition = CurrentTemplateComposition::new(
        proposal_limit,
        PROPOSAL_BYTES,
        candidate_uncles,
        0,
        max_block_bytes,
        u64::MAX,
    );
    TemplateServicePremise::compile(
        TemplateServiceSourceCut::from_accepted_authority(authority, composition)
            .expect("the accepted model cut projects one exact template source"),
    )
    .expect("the exact source cut has positive proposal and commit capacity")
}

fn remote(transaction: Transaction, peer: u8) -> KernelCommand {
    remote_at(transaction, peer, 0)
}

fn remote_at(transaction: Transaction, peer: u8, observed_at: u64) -> KernelCommand {
    remote_until_at(transaction, peer, u64::MAX, observed_at)
}

fn remote_until(transaction: Transaction, peer: u8, expires_at_wall: u64) -> KernelCommand {
    remote_until_at(transaction, peer, expires_at_wall, 0)
}

fn remote_until_at(
    transaction: Transaction,
    peer: u8,
    expires_at_wall: u64,
    observed_at: u64,
) -> KernelCommand {
    KernelCommand::Admit(Admission {
        transaction,
        source: remote_source(peer, expires_at_wall),
        observed_at: MonotonicTick(observed_at),
    })
}

fn remote_source(peer: u8, expires_at_wall: u64) -> RetainedSource {
    RetainedSource::Remote(RemoteResidency::new(
        PeerId(peer),
        RemoteDeadline(expires_at_wall),
    ))
}

fn retained(transaction: Transaction, source: RetainedSource) -> KernelCommand {
    KernelCommand::Admit(Admission {
        transaction,
        source,
        observed_at: MonotonicTick(0),
    })
}

fn missing(transaction: &Transaction, cells: BTreeSet<CellId>) -> MissingDependencies {
    MissingDependencies::for_transaction(transaction, cells)
        .expect("the model fixture names a non-empty transaction dependency subset")
}

fn reconcile_view(from: ChainView, view: ViewId) -> KernelCommand {
    reconcile_proposals(from, view, BTreeSet::new(), BTreeSet::new())
}

fn reconcile_proposals(
    from: ChainView,
    view: ViewId,
    proposed: BTreeSet<ProposalId>,
    gap: BTreeSet<ProposalId>,
) -> KernelCommand {
    KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(
            super::state::RulesId(1),
            false,
        ),
        from,
        to_tip: view,
        committed: BTreeSet::new(),
        available_cells: BTreeSet::new(),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::new(),
        conflicting_cells: BTreeSet::new(),
        recovered: Vec::new(),
        proposals: super::proposal::ProposalContext::status_witness(proposed, gap)
            .expect("the proposal witness sets are disjoint")
            .view(),
    })
}

fn checked_out(step: KernelStep) -> CapabilityId {
    match step {
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::CheckedOut(capability),
            ..
        } => capability.id,
        other => panic!("expected checkout, got {other:?}"),
    }
}

fn direct_checked_out(step: KernelStep) -> CapabilityId {
    match step {
        KernelStep::NoAuthorityCommit(KernelDisposition::DirectCheckedOut(capability)) => {
            capability.id
        }
        other => panic!("expected direct checkout, got {other:?}"),
    }
}

fn drive_ready(omega: &mut Omega, transaction: &Transaction, peer: u8) {
    drive_ready_from(omega, transaction, remote_source(peer, u64::MAX));
}

fn drive_ready_from(omega: &mut Omega, transaction: &Transaction, source: RetainedSource) {
    let admitted = omega.kernel_step(retained(transaction.clone(), source));
    assert_eq!(
        admitted.disposition(),
        &KernelDisposition::Retained(transaction.id)
    );

    let resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    let evidence = ResolvedEvidence::for_transaction(
        transaction,
        omega.authority.chain,
        omega.authority.rules,
    )
    .expect("direct transaction has no dep-group expansion");
    let resolved = omega.kernel_step(KernelCommand::Complete(Completion {
        capability: resolve,
        result: WorkResult::Resolved(evidence),
    }));
    assert_eq!(
        resolved.disposition(),
        &KernelDisposition::Continued(transaction.id)
    );

    let verify = checked_out(omega.kernel_step(KernelCommand::Checkout));
    complete_verify(omega, transaction, verify);
}

fn complete_verify(omega: &mut Omega, transaction: &Transaction, verify: CapabilityId) {
    let verified = omega.kernel_step(KernelCommand::Complete(Completion {
        capability: verify,
        result: WorkResult::Verified,
    }));
    assert_eq!(
        verified.disposition(),
        &KernelDisposition::Ready(transaction.id)
    );
}

fn drive_ready_with_evidence(
    omega: &mut Omega,
    transaction: &Transaction,
    source: RetainedSource,
    evidence: ResolvedEvidence,
) {
    let admitted = omega.kernel_step(retained(transaction.clone(), source));
    assert_eq!(
        admitted.disposition(),
        &KernelDisposition::Retained(transaction.id)
    );
    let resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    assert_eq!(
        omega
            .kernel_step(KernelCommand::Complete(Completion {
                capability: resolve,
                result: WorkResult::Resolved(evidence),
            }))
            .disposition(),
        &KernelDisposition::Continued(transaction.id)
    );
    let verify = checked_out(omega.kernel_step(KernelCommand::Checkout));
    complete_verify(omega, transaction, verify);
}

fn accept(omega: &mut Omega, transaction: &Transaction, peer: u8, wall_time: u64) {
    drive_ready(omega, transaction, peer);
    assert!(matches!(
        omega.kernel_step(KernelCommand::FinalizeNext { wall_time }),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::Accepted(_),
            ..
        }
    ));
}

fn accept_with_evidence(
    omega: &mut Omega,
    transaction: &Transaction,
    evidence: ResolvedEvidence,
    wall_time: u64,
) {
    drive_ready_with_evidence(omega, transaction, RetainedSource::Proposal, evidence);
    assert!(matches!(
        omega.kernel_step(KernelCommand::FinalizeNext { wall_time }),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::Accepted(_),
            ..
        }
    ));
}

fn ready_capture(step: KernelStep) -> ReadyCapture {
    match step {
        KernelStep::NoAuthorityCommit(KernelDisposition::ReadyCaptured(capture)) => capture,
        other => panic!("expected ready capture, got {other:?}"),
    }
}

fn running_system() -> SystemState {
    let mut system = SystemState::constructing(ProtocolLimits::small());
    assert_eq!(
        system.step(SystemEvent::Assemble {
            limits: ModelLimits::small(),
            view: ViewId(1),
            rules: RulesId(1),
            succeed: true,
        }),
        SystemDisposition::Assembled
    );
    assert_eq!(system.step(SystemEvent::Ready), SystemDisposition::Running);
    system
}

#[test]
fn model_sequential_lifecycle_preserves_owner_charge_capability_and_effect_laws() {
    let transaction = Transaction::independent(1, 1, 10, 20);
    let mut omega = model();
    drive_ready(&mut omega, &transaction, 7);
    assert_eq!(omega.check_invariants(), Ok(()));

    let accepted = omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 100 });
    assert_eq!(
        accepted.disposition(),
        &KernelDisposition::Accepted(TxId(1))
    );
    assert_eq!(omega.check_invariants(), Ok(()));
    assert!(matches!(
        omega
            .authority
            .owners
            .get(&TxId(1))
            .map(|owner| &owner.location),
        Some(OwnerLocation::Accepted { .. })
    ));

    let claim = match omega.kernel_step(KernelCommand::ClaimEffect) {
        KernelStep::NoAuthorityCommit(KernelDisposition::EffectClaimed(claim)) => claim,
        other => panic!("expected effect claim, got {other:?}"),
    };
    assert_eq!(omega.check_invariants(), Ok(()));
    let settled = omega.kernel_step(KernelCommand::SettleEffect(claim));
    assert_eq!(
        settled.disposition(),
        &KernelDisposition::EffectSettled(claim)
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_cold_retained_lifecycle_exposes_the_exact_sequential_apply_cost() {
    fn record(stamps: &mut Vec<ApplyStamp>, step: KernelStep) {
        if let KernelStep::AuthorityCommit { stamp, .. } = step {
            stamps.push(stamp);
        }
    }

    let transaction = Transaction::independent(1, 1, 10, 20);
    let mut omega = model();
    let mut stamps = Vec::new();

    record(
        &mut stamps,
        omega.kernel_step(remote(transaction.clone(), 7)),
    );
    let resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    stamps.push(omega.authority.last_apply);
    record(
        &mut stamps,
        omega.kernel_step(KernelCommand::Complete(Completion {
            capability: resolve,
            result: WorkResult::Resolved(
                ResolvedEvidence::for_transaction(
                    &transaction,
                    omega.authority.chain,
                    omega.authority.rules,
                )
                .expect("direct transaction has no dep-group expansion"),
            ),
        })),
    );
    let verify = checked_out(omega.kernel_step(KernelCommand::Checkout));
    stamps.push(omega.authority.last_apply);
    record(
        &mut stamps,
        omega.kernel_step(KernelCommand::Complete(Completion {
            capability: verify,
            result: WorkResult::Verified,
        })),
    );
    record(
        &mut stamps,
        omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 10 }),
    );
    let claim = match omega.kernel_step(KernelCommand::ClaimEffect) {
        KernelStep::NoAuthorityCommit(KernelDisposition::EffectClaimed(claim)) => claim,
        other => panic!("expected effect claim, got {other:?}"),
    };
    record(
        &mut stamps,
        omega.kernel_step(KernelCommand::SettleEffect(claim)),
    );

    assert_eq!(
        stamps,
        (1..=7).map(ApplyStamp).collect::<Vec<_>>(),
        "the sequential oracle exposes every current lifecycle Apply; M2 may refine commuting steps into fewer Applies but may not hide the baseline cost"
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_continuous_resolve_verify_preserves_one_capability_and_exact_apply_cost() {
    let transaction = Transaction::independent(1, 1, 10, 20);
    let mut continuous = model();

    assert!(matches!(
        continuous.kernel_step(remote(transaction.clone(), 7)),
        KernelStep::AuthorityCommit {
            stamp: ApplyStamp(1),
            disposition: KernelDisposition::Retained(TxId(1)),
        }
    ));
    let capability =
        match continuous.kernel_step(KernelCommand::CheckoutContinuous(VerifyCapability::Any)) {
            KernelStep::AuthorityCommit {
                stamp: ApplyStamp(2),
                disposition: KernelDisposition::CheckedOut(capability),
            } => capability,
            other => panic!("expected continuous resolve checkout, got {other:?}"),
        };
    assert_eq!(
        capability.permit(),
        WorkPermit::ResolveThenVerify(VerifyCapability::Any)
    );
    assert!(matches!(capability.stage(), WorkStage::Resolve));

    let evidence = ResolvedEvidence::for_transaction(
        &transaction,
        continuous.authority.chain,
        continuous.authority.rules,
    )
    .expect("direct transaction has no dep-group expansion");
    let authority_before_continuation = continuous.authority.clone();
    let linear_permits_before_continuation = continuous.linear.free_compute_permits;
    let continued = continuous.kernel_step(KernelCommand::ContinueResolveThenVerify(
        ResolveContinuation {
            capability: capability.id,
            evidence: evidence.clone(),
        },
    ));
    let updated = match continued {
        KernelStep::NoAuthorityCommit(KernelDisposition::ResolveContinued(capability)) => {
            capability
        }
        other => panic!("expected a no-Apply resolve continuation, got {other:?}"),
    };
    assert_eq!(continuous.authority, authority_before_continuation);
    assert_eq!(updated.id, capability.id);
    assert_eq!(
        updated.permit(),
        WorkPermit::ResolveThenVerify(VerifyCapability::Any)
    );
    assert!(matches!(
        updated.stage(),
        WorkStage::Verify(current)
            if **current == evidence && current.dependency_cut == capability.dependency_cut
    ));
    assert_eq!(continuous.linear.work.get(&capability.id), Some(&updated));
    assert_eq!(
        continuous.linear.free_compute_permits,
        linear_permits_before_continuation
    );
    assert_eq!(
        query_subject(&continuous, transaction.id),
        QuerySubject::PreAcceptedPending,
        "the authority-visible continuous permit stays conservatively Pending"
    );
    assert_eq!(continuous.check_invariants(), Ok(()));

    assert!(matches!(
        continuous.kernel_step(KernelCommand::Complete(Completion {
            capability: capability.id,
            result: WorkResult::Verified,
        })),
        KernelStep::AuthorityCommit {
            stamp: ApplyStamp(3),
            disposition: KernelDisposition::Ready(TxId(1)),
        }
    ));
    assert!(matches!(
        continuous.kernel_step(KernelCommand::FinalizeNext { wall_time: 10 }),
        KernelStep::AuthorityCommit {
            stamp: ApplyStamp(4),
            disposition: KernelDisposition::Accepted(TxId(1)),
        }
    ));
    let claim = match continuous.kernel_step(KernelCommand::ClaimEffect) {
        KernelStep::NoAuthorityCommit(KernelDisposition::EffectClaimed(claim)) => claim,
        other => panic!("expected committed effect claim, got {other:?}"),
    };
    assert!(matches!(
        continuous.kernel_step(KernelCommand::SettleEffect(claim)),
        KernelStep::AuthorityCommit {
            stamp: ApplyStamp(5),
            disposition: KernelDisposition::EffectSettled(settled),
        } if settled == claim
    ));

    let mut split = model();
    drive_ready(&mut split, &transaction, 7);
    assert!(matches!(
        split.kernel_step(KernelCommand::FinalizeNext { wall_time: 10 }),
        KernelStep::AuthorityCommit {
            stamp: ApplyStamp(6),
            disposition: KernelDisposition::Accepted(TxId(1)),
        }
    ));
    let split_claim = match split.kernel_step(KernelCommand::ClaimEffect) {
        KernelStep::NoAuthorityCommit(KernelDisposition::EffectClaimed(claim)) => claim,
        other => panic!("expected split-route effect claim, got {other:?}"),
    };
    assert!(matches!(
        split.kernel_step(KernelCommand::SettleEffect(split_claim)),
        KernelStep::AuthorityCommit {
            stamp: ApplyStamp(7),
            disposition: KernelDisposition::EffectSettled(settled),
        } if settled == split_claim
    ));

    assert_eq!(continuous.authority.last_apply, ApplyStamp(5));
    assert_eq!(split.authority.last_apply, ApplyStamp(7));
    assert_eq!(
        continuous.authority.owners.keys().collect::<Vec<_>>(),
        split.authority.owners.keys().collect::<Vec<_>>()
    );
    for (transaction, continuous_owner) in &continuous.authority.owners {
        let Some(split_owner) = split.authority.owners.get(transaction) else {
            panic!("split route lost owner {transaction:?}");
        };
        assert_eq!(continuous_owner.arrival, split_owner.arrival);
        assert_eq!(continuous_owner.transaction, split_owner.transaction);
        assert_eq!(continuous_owner.location, split_owner.location);
    }
    assert_ne!(
        continuous.authority.owners[&transaction.id].version,
        split.authority.owners[&transaction.id].version,
        "opaque owner-incarnation names reflect the two extra split transitions"
    );
    assert_eq!(continuous.authority.effects, split.authority.effects);
    assert_eq!(
        continuous.authority.latest_generation_reset,
        split.authority.latest_generation_reset
    );
    assert_eq!(
        continuous.linear.free_compute_permits,
        split.linear.free_compute_permits
    );
    assert!(continuous.linear.work.is_empty() && split.linear.work.is_empty());
    assert!(continuous.linear.finished_work.is_empty() && split.linear.finished_work.is_empty());
    assert_eq!(continuous.linear.next_capability, 2);
    assert_eq!(split.linear.next_capability, 3);
    assert_eq!(continuous.check_invariants(), Ok(()));
    assert_eq!(split.check_invariants(), Ok(()));
}

#[test]
fn model_continuous_resolve_falls_back_when_worker_cannot_verify_the_resolved_class() {
    let transaction = Transaction::independent(1, 1, 10, 20);
    let mut omega = model();
    assert!(matches!(
        omega.kernel_step(remote(transaction.clone(), 7)),
        KernelStep::AuthorityCommit {
            stamp: ApplyStamp(1),
            disposition: KernelDisposition::Retained(TxId(1)),
        }
    ));
    let continuous = checked_out(omega.kernel_step(KernelCommand::CheckoutContinuous(
        VerifyCapability::SmallCycleOnly,
    )));
    assert_eq!(omega.authority.last_apply, ApplyStamp(2));

    let evidence = ResolvedEvidence::for_transaction(
        &transaction,
        omega.authority.chain,
        omega.authority.rules,
    )
    .expect("direct transaction has no dep-group expansion")
    .with_verify_class(VerifyCycleClass::Large);
    assert_eq!(evidence.verify_class, VerifyCycleClass::Large);
    assert!(matches!(
        omega.kernel_step(KernelCommand::Complete(Completion {
            capability: continuous,
            result: WorkResult::Resolved(evidence.clone()),
        })),
        KernelStep::AuthorityCommit {
            stamp: ApplyStamp(3),
            disposition: KernelDisposition::Continued(TxId(1)),
        }
    ));
    assert!(matches!(
        &omega.authority.owners[&TxId(1)].location,
        OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Queued(WorkStage::Verify(queued)),
            ..
        }) if **queued == evidence
    ));
    assert!(!omega.linear.work.contains_key(&continuous));
    assert_eq!(
        omega.linear.free_compute_permits,
        omega.authority.limits.compute_permits
    );

    let verify = match omega.kernel_step(KernelCommand::Checkout) {
        KernelStep::AuthorityCommit {
            stamp: ApplyStamp(4),
            disposition: KernelDisposition::CheckedOut(capability),
        } => capability,
        other => panic!("expected split verify checkout, got {other:?}"),
    };
    assert_eq!(
        verify.permit(),
        WorkPermit::VerifyOnly(VerifyCapability::Any)
    );
    assert!(matches!(verify.stage(), WorkStage::Verify(current) if **current == evidence));
    assert!(matches!(
        omega.kernel_step(KernelCommand::Complete(Completion {
            capability: verify.id,
            result: WorkResult::Verified,
        })),
        KernelStep::AuthorityCommit {
            stamp: ApplyStamp(5),
            disposition: KernelDisposition::Ready(TxId(1)),
        }
    ));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_resolve_continuation_rejects_incompatible_or_stale_evidence_without_mutation() {
    let transaction = Transaction::independent(1, 1, 10, 20);

    let mut split = model();
    split.kernel_step(remote(transaction.clone(), 7));
    let split_capability = checked_out(split.kernel_step(KernelCommand::Checkout));
    let split_before = split.clone();
    let current_evidence = ResolvedEvidence::for_transaction(
        &transaction,
        split.authority.chain,
        split.authority.rules,
    )
    .expect("direct transaction has no dep-group expansion");
    assert_eq!(
        split.kernel_step(KernelCommand::ContinueResolveThenVerify(
            ResolveContinuation {
                capability: split_capability,
                evidence: current_evidence,
            },
        )),
        KernelStep::NoAuthorityCommit(KernelDisposition::WorkTransitionRejected(split_capability))
    );
    assert_eq!(split, split_before);

    let large = transaction.clone();
    let mut incompatible = model();
    incompatible.kernel_step(remote(large.clone(), 7));
    let incompatible_capability = checked_out(incompatible.kernel_step(
        KernelCommand::CheckoutContinuous(VerifyCapability::SmallCycleOnly),
    ));
    let incompatible_before = incompatible.clone();
    assert_eq!(
        incompatible.kernel_step(KernelCommand::ContinueResolveThenVerify(
            ResolveContinuation {
                capability: incompatible_capability,
                evidence: ResolvedEvidence::for_transaction(
                    &large,
                    incompatible.authority.chain,
                    incompatible.authority.rules,
                )
                .expect("the direct transaction has no dep-group expansion")
                .with_verify_class(VerifyCycleClass::Large),
            },
        )),
        KernelStep::NoAuthorityCommit(KernelDisposition::WorkTransitionRejected(
            incompatible_capability
        ))
    );
    assert_eq!(incompatible, incompatible_before);

    let mut continuous = model();
    continuous.kernel_step(remote(transaction.clone(), 7));
    let continuous_capability = checked_out(
        continuous.kernel_step(KernelCommand::CheckoutContinuous(VerifyCapability::Any)),
    );
    let continuous_before = continuous.clone();
    let stale_evidence = ResolvedEvidence::for_transaction(
        &transaction,
        ChainView {
            tip: ViewId(99),
            revision: continuous.authority.chain.revision,
        },
        continuous.authority.rules,
    )
    .expect("direct transaction has no dep-group expansion");
    assert_eq!(
        continuous.kernel_step(KernelCommand::ContinueResolveThenVerify(
            ResolveContinuation {
                capability: continuous_capability,
                evidence: stale_evidence,
            },
        )),
        KernelStep::NoAuthorityCommit(KernelDisposition::WorkTransitionRejected(
            continuous_capability
        ))
    );
    assert_eq!(continuous, continuous_before);
    assert_eq!(continuous.check_invariants(), Ok(()));
}

#[test]
fn model_chain_bound_completion_after_revision_requeues_exact_owner() {
    let transaction = Transaction::independent(1, 1, 10, 20);
    let mut omega = model();
    omega.kernel_step(remote(transaction.clone(), 7));
    let capability = checked_out(omega.kernel_step(KernelCommand::Checkout));
    let before_owner = omega.authority.owners.get(&TxId(1)).cloned();

    omega.kernel_step(reconcile_view(omega.authority.chain, ViewId(2)));
    let active = omega.authority.owners.get(&TxId(1)).cloned();
    assert_eq!(before_owner, active);
    assert!(matches!(
        active.map(|owner| owner.location),
        Some(OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Computing(active),
            ..
        })) if active.permit == WorkPermit::ResolveOnly
    ));

    let settled = omega.kernel_step(KernelCommand::Complete(Completion {
        capability,
        result: WorkResult::resolve_rejected(),
    }));
    assert_eq!(
        settled.disposition(),
        &KernelDisposition::Continued(transaction.id)
    );
    assert!(matches!(
        omega.authority.owners[&transaction.id].location,
        OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Queued(WorkStage::Resolve),
            ..
        })
    ));
    assert!(!omega.linear.work.contains_key(&capability));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_chain_advance_preserves_finished_capability_until_settlement_or_cancel() {
    let transaction = Transaction::independent(1, 1, 10, 20);
    let mut omega = model();
    omega.kernel_step(remote(transaction.clone(), 7));
    let capability = checked_out(omega.kernel_step(KernelCommand::Checkout));
    let evidence = ResolvedEvidence::for_transaction(
        &transaction,
        omega.authority.chain,
        omega.authority.rules,
    )
    .expect("direct transaction has no dep-group expansion");
    assert_eq!(
        omega.kernel_step(KernelCommand::FinishExecution(Completion {
            capability,
            result: WorkResult::Resolved(evidence),
        })),
        KernelStep::NoAuthorityCommit(KernelDisposition::Finished(capability))
    );
    assert!(omega.linear.work.is_empty());
    assert!(omega.linear.finished_work.contains_key(&capability));
    let released_permits = omega.linear.free_compute_permits;

    assert!(matches!(
        omega.kernel_step(reconcile_view(omega.authority.chain, ViewId(2))),
        KernelStep::AuthorityCommit { .. }
    ));
    assert!(matches!(
        omega.authority.owners[&transaction.id].location,
        OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Computing(ref active),
            ..
        }) if active.permit == WorkPermit::ResolveOnly
    ));
    assert!(omega.linear.finished_work.contains_key(&capability));
    assert_eq!(omega.check_invariants(), Ok(()));

    let mut settled = omega.clone();
    assert_eq!(
        settled
            .kernel_step(KernelCommand::SettleFinished(capability))
            .disposition(),
        &KernelDisposition::Continued(transaction.id)
    );
    assert_eq!(settled.linear.free_compute_permits, released_permits);
    assert!(settled.linear.work.is_empty());
    assert!(settled.linear.finished_work.is_empty());
    assert!(matches!(
        settled.authority.owners[&transaction.id].location,
        OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Queued(WorkStage::Resolve),
            ..
        })
    ));
    assert_eq!(settled.check_invariants(), Ok(()));

    let mut cancelled = omega;
    assert_eq!(
        cancelled
            .kernel_step(KernelCommand::CancelCapability(capability))
            .disposition(),
        &KernelDisposition::Removed(vec![transaction.id])
    );
    assert_eq!(cancelled.linear.free_compute_permits, released_permits);
    assert!(cancelled.linear.work.is_empty());
    assert!(cancelled.linear.finished_work.is_empty());
    assert!(!cancelled.authority.owners.contains_key(&transaction.id));
    assert_eq!(cancelled.check_invariants(), Ok(()));
}

#[test]
fn model_chain_revision_prevents_view_hash_aba_from_validating_chain_bound_work() {
    let transaction = Transaction::independent(1, 1, 10, 20);
    let mut omega = model();
    omega.kernel_step(remote(transaction.clone(), 7));
    let capability = checked_out(omega.kernel_step(KernelCommand::Checkout));
    omega.kernel_step(reconcile_view(omega.authority.chain, ViewId(2)));
    omega.kernel_step(reconcile_view(omega.authority.chain, ViewId(1)));
    assert_eq!(omega.authority.chain.revision.0, 2);
    assert_eq!(
        omega
            .kernel_step(KernelCommand::Complete(Completion {
                capability,
                result: WorkResult::resolve_rejected(),
            }))
            .disposition(),
        &KernelDisposition::Continued(transaction.id)
    );
    assert!(matches!(
        omega.authority.owners[&transaction.id].location,
        OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Queued(WorkStage::Resolve),
            ..
        })
    ));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_chain_cut_allows_same_tip_progress_but_rejects_a_stale_receipt() {
    let retained = Transaction::independent(1, 1, 10, 20);
    let recovery = Transaction::independent(2, 2, 11, 21);
    let mut omega = model();
    accept(&mut omega, &retained, 7, 10);
    let original = omega.authority.chain;

    let first = omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(
            super::state::RulesId(1),
            false,
        ),
        from: original,
        to_tip: original.tip,
        committed: BTreeSet::new(),
        available_cells: BTreeSet::new(),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::new(),
        conflicting_cells: BTreeSet::new(),
        recovered: Vec::new(),
        proposals: super::proposal::ProposalContext::status_witness(
            BTreeSet::new(),
            BTreeSet::from([retained.proposal]),
        )
        .expect("the proposal witness sets are disjoint")
        .view(),
    }));
    assert!(matches!(first, KernelStep::AuthorityCommit { .. }));
    assert_eq!(omega.authority.chain.tip, original.tip);
    assert!(omega.authority.chain.revision > original.revision);
    let after_first = omega.clone();

    let stale = omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(
            super::state::RulesId(1),
            true,
        ),
        from: original,
        to_tip: original.tip,
        committed: BTreeSet::from([retained.id]),
        available_cells: BTreeSet::new(),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::new(),
        conflicting_cells: BTreeSet::from([CellId(10)]),
        recovered: vec![recovery],
        proposals: super::proposal::ProposalContext::status_witness(
            BTreeSet::from([retained.proposal]),
            BTreeSet::new(),
        )
        .expect("the proposal witness sets are disjoint")
        .view(),
    }));

    assert_eq!(
        stale,
        KernelStep::NoAuthorityCommit(KernelDisposition::StaleChainTransition {
            expected: original,
            actual: after_first.authority.chain,
        })
    );
    assert_eq!(omega, after_first);

    let second = omega.kernel_step(reconcile_view(omega.authority.chain, original.tip));
    assert!(matches!(second, KernelStep::AuthorityCommit { .. }));
    assert_eq!(omega.authority.chain.tip, original.tip);
    assert!(omega.authority.chain.revision > after_first.authority.chain.revision);
    assert!(matches!(
        omega.authority.owners[&retained.id].location,
        OwnerLocation::Accepted { .. }
    ));
    assert_eq!(omega.proposal_status(&retained), AcceptedStatus::Pending);
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_queued_verify_evidence_is_lazily_requeued_after_a_chain_advance() {
    let mut transaction = Transaction::independent(1, 1, 10, 20);
    transaction.header_deps.insert(HeaderId(1));
    let mut omega = model();
    assert!(matches!(
        omega.kernel_step(remote(transaction.clone(), 7)),
        KernelStep::AuthorityCommit { .. }
    ));
    let resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    let old_chain = omega.authority.chain;
    let evidence =
        ResolvedEvidence::for_transaction(&transaction, old_chain, omega.authority.rules)
            .expect("direct transaction has no dep-group expansion");
    assert_eq!(
        omega
            .kernel_step(KernelCommand::Complete(Completion {
                capability: resolve,
                result: WorkResult::Resolved(evidence),
            }))
            .disposition(),
        &KernelDisposition::Continued(transaction.id)
    );
    assert!(matches!(
        omega.authority.owners[&transaction.id].location,
        OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Queued(WorkStage::Verify(_)),
            ..
        })
    ));

    assert!(matches!(
        omega.kernel_step(reconcile_view(old_chain, ViewId(2))),
        KernelStep::AuthorityCommit { .. }
    ));
    let permits = omega.linear.free_compute_permits;
    assert_eq!(
        omega.kernel_step(KernelCommand::Checkout),
        KernelStep::AuthorityCommit {
            stamp: omega.authority.last_apply,
            disposition: KernelDisposition::Continued(transaction.id),
        }
    );
    assert_eq!(omega.linear.free_compute_permits, permits);
    assert!(omega.linear.work.is_empty());
    assert!(matches!(
        omega.authority.owners[&transaction.id].location,
        OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Queued(WorkStage::Resolve),
            ..
        })
    ));

    let KernelStep::AuthorityCommit {
        disposition: KernelDisposition::CheckedOut(capability),
        ..
    } = omega.kernel_step(KernelCommand::Checkout)
    else {
        panic!("the lazily requeued owner must be checked out for resolve");
    };
    assert_eq!(capability.kind(), WorkKind::Resolve);
    assert_eq!(capability.chain, omega.authority.chain);
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_header_dependencies_are_chain_only_evidence_and_charged_edges() {
    let mut transaction = Transaction::independent(1, 1, 10, 20);
    transaction.header_deps = BTreeSet::from([HeaderId(1), HeaderId(2)]);
    let charge = transaction
        .charge()
        .expect("the bounded fixture has a representable resource charge");
    assert_eq!(charge.edges, 3);

    let omega = model();
    let evidence = ResolvedEvidence::for_transaction(
        &transaction,
        omega.authority.chain,
        omega.authority.rules,
    )
    .expect("direct transaction has no dep-group expansion");
    assert_eq!(evidence.header_deps, transaction.header_deps);

    let mut different_headers = transaction;
    different_headers.header_deps.insert(HeaderId(3));
    assert!(!evidence.has_transaction_shape(&different_headers, omega.authority.rules));
}

#[test]
fn model_ready_membership_revalidates_a_chain_to_pool_origin_change() {
    let child = Transaction::dependent(2, 2, 20, 30);
    let parent = Transaction::independent(1, 1, 10, 20);
    let mut omega = model();
    drive_ready(&mut omega, &child, 7);

    let parent_capability = direct_checked_out(omega.kernel_step(KernelCommand::BeginDirect {
        request: DirectRequestId(1),
        kind: DirectKind::Local,
        transaction: parent.clone(),
    }));
    let parent_evidence =
        ResolvedEvidence::for_transaction(&parent, omega.authority.chain, omega.authority.rules)
            .expect("direct transaction has no dep-group expansion");
    assert_eq!(
        omega
            .kernel_step(KernelCommand::CompleteDirect(DirectCompletion {
                capability: parent_capability,
                wall_time: 10,
                result: DirectWorkResult::Verified(parent_evidence),
            }))
            .disposition(),
        &KernelDisposition::DirectValid(DirectRequestId(1))
    );

    assert_eq!(
        omega
            .kernel_step(KernelCommand::FinalizeNext { wall_time: 20 })
            .disposition(),
        &KernelDisposition::Continued(child.id)
    );
    assert!(matches!(
        omega.authority.owners[&child.id].location,
        OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Queued(WorkStage::Resolve),
            ..
        })
    ));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_direct_membership_revalidates_a_chain_to_pool_origin_change() {
    let child = Transaction::dependent(2, 2, 20, 30);
    let parent = Transaction::independent(1, 1, 10, 20);
    let mut omega = model();
    let child_capability = direct_checked_out(omega.kernel_step(KernelCommand::BeginDirect {
        request: DirectRequestId(1),
        kind: DirectKind::TestAccept,
        transaction: child.clone(),
    }));
    let child_evidence =
        ResolvedEvidence::for_transaction(&child, omega.authority.chain, omega.authority.rules)
            .expect("direct transaction has no dep-group expansion");
    let parent_capability = direct_checked_out(omega.kernel_step(KernelCommand::BeginDirect {
        request: DirectRequestId(2),
        kind: DirectKind::Local,
        transaction: parent.clone(),
    }));
    let parent_evidence =
        ResolvedEvidence::for_transaction(&parent, omega.authority.chain, omega.authority.rules)
            .expect("direct transaction has no dep-group expansion");
    assert_eq!(
        omega
            .kernel_step(KernelCommand::CompleteDirect(DirectCompletion {
                capability: parent_capability,
                wall_time: 10,
                result: DirectWorkResult::Verified(parent_evidence),
            }))
            .disposition(),
        &KernelDisposition::DirectValid(DirectRequestId(2))
    );

    let authority_before = omega.authority.clone();
    assert_eq!(
        omega.kernel_step(KernelCommand::CompleteDirect(DirectCompletion {
            capability: child_capability,
            wall_time: 20,
            result: DirectWorkResult::Verified(child_evidence),
        })),
        KernelStep::NoAuthorityCommit(KernelDisposition::DirectRelevantChange(DirectRequestId(1),))
    );
    assert_eq!(omega.authority, authority_before);
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_generation_replacement_clears_owners_and_leaves_only_bounded_stale_capabilities() {
    let accepted = Transaction::independent(1, 1, 10, 20);
    let retained_tx = Transaction::independent(2, 2, 11, 21);
    let mut omega = model();
    accept(&mut omega, &accepted, 7, 10);
    omega.kernel_step(remote(retained_tx, 8));
    let capability = checked_out(omega.kernel_step(KernelCommand::Checkout));
    let generation = omega.authority.generation;
    let revision = omega.authority.chain.revision;
    assert_eq!(
        omega
            .kernel_step(KernelCommand::ReplaceGeneration { view: ViewId(2) })
            .disposition(),
        &KernelDisposition::GenerationReplaced {
            removed: vec![TxId(1), TxId(2)],
        }
    );
    assert!(omega.authority.owners.is_empty());
    assert!(omega.authority.generation > generation);
    assert!(omega.authority.chain.revision > revision);
    assert!(omega.linear.work.contains_key(&capability));
    assert_eq!(
        omega
            .authority
            .latest_generation_reset
            .as_ref()
            .map(|effect| &effect.logical),
        Some(&LogicalEffect::GenerationReset)
    );
    assert_eq!(omega.check_invariants(), Ok(()));

    let before_stale_recovery = omega.clone();
    let stale_recovery = Transaction::independent(3, 3, 12, 22);
    assert_eq!(
        omega.kernel_step(retained(
            stale_recovery.clone(),
            RetainedSource::Recovery(generation),
        )),
        KernelStep::NoAuthorityCommit(KernelDisposition::StaleRecovery(stale_recovery.id))
    );
    assert_eq!(omega, before_stale_recovery);

    omega.kernel_step(KernelCommand::CancelCapability(capability));
    assert!(!omega.linear.work.contains_key(&capability));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_notification_acknowledgement_is_orthogonal_to_payload_ownership() {
    let mut system = SystemState::constructing(ProtocolLimits::small());
    let request = RequestId(1);
    assert_eq!(
        system.step(SystemEvent::Enqueue {
            request,
            kind: RequestKind::Notification,
            cost: PayloadCost::small(),
        }),
        SystemDisposition::Enqueued(request)
    );
    let record = system
        .protocol
        .requests
        .get(&request)
        .expect("request exists");
    assert_eq!(record.payload, PayloadLocation::Queued);
    assert_eq!(record.response, None);
    assert_eq!(system.check_invariants(), Ok(()));
}

#[test]
fn model_abandoned_response_cannot_veto_a_later_kernel_commit() {
    let mut system = running_system();
    let request = RequestId(1);
    system.step(SystemEvent::Enqueue {
        request,
        kind: RequestKind::Ordinary { response: true },
        cost: PayloadCost::small(),
    });
    system.step(SystemEvent::Dispatch(request));
    system.step(SystemEvent::AbandonReceiver(request));

    let transaction = Transaction::independent(1, 1, 10, 20);
    let commit = system.step(SystemEvent::Kernel {
        access: KernelAccess::Ordinary,
        command: remote(transaction, 7),
    });
    assert!(matches!(
        commit,
        SystemDisposition::Kernel(KernelStep::AuthorityCommit {
            disposition: KernelDisposition::Retained(TxId(1)),
            ..
        })
    ));
    assert_eq!(
        system.step(SystemEvent::Finish {
            request,
            send_response: true,
        }),
        SystemDisposition::Finished {
            request,
            response: ResponseResult::Dropped,
        }
    );
    assert!(!system.protocol.requests.contains_key(&request));
    assert!(
        system
            .authority
            .as_ref()
            .is_some_and(|authority| authority.authority.owners.contains_key(&TxId(1)))
    );
    assert_eq!(system.check_invariants(), Ok(()));
}

#[test]
fn model_initialization_replay_failure_drains_without_publishing_partial_authority() {
    let mut system = SystemState::constructing(ProtocolLimits::small());
    assert_eq!(
        system.step(SystemEvent::Assemble {
            limits: ModelLimits::small(),
            view: ViewId(1),
            rules: RulesId(1),
            succeed: true,
        }),
        SystemDisposition::Assembled
    );
    assert!(matches!(
        system.step(SystemEvent::Kernel {
            access: KernelAccess::Initialization,
            command: retained(
                Transaction::independent(1, 1, 10, 20),
                RetainedSource::Recovery(PoolGeneration(0)),
            ),
        }),
        SystemDisposition::Kernel(KernelStep::AuthorityCommit { .. })
    ));
    let capability = match system.step(SystemEvent::Kernel {
        access: KernelAccess::Initialization,
        command: KernelCommand::Checkout,
    }) {
        SystemDisposition::Kernel(KernelStep::AuthorityCommit {
            disposition: KernelDisposition::CheckedOut(capability),
            ..
        }) => capability.id,
        other => panic!("expected initialization checkout, got {other:?}"),
    };

    assert_eq!(
        system.step(SystemEvent::InitializationReplayFailed),
        SystemDisposition::InitializationDraining
    );
    assert_eq!(system.lifecycle, Lifecycle::Draining);
    assert_eq!(
        system.step(SystemEvent::Ready),
        SystemDisposition::KernelUnavailable
    );
    assert_eq!(
        system.step(SystemEvent::FinishDrain),
        SystemDisposition::DrainPending
    );
    assert!(matches!(
        system.step(SystemEvent::Kernel {
            access: KernelAccess::Drain,
            command: KernelCommand::CancelCapability(capability),
        }),
        SystemDisposition::Kernel(KernelStep::AuthorityCommit { .. })
    ));
    assert_eq!(
        system.step(SystemEvent::FinishDrain),
        SystemDisposition::Stopped
    );
    assert!(system.authority.is_none());
    assert_eq!(system.check_invariants(), Ok(()));
}

#[test]
fn model_drain_cannot_drop_an_outstanding_direct_capability() {
    let mut system = running_system();
    let capability = match system.step(SystemEvent::Kernel {
        access: KernelAccess::Ordinary,
        command: KernelCommand::BeginDirect {
            request: DirectRequestId(1),
            kind: DirectKind::Local,
            transaction: Transaction::independent(1, 1, 10, 20),
        },
    }) {
        SystemDisposition::Kernel(KernelStep::NoAuthorityCommit(
            KernelDisposition::DirectCheckedOut(capability),
        )) => capability.id,
        other => panic!("expected direct checkout, got {other:?}"),
    };
    assert_eq!(
        system.step(SystemEvent::BeginDrain),
        SystemDisposition::Draining
    );
    assert_eq!(
        system.step(SystemEvent::FinishDrain),
        SystemDisposition::DrainPending
    );
    assert!(matches!(
        system.step(SystemEvent::Kernel {
            access: KernelAccess::Drain,
            command: KernelCommand::CancelCapability(capability),
        }),
        SystemDisposition::Kernel(KernelStep::NoAuthorityCommit(
            KernelDisposition::StaleCapabilityRetired(id),
        )) if id == capability
    ));
    assert_eq!(
        system.step(SystemEvent::FinishDrain),
        SystemDisposition::Stopped
    );
    assert_eq!(system.check_invariants(), Ok(()));
}

#[test]
fn model_drain_cannot_drop_a_committed_effect_or_its_unique_claim() {
    let mut system = running_system();
    let transaction = Transaction::independent(1, 1, 10, 20);
    system.step(SystemEvent::Kernel {
        access: KernelAccess::Ordinary,
        command: remote(transaction, 7),
    });
    assert!(matches!(
        system.step(SystemEvent::Kernel {
            access: KernelAccess::Ordinary,
            command: KernelCommand::BanPeer {
                peer: PeerId(7),
                observed_at: MonotonicTick(1),
            },
        }),
        SystemDisposition::Kernel(KernelStep::AuthorityCommit { .. })
    ));
    assert_eq!(
        system.step(SystemEvent::BeginDrain),
        SystemDisposition::Draining
    );
    assert_eq!(
        system.step(SystemEvent::FinishDrain),
        SystemDisposition::DrainPending
    );
    let claim = match system.step(SystemEvent::Kernel {
        access: KernelAccess::Drain,
        command: KernelCommand::ClaimEffect,
    }) {
        SystemDisposition::Kernel(KernelStep::NoAuthorityCommit(
            KernelDisposition::EffectClaimed(claim),
        )) => claim,
        other => panic!("expected effect claim, got {other:?}"),
    };
    assert_eq!(
        system.step(SystemEvent::FinishDrain),
        SystemDisposition::DrainPending
    );
    assert!(matches!(
        system.step(SystemEvent::Kernel {
            access: KernelAccess::Drain,
            command: KernelCommand::SettleEffect(claim),
        }),
        SystemDisposition::Kernel(KernelStep::AuthorityCommit {
            disposition: KernelDisposition::EffectSettled(settled),
            ..
        }) if settled == claim
    ));
    assert_eq!(
        system.step(SystemEvent::FinishDrain),
        SystemDisposition::Stopped
    );
    assert_eq!(system.check_invariants(), Ok(()));
}

#[test]
fn model_shutdown_orders_capability_drain_before_persistence() {
    use super::protocol::{ShutdownAction, ShutdownDisposition, ShutdownPhase, ShutdownProtocol};

    let mut shutdown = ShutdownProtocol::running();
    for (action, phase) in [
        (ShutdownAction::CloseIngress, ShutdownPhase::IngressClosed),
        (
            ShutdownAction::DrainHandlers,
            ShutdownPhase::HandlersDrained,
        ),
        (
            ShutdownAction::JoinChainControl,
            ShutdownPhase::ChainControlJoined,
        ),
        (
            ShutdownAction::JoinAuthorityWorkers,
            ShutdownPhase::AuthorityWorkersJoined,
        ),
        (ShutdownAction::CloseEffects, ShutdownPhase::EffectsClosed),
        (ShutdownAction::DrainEffects, ShutdownPhase::EffectsDrained),
        (
            ShutdownAction::JoinDerivedTasks,
            ShutdownPhase::DerivedTasksJoined,
        ),
        (
            ShutdownAction::CapturePersistence,
            ShutdownPhase::PersistenceCaptured,
        ),
        (ShutdownAction::WritePersistence, ShutdownPhase::Persisted),
    ] {
        assert_eq!(shutdown.step(action), ShutdownDisposition::Advanced(phase));
    }
    assert_eq!(shutdown.phase(), ShutdownPhase::Persisted);
    assert_eq!(
        shutdown.step(ShutdownAction::AuthorityCapabilityLost),
        ShutdownDisposition::OutOfOrder(ShutdownPhase::Persisted),
        "a terminal durable state cannot retroactively lose a retired capability"
    );
    assert_eq!(
        shutdown.step(ShutdownAction::DerivedTaskFailed),
        ShutdownDisposition::OutOfOrder(ShutdownPhase::Persisted),
        "joined derived tasks cannot fail after persistence"
    );
}

#[test]
fn model_shutdown_rejects_early_persistence_and_distinguishes_derived_failure() {
    use super::protocol::{ShutdownAction, ShutdownDisposition, ShutdownPhase, ShutdownProtocol};

    let mut shutdown = ShutdownProtocol::running();
    assert_eq!(
        shutdown.step(ShutdownAction::CapturePersistence),
        ShutdownDisposition::OutOfOrder(ShutdownPhase::Running)
    );
    assert_eq!(
        shutdown.step(ShutdownAction::DerivedTaskFailed),
        ShutdownDisposition::DerivedDegraded(ShutdownPhase::Running)
    );
    assert!(shutdown.derived_degraded());
    assert_eq!(
        shutdown.step(ShutdownAction::AuthorityCapabilityLost),
        ShutdownDisposition::Advanced(ShutdownPhase::Invalidating)
    );
    assert_eq!(
        shutdown.step(ShutdownAction::ReportPersistenceForbidden),
        ShutdownDisposition::OutOfOrder(ShutdownPhase::Invalidating),
        "an invalidity decision cannot be returned before abort is requested"
    );
    assert_eq!(
        shutdown.step(ShutdownAction::RequestAbort),
        ShutdownDisposition::Advanced(ShutdownPhase::AbortRequested)
    );
    assert_eq!(
        shutdown.step(ShutdownAction::ReportPersistenceForbidden),
        ShutdownDisposition::OutOfOrder(ShutdownPhase::AbortRequested),
        "an abort request is not evidence that its task owners have retired"
    );
    assert_eq!(
        shutdown.step(ShutdownAction::JoinAbortedTasks),
        ShutdownDisposition::Advanced(ShutdownPhase::InvalidTasksJoined)
    );
    assert_eq!(
        shutdown.step(ShutdownAction::ReportPersistenceForbidden),
        ShutdownDisposition::PersistenceForbidden
    );
    assert_eq!(
        shutdown.step(ShutdownAction::CloseIngress),
        ShutdownDisposition::PersistenceForbidden
    );

    let mut write_failure = ShutdownProtocol::running();
    for action in [
        ShutdownAction::CloseIngress,
        ShutdownAction::DrainHandlers,
        ShutdownAction::JoinChainControl,
        ShutdownAction::JoinAuthorityWorkers,
        ShutdownAction::CloseEffects,
        ShutdownAction::DrainEffects,
        ShutdownAction::JoinDerivedTasks,
        ShutdownAction::CapturePersistence,
    ] {
        assert!(matches!(
            write_failure.step(action),
            ShutdownDisposition::Advanced(_)
        ));
    }
    assert_eq!(
        write_failure.step(ShutdownAction::PersistenceWriteFailed),
        ShutdownDisposition::PersistenceFailed
    );
    assert_eq!(
        write_failure.step(ShutdownAction::WritePersistence),
        ShutdownDisposition::OutOfOrder(ShutdownPhase::PersistenceFailed),
        "an external write failure is terminal and cannot be relabeled durable"
    );
}

#[test]
fn model_completion_ingress_closure_strictly_lowers_the_effect_drain_rank() {
    use super::protocol::{
        ModelCoordinatorCompletionReadiness, ModelCoordinatorShutdownObservation,
        ModelEffectBlockedShutdownCut, completion_ingress_shutdown_step,
    };

    let disconnected = ModelEffectBlockedShutdownCut::new(
        ModelCoordinatorCompletionReadiness::DisconnectedReady,
        2,
        true,
    );
    let (closed, closure) = completion_ingress_shutdown_step(disconnected);
    assert_eq!(
        closure,
        ModelCoordinatorShutdownObservation::CompletionIngressClosed
    );
    assert_eq!(
        closed.completion(),
        ModelCoordinatorCompletionReadiness::Closed
    );
    assert_eq!(closed.effect_waiters(), 2);

    let (first, first_observation) = completion_ingress_shutdown_step(closed);
    assert_eq!(
        first_observation,
        ModelCoordinatorShutdownObservation::EffectWaitersPromoted
    );
    assert_eq!(first.effect_waiters(), 1);
    assert_eq!(
        first.completion(),
        ModelCoordinatorCompletionReadiness::Closed
    );

    let (second, second_observation) =
        completion_ingress_shutdown_step(first.with_effect_notification());
    assert_eq!(
        second_observation,
        ModelCoordinatorShutdownObservation::EffectWaitersPromoted
    );
    assert_eq!(second.effect_waiters(), 0);
    assert_eq!(
        second.completion(),
        ModelCoordinatorCompletionReadiness::Closed
    );

    let open_pending = ModelEffectBlockedShutdownCut::new(
        ModelCoordinatorCompletionReadiness::OpenPending,
        1,
        false,
    );
    assert_eq!(
        completion_ingress_shutdown_step(open_pending).1,
        ModelCoordinatorShutdownObservation::Pending
    );
}

#[test]
fn model_generation_task_contract_is_a_total_unique_owner_join_partition() {
    use super::protocol::{
        GENERATION_TASK_ROLES, GenerationTaskCriticality, GenerationTaskOwner, GenerationTaskRole,
        ShutdownPhase, generation_task_contract,
    };
    use std::collections::HashSet;

    let roles = GENERATION_TASK_ROLES.into_iter().collect::<HashSet<_>>();
    assert_eq!(roles.len(), GENERATION_TASK_ROLES.len());
    for role in GENERATION_TASK_ROLES {
        let contract = generation_task_contract(role);
        assert_eq!(contract.role, role);
        match role {
            GenerationTaskRole::DispatcherRoot => {
                assert_eq!(contract.owner, GenerationTaskOwner::ProcessRuntimeGuard);
                assert_eq!(
                    contract.criticality,
                    GenerationTaskCriticality::LifecycleRoot
                );
                assert_eq!(contract.join_cut, None);
            }
            GenerationTaskRole::MessageHandler => {
                assert_eq!(contract.owner, GenerationTaskOwner::DispatcherJoinSet);
                assert_eq!(
                    contract.criticality,
                    GenerationTaskCriticality::AuthorityCapability
                );
                assert_eq!(contract.join_cut, Some(ShutdownPhase::HandlersDrained));
            }
            GenerationTaskRole::ChainControl => {
                assert_eq!(contract.owner, GenerationTaskOwner::AuthorityGeneration);
                assert_eq!(
                    contract.criticality,
                    GenerationTaskCriticality::AuthorityCapability
                );
                assert_eq!(contract.join_cut, Some(ShutdownPhase::ChainControlJoined));
            }
            GenerationTaskRole::ComputeCoordinator
            | GenerationTaskRole::ComputeWorker
            | GenerationTaskRole::Ready
            | GenerationTaskRole::Maintenance => {
                assert_eq!(contract.owner, GenerationTaskOwner::AuthorityTaskTopology);
                assert_eq!(
                    contract.criticality,
                    GenerationTaskCriticality::AuthorityCapability
                );
                assert_eq!(
                    contract.join_cut,
                    Some(ShutdownPhase::AuthorityWorkersJoined)
                );
            }
            GenerationTaskRole::EffectPublisher => {
                assert_eq!(contract.owner, GenerationTaskOwner::AuthorityTaskTopology);
                assert_eq!(
                    contract.criticality,
                    GenerationTaskCriticality::AuthorityCapability
                );
                assert_eq!(contract.join_cut, Some(ShutdownPhase::EffectsDrained));
            }
            GenerationTaskRole::VerificationCache | GenerationTaskRole::TemplateLane => {
                assert_eq!(contract.owner, GenerationTaskOwner::AuthorityTaskTopology);
                assert_eq!(
                    contract.criticality,
                    GenerationTaskCriticality::DerivedProjection
                );
                assert_eq!(contract.join_cut, Some(ShutdownPhase::DerivedTasksJoined));
            }
        }
    }
}

#[test]
fn model_ordinary_endpoint_failure_cannot_reach_generation_shutdown() {
    use super::protocol::{
        GENERATION_TASK_ROLES, GenerationTaskCriticality, GenerationTaskDisposition,
        GenerationTaskExit, GenerationTaskRole, generation_task_contract,
        generation_task_disposition,
    };

    for role in GENERATION_TASK_ROLES {
        assert_eq!(
            generation_task_disposition(role, GenerationTaskExit::LifecycleCancellation),
            GenerationTaskDisposition::Join
        );
        assert_eq!(
            generation_task_disposition(role, GenerationTaskExit::OrdinaryEndpointFailure),
            GenerationTaskDisposition::Continue,
            "ordinary endpoint failure escaped {role:?}"
        );
        let expected_failure = match generation_task_contract(role).criticality {
            GenerationTaskCriticality::DerivedProjection => {
                GenerationTaskDisposition::DerivedDegraded
            }
            GenerationTaskCriticality::LifecycleRoot
            | GenerationTaskCriticality::AuthorityCapability => {
                GenerationTaskDisposition::PersistenceForbidden
            }
        };
        for exit in [
            GenerationTaskExit::StructuralFailure,
            GenerationTaskExit::JoinFailure,
            GenerationTaskExit::Timeout,
        ] {
            assert_eq!(generation_task_disposition(role, exit), expected_failure);
        }
    }

    for role in [
        GenerationTaskRole::DispatcherRoot,
        GenerationTaskRole::ChainControl,
        GenerationTaskRole::ComputeCoordinator,
        GenerationTaskRole::EffectPublisher,
    ] {
        assert_eq!(
            generation_task_disposition(role, GenerationTaskExit::OwnerClosed),
            GenerationTaskDisposition::RequestShutdown
        );
    }
    for role in [
        GenerationTaskRole::VerificationCache,
        GenerationTaskRole::TemplateLane,
    ] {
        assert_eq!(
            generation_task_disposition(role, GenerationTaskExit::OwnerClosed),
            GenerationTaskDisposition::DerivedDegraded
        );
    }
    for role in [
        GenerationTaskRole::MessageHandler,
        GenerationTaskRole::ComputeWorker,
        GenerationTaskRole::Ready,
        GenerationTaskRole::Maintenance,
    ] {
        assert_eq!(
            generation_task_disposition(role, GenerationTaskExit::OwnerClosed),
            GenerationTaskDisposition::Continue
        );
    }
}

#[test]
fn model_wall_clock_rollback_never_fabricates_expiry_progress() {
    let transaction = Transaction::independent(1, 1, 10, 20);
    let mut omega = model();
    drive_ready(&mut omega, &transaction, 7);
    omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 100 });

    let rollback = omega.kernel_step(KernelCommand::ExpireAccepted {
        wall_time: 50,
        residency: 10,
    });
    assert_eq!(
        rollback,
        KernelStep::NoAuthorityCommit(KernelDisposition::Idle)
    );
    assert!(omega.authority.owners.contains_key(&TxId(1)));

    let advanced = omega.kernel_step(KernelCommand::ExpireAccepted {
        wall_time: 111,
        residency: 10,
    });
    assert!(matches!(
        advanced,
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::Removed(_),
            ..
        }
    ));
    assert!(!omega.authority.owners.contains_key(&TxId(1)));

    let remote_tx = Transaction::independent(2, 2, 11, 21);
    omega.kernel_step(remote_until(remote_tx.clone(), 7, 100));
    assert_eq!(
        omega.kernel_step(KernelCommand::ExpireRemote {
            wall_time: 50,
            limit: NonZeroU16::new(1).expect("one is non-zero"),
        }),
        KernelStep::NoAuthorityCommit(KernelDisposition::Idle)
    );
    assert!(omega.authority.owners.contains_key(&remote_tx.id));
    assert!(matches!(
        omega.kernel_step(KernelCommand::ExpireRemote {
            wall_time: 100,
            limit: NonZeroU16::new(1).expect("one is non-zero"),
        }),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::Removed(_),
            ..
        }
    ));
    assert!(!omega.authority.owners.contains_key(&remote_tx.id));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_effect_claim_remains_bound_to_the_head_across_later_commits() {
    let first = Transaction::independent(1, 1, 10, 20);
    let second = Transaction::independent(2, 2, 11, 21);
    let mut omega = model();
    drive_ready(&mut omega, &first, 7);
    omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 100 });
    let claim = match omega.kernel_step(KernelCommand::ClaimEffect) {
        KernelStep::NoAuthorityCommit(KernelDisposition::EffectClaimed(claim)) => claim,
        other => panic!("expected effect claim, got {other:?}"),
    };
    omega.kernel_step(remote(second, 8));
    assert_eq!(omega.linear.effect_claim, Some(claim));
    assert_eq!(omega.check_invariants(), Ok(()));
    omega.kernel_step(KernelCommand::SettleEffect(claim));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_generation_reset_claim_is_superseded_without_mutating_authority() {
    let mut omega = model();
    assert!(matches!(
        omega.kernel_step(KernelCommand::ReplaceGeneration { view: ViewId(2) }),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::GenerationReplaced { .. },
            ..
        }
    ));
    assert!(omega.authority.effects.is_empty());
    assert_eq!(omega.effect_usage(), Some(EffectUsage::default()));

    let old_reset = match omega.kernel_step(KernelCommand::ClaimEffect) {
        KernelStep::NoAuthorityCommit(KernelDisposition::EffectClaimed(claim)) => claim,
        other => panic!("expected generation-reset claim, got {other:?}"),
    };
    assert_eq!(old_reset.source, EffectClaimSource::GenerationReset);

    assert!(omega.append_effect_fixture(
        super::state::EffectClass::Trusted,
        vec![LogicalEffect::IngressReleased(TxId(1))],
    ));
    let queued_stamp = omega
        .authority
        .effects
        .front()
        .map(|effect| effect.stamp)
        .expect("the ordinary fixture creates one queued record");
    assert!(queued_stamp > old_reset.stamp);

    assert!(matches!(
        omega.kernel_step(KernelCommand::ReplaceGeneration { view: ViewId(3) }),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::GenerationReplaced { .. },
            ..
        }
    ));
    let latest_reset = omega
        .authority
        .latest_generation_reset
        .as_ref()
        .map(|reset| reset.stamp)
        .expect("the newer generation reset remains resident");
    assert!(latest_reset > queued_stamp);
    assert_eq!(omega.linear.effect_claim, Some(old_reset));
    assert_eq!(omega.check_invariants(), Ok(()));

    let authority_before_superseded_settlement = omega.authority.clone();
    assert_eq!(
        omega.kernel_step(KernelCommand::SettleEffect(old_reset)),
        KernelStep::NoAuthorityCommit(KernelDisposition::EffectSuperseded(old_reset))
    );
    assert_eq!(omega.authority, authority_before_superseded_settlement);
    assert_eq!(omega.linear.effect_claim, None);

    let queued = match omega.kernel_step(KernelCommand::ClaimEffect) {
        KernelStep::NoAuthorityCommit(KernelDisposition::EffectClaimed(claim)) => claim,
        other => panic!("expected queued-effect claim, got {other:?}"),
    };
    assert_eq!(queued.source, EffectClaimSource::Queued);
    assert_eq!(queued.stamp, queued_stamp);
    assert!(matches!(
        omega.kernel_step(KernelCommand::SettleEffect(queued)),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::EffectSettled(observed),
            ..
        } if observed == queued
    ));

    let current_reset = match omega.kernel_step(KernelCommand::ClaimEffect) {
        KernelStep::NoAuthorityCommit(KernelDisposition::EffectClaimed(claim)) => claim,
        other => panic!("expected current generation-reset claim, got {other:?}"),
    };
    assert_eq!(current_reset.source, EffectClaimSource::GenerationReset);
    assert_eq!(current_reset.stamp, latest_reset);
    assert!(matches!(
        omega.kernel_step(KernelCommand::SettleEffect(current_reset)),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::EffectSettled(observed),
            ..
        } if observed == current_reset
    ));
    assert!(!omega.has_pending_effects());
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_invariant_detects_a_computing_owner_without_its_capability() {
    let transaction = Transaction::independent(1, 1, 10, 20);
    let mut omega = model();
    omega.kernel_step(remote(transaction, 7));
    let capability = checked_out(omega.kernel_step(KernelCommand::Checkout));
    omega.linear.work.remove(&capability);
    omega.linear.free_compute_permits = omega.authority.limits.compute_permits;
    assert_eq!(
        omega.check_invariants(),
        Err(ModelInvariantError::ComputingWithoutCapability)
    );
}

#[test]
fn model_startup_rejects_an_effect_partition_smaller_than_one_indivisible_batch() {
    let mut limits = ModelLimits::small();
    limits.effects.remote_bound = EffectBatchBound::new(3, 12);
    let mut system = SystemState::constructing(ProtocolLimits::small());
    assert_eq!(
        system.step(SystemEvent::Assemble {
            limits,
            view: ViewId(1),
            rules: RulesId(1),
            succeed: true,
        }),
        SystemDisposition::StartupFailed
    );
    assert_eq!(system.lifecycle, Lifecycle::StartupFailed);
    assert!(system.authority.is_none());
    assert_eq!(system.check_invariants(), Ok(()));
}

#[test]
fn model_startup_effect_bound_covers_mixed_payload_and_dependency_publication() {
    let limits = ModelLimits::small();
    let Some((records, bytes)) = limits.largest_indivisible_effect_batch() else {
        panic!("the bounded model configuration has a representable effect batch")
    };
    let Some(too_small) = bytes.checked_sub(1) else {
        panic!("the non-zero model effect bound has a predecessor")
    };

    let mut rejected = limits;
    rejected.effects.trusted_bound = EffectBatchBound::new(records, too_small);
    assert!(matches!(
        rejected.validate(),
        Err(super::state::ConfigurationError::IndivisibleEffectBatch)
    ));

    let mut exact = limits;
    let exact_bound = EffectBatchBound::new(records, bytes);
    exact.effects.remote_bound = exact_bound;
    exact.effects.trusted_bound = exact_bound;
    exact.effects.critical_bound = exact_bound;
    assert!(exact.validate().is_ok());
}

#[test]
fn model_startup_rejects_recovery_history_larger_than_retained_capacity() {
    let mut limits = ModelLimits::small();
    limits.owners.bytes = 128;
    limits.replacement_history.bytes = 65;
    let mut system = SystemState::constructing(ProtocolLimits::small());
    assert_eq!(
        system.step(SystemEvent::Assemble {
            limits,
            view: ViewId(1),
            rules: RulesId(1),
            succeed: true,
        }),
        SystemDisposition::StartupFailed
    );
    assert_eq!(system.check_invariants(), Ok(()));
}

#[test]
fn model_controller_bounds_payload_bytes_as_well_as_request_count() {
    let mut system = SystemState::constructing(ProtocolLimits::small());
    let oversized = PayloadCost {
        items: 1,
        bytes: 33,
    };
    assert_eq!(
        system.step(SystemEvent::Enqueue {
            request: RequestId(1),
            kind: RequestKind::Notification,
            cost: oversized,
        }),
        SystemDisposition::QueueFull(RequestId(1))
    );
    assert!(!system.protocol.requests.contains_key(&RequestId(1)));
    assert_eq!(system.check_invariants(), Ok(()));
}

#[test]
fn model_dependency_change_wakes_a_missing_child_in_the_same_apply() {
    let parent = Transaction::independent(1, 1, 10, 20);
    let child = Transaction::dependent(2, 2, 20, 30);
    let mut omega = model();
    assert!(matches!(
        omega.kernel_step(remote(child.clone(), 7)),
        KernelStep::AuthorityCommit { .. }
    ));
    let capability = checked_out(omega.kernel_step(KernelCommand::Checkout));
    assert_eq!(
        omega
            .kernel_step(KernelCommand::Complete(Completion {
                capability,
                result: WorkResult::Missing(missing(&child, BTreeSet::from([CellId(20)]))),
            }))
            .disposition(),
        &KernelDisposition::Waiting(child.id)
    );
    drive_ready_from(
        &mut omega,
        &parent,
        RetainedSource::Recovery(PoolGeneration(0)),
    );
    omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 10 });
    assert!(matches!(
        omega
            .authority
            .owners
            .get(&child.id)
            .map(|owner| &owner.location),
        Some(OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Queued(WorkStage::Resolve),
            ..
        }))
    ));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_trusted_missing_policy_waits_only_for_a_preaccepted_cell_parent() {
    let parent = Transaction::independent(1, 1, 10, 20);
    let child = Transaction::dependent(2, 2, 20, 30);
    let mut omega = model();

    omega.kernel_step(retained(child.clone(), RetainedSource::Proposal));
    let external_missing = checked_out(omega.kernel_step(KernelCommand::Checkout));
    assert_eq!(
        omega
            .kernel_step(KernelCommand::Complete(Completion {
                capability: external_missing,
                result: WorkResult::Missing(missing(&child, BTreeSet::from([CellId(20)]))),
            }))
            .disposition(),
        &KernelDisposition::Rejected(child.id)
    );

    omega.kernel_step(remote(parent, 8));
    omega.kernel_step(retained(child.clone(), RetainedSource::Proposal));
    let pool_missing = checked_out(omega.kernel_step(KernelCommand::Checkout));
    assert_eq!(
        omega
            .kernel_step(KernelCommand::Complete(Completion {
                capability: pool_missing,
                result: WorkResult::Missing(missing(&child, BTreeSet::from([CellId(20)]))),
            }))
            .disposition(),
        &KernelDisposition::Waiting(child.id)
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_trusted_invalid_header_rejection_is_terminal_because_headers_are_chain_only() {
    let mut transaction = Transaction::independent(1, 1, 10, 20);
    transaction.header_deps.insert(HeaderId(1));
    let mut omega = model();
    omega.kernel_step(retained(transaction.clone(), RetainedSource::Proposal));
    let resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));

    assert_eq!(
        omega
            .kernel_step(KernelCommand::Complete(Completion {
                capability: resolve,
                result: WorkResult::resolve_rejected(),
            }))
            .disposition(),
        &KernelDisposition::Rejected(transaction.id)
    );
    assert!(!omega.authority.owners.contains_key(&transaction.id));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_dep_group_resolution_is_the_unique_expanded_dependency_projection() {
    let mut transaction = Transaction::independent(1, 1, 10, 20);
    transaction.cell_deps.insert(CellId(50));
    transaction.dep_groups.insert(CellId(50));
    let group_members = BTreeMap::from([(CellId(50), BTreeSet::from([CellId(10), CellId(11)]))]);

    assert!(
        ResolvedEvidence::for_transaction(&transaction, ChainView::initial(ViewId(1)), RulesId(1))
            .is_none(),
        "a declared dep-group cannot be resolved without its cell-data expansion"
    );
    let evidence = ResolvedEvidence::with_dep_group_members(
        &transaction,
        ChainView::initial(ViewId(1)),
        RulesId(1),
        group_members,
    )
    .expect("the exact declared dep-group expansion is admissible");
    assert_eq!(
        evidence.conditional_reads(),
        BTreeSet::from([CellId(11), CellId(50)]),
        "an input dominates an overlapping dep-group read"
    );
    assert_eq!(
        evidence.dependencies(&transaction),
        Some(BTreeSet::from([
            super::dependency_progress::ModelDependencyKey::cell(10),
            super::dependency_progress::ModelDependencyKey::cell(11),
            super::dependency_progress::ModelDependencyKey::cell(50),
        ]))
    );

    assert!(
        MissingDependencies::for_transaction(&transaction, BTreeSet::from([CellId(11)])).is_none(),
        "a late group member is not a raw transaction declaration"
    );
    let missing =
        MissingDependencies::from_resolved(&transaction, &evidence, BTreeSet::from([CellId(11)]))
            .expect("resolved group membership proves the late missing key");
    assert_eq!(missing.cells(), &BTreeSet::from([CellId(11)]));
    assert!(missing.is_for(&transaction));

    let mut ordinary = Transaction::independent(2, 2, 12, 22);
    ordinary.cell_deps.insert(CellId(50));
    assert!(
        ResolvedEvidence::with_dep_group_members(
            &ordinary,
            ChainView::initial(ViewId(1)),
            RulesId(1),
            BTreeMap::from([(CellId(50), BTreeSet::from([CellId(11)]))]),
        )
        .is_none(),
        "an ordinary code dependency cannot acquire an invented group expansion"
    );
}

#[test]
fn model_dependency_loss_preserves_active_remote_compute_then_requeues_stale_result() {
    let parent = Transaction::independent(1, 1, 10, 20);
    let child = Transaction::dependent(2, 2, 20, 30);
    let mut omega = model();
    drive_ready(&mut omega, &parent, 7);
    omega.kernel_step(remote(child.clone(), 8));
    let capability = match omega.kernel_step(KernelCommand::Checkout) {
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::CheckedOut(capability),
            ..
        } => capability,
        other => panic!("expected the remote child Resolve checkout, got {other:?}"),
    };
    let active_version = omega.authority.owners[&child.id].version;

    assert_eq!(
        omega
            .kernel_step(KernelCommand::Remove {
                transaction: parent.id,
            })
            .disposition(),
        &KernelDisposition::Removed(vec![parent.id])
    );
    assert!(matches!(
        &omega.authority.owners[&child.id],
        super::state::Owner {
            version,
            location: OwnerLocation::Retained(RetainedOwner {
                phase: RetainedPhase::Computing(active),
                ..
            }),
            ..
        } if *version == active_version
            && active.permit == WorkPermit::ResolveOnly
            && active.dependency_cut == capability.dependency_cut
    ));
    assert!(omega.linear.work.contains_key(&capability.id));

    assert_eq!(
        omega
            .kernel_step(KernelCommand::Complete(Completion {
                capability: capability.id,
                result: WorkResult::Missing(missing(&child, BTreeSet::from([CellId(20)]),)),
            }))
            .disposition(),
        &KernelDisposition::Continued(child.id),
        "the changed dependency cut retires the result before missing policy"
    );
    assert!(matches!(
        omega.authority.owners[&child.id].location,
        OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Queued(WorkStage::Resolve),
            ..
        })
    ));
    assert!(!omega.linear.work.contains_key(&capability.id));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_pool_origin_is_evidence_and_parent_loss_removes_the_accepted_causal_closure() {
    let parent = Transaction::independent(1, 1, 10, 20);
    let child = Transaction::dependent(2, 2, 20, 30);
    let mut omega = model();
    accept(&mut omega, &parent, 7, 10);
    let evidence = ResolvedEvidence::with_pool_input(
        &child,
        omega.authority.chain,
        omega.authority.rules,
        CellId(20),
        parent.id,
    )
    .expect("pool input belongs to a direct transaction input");
    drive_ready_with_evidence(&mut omega, &child, RetainedSource::Proposal, evidence);
    assert_eq!(
        omega
            .kernel_step(KernelCommand::FinalizeNext { wall_time: 20 })
            .disposition(),
        &KernelDisposition::Accepted(child.id)
    );
    assert_eq!(omega.check_invariants(), Ok(()));

    let removed = omega.kernel_step(KernelCommand::Remove {
        transaction: parent.id,
    });
    assert_eq!(
        removed.disposition(),
        &KernelDisposition::Removed(vec![parent.id, child.id])
    );
    assert!(omega.authority.owners.is_empty());
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_definitive_preaccepted_parent_loss_cannot_strand_a_proposal_child() {
    let parent = Transaction::independent(1, 1, 10, 20);
    let child = Transaction::dependent(2, 2, 20, 30);
    let mut omega = model();
    drive_ready_from(
        &mut omega,
        &parent,
        RetainedSource::Recovery(PoolGeneration(0)),
    );
    omega.kernel_step(retained(child.clone(), RetainedSource::Proposal));
    let child_resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    assert_eq!(
        omega
            .kernel_step(KernelCommand::Complete(Completion {
                capability: child_resolve,
                result: WorkResult::Missing(missing(&child, BTreeSet::from([CellId(20)]))),
            }))
            .disposition(),
        &KernelDisposition::Waiting(child.id)
    );
    let removed = omega.kernel_step(KernelCommand::Remove {
        transaction: parent.id,
    });
    assert_eq!(
        removed.disposition(),
        &KernelDisposition::Removed(vec![parent.id, child.id])
    );
    assert!(!omega.authority.owners.contains_key(&child.id));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_verification_rejection_terminalizes_the_complete_dependency_closure() {
    let parent = Transaction::independent(1, 1, 10, 20);
    let child = Transaction::dependent(2, 2, 20, 30);
    let mut omega = model();

    omega.kernel_step(retained(
        parent.clone(),
        RetainedSource::Recovery(PoolGeneration(0)),
    ));
    let parent_resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    omega.kernel_step(KernelCommand::Complete(Completion {
        capability: parent_resolve,
        result: WorkResult::Resolved(
            ResolvedEvidence::for_transaction(
                &parent,
                omega.authority.chain,
                omega.authority.rules,
            )
            .expect("direct transaction has no dep-group expansion"),
        ),
    }));
    let parent_verify = checked_out(omega.kernel_step(KernelCommand::Checkout));

    omega.kernel_step(retained(child.clone(), RetainedSource::Proposal));
    let child_resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    omega.kernel_step(KernelCommand::Complete(Completion {
        capability: child_resolve,
        result: WorkResult::Missing(missing(&child, BTreeSet::from([CellId(20)]))),
    }));
    let effects_before = omega.authority.effects.len();

    let parent_rejection = omega.linear.work[&parent_verify].stage_rejection_result();
    let rejected = omega.kernel_step(KernelCommand::Complete(Completion {
        capability: parent_verify,
        result: parent_rejection,
    }));
    let stamp = match rejected {
        KernelStep::AuthorityCommit {
            stamp,
            disposition: KernelDisposition::Rejected(id),
        } if id == parent.id => stamp,
        other => panic!("expected definitive parent rejection, got {other:?}"),
    };

    assert!(omega.authority.owners.is_empty());
    let closure_effects = omega
        .authority
        .effects
        .iter()
        .skip(effects_before)
        .map(|effect| (effect.stamp, effect.ordinal, effect.logical.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        closure_effects,
        vec![
            (stamp, 0, LogicalEffect::validation_rejected(&parent, None)),
            (stamp, 1, LogicalEffect::validation_rejected(&child, None)),
        ]
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_definitive_parent_loss_closes_a_trusted_child_queued_for_verify() {
    let parent = Transaction::independent(1, 1, 10, 20);
    let child = Transaction::dependent(2, 2, 20, 30);
    let mut omega = model();
    accept(&mut omega, &parent, 7, 10);
    omega.kernel_step(retained(child.clone(), RetainedSource::Proposal));
    let resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    let evidence = ResolvedEvidence::with_pool_input(
        &child,
        omega.authority.chain,
        omega.authority.rules,
        CellId(20),
        parent.id,
    )
    .expect("pool input belongs to a direct transaction input");
    omega.kernel_step(KernelCommand::Complete(Completion {
        capability: resolve,
        result: WorkResult::Resolved(evidence),
    }));

    assert_eq!(
        omega
            .kernel_step(KernelCommand::Remove {
                transaction: parent.id,
            })
            .disposition(),
        &KernelDisposition::Removed(vec![parent.id, child.id])
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_definitive_parent_loss_closes_a_trusted_child_computing_verify() {
    let parent = Transaction::independent(1, 1, 10, 20);
    let child = Transaction::dependent(2, 2, 20, 30);
    let mut omega = model();
    accept(&mut omega, &parent, 7, 10);
    omega.kernel_step(retained(child.clone(), RetainedSource::Proposal));
    let resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    let evidence = ResolvedEvidence::with_pool_input(
        &child,
        omega.authority.chain,
        omega.authority.rules,
        CellId(20),
        parent.id,
    )
    .expect("pool input belongs to a direct transaction input");
    omega.kernel_step(KernelCommand::Complete(Completion {
        capability: resolve,
        result: WorkResult::Resolved(evidence),
    }));
    let verify = checked_out(omega.kernel_step(KernelCommand::Checkout));

    omega.kernel_step(KernelCommand::Remove {
        transaction: parent.id,
    });
    assert!(!omega.authority.owners.contains_key(&child.id));
    assert_eq!(omega.check_invariants(), Ok(()));
    assert_eq!(
        omega
            .kernel_step(KernelCommand::Complete(Completion {
                capability: verify,
                result: WorkResult::Verified,
            }))
            .disposition(),
        &KernelDisposition::StaleCapabilityRetired(verify)
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_definitive_parent_loss_preserves_remote_verify_until_stale_settlement() {
    let parent = Transaction::independent(1, 1, 10, 20);
    let child = Transaction::dependent(2, 2, 20, 30);
    let mut omega = model();
    accept(&mut omega, &parent, 7, 10);
    omega.kernel_step(remote(child.clone(), 8));
    let resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    let evidence = ResolvedEvidence::with_pool_input(
        &child,
        omega.authority.chain,
        omega.authority.rules,
        CellId(20),
        parent.id,
    )
    .expect("pool input belongs to a direct transaction input");
    omega.kernel_step(KernelCommand::Complete(Completion {
        capability: resolve,
        result: WorkResult::Resolved(evidence),
    }));
    let verify = checked_out(omega.kernel_step(KernelCommand::Checkout));
    let version_before = omega.authority.owners[&child.id].version;
    let effects_before = omega.authority.effects.len();

    let removed = omega.kernel_step(KernelCommand::Remove {
        transaction: parent.id,
    });
    match removed {
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::Removed(removed),
            ..
        } if removed == vec![parent.id] => {}
        other => panic!("expected exact parent removal, got {other:?}"),
    }
    assert_eq!(
        omega
            .authority
            .effects
            .iter()
            .skip(effects_before)
            .map(|effect| (effect.stamp, effect.ordinal, effect.logical.clone()))
            .collect::<Vec<_>>(),
        Vec::new(),
        "an active capability owns settlement; the dependency event cannot publish a competing wait"
    );
    let child_owner = &omega.authority.owners[&child.id];
    assert_eq!(child_owner.version, version_before);
    assert!(matches!(
        &child_owner.location,
        OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Computing(active),
            ..
        }) if matches!(active.permit, WorkPermit::VerifyOnly(_))
            && active.dependency_cut == omega.linear.work[&verify].dependency_cut
    ));
    assert_eq!(omega.check_invariants(), Ok(()));
    assert_eq!(
        omega
            .kernel_step(KernelCommand::Complete(Completion {
                capability: verify,
                result: WorkResult::Verified,
            }))
            .disposition(),
        &KernelDisposition::Continued(child.id)
    );
    assert!(matches!(
        omega.authority.owners[&child.id].location,
        OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Queued(WorkStage::Resolve),
            ..
        })
    ));
    assert!(!omega.linear.work.contains_key(&verify));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_rbf_apply_terminalizes_trusted_victim_dependents_in_the_same_effect_batch() {
    let victim = Transaction::independent(1, 1, 10, 20);
    let child = Transaction::dependent(2, 2, 20, 30);
    let mut replacement = Transaction::independent(3, 3, 10, 40);
    replacement = replacement.with_fee(30);
    let mut omega = model();
    accept(&mut omega, &victim, 7, 10);

    omega.kernel_step(retained(child.clone(), RetainedSource::Proposal));
    let child_resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    omega.kernel_step(KernelCommand::Complete(Completion {
        capability: child_resolve,
        result: WorkResult::Resolved(
            ResolvedEvidence::with_pool_input(
                &child,
                omega.authority.chain,
                omega.authority.rules,
                CellId(20),
                victim.id,
            )
            .expect("pool input belongs to a direct transaction input"),
        ),
    }));
    let child_verify = checked_out(omega.kernel_step(KernelCommand::Checkout));
    drive_ready(&mut omega, &replacement, 9);
    let effects_before = omega.authority.effects.len();

    let accepted = omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 20 });
    let stamp = match accepted {
        KernelStep::AuthorityCommit {
            stamp,
            disposition:
                KernelDisposition::ReplacementAccepted {
                    winner,
                    replacement_victims,
                    capacity_victims,
                    terminal_dependents,
                    history_retained,
                },
        } if winner == replacement.id
            && replacement_victims == vec![victim.id]
            && capacity_victims.is_empty()
            && terminal_dependents == vec![child.id]
            && history_retained =>
        {
            stamp
        }
        other => panic!("expected atomic RBF dependency loss, got {other:?}"),
    };
    assert!(!omega.authority.owners.contains_key(&child.id));
    assert_eq!(
        omega
            .authority
            .effects
            .iter()
            .skip(effects_before)
            .map(|effect| (effect.stamp, effect.ordinal, effect.logical.clone()))
            .collect::<Vec<_>>(),
        vec![
            (
                stamp,
                0,
                LogicalEffect::admitted(&replacement, AcceptedStatus::Pending, Some(PeerId(9)),),
            ),
            (stamp, 1, LogicalEffect::replaced(&victim, replacement.id)),
            (
                stamp,
                2,
                LogicalEffect::membership_rejected(&child, None, MembershipRejection::Unavailable,),
            ),
        ]
    );
    assert_eq!(omega.check_invariants(), Ok(()));
    assert_eq!(
        omega
            .kernel_step(KernelCommand::Complete(Completion {
                capability: child_verify,
                result: WorkResult::Verified,
            }))
            .disposition(),
        &KernelDisposition::StaleCapabilityRetired(child_verify)
    );
}

#[test]
fn model_capacity_apply_preserves_remote_compute_until_stale_settlement() {
    let victim = Transaction::independent(1, 1, 10, 20);
    let child = Transaction::dependent(2, 2, 20, 30);
    let candidate = Transaction::independent(3, 3, 11, 40).with_fee(30);
    let mut omega = model_with_accepted_limit(1, 4);
    accept(&mut omega, &victim, 7, 10);

    omega.kernel_step(remote(child.clone(), 8));
    let child_resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    omega.kernel_step(KernelCommand::Complete(Completion {
        capability: child_resolve,
        result: WorkResult::Resolved(
            ResolvedEvidence::with_pool_input(
                &child,
                omega.authority.chain,
                omega.authority.rules,
                CellId(20),
                victim.id,
            )
            .expect("pool input belongs to a direct transaction input"),
        ),
    }));
    let child_verify = checked_out(omega.kernel_step(KernelCommand::Checkout));
    let child_version = omega.authority.owners[&child.id].version;
    drive_ready(&mut omega, &candidate, 9);
    let effects_before = omega.authority.effects.len();

    let accepted = omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 20 });
    let stamp = match accepted {
        KernelStep::AuthorityCommit {
            stamp,
            disposition:
                KernelDisposition::CapacityAccepted {
                    winner,
                    victims,
                    terminal_dependents,
                },
        } if winner == candidate.id
            && victims == vec![victim.id]
            && terminal_dependents.is_empty() =>
        {
            stamp
        }
        other => panic!("expected exact capacity dependency wait, got {other:?}"),
    };
    assert_eq!(
        omega
            .authority
            .effects
            .iter()
            .skip(effects_before)
            .map(|effect| (effect.stamp, effect.ordinal, effect.logical.clone()))
            .collect::<Vec<_>>(),
        vec![
            (
                stamp,
                0,
                LogicalEffect::admitted(&candidate, AcceptedStatus::Pending, Some(PeerId(9))),
            ),
            (stamp, 1, LogicalEffect::capacity_evicted(&victim)),
        ]
    );
    let child_owner = &omega.authority.owners[&child.id];
    assert_eq!(child_owner.version, child_version);
    assert!(matches!(
        &child_owner.location,
        OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Computing(active),
            ..
        }) if matches!(active.permit, WorkPermit::VerifyOnly(_))
            && active.dependency_cut == omega.linear.work[&child_verify].dependency_cut
    ));
    assert_eq!(omega.check_invariants(), Ok(()));
    assert_eq!(
        omega
            .kernel_step(KernelCommand::Complete(Completion {
                capability: child_verify,
                result: WorkResult::Verified,
            }))
            .disposition(),
        &KernelDisposition::Continued(child.id)
    );
    assert!(matches!(
        omega.authority.owners[&child.id].location,
        OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Queued(WorkStage::Resolve),
            ..
        })
    ));
    assert!(!omega.linear.work.contains_key(&child_verify));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_chain_conflict_closes_a_computing_dependency_tree_in_one_apply() {
    let parent = Transaction::independent(1, 1, 10, 20);
    let child = Transaction::dependent(2, 2, 20, 30);
    let mut omega = model();
    accept(&mut omega, &parent, 7, 10);
    omega.kernel_step(retained(child.clone(), RetainedSource::Proposal));
    let resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    omega.kernel_step(KernelCommand::Complete(Completion {
        capability: resolve,
        result: WorkResult::Resolved(
            ResolvedEvidence::with_pool_input(
                &child,
                omega.authority.chain,
                omega.authority.rules,
                CellId(20),
                parent.id,
            )
            .expect("pool input belongs to a direct transaction input"),
        ),
    }));
    let verify = checked_out(omega.kernel_step(KernelCommand::Checkout));
    let from = omega.authority.chain;

    let reconciled = omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
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
        conflicting_cells: BTreeSet::from([CellId(10)]),
        recovered: Vec::new(),
        proposals: super::proposal::ProposalContext::empty().view(),
    }));
    assert!(matches!(
        reconciled,
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::ChainReconciled { ref removed, .. },
            ..
        } if removed == &vec![parent.id, child.id]
    ));
    assert!(omega.authority.owners.is_empty());
    assert_eq!(omega.check_invariants(), Ok(()));
    assert_eq!(
        omega
            .kernel_step(KernelCommand::Complete(Completion {
                capability: verify,
                result: WorkResult::Verified,
            }))
            .disposition(),
        &KernelDisposition::StaleCapabilityRetired(verify)
    );
}

#[test]
fn model_chain_reconciliation_promotes_committed_parent_evidence_without_losing_child() {
    let parent = Transaction::independent(1, 1, 10, 20);
    let child = Transaction::dependent(2, 2, 20, 30);
    let mut omega = model();
    accept(&mut omega, &parent, 7, 10);
    let child_evidence = ResolvedEvidence::with_pool_input(
        &child,
        omega.authority.chain,
        omega.authority.rules,
        CellId(20),
        parent.id,
    )
    .expect("pool input belongs to a direct transaction input");
    drive_ready_with_evidence(&mut omega, &child, RetainedSource::Proposal, child_evidence);
    omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 20 });

    let child_version_before = omega.authority.owners[&child.id].version;
    let effects_before = omega.authority.effects.len();
    let reconciled = omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(
            super::state::RulesId(1),
            false,
        ),
        from: omega.authority.chain,
        to_tip: ViewId(2),
        committed: BTreeSet::from([parent.id]),
        available_cells: BTreeSet::new(),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::new(),
        conflicting_cells: BTreeSet::new(),
        recovered: Vec::new(),
        proposals: super::proposal::ProposalContext::status_witness(
            BTreeSet::new(),
            BTreeSet::from([child.proposal]),
        )
        .expect("the proposal witness sets are disjoint")
        .view(),
    }));
    assert_eq!(
        reconciled.disposition(),
        &KernelDisposition::ChainReconciled {
            removed: vec![parent.id],
            recovered: Vec::new(),
            recovery_excluded: Vec::new(),
        }
    );
    let Some(owner) = omega.authority.owners.get(&child.id) else {
        panic!("surviving child must remain owned");
    };
    let OwnerLocation::Accepted { evidence, .. } = &owner.location else {
        panic!("surviving child must remain accepted");
    };
    assert_eq!(omega.proposal_status(&child), AcceptedStatus::Gap);
    assert_eq!(evidence.context.chain.tip, ViewId(2));
    assert_eq!(
        evidence.input_origins.get(&CellId(20)),
        Some(&super::state::InputOrigin::Chain)
    );
    assert_ne!(owner.version, child_version_before);
    assert_eq!(
        omega
            .authority
            .effects
            .iter()
            .skip(effects_before)
            .map(|effect| effect.logical.clone())
            .collect::<Vec<_>>(),
        vec![LogicalEffect::status_changed(&child, AcceptedStatus::Gap)]
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_chain_effect_plan_is_complete_canonical_and_atomic() {
    let committed = Transaction::independent(1, 1, 10, 20);
    let retained_conflict = Transaction::independent(2, 2, 11, 21);
    let accepted_conflict = Transaction::independent(3, 3, 12, 22);
    let accepted_child = Transaction::dependent(4, 4, 22, 23);
    let mut omega = model();
    accept(&mut omega, &accepted_conflict, 9, 10);
    let child_evidence = ResolvedEvidence::with_pool_input(
        &accepted_child,
        omega.authority.chain,
        omega.authority.rules,
        CellId(22),
        accepted_conflict.id,
    )
    .expect("pool input belongs to a direct transaction input");
    drive_ready_with_evidence(
        &mut omega,
        &accepted_child,
        RetainedSource::Proposal,
        child_evidence,
    );
    omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 10 });
    omega.kernel_step(remote(committed.clone(), 7));
    omega.kernel_step(remote(retained_conflict.clone(), 8));
    let effects_before = omega.authority.effects.len();

    let step = omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(
            super::state::RulesId(1),
            false,
        ),
        from: omega.authority.chain,
        to_tip: ViewId(2),
        committed: BTreeSet::from([committed.id]),
        available_cells: BTreeSet::new(),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::new(),
        conflicting_cells: BTreeSet::from([CellId(11), CellId(12)]),
        recovered: Vec::new(),
        proposals: super::proposal::ProposalContext::empty().view(),
    }));
    let KernelStep::AuthorityCommit { stamp, .. } = step else {
        panic!("chain transition must commit its complete effect plan");
    };
    assert!(omega.authority.owners.is_empty());
    assert_eq!(
        omega
            .authority
            .effects
            .iter()
            .skip(effects_before)
            .map(|effect| (effect.stamp, effect.ordinal, effect.logical.clone()))
            .collect::<Vec<_>>(),
        vec![
            (stamp, 0, LogicalEffect::ChainCommitted(committed.id)),
            (
                stamp,
                1,
                LogicalEffect::chain_conflict(&retained_conflict, CellId(11), false),
            ),
            (
                stamp,
                2,
                LogicalEffect::chain_conflict(&accepted_conflict, CellId(12), true),
            ),
            (
                stamp,
                3,
                LogicalEffect::chain_conflict(&accepted_child, CellId(12), true),
            ),
        ]
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_chain_recovery_skips_an_excluded_root_and_descendants_but_keeps_unrelated_work() {
    let mut oversized = Transaction::independent(1, 1, 10, 20);
    oversized = oversized
        .with_payload_bytes(100)
        .expect("the fixed oversized payload is representable");
    let child = Transaction::dependent(2, 2, 20, 30);
    let unrelated = Transaction::independent(3, 3, 11, 21);
    let mut omega = model();
    let reconciled = omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(
            super::state::RulesId(1),
            true,
        ),
        from: omega.authority.chain,
        to_tip: ViewId(2),
        committed: BTreeSet::new(),
        available_cells: BTreeSet::new(),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::new(),
        conflicting_cells: BTreeSet::new(),
        recovered: vec![child, unrelated.clone(), oversized],
        proposals: super::proposal::ProposalContext::empty().view(),
    }));
    assert_eq!(
        reconciled.disposition(),
        &KernelDisposition::ChainReconciled {
            removed: Vec::new(),
            recovered: vec![unrelated.id],
            recovery_excluded: vec![TxId(1), TxId(2)],
        }
    );
    assert!(omega.authority.owners.contains_key(&unrelated.id));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_gap_is_demoted_to_pending_when_the_new_window_contains_no_proposal() {
    let transaction = Transaction::independent(1, 1, 10, 20);
    let mut omega = model();
    accept(&mut omega, &transaction, 7, 10);
    let effects_before = omega.authority.effects.len();
    omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(
            super::state::RulesId(1),
            false,
        ),
        from: omega.authority.chain,
        to_tip: ViewId(2),
        committed: BTreeSet::new(),
        available_cells: BTreeSet::new(),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::new(),
        conflicting_cells: BTreeSet::new(),
        recovered: Vec::new(),
        proposals: super::proposal::ProposalContext::status_witness(
            BTreeSet::new(),
            BTreeSet::from([transaction.proposal]),
        )
        .expect("the proposal witness sets are disjoint")
        .view(),
    }));
    assert!(matches!(
        omega
            .authority
            .owners
            .get(&transaction.id)
            .map(|owner| &owner.location),
        Some(OwnerLocation::Accepted { .. })
    ));
    assert_eq!(omega.proposal_status(&transaction), AcceptedStatus::Gap);
    omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(
            super::state::RulesId(1),
            false,
        ),
        from: omega.authority.chain,
        to_tip: ViewId(3),
        committed: BTreeSet::new(),
        available_cells: BTreeSet::new(),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::new(),
        conflicting_cells: BTreeSet::new(),
        recovered: Vec::new(),
        proposals: super::proposal::ProposalContext::empty().view(),
    }));
    assert!(matches!(
        omega
            .authority
            .owners
            .get(&transaction.id)
            .map(|owner| &owner.location),
        Some(OwnerLocation::Accepted { .. })
    ));
    assert_eq!(omega.proposal_status(&transaction), AcceptedStatus::Pending);
    assert_eq!(
        omega
            .authority
            .effects
            .iter()
            .skip(effects_before)
            .map(|effect| effect.logical.clone())
            .collect::<Vec<_>>(),
        vec![
            LogicalEffect::status_changed(&transaction, AcceptedStatus::Gap),
            LogicalEffect::status_changed(&transaction, AcceptedStatus::Pending),
        ]
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_peer_ban_removes_only_retained_remote_owners_and_releases_refetch() {
    let accepted = Transaction::independent(1, 1, 10, 20);
    let retained_tx = Transaction::independent(2, 2, 11, 21);
    let mut omega = model();
    accept(&mut omega, &accepted, 7, 10);
    omega.kernel_step(remote(retained_tx.clone(), 7));
    assert_eq!(
        omega
            .kernel_step(retained(retained_tx.clone(), RetainedSource::Proposal))
            .disposition(),
        &KernelDisposition::Promoted(retained_tx.id)
    );

    let ban = omega.kernel_step(KernelCommand::BanPeer {
        peer: PeerId(7),
        observed_at: MonotonicTick(1),
    });
    assert_eq!(
        ban.disposition(),
        &KernelDisposition::PeerBanned {
            peer: PeerId(7),
            removed: vec![retained_tx.id],
        }
    );
    assert!(omega.authority.owners.contains_key(&accepted.id));
    assert!(!omega.authority.owners.contains_key(&retained_tx.id));
    assert!(
        omega
            .authority
            .effects
            .iter()
            .any(|effect| { effect.logical == LogicalEffect::PeerCohortRevoked(PeerId(7)) })
    );
    assert_eq!(
        omega
            .kernel_step(remote(retained_tx.clone(), 8))
            .disposition(),
        &KernelDisposition::Retained(retained_tx.id)
    );
    assert_eq!(
        omega
            .kernel_step(remote_at(retained_tx.clone(), 7, 2))
            .disposition(),
        &KernelDisposition::PeerRejected {
            transaction: retained_tx.id,
            peer: PeerId(7),
        }
    );
    assert_eq!(
        omega
            .kernel_step(remote_at(retained_tx.clone(), 7, 12))
            .disposition(),
        &KernelDisposition::Duplicate(retained_tx.id)
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_refetchable_parent_removal_requeues_trusted_waiters_for_every_cause() {
    fn waiting_child(parent: Transaction, child: Transaction, source: RetainedSource) -> Omega {
        let mut omega = model();
        if source == RetainedSource::Proposal {
            let proposal_view = reconcile_proposals(
                omega.authority.chain,
                omega.authority.chain.tip,
                BTreeSet::new(),
                BTreeSet::from([parent.proposal]),
            );
            assert!(matches!(
                omega.kernel_step(proposal_view),
                KernelStep::AuthorityCommit { .. }
            ));
        }
        omega.kernel_step(retained(parent, source));
        omega.kernel_step(retained(
            child.clone(),
            RetainedSource::Recovery(PoolGeneration(0)),
        ));
        let resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
        assert_eq!(
            omega
                .kernel_step(KernelCommand::Complete(Completion {
                    capability: resolve,
                    result: WorkResult::Missing(missing(&child, BTreeSet::from([CellId(20)]))),
                }))
                .disposition(),
            &KernelDisposition::Waiting(child.id)
        );
        omega
    }

    fn assert_requeued(omega: &Omega, parent: TxId, child: TxId) {
        assert!(!omega.authority.owners.contains_key(&parent));
        assert!(matches!(
            omega
                .authority
                .owners
                .get(&child)
                .map(|owner| &owner.location),
            Some(OwnerLocation::Retained(RetainedOwner {
                source: super::state::Source::Recovery(_),
                phase: RetainedPhase::Queued(WorkStage::Resolve),
            }))
        ));
        assert_eq!(omega.check_invariants(), Ok(()));
    }

    let parent = Transaction::independent(1, 1, 10, 20);
    let child = Transaction::dependent(2, 2, 20, 30);

    let mut banned = waiting_child(parent.clone(), child.clone(), remote_source(7, u64::MAX));
    banned.kernel_step(KernelCommand::BanPeer {
        peer: PeerId(7),
        observed_at: MonotonicTick(1),
    });
    assert_requeued(&banned, parent.id, child.id);

    let mut expired = waiting_child(parent.clone(), child.clone(), remote_source(7, 10));
    expired.kernel_step(KernelCommand::ExpireRemote {
        wall_time: 10,
        limit: NonZeroU16::new(1).expect("one is non-zero"),
    });
    assert_requeued(&expired, parent.id, child.id);

    let mut proposal_expired =
        waiting_child(parent.clone(), child.clone(), RetainedSource::Proposal);
    proposal_expired.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(
            super::state::RulesId(1),
            false,
        ),
        from: proposal_expired.authority.chain,
        to_tip: ViewId(2),
        committed: BTreeSet::new(),
        available_cells: BTreeSet::new(),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::new(),
        conflicting_cells: BTreeSet::new(),
        recovered: Vec::new(),
        proposals: super::proposal::ProposalContext::empty().view(),
    }));
    assert_requeued(&proposal_expired, parent.id, child.id);
}

#[test]
fn model_remote_expiry_is_bounded_canonical_and_ignores_promoted_residency() {
    let early = Transaction::independent(1, 1, 10, 20);
    let later = Transaction::independent(2, 2, 11, 21);
    let promoted = Transaction::independent(3, 3, 12, 22);
    let mut omega = model();
    omega.kernel_step(remote_until(later.clone(), 7, 20));
    omega.kernel_step(remote_until(early.clone(), 7, 10));
    omega.kernel_step(remote_until(promoted.clone(), 8, 5));
    assert_eq!(
        omega
            .kernel_step(retained(promoted.clone(), RetainedSource::Proposal))
            .disposition(),
        &KernelDisposition::Promoted(promoted.id)
    );

    let first = omega.kernel_step(KernelCommand::ExpireRemote {
        wall_time: 20,
        limit: NonZeroU16::new(1).expect("one is non-zero"),
    });
    assert_eq!(
        first.disposition(),
        &KernelDisposition::Removed(vec![early.id])
    );
    assert!(omega.authority.owners.contains_key(&later.id));
    assert!(omega.authority.owners.contains_key(&promoted.id));
    assert_eq!(
        omega.authority.effects.back().map(|effect| &effect.logical),
        Some(&LogicalEffect::RemoteExpired(early.id))
    );

    let second = omega.kernel_step(KernelCommand::ExpireRemote {
        wall_time: 20,
        limit: NonZeroU16::new(1).expect("one is non-zero"),
    });
    assert_eq!(
        second.disposition(),
        &KernelDisposition::Removed(vec![later.id])
    );
    assert!(matches!(
        omega
            .authority
            .owners
            .get(&promoted.id)
            .and_then(|owner| owner.retained_source()),
        Some(super::state::Source::Proposal {
            base: ProposalBase::Remote(RemoteResidency {
                peer: PeerId(8),
                expires_at: RemoteDeadline(5),
            }),
        })
    ));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_chain_window_demotes_remote_base_and_expires_trusted_proposal_atomically() {
    let promoted = Transaction::independent(1, 1, 10, 20);
    let trusted = Transaction::independent(2, 2, 11, 21);
    let mut omega = model();
    let proposal_view = reconcile_proposals(
        omega.authority.chain,
        omega.authority.chain.tip,
        BTreeSet::new(),
        BTreeSet::from([promoted.proposal, trusted.proposal]),
    );
    assert!(matches!(
        omega.kernel_step(proposal_view),
        KernelStep::AuthorityCommit { .. }
    ));
    omega.kernel_step(remote_until(promoted.clone(), 7, 10));
    omega.kernel_step(retained(promoted.clone(), RetainedSource::Proposal));
    omega.kernel_step(retained(trusted.clone(), RetainedSource::Proposal));
    let effects_before = omega.authority.effects.len();

    let reconciled = omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(
            super::state::RulesId(1),
            false,
        ),
        from: omega.authority.chain,
        to_tip: ViewId(2),
        committed: BTreeSet::new(),
        available_cells: BTreeSet::new(),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::new(),
        conflicting_cells: BTreeSet::new(),
        recovered: Vec::new(),
        proposals: super::proposal::ProposalContext::empty().view(),
    }));
    assert!(matches!(
        reconciled,
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::ChainReconciled { ref removed, .. },
            ..
        } if removed == &vec![trusted.id]
    ));
    assert!(matches!(
        omega
            .authority
            .owners
            .get(&promoted.id)
            .and_then(|owner| owner.retained_source()),
        Some(super::state::Source::Remote(RemoteResidency {
            peer: PeerId(7),
            expires_at: RemoteDeadline(10),
        }))
    ));
    assert!(!omega.authority.owners.contains_key(&trusted.id));
    assert_eq!(
        omega
            .authority
            .effects
            .iter()
            .skip(effects_before)
            .map(|effect| effect.logical.clone())
            .collect::<Vec<_>>(),
        vec![LogicalEffect::IngressReleased(trusted.id)]
    );

    assert_eq!(
        omega
            .kernel_step(KernelCommand::ExpireRemote {
                wall_time: 10,
                limit: NonZeroU16::new(1).expect("one is non-zero"),
            })
            .disposition(),
        &KernelDisposition::Removed(vec![promoted.id])
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_peer_ban_fence_is_bounded_and_expires_in_the_monotonic_clock_domain() {
    let mut limits = ModelLimits::small();
    limits.peer_ban_fences = 1;
    limits.peer_ban_duration = 10;
    let validated = limits.validate().expect("valid bounded peer-ban fixture");
    let mut omega = Omega::new(validated, ViewId(1), RulesId(1));

    omega.kernel_step(KernelCommand::BanPeer {
        peer: PeerId(7),
        observed_at: MonotonicTick(1),
    });
    omega.kernel_step(KernelCommand::BanPeer {
        peer: PeerId(8),
        observed_at: MonotonicTick(2),
    });
    assert_eq!(omega.authority.peer_bans.len(), 1);
    assert!(!omega.authority.peer_bans.contains_key(&PeerId(7)));
    assert!(omega.authority.peer_bans.contains_key(&PeerId(8)));

    let from_evicted_peer = Transaction::independent(1, 1, 10, 20);
    assert_eq!(
        omega
            .kernel_step(remote_at(from_evicted_peer.clone(), 7, 3))
            .disposition(),
        &KernelDisposition::Retained(from_evicted_peer.id)
    );
    let from_banned_peer = Transaction::independent(2, 2, 11, 21);
    assert_eq!(
        omega
            .kernel_step(remote_at(from_banned_peer.clone(), 8, 3))
            .disposition(),
        &KernelDisposition::PeerRejected {
            transaction: from_banned_peer.id,
            peer: PeerId(8),
        }
    );
    assert_eq!(
        omega
            .kernel_step(remote_at(from_banned_peer.clone(), 8, 12))
            .disposition(),
        &KernelDisposition::Retained(from_banned_peer.id)
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_successful_rbf_moves_the_complete_victim_set_to_history_and_recovers_it() {
    let victim = Transaction::independent(1, 1, 10, 20);
    let mut replacement = Transaction::independent(2, 2, 10, 30);
    replacement = replacement.with_fee(30);
    let mut omega = model();
    accept(&mut omega, &victim, 7, 10);
    drive_ready(&mut omega, &replacement, 8);

    let replaced = omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 20 });
    let replacement_stamp = match &replaced {
        KernelStep::AuthorityCommit { stamp, .. } => *stamp,
        other => panic!("expected committed replacement, got {other:?}"),
    };
    assert_eq!(
        replaced.disposition(),
        &KernelDisposition::ReplacementAccepted {
            winner: replacement.id,
            replacement_victims: vec![victim.id],
            capacity_victims: Vec::new(),
            terminal_dependents: Vec::new(),
            history_retained: true,
        }
    );
    assert!(matches!(
        omega.authority.owners.get(&victim.id).map(|owner| &owner.location),
        Some(OwnerLocation::ReplacementHistory { missing })
            if missing.cells() == &BTreeSet::from([CellId(10)])
    ));
    assert_eq!(query_subject(&omega, victim.id), QuerySubject::Hidden);
    assert_eq!(
        omega
            .authority
            .effects
            .iter()
            .filter(|effect| effect.stamp == replacement_stamp)
            .map(|effect| effect.logical.clone())
            .collect::<Vec<_>>(),
        vec![
            LogicalEffect::admitted(&replacement, AcceptedStatus::Pending, Some(PeerId(8)),),
            LogicalEffect::replaced(&victim, replacement.id),
        ]
    );

    omega.kernel_step(KernelCommand::Remove {
        transaction: replacement.id,
    });
    assert!(matches!(
        omega.authority.owners.get(&victim.id),
        Some(owner)
            if matches!(owner.retained_source(), Some(super::state::Source::Recovery(_)))
                && matches!(
                    owner.location,
                    OwnerLocation::Retained(RetainedOwner {
                        phase: RetainedPhase::Queued(WorkStage::Resolve),
                        ..
                    })
                )
    ));
    assert!(!omega.authority.owners.contains_key(&replacement.id));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_replacement_history_survives_commit_and_recovers_on_detach_availability() {
    let victim = Transaction::independent(1, 1, 10, 20);
    let mut winner = Transaction::independent(2, 2, 10, 30);
    winner = winner.with_fee(30);
    let mut omega = model();
    accept(&mut omega, &victim, 7, 10);
    drive_ready(&mut omega, &winner, 8);
    omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 20 });

    assert!(matches!(
        omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
            context: super::kernel::ChainContextTransition::from_primitives(
                super::state::RulesId(1),
                false,
            ),
            from: omega.authority.chain,
            to_tip: ViewId(2),
            committed: BTreeSet::from([winner.id]),
            available_cells: BTreeSet::new(),
            available_headers: BTreeSet::new(),
            lost_cells: BTreeSet::new(),
            lost_headers: BTreeSet::new(),
            conflicting_cells: BTreeSet::new(),
            recovered: Vec::new(),
            proposals: super::proposal::ProposalContext::empty().view(),
        })),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::ChainReconciled { .. },
            ..
        }
    ));
    assert!(!omega.authority.owners.contains_key(&winner.id));
    assert!(matches!(
        omega.authority.owners.get(&victim.id).map(|owner| &owner.location),
        Some(OwnerLocation::ReplacementHistory { missing })
            if missing.cells() == &BTreeSet::from([CellId(10)])
    ));

    assert_eq!(
        omega
            .kernel_step(KernelCommand::ReconcileChain(ChainTransition {
                context: super::kernel::ChainContextTransition::from_primitives(
                    super::state::RulesId(1),
                    true,
                ),
                from: omega.authority.chain,
                to_tip: ViewId(3),
                committed: BTreeSet::new(),
                available_cells: BTreeSet::new(),
                available_headers: BTreeSet::new(),
                lost_cells: BTreeSet::new(),
                lost_headers: BTreeSet::new(),
                conflicting_cells: BTreeSet::new(),
                recovered: vec![winner.clone()],
                proposals: super::proposal::ProposalContext::empty().view(),
            }))
            .disposition(),
        &KernelDisposition::ChainReconciled {
            removed: Vec::new(),
            recovered: vec![victim.id, winner.id],
            recovery_excluded: Vec::new(),
        }
    );
    for id in [victim.id, winner.id] {
        assert!(matches!(
            omega.authority.owners.get(&id),
            Some(owner) if matches!(
                owner.location,
                OwnerLocation::Retained(RetainedOwner {
                    source: super::state::Source::Recovery(_),
                    phase: RetainedPhase::Queued(WorkStage::Resolve),
                })
            )
        ));
    }
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_nested_replacement_history_waits_for_every_observed_dependency() {
    let mut victim = Transaction::independent(1, 1, 10, 20);
    victim.inputs.insert(CellId(11));
    let mut first_winner = Transaction::independent(2, 2, 10, 30);
    first_winner.inputs.insert(CellId(11));
    first_winner = first_winner.with_fee(50);
    let mut second_winner = Transaction::independent(3, 3, 11, 40);
    second_winner = second_winner.with_fee(100);
    let mut omega = model();
    accept(&mut omega, &victim, 7, 10);
    drive_ready(&mut omega, &first_winner, 8);
    omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 20 });
    assert!(matches!(
        omega.authority.owners.get(&victim.id).map(|owner| &owner.location),
        Some(OwnerLocation::ReplacementHistory { missing })
            if missing.cells() == &BTreeSet::from([CellId(10), CellId(11)])
    ));

    drive_ready(&mut omega, &second_winner, 9);
    omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 30 });
    for id in [victim.id, first_winner.id] {
        assert!(matches!(
            omega.authority.owners.get(&id).map(|owner| &owner.location),
            Some(OwnerLocation::ReplacementHistory { missing })
                if missing.cells() == &BTreeSet::from([CellId(11)])
        ));
    }

    omega.kernel_step(KernelCommand::Remove {
        transaction: second_winner.id,
    });
    for id in [victim.id, first_winner.id] {
        assert!(matches!(
            omega.authority.owners.get(&id),
            Some(owner) if matches!(
                owner.location,
                OwnerLocation::Retained(RetainedOwner {
                    source: super::state::Source::Recovery(_),
                    phase: RetainedPhase::Queued(WorkStage::Resolve),
                })
            )
        ));
    }
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_committed_parent_output_wakes_a_waiting_remote_child() {
    let parent = Transaction::independent(1, 1, 10, 20);
    let child = Transaction::dependent(2, 2, 20, 30);
    let mut omega = model();
    drive_ready(&mut omega, &parent, 7);
    omega.kernel_step(remote(child.clone(), 8));
    let child_resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    omega.kernel_step(KernelCommand::Complete(Completion {
        capability: child_resolve,
        result: WorkResult::Missing(missing(&child, BTreeSet::from([CellId(20)]))),
    }));

    omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(
            super::state::RulesId(1),
            false,
        ),
        from: omega.authority.chain,
        to_tip: ViewId(2),
        committed: BTreeSet::from([parent.id]),
        available_cells: BTreeSet::from([CellId(20)]),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::new(),
        conflicting_cells: BTreeSet::new(),
        recovered: Vec::new(),
        proposals: super::proposal::ProposalContext::empty().view(),
    }));
    assert!(!omega.authority.owners.contains_key(&parent.id));
    assert!(matches!(
        omega
            .authority
            .owners
            .get(&child.id)
            .map(|owner| &owner.location),
        Some(OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Queued(WorkStage::Resolve),
            ..
        }))
    ));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_external_chain_availability_wakes_a_waiting_remote_child() {
    let child = Transaction::dependent(2, 2, 20, 30);
    let mut omega = model();
    omega.kernel_step(remote(child.clone(), 8));
    let child_resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    omega.kernel_step(KernelCommand::Complete(Completion {
        capability: child_resolve,
        result: WorkResult::Missing(missing(&child, BTreeSet::from([CellId(20)]))),
    }));

    omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(
            super::state::RulesId(1),
            false,
        ),
        from: omega.authority.chain,
        to_tip: ViewId(2),
        committed: BTreeSet::new(),
        available_cells: BTreeSet::from([CellId(20)]),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::new(),
        conflicting_cells: BTreeSet::new(),
        recovered: Vec::new(),
        proposals: super::proposal::ProposalContext::empty().view(),
    }));
    assert!(matches!(
        omega
            .authority
            .owners
            .get(&child.id)
            .map(|owner| &owner.location),
        Some(OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Queued(WorkStage::Resolve),
            ..
        }))
    ));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_remote_invalid_header_rejection_is_terminal_without_a_missing_wait_state() {
    let mut transaction = Transaction::independent(1, 1, 10, 20);
    transaction.header_deps.insert(HeaderId(1));
    let mut omega = model();
    omega.kernel_step(remote(transaction.clone(), 7));
    let resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    assert_eq!(
        omega
            .kernel_step(KernelCommand::Complete(Completion {
                capability: resolve,
                result: WorkResult::resolve_rejected(),
            }))
            .disposition(),
        &KernelDisposition::Rejected(transaction.id)
    );
    assert!(!omega.authority.owners.contains_key(&transaction.id));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_header_availability_cannot_discharge_a_cell_missing_wait() {
    let mut transaction = Transaction::independent(1, 1, 10, 20);
    transaction.header_deps.insert(HeaderId(1));
    let missing = missing(&transaction, BTreeSet::from([CellId(10)]));
    let mut omega = model();
    omega.kernel_step(remote(transaction.clone(), 7));
    let resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    omega.kernel_step(KernelCommand::Complete(Completion {
        capability: resolve,
        result: WorkResult::Missing(missing),
    }));
    let waiting_version = omega.authority.owners[&transaction.id].version;
    assert!(matches!(
        omega.authority.effects.back().map(|effect| &effect.logical),
        Some(LogicalEffect::ParentTransactionsRequested {
            transaction: id,
            parent_count: 1,
        }) if *id == transaction.id
    ));

    omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(
            super::state::RulesId(1),
            false,
        ),
        from: omega.authority.chain,
        to_tip: ViewId(2),
        committed: BTreeSet::new(),
        available_cells: BTreeSet::new(),
        available_headers: BTreeSet::from([HeaderId(1)]),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::new(),
        conflicting_cells: BTreeSet::new(),
        recovered: Vec::new(),
        proposals: super::proposal::ProposalContext::empty().view(),
    }));
    assert!(matches!(
        omega.authority.owners[&transaction.id].location,
        OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Waiting { .. },
            ..
        })
    ));
    assert_eq!(
        omega.authority.owners[&transaction.id].version,
        waiting_version
    );

    omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(
            super::state::RulesId(1),
            false,
        ),
        from: omega.authority.chain,
        to_tip: ViewId(3),
        committed: BTreeSet::new(),
        available_cells: BTreeSet::from([CellId(10)]),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::new(),
        conflicting_cells: BTreeSet::new(),
        recovered: Vec::new(),
        proposals: super::proposal::ProposalContext::empty().view(),
    }));
    assert!(matches!(
        omega.authority.owners[&transaction.id].location,
        OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Queued(WorkStage::Resolve),
            ..
        })
    ));
    assert_eq!(
        omega.authority.owners[&transaction.id].version,
        waiting_version
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_extension_preserves_time_proofs_but_blank_detach_recovers_exact_context_closure() {
    let stable = Transaction::independent(1, 1, 10, 20);
    let since = Transaction::independent(2, 2, 11, 21)
        .with_since(CellId(11))
        .expect("the since producer is an exact input");
    let child = Transaction::dependent(3, 3, 21, 31);
    let cellbase = Transaction::independent(4, 4, 12, 22);
    let mut omega = model();

    accept(&mut omega, &stable, 7, 10);
    accept(&mut omega, &since, 8, 11);
    let child_evidence = ResolvedEvidence::with_pool_input(
        &child,
        omega.authority.chain,
        omega.authority.rules,
        CellId(21),
        since.id,
    )
    .expect("pool input belongs to a direct transaction input");
    accept_with_evidence(&mut omega, &child, child_evidence, 12);
    let cellbase_evidence =
        ResolvedEvidence::for_transaction(&cellbase, omega.authority.chain, omega.authority.rules)
            .expect("direct transaction has no dep-group expansion")
            .with_chain_cellbase(CellId(12))
            .expect("the cellbase marker names one chain-origin input");
    accept_with_evidence(&mut omega, &cellbase, cellbase_evidence, 13);

    let extension = omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(RulesId(1), false),
        from: omega.authority.chain,
        to_tip: ViewId(2),
        committed: BTreeSet::new(),
        available_cells: BTreeSet::new(),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::new(),
        conflicting_cells: BTreeSet::new(),
        recovered: Vec::new(),
        proposals: super::proposal::ProposalContext::empty().view(),
    }));
    assert!(matches!(
        extension.disposition(),
        KernelDisposition::ChainReconciled {
            removed,
            recovered,
            ..
        } if removed.is_empty() && recovered.is_empty()
    ));
    assert!(
        [stable.id, since.id, child.id, cellbase.id]
            .into_iter()
            .all(|id| {
                matches!(
                    omega.authority.owners.get(&id).map(|owner| &owner.location),
                    Some(OwnerLocation::Accepted { .. })
                )
            })
    );

    let detach = omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(RulesId(1), true),
        from: omega.authority.chain,
        to_tip: ViewId(3),
        committed: BTreeSet::new(),
        available_cells: BTreeSet::new(),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::new(),
        conflicting_cells: BTreeSet::new(),
        recovered: Vec::new(),
        proposals: super::proposal::ProposalContext::empty().view(),
    }));
    assert!(matches!(
        detach.disposition(),
        KernelDisposition::ChainReconciled {
            removed,
            recovered,
            recovery_excluded,
        } if removed == &vec![since.id, child.id, cellbase.id]
            && recovered == &vec![since.id, child.id, cellbase.id]
            && recovery_excluded.is_empty()
    ));
    assert!(matches!(
        omega.authority.owners[&stable.id].location,
        OwnerLocation::Accepted { .. }
    ));
    for id in [since.id, child.id, cellbase.id] {
        assert!(matches!(
            omega.authority.owners[&id].location,
            OwnerLocation::Retained(RetainedOwner {
                source: super::state::Source::Recovery(_),
                phase: RetainedPhase::Queued(WorkStage::Resolve),
            })
        ));
    }
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_rules_change_dominates_detach_and_recovers_every_accepted_owner() {
    let stable = Transaction::independent(1, 1, 10, 20);
    let contextual = Transaction::independent(2, 2, 11, 21)
        .with_since(CellId(11))
        .expect("the since producer is an exact input");
    let mut omega = model();
    accept(&mut omega, &stable, 7, 10);
    accept(&mut omega, &contextual, 8, 11);

    let changed = omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(RulesId(2), true),
        from: omega.authority.chain,
        to_tip: ViewId(2),
        committed: BTreeSet::new(),
        available_cells: BTreeSet::new(),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::new(),
        conflicting_cells: BTreeSet::new(),
        recovered: Vec::new(),
        proposals: super::proposal::ProposalContext::empty().view(),
    }));
    assert!(matches!(
        changed.disposition(),
        KernelDisposition::ChainReconciled {
            removed,
            recovered,
            recovery_excluded,
        } if removed == &vec![stable.id, contextual.id]
            && recovered == &vec![stable.id, contextual.id]
            && recovery_excluded.is_empty()
    ));
    assert_eq!(omega.authority.rules, RulesId(2));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_detached_header_recovers_accepted_consumer_without_public_rejection() {
    let mut transaction = Transaction::independent(1, 1, 10, 20);
    transaction.header_deps.insert(HeaderId(1));
    let mut omega = model();
    accept(&mut omega, &transaction, 7, 10);
    let before = omega.authority.owners[&transaction.id].clone();
    let effects_before = omega.authority.effects.len();

    let reconciled = omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(
            super::state::RulesId(1),
            true,
        ),
        from: omega.authority.chain,
        to_tip: ViewId(2),
        committed: BTreeSet::new(),
        available_cells: BTreeSet::new(),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::from([HeaderId(1)]),
        conflicting_cells: BTreeSet::new(),
        recovered: Vec::new(),
        proposals: super::proposal::ProposalContext::empty().view(),
    }));
    assert!(matches!(
        reconciled.disposition(),
        KernelDisposition::ChainReconciled {
            removed,
            recovered,
            ..
        } if removed == &vec![transaction.id] && recovered == &vec![transaction.id]
    ));
    let after = &omega.authority.owners[&transaction.id];
    assert!(after.version > before.version);
    assert!(after.arrival > before.arrival);
    assert!(matches!(
        after.location,
        OwnerLocation::Retained(RetainedOwner {
            source: super::state::Source::Recovery(_),
            phase: RetainedPhase::Queued(WorkStage::Resolve),
        })
    ));
    assert_eq!(omega.authority.effects.len(), effects_before);
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_detached_chain_cell_recovers_the_complete_accepted_causal_closure() {
    let parent = Transaction::independent(1, 1, 10, 20);
    let child = Transaction::dependent(2, 2, 20, 30);
    let mut omega = model();
    accept(&mut omega, &parent, 7, 10);
    let child_evidence = ResolvedEvidence::with_pool_input(
        &child,
        omega.authority.chain,
        omega.authority.rules,
        CellId(20),
        parent.id,
    )
    .expect("pool input belongs to a direct transaction input");
    drive_ready_with_evidence(&mut omega, &child, RetainedSource::Proposal, child_evidence);
    omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 20 });

    let reconciled = omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(
            super::state::RulesId(1),
            true,
        ),
        from: omega.authority.chain,
        to_tip: ViewId(2),
        committed: BTreeSet::new(),
        available_cells: BTreeSet::new(),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::from([CellId(10)]),
        lost_headers: BTreeSet::new(),
        conflicting_cells: BTreeSet::new(),
        recovered: Vec::new(),
        proposals: super::proposal::ProposalContext::empty().view(),
    }));
    assert!(matches!(
        reconciled.disposition(),
        KernelDisposition::ChainReconciled {
            removed,
            recovered,
            ..
        } if removed == &vec![parent.id, child.id]
            && recovered == &vec![parent.id, child.id]
    ));
    for id in [parent.id, child.id] {
        assert!(matches!(
            omega.authority.owners[&id].location,
            OwnerLocation::Retained(RetainedOwner {
                source: super::state::Source::Recovery(_),
                phase: RetainedPhase::Queued(WorkStage::Resolve),
            })
        ));
    }
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_new_chain_loss_requeues_a_waiter_that_had_other_missing_evidence() {
    let mut transaction = Transaction::dependent(1, 1, 10, 20);
    transaction.cell_deps.insert(CellId(11));
    transaction.header_deps.insert(HeaderId(1));
    let mut omega = model();
    omega.kernel_step(remote(transaction.clone(), 7));
    let resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    omega.kernel_step(KernelCommand::Complete(Completion {
        capability: resolve,
        result: WorkResult::Missing(missing(&transaction, BTreeSet::from([CellId(11)]))),
    }));
    let before = omega.authority.owners[&transaction.id].clone();

    omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(
            super::state::RulesId(1),
            true,
        ),
        from: omega.authority.chain,
        to_tip: ViewId(2),
        committed: BTreeSet::new(),
        available_cells: BTreeSet::new(),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::from([HeaderId(1)]),
        conflicting_cells: BTreeSet::new(),
        recovered: Vec::new(),
        proposals: super::proposal::ProposalContext::empty().view(),
    }));
    let after = &omega.authority.owners[&transaction.id];
    assert!(after.version > before.version);
    assert!(after.arrival > before.arrival);
    assert!(matches!(
        after.location,
        OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Queued(WorkStage::Resolve),
            ..
        })
    ));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_same_chain_spend_cannot_publish_false_availability_to_history() {
    let victim = Transaction::independent(1, 1, 10, 20);
    let mut winner = Transaction::independent(2, 2, 10, 30);
    winner = winner.with_fee(30);
    let mut omega = model();
    accept(&mut omega, &victim, 7, 10);
    drive_ready(&mut omega, &winner, 8);
    omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 20 });

    omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(
            super::state::RulesId(1),
            false,
        ),
        from: omega.authority.chain,
        to_tip: ViewId(2),
        committed: BTreeSet::new(),
        available_cells: BTreeSet::from([CellId(10)]),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::new(),
        conflicting_cells: BTreeSet::from([CellId(10)]),
        recovered: Vec::new(),
        proposals: super::proposal::ProposalContext::empty().view(),
    }));
    assert!(!omega.authority.owners.contains_key(&winner.id));
    assert!(matches!(
        omega
            .authority
            .owners
            .get(&victim.id)
            .map(|owner| &owner.location),
        Some(OwnerLocation::ReplacementHistory { missing })
            if missing.cells() == &BTreeSet::from([CellId(10)])
    ));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_capacity_self_eviction_preserves_existing_membership_and_terminalizes_the_candidate() {
    let incumbent = Transaction::independent(1, 1, 10, 20).with_fee(100);
    let candidate = Transaction::independent(2, 2, 11, 21).with_fee(1);
    let mut omega = model_with_accepted_limit(1, 4);
    accept(&mut omega, &incumbent, 7, 10);
    drive_ready(&mut omega, &candidate, 8);
    let incumbent_before = omega.authority.owners.get(&incumbent.id).cloned();
    let effects_before = omega.authority.effects.len();

    let rejected = omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 20 });
    let KernelStep::AuthorityCommit {
        stamp,
        disposition: KernelDisposition::ResourceRejected(id),
    } = rejected
    else {
        panic!("the weaker candidate must terminalize without evicting membership");
    };
    assert_eq!(id, candidate.id);
    assert_eq!(
        omega.authority.owners.get(&incumbent.id),
        incumbent_before.as_ref()
    );
    assert!(!omega.authority.owners.contains_key(&candidate.id));
    assert_eq!(
        omega
            .authority
            .effects
            .iter()
            .skip(effects_before)
            .map(|effect| (effect.stamp, effect.ordinal, effect.logical.clone()))
            .collect::<Vec<_>>(),
        vec![(
            stamp,
            0,
            LogicalEffect::membership_rejected(
                &candidate,
                Some(PeerId(8)),
                MembershipRejection::CandidateEvicted,
            ),
        )]
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_membership_rejection_closes_the_candidate_dependency_loss_in_one_apply() {
    let mut incumbent = Transaction::independent(1, 1, 10, 40);
    incumbent = incumbent.with_fee(100);
    let mut parent = Transaction::independent(2, 2, 11, 20);
    parent = parent.with_fee(1);
    let child = Transaction::dependent(3, 3, 20, 30);
    let mut omega = model_with_accepted_limit(1, 4);
    accept(&mut omega, &incumbent, 7, 10);
    drive_ready(&mut omega, &parent, 8);
    assert!(matches!(
        omega.kernel_step(retained(child.clone(), RetainedSource::Proposal)),
        KernelStep::AuthorityCommit { .. }
    ));
    let child_resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    assert_eq!(
        omega
            .kernel_step(KernelCommand::Complete(Completion {
                capability: child_resolve,
                result: WorkResult::Missing(missing(&child, BTreeSet::from([CellId(20)]),)),
            }))
            .disposition(),
        &KernelDisposition::Waiting(child.id)
    );
    let effects_before = omega.authority.effects.len();

    let KernelStep::AuthorityCommit {
        stamp,
        disposition: KernelDisposition::ResourceRejected(id),
    } = omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 20 })
    else {
        panic!("the weaker parent must be rejected by the bounded capacity policy");
    };
    assert_eq!(id, parent.id);
    assert!(!omega.authority.owners.contains_key(&parent.id));
    assert!(!omega.authority.owners.contains_key(&child.id));
    assert_eq!(
        omega
            .authority
            .effects
            .iter()
            .skip(effects_before)
            .map(|effect| (effect.stamp, effect.ordinal, effect.logical.clone()))
            .collect::<Vec<_>>(),
        vec![
            (
                stamp,
                0,
                LogicalEffect::membership_rejected(
                    &parent,
                    Some(PeerId(8)),
                    MembershipRejection::CandidateEvicted,
                ),
            ),
            (
                stamp,
                1,
                LogicalEffect::membership_rejected(&child, None, MembershipRejection::Unavailable,),
            ),
        ]
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_capacity_eviction_recovers_history_blocked_by_the_removed_winner() {
    let victim = Transaction::independent(1, 1, 10, 20);
    let winner = Transaction::independent(2, 2, 10, 30).with_fee(30);
    let candidate = Transaction::independent(3, 3, 11, 40).with_fee(100);
    let mut omega = model_with_accepted_limit(1, 4);
    accept(&mut omega, &victim, 7, 10);
    drive_ready(&mut omega, &winner, 8);
    assert!(matches!(
        omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 20 }),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::ReplacementAccepted { .. },
            ..
        }
    ));
    assert!(matches!(
        omega.authority.owners[&victim.id].location,
        OwnerLocation::ReplacementHistory { ref missing }
            if missing.cells() == &BTreeSet::from([CellId(10)])
    ));

    drive_ready(&mut omega, &candidate, 9);
    assert!(matches!(
        omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 30 }),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::CapacityAccepted { ref victims, .. },
            ..
        } if victims == &vec![winner.id]
    ));
    assert!(!omega.authority.owners.contains_key(&winner.id));
    assert!(matches!(
        omega.authority.owners.get(&victim.id),
        Some(owner) if matches!(
            owner.location,
            OwnerLocation::Retained(RetainedOwner {
                source: super::state::Source::Recovery(_),
                phase: RetainedPhase::Queued(WorkStage::Resolve),
            })
        )
    ));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_capacity_eviction_removes_one_complete_accepted_component() {
    let parent = Transaction::independent(1, 1, 10, 20).with_fee(1);
    let mut child = Transaction::dependent(2, 2, 20, 30);
    // The parent package rate is lower than the child's self rate, so the
    // production eviction tuple selects the parent and therefore its complete
    // descendant closure. Equal rates would deliberately select the leaf by
    // the smaller-descendant-count tie-break.
    child = child.with_fee(100);
    let candidate = Transaction::independent(3, 3, 11, 21).with_fee(100);
    let mut omega = model_with_accepted_limit(2, 8);
    accept(&mut omega, &parent, 7, 10);
    let child_evidence = ResolvedEvidence::with_pool_input(
        &child,
        omega.authority.chain,
        omega.authority.rules,
        CellId(20),
        parent.id,
    )
    .expect("pool input belongs to a direct transaction input");
    drive_ready_with_evidence(
        &mut omega,
        &child,
        remote_source(8, u64::MAX),
        child_evidence,
    );
    assert!(matches!(
        omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 11 }),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::Accepted(_),
            ..
        }
    ));
    drive_ready(&mut omega, &candidate, 9);
    let effects_before = omega.authority.effects.len();

    let accepted = omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 20 });
    let stamp = match accepted {
        KernelStep::AuthorityCommit {
            stamp,
            disposition:
                KernelDisposition::CapacityAccepted {
                    winner,
                    victims,
                    terminal_dependents,
                },
        } if winner == candidate.id
            && victims == vec![parent.id, child.id]
            && terminal_dependents.is_empty() =>
        {
            stamp
        }
        other => panic!("expected complete causal-component eviction, got {other:?}"),
    };
    assert_eq!(
        omega.authority.owners.keys().copied().collect::<Vec<_>>(),
        vec![candidate.id]
    );
    assert_eq!(
        omega
            .authority
            .effects
            .iter()
            .skip(effects_before)
            .map(|effect| (effect.stamp, effect.ordinal, effect.logical.clone()))
            .collect::<Vec<_>>(),
        vec![
            (
                stamp,
                0,
                LogicalEffect::admitted(&candidate, AcceptedStatus::Pending, Some(PeerId(9)),),
            ),
            (stamp, 1, LogicalEffect::capacity_evicted(&parent)),
            (stamp, 2, LogicalEffect::capacity_evicted(&child)),
        ]
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_capacity_eviction_uses_status_weight_count_and_arrival_in_order() {
    // Status is the first production key field: a Pending candidate cannot
    // evict a Proposed member merely by paying a higher fee.
    let mut proposed = Transaction::independent(1, 1, 10, 20);
    proposed = proposed.with_fee(1);
    let mut pending = Transaction::independent(2, 2, 11, 21);
    pending = pending.with_fee(1_000);
    let mut status_omega = model_with_accepted_limit(1, 8);
    assert!(matches!(
        status_omega.kernel_step(reconcile_proposals(
            status_omega.authority.chain,
            ViewId(2),
            BTreeSet::from([proposed.proposal]),
            BTreeSet::new(),
        )),
        KernelStep::AuthorityCommit { .. }
    ));
    let proposed_evidence = ResolvedEvidence::for_transaction(
        &proposed,
        status_omega.authority.chain,
        status_omega.authority.rules,
    )
    .expect("direct transaction has no dep-group expansion");
    drive_ready_with_evidence(
        &mut status_omega,
        &proposed,
        remote_source(7, u64::MAX),
        proposed_evidence,
    );
    assert!(matches!(
        status_omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 10 }),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::Accepted(_),
            ..
        }
    ));
    drive_ready(&mut status_omega, &pending, 8);
    assert!(matches!(
        status_omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 11 }),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::ResourceRejected(id),
            ..
        } if id == pending.id
    ));

    // CKB weight, not absolute fee, controls the second field. The expensive
    // candidate has a lower integer fee rate once its cycles dominate size.
    let mut resident = Transaction::independent(3, 3, 12, 22);
    resident = resident.with_fee(10);
    let mut cycle_heavy = Transaction::independent(4, 4, 13, 23).with_cycles(1_000_000);
    cycle_heavy = cycle_heavy.with_fee(100);
    let mut weight_omega = model_with_accepted_limit(1, 8);
    accept(&mut weight_omega, &resident, 9, 20);
    drive_ready(&mut weight_omega, &cycle_heavy, 10);
    assert!(matches!(
        weight_omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 21 }),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::ResourceRejected(id),
            ..
        } if id == cycle_heavy.id
    ));

    // Equal-rate roots prefer the smaller descendant closure. Removing the
    // leaf is enough to fit and preserves its still-valid parent.
    let mut parent = Transaction::independent(5, 5, 14, 24);
    parent = parent.with_fee(1);
    let mut child = Transaction::dependent(6, 6, 24, 25);
    child = child.with_fee(1);
    let mut stronger = Transaction::independent(7, 7, 15, 26);
    stronger = stronger.with_fee(100);
    let mut count_omega = model_with_accepted_limit(2, 8);
    accept(&mut count_omega, &parent, 11, 30);
    let child_evidence = ResolvedEvidence::with_pool_input(
        &child,
        count_omega.authority.chain,
        count_omega.authority.rules,
        CellId(24),
        parent.id,
    )
    .expect("pool input belongs to a direct transaction input");
    drive_ready_with_evidence(
        &mut count_omega,
        &child,
        remote_source(12, u64::MAX),
        child_evidence,
    );
    assert!(matches!(
        count_omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 31 }),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::Accepted(_),
            ..
        }
    ));
    drive_ready(&mut count_omega, &stronger, 13);
    assert!(matches!(
        count_omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 32 }),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::CapacityAccepted { ref victims, .. },
            ..
        } if victims == &vec![child.id]
    ));
    assert!(count_omega.authority.owners.contains_key(&parent.id));

    // Arrival is globally monotonic, so it resolves equal status/rate/count
    // keys before the identity fallback. The earlier resident is weaker.
    let mut earlier = Transaction::independent(8, 8, 16, 27);
    earlier = earlier.with_fee(1);
    let mut later = Transaction::independent(9, 9, 17, 28);
    later = later.with_fee(1);
    let mut winner = Transaction::independent(10, 10, 18, 29);
    winner = winner.with_fee(100);
    let mut arrival_omega = model_with_accepted_limit(2, 8);
    accept(&mut arrival_omega, &earlier, 14, 40);
    accept(&mut arrival_omega, &later, 15, 41);
    drive_ready(&mut arrival_omega, &winner, 16);
    assert!(matches!(
        arrival_omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 42 }),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::CapacityAccepted { ref victims, .. },
            ..
        } if victims == &vec![earlier.id]
    ));
    assert_eq!(status_omega.check_invariants(), Ok(()));
    assert_eq!(weight_omega.check_invariants(), Ok(()));
    assert_eq!(count_omega.check_invariants(), Ok(()));
    assert_eq!(arrival_omega.check_invariants(), Ok(()));
}

#[test]
fn model_capacity_eviction_never_removes_a_candidate_ancestor() {
    let parent = Transaction::independent(1, 1, 10, 20).with_fee(1);
    let unrelated = Transaction::independent(2, 2, 11, 21).with_fee(100);
    let candidate = Transaction::dependent(3, 3, 20, 30).with_fee(10);
    let mut omega = model_with_accepted_limit(2, 8);
    accept(&mut omega, &parent, 7, 10);
    accept(&mut omega, &unrelated, 8, 11);
    let candidate_evidence = ResolvedEvidence::with_pool_input(
        &candidate,
        omega.authority.chain,
        omega.authority.rules,
        CellId(20),
        parent.id,
    )
    .expect("pool input belongs to a direct transaction input");
    drive_ready_with_evidence(
        &mut omega,
        &candidate,
        remote_source(9, u64::MAX),
        candidate_evidence,
    );
    let accepted_before = omega
        .authority
        .owners
        .iter()
        .filter_map(|(id, owner)| {
            matches!(owner.location, OwnerLocation::Accepted { .. }).then_some((*id, owner.clone()))
        })
        .collect::<BTreeMap<_, _>>();

    assert_eq!(
        omega
            .kernel_step(KernelCommand::FinalizeNext { wall_time: 20 })
            .disposition(),
        &KernelDisposition::ResourceRejected(candidate.id)
    );
    for (id, before) in accepted_before {
        assert_eq!(omega.authority.owners.get(&id), Some(&before));
    }
    assert!(!omega.authority.owners.contains_key(&candidate.id));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_rbf_and_capacity_share_one_apply_without_collapsing_removal_causes() {
    let mut victim = Transaction::independent(1, 1, 10, 20);
    victim = victim.with_fee(1);
    let mut unrelated = Transaction::independent(2, 2, 11, 21);
    unrelated = unrelated.with_fee(1);
    let mut replacement = Transaction::independent(3, 3, 10, 30);
    replacement = replacement
        .with_payload_bytes(8)
        .expect("the fixed replacement payload is representable")
        .with_fee(100);
    let mut omega = model_with_accepted_limit(2, 8);
    accept(&mut omega, &victim, 7, 10);
    accept(&mut omega, &unrelated, 8, 11);
    drive_ready(&mut omega, &replacement, 9);
    let effects_before = omega.authority.effects.len();

    let accepted = omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 20 });
    let stamp = match accepted {
        KernelStep::AuthorityCommit {
            stamp,
            disposition:
                KernelDisposition::ReplacementAccepted {
                    winner,
                    replacement_victims,
                    capacity_victims,
                    terminal_dependents,
                    history_retained,
                },
        } if winner == replacement.id
            && replacement_victims == vec![victim.id]
            && capacity_victims == vec![unrelated.id]
            && terminal_dependents.is_empty()
            && history_retained =>
        {
            stamp
        }
        other => panic!("expected one mixed replacement/capacity Apply, got {other:?}"),
    };
    assert!(matches!(
        omega.authority.owners.get(&victim.id).map(|owner| &owner.location),
        Some(OwnerLocation::ReplacementHistory { missing })
            if missing.cells() == &BTreeSet::from([CellId(10)])
    ));
    assert!(!omega.authority.owners.contains_key(&unrelated.id));
    assert!(matches!(
        omega
            .authority
            .owners
            .get(&replacement.id)
            .map(|owner| &owner.location),
        Some(OwnerLocation::Accepted { .. })
    ));
    assert_eq!(
        omega
            .authority
            .effects
            .iter()
            .skip(effects_before)
            .map(|effect| (effect.stamp, effect.ordinal, effect.logical.clone()))
            .collect::<Vec<_>>(),
        vec![
            (
                stamp,
                0,
                LogicalEffect::admitted(&replacement, AcceptedStatus::Pending, Some(PeerId(9)),),
            ),
            (stamp, 1, LogicalEffect::replaced(&victim, replacement.id)),
            (stamp, 2, LogicalEffect::capacity_evicted(&unrelated)),
        ]
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_late_parent_is_coupled_and_rewrites_surviving_child_evidence_atomically() {
    let mut child = Transaction::dependent(2, 2, 20, 30);
    child = child.with_fee(10);
    let mut unrelated = Transaction::independent(3, 3, 11, 21);
    unrelated = unrelated.with_fee(1);
    let mut parent = Transaction::independent(1, 1, 10, 20);
    parent = parent.with_fee(100);
    let mut other_ready = Transaction::independent(4, 4, 12, 22);
    other_ready = other_ready.with_fee(50);
    let mut omega = model_with_accepted_limit(2, 8);
    accept(&mut omega, &child, 5, 10);
    accept(&mut omega, &unrelated, 6, 11);
    drive_ready(&mut omega, &parent, 7);
    drive_ready(&mut omega, &other_ready, 8);
    let child_version_before = omega.authority.owners[&child.id].version;
    let authority_before = omega.authority.clone();
    let capture = ready_capture(omega.kernel_step(KernelCommand::CaptureReady { limit: 2 }));

    assert_eq!(
        omega
            .kernel_step(KernelCommand::FinalizeCaptured {
                capture,
                wall_time: 20,
            })
            .disposition(),
        &KernelDisposition::ReadyCutChanged
    );
    assert_eq!(omega.authority, authority_before);

    assert_eq!(
        omega
            .kernel_step(KernelCommand::FinalizeNext { wall_time: 20 })
            .disposition(),
        &KernelDisposition::CapacityAccepted {
            winner: parent.id,
            victims: vec![unrelated.id],
            terminal_dependents: Vec::new(),
        }
    );
    let Some(child_owner) = omega.authority.owners.get(&child.id) else {
        panic!("the surviving late child must remain accepted");
    };
    let OwnerLocation::Accepted { evidence, .. } = &child_owner.location else {
        panic!("the surviving late child must remain in accepted membership");
    };
    assert_eq!(
        evidence.input_origins.get(&CellId(20)),
        Some(&super::state::InputOrigin::Pool(parent.id))
    );
    assert_ne!(child_owner.version, child_version_before);
    assert!(matches!(
        omega
            .authority
            .owners
            .get(&parent.id)
            .map(|owner| &owner.location),
        Some(OwnerLocation::Accepted { .. })
    ));
    assert!(matches!(
        omega
            .authority
            .owners
            .get(&other_ready.id)
            .map(|owner| &owner.location),
        Some(OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Ready(_),
            ..
        }))
    ));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_rbf_fee_floor_and_new_unconfirmed_input_match_policy() {
    let victim = Transaction::independent(1, 1, 10, 20);
    let mut exact_floor = Transaction::independent(2, 2, 10, 30);
    let minimum_rate = ModelFeeRate::from_u64(1_500);
    let exact_floor_fee =
        victim.cost.fee() + minimum_rate.fee(u64::from(exact_floor.cost.serialized_bytes()));
    exact_floor = exact_floor.with_fee(exact_floor_fee);
    let mut exact = model();
    accept(&mut exact, &victim, 7, 10);
    drive_ready(&mut exact, &exact_floor, 8);
    assert!(matches!(
        exact.kernel_step(KernelCommand::FinalizeNext { wall_time: 20 }),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::ReplacementAccepted { .. },
            ..
        }
    ));

    let parent = Transaction::independent(3, 3, 11, 21);
    let victim = Transaction::independent(4, 4, 10, 20);
    let mut candidate = Transaction::independent(5, 5, 10, 30);
    candidate.inputs.insert(CellId(21));
    candidate = candidate.with_fee(100);
    let mut rejected = model();
    accept(&mut rejected, &parent, 7, 10);
    accept(&mut rejected, &victim, 8, 11);
    let evidence = ResolvedEvidence::with_pool_input(
        &candidate,
        rejected.authority.chain,
        rejected.authority.rules,
        CellId(21),
        parent.id,
    )
    .expect("pool input belongs to a direct transaction input");
    drive_ready_with_evidence(
        &mut rejected,
        &candidate,
        remote_source(9, u64::MAX),
        evidence,
    );
    assert_eq!(
        rejected
            .kernel_step(KernelCommand::FinalizeNext { wall_time: 20 })
            .disposition(),
        &KernelDisposition::MembershipRejected(candidate.id)
    );
    assert!(rejected.authority.owners.contains_key(&parent.id));
    assert!(rejected.authority.owners.contains_key(&victim.id));
    assert!(!rejected.authority.owners.contains_key(&candidate.id));
    assert_eq!(rejected.check_invariants(), Ok(()));
}

#[test]
fn model_fee_policy_matches_configured_production_arithmetic() {
    let rate = ModelFeeRate::from_u64(1_500);
    assert_eq!(rate.as_u64(), 1_500);
    assert_eq!(rate.fee(999), 1_498);
    assert_eq!(rate.fee(1_000), 1_500);
    assert_eq!(ModelFeeRate::from_u64(u64::MAX).fee(2), u64::MAX / 1_000);
    assert_eq!(
        minimum_fee_observation(11, 8, rate),
        ModelMinimumFeeObservation::Rejected {
            actual: 11,
            required: 12,
        }
    );
    assert_eq!(
        minimum_fee_observation(12, 8, rate),
        ModelMinimumFeeObservation::Accepted {
            actual: 12,
            required: 12,
        }
    );

    let limits = ModelLimits::small()
        .validate()
        .expect("the policy fixture uses valid resource bounds");
    let mut disabled = Omega::new_with_replacement_policy(
        limits,
        ViewId(1),
        RulesId(1),
        ModelReplacementPolicy::Disabled,
    );
    assert_eq!(disabled.authority.replacement_policy.minimum_rate(), None);
    let victim = Transaction::independent(31, 31, 40, 41);
    let replacement = Transaction::independent(32, 32, 40, 42).with_fee(u64::MAX);
    accept(&mut disabled, &victim, 7, 10);
    drive_ready(&mut disabled, &replacement, 8);
    assert_eq!(
        disabled
            .kernel_step(KernelCommand::FinalizeNext { wall_time: 20 })
            .disposition(),
        &KernelDisposition::MembershipRejected(replacement.id)
    );

    let configured_limits = ModelLimits::small()
        .validate()
        .expect("the configured-rate fixtures use valid resource bounds");
    let mut low_rate = Omega::new_with_replacement_policy(
        configured_limits,
        ViewId(1),
        RulesId(1),
        ModelReplacementPolicy::Enabled {
            minimum_rate: ModelFeeRate::from_u64(1_000),
        },
    );
    let mut high_rate = Omega::new_with_replacement_policy(
        configured_limits,
        ViewId(1),
        RulesId(1),
        ModelReplacementPolicy::Enabled {
            minimum_rate: ModelFeeRate::from_u64(1_500),
        },
    );
    let victim = Transaction::independent(41, 41, 50, 51);
    let candidate = Transaction::independent(42, 42, 50, 52).with_fee(20);
    for omega in [&mut low_rate, &mut high_rate] {
        accept(omega, &victim, 7, 10);
        drive_ready(omega, &candidate, 8);
    }
    assert!(matches!(
        low_rate.kernel_step(KernelCommand::FinalizeNext { wall_time: 20 }),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::ReplacementAccepted { .. },
            ..
        }
    ));
    assert_eq!(
        high_rate
            .kernel_step(KernelCommand::FinalizeNext { wall_time: 20 })
            .disposition(),
        &KernelDisposition::MembershipRejected(candidate.id)
    );
}

#[test]
fn model_history_saturation_discards_the_complete_optional_set_without_losing_winner() {
    let mut limits = ModelLimits::small();
    limits.replacement_history = ResourceVector::ZERO;
    let validated = limits
        .validate()
        .expect("zero optional history is a valid configuration");
    let mut omega = Omega::new(validated, ViewId(1), RulesId(1));
    let victim = Transaction::independent(1, 1, 10, 20);
    let mut replacement = Transaction::independent(2, 2, 10, 30);
    replacement = replacement.with_fee(30);
    accept(&mut omega, &victim, 7, 10);
    drive_ready(&mut omega, &replacement, 8);
    assert_eq!(
        omega
            .kernel_step(KernelCommand::FinalizeNext { wall_time: 20 })
            .disposition(),
        &KernelDisposition::ReplacementAccepted {
            winner: replacement.id,
            replacement_victims: vec![victim.id],
            capacity_victims: Vec::new(),
            terminal_dependents: Vec::new(),
            history_retained: false,
        }
    );
    assert!(!omega.authority.owners.contains_key(&victim.id));
    assert!(matches!(
        omega
            .authority
            .owners
            .get(&replacement.id)
            .map(|owner| &owner.location),
        Some(OwnerLocation::Accepted { .. })
    ));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_failed_rbf_is_terminal_and_never_mutates_the_victim() {
    let victim = Transaction::independent(1, 1, 10, 20);
    let replacement = Transaction::independent(2, 2, 10, 30);
    let mut omega = model();
    accept(&mut omega, &victim, 7, 10);
    let victim_before = omega.authority.owners.get(&victim.id).cloned();
    drive_ready(&mut omega, &replacement, 8);
    assert_eq!(
        omega
            .kernel_step(KernelCommand::FinalizeNext { wall_time: 20 })
            .disposition(),
        &KernelDisposition::MembershipRejected(replacement.id)
    );
    assert_eq!(
        omega.authority.owners.get(&victim.id).cloned(),
        victim_before
    );
    assert!(!omega.authority.owners.contains_key(&replacement.id));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_direct_negative_receipt_ignores_unrelated_commits_but_detects_relevant_change() {
    let direct = Transaction::dependent(1, 1, 20, 30);
    let unrelated = Transaction::independent(2, 2, 11, 21);
    let parent = Transaction::independent(9, 9, 12, 20);
    let mut omega = model();

    let first = direct_checked_out(omega.kernel_step(KernelCommand::BeginDirect {
        request: DirectRequestId(1),
        kind: DirectKind::TestAccept,
        transaction: direct.clone(),
    }));
    let first_negative =
        omega.capture_direct_negative(&direct, DirectNegativeReason::MissingDependency);
    accept(&mut omega, &unrelated, 7, 10);
    assert_eq!(
        omega
            .kernel_step(KernelCommand::CompleteDirect(DirectCompletion {
                capability: first,
                wall_time: 10,
                result: DirectWorkResult::Rejected(first_negative),
            }))
            .disposition(),
        &KernelDisposition::DirectRejected(
            DirectRequestId(1),
            DirectNegativeReason::MissingDependency,
        )
    );

    let second = direct_checked_out(omega.kernel_step(KernelCommand::BeginDirect {
        request: DirectRequestId(2),
        kind: DirectKind::TestAccept,
        transaction: direct.clone(),
    }));
    let second_negative =
        omega.capture_direct_negative(&direct, DirectNegativeReason::MissingDependency);
    accept(&mut omega, &parent, 8, 20);
    assert_eq!(
        omega
            .kernel_step(KernelCommand::CompleteDirect(DirectCompletion {
                capability: second,
                wall_time: 20,
                result: DirectWorkResult::Rejected(second_negative),
            }))
            .disposition(),
        &KernelDisposition::DirectRelevantChange(DirectRequestId(2))
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_local_direct_success_uses_no_retained_owner_before_its_single_final_apply() {
    let transaction = Transaction::independent(1, 1, 10, 20);
    let mut omega = model();
    let capability = direct_checked_out(omega.kernel_step(KernelCommand::BeginDirect {
        request: DirectRequestId(1),
        kind: DirectKind::Local,
        transaction: transaction.clone(),
    }));
    assert!(!omega.authority.owners.contains_key(&transaction.id));
    let apply_before = omega.authority.last_apply;
    let evidence = ResolvedEvidence::for_transaction(
        &transaction,
        omega.authority.chain,
        omega.authority.rules,
    )
    .expect("direct transaction has no dep-group expansion");
    assert_eq!(
        omega
            .kernel_step(KernelCommand::CompleteDirect(DirectCompletion {
                capability,
                wall_time: 77,
                result: DirectWorkResult::Verified(evidence),
            }))
            .disposition(),
        &KernelDisposition::DirectValid(DirectRequestId(1))
    );
    assert_eq!(omega.authority.last_apply.0, apply_before.0 + 1);
    assert!(matches!(
        omega
            .authority
            .owners
            .get(&transaction.id)
            .map(|owner| &owner.location),
        Some(OwnerLocation::Accepted {
            accepted_at_wall: 77,
            ..
        })
    ));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_test_accept_observes_the_same_success_policy_without_authority_mutation() {
    let transaction = Transaction::independent(1, 1, 10, 20);
    let mut omega = model();
    let test_capability = direct_checked_out(omega.kernel_step(KernelCommand::BeginDirect {
        request: DirectRequestId(1),
        kind: DirectKind::TestAccept,
        transaction: transaction.clone(),
    }));
    let test_evidence = ResolvedEvidence::for_transaction(
        &transaction,
        omega.authority.chain,
        omega.authority.rules,
    )
    .expect("direct transaction has no dep-group expansion");
    let authority_before = omega.authority.clone();
    assert_eq!(
        omega.kernel_step(KernelCommand::CompleteDirect(DirectCompletion {
            capability: test_capability,
            wall_time: 10,
            result: DirectWorkResult::Verified(test_evidence),
        })),
        KernelStep::NoAuthorityCommit(KernelDisposition::DirectValid(DirectRequestId(1)))
    );
    assert_eq!(omega.authority, authority_before);

    let local_capability = direct_checked_out(omega.kernel_step(KernelCommand::BeginDirect {
        request: DirectRequestId(2),
        kind: DirectKind::Local,
        transaction: transaction.clone(),
    }));
    let local_evidence = ResolvedEvidence::for_transaction(
        &transaction,
        omega.authority.chain,
        omega.authority.rules,
    )
    .expect("direct transaction has no dep-group expansion");
    assert!(matches!(
        omega.kernel_step(KernelCommand::CompleteDirect(DirectCompletion {
            capability: local_capability,
            wall_time: 10,
            result: DirectWorkResult::Verified(local_evidence),
        })),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::DirectValid(DirectRequestId(2)),
            ..
        }
    ));
    assert!(matches!(
        omega
            .authority
            .owners
            .get(&transaction.id)
            .map(|owner| &owner.location),
        Some(OwnerLocation::Accepted { .. })
    ));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_test_accept_and_local_share_the_exact_rbf_rejection_policy() {
    let victim = Transaction::independent(1, 1, 10, 20);
    let replacement = Transaction::independent(2, 2, 10, 30);
    let mut omega = model();
    accept(&mut omega, &victim, 7, 10);
    let victim_before = omega.authority.owners.get(&victim.id).cloned();

    let test_capability = direct_checked_out(omega.kernel_step(KernelCommand::BeginDirect {
        request: DirectRequestId(1),
        kind: DirectKind::TestAccept,
        transaction: replacement.clone(),
    }));
    let test_evidence = ResolvedEvidence::for_transaction(
        &replacement,
        omega.authority.chain,
        omega.authority.rules,
    )
    .expect("direct transaction has no dep-group expansion");
    let authority_before = omega.authority.clone();
    assert_eq!(
        omega.kernel_step(KernelCommand::CompleteDirect(DirectCompletion {
            capability: test_capability,
            wall_time: 20,
            result: DirectWorkResult::Verified(test_evidence),
        })),
        KernelStep::NoAuthorityCommit(KernelDisposition::DirectRejected(
            DirectRequestId(1),
            DirectNegativeReason::Policy,
        ))
    );
    assert_eq!(omega.authority, authority_before);

    let local_capability = direct_checked_out(omega.kernel_step(KernelCommand::BeginDirect {
        request: DirectRequestId(2),
        kind: DirectKind::Local,
        transaction: replacement.clone(),
    }));
    let local_evidence = ResolvedEvidence::for_transaction(
        &replacement,
        omega.authority.chain,
        omega.authority.rules,
    )
    .expect("direct transaction has no dep-group expansion");
    assert!(matches!(
        omega.kernel_step(KernelCommand::CompleteDirect(DirectCompletion {
            capability: local_capability,
            wall_time: 20,
            result: DirectWorkResult::Verified(local_evidence),
        })),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::DirectRejected(
                DirectRequestId(2),
                DirectNegativeReason::Policy,
            ),
            ..
        }
    ));
    assert_eq!(
        omega.authority.owners.get(&victim.id).cloned(),
        victim_before
    );
    assert!(!omega.authority.owners.contains_key(&replacement.id));
    assert_eq!(
        omega.authority.effects.back().map(|effect| &effect.logical),
        Some(&LogicalEffect::membership_rejected(
            &replacement,
            None,
            super::state::MembershipRejection::Policy,
        ))
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_test_accept_duplicate_is_read_only_and_local_duplicate_is_acknowledged() {
    let transaction = Transaction::independent(1, 1, 10, 20);
    let mut omega = model();
    accept(&mut omega, &transaction, 7, 10);

    let test_capability = direct_checked_out(omega.kernel_step(KernelCommand::BeginDirect {
        request: DirectRequestId(1),
        kind: DirectKind::TestAccept,
        transaction: transaction.clone(),
    }));
    let test_evidence = ResolvedEvidence::for_transaction(
        &transaction,
        omega.authority.chain,
        omega.authority.rules,
    )
    .expect("direct transaction has no dep-group expansion");
    let authority_before = omega.authority.clone();
    assert_eq!(
        omega.kernel_step(KernelCommand::CompleteDirect(DirectCompletion {
            capability: test_capability,
            wall_time: 20,
            result: DirectWorkResult::Verified(test_evidence),
        })),
        KernelStep::NoAuthorityCommit(KernelDisposition::DirectDuplicate(DirectRequestId(1)))
    );
    assert_eq!(omega.authority, authority_before);

    let local_capability = direct_checked_out(omega.kernel_step(KernelCommand::BeginDirect {
        request: DirectRequestId(2),
        kind: DirectKind::Local,
        transaction: transaction.clone(),
    }));
    let local_evidence = ResolvedEvidence::for_transaction(
        &transaction,
        omega.authority.chain,
        omega.authority.rules,
    )
    .expect("direct transaction has no dep-group expansion");
    let owner_before = omega.authority.owners.get(&transaction.id).cloned();
    assert!(matches!(
        omega.kernel_step(KernelCommand::CompleteDirect(DirectCompletion {
            capability: local_capability,
            wall_time: 20,
            result: DirectWorkResult::Verified(local_evidence),
        })),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::DirectDuplicate(DirectRequestId(2)),
            ..
        }
    ));
    assert_eq!(
        omega.authority.owners.get(&transaction.id),
        owner_before.as_ref()
    );
    assert_eq!(
        omega.authority.effects.back().map(|effect| &effect.logical),
        Some(&LogicalEffect::accepted_duplicate(transaction.id, None))
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_test_accept_and_local_share_bounded_resource_exclusion() {
    let mut limits = ModelLimits::small();
    limits.accepted = ResourceVector::ZERO;
    let mut omega = Omega::new(
        limits
            .validate()
            .expect("an empty Accepted partition is a valid bounded policy"),
        ViewId(1),
        RulesId(1),
    );
    let transaction = Transaction::independent(1, 1, 10, 20);

    let test_capability = direct_checked_out(omega.kernel_step(KernelCommand::BeginDirect {
        request: DirectRequestId(1),
        kind: DirectKind::TestAccept,
        transaction: transaction.clone(),
    }));
    let test_evidence = ResolvedEvidence::for_transaction(
        &transaction,
        omega.authority.chain,
        omega.authority.rules,
    )
    .expect("direct transaction has no dep-group expansion");
    let authority_before = omega.authority.clone();
    assert_eq!(
        omega.kernel_step(KernelCommand::CompleteDirect(DirectCompletion {
            capability: test_capability,
            wall_time: 10,
            result: DirectWorkResult::Verified(test_evidence),
        })),
        KernelStep::NoAuthorityCommit(KernelDisposition::DirectResourceExcluded(DirectRequestId(
            1
        ),))
    );
    assert_eq!(omega.authority, authority_before);

    let local_capability = direct_checked_out(omega.kernel_step(KernelCommand::BeginDirect {
        request: DirectRequestId(2),
        kind: DirectKind::Local,
        transaction: transaction.clone(),
    }));
    let local_evidence = ResolvedEvidence::for_transaction(
        &transaction,
        omega.authority.chain,
        omega.authority.rules,
    )
    .expect("direct transaction has no dep-group expansion");
    let authority_before = omega.authority.clone();
    assert_eq!(
        omega.kernel_step(KernelCommand::CompleteDirect(DirectCompletion {
            capability: local_capability,
            wall_time: 10,
            result: DirectWorkResult::Verified(local_evidence),
        })),
        KernelStep::NoAuthorityCommit(KernelDisposition::DirectResourceExcluded(DirectRequestId(
            2
        ),))
    );
    assert_eq!(omega.authority, authority_before);
    assert!(!omega.authority.owners.contains_key(&transaction.id));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_direct_positive_dependency_loss_is_a_relevant_change_not_policy_rejection() {
    let parent = Transaction::independent(1, 1, 10, 20);
    let child = Transaction::dependent(2, 2, 20, 30);

    for (request, kind) in [
        (DirectRequestId(1), DirectKind::TestAccept),
        (DirectRequestId(2), DirectKind::Local),
    ] {
        let mut omega = model();
        accept(&mut omega, &parent, 7, 10);
        let capability = direct_checked_out(omega.kernel_step(KernelCommand::BeginDirect {
            request,
            kind,
            transaction: child.clone(),
        }));
        let evidence = ResolvedEvidence::with_pool_input(
            &child,
            omega.authority.chain,
            omega.authority.rules,
            CellId(20),
            parent.id,
        )
        .expect("pool input belongs to a direct transaction input");
        omega.kernel_step(KernelCommand::Remove {
            transaction: parent.id,
        });
        let authority_before = omega.authority.clone();
        assert_eq!(
            omega.kernel_step(KernelCommand::CompleteDirect(DirectCompletion {
                capability,
                wall_time: 20,
                result: DirectWorkResult::Verified(evidence),
            })),
            KernelStep::NoAuthorityCommit(KernelDisposition::DirectRelevantChange(request))
        );
        assert_eq!(omega.authority, authority_before);
        assert_eq!(omega.check_invariants(), Ok(()));
    }
}

#[test]
fn model_ready_capture_commits_the_unchanged_strict_priority_prefix_with_one_stamp() {
    let first = Transaction::independent(1, 1, 10, 20);
    let second = Transaction::independent(2, 2, 11, 21);
    let mut omega = model();
    drive_ready(&mut omega, &first, 7);
    drive_ready(&mut omega, &second, 8);
    let capture = ready_capture(omega.kernel_step(KernelCommand::CaptureReady { limit: 2 }));
    omega.kernel_step(KernelCommand::Remove {
        transaction: second.id,
    });
    let before = omega.authority.last_apply;
    assert_eq!(
        omega
            .kernel_step(KernelCommand::FinalizeCaptured {
                capture,
                wall_time: 10,
            })
            .disposition(),
        &KernelDisposition::Accepted(first.id)
    );
    assert_eq!(omega.authority.last_apply.0, before.0 + 1);
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_ready_capture_commits_every_unchanged_owner_before_a_mutated_weaker_tail() {
    let first = Transaction::independent(1, 1, 10, 20);
    let second = Transaction::independent(2, 2, 11, 21);
    let third = Transaction::independent(3, 3, 12, 22);
    let mut omega = model();
    drive_ready(&mut omega, &first, 7);
    drive_ready(&mut omega, &second, 8);
    drive_ready(&mut omega, &third, 9);
    let capture = ready_capture(omega.kernel_step(KernelCommand::CaptureReady { limit: 3 }));
    omega.kernel_step(KernelCommand::Remove {
        transaction: third.id,
    });
    let before = omega.authority.last_apply;

    assert_eq!(
        omega.kernel_step(KernelCommand::FinalizeCaptured {
            capture,
            wall_time: 10,
        }),
        KernelStep::AuthorityCommit {
            stamp: ApplyStamp(before.0 + 1),
            disposition: KernelDisposition::AcceptedBatch(vec![first.id, second.id]),
        }
    );
    assert_eq!(omega.authority.last_apply.0, before.0 + 1);
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_multi_owner_apply_uses_one_stamp_distinct_versions_and_canonical_effect_order() {
    let first = Transaction::independent(1, 1, 10, 20);
    let second = Transaction::independent(2, 2, 11, 21);
    let mut omega = model();
    drive_ready(&mut omega, &first, 7);
    drive_ready(&mut omega, &second, 8);
    let first_version = omega
        .authority
        .owners
        .get(&first.id)
        .map(|owner| owner.version)
        .expect("first owner exists");
    let second_version = omega
        .authority
        .owners
        .get(&second.id)
        .map(|owner| owner.version)
        .expect("second owner exists");
    assert_ne!(first_version, second_version);
    let next_version_before = omega.authority.next_version;
    let capture = ready_capture(omega.kernel_step(KernelCommand::CaptureReady { limit: 2 }));
    let apply_before = omega.authority.last_apply;
    let effects_before = omega.authority.effects.len();

    let step = omega.kernel_step(KernelCommand::FinalizeCaptured {
        capture,
        wall_time: 10,
    });
    let stamp = match step {
        KernelStep::AuthorityCommit {
            stamp,
            disposition: KernelDisposition::AcceptedBatch(ids),
        } if ids == vec![first.id, second.id] => stamp,
        other => panic!("expected two-owner Apply, got {other:?}"),
    };
    assert_eq!(stamp.0, apply_before.0 + 1);
    assert_eq!(omega.authority.last_apply, stamp);
    assert_eq!(
        omega
            .authority
            .owners
            .get(&first.id)
            .map(|owner| owner.version),
        Some(EntryVersion(next_version_before))
    );
    assert_eq!(
        omega
            .authority
            .owners
            .get(&second.id)
            .map(|owner| owner.version),
        Some(EntryVersion(next_version_before + 1))
    );
    assert_ne!(
        omega
            .authority
            .owners
            .get(&first.id)
            .map(|owner| owner.version),
        Some(first_version)
    );
    assert_ne!(
        omega
            .authority
            .owners
            .get(&second.id)
            .map(|owner| owner.version),
        Some(second_version)
    );
    assert_eq!(omega.authority.next_version, next_version_before + 2);
    assert_eq!(
        omega
            .authority
            .effects
            .iter()
            .skip(effects_before)
            .map(|effect| (effect.stamp, effect.ordinal, effect.logical.clone()))
            .collect::<Vec<_>>(),
        vec![
            (
                stamp,
                0,
                LogicalEffect::admitted(&first, AcceptedStatus::Pending, Some(PeerId(7)),),
            ),
            (
                stamp,
                1,
                LogicalEffect::admitted(&second, AcceptedStatus::Pending, Some(PeerId(8)),),
            ),
        ]
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_ready_capture_never_skips_a_new_stronger_head() {
    let first = Transaction::independent(1, 1, 10, 20);
    let mut stronger = Transaction::independent(2, 2, 11, 21);
    stronger = stronger.with_fee(100);
    let mut omega = model();
    drive_ready(&mut omega, &first, 7);
    let capture = ready_capture(omega.kernel_step(KernelCommand::CaptureReady { limit: 1 }));
    drive_ready(&mut omega, &stronger, 8);
    let before = omega.clone();
    assert_eq!(
        omega.kernel_step(KernelCommand::FinalizeCaptured {
            capture,
            wall_time: 10,
        }),
        KernelStep::NoAuthorityCommit(KernelDisposition::ReadyCutChanged)
    );
    assert_eq!(omega, before);
}

#[test]
fn model_counter_exhaustion_is_a_mutation_free_ordinary_outcome() {
    let mut omega = model();
    omega.authority.last_apply = ApplyStamp(u16::MAX);
    let before = omega.clone();
    assert_eq!(
        omega.kernel_step(remote(Transaction::independent(1, 1, 10, 20), 7)),
        KernelStep::NoAuthorityCommit(KernelDisposition::CounterExhausted)
    );
    assert_eq!(omega, before);
}

#[test]
fn model_cancel_retires_a_checked_out_capability_exactly_once() {
    let transaction = Transaction::independent(1, 1, 10, 20);
    let mut omega = model();
    omega.kernel_step(remote(transaction.clone(), 7));
    let capability = checked_out(omega.kernel_step(KernelCommand::Checkout));
    assert!(matches!(
        omega.kernel_step(KernelCommand::CancelCapability(capability)),
        KernelStep::AuthorityCommit { .. }
    ));
    assert!(!omega.authority.owners.contains_key(&transaction.id));
    assert_eq!(
        omega.linear.free_compute_permits,
        omega.authority.limits.compute_permits
    );
    assert_eq!(
        omega.kernel_step(KernelCommand::CancelCapability(capability)),
        KernelStep::NoAuthorityCommit(KernelDisposition::StaleCapabilityRetired(capability))
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_deterministic_replay_produces_identical_states_and_dispositions() {
    let transaction = Transaction::independent(1, 1, 10, 20);
    let commands = vec![remote(transaction, 7), KernelCommand::Checkout];
    let mut left = model();
    let mut right = model();
    let left_steps = invariant_after_each(&mut left, commands.clone())
        .expect("valid deterministic model sequence");
    let right_steps =
        invariant_after_each(&mut right, commands).expect("valid deterministic model sequence");
    assert_eq!(left_steps, right_steps);
    assert_eq!(left, right);
}

#[test]
fn model_payload_variant_is_an_ordinary_outcome_and_same_witness_promotion_is_atomic() {
    let first = Transaction::independent(1, 1, 10, 20);
    let variant = Transaction::independent(1, 2, 10, 20);
    let mut omega = model();
    omega.kernel_step(remote(first.clone(), 7));
    let before_variant = omega.clone();
    assert_eq!(
        omega
            .kernel_step(retained(variant, RetainedSource::Proposal))
            .disposition(),
        &KernelDisposition::PayloadVariant(first.id)
    );
    assert_eq!(omega, before_variant);
    assert_eq!(
        omega
            .kernel_step(retained(first.clone(), RetainedSource::Proposal))
            .disposition(),
        &KernelDisposition::Promoted(first.id)
    );
    assert_eq!(
        omega
            .authority
            .owners
            .get(&first.id)
            .and_then(|owner| owner.retained_source()),
        Some(super::state::Source::Proposal {
            base: ProposalBase::Remote(RemoteResidency::new(PeerId(7), RemoteDeadline(u64::MAX),)),
        })
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_proposal_promotion_reclassifies_remote_wait_without_changing_owner_identity() {
    let transaction = Transaction::independent(1, 1, 10, 20);
    let mut omega = model();
    omega.kernel_step(remote_until(transaction.clone(), 7, 10));
    let resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    omega.kernel_step(KernelCommand::Complete(Completion {
        capability: resolve,
        result: WorkResult::Missing(missing(&transaction, BTreeSet::from([CellId(10)]))),
    }));
    let owner_before = omega.authority.owners[&transaction.id].clone();

    assert_eq!(
        omega
            .kernel_step(retained(transaction.clone(), RetainedSource::Proposal))
            .disposition(),
        &KernelDisposition::Promoted(transaction.id)
    );
    let owner_after = &omega.authority.owners[&transaction.id];
    assert_eq!(owner_after.version, owner_before.version);
    assert_eq!(owner_after.arrival, owner_before.arrival);
    assert!(matches!(
        owner_after.location,
        OwnerLocation::Retained(RetainedOwner {
            source: super::state::Source::Proposal {
                base: ProposalBase::Remote(_),
            },
            phase: RetainedPhase::Queued(WorkStage::Resolve),
        })
    ));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_proposal_collision_is_bounded_and_never_aliases_full_transaction_identity() {
    let first = Transaction::independent(1, 1, 10, 20);
    let mut collision = Transaction::independent(2, 2, 11, 21);
    collision.proposal = first.proposal;
    let mut omega = model();
    omega.kernel_step(remote(first.clone(), 7));
    let authority_before = omega.authority.clone();

    assert_eq!(
        omega.kernel_step(remote(collision.clone(), 8)),
        KernelStep::NoAuthorityCommit(KernelDisposition::ProposalCollision(collision.id))
    );
    assert_eq!(omega.authority, authority_before);
    assert_eq!(omega.proposal_owner(first.proposal), Some(first.id));

    let direct = direct_checked_out(omega.kernel_step(KernelCommand::BeginDirect {
        request: DirectRequestId(1),
        kind: DirectKind::TestAccept,
        transaction: collision.clone(),
    }));
    let evidence =
        ResolvedEvidence::for_transaction(&collision, omega.authority.chain, omega.authority.rules)
            .expect("direct transaction has no dep-group expansion");
    assert_eq!(
        omega.kernel_step(KernelCommand::CompleteDirect(DirectCompletion {
            capability: direct,
            wall_time: 10,
            result: DirectWorkResult::Verified(evidence),
        })),
        KernelStep::NoAuthorityCommit(KernelDisposition::DirectResourceExcluded(DirectRequestId(
            1
        ),))
    );
    assert_eq!(omega.authority, authority_before);
    assert_eq!(
        omega.linear.free_compute_permits,
        omega.authority.limits.compute_permits
    );

    assert_eq!(
        omega
            .kernel_step(KernelCommand::ReconcileChain(ChainTransition {
                context: super::kernel::ChainContextTransition::from_primitives(
                    super::state::RulesId(1),
                    true,
                ),
                from: omega.authority.chain,
                to_tip: ViewId(2),
                committed: BTreeSet::new(),
                available_cells: BTreeSet::new(),
                available_headers: BTreeSet::new(),
                lost_cells: BTreeSet::new(),
                lost_headers: BTreeSet::new(),
                conflicting_cells: BTreeSet::new(),
                recovered: vec![collision.clone()],
                proposals: super::proposal::ProposalContext::empty().view(),
            }))
            .disposition(),
        &KernelDisposition::ChainReconciled {
            removed: Vec::new(),
            recovered: Vec::new(),
            recovery_excluded: vec![collision.id],
        }
    );
    assert_eq!(omega.proposal_owner(first.proposal), Some(first.id));
    assert!(!omega.authority.owners.contains_key(&collision.id));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_relay_query_uses_full_raw_identity_not_proposal_projection() {
    let resident = Transaction::independent(1, 1, 10, 20);
    let mut colliding_request = Transaction::independent(2, 2, 11, 21);
    colliding_request.proposal = resident.proposal;
    let mut omega = model();
    accept(&mut omega, &resident, 7, 10);

    assert_eq!(
        relay_query_owner(&omega, &resident, RelayLookupIdentity::FullRaw),
        Some(resident.id)
    );
    assert_eq!(
        relay_query_owner(&omega, &colliding_request, RelayLookupIdentity::FullRaw),
        None,
        "a full-hash miss is not a proposal-index hit"
    );
    assert_eq!(
        relay_query_owner(
            &omega,
            &colliding_request,
            RelayLookupIdentity::ProposalShort,
        ),
        Some(resident.id),
        "the shorter projection constructs the collision counterexample"
    );

    let child = Transaction::dependent(3, 3, 20, 30);
    let colliding_parent_evidence = ResolvedEvidence::with_pool_input(
        &child,
        omega.authority.chain,
        omega.authority.rules,
        CellId(20),
        colliding_request.id,
    )
    .expect("the child input can name the alternate raw producer in the counterexample");
    assert_eq!(
        omega
            .kernel_step(retained(child.clone(), RetainedSource::Proposal))
            .disposition(),
        &KernelDisposition::Retained(child.id)
    );
    let resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    assert_eq!(
        omega
            .kernel_step(KernelCommand::Complete(Completion {
                capability: resolve,
                result: WorkResult::Resolved(colliding_parent_evidence),
            }))
            .disposition(),
        &KernelDisposition::Continued(child.id)
    );
    assert!(matches!(
        omega.authority.owners[&child.id].location,
        OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Queued(WorkStage::Resolve),
            ..
        })
    ));

    let before_miss = omega.clone();
    assert_eq!(
        omega.kernel_step(KernelCommand::Remove {
            transaction: colliding_request.id,
        }),
        KernelStep::NoAuthorityCommit(KernelDisposition::Removed(Vec::new()))
    );
    assert_eq!(omega, before_miss);

    let mut removal = model();
    accept(&mut removal, &resident, 7, 10);
    let exact_parent_evidence = ResolvedEvidence::with_pool_input(
        &child,
        removal.authority.chain,
        removal.authority.rules,
        CellId(20),
        resident.id,
    )
    .expect("the child input names the exact resident producer");
    accept_with_evidence(&mut removal, &child, exact_parent_evidence, 20);
    let before_miss = removal.clone();
    assert_eq!(
        removal.kernel_step(KernelCommand::Remove {
            transaction: colliding_request.id,
        }),
        KernelStep::NoAuthorityCommit(KernelDisposition::Removed(Vec::new()))
    );
    assert_eq!(removal, before_miss);
    assert_eq!(
        removal
            .kernel_step(KernelCommand::Remove {
                transaction: resident.id,
            })
            .disposition(),
        &KernelDisposition::Removed(vec![resident.id, child.id])
    );
    assert!(removal.authority.owners.is_empty());
    assert_eq!(removal.check_invariants(), Ok(()));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_per_peer_resource_exclusion_is_pre_apply_and_other_peers_remain_independent() {
    let mut limits = ModelLimits::small();
    limits.remote_per_peer.entries = 1;
    let validated = limits
        .validate()
        .expect("the per-peer partition remains within retained limits");
    let mut omega = Omega::new(validated, ViewId(1), RulesId(1));
    let first = Transaction::independent(1, 1, 10, 20);
    let same_peer = Transaction::independent(2, 2, 11, 21);
    let other_peer = Transaction::independent(3, 3, 12, 22);
    omega.kernel_step(remote(first, 7));
    let before = omega.clone();
    assert_eq!(
        omega.kernel_step(remote(same_peer.clone(), 7)),
        KernelStep::NoAuthorityCommit(KernelDisposition::ResourceRejected(same_peer.id))
    );
    assert_eq!(omega, before);
    assert_eq!(
        omega
            .kernel_step(remote(other_peer.clone(), 8))
            .disposition(),
        &KernelDisposition::Retained(other_peer.id)
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_effect_capacity_wait_is_mutation_free_and_keeps_the_ready_owner() {
    let mut limits = ModelLimits::small();
    limits.effects.remote.batches = 1;
    limits.effects.trusted_headroom.batches = 4;
    let ordinary_batches = limits
        .effects
        .remote
        .batches
        .checked_add(limits.effects.trusted_headroom.batches)
        .expect("the small ordinary batch partition is representable");
    let validated = limits
        .validate()
        .expect("the journal holds the largest owner-derived effect batch");
    let mut omega = Omega::new(validated, ViewId(1), RulesId(1));
    let second = Transaction::independent(2, 2, 11, 21);

    for index in 0..ordinary_batches {
        let transaction = Transaction::independent(
            10 + u8::try_from(index).expect("small model record count"),
            10 + u8::try_from(index).expect("small model record count"),
            30 + u8::try_from(index).expect("small model record count"),
            40 + u8::try_from(index).expect("small model record count"),
        );
        let capability = direct_checked_out(omega.kernel_step(KernelCommand::BeginDirect {
            request: DirectRequestId(10 + index),
            kind: DirectKind::Local,
            transaction: transaction.clone(),
        }));
        let evidence = omega.capture_direct_negative(&transaction, DirectNegativeReason::Policy);
        assert!(matches!(
            omega.kernel_step(KernelCommand::CompleteDirect(DirectCompletion {
                capability,
                wall_time: 10,
                result: DirectWorkResult::Rejected(evidence),
            })),
            KernelStep::AuthorityCommit {
                disposition: KernelDisposition::DirectRejected(_, _),
                ..
            }
        ));
    }
    assert_eq!(omega.authority.effects.len(), usize::from(ordinary_batches));
    drive_ready(&mut omega, &second, 8);
    let before = omega.clone();
    assert_eq!(
        omega
            .kernel_step(KernelCommand::FinalizeNext { wall_time: 20 })
            .disposition(),
        &KernelDisposition::EffectCapacityWait(second.id)
    );
    assert_eq!(omega, before);
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_effect_payload_bytes_can_saturate_before_record_count() {
    let mut limits = ModelLimits::small();
    let Some((largest_batch_effects, largest_batch_bytes)) =
        limits.largest_indivisible_effect_batch()
    else {
        panic!("the small model effect bound is representable")
    };
    let ordinary_bytes = 192u32;
    limits.effects.remote.bytes = largest_batch_bytes;
    limits.effects.trusted_headroom.bytes = ordinary_bytes
        .checked_sub(largest_batch_bytes)
        .expect("the byte-saturation fixture covers one indivisible batch");
    let batch_bound = EffectBatchBound::new(largest_batch_effects, largest_batch_bytes);
    limits.effects.remote_bound = batch_bound;
    limits.effects.trusted_bound = batch_bound;
    limits.effects.critical_bound = batch_bound;
    let mut omega = Omega::new(
        limits
            .validate()
            .expect("the byte-saturation fixture keeps every batch representable"),
        ViewId(1),
        RulesId(1),
    );
    for index in 0..3u16 {
        let byte = u8::try_from(index).expect("three fixtures fit u8");
        let transaction = Transaction::independent(10 + byte, 10 + byte, 30 + byte, 40 + byte)
            .with_payload_bytes(60)
            .expect("the finite payload size leaves room for the block offset");
        let capability = direct_checked_out(omega.kernel_step(KernelCommand::BeginDirect {
            request: DirectRequestId(10 + index),
            kind: DirectKind::Local,
            transaction: transaction.clone(),
        }));
        let evidence = omega.capture_direct_negative(&transaction, DirectNegativeReason::Policy);
        assert!(matches!(
            omega.kernel_step(KernelCommand::CompleteDirect(DirectCompletion {
                capability,
                wall_time: 10,
                result: DirectWorkResult::Rejected(evidence),
            })),
            KernelStep::AuthorityCommit {
                disposition: KernelDisposition::DirectRejected(_, _),
                ..
            }
        ));
    }
    assert_eq!(
        omega.effect_usage(),
        Some(EffectUsage {
            remote: EffectCapacity::default(),
            ordinary: EffectCapacity::new(3, ordinary_bytes),
            total: EffectCapacity::new(3, ordinary_bytes),
        })
    );

    let candidate = Transaction::independent(1, 1, 10, 20);
    drive_ready(&mut omega, &candidate, 7);
    let before = omega.clone();
    assert_eq!(
        omega
            .kernel_step(KernelCommand::FinalizeNext { wall_time: 20 })
            .disposition(),
        &KernelDisposition::EffectCapacityWait(candidate.id)
    );
    assert_eq!(omega, before);
    let ordinary_batches = limits.effects.remote.batches + limits.effects.trusted_headroom.batches;
    assert!(omega.authority.effects.len() < usize::from(ordinary_batches));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_effect_regions_preserve_trusted_and_critical_headroom() {
    let mut omega = model();
    let limits = omega.authority.limits.effects;
    let remote_effect = vec![LogicalEffect::IngressReleased(TxId(200))];
    let trusted_effect = vec![LogicalEffect::IngressReleased(TxId(201))];
    let critical_effect = vec![LogicalEffect::PeerCohortRevoked(PeerId(9))];

    let mut remote_batches = 0u16;
    while omega.append_effect_fixture(super::state::EffectClass::Remote, remote_effect.clone()) {
        remote_batches += 1;
    }
    assert_eq!(remote_batches, limits.remote.batches);
    assert!(omega.can_append_effects(super::state::EffectClass::Trusted, &trusted_effect));
    assert!(!omega.can_append_effects(super::state::EffectClass::Remote, &remote_effect));

    let mut trusted_batches = 0u16;
    while omega.append_effect_fixture(super::state::EffectClass::Trusted, trusted_effect.clone()) {
        trusted_batches += 1;
    }
    assert_eq!(trusted_batches, limits.trusted_headroom.batches);
    assert!(omega.can_append_effects(super::state::EffectClass::Critical, &critical_effect));
    assert!(!omega.can_append_effects(super::state::EffectClass::Trusted, &trusted_effect));

    let mut critical_batches = 0u16;
    while omega.append_effect_fixture(super::state::EffectClass::Critical, critical_effect.clone())
    {
        critical_batches += 1;
    }
    assert_eq!(critical_batches, limits.critical_headroom.batches);
    assert!(!omega.can_append_effects(super::state::EffectClass::Critical, &critical_effect));
    let usage_before_reset = omega.effect_usage();
    let reset = vec![LogicalEffect::GenerationReset];
    assert!(omega.can_append_effects(super::state::EffectClass::Critical, &reset));
    assert!(omega.append_effect_fixture(super::state::EffectClass::Critical, reset));
    assert_eq!(omega.effect_usage(), usage_before_reset);
    assert!(omega.authority.latest_generation_reset.is_some());
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_partially_settled_effect_batch_retains_its_full_capacity_charge() {
    let mut omega = model();
    let effects = vec![
        LogicalEffect::IngressReleased(TxId(200)),
        LogicalEffect::IngressReleased(TxId(201)),
    ];
    assert!(omega.append_effect_fixture(super::state::EffectClass::Trusted, effects));
    let charged = EffectUsage {
        remote: EffectCapacity::default(),
        ordinary: EffectCapacity::new(1, 8),
        total: EffectCapacity::new(1, 8),
    };
    assert_eq!(omega.effect_usage(), Some(charged));

    let first = match omega.kernel_step(KernelCommand::ClaimEffect) {
        KernelStep::NoAuthorityCommit(KernelDisposition::EffectClaimed(claim)) => claim,
        other => panic!("expected first effect claim, got {other:?}"),
    };
    assert!(matches!(
        omega.kernel_step(KernelCommand::SettleEffect(first)),
        KernelStep::AuthorityCommit { .. }
    ));
    assert_eq!(omega.effect_usage(), Some(charged));
    assert_eq!(omega.check_invariants(), Ok(()));

    let second = match omega.kernel_step(KernelCommand::ClaimEffect) {
        KernelStep::NoAuthorityCommit(KernelDisposition::EffectClaimed(claim)) => claim,
        other => panic!("expected second effect claim, got {other:?}"),
    };
    assert!(matches!(
        omega.kernel_step(KernelCommand::SettleEffect(second)),
        KernelStep::AuthorityCommit { .. }
    ));
    assert_eq!(omega.effect_usage(), Some(EffectUsage::default()));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_invariant_basis_rejects_removed_resource_version_and_effect_premises() {
    let first = Transaction::independent(1, 1, 10, 20);
    let second = Transaction::independent(2, 2, 11, 21);
    let mut duplicate_version = model();
    duplicate_version.kernel_step(remote(first.clone(), 7));
    duplicate_version.kernel_step(remote(second.clone(), 8));
    let version = duplicate_version
        .authority
        .owners
        .get(&first.id)
        .map(|owner| owner.version)
        .expect("first model owner exists");
    duplicate_version
        .authority
        .owners
        .get_mut(&TxId(2))
        .expect("second model owner exists")
        .version = version;
    assert_eq!(
        duplicate_version.check_invariants(),
        Err(ModelInvariantError::DuplicateOwnerVersion)
    );

    let mut duplicate_proposal = model();
    duplicate_proposal.kernel_step(remote(first.clone(), 7));
    duplicate_proposal.kernel_step(remote(second.clone(), 8));
    duplicate_proposal
        .authority
        .owners
        .get_mut(&second.id)
        .expect("the second model owner exists")
        .transaction
        .proposal = first.proposal;
    assert_eq!(
        duplicate_proposal.check_invariants(),
        Err(ModelInvariantError::DuplicateProposalId)
    );

    let mut unbounded_peer = model();
    unbounded_peer.kernel_step(remote(first.clone(), 7));
    unbounded_peer.authority.limits.remote_per_peer = ResourceVector::ZERO;
    assert_eq!(
        unbounded_peer.check_invariants(),
        Err(ModelInvariantError::RemotePeerResourceLimit)
    );

    let mut stale_effect_claim = model();
    accept(&mut stale_effect_claim, &first, 7, 10);
    let claim = match stale_effect_claim.kernel_step(KernelCommand::ClaimEffect) {
        KernelStep::NoAuthorityCommit(KernelDisposition::EffectClaimed(claim)) => claim,
        other => panic!("expected effect claim, got {other:?}"),
    };
    stale_effect_claim.authority.effects.pop_front();
    assert_eq!(stale_effect_claim.linear.effect_claim, Some(claim));
    assert_eq!(
        stale_effect_claim.check_invariants(),
        Err(ModelInvariantError::EffectClaim)
    );

    let mut owner_key = model();
    owner_key.kernel_step(remote(first.clone(), 7));
    let owner = owner_key
        .authority
        .owners
        .remove(&first.id)
        .expect("the admitted owner exists");
    owner_key.authority.owners.insert(TxId(9), owner);
    assert_eq!(
        owner_key.check_invariants(),
        Err(ModelInvariantError::OwnerKey)
    );

    let mut stale_evidence = model();
    drive_ready(&mut stale_evidence, &first, 7);
    let Some(owner) = stale_evidence.authority.owners.get_mut(&first.id) else {
        panic!("the Ready owner exists")
    };
    let OwnerLocation::Retained(RetainedOwner {
        phase: RetainedPhase::Ready(evidence),
        ..
    }) = &mut owner.location
    else {
        panic!("the owner is Ready")
    };
    evidence.context.witness = super::state::WitnessId(9);
    assert_eq!(
        stale_evidence.check_invariants(),
        Err(ModelInvariantError::InvalidStoredEvidence)
    );

    let mut header_transaction = Transaction::independent(5, 5, 14, 24);
    header_transaction.header_deps.insert(HeaderId(1));
    let mut stale_header_evidence = model();
    drive_ready(&mut stale_header_evidence, &header_transaction, 7);
    let Some(owner) = stale_header_evidence
        .authority
        .owners
        .get_mut(&header_transaction.id)
    else {
        panic!("the header-dependent Ready owner exists")
    };
    let OwnerLocation::Retained(RetainedOwner {
        phase: RetainedPhase::Ready(evidence),
        ..
    }) = &mut owner.location
    else {
        panic!("the header-dependent owner is Ready")
    };
    evidence.header_deps.clear();
    assert_eq!(
        stale_header_evidence.check_invariants(),
        Err(ModelInvariantError::InvalidStoredEvidence)
    );

    let foreign_history = Transaction::independent(6, 6, 15, 25);
    let mut invalid_history = model();
    invalid_history.kernel_step(remote(header_transaction.clone(), 7));
    invalid_history
        .authority
        .owners
        .get_mut(&header_transaction.id)
        .expect("the retained header owner exists")
        .location = OwnerLocation::ReplacementHistory {
        missing: missing(&foreign_history, BTreeSet::from([CellId(15)])),
    };
    assert_eq!(
        invalid_history.check_invariants(),
        Err(ModelInvariantError::InvalidReplacementHistory)
    );

    let parent = Transaction::independent(3, 3, 12, 22);
    let child = Transaction::dependent(4, 4, 22, 32);
    let mut missing_parent = model();
    accept(&mut missing_parent, &parent, 7, 10);
    let child_evidence = ResolvedEvidence::with_pool_input(
        &child,
        missing_parent.authority.chain,
        missing_parent.authority.rules,
        CellId(22),
        parent.id,
    )
    .expect("pool input belongs to a direct transaction input");
    drive_ready_with_evidence(
        &mut missing_parent,
        &child,
        RetainedSource::Proposal,
        child_evidence,
    );
    missing_parent.kernel_step(KernelCommand::FinalizeNext { wall_time: 20 });

    let mut stale_chain_origin = missing_parent.clone();
    let Some(owner) = stale_chain_origin.authority.owners.get_mut(&child.id) else {
        panic!("the accepted child exists")
    };
    let OwnerLocation::Accepted { evidence, .. } = &mut owner.location else {
        panic!("the child is Accepted")
    };
    evidence
        .input_origins
        .insert(CellId(22), super::state::InputOrigin::Chain);
    assert_eq!(
        stale_chain_origin.check_invariants(),
        Err(ModelInvariantError::StaleChainOrigin)
    );

    missing_parent.authority.owners.remove(&parent.id);
    assert_eq!(
        missing_parent.check_invariants(),
        Err(ModelInvariantError::MissingPoolParent)
    );

    let second_input = Transaction::independent(2, 2, 11, 21);
    let mut duplicate_output = model();
    accept(&mut duplicate_output, &first, 7, 10);
    accept(&mut duplicate_output, &second_input, 8, 20);
    duplicate_output
        .authority
        .owners
        .get_mut(&second_input.id)
        .expect("the second Accepted owner exists")
        .transaction
        .outputs = BTreeSet::from([CellId(20)]);
    assert_eq!(
        duplicate_output.check_invariants(),
        Err(ModelInvariantError::DuplicateAcceptedOutput)
    );

    let mut causal_cycle = model();
    accept(&mut causal_cycle, &first, 7, 10);
    accept(&mut causal_cycle, &second_input, 8, 20);
    let Some(first_owner) = causal_cycle.authority.owners.get_mut(&first.id) else {
        panic!("the first Accepted owner exists")
    };
    first_owner.transaction.inputs = BTreeSet::from([CellId(21)]);
    let OwnerLocation::Accepted { evidence, .. } = &mut first_owner.location else {
        panic!("the first owner is Accepted")
    };
    evidence.input_origins =
        BTreeMap::from([(CellId(21), super::state::InputOrigin::Pool(second_input.id))]);
    let Some(second_owner) = causal_cycle.authority.owners.get_mut(&second_input.id) else {
        panic!("the second Accepted owner exists")
    };
    second_owner.transaction.inputs = BTreeSet::from([CellId(20)]);
    let OwnerLocation::Accepted { evidence, .. } = &mut second_owner.location else {
        panic!("the second owner is Accepted")
    };
    evidence.input_origins =
        BTreeMap::from([(CellId(20), super::state::InputOrigin::Pool(first.id))]);
    assert_eq!(
        causal_cycle.check_invariants(),
        Err(ModelInvariantError::AcceptedCausalCycle)
    );

    let mut double_spend = model();
    accept(&mut double_spend, &first, 7, 10);
    accept(&mut double_spend, &second_input, 8, 20);
    let Some(owner) = double_spend.authority.owners.get_mut(&second_input.id) else {
        panic!("the second Accepted owner exists")
    };
    owner.transaction.inputs = BTreeSet::from([CellId(10)]);
    let OwnerLocation::Accepted { evidence, .. } = &mut owner.location else {
        panic!("the second owner is Accepted")
    };
    evidence.input_origins = BTreeMap::from([(CellId(10), super::state::InputOrigin::Chain)]);
    assert_eq!(
        double_spend.check_invariants(),
        Err(ModelInvariantError::AcceptedDoubleSpend)
    );

    let mut permit_conservation = model();
    direct_checked_out(permit_conservation.kernel_step(KernelCommand::BeginDirect {
        request: DirectRequestId(1),
        kind: DirectKind::TestAccept,
        transaction: first.clone(),
    }));
    permit_conservation.linear.free_compute_permits += 1;
    assert_eq!(
        permit_conservation.check_invariants(),
        Err(ModelInvariantError::CapabilityPermitConservation)
    );

    let mut duplicate_direct_request = model();
    direct_checked_out(
        duplicate_direct_request.kernel_step(KernelCommand::BeginDirect {
            request: DirectRequestId(1),
            kind: DirectKind::TestAccept,
            transaction: first.clone(),
        }),
    );
    let second_direct = direct_checked_out(duplicate_direct_request.kernel_step(
        KernelCommand::BeginDirect {
            request: DirectRequestId(2),
            kind: DirectKind::Local,
            transaction: second_input.clone(),
        },
    ));
    duplicate_direct_request
        .linear
        .direct_work
        .get_mut(&second_direct)
        .expect("the second direct capability exists")
        .request = DirectRequestId(1);
    assert_eq!(
        duplicate_direct_request.check_invariants(),
        Err(ModelInvariantError::DuplicateDirectRequest)
    );

    let mut effect_order = model();
    accept(&mut effect_order, &first, 7, 10);
    accept(&mut effect_order, &second_input, 8, 20);
    let first_key = effect_order
        .authority
        .effects
        .front()
        .map(|effect| (effect.stamp, effect.ordinal))
        .expect("the first committed effect exists");
    let last = effect_order
        .authority
        .effects
        .back_mut()
        .expect("the second committed effect exists");
    (last.stamp, last.ordinal) = first_key;
    assert_eq!(
        effect_order.check_invariants(),
        Err(ModelInvariantError::EffectOrder)
    );

    let mut future_effect = model();
    accept(&mut future_effect, &first, 7, 10);
    let future_stamp = ApplyStamp(future_effect.authority.last_apply.0 + 1);
    future_effect
        .authority
        .effects
        .front_mut()
        .expect("the committed effect exists")
        .stamp = future_stamp;
    assert_eq!(
        future_effect.check_invariants(),
        Err(ModelInvariantError::EffectOrder)
    );

    let mut invalid_history = model();
    let victim = Transaction::independent(5, 5, 13, 23);
    let mut replacement = Transaction::independent(6, 6, 13, 33);
    replacement = replacement.with_fee(30);
    accept(&mut invalid_history, &victim, 7, 10);
    drive_ready(&mut invalid_history, &replacement, 8);
    invalid_history.kernel_step(KernelCommand::FinalizeNext { wall_time: 20 });
    invalid_history
        .authority
        .owners
        .get_mut(&victim.id)
        .expect("the replacement history exists")
        .location = OwnerLocation::ReplacementHistory {
        missing: missing(&replacement, BTreeSet::from([CellId(13)])),
    };
    assert_eq!(
        invalid_history.check_invariants(),
        Err(ModelInvariantError::InvalidReplacementHistory)
    );

    let mut peer_ban_order = model();
    peer_ban_order.kernel_step(KernelCommand::BanPeer {
        peer: PeerId(7),
        observed_at: MonotonicTick(1),
    });
    let impossible_order = ApplyStamp(peer_ban_order.authority.last_apply.0 + 1);
    peer_ban_order
        .authority
        .peer_bans
        .get_mut(&PeerId(7))
        .expect("the peer-ban fence exists")
        .order = impossible_order;
    assert_eq!(
        peer_ban_order.check_invariants(),
        Err(ModelInvariantError::PeerBanOrder)
    );
}

#[test]
fn model_relay_handoff_releases_known_state_on_pre_authority_failure() {
    let item = RelayItem {
        raw: TxId(1),
        witness: WitnessId(1),
    };
    let mut handoff = RelayHandoff::new(RelayLimits {
        records: 2,
        bytes: 16,
    });
    assert_eq!(
        handoff.offer(item, RelaySource::Remote(PeerId(7)), 4),
        RelayDisposition::Offered(item)
    );
    assert_eq!(
        handoff.enqueue(item, RequestId(1), false),
        RelayDisposition::Released(item)
    );
    assert!(!handoff.records.contains_key(&item.raw));
    assert_eq!(handoff.check_invariants(), Ok(()));
}

#[test]
fn model_relay_raw_identity_cannot_alias_a_witness_variant() {
    let first = RelayItem {
        raw: TxId(1),
        witness: WitnessId(1),
    };
    let witness_variant = RelayItem {
        raw: first.raw,
        witness: WitnessId(2),
    };
    let mut handoff = RelayHandoff::new(RelayLimits {
        records: 2,
        bytes: 16,
    });

    assert_eq!(
        handoff.offer(first, RelaySource::Remote(PeerId(7)), 4),
        RelayDisposition::Offered(first)
    );
    assert_eq!(
        handoff.offer(witness_variant, RelaySource::Remote(PeerId(8)), 4),
        RelayDisposition::PayloadVariant(witness_variant)
    );
    assert_eq!(handoff.records.len(), 1);
    assert_eq!(
        handoff.records.get(&first.raw).map(|record| record.item),
        Some(first)
    );
    let mut invalid_identity = handoff.clone();
    invalid_identity
        .records
        .get_mut(&first.raw)
        .expect("the raw lifecycle record exists")
        .item
        .raw = TxId(9);
    assert_eq!(
        invalid_identity.check_invariants(),
        Err(RelayInvariantError::RawIdentityMismatch)
    );

    assert_eq!(
        handoff.enqueue(first, RequestId(1), false),
        RelayDisposition::Released(first)
    );
    assert_eq!(
        handoff.offer(witness_variant, RelaySource::Remote(PeerId(8)), 4),
        RelayDisposition::Offered(witness_variant)
    );
    assert_eq!(handoff.check_invariants(), Ok(()));
}

#[test]
fn model_relay_batch_abort_releases_only_the_uncommitted_suffix() {
    let first = RelayItem {
        raw: TxId(1),
        witness: WitnessId(1),
    };
    let second = RelayItem {
        raw: TxId(2),
        witness: WitnessId(2),
    };
    let request = RequestId(1);
    let mut handoff = RelayHandoff::new(RelayLimits {
        records: 4,
        bytes: 32,
    });
    for item in [first, second] {
        handoff.offer(item, RelaySource::Proposal, 4);
        handoff.enqueue(item, request, true);
        handoff.dispatch(item, request);
    }
    handoff.authority_accept(first, request);
    assert_eq!(handoff.abort_request(request), vec![second]);
    assert_eq!(
        handoff
            .records
            .get(&first.raw)
            .map(|record| record.location),
        Some(RelayLocation::AuthorityOwned)
    );
    assert!(!handoff.records.contains_key(&second.raw));
    assert_eq!(
        handoff.settle(first, RelayTerminal::Accepted),
        RelayDisposition::KnownSettled(first)
    );
    assert_eq!(handoff.forget(first), RelayDisposition::Forgotten(first));
    assert_eq!(handoff.check_invariants(), Ok(()));
}

#[test]
fn model_peer_ban_releases_only_matching_pre_authority_relay_handoffs() {
    let queued = RelayItem {
        raw: TxId(1),
        witness: WitnessId(1),
    };
    let authority_owned = RelayItem {
        raw: TxId(2),
        witness: WitnessId(2),
    };
    let other_peer = RelayItem {
        raw: TxId(3),
        witness: WitnessId(3),
    };
    let proposal = RelayItem {
        raw: TxId(4),
        witness: WitnessId(4),
    };
    let mut handoff = RelayHandoff::new(RelayLimits {
        records: 4,
        bytes: 16,
    });
    for item in [queued, authority_owned] {
        handoff.offer(item, RelaySource::Remote(PeerId(7)), 4);
        handoff.enqueue(item, RequestId(1), true);
    }
    handoff.dispatch(authority_owned, RequestId(1));
    handoff.authority_accept(authority_owned, RequestId(1));
    handoff.offer(other_peer, RelaySource::Remote(PeerId(8)), 4);
    handoff.offer(proposal, RelaySource::Proposal, 4);

    assert_eq!(
        handoff.revoke_peer_before_authority(PeerId(7)),
        vec![queued]
    );
    assert!(!handoff.records.contains_key(&queued.raw));
    assert!(handoff.records.contains_key(&authority_owned.raw));
    assert!(handoff.records.contains_key(&other_peer.raw));
    assert!(handoff.records.contains_key(&proposal.raw));
    assert_eq!(handoff.check_invariants(), Ok(()));
}

#[test]
fn model_relay_terminal_rejection_releases_the_exact_authority_handoff() {
    let item = RelayItem {
        raw: TxId(1),
        witness: WitnessId(1),
    };
    let request = RequestId(1);
    let mut handoff = RelayHandoff::new(RelayLimits {
        records: 2,
        bytes: 16,
    });
    handoff.offer(item, RelaySource::Proposal, 4);
    handoff.enqueue(item, request, true);
    handoff.dispatch(item, request);
    handoff.authority_accept(item, request);
    assert_eq!(
        handoff.settle(item, RelayTerminal::UnknownParents),
        RelayDisposition::Released(item)
    );
    assert!(!handoff.records.contains_key(&item.raw));

    let second = RelayItem {
        raw: TxId(2),
        witness: WitnessId(2),
    };
    handoff.offer(second, RelaySource::Remote(PeerId(8)), 4);
    handoff.enqueue(second, request, true);
    handoff.dispatch(second, request);
    handoff.authority_accept(second, request);
    assert_eq!(
        handoff.settle(second, RelayTerminal::Rejected),
        RelayDisposition::Released(second)
    );
    assert_eq!(handoff.check_invariants(), Ok(()));
}

#[test]
fn model_endpoint_timeout_allows_at_most_one_detached_foreign_call() {
    let circuit = EndpointCircuit::Available.step(EndpointEvent::CallTimedOut);
    assert_eq!(circuit, EndpointCircuit::DetachedOne);
    assert_eq!(
        circuit.step(EndpointEvent::CallTimedOut),
        EndpointCircuit::DetachedOne
    );
    assert_eq!(
        circuit.step(EndpointEvent::DetachedReturned),
        EndpointCircuit::Disabled
    );
    assert_eq!(
        EndpointCircuit::Available.step(EndpointEvent::CallReturned),
        EndpointCircuit::Available
    );
    assert_eq!(
        EndpointCircuit::Available.step(EndpointEvent::Disable),
        EndpointCircuit::Disabled
    );
}

#[test]
fn model_post_apply_assignment_and_completion_delivery_failure_return_the_exact_capability() {
    let transaction = Transaction::independent(1, 1, 10, 20);
    let mut omega = model();
    omega.kernel_step(remote(transaction.clone(), 7));
    let capability = checked_out(omega.kernel_step(KernelCommand::Checkout));

    let (transport, assignment) = CapabilityTransport::new(capability).send_assignment(false);
    assert_eq!(
        assignment,
        CapabilityTransportDisposition::AssignmentReturned(capability)
    );
    assert!(matches!(
        omega.kernel_step(KernelCommand::CancelCapability(capability)),
        KernelStep::AuthorityCommit { .. }
    ));
    let (transport, settled) = transport.settle();
    assert_eq!(settled, CapabilityTransportDisposition::Settled(capability));
    assert!(transport.is_terminal());
    assert!(!omega.authority.owners.contains_key(&transaction.id));

    let second = Transaction::independent(2, 2, 11, 21);
    omega.kernel_step(remote(second.clone(), 8));
    let second_capability = checked_out(omega.kernel_step(KernelCommand::Checkout));
    let (transport, _) = CapabilityTransport::new(second_capability).send_assignment(true);
    let (transport, _) = transport.receive_assignment();
    let (transport, completion) = transport.send_completion(false);
    assert_eq!(
        completion,
        CapabilityTransportDisposition::CompletionReturned(second_capability)
    );
    assert!(matches!(
        omega.kernel_step(KernelCommand::CancelCapability(second_capability)),
        KernelStep::AuthorityCommit { .. }
    ));
    let (transport, _) = transport.settle();
    assert!(transport.is_terminal());
    assert!(!omega.authority.owners.contains_key(&second.id));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_callback_reentrancy_rejects_mutation_without_blocking_reads_or_derived_control() {
    assert_eq!(
        callback_disposition(true, CallbackAccess::AuthorityMutation),
        CallbackDisposition::ReentrantMutationRejected
    );
    assert_eq!(
        callback_disposition(true, CallbackAccess::CoherentRead),
        CallbackDisposition::Allowed
    );
    assert_eq!(
        callback_disposition(true, CallbackAccess::NonblockingDerivedControl),
        CallbackDisposition::Allowed
    );
    assert_eq!(
        callback_disposition(false, CallbackAccess::AuthorityMutation),
        CallbackDisposition::Allowed
    );
}

#[test]
fn model_derived_publication_is_versioned_and_failure_cannot_mutate_authority() {
    let mut system = running_system();
    let chain = system
        .authority
        .as_ref()
        .expect("a running system owns the authority")
        .authority
        .chain;
    assert!(matches!(
        system.step(SystemEvent::Kernel {
            access: KernelAccess::Ordinary,
            command: reconcile_view(chain, ViewId(2)),
        }),
        SystemDisposition::Kernel(KernelStep::AuthorityCommit { .. })
    ));
    assert_eq!(
        system.step(SystemEvent::PublishTemplate { captured_source: 0 }),
        SystemDisposition::StaleTemplate(0)
    );
    assert_eq!(
        system.step(SystemEvent::PublishTemplate { captured_source: 1 }),
        SystemDisposition::TemplatePublished(1)
    );
    assert_eq!(
        system.derived.template,
        DerivedComponent::Enabled {
            source: 1,
            published: 1,
        }
    );

    let authority = system.authority.clone();
    assert_eq!(
        system.step(SystemEvent::DegradeRecentReject),
        SystemDisposition::DerivedDegraded
    );
    assert_eq!(system.derived.recent_reject, DerivedHealth::Degraded);
    assert_eq!(system.authority, authority);

    let mut invalid = system;
    invalid.derived.template = DerivedComponent::Enabled {
        source: 1,
        published: 2,
    };
    assert_eq!(
        invalid.check_invariants(),
        Err(SystemInvariantError::DerivedPublicationOrder)
    );
}

#[test]
fn model_verification_suspend_blocks_checkout_without_discarding_active_work() {
    let running = VerificationControl::Running;
    assert!(running.checkout_allowed());
    let suspended = running.suspend();
    assert!(!suspended.checkout_allowed());
    assert_eq!(
        suspended.active_action(),
        ActiveVerificationAction::Continue
    );
    assert_eq!(suspended.resume(), VerificationControl::Running);
    let stopped = suspended.stop();
    assert!(!stopped.checkout_allowed());
    assert_eq!(
        stopped.active_action(),
        ActiveVerificationAction::ReturnCapability
    );
}

#[test]
fn model_template_full_preempts_reset_without_serializing_optimistic_lanes() {
    let mut template = TemplateProtocol::default();
    let reset = match template.capture(TemplateLane::Reset) {
        TemplateDisposition::Captured(receipt) => receipt,
        other => panic!("expected reset capture, got {other:?}"),
    };
    let transactions = match template.capture(TemplateLane::Transactions) {
        TemplateDisposition::Captured(receipt) => receipt,
        other => panic!("expected optimistic transaction capture, got {other:?}"),
    };
    let full = match template.capture(TemplateLane::Full) {
        TemplateDisposition::FullPreemptedReset(receipt) => receipt,
        other => panic!("expected full to preempt reset, got {other:?}"),
    };
    assert_eq!(
        template.publish(reset),
        TemplateDisposition::Stale(TemplateLane::Reset)
    );
    assert_eq!(
        template.publish(transactions),
        TemplateDisposition::Stale(TemplateLane::Transactions)
    );
    assert_eq!(
        template.publish(full),
        TemplateDisposition::Published(TemplateLane::Full)
    );
    assert_eq!(template.published, full.sources);

    assert!(template.advance(TemplateLane::Proposals));
    let proposals = match template.capture(TemplateLane::Proposals) {
        TemplateDisposition::Captured(receipt) => receipt,
        other => panic!("expected proposal capture, got {other:?}"),
    };
    let uncles = match template.capture(TemplateLane::Uncles) {
        TemplateDisposition::Captured(receipt) => receipt,
        other => panic!("expected uncle capture, got {other:?}"),
    };
    assert_eq!(
        template.publish(proposals),
        TemplateDisposition::Published(TemplateLane::Proposals)
    );
    assert_eq!(
        template.publish(uncles),
        TemplateDisposition::Published(TemplateLane::Uncles)
    );
    assert_eq!(template.published.proposals, proposals.sources.proposals);
    assert_eq!(template.published.uncles, uncles.sources.uncles);
}

#[test]
fn model_reorg_template_and_window_service_composition_commutes() {
    let window = ProposalWindow::new(2, 4).expect("the protocol window is valid");
    let resident = Transaction::independent(1, 1, 10, 20);
    let recovered = Transaction::independent(2, 2, 11, 21);
    let gap_context = ProposalContext::new(
        10,
        window,
        [ProposalBlock::new(
            10,
            BTreeSet::new(),
            BTreeSet::from([resident.proposal]),
        )],
    )
    .expect("the pre-reorg uncle history is canonical");
    let stable_context =
        ProposalContext::new(10, window, []).expect("the post-reorg proposal history is canonical");
    assert_eq!(
        gap_context.transition_to(&stable_context).changed(),
        &BTreeSet::from([resident.proposal]),
        "the primitive-history reorg has one exact sparse status delta"
    );

    let mut omega = model();
    assert!(matches!(
        omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
            context: super::kernel::ChainContextTransition::from_primitives(
                super::state::RulesId(1),
                false,
            ),
            from: omega.authority.chain,
            to_tip: ViewId(2),
            committed: BTreeSet::new(),
            available_cells: BTreeSet::new(),
            available_headers: BTreeSet::new(),
            lost_cells: BTreeSet::new(),
            lost_headers: BTreeSet::new(),
            conflicting_cells: BTreeSet::new(),
            recovered: Vec::new(),
            proposals: gap_context.view(),
        })),
        KernelStep::AuthorityCommit { .. }
    ));
    accept(&mut omega, &resident, 7, 10);
    assert_eq!(
        query_subject(&omega, resident.id),
        QuerySubject::Accepted(AcceptedStatus::Gap)
    );

    let reorg = omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(
            super::state::RulesId(1),
            true,
        ),
        from: omega.authority.chain,
        to_tip: ViewId(3),
        committed: BTreeSet::new(),
        available_cells: BTreeSet::new(),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::new(),
        conflicting_cells: BTreeSet::new(),
        recovered: vec![recovered.clone()],
        proposals: stable_context.view(),
    }));
    assert!(matches!(
        reorg.disposition(),
        KernelDisposition::ChainReconciled {
            recovered: ids,
            ..
        } if ids == &vec![recovered.id]
    ));
    assert_eq!(
        query_subject(&omega, resident.id),
        QuerySubject::Accepted(AcceptedStatus::Pending),
        "the same Apply installs the post-reorg view and sparse cache delta"
    );

    let resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    let evidence =
        ResolvedEvidence::for_transaction(&recovered, omega.authority.chain, omega.authority.rules)
            .expect("direct transaction has no dep-group expansion");
    assert_eq!(
        omega
            .kernel_step(KernelCommand::Complete(Completion {
                capability: resolve,
                result: WorkResult::Resolved(evidence),
            }))
            .disposition(),
        &KernelDisposition::Continued(recovered.id)
    );
    let verify = checked_out(omega.kernel_step(KernelCommand::Checkout));
    complete_verify(&mut omega, &recovered, verify);
    assert!(matches!(
        omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 11 }),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::Accepted(id),
            ..
        } if id == recovered.id
    ));
    assert_eq!(
        query_subject(&omega, recovered.id),
        QuerySubject::Accepted(AcceptedStatus::Pending)
    );

    let cohort = BTreeSet::from([resident.proposal, recovered.proposal]);
    let candidate_uncles = vec![
        CandidateUncleInput::new(1, cohort.clone(), 1),
        CandidateUncleInput::new(2, BTreeSet::from([ProposalId(9)]), 1),
    ];
    let premise = exact_model_template_premise(&omega, 2, 16, candidate_uncles);
    let service_window = stable_context.window();
    let mut liveness = TwoPhaseLiveness::from_context(stable_context, premise)
        .expect("the sealed source cut matches the captured proposal context");
    let proposed = liveness
        .step(CanonicalBlockService::CurrentOfferWithCompatibleUncles)
        .expect("the current mandatory offer plus compatible uncles is consensus safe");
    assert_eq!(proposed.offered_proposals(), &cohort);
    assert_eq!(proposed.proposals(), &cohort);
    assert_eq!(
        proposed.uncle_proposals(),
        &BTreeSet::from([ProposalId(9)]),
        "mandatory top-level proposals remove only their conflicting optional uncle"
    );
    let serviced = liveness
        .run_window_serviced(
            CanonicalServicePremise::for_window(2, service_window)
                .expect("the external proposal and window service premise is explicit"),
        )
        .expect("the post-reorg stable cohort has finite progress");
    assert!(
        serviced
            .iter()
            .all(|block| block.uncle_proposals().is_empty()),
        "finite progress depends only on mandatory offers and permits every optional uncle to be omitted"
    );
    assert_eq!(liveness.rank(), 0);
    assert!(cohort.into_iter().all(|id| liveness.is_committed(id)));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_two_phase_liveness_starts_only_from_a_causally_closed_accepted_cut() {
    let parent = Transaction::independent(1, 1, 10, 20);
    let child = Transaction::dependent(2, 2, 20, 30);
    let retained_proposal = Transaction::independent(3, 3, 12, 22);
    let mut omega = model();
    accept(&mut omega, &parent, 7, 10);
    let child_evidence = ResolvedEvidence::with_pool_input(
        &child,
        omega.authority.chain,
        omega.authority.rules,
        CellId(20),
        parent.id,
    )
    .expect("the child input is produced by the accepted parent");
    accept_with_evidence(&mut omega, &child, child_evidence, 11);
    assert_eq!(
        omega
            .kernel_step(retained(
                retained_proposal.clone(),
                RetainedSource::Proposal,
            ))
            .disposition(),
        &KernelDisposition::Retained(retained_proposal.id)
    );

    let cohort = StableAcceptedCohort::from_authority(&omega)
        .expect("the authority invariants seal a complete accepted cohort");
    assert_eq!(
        cohort.len(),
        2,
        "retained Proposal work is outside the theorem"
    );
    let premise = exact_model_template_premise(&omega, 2, 16, Vec::new());
    let window = ProposalWindow::new(2, 4).expect("the proposal window is valid");
    let mut liveness = TwoPhaseLiveness::new(window, premise)
        .expect("the captured Pending source cut matches the initial context");
    let observations = liveness
        .run_window_serviced(
            CanonicalServicePremise::for_window(1, window)
                .expect("each current offer is realized on the next canonical height"),
        )
        .expect("the accepted causal cohort has finite two-phase progress");
    let mut committed = BTreeSet::<ProposalId>::new();
    for block in observations {
        if block.committed().contains(&child.proposal) {
            assert!(
                block.committed().contains(&parent.proposal)
                    || committed.contains(&parent.proposal),
                "a child commit cannot outrun its accepted causal parent"
            );
        }
        assert!(
            !block
                .offered_proposals()
                .contains(&retained_proposal.proposal)
        );
        assert!(
            !block
                .offered_commits()
                .contains(&retained_proposal.proposal)
        );
        committed.extend(block.committed().iter().copied());
    }
    assert!(liveness.is_committed(parent.proposal));
    assert!(liveness.is_committed(child.proposal));
    assert!(!liveness.is_committed(retained_proposal.proposal));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_two_phase_offer_uses_accepted_package_order_not_insertion_order() {
    let earlier_weak = Transaction::independent(1, 1, 10, 20).with_fee(1);
    let later_strong = Transaction::independent(2, 2, 11, 21).with_fee(100);
    let mut omega = model();
    accept(&mut omega, &earlier_weak, 7, 10);
    accept(&mut omega, &later_strong, 8, 11);

    let premise = exact_model_template_premise(&omega, 1, 16, Vec::new());
    let mut liveness = TwoPhaseLiveness::new(
        ProposalWindow::new(2, 4).expect("the proposal window is valid"),
        premise,
    )
    .expect("the captured Pending source cut matches the initial context");
    let observation = liveness
        .step(CanonicalBlockService::CurrentOfferWithoutOptionalUncles)
        .expect("the strongest current offer is safe");
    assert_eq!(
        observation.offered_proposals(),
        &BTreeSet::from([later_strong.proposal]),
        "production package score dominates insertion order under a bounded prefix"
    );
}

#[test]
fn model_two_phase_liveness_rejects_a_conditional_template_cycle() {
    let mut left = Transaction::independent(1, 1, 10, 20);
    left.cell_deps.insert(CellId(11));
    let mut right = Transaction::independent(2, 2, 11, 21);
    right.cell_deps.insert(CellId(10));
    let mut omega = model();
    accept(&mut omega, &left, 7, 10);
    accept(&mut omega, &right, 8, 11);

    assert_eq!(omega.check_invariants(), Ok(()));
    assert_eq!(
        StableAcceptedCohort::from_authority(&omega),
        Err(StableAcceptedCohortError::ConditionalTemplateCycle),
        "the liveness theorem cannot silently ignore production's reader-before-spender cycle"
    );
}

#[test]
fn model_conditional_template_cycle_sheds_the_weakest_and_its_causal_descendants() {
    let inputs = vec![
        ConditionalSelectionInput::new(BTreeSet::new(), BTreeSet::from([1]), 0, 0),
        ConditionalSelectionInput::new(BTreeSet::new(), BTreeSet::from([0]), 1, 2),
        ConditionalSelectionInput::new(BTreeSet::from([0]), BTreeSet::new(), 2, 1),
    ];
    let observation = conditional_template_selection_refinement(&inputs)
        .expect("the finite conditional cycle has one deterministic production quotient");
    assert_eq!(observation.ordered, vec![1]);
    assert_eq!(observation.shed, BTreeSet::from([0, 2]));
}

#[test]
fn model_conditional_template_cycle_uses_descendant_aware_eviction_strength() {
    let inputs = vec![
        ConditionalSelectionInput::new(BTreeSet::new(), BTreeSet::from([1]), 0, 2),
        ConditionalSelectionInput::new(BTreeSet::new(), BTreeSet::from([0]), 1, 0),
        ConditionalSelectionInput::new(BTreeSet::from([0]), BTreeSet::new(), 2, 1),
    ];
    let observation = conditional_template_selection_refinement(&inputs)
        .expect("the package score supplies the cycle compiler's total order");
    assert_eq!(observation.ordered, vec![0, 2]);
    assert_eq!(observation.shed, BTreeSet::from([1]));
}

#[test]
fn model_conditional_template_cycle_has_a_bounded_dense_fallback() {
    let len = super::two_phase::CONDITIONAL_CYCLE_ROUND_BOUND + 2;
    let inputs = (0..len)
        .map(|index| {
            ConditionalSelectionInput::new(
                BTreeSet::new(),
                (0..len).filter(|other| *other != index).collect(),
                index,
                index,
            )
        })
        .collect::<Vec<_>>();
    let observation = conditional_template_selection_refinement(&inputs)
        .expect("the dense hostile SCC uses the frozen bounded fallback");
    assert_eq!(observation.ordered, vec![len - 1]);
    assert_eq!(observation.shed.len(), len - 1);
}

fn packing_identity(byte: u8) -> [u8; 32] {
    [byte; 32]
}

#[test]
fn model_template_packing_selects_an_exact_fit_cpfp_package_parent_first() {
    let inputs = vec![
        TemplatePackingInput::new(
            EvictionRefinementMetrics::new(1, 10, 10),
            BTreeSet::new(),
            true,
            0,
            packing_identity(1),
        ),
        TemplatePackingInput::new(
            EvictionRefinementMetrics::new(1_000_000_000, 10, 20),
            BTreeSet::from([0]),
            true,
            1,
            packing_identity(2),
        ),
        TemplatePackingInput::new(
            EvictionRefinementMetrics::new(1_000, 10, 30),
            BTreeSet::new(),
            true,
            2,
            packing_identity(3),
        ),
    ];
    let observation = template_packing_refinement(
        &inputs,
        ModelTemplatePackingLimits::new(20, 30),
        TEMPLATE_PACKING_FAILURE_BOUND,
    )
    .expect("the exact parent-child package fits both resource coordinates");
    assert_eq!(observation.selected, vec![0, 1]);
}

#[test]
fn model_template_packing_rescores_all_shared_parent_descendants() {
    let inputs = vec![
        TemplatePackingInput::new(
            EvictionRefinementMetrics::new(1, 1, 1),
            BTreeSet::new(),
            true,
            0,
            packing_identity(1),
        ),
        TemplatePackingInput::new(
            EvictionRefinementMetrics::new(1_000_000, 1, 1),
            BTreeSet::from([0]),
            true,
            1,
            packing_identity(2),
        ),
        TemplatePackingInput::new(
            EvictionRefinementMetrics::new(100_000, 1, 1),
            BTreeSet::from([0]),
            true,
            2,
            packing_identity(3),
        ),
        TemplatePackingInput::new(
            EvictionRefinementMetrics::new(60_000, 1, 1),
            BTreeSet::new(),
            true,
            3,
            packing_identity(4),
        ),
    ];
    let observation = template_packing_refinement(
        &inputs,
        ModelTemplatePackingLimits::new(u64::MAX, 3),
        TEMPLATE_PACKING_FAILURE_BOUND,
    )
    .expect("selected-parent subtraction changes every descendant package score once");
    assert_eq!(observation.selected, vec![0, 1, 2]);
}

#[test]
fn model_template_packing_failure_bound_is_a_deterministic_underfill() {
    let inputs = vec![
        TemplatePackingInput::new(
            EvictionRefinementMetrics::new(1_000_000_000, 1, 1),
            BTreeSet::new(),
            true,
            0,
            packing_identity(1),
        ),
        TemplatePackingInput::new(
            EvictionRefinementMetrics::new(100_000_000, 1, 2),
            BTreeSet::new(),
            true,
            1,
            packing_identity(2),
        ),
        TemplatePackingInput::new(
            EvictionRefinementMetrics::new(10_000_000, 1, 2),
            BTreeSet::new(),
            true,
            2,
            packing_identity(3),
        ),
        TemplatePackingInput::new(
            EvictionRefinementMetrics::new(1, 1, 1),
            BTreeSet::new(),
            true,
            3,
            packing_identity(4),
        ),
    ];
    let bounded =
        template_packing_refinement(&inputs, ModelTemplatePackingLimits::new(u64::MAX, 2), 1)
            .expect("the finite failure rank produces one deterministic prefix");
    let complete = template_packing_refinement(
        &inputs,
        ModelTemplatePackingLimits::new(u64::MAX, 2),
        TEMPLATE_PACKING_FAILURE_BOUND,
    )
    .expect("the production failure rank reaches later fitting work");
    assert_eq!(bounded.selected, vec![0]);
    assert_eq!(complete.selected, vec![0, 3]);
}

#[test]
fn model_complete_dependency_scan_preserves_causal_and_independent_work() {
    let inputs = vec![
        DependencyScanInput::new(BTreeSet::new(), 1),
        DependencyScanInput::new(BTreeSet::from([0]), 0),
        DependencyScanInput::new(BTreeSet::new(), 1),
    ];
    let observation = complete_dependency_scan_refinement(&inputs, &[0, 1, 2], 2)
        .expect("the exact captured edge domain bounds the complete scan");
    assert_eq!(observation.retained, vec![0, 1, 2]);
    assert_eq!(observation.inspected_dependency_edges, 2);
}

#[test]
fn model_complete_dependency_scan_is_order_invariant_for_independent_work() {
    let inputs = vec![
        DependencyScanInput::new(BTreeSet::new(), 1),
        DependencyScanInput::new(BTreeSet::new(), 1),
    ];
    let forward = complete_dependency_scan_refinement(&inputs, &[0, 1], 2)
        .expect("every selected independent edge is inside the captured bound");
    let reversed = complete_dependency_scan_refinement(&inputs, &[1, 0], 2)
        .expect("ordering cannot change membership of the complete scan");
    assert_eq!(forward.retained, vec![0, 1]);
    assert_eq!(reversed.retained, vec![1, 0]);
    assert_eq!(forward.inspected_dependency_edges, 2);
    assert_eq!(reversed.inspected_dependency_edges, 2);
    assert_eq!(
        complete_dependency_scan_refinement(&inputs, &[0, 1], 1),
        Err(super::two_phase::TemplatePackingError::InvalidGraph),
        "a false captured bound cannot silently truncate semantic work"
    );
}

#[test]
fn model_descendant_cache_bound_changes_only_the_cost_path() {
    let inputs = vec![
        TemplatePackingInput::new(
            EvictionRefinementMetrics::new(10, 1, 1),
            BTreeSet::new(),
            true,
            0,
            packing_identity(1),
        ),
        TemplatePackingInput::new(
            EvictionRefinementMetrics::new(20, 1, 1),
            BTreeSet::from([0]),
            true,
            1,
            packing_identity(2),
        ),
    ];
    let observation = template_packing_refinement(
        &inputs,
        ModelTemplatePackingLimits::new(u64::MAX, u64::MAX),
        TEMPLATE_PACKING_FAILURE_BOUND,
    )
    .expect("descendant caching is outside the semantic packing observation");
    assert_eq!(observation.selected, vec![0, 1]);
    assert!(TEMPLATE_DESCENDANT_CACHE_MEMBER_BOUND > inputs.len());
}

#[test]
fn model_persistence_contains_only_accepted_and_recovery_retained_owners() {
    let accepted = Transaction::independent(1, 1, 10, 20);
    let recovery = Transaction::independent(2, 2, 11, 21);
    let remote_tx = Transaction::independent(3, 3, 12, 22);
    let proposal = Transaction::independent(4, 4, 13, 23);
    let mut omega = model();
    accept(&mut omega, &accepted, 7, 10);
    omega.kernel_step(retained(
        recovery.clone(),
        RetainedSource::Recovery(PoolGeneration(0)),
    ));
    omega.kernel_step(remote(remote_tx, 8));
    omega.kernel_step(retained(proposal, RetainedSource::Proposal));
    assert_eq!(
        persistence_projection(&omega),
        vec![accepted.id, recovery.id]
    );
}

#[test]
fn model_verification_cache_identity_is_witness_plus_rules_not_raw_identity() {
    let first = VerificationKey::new(WitnessId(1), RulesId(1));
    let witness_variant = VerificationKey::new(WitnessId(2), RulesId(1));
    let rules_variant = VerificationKey::new(WitnessId(1), RulesId(2));
    assert_ne!(first, witness_variant);
    assert_ne!(first, rules_variant);
}

fn finite_script_semantics(key: VerificationKey) -> ModelScriptBehavior {
    if key.witness.0 == 0 {
        ModelScriptBehavior::invalid()
    } else {
        ModelScriptBehavior::valid(
            u64::from(key.witness.0)
                .saturating_mul(4)
                .saturating_add(u64::from(key.rules.0)),
        )
    }
}

fn fresh_verification(
    time_eligible: bool,
    capacity_valid: bool,
    dao_valid: bool,
) -> ModelFreshVerificationReceipts {
    ModelFreshVerificationReceipts::new(
        ModelFreshCapacityReceipt::new(capacity_valid),
        ModelFreshTimeReceipt::new(time_eligible),
        ModelFreshDaoReceipt::new(dao_valid),
    )
}

#[test]
fn model_script_proof_requires_vm_success_and_rechecks_the_current_max() {
    assert_eq!(
        ModelRemoteCycleLimit::checked(2, 1),
        Err(ModelRemoteCycleLimitError::ExceedsConsensusMax {
            declared: 2,
            consensus_max: 1,
        })
    );
    for (declared, consensus_max) in [(0, 0), (1, 1), (1, u64::MAX), (u64::MAX, u64::MAX)] {
        let limit = ModelRemoteCycleLimit::checked(declared, consensus_max)
            .expect("ingress accepts exactly declared <= consensus max");
        assert_eq!(limit.declared(), declared);
        assert_eq!(limit.consensus_max(), consensus_max);
    }

    let invalid = VerificationKey::new(WitnessId(0), RulesId(1));
    assert_eq!(
        execute_model_script_vm(invalid, u64::MAX, finite_script_semantics),
        ModelScriptExecution::Rejected(ModelScriptRejection::InvalidScript)
    );

    let key = VerificationKey::new(WitnessId(2), RulesId(1));
    let cycles = 9;
    assert_eq!(
        execute_model_script_vm(key, cycles - 1, finite_script_semantics),
        ModelScriptExecution::Rejected(ModelScriptRejection::ExceededMaximumCycles(cycles - 1))
    );
    let ModelScriptExecution::Verified(proof) =
        execute_model_script_vm(key, cycles, finite_script_semantics)
    else {
        panic!("a proof can originate only from successful VM execution")
    };
    assert_eq!(proof.key(), key);
    assert_eq!(proof.cycles(), cycles);
    let remote_limit = ModelRemoteCycleLimit::checked(cycles, cycles + 1)
        .expect("the successful VM work is inside the sealed remote envelope");
    assert!(remote_limit.bounds_vm_work(proof.cycles()));
    assert_eq!(
        reuse_model_script_proof(proof, key, cycles - 1),
        ModelScriptReuse::Rejected(ModelScriptRejection::ExceededMaximumCycles(cycles - 1))
    );
    assert_eq!(
        reuse_model_script_proof(proof, key, cycles),
        ModelScriptReuse::Verified(proof)
    );
}

#[test]
fn model_remote_cycle_limit_is_sealed_at_every_ingress_route() {
    for route in [
        ModelRemoteIngressRoute::NetworkRelay,
        ModelRemoteIngressRoute::DirectTxPool,
    ] {
        assert_eq!(
            remote_cycle_observation(route, 21, 20),
            ModelRemoteCycleObservation::Rejected(
                ModelRemoteCycleLimitError::ExceedsConsensusMax {
                    declared: 21,
                    consensus_max: 20,
                }
            ),
            "every route must reject d > M before ownership or VM work"
        );
        let ModelRemoteCycleObservation::Sealed(limit) = remote_cycle_observation(route, 9, 20)
        else {
            panic!("every route must seal legal d <= M")
        };
        let key = VerificationKey::new(WitnessId(2), RulesId(1));
        let observation = model_tx_pool_verification(
            key,
            finite_script_semantics,
            ModelTxPoolVerificationContext::remote(
                ModelTxPoolResolutionReceipt::current(true, 17, 31),
                limit,
                None,
            ),
            fresh_verification(true, true, true),
        );
        let ModelVerificationObservation::TxPoolVerified(receipt) = observation else {
            panic!("a legal exact declaration must reach the shared verification path")
        };
        assert_eq!(receipt.cycles(), limit.declared());
        assert!(limit.bounds_vm_work(receipt.cycles()));
    }
}

#[test]
fn model_script_cache_hot_and_cold_observations_are_equal_for_every_current_limit() {
    for witness in 0..=3 {
        for rules in 0..=2 {
            let key = VerificationKey::new(WitnessId(witness), RulesId(rules));
            let cached = match execute_model_script_vm(key, u64::MAX, finite_script_semantics) {
                ModelScriptExecution::Verified(proof) => Some(proof),
                ModelScriptExecution::Rejected(_) => None,
            };
            for current_max in 0..=20 {
                let remote_limit = ModelRemoteCycleLimit::checked(current_max, 20)
                    .expect("the finite current-limit domain is inside consensus max");
                let tx_pool_cold = model_tx_pool_verification(
                    key,
                    finite_script_semantics,
                    ModelTxPoolVerificationContext::trusted(
                        ModelTxPoolResolutionReceipt::current(true, 17, 31),
                        current_max,
                        None,
                    ),
                    fresh_verification(true, true, true),
                );
                let tx_pool_hot = model_tx_pool_verification(
                    key,
                    finite_script_semantics,
                    ModelTxPoolVerificationContext::trusted(
                        ModelTxPoolResolutionReceipt::current(true, 17, 31),
                        current_max,
                        cached,
                    ),
                    fresh_verification(true, true, true),
                );
                assert_eq!(
                    tx_pool_hot, tx_pool_cold,
                    "tx-pool key={key:?}, current_max={current_max}"
                );

                let remote_cold = model_tx_pool_verification(
                    key,
                    finite_script_semantics,
                    ModelTxPoolVerificationContext::remote(
                        ModelTxPoolResolutionReceipt::current(true, 17, 31),
                        remote_limit,
                        None,
                    ),
                    fresh_verification(true, true, true),
                );
                let remote_hot = model_tx_pool_verification(
                    key,
                    finite_script_semantics,
                    ModelTxPoolVerificationContext::remote(
                        ModelTxPoolResolutionReceipt::current(true, 17, 31),
                        remote_limit,
                        cached,
                    ),
                    fresh_verification(true, true, true),
                );
                assert_eq!(
                    remote_hot, remote_cold,
                    "remote key={key:?}, declared={current_max}"
                );
                if let ModelVerificationObservation::TxPoolVerified(receipt) = remote_hot {
                    assert_eq!(receipt.cycles(), remote_limit.declared());
                }
            }
        }
    }
}

#[test]
fn model_script_cache_key_excludes_max_but_rejects_witness_and_rules_substitution() {
    let cached_key = VerificationKey::new(WitnessId(2), RulesId(1));
    let ModelScriptExecution::Verified(proof) =
        execute_model_script_vm(cached_key, u64::MAX, finite_script_semantics)
    else {
        panic!("the cache fixture must originate in a successful VM run")
    };
    assert_eq!(proof.key(), cached_key);
    assert_eq!(
        reuse_model_script_proof(
            proof,
            VerificationKey::new(WitnessId(3), RulesId(1)),
            u64::MAX,
        ),
        ModelScriptReuse::Miss
    );
    assert_eq!(
        reuse_model_script_proof(
            proof,
            VerificationKey::new(WitnessId(2), RulesId(2)),
            u64::MAX,
        ),
        ModelScriptReuse::Miss
    );

    // One proof and key serve every limit. The limit changes only the O(1)
    // reuse decision; splitting the cache identity by limit is unnecessary.
    assert_eq!(
        reuse_model_script_proof(proof, cached_key, proof.cycles() - 1),
        ModelScriptReuse::Rejected(ModelScriptRejection::ExceededMaximumCycles(
            proof.cycles() - 1,
        ))
    );
    assert_eq!(
        reuse_model_script_proof(proof, cached_key, proof.cycles()),
        ModelScriptReuse::Verified(proof)
    );

    for requested in [
        VerificationKey::new(WitnessId(3), RulesId(1)),
        VerificationKey::new(WitnessId(2), RulesId(2)),
    ] {
        assert_eq!(
            model_tx_pool_verification(
                requested,
                finite_script_semantics,
                ModelTxPoolVerificationContext::trusted(
                    ModelTxPoolResolutionReceipt::current(true, 17, 31),
                    20,
                    Some(proof),
                ),
                fresh_verification(true, true, true),
            ),
            model_tx_pool_verification(
                requested,
                finite_script_semantics,
                ModelTxPoolVerificationContext::trusted(
                    ModelTxPoolResolutionReceipt::current(true, 17, 31),
                    20,
                    None,
                ),
                fresh_verification(true, true, true),
            ),
            "a mismatched cache proof must become an ordinary cold execution"
        );
    }
}

#[test]
fn model_tx_pool_non_script_receipts_and_hot_cold_order_are_exact() {
    let key = VerificationKey::new(WitnessId(2), RulesId(1));
    let ModelScriptExecution::Verified(proof) =
        execute_model_script_vm(key, u64::MAX, finite_script_semantics)
    else {
        panic!("the cache fixture must originate in a successful VM run")
    };
    let cycles = proof.cycles();
    let remote_limit = ModelRemoteCycleLimit::checked(cycles + 1, cycles + 1)
        .expect("the remote exact declaration is inside the consensus bound");

    let tx_pool_cases = [
        (
            ModelTxPoolVerificationContext::trusted(
                ModelTxPoolResolutionReceipt::current(false, 11, 31),
                cycles - 1,
                None,
            ),
            ModelTxPoolVerificationContext::trusted(
                ModelTxPoolResolutionReceipt::current(false, 11, 31),
                cycles - 1,
                Some(proof),
            ),
            fresh_verification(false, false, false),
            ModelVerificationObservation::Rejected(ModelVerificationRejection::Fee),
        ),
        (
            ModelTxPoolVerificationContext::trusted(
                ModelTxPoolResolutionReceipt::current(true, 11, 31),
                cycles - 1,
                None,
            ),
            ModelTxPoolVerificationContext::trusted(
                ModelTxPoolResolutionReceipt::current(true, 11, 31),
                cycles - 1,
                Some(proof),
            ),
            fresh_verification(false, false, false),
            ModelVerificationObservation::Rejected(ModelVerificationRejection::Time),
        ),
        (
            ModelTxPoolVerificationContext::trusted(
                ModelTxPoolResolutionReceipt::current(true, 11, 31),
                cycles - 1,
                None,
            ),
            ModelTxPoolVerificationContext::trusted(
                ModelTxPoolResolutionReceipt::current(true, 11, 31),
                cycles - 1,
                Some(proof),
            ),
            fresh_verification(true, false, false),
            ModelVerificationObservation::Rejected(ModelVerificationRejection::Capacity),
        ),
        (
            ModelTxPoolVerificationContext::trusted(
                ModelTxPoolResolutionReceipt::current(true, 11, 31),
                cycles - 1,
                None,
            ),
            ModelTxPoolVerificationContext::trusted(
                ModelTxPoolResolutionReceipt::current(true, 11, 31),
                cycles - 1,
                Some(proof),
            ),
            fresh_verification(true, true, false),
            ModelVerificationObservation::Rejected(ModelVerificationRejection::Script(
                ModelScriptRejection::ExceededMaximumCycles(cycles - 1),
            )),
        ),
        (
            ModelTxPoolVerificationContext::remote(
                ModelTxPoolResolutionReceipt::current(true, 11, 31),
                remote_limit,
                None,
            ),
            ModelTxPoolVerificationContext::remote(
                ModelTxPoolResolutionReceipt::current(true, 11, 31),
                remote_limit,
                Some(proof),
            ),
            fresh_verification(true, true, false),
            ModelVerificationObservation::Rejected(ModelVerificationRejection::Dao),
        ),
        (
            ModelTxPoolVerificationContext::remote(
                ModelTxPoolResolutionReceipt::current(true, 11, 31),
                remote_limit,
                None,
            ),
            ModelTxPoolVerificationContext::remote(
                ModelTxPoolResolutionReceipt::current(true, 11, 31),
                remote_limit,
                Some(proof),
            ),
            fresh_verification(true, true, true),
            ModelVerificationObservation::Rejected(
                ModelVerificationRejection::DeclaredWrongCycles {
                    declared: cycles + 1,
                    actual: cycles,
                },
            ),
        ),
    ];
    for (cold_context, hot_context, fresh, expected) in tx_pool_cases {
        let cold = model_tx_pool_verification(key, finite_script_semantics, cold_context, fresh);
        let hot = model_tx_pool_verification(key, finite_script_semantics, hot_context, fresh);
        assert_eq!(cold, expected);
        assert_eq!(hot, expected);
    }

    for cached in [None, Some(proof)] {
        let observation = model_tx_pool_verification(
            key,
            finite_script_semantics,
            ModelTxPoolVerificationContext::trusted(
                ModelTxPoolResolutionReceipt::current(true, 37, 31),
                cycles,
                cached,
            ),
            fresh_verification(true, true, true),
        );
        let ModelVerificationObservation::TxPoolVerified(receipt) = observation else {
            panic!("the valid tx-pool fixture must verify")
        };
        assert_eq!(receipt.script_proof(), proof);
        assert_eq!(receipt.cycles(), cycles);
        assert_eq!(receipt.fee(), 37);
        assert_eq!(receipt.accepted_resident_bytes(), 31);
    }
}

#[test]
fn model_compute_grant_uses_total_retained_bytes_not_payload_only_evidence() {
    let grant = ComputeGrant {
        max_total_retained: TotalRetainedBytes(4_096),
    };
    let inputs = RetainedChargeInputs {
        payload: PayloadBytes(4_096),
        resolved: ResolvedResidentBytes(4_096),
        entry_metadata: EntryMetadataBytes(128),
        edge_metadata: EdgeMetadataBytes(64),
    };
    assert_eq!(grant.admit(inputs), ComputeAdmission::ResourceExcluded);
    assert_eq!(
        ComputeGrant {
            max_total_retained: TotalRetainedBytes(4_288),
        }
        .admit(inputs),
        ComputeAdmission::Granted(TotalRetainedBytes(4_288))
    );
    assert_eq!(
        RetainedChargeInputs {
            payload: PayloadBytes(u32::MAX),
            resolved: ResolvedResidentBytes(u32::MAX),
            entry_metadata: EntryMetadataBytes(1),
            edge_metadata: EdgeMetadataBytes(0),
        }
        .compile(),
        None
    );
}

#[test]
fn model_allocation_pressure_is_an_ordinary_terminal_outcome_not_a_timer_retry() {
    assert_eq!(
        prepare_bounded_scratch(4, 4, false),
        ScratchDisposition::OrdinaryUnavailable
    );
    assert_eq!(
        prepare_bounded_scratch(5, 4, true),
        ScratchDisposition::OrdinaryUnavailable
    );
    assert_eq!(
        prepare_bounded_scratch(4, 4, true),
        ScratchDisposition::Prepared
    );
    assert_eq!(
        terminal_allocation_disposition(AllocationObligation::CallerOwnedRequest),
        AllocationTerminal::ReleaseCaller
    );
    assert_eq!(
        terminal_allocation_disposition(AllocationObligation::CurrentGeneration),
        AllocationTerminal::ReplaceCurrentGeneration
    );
    assert_eq!(
        terminal_allocation_disposition(AllocationObligation::CheckedOutCapability),
        AllocationTerminal::CancelAndReplaceCurrentGeneration
    );
    assert_eq!(
        terminal_allocation_disposition(AllocationObligation::OrderedChainTransition),
        AllocationTerminal::ReplaceChainGeneration
    );
    for terminal in [
        AllocationTerminal::ReleaseCaller,
        AllocationTerminal::ReplaceCurrentGeneration,
        AllocationTerminal::CancelAndReplaceCurrentGeneration,
        AllocationTerminal::ReplaceChainGeneration,
    ] {
        let conservation = terminal.conservation();
        assert!(
            conservation.conserves_capabilities(),
            "{terminal:?} must consume, retain or stale every linear capability exactly once"
        );
        assert_eq!(
            conservation.caller_released(),
            terminal == AllocationTerminal::ReleaseCaller
        );
        assert_eq!(
            conservation.generation_advanced(),
            terminal != AllocationTerminal::ReleaseCaller
        );
        assert_eq!(
            conservation.checked_out_work_consumed(),
            terminal == AllocationTerminal::CancelAndReplaceCurrentGeneration
        );
        assert_eq!(
            conservation.ordered_chain_command_consumed(),
            terminal == AllocationTerminal::ReplaceChainGeneration
        );
        assert_eq!(
            conservation.ordered_snapshot_installed(),
            terminal == AllocationTerminal::ReplaceChainGeneration
        );
    }
}

#[test]
fn model_retained_ingress_outcomes_cannot_construct_integrity_evidence() {
    for outcome in [
        ModelRetainedIngressOutcome::Validated,
        ModelRetainedIngressOutcome::Rejected,
        ModelRetainedIngressOutcome::ProposalUnavailable,
    ] {
        assert_eq!(outcome.service_failure(), None);
    }
}

#[test]
fn model_service_fault_boundary_is_the_total_sealed_failure_relation() {
    for failure in [
        ModelOperationalFailure::Cancelled,
        ModelOperationalFailure::BlockAssemblerDisabled,
        ModelOperationalFailure::TemplateUnavailable,
        ModelOperationalFailure::ResourceUnavailable,
        ModelOperationalFailure::EffectCapacity,
        ModelOperationalFailure::LifecycleClosed,
    ] {
        assert_eq!(
            service_failure_disposition(ModelServiceFailure::Operational(failure)),
            ModelFailureDisposition::Ordinary(failure)
        );
    }

    for fault in [
        ModelStructuralFault::InvalidChainEvidence,
        ModelStructuralFault::CounterExhausted,
        ModelStructuralFault::EffectLifecycleClosed,
        ModelStructuralFault::ResourceProjection,
        ModelStructuralFault::MembershipProjection,
        ModelStructuralFault::IndexProjection,
        ModelStructuralFault::SchedulerProjection,
        ModelStructuralFault::DependencyProjection,
        ModelStructuralFault::EffectProjection,
    ] {
        assert_eq!(
            service_failure_disposition(chain_structural_failure(fault)),
            ModelFailureDisposition::Integrity(fault)
        );
        assert_eq!(
            authority_structural_failure(fault).map(service_failure_disposition),
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
                    Some(ModelFailureDisposition::Integrity(fault))
                }
            }
        );
    }

    assert_eq!(
        recovery_admission_disposition(ModelRecoveryAdmissionFailure::InvalidTransaction),
        ModelFailureDisposition::Integrity(ModelStructuralFault::InvalidChainEvidence)
    );
    assert_eq!(
        recovery_admission_disposition(ModelRecoveryAdmissionFailure::ResourceUnavailable),
        ModelFailureDisposition::Ordinary(ModelOperationalFailure::ResourceUnavailable)
    );
}

#[test]
fn model_shared_reserved_vec_is_the_unique_fallible_variable_residency_normal_form() {
    let candidates = [
        SharedVariableResidencyNormalForm::DeepOwnedClone,
        SharedVariableResidencyNormalForm::ReboxedArcSlice,
        SharedVariableResidencyNormalForm::SharedReservedVec,
    ];
    let feasible = candidates
        .into_iter()
        .filter(|candidate| candidate.feasible())
        .collect::<Vec<_>>();
    assert_eq!(
        feasible,
        [SharedVariableResidencyNormalForm::SharedReservedVec]
    );
    assert_eq!(
        SharedVariableResidencyNormalForm::SharedReservedVec.static_cost(),
        SharedVariableResidencyCost {
            variable_allocations: 1,
            variable_copies: 0,
            fixed_wrapper_objects: 1,
        }
    );
    assert!(
        SharedVariableResidencyNormalForm::SharedReservedVec.static_cost()
            < SharedVariableResidencyNormalForm::ReboxedArcSlice.static_cost()
    );
}

#[test]
fn model_template_failure_retries_iff_the_minimum_source_cut_changed() {
    for failed in u8::MIN..=u8::MAX {
        for observed in u8::MIN..=u8::MAX {
            let failed = TemplateFailureCut(failed);
            let observed = TemplateFailureCut(observed);
            let progress = template_failure_progress(failed, observed);
            assert_eq!(
                matches!(progress, TemplateFailureProgress::RetryAfterChange(cut) if cut == observed),
                failed != observed
            );
            assert_eq!(
                matches!(progress, TemplateFailureProgress::Parked(cut) if cut == observed),
                failed == observed
            );
        }
    }
}

#[test]
fn model_query_cost_keeps_concurrency_scan_sort_and_output_terms_explicit() {
    assert_eq!(
        QueryCostInputs {
            concurrent_queries: 8,
            owner_rows: 100,
            accepted_order_rows: 80,
            output_items: 100,
            output_item_bytes: 32,
        }
        .compile(),
        Some(QueryCostUpperBound {
            authority_row_visits: 1_440,
            sort_comparisons: 4_480,
            output_resident_bytes: 25_600,
        })
    );
    assert_eq!(
        QueryCostInputs {
            concurrent_queries: u32::MAX,
            owner_rows: u32::MAX,
            accepted_order_rows: u32::MAX,
            output_items: u32::MAX,
            output_item_bytes: u32::MAX,
        }
        .compile(),
        None,
        "an unrepresentable bound is an explicit exclusion, never a wrapped cost"
    );
}

#[test]
fn model_service_ingress_residency_bounds_typed_ordered_senders() {
    use super::resource::{ServiceIngressResidencyBound, ServiceIngressResidencyInputs};

    let handler_multiplier = u32::try_from(crate::constants::MESSAGE_CONCURRENCY_MULTIPLIER)
        .expect("production handler multiplier fits the model domain");
    let ordinary_queue = u32::try_from(crate::service::DEFAULT_CHANNEL_SIZE)
        .expect("production ordinary channel fits the model domain");
    let ordered_queue = u32::try_from(crate::service::CHAIN_CONTROL_CHANNEL_SIZE)
        .expect("production ordered channel fits the model domain");
    let verify_workers = 8u32;
    let ordinary_payload_bytes_per_request = 1_048_576u64;
    let ordered_payload_bytes_per_request = 8_388_608u64;
    let ordinary_handlers = verify_workers
        .checked_mul(handler_multiplier)
        .expect("small production-shaped fixture is representable");
    assert_eq!(
        ServiceIngressResidencyInputs {
            verify_workers,
            handler_multiplier,
            ordinary_queue,
            ordered_queue,
            trusted_reorg_waiting_senders: 0,
            admitted_admin_waiting_senders: 0,
            ordinary_payload_bytes_per_request,
            ordered_payload_bytes_per_request,
        }
        .compile(),
        Some(ServiceIngressResidencyBound {
            ordinary_handlers,
            ordinary_owned_requests: ordinary_queue + ordinary_handlers,
            ordered_owned_requests: ordered_queue + 1,
            ordinary_payload_bytes: u64::from(ordinary_queue + ordinary_handlers)
                * ordinary_payload_bytes_per_request,
            ordered_payload_bytes: u64::from(ordered_queue + 1) * ordered_payload_bytes_per_request,
            total_payload_bytes: u64::from(ordinary_queue + ordinary_handlers)
                * ordinary_payload_bytes_per_request
                + u64::from(ordered_queue + 1) * ordered_payload_bytes_per_request,
        })
    );
    assert_eq!(
        ServiceIngressResidencyInputs {
            verify_workers: 0,
            handler_multiplier,
            ordinary_queue: 0,
            ordered_queue: 0,
            trusted_reorg_waiting_senders: 1,
            admitted_admin_waiting_senders: 1,
            ordinary_payload_bytes_per_request,
            ordered_payload_bytes_per_request,
        }
        .compile(),
        Some(ServiceIngressResidencyBound {
            ordinary_handlers: handler_multiplier,
            ordinary_owned_requests: handler_multiplier,
            ordered_owned_requests: 3,
            ordinary_payload_bytes: u64::from(handler_multiplier)
                * ordinary_payload_bytes_per_request,
            ordered_payload_bytes: 3 * ordered_payload_bytes_per_request,
            total_payload_bytes: u64::from(handler_multiplier) * ordinary_payload_bytes_per_request
                + 3 * ordered_payload_bytes_per_request,
        }),
        "the trusted reorg and unique public admission are the complete waiting term"
    );
    for (trusted_reorg_waiting_senders, admitted_admin_waiting_senders) in [(2, 0), (0, 2)] {
        assert_eq!(
            ServiceIngressResidencyInputs {
                verify_workers: 0,
                handler_multiplier,
                ordinary_queue: 0,
                ordered_queue: 0,
                trusted_reorg_waiting_senders,
                admitted_admin_waiting_senders,
                ordinary_payload_bytes_per_request,
                ordered_payload_bytes_per_request,
            }
            .compile(),
            None,
            "the typed producer boundary rejects an unbounded waiting population"
        );
    }
    assert_eq!(
        ServiceIngressResidencyInputs {
            verify_workers: 0,
            handler_multiplier,
            ordinary_queue: 0,
            ordered_queue: u32::MAX,
            trusted_reorg_waiting_senders: 1,
            admitted_admin_waiting_senders: 1,
            ordinary_payload_bytes_per_request,
            ordered_payload_bytes_per_request,
        }
        .compile(),
        None,
        "the complete ordered residency sum is checked"
    );
    assert_eq!(
        ServiceIngressResidencyInputs {
            verify_workers: u32::MAX,
            handler_multiplier: 2,
            ordinary_queue: 0,
            ordered_queue: 0,
            trusted_reorg_waiting_senders: 0,
            admitted_admin_waiting_senders: 0,
            ordinary_payload_bytes_per_request,
            ordered_payload_bytes_per_request,
        }
        .compile(),
        None
    );
    assert_eq!(
        ServiceIngressResidencyInputs {
            verify_workers: 1,
            handler_multiplier: 1,
            ordinary_queue: 0,
            ordered_queue: 0,
            trusted_reorg_waiting_senders: 0,
            admitted_admin_waiting_senders: 0,
            ordinary_payload_bytes_per_request: u64::MAX,
            ordered_payload_bytes_per_request: 0,
        }
        .compile(),
        Some(ServiceIngressResidencyBound {
            ordinary_handlers: 1,
            ordinary_owned_requests: 1,
            ordered_owned_requests: 1,
            ordinary_payload_bytes: u64::MAX,
            ordered_payload_bytes: 0,
            total_payload_bytes: u64::MAX,
        })
    );
    assert_eq!(
        ServiceIngressResidencyInputs {
            verify_workers: 1,
            handler_multiplier: 1,
            ordinary_queue: 1,
            ordered_queue: 0,
            trusted_reorg_waiting_senders: 0,
            admitted_admin_waiting_senders: 0,
            ordinary_payload_bytes_per_request: u64::MAX,
            ordered_payload_bytes_per_request: 0,
        }
        .compile(),
        None,
        "the byte equation is checked independently of the request-count equation"
    );
}

#[test]
fn model_compute_completion_returns_to_the_fair_arbiter_before_new_work_can_reuse_it() {
    let mut scheduler =
        FairPermitScheduler::new(PermitDomain(1), 1, 4).expect("valid permit fixture");
    let retained = PermitRequest {
        id: PermitRequestId(1),
        class: PermitClass::Retained,
    };
    let local = PermitRequest {
        id: PermitRequestId(2),
        class: PermitClass::Direct,
    };
    let older_retained_waiter = PermitRequest {
        id: PermitRequestId(3),
        class: PermitClass::Retained,
    };
    let retrying_completion = PermitRequest {
        id: PermitRequestId(4),
        class: PermitClass::Retained,
    };

    let retained_token = match scheduler.request(retained) {
        PermitRequestDisposition::Granted {
            grant: PermitGrant::Retained(token),
        } => token,
        other => panic!("expected the initial permit, got {other:?}"),
    };
    assert_eq!(
        scheduler.request(local),
        PermitRequestDisposition::Queued(local.id)
    );
    assert_eq!(
        scheduler.request(older_retained_waiter),
        PermitRequestDisposition::Queued(older_retained_waiter.id)
    );
    assert_eq!(scheduler.waiting_position(local.id), Some(0));

    let local_token = match scheduler.release(retained_token.into()) {
        PermitReleaseDisposition::Released {
            request,
            next: Some(PermitGrant::Direct(token)),
        } if request == retained && token.request() == local => token,
        other => panic!("expected the queued Local request to receive the permit, got {other:?}"),
    };
    assert_eq!(
        scheduler.request(retrying_completion),
        PermitRequestDisposition::Queued(retrying_completion.id)
    );
    assert_eq!(
        scheduler.waiting_position(older_retained_waiter.id),
        Some(0)
    );
    assert_eq!(scheduler.waiting_position(retrying_completion.id), Some(1));

    assert!(matches!(
        scheduler.release(local_token.into()),
        PermitReleaseDisposition::Released {
            next: Some(PermitGrant::Retained(token)),
            ..
        } if token.request() == older_retained_waiter
    ));
    assert_eq!(scheduler.check_invariants(), Ok(()));
}

#[test]
fn model_optional_query_arithmetic_failure_never_invalidates_the_authority_result() {
    assert_eq!(
        query_projection(
            QuerySubject::Accepted(AcceptedStatus::Pending),
            u64::MAX,
            Some(ModelFeeRate::from_u64(u64::MAX)),
            2,
        ),
        QueryProjection {
            status: QueryStatus::Pending,
            minimum_replacement_fee: None,
        }
    );
    assert_eq!(
        query_projection(
            QuerySubject::Accepted(AcceptedStatus::Gap),
            1,
            Some(ModelFeeRate::from_u64(2_000)),
            3,
        ),
        QueryProjection {
            status: QueryStatus::Pending,
            minimum_replacement_fee: Some(7),
        }
    );
    assert_eq!(
        query_projection(
            QuerySubject::PreAcceptedProposalAware(AcceptedStatus::Proposed),
            1,
            Some(ModelFeeRate::from_u64(2_000)),
            3,
        ),
        QueryProjection {
            status: QueryStatus::Proposed,
            minimum_replacement_fee: None,
        }
    );
    assert_eq!(
        query_projection(QuerySubject::Hidden, 0, None, 0).status,
        QueryStatus::Unknown
    );
}

#[test]
fn model_query_projection_collapses_only_the_documented_internal_states() {
    let transaction = Transaction::independent(1, 1, 10, 20);
    let mut omega = model();
    assert_eq!(query_subject(&omega, transaction.id), QuerySubject::Hidden);

    omega.kernel_step(remote(transaction.clone(), 7));
    assert_eq!(
        query_subject(&omega, transaction.id),
        QuerySubject::PreAcceptedPending
    );
    let resolve = checked_out(omega.kernel_step(KernelCommand::Checkout));
    assert_eq!(
        query_subject(&omega, transaction.id),
        QuerySubject::PreAcceptedPending
    );
    omega.kernel_step(KernelCommand::Complete(Completion {
        capability: resolve,
        result: WorkResult::Resolved(
            ResolvedEvidence::for_transaction(
                &transaction,
                omega.authority.chain,
                omega.authority.rules,
            )
            .expect("direct transaction has no dep-group expansion"),
        ),
    }));
    assert_eq!(
        query_subject(&omega, transaction.id),
        QuerySubject::PreAcceptedProposalAware(AcceptedStatus::Pending)
    );
    let verify = checked_out(omega.kernel_step(KernelCommand::Checkout));
    assert_eq!(
        query_subject(&omega, transaction.id),
        QuerySubject::PreAcceptedProposalAware(AcceptedStatus::Pending)
    );
    complete_verify(&mut omega, &transaction, verify);
    assert_eq!(
        query_subject(&omega, transaction.id),
        QuerySubject::PreAcceptedProposalAware(AcceptedStatus::Pending)
    );

    omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 10 });
    assert_eq!(
        query_subject(&omega, transaction.id),
        QuerySubject::Accepted(AcceptedStatus::Pending)
    );
    omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(
            super::state::RulesId(1),
            false,
        ),
        from: omega.authority.chain,
        to_tip: ViewId(2),
        committed: BTreeSet::new(),
        available_cells: BTreeSet::new(),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::new(),
        conflicting_cells: BTreeSet::new(),
        recovered: Vec::new(),
        proposals: super::proposal::ProposalContext::status_witness(
            BTreeSet::new(),
            BTreeSet::from([transaction.proposal]),
        )
        .expect("the proposal witness sets are disjoint")
        .view(),
    }));
    assert_eq!(
        query_projection(
            query_subject(&omega, transaction.id),
            1,
            Some(ModelFeeRate::from_u64(2_000)),
            3,
        )
        .status,
        QueryStatus::Pending
    );
    omega.kernel_step(KernelCommand::ReconcileChain(ChainTransition {
        context: super::kernel::ChainContextTransition::from_primitives(
            super::state::RulesId(1),
            false,
        ),
        from: omega.authority.chain,
        to_tip: ViewId(3),
        committed: BTreeSet::new(),
        available_cells: BTreeSet::new(),
        available_headers: BTreeSet::new(),
        lost_cells: BTreeSet::new(),
        lost_headers: BTreeSet::new(),
        conflicting_cells: BTreeSet::new(),
        recovered: Vec::new(),
        proposals: super::proposal::ProposalContext::status_witness(
            BTreeSet::from([transaction.proposal]),
            BTreeSet::new(),
        )
        .expect("the proposal witness sets are disjoint")
        .view(),
    }));
    assert_eq!(
        query_projection(
            query_subject(&omega, transaction.id),
            1,
            Some(ModelFeeRate::from_u64(2_000)),
            3,
        ),
        QueryProjection {
            status: QueryStatus::Proposed,
            minimum_replacement_fee: None,
        }
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_total_system_step_preserves_invariants_for_bounded_traces() {
    let initial = SystemState::constructing(ProtocolLimits::small());
    explore(initial, 0, 4, &mut Vec::new());
}

#[test]
fn model_kernel_step_is_total_and_preserves_invariants_for_bounded_traces() {
    let initial = model();
    let mut frontier = VecDeque::from([(initial.clone(), Vec::new())]);
    let mut seen = BTreeSet::from([format!("{initial:?}")]);

    while let Some((state, trace)) = frontier.pop_front() {
        if trace.len() == 5 {
            continue;
        }
        for command in model_kernel_commands(&state) {
            let mut next = state.clone();
            let before_authority = next.authority.clone();
            let step = next.kernel_step(command.clone());
            let mut next_trace = trace.clone();
            next_trace.push(command);

            if let Err(error) = next.check_invariants() {
                panic!("kernel trace violated {error:?}: {next_trace:#?}");
            }
            match step {
                KernelStep::NoAuthorityCommit(_) => {
                    assert_eq!(
                        next.authority, before_authority,
                        "NoAuthorityCommit changed authority: {next_trace:#?}"
                    );
                }
                KernelStep::AuthorityCommit { stamp, .. } => {
                    assert_eq!(
                        stamp.0,
                        before_authority.last_apply.0 + 1,
                        "one Apply must advance exactly once: {next_trace:#?}"
                    );
                    assert_eq!(next.authority.last_apply, stamp);
                }
            }
            let key = format!("{next:?}");
            if seen.insert(key) {
                frontier.push_back((next, next_trace));
            }
        }
    }
}

fn explore(state: SystemState, depth: usize, maximum: usize, trace: &mut Vec<SystemEvent>) {
    if let Err(error) = state.check_invariants() {
        panic!("model trace violated {error:?}: {trace:#?}");
    }
    if depth == maximum {
        return;
    }
    for event in model_events(&state) {
        let mut next = state.clone();
        next.step(event.clone());
        trace.push(event);
        explore(next, depth + 1, maximum, trace);
        trace.pop();
    }
}

fn model_events(state: &SystemState) -> Vec<SystemEvent> {
    let chain = state
        .authority
        .as_ref()
        .map_or(ChainView::initial(ViewId(1)), |authority| {
            authority.authority.chain
        });
    let mut events = vec![
        SystemEvent::Enqueue {
            request: RequestId(0),
            kind: RequestKind::Notification,
            cost: PayloadCost::small(),
        },
        SystemEvent::Enqueue {
            request: RequestId(1),
            kind: RequestKind::Ordinary { response: true },
            cost: PayloadCost::small(),
        },
        SystemEvent::Enqueue {
            request: RequestId(2),
            kind: RequestKind::OrderedChain { response: true },
            cost: PayloadCost::small(),
        },
        SystemEvent::Dispatch(RequestId(0)),
        SystemEvent::Dispatch(RequestId(1)),
        SystemEvent::Dispatch(RequestId(2)),
        SystemEvent::Finish {
            request: RequestId(0),
            send_response: false,
        },
        SystemEvent::Finish {
            request: RequestId(1),
            send_response: true,
        },
        SystemEvent::AbandonReceiver(RequestId(1)),
        SystemEvent::Ready,
        SystemEvent::InitializationReplayFailed,
        SystemEvent::BeginDrain,
        SystemEvent::FinishDrain,
        SystemEvent::PublishTemplate { captured_source: 0 },
        SystemEvent::DegradeRecentReject,
    ];
    if state.lifecycle == Lifecycle::Constructing {
        events.push(SystemEvent::Assemble {
            limits: ModelLimits::small(),
            view: ViewId(1),
            rules: RulesId(1),
            succeed: true,
        });
        events.push(SystemEvent::Assemble {
            limits: ModelLimits::small(),
            view: ViewId(1),
            rules: RulesId(1),
            succeed: false,
        });
    }
    let access = match state.lifecycle {
        Lifecycle::Initializing => KernelAccess::Initialization,
        Lifecycle::Running => KernelAccess::Ordinary,
        Lifecycle::Draining => KernelAccess::Drain,
        _ => KernelAccess::Ordinary,
    };
    events.extend([
        SystemEvent::Kernel {
            access,
            command: remote(Transaction::independent(1, 1, 10, 20), 7),
        },
        SystemEvent::Kernel {
            access,
            command: retained(
                Transaction::independent(2, 2, 11, 21),
                RetainedSource::Proposal,
            ),
        },
        SystemEvent::Kernel {
            access,
            command: KernelCommand::Checkout,
        },
        SystemEvent::Kernel {
            access,
            command: KernelCommand::CheckoutContinuous(VerifyCapability::Any),
        },
        SystemEvent::Kernel {
            access,
            command: KernelCommand::BeginDirect {
                request: DirectRequestId(1),
                kind: DirectKind::TestAccept,
                transaction: Transaction::independent(3, 3, 12, 22),
            },
        },
        SystemEvent::Kernel {
            access,
            command: KernelCommand::CaptureReady { limit: 2 },
        },
        SystemEvent::Kernel {
            access,
            command: KernelCommand::FinalizeNext { wall_time: 10 },
        },
        SystemEvent::Kernel {
            access,
            command: KernelCommand::Remove {
                transaction: TxId(1),
            },
        },
        SystemEvent::Kernel {
            access,
            command: KernelCommand::BanPeer {
                peer: PeerId(7),
                observed_at: MonotonicTick(1),
            },
        },
        SystemEvent::Kernel {
            access,
            command: KernelCommand::ExpireRemote {
                wall_time: 10,
                limit: NonZeroU16::new(1).expect("one is non-zero"),
            },
        },
        SystemEvent::Kernel {
            access,
            command: KernelCommand::ReconcileChain(ChainTransition {
                context: super::kernel::ChainContextTransition::from_primitives(
                    super::state::RulesId(1),
                    true,
                ),
                from: chain,
                to_tip: ViewId(2),
                committed: BTreeSet::new(),
                available_cells: BTreeSet::new(),
                available_headers: BTreeSet::new(),
                lost_cells: BTreeSet::new(),
                lost_headers: BTreeSet::new(),
                conflicting_cells: BTreeSet::new(),
                recovered: vec![Transaction::independent(4, 4, 13, 23)],
                proposals: super::proposal::ProposalContext::empty().view(),
            }),
        },
        SystemEvent::Kernel {
            access,
            command: KernelCommand::ReplaceGeneration { view: ViewId(3) },
        },
        SystemEvent::Kernel {
            access,
            command: reconcile_view(chain, ViewId(2)),
        },
        SystemEvent::Kernel {
            access,
            command: KernelCommand::ClaimEffect,
        },
    ]);
    if let Some(authority) = &state.authority {
        if let Some(capability) = authority.linear.work.values().next() {
            events.push(SystemEvent::Kernel {
                access,
                command: KernelCommand::Complete(Completion {
                    capability: capability.id,
                    result: capability.stage_rejection_result(),
                }),
            });
            events.push(SystemEvent::Kernel {
                access,
                command: KernelCommand::FinishExecution(Completion {
                    capability: capability.id,
                    result: capability.stage_rejection_result(),
                }),
            });
            events.push(SystemEvent::Kernel {
                access,
                command: KernelCommand::CancelCapability(capability.id),
            });
            if let Some(owner) = authority.authority.owners.get(&capability.transaction) {
                if matches!(capability.permit(), WorkPermit::ResolveThenVerify(_))
                    && matches!(capability.stage(), WorkStage::Resolve)
                {
                    events.push(SystemEvent::Kernel {
                        access,
                        command: KernelCommand::ContinueResolveThenVerify(ResolveContinuation {
                            capability: capability.id,
                            evidence: ResolvedEvidence::for_transaction(
                                &owner.transaction,
                                authority.authority.chain,
                                authority.authority.rules,
                            )
                            .expect("direct transaction has no dep-group expansion"),
                        }),
                    });
                }
                events.push(SystemEvent::Kernel {
                    access,
                    command: KernelCommand::Complete(Completion {
                        capability: capability.id,
                        result: if capability.kind() == super::state::WorkKind::Resolve {
                            WorkResult::Resolved(
                                ResolvedEvidence::for_transaction(
                                    &owner.transaction,
                                    authority.authority.chain,
                                    authority.authority.rules,
                                )
                                .expect("direct transaction has no dep-group expansion"),
                            )
                        } else {
                            WorkResult::Verified
                        },
                    }),
                });
            }
        }
        if let Some(finished) = authority.linear.finished_work.values().next() {
            events.push(SystemEvent::Kernel {
                access,
                command: KernelCommand::SettleFinished(finished.capability.id),
            });
            events.push(SystemEvent::Kernel {
                access,
                command: KernelCommand::CancelCapability(finished.capability.id),
            });
        }
        if let Some(capability) = authority.linear.direct_work.values().next() {
            events.push(SystemEvent::Kernel {
                access,
                command: KernelCommand::CompleteDirect(DirectCompletion {
                    capability: capability.id,
                    wall_time: 10,
                    result: DirectWorkResult::Verified(
                        ResolvedEvidence::for_transaction(
                            &capability.transaction,
                            authority.authority.chain,
                            authority.authority.rules,
                        )
                        .expect("direct transaction has no dep-group expansion"),
                    ),
                }),
            });
        }
        if let Some(claim) = authority.linear.effect_claim {
            events.push(SystemEvent::Kernel {
                access,
                command: KernelCommand::SettleEffect(claim),
            });
        }
    }
    events
}

fn model_kernel_commands(state: &Omega) -> Vec<KernelCommand> {
    let first = Transaction::independent(1, 1, 10, 20);
    let first_variant = Transaction::independent(1, 9, 10, 20);
    let mut second = Transaction::independent(2, 2, 11, 21);
    second.header_deps.insert(HeaderId(1));
    let child = Transaction::dependent(3, 3, 20, 30);
    let mut commands = vec![
        remote(first.clone(), 7),
        retained(first_variant, RetainedSource::Proposal),
        retained(
            second.clone(),
            RetainedSource::Recovery(state.authority.generation),
        ),
        retained(child, RetainedSource::Proposal),
        KernelCommand::Checkout,
        KernelCommand::CheckoutContinuous(VerifyCapability::Any),
        KernelCommand::CheckoutContinuous(VerifyCapability::SmallCycleOnly),
        KernelCommand::BeginDirect {
            request: DirectRequestId(1),
            kind: DirectKind::Local,
            transaction: first.clone(),
        },
        KernelCommand::BeginDirect {
            request: DirectRequestId(2),
            kind: DirectKind::TestAccept,
            transaction: second.clone(),
        },
        KernelCommand::CaptureReady { limit: 2 },
        KernelCommand::FinalizeNext { wall_time: 10 },
        KernelCommand::Remove {
            transaction: first.id,
        },
        KernelCommand::BanPeer {
            peer: PeerId(7),
            observed_at: MonotonicTick(1),
        },
        KernelCommand::ReconcileChain(ChainTransition {
            context: super::kernel::ChainContextTransition::from_primitives(
                super::state::RulesId(1),
                true,
            ),
            from: state.authority.chain,
            to_tip: ViewId(2),
            committed: BTreeSet::from([first.id]),
            available_cells: BTreeSet::new(),
            available_headers: BTreeSet::new(),
            lost_cells: BTreeSet::new(),
            lost_headers: BTreeSet::from([HeaderId(1)]),
            conflicting_cells: BTreeSet::from([CellId(11)]),
            recovered: vec![first, second.clone()],
            proposals: super::proposal::ProposalContext::empty().view(),
        }),
        KernelCommand::ReplaceGeneration { view: ViewId(3) },
        KernelCommand::ExpireAccepted {
            wall_time: 20,
            residency: 5,
        },
        KernelCommand::ExpireRemote {
            wall_time: 20,
            limit: NonZeroU16::new(1).expect("one is non-zero"),
        },
        reconcile_view(state.authority.chain, ViewId(2)),
        KernelCommand::ClaimEffect,
        KernelCommand::SettleEffect(super::state::EffectClaim {
            source: super::state::EffectClaimSource::Queued,
            stamp: ApplyStamp(1),
            ordinal: 0,
        }),
        KernelCommand::Complete(Completion {
            capability: CapabilityId(u16::MAX),
            result: WorkResult::resolve_rejected(),
        }),
        KernelCommand::FinishExecution(Completion {
            capability: CapabilityId(u16::MAX),
            result: WorkResult::resolve_rejected(),
        }),
        KernelCommand::SettleFinished(CapabilityId(u16::MAX)),
        KernelCommand::CancelCapability(CapabilityId(u16::MAX)),
    ];

    let capture = match state
        .clone()
        .kernel_step(KernelCommand::CaptureReady { limit: 2 })
    {
        KernelStep::NoAuthorityCommit(KernelDisposition::ReadyCaptured(capture)) => capture,
        _ => ReadyCapture { keys: Vec::new() },
    };
    commands.push(KernelCommand::FinalizeCaptured {
        capture,
        wall_time: 10,
    });

    for capability in state.linear.work.values() {
        commands.extend([
            KernelCommand::Complete(Completion {
                capability: capability.id,
                result: WorkResult::Verified,
            }),
            KernelCommand::Complete(Completion {
                capability: capability.id,
                result: WorkResult::resolve_rejected(),
            }),
            KernelCommand::FinishExecution(Completion {
                capability: capability.id,
                result: WorkResult::Verified,
            }),
            KernelCommand::FinishExecution(Completion {
                capability: capability.id,
                result: WorkResult::resolve_rejected(),
            }),
            KernelCommand::CancelCapability(capability.id),
        ]);
        if let Some(owner) = state.authority.owners.get(&capability.transaction) {
            if matches!(capability.permit(), WorkPermit::ResolveThenVerify(_))
                && matches!(capability.stage(), WorkStage::Resolve)
            {
                commands.push(KernelCommand::ContinueResolveThenVerify(
                    ResolveContinuation {
                        capability: capability.id,
                        evidence: ResolvedEvidence::for_transaction(
                            &owner.transaction,
                            state.authority.chain,
                            state.authority.rules,
                        )
                        .expect("direct transaction has no dep-group expansion"),
                    },
                ));
            }
            if let Some(cell) = owner
                .transaction
                .inputs
                .iter()
                .chain(&owner.transaction.cell_deps)
                .next()
            {
                commands.push(KernelCommand::Complete(Completion {
                    capability: capability.id,
                    result: WorkResult::Missing(missing(
                        &owner.transaction,
                        BTreeSet::from([*cell]),
                    )),
                }));
            }
            commands.push(KernelCommand::Complete(Completion {
                capability: capability.id,
                result: WorkResult::Resolved(
                    ResolvedEvidence::for_transaction(
                        &owner.transaction,
                        state.authority.chain,
                        state.authority.rules,
                    )
                    .expect("direct transaction has no dep-group expansion"),
                ),
            }));
            commands.push(KernelCommand::FinishExecution(Completion {
                capability: capability.id,
                result: WorkResult::Resolved(
                    ResolvedEvidence::for_transaction(
                        &owner.transaction,
                        state.authority.chain,
                        state.authority.rules,
                    )
                    .expect("direct transaction has no dep-group expansion"),
                ),
            }));
            commands.push(KernelCommand::Complete(Completion {
                capability: capability.id,
                result: WorkResult::Resolved(
                    ResolvedEvidence::for_transaction(
                        &owner.transaction,
                        ChainView {
                            tip: ViewId(99),
                            revision: state.authority.chain.revision,
                        },
                        state.authority.rules,
                    )
                    .expect("direct transaction has no dep-group expansion"),
                ),
            }));
        }
    }
    for finished in state.linear.finished_work.values() {
        commands.push(KernelCommand::SettleFinished(finished.capability.id));
        commands.push(KernelCommand::CancelCapability(finished.capability.id));
    }
    for capability in state.linear.direct_work.values() {
        commands.push(KernelCommand::CompleteDirect(DirectCompletion {
            capability: capability.id,
            wall_time: 10,
            result: DirectWorkResult::Verified(
                ResolvedEvidence::for_transaction(
                    &capability.transaction,
                    state.authority.chain,
                    state.authority.rules,
                )
                .expect("direct transaction has no dep-group expansion"),
            ),
        }));
        commands.push(KernelCommand::CompleteDirect(DirectCompletion {
            capability: capability.id,
            wall_time: 10,
            result: DirectWorkResult::Rejected(state.capture_direct_negative(
                &capability.transaction,
                DirectNegativeReason::MissingDependency,
            )),
        }));
        commands.push(KernelCommand::CancelCapability(capability.id));
    }
    if let Some((source, effect)) = state.next_effect_record() {
        commands.push(KernelCommand::SettleEffect(super::state::EffectClaim {
            source,
            stamp: effect.stamp,
            ordinal: effect.ordinal,
        }));
    }
    commands
}

#[test]
fn model_kernel_sequence_helper_rejects_no_valid_transition() {
    let transaction = Transaction::independent(1, 1, 10, 20);
    let mut omega = model();
    let result = invariant_after_each(
        &mut omega,
        [remote(transaction, 7), KernelCommand::Checkout],
    );
    assert!(result.is_ok());
}
