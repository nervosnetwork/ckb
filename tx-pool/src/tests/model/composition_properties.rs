use super::composition::{
    BatchApplyDisposition, BatchPlanError, CohortClass, CompletionDrainApplyDisposition,
    ComputeExchangeApplyDisposition, CouplingReason, ExecutionCompletion, OrderedBatchFamily,
    RelationKind, RetainedAcquireDisposition, RetainedAcquireStop, RetainedPermitAcquirer,
    RetainedPermitGrant, RetainedPermitGrantErrorKind, analyze_ready_prefix, plan_completion_drain,
    plan_compute_exchange, plan_ordered_batch, plan_ready_batch, transaction_footprint,
};
use super::kernel::{
    Admission, Completion, KernelCommand, KernelDisposition, KernelStep, WorkResult,
};
use super::permit::{
    FairPermitScheduler, PermitClass, PermitDomain, PermitGrant, PermitReleaseDisposition,
    PermitReleaseError, PermitRequest, PermitRequestDisposition, PermitRequestId,
    RetainedPermitToken,
};
use super::state::{
    AcceptedStatus, ApplyStamp, CellId, EffectClass, HeaderId, InputOrigin, LogicalEffect,
    ModelLimits, Omega, OwnerLocation, PeerId, ProposalBase, RemoteDeadline, RemoteResidency,
    ResolvedEvidence, RetainedSource, RulesId, Source, Transaction, TxId, ViewId, WorkCapability,
};

fn model() -> Omega {
    Omega::new(
        ModelLimits::small()
            .validate()
            .expect("the composition fixture uses valid bounded limits"),
        ViewId(1),
        RulesId(1),
    )
}

fn proposal(transaction: Transaction) -> Admission {
    Admission {
        transaction,
        source: RetainedSource::Proposal,
        observed_at: super::state::MonotonicTick(1),
    }
}

fn remote(transaction: Transaction, peer: u8) -> Admission {
    Admission {
        transaction,
        source: RetainedSource::Remote(RemoteResidency {
            peer: PeerId(peer),
            expires_at: RemoteDeadline(100),
        }),
        observed_at: super::state::MonotonicTick(1),
    }
}

fn ready(omega: &mut Omega, transaction: Transaction, evidence: ResolvedEvidence) {
    let capability = computing_verify(omega, transaction, evidence);
    assert!(matches!(
        omega.kernel_step(KernelCommand::Complete(Completion {
            capability: capability.id,
            result: WorkResult::Verified,
        })),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::Ready(_),
            ..
        }
    ));
}

fn computing_verify(
    omega: &mut Omega,
    transaction: Transaction,
    evidence: ResolvedEvidence,
) -> WorkCapability {
    assert!(matches!(
        omega.kernel_step(KernelCommand::Admit(proposal(transaction.clone()))),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::Retained(_),
            ..
        }
    ));
    let first = match omega.kernel_step(KernelCommand::Checkout) {
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::CheckedOut(capability),
            ..
        } => capability,
        other => panic!("expected resolve checkout, got {other:?}"),
    };
    assert!(matches!(
        omega.kernel_step(KernelCommand::Complete(Completion {
            capability: first.id,
            result: WorkResult::Resolved(evidence),
        })),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::Continued(_),
            ..
        }
    ));
    match omega.kernel_step(KernelCommand::Checkout) {
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::CheckedOut(capability),
            ..
        } => capability,
        other => panic!("expected verify checkout, got {other:?}"),
    }
}

fn computing_verify_chain(omega: &mut Omega, transaction: Transaction) -> WorkCapability {
    let evidence = ResolvedEvidence::for_transaction(
        &transaction,
        omega.authority.chain,
        omega.authority.rules,
    );
    computing_verify(omega, transaction, evidence)
}

fn granted_retained(
    scheduler: &mut FairPermitScheduler,
    request: u8,
) -> (PermitRequest, RetainedPermitToken) {
    let request = PermitRequest {
        id: PermitRequestId(request),
        class: PermitClass::Retained,
    };
    match scheduler.request(request) {
        PermitRequestDisposition::Granted {
            grant: PermitGrant::Retained(token),
        } => (token.request(), token),
        other => panic!("expected retained permit, got {other:?}"),
    }
}

fn retained_grant(tokens: impl IntoIterator<Item = RetainedPermitToken>) -> RetainedPermitGrant {
    RetainedPermitGrant::try_from_tokens(tokens)
        .unwrap_or_else(|error| panic!("the fixture must contain one scheduler domain: {error:?}"))
}

fn accept(omega: &mut Omega, transaction: Transaction) {
    let evidence = ResolvedEvidence::for_transaction(
        &transaction,
        omega.authority.chain,
        omega.authority.rules,
    );
    ready(omega, transaction, evidence);
    assert!(matches!(
        omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 1 }),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::Accepted(_),
            ..
        } | KernelStep::AuthorityCommit {
            disposition: KernelDisposition::AcceptedBatch(_),
            ..
        }
    ));
}

#[test]
fn model_dynamic_footprint_keeps_reads_writes_headers_and_pool_origin_distinct() {
    let mut transaction = Transaction::independent(1, 1, 10, 20);
    transaction.deps.insert(CellId(11));
    transaction.header_deps.insert(HeaderId(7));
    let mut evidence = ResolvedEvidence::for_transaction(
        &transaction,
        super::state::ChainView::initial(ViewId(1)),
        RulesId(1),
    );
    evidence
        .input_origins
        .insert(CellId(10), InputOrigin::Pool(super::state::TxId(9)));
    let footprint = transaction_footprint(
        &transaction,
        &evidence,
        super::state::EntryVersion(1),
        Source::Proposal {
            base: ProposalBase::Trusted,
        },
    )
    .expect("the footprint fixture is exact");
    assert_eq!(footprint.consumes, [CellId(10)].into_iter().collect());
    assert_eq!(footprint.produces, [CellId(20)].into_iter().collect());
    assert_eq!(footprint.reads, [CellId(11)].into_iter().collect());
    assert_eq!(footprint.header_reads, [HeaderId(7)].into_iter().collect());
    assert_eq!(
        footprint.pool_inputs,
        [(CellId(10), super::state::TxId(9))].into_iter().collect()
    );
    assert!(footprint.pool_reads.is_empty());
    assert_eq!(footprint.context, evidence.context);
}

#[test]
fn model_shared_read_only_dependencies_and_headers_remain_composable() {
    let mut omega = model();
    let mut first = Transaction::independent(1, 1, 10, 20);
    first.deps.insert(CellId(50));
    first.header_deps.insert(HeaderId(5));
    let mut second = Transaction::independent(2, 2, 11, 21);
    second.deps.insert(CellId(50));
    second.header_deps.insert(HeaderId(5));
    for transaction in [first, second] {
        let evidence = ResolvedEvidence::for_transaction(
            &transaction,
            omega.authority.chain,
            omega.authority.rules,
        );
        ready(&mut omega, transaction, evidence);
    }
    let analysis = analyze_ready_prefix(&omega, 2);
    assert_eq!(analysis.class, CohortClass::IndependentComposable);
    assert_eq!(analysis.prefix.len(), 2);
    assert_eq!(analysis.stopped_by, None);
    assert_eq!(analysis.cost.candidates, 2);
    assert_eq!(analysis.cost.header_keys, 2);
    assert_eq!(analysis.cost.pool_edges, 0);
}

#[test]
fn model_shared_input_stops_at_the_first_coupled_ready_member() {
    let mut omega = model();
    let first = Transaction::independent(1, 1, 10, 20);
    let second = Transaction::independent(2, 2, 10, 21);
    for transaction in [first, second] {
        let evidence = ResolvedEvidence::for_transaction(
            &transaction,
            omega.authority.chain,
            omega.authority.rules,
        );
        ready(&mut omega, transaction, evidence);
    }
    let analysis = analyze_ready_prefix(&omega, 2);
    assert_eq!(analysis.class, CohortClass::IndependentComposable);
    assert_eq!(analysis.prefix.len(), 1);
    assert!(matches!(
        analysis.stopped_by,
        Some(CouplingReason::CandidateRelation {
            kind: RelationKind::SharedInput,
            ..
        })
    ));
}

#[test]
fn model_reader_spender_relation_is_coupled_but_shared_readers_are_not() {
    let mut omega = model();
    let mut reader = Transaction::independent(1, 1, 11, 20);
    reader.deps.insert(CellId(10));
    let spender = Transaction::independent(2, 2, 10, 21);
    for transaction in [reader, spender] {
        let evidence = ResolvedEvidence::for_transaction(
            &transaction,
            omega.authority.chain,
            omega.authority.rules,
        );
        ready(&mut omega, transaction, evidence);
    }
    let analysis = analyze_ready_prefix(&omega, 2);
    assert_eq!(analysis.prefix.len(), 1);
    assert!(matches!(
        analysis.stopped_by,
        Some(CouplingReason::CandidateRelation {
            kind: RelationKind::CandidateSpendsRead,
            ..
        })
    ));
}

#[test]
fn model_candidate_parent_child_relation_is_coupled() {
    let mut omega = model();
    let parent = Transaction::independent(1, 1, 10, 20);
    let child = Transaction::dependent(2, 2, 20, 30);
    for transaction in [parent, child] {
        let evidence = ResolvedEvidence::for_transaction(
            &transaction,
            omega.authority.chain,
            omega.authority.rules,
        );
        ready(&mut omega, transaction, evidence);
    }
    let analysis = analyze_ready_prefix(&omega, 2);
    assert_eq!(analysis.prefix.len(), 1);
    assert!(matches!(
        analysis.stopped_by,
        Some(CouplingReason::CandidateRelation {
            kind: RelationKind::CandidateProducesInput,
            ..
        })
    ));
}

#[test]
fn model_pool_origin_routes_the_ready_head_to_the_coupled_planner() {
    let mut omega = model();
    let parent = Transaction::independent(1, 1, 10, 20);
    accept(&mut omega, parent.clone());
    let child = Transaction::dependent(2, 2, 20, 30);
    let evidence = ResolvedEvidence::with_pool_input(
        &child,
        omega.authority.chain,
        omega.authority.rules,
        CellId(20),
        parent.id,
    );
    ready(&mut omega, child.clone(), evidence);
    let analysis = analyze_ready_prefix(&omega, 1);
    assert!(matches!(
        analysis.class,
        CohortClass::Coupled(CouplingReason::PoolOrigin {
            transaction,
            parent: observed_parent,
        }) if transaction == child.id && observed_parent == parent.id
    ));
}

#[test]
fn model_accepted_reader_relation_is_visible_before_ready_apply() {
    let mut omega = model();
    let mut reader = Transaction::independent(1, 1, 11, 20);
    reader.deps.insert(CellId(10));
    accept(&mut omega, reader);
    let spender = Transaction::independent(2, 2, 10, 21);
    let evidence =
        ResolvedEvidence::for_transaction(&spender, omega.authority.chain, omega.authority.rules);
    ready(&mut omega, spender, evidence);
    let analysis = analyze_ready_prefix(&omega, 1);
    assert!(matches!(
        analysis.class,
        CohortClass::Coupled(CouplingReason::AcceptedRelation {
            kind: RelationKind::CandidateSpendsRead,
            ..
        })
    ));
}

#[test]
fn model_ready_prefix_stops_before_aggregate_accepted_capacity_exclusion() {
    let mut limits = ModelLimits::small();
    limits.accepted.entries = 1;
    let mut omega = Omega::new(
        limits
            .validate()
            .expect("the accepted partition remains a valid sub-partition"),
        ViewId(1),
        RulesId(1),
    );
    for transaction in [
        Transaction::independent(1, 1, 10, 20),
        Transaction::independent(2, 2, 11, 21),
    ] {
        let evidence = ResolvedEvidence::for_transaction(
            &transaction,
            omega.authority.chain,
            omega.authority.rules,
        );
        ready(&mut omega, transaction, evidence);
    }
    let analysis = analyze_ready_prefix(&omega, 2);
    assert_eq!(analysis.prefix.len(), 1);
    assert!(matches!(
        analysis.stopped_by,
        Some(CouplingReason::AcceptedCapacity(_))
    ));
}

#[test]
fn model_ready_footprint_cost_is_linear_in_scanned_owners_and_keys() {
    let mut omega = model();
    let accepted = Transaction::independent(9, 9, 90, 91);
    accept(&mut omega, accepted);
    for transaction in [
        Transaction::independent(1, 1, 10, 20),
        Transaction::independent(2, 2, 11, 21),
    ] {
        let evidence = ResolvedEvidence::for_transaction(
            &transaction,
            omega.authority.chain,
            omega.authority.rules,
        );
        ready(&mut omega, transaction, evidence);
    }
    let analysis = analyze_ready_prefix(&omega, 2);
    assert_eq!(analysis.cost.accepted_owners_scanned, 1);
    assert_eq!(analysis.cost.accepted_edges_scanned, 2);
    assert_eq!(analysis.cost.candidates, 2);
    assert_eq!(analysis.cost.cell_keys, 4);
    assert_eq!(analysis.cost.linear_key_bound(), Some(6));
    assert_eq!(analysis.cost.index_operations, 6);
    assert_eq!(analysis.cost.scratch_entries, 6);
}

#[test]
fn model_remote_source_is_a_footprint_policy_term_not_a_second_authority() {
    let mut omega = model();
    let transaction = Transaction::independent(1, 1, 10, 20);
    assert!(matches!(
        omega.kernel_step(KernelCommand::Admit(remote(transaction.clone(), 7))),
        KernelStep::AuthorityCommit { .. }
    ));
    let owner = omega
        .authority
        .owners
        .get(&transaction.id)
        .expect("the retained owner exists");
    assert!(matches!(owner.location, OwnerLocation::Retained(_)));
    assert_eq!(owner.ingress_peer(), Some(PeerId(7)));
    assert!(!matches!(
        owner.location,
        OwnerLocation::Accepted {
            status: AcceptedStatus::Pending,
            ..
        }
    ));
}

#[test]
fn model_ready_prefix_stops_at_the_first_effect_control_class_boundary() {
    let mut omega = model();
    let trusted = Transaction::independent(1, 1, 10, 20);
    let remote_transaction = Transaction::independent(2, 2, 11, 21);
    let trusted_evidence =
        ResolvedEvidence::for_transaction(&trusted, omega.authority.chain, omega.authority.rules);
    ready(&mut omega, trusted.clone(), trusted_evidence);

    assert!(matches!(
        omega.kernel_step(KernelCommand::Admit(remote(remote_transaction.clone(), 7))),
        KernelStep::AuthorityCommit { .. }
    ));
    let resolve = match omega.kernel_step(KernelCommand::Checkout) {
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::CheckedOut(capability),
            ..
        } => capability,
        other => panic!("expected remote resolve checkout, got {other:?}"),
    };
    let evidence = ResolvedEvidence::for_transaction(
        &remote_transaction,
        omega.authority.chain,
        omega.authority.rules,
    );
    assert!(matches!(
        omega.kernel_step(KernelCommand::Complete(Completion {
            capability: resolve.id,
            result: WorkResult::Resolved(evidence),
        })),
        KernelStep::AuthorityCommit { .. }
    ));
    let verify = match omega.kernel_step(KernelCommand::Checkout) {
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::CheckedOut(capability),
            ..
        } => capability,
        other => panic!("expected remote verify checkout, got {other:?}"),
    };
    assert!(matches!(
        omega.kernel_step(KernelCommand::Complete(Completion {
            capability: verify.id,
            result: WorkResult::Verified,
        })),
        KernelStep::AuthorityCommit { .. }
    ));

    let analysis = analyze_ready_prefix(&omega, 2);
    assert_eq!(
        analysis
            .prefix
            .iter()
            .map(|footprint| footprint.transaction)
            .collect::<Vec<_>>(),
        vec![trusted.id]
    );
    assert!(matches!(
        analysis.stopped_by,
        Some(CouplingReason::EffectClassBoundary(id)) if id == remote_transaction.id
    ));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_ordered_ingress_batch_is_one_apply_and_matches_canonical_submission() {
    let mut omega = model();
    let first = Transaction::independent(1, 1, 10, 20);
    let second = Transaction::independent(2, 2, 11, 21);
    let commands = vec![
        KernelCommand::Admit(proposal(first.clone())),
        KernelCommand::Admit(proposal(second.clone())),
    ];
    let before = omega.clone();
    let plan = plan_ordered_batch(&omega, OrderedBatchFamily::RetainedIngress, commands)
        .expect("the canonical ingress cohort is representable");
    assert_eq!(omega, before, "Plan must be read-only");
    assert_eq!(plan.class, CohortClass::CanonicalOrdered);
    assert_eq!(plan.sequential_apply_count, 2);
    assert_eq!(plan.committed_stamp, Some(ApplyStamp(1)));
    let planned = plan.planned_state();
    assert_eq!(planned.authority.last_apply, ApplyStamp(1));
    let first_owner = planned
        .authority
        .owners
        .get(&first.id)
        .expect("first owner exists");
    let second_owner = planned
        .authority
        .owners
        .get(&second.id)
        .expect("second owner exists");
    assert!(first_owner.arrival < second_owner.arrival);
    assert_ne!(first_owner.version, second_owner.version);
    assert!(matches!(
        plan.apply(&mut omega),
        BatchApplyDisposition::Applied {
            stamp: Some(ApplyStamp(1)),
            ..
        }
    ));
    assert!(omega.check_invariants().is_ok());
}

#[test]
fn model_batch_reserves_one_apply_stamp_even_at_the_counter_boundary() {
    let mut omega = model();
    omega.authority.last_apply = ApplyStamp(u16::MAX - 1);
    let plan = plan_ordered_batch(
        &omega,
        OrderedBatchFamily::RetainedIngress,
        vec![
            KernelCommand::Admit(proposal(Transaction::independent(1, 1, 10, 20))),
            KernelCommand::Admit(proposal(Transaction::independent(2, 2, 11, 21))),
        ],
    )
    .expect("one batch stamp remains even though two sequential stamps do not");
    assert_eq!(plan.sequential_apply_count, 2);
    assert_eq!(plan.committed_stamp, Some(ApplyStamp(u16::MAX)));
    assert_eq!(
        plan.planned_state().authority.last_apply,
        ApplyStamp(u16::MAX)
    );
    assert!(matches!(
        plan.apply(&mut omega),
        BatchApplyDisposition::Applied {
            stamp: Some(ApplyStamp(u16::MAX)),
            ..
        }
    ));
}

#[test]
fn model_batch_plan_rejects_a_changed_authority_cut_without_mutation() {
    let mut omega = model();
    let plan = plan_ordered_batch(
        &omega,
        OrderedBatchFamily::RetainedIngress,
        vec![KernelCommand::Admit(proposal(Transaction::independent(
            1, 1, 10, 20,
        )))],
    )
    .expect("one ingress item is a canonical ordered batch");
    assert!(matches!(
        omega.kernel_step(KernelCommand::Admit(proposal(Transaction::independent(
            2, 2, 11, 21,
        )))),
        KernelStep::AuthorityCommit { .. }
    ));
    let changed = omega.clone();
    assert_eq!(plan.apply(&mut omega), BatchApplyDisposition::Stale);
    assert_eq!(omega, changed);
}

#[test]
fn model_ready_batch_equals_the_canonical_no_interleave_fold_with_one_stamp() {
    let mut omega = model();
    for transaction in [
        Transaction::independent(1, 1, 10, 20),
        Transaction::independent(2, 2, 11, 21),
    ] {
        let evidence = ResolvedEvidence::for_transaction(
            &transaction,
            omega.authority.chain,
            omega.authority.rules,
        );
        ready(&mut omega, transaction, evidence);
    }
    let before_stamp = omega.authority.last_apply;
    let plan =
        plan_ready_batch(&omega, 2, 1).expect("two chain-backed disjoint Ready owners compose");
    assert_eq!(plan.class, CohortClass::IndependentComposable);
    assert_eq!(plan.sequential_apply_count, 2);
    assert_eq!(plan.committed_stamp, Some(ApplyStamp(before_stamp.0 + 1)));
    assert_eq!(
        plan.planned_state().authority.last_apply,
        ApplyStamp(before_stamp.0 + 1)
    );
    assert!(
        plan.planned_state()
            .authority
            .owners
            .values()
            .all(|owner| matches!(owner.location, OwnerLocation::Accepted { .. }))
    );
    assert!(matches!(
        plan.apply(&mut omega),
        BatchApplyDisposition::Applied { .. }
    ));
    assert!(omega.check_invariants().is_ok());
}

#[test]
fn model_ordered_batch_refuses_an_unowned_transition_family() {
    let omega = model();
    assert_eq!(
        plan_ordered_batch(
            &omega,
            OrderedBatchFamily::RetainedIngress,
            vec![KernelCommand::Remove {
                transaction: super::state::TxId(1),
            }],
        ),
        Err(BatchPlanError::UnsupportedCommand)
    );
}

#[test]
fn model_compute_exchange_settles_and_refills_all_available_slots_in_one_apply() {
    let mut omega = model();
    let first = Transaction::independent(1, 1, 10, 20);
    let second = Transaction::independent(2, 2, 11, 21);
    let third = Transaction::independent(3, 3, 12, 22);
    let fourth = Transaction::independent(4, 4, 13, 23);
    let first_capability = computing_verify_chain(&mut omega, first.clone());
    let second_capability = computing_verify_chain(&mut omega, second.clone());
    assert!(matches!(
        omega.kernel_step(KernelCommand::Admit(proposal(third.clone()))),
        KernelStep::AuthorityCommit { .. }
    ));
    assert!(matches!(
        omega.kernel_step(KernelCommand::Admit(proposal(fourth.clone()))),
        KernelStep::AuthorityCommit { .. }
    ));
    let mut permits =
        FairPermitScheduler::new(PermitDomain(1), 2, 4).expect("valid fair permit fixture");
    let (_, old_first) = granted_retained(&mut permits, 1);
    let (_, old_second) = granted_retained(&mut permits, 2);
    let old_first_id = old_first.request().id;
    let old_second_id = old_second.request().id;
    let drain = plan_completion_drain(
        &omega,
        &permits,
        vec![
            ExecutionCompletion {
                permit: old_second,
                completion: Completion {
                    capability: second_capability.id,
                    result: WorkResult::Verified,
                },
            },
            ExecutionCompletion {
                permit: old_first,
                completion: Completion {
                    capability: first_capability.id,
                    result: WorkResult::Verified,
                },
            },
        ],
    )
    .expect("the already-available completion wave drains without a timer");
    assert_eq!(drain.batch.sequential_apply_count, 0);
    assert_eq!(drain.batch.committed_stamp, None);
    let released = match drain.apply(&mut omega) {
        CompletionDrainApplyDisposition::Applied {
            finished,
            retired,
            released,
            ..
        } => {
            assert_eq!(finished, vec![first_capability.id, second_capability.id]);
            assert!(retired.is_empty());
            released
        }
        other => panic!("expected completion drain, got {other:?}"),
    };
    assert_eq!(
        released
            .iter()
            .map(|(token, capability)| (token.request().id, *capability))
            .collect::<Vec<_>>(),
        vec![
            (old_first_id, first_capability.id),
            (old_second_id, second_capability.id),
        ]
    );
    for (token, _) in released {
        assert!(matches!(
            permits.release(token.into()),
            PermitReleaseDisposition::Released { next: None, .. }
        ));
    }
    let (_, next_first) = granted_retained(&mut permits, 3);
    let (_, next_second) = granted_retained(&mut permits, 4);
    let grants = retained_grant([next_second, next_first]);
    let before_stamp = omega.authority.last_apply;
    let plan = plan_compute_exchange(
        &omega,
        vec![second_capability.id, first_capability.id],
        grants,
    )
    .expect("the independent completion exchange is representable");
    assert_eq!(
        plan.settled,
        vec![first_capability.id, second_capability.id]
    );
    assert_eq!(plan.unused_grants, Vec::new());
    assert_eq!(plan.assigned.len(), 2);
    assert_eq!(plan.assigned[0].1.transaction, third.id);
    assert_eq!(plan.assigned[1].1.transaction, fourth.id);
    assert_eq!(plan.batch.sequential_apply_count, 4);
    assert_eq!(
        plan.batch.committed_stamp,
        Some(ApplyStamp(before_stamp.0 + 1))
    );
    assert!(matches!(
        plan.apply(&mut omega),
        ComputeExchangeApplyDisposition::Applied {
            ref assignments,
            ref unused_grants,
            ..
        } if assignments.len() == 2 && unused_grants.is_empty()
    ));
    assert_eq!(omega.linear.work.len(), 2);
    assert_eq!(omega.linear.free_compute_permits, 0);
    assert!(matches!(
        omega
            .authority
            .owners
            .get(&first.id)
            .map(|owner| &owner.location),
        Some(OwnerLocation::Retained(super::state::RetainedOwner {
            phase: super::state::RetainedPhase::Ready(_),
            ..
        }))
    ));
    assert!(omega.check_invariants().is_ok());
}

#[test]
fn model_initial_compute_wave_checks_out_every_available_worker_in_one_apply() {
    let mut omega = model();
    let first = Transaction::independent(1, 1, 10, 20);
    let second = Transaction::independent(2, 2, 11, 21);
    for transaction in [first.clone(), second.clone()] {
        assert!(matches!(
            omega.kernel_step(KernelCommand::Admit(proposal(transaction))),
            KernelStep::AuthorityCommit { .. }
        ));
    }
    let mut permits =
        FairPermitScheduler::new(PermitDomain(1), 2, 2).expect("valid fair permit fixture");
    let (_, first_lease) = granted_retained(&mut permits, 1);
    let (_, second_lease) = granted_retained(&mut permits, 2);
    let before = omega.authority.last_apply;
    let exchange = plan_compute_exchange(
        &omega,
        Vec::new(),
        retained_grant([second_lease, first_lease]),
    )
    .expect("the initial queued wave is canonically ordered");
    assert!(exchange.settled.is_empty());
    assert_eq!(exchange.assigned.len(), 2);
    assert_eq!(exchange.assigned[0].1.transaction, first.id);
    assert_eq!(exchange.assigned[1].1.transaction, second.id);
    assert_eq!(exchange.batch.sequential_apply_count, 2);
    assert_eq!(
        exchange.batch.committed_stamp,
        Some(ApplyStamp(before.0 + 1))
    );
    assert!(matches!(
        exchange.apply(&mut omega),
        ComputeExchangeApplyDisposition::Applied {
            ref assignments,
            ref unused_grants,
            ..
        } if assignments.len() == 2 && unused_grants.is_empty()
    ));
    assert_eq!(omega.linear.work.len(), 2);
    assert_eq!(omega.linear.free_compute_permits, 0);
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_compute_exchange_is_invariant_to_worker_completion_order() {
    let mut omega = model();
    let first = Transaction::independent(1, 1, 10, 20);
    let second = Transaction::independent(2, 2, 11, 21);
    let first_capability = computing_verify_chain(&mut omega, first.clone());
    let second_capability = computing_verify_chain(&mut omega, second.clone());
    for capability in [&first_capability, &second_capability] {
        assert_eq!(
            omega.kernel_step(KernelCommand::FinishExecution(Completion {
                capability: capability.id,
                result: WorkResult::Verified,
            })),
            KernelStep::NoAuthorityCommit(KernelDisposition::Finished(capability.id))
        );
    }
    let completions = [first_capability.id, second_capability.id];
    let left = plan_compute_exchange(&omega, completions.to_vec(), RetainedPermitGrant::empty())
        .expect("forward completion order is representable");
    let right = plan_compute_exchange(
        &omega,
        completions.into_iter().rev().collect(),
        RetainedPermitGrant::empty(),
    )
    .expect("reverse completion order is representable");
    assert_eq!(left.settled, right.settled);
    assert_eq!(left.batch.planned_state(), right.batch.planned_state());
    assert_eq!(left.batch.dispositions, right.batch.dispositions);
}

#[test]
fn model_stale_compute_exchange_returns_every_fair_grant_without_mutation() {
    let mut limits = ModelLimits::small();
    limits.compute_permits = 1;
    let mut omega = Omega::new(
        limits.validate().expect("one compute permit is valid"),
        ViewId(1),
        RulesId(1),
    );
    let first = Transaction::independent(1, 1, 10, 20);
    let second = Transaction::independent(2, 2, 11, 21);
    let capability = computing_verify_chain(&mut omega, first);
    assert!(matches!(
        omega.kernel_step(KernelCommand::Admit(proposal(second))),
        KernelStep::AuthorityCommit { .. }
    ));
    assert_eq!(
        omega.kernel_step(KernelCommand::FinishExecution(Completion {
            capability: capability.id,
            result: WorkResult::Verified,
        })),
        KernelStep::NoAuthorityCommit(KernelDisposition::Finished(capability.id))
    );

    let mut permits =
        FairPermitScheduler::new(PermitDomain(1), 1, 1).expect("valid fair permit fixture");
    let (_, token) = granted_retained(&mut permits, 1);
    let request_id = token.request().id;
    let plan = plan_compute_exchange(&omega, vec![capability.id], retained_grant([token]))
        .expect("the original cut admits one settlement and checkout");

    assert!(matches!(
        omega.kernel_step(KernelCommand::Admit(proposal(Transaction::independent(
            3, 3, 12, 22,
        )))),
        KernelStep::AuthorityCommit { .. }
    ));
    let changed = omega.clone();
    let ComputeExchangeApplyDisposition::Stale { grants } = plan.apply(&mut omega) else {
        panic!("the changed authority cut must return the move-only grant");
    };
    assert_eq!(grants.request_ids(), [request_id].into_iter().collect());
    assert_eq!(omega, changed);
    assert!(omega.linear.finished_work.contains_key(&capability.id));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_chain_race_retires_finished_evidence_and_rechecks_out_current_resolve_work() {
    let mut limits = ModelLimits::small();
    limits.compute_permits = 1;
    let mut omega = Omega::new(
        limits.validate().expect("one compute permit is valid"),
        ViewId(1),
        RulesId(1),
    );
    let transaction = Transaction::independent(1, 1, 10, 20);
    let capability = computing_verify_chain(&mut omega, transaction.clone());
    let mut permits =
        FairPermitScheduler::new(PermitDomain(1), 1, 2).expect("valid fair permit fixture");
    let (_, old_token) = granted_retained(&mut permits, 1);
    let drain = plan_completion_drain(
        &omega,
        &permits,
        vec![ExecutionCompletion {
            permit: old_token,
            completion: Completion {
                capability: capability.id,
                result: WorkResult::Verified,
            },
        }],
    )
    .expect("the completion is current before the chain race");
    let released = match drain.apply(&mut omega) {
        CompletionDrainApplyDisposition::Applied { released, .. } => released,
        other => panic!("expected the completion token back, got {other:?}"),
    };
    let [(old_token, observed_capability)] = released
        .try_into()
        .unwrap_or_else(|_| panic!("one completion must return exactly one permit token"));
    assert_eq!(observed_capability, capability.id);
    assert!(matches!(
        permits.release(old_token.into()),
        PermitReleaseDisposition::Released { next: None, .. }
    ));
    let (_, next_token) = granted_retained(&mut permits, 2);
    let next_request_id = next_token.request().id;

    let from = omega.authority.chain;
    assert!(matches!(
        omega.kernel_step(KernelCommand::ReconcileChain(
            super::kernel::ChainTransition {
                from,
                to_tip: ViewId(2),
                committed: Default::default(),
                available_cells: Default::default(),
                available_headers: Default::default(),
                lost_cells: Default::default(),
                lost_headers: Default::default(),
                conflicting_cells: Default::default(),
                recovered: Vec::new(),
                proposed: [transaction.id].into_iter().collect(),
                gap: Default::default(),
            },
        )),
        KernelStep::AuthorityCommit { .. }
    ));
    let exchange = plan_compute_exchange(&omega, vec![capability.id], retained_grant([next_token]))
        .expect("stale settlement and current checkout compose");
    assert_eq!(
        exchange.batch.sequential_apply_count, 1,
        "exchange dispositions: {:?}",
        exchange.batch.dispositions
    );
    let assignments = match exchange.apply(&mut omega) {
        ComputeExchangeApplyDisposition::Applied {
            settled,
            assignments,
            unused_grants,
            ..
        } => {
            assert_eq!(settled, vec![capability.id]);
            assert!(unused_grants.is_empty());
            assignments
        }
        other => panic!("expected current re-checkout, got {other:?}"),
    };
    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0].0.request().id, next_request_id);
    assert_eq!(assignments[0].1.transaction, transaction.id);
    assert_eq!(assignments[0].1.kind(), super::state::WorkKind::Resolve);
    assert_eq!(assignments[0].1.chain, omega.authority.chain);
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_effect_pressure_retries_the_bounded_finished_slot_after_capacity_frees() {
    let mut omega = model();
    let first = Transaction::independent(1, 1, 10, 20);
    let second = Transaction::independent(2, 2, 11, 21);
    let first_capability = computing_verify_chain(&mut omega, first.clone());
    let second_capability = computing_verify_chain(&mut omega, second.clone());
    let mut permits =
        FairPermitScheduler::new(PermitDomain(1), 2, 2).expect("valid fair permit fixture");
    let (_, first_token) = granted_retained(&mut permits, 1);
    let (_, second_token) = granted_retained(&mut permits, 2);
    let first_request_id = first_token.request().id;
    let second_request_id = second_token.request().id;
    let drain = plan_completion_drain(
        &omega,
        &permits,
        vec![
            ExecutionCompletion {
                permit: first_token,
                completion: Completion {
                    capability: first_capability.id,
                    result: WorkResult::Rejected,
                },
            },
            ExecutionCompletion {
                permit: second_token,
                completion: Completion {
                    capability: second_capability.id,
                    result: WorkResult::Verified,
                },
            },
        ],
    )
    .expect("the complete worker wave is bounded independently of effect capacity");
    let released = match drain.apply(&mut omega) {
        CompletionDrainApplyDisposition::Applied {
            finished,
            retired,
            released,
            ..
        } => {
            assert_eq!(finished, vec![first_capability.id, second_capability.id]);
            assert!(retired.is_empty());
            released
        }
        other => panic!("expected completion drain, got {other:?}"),
    };
    assert_eq!(
        released
            .iter()
            .map(|(token, capability)| (token.request().id, *capability))
            .collect::<Vec<_>>(),
        vec![
            (first_request_id, first_capability.id),
            (second_request_id, second_capability.id),
        ]
    );
    for (token, _) in released {
        assert!(matches!(
            permits.release(token.into()),
            PermitReleaseDisposition::Released { next: None, .. }
        ));
    }
    assert_eq!(
        omega.linear.free_compute_permits,
        omega.authority.limits.compute_permits
    );
    while omega.append_effect_fixture(
        EffectClass::Trusted,
        vec![LogicalEffect::IngressReleased(TxId(200))],
    ) {}
    assert_eq!(omega.check_invariants(), Ok(()));
    let plan = plan_compute_exchange(
        &omega,
        vec![second_capability.id, first_capability.id],
        RetainedPermitGrant::empty(),
    )
    .expect("effect pressure cannot block the unrelated no-effect member");
    assert_eq!(
        plan.attempted,
        vec![first_capability.id, second_capability.id]
    );
    assert_eq!(plan.settled, vec![second_capability.id]);
    assert_eq!(plan.blocked, vec![first_capability.id]);
    assert_eq!(plan.batch.sequential_apply_count, 1);
    let planned = plan.batch.planned_state();
    assert!(
        planned
            .linear
            .finished_work
            .contains_key(&first_capability.id)
    );
    assert!(
        !planned
            .linear
            .finished_work
            .contains_key(&second_capability.id)
    );
    assert_eq!(
        planned.linear.free_compute_permits,
        planned.authority.limits.compute_permits
    );
    assert!(matches!(
        plan.apply(&mut omega),
        ComputeExchangeApplyDisposition::Applied {
            ref settled,
            ref blocked,
            ref assignments,
            ref unused_grants,
            ..
        } if settled == &[second_capability.id]
            && blocked == &[first_capability.id]
            && assignments.is_empty()
            && unused_grants.is_empty()
    ));
    assert!(
        omega
            .linear
            .finished_work
            .contains_key(&first_capability.id)
    );
    assert!(
        !omega
            .linear
            .finished_work
            .contains_key(&second_capability.id)
    );
    assert_eq!(permits.check_invariants(), Ok(()));

    let claim = match omega.kernel_step(KernelCommand::ClaimEffect) {
        KernelStep::NoAuthorityCommit(KernelDisposition::EffectClaimed(claim)) => claim,
        other => panic!("the publisher must claim the oldest committed effect, got {other:?}"),
    };
    assert!(matches!(
        omega.kernel_step(KernelCommand::SettleEffect(claim)),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::EffectSettled(observed),
            ..
        } if observed == claim
    ));

    let retry = plan_compute_exchange(
        &omega,
        vec![first_capability.id],
        RetainedPermitGrant::empty(),
    )
    .expect("freed effect capacity level-triggers the same finished slot");
    assert_eq!(retry.settled, vec![first_capability.id]);
    assert!(retry.blocked.is_empty());
    assert!(matches!(
        retry.apply(&mut omega),
        ComputeExchangeApplyDisposition::Applied {
            ref settled,
            ref blocked,
            ref assignments,
            ref unused_grants,
            ..
        } if settled == &[first_capability.id]
            && blocked.is_empty()
            && assignments.is_empty()
            && unused_grants.is_empty()
    ));
    assert!(omega.linear.finished_work.is_empty());
    assert!(!omega.authority.owners.contains_key(&first.id));
    assert!(omega.authority.owners.contains_key(&second.id));
    assert_eq!(permits.check_invariants(), Ok(()));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_local_waiter_receives_the_released_permit_before_retained_reuse() {
    let mut limits = ModelLimits::small();
    limits.compute_permits = 1;
    let mut omega = Omega::new(
        limits.validate().expect("one compute permit is valid"),
        ViewId(1),
        RulesId(1),
    );
    let first = Transaction::independent(1, 1, 10, 20);
    let second = Transaction::independent(2, 2, 11, 21);
    let capability = computing_verify_chain(&mut omega, first.clone());
    assert!(matches!(
        omega.kernel_step(KernelCommand::Admit(proposal(second))),
        KernelStep::AuthorityCommit { .. }
    ));
    let mut permits =
        FairPermitScheduler::new(PermitDomain(1), 1, 3).expect("valid fair permit fixture");
    let (_, old) = granted_retained(&mut permits, 1);
    let local = PermitRequest {
        id: PermitRequestId(2),
        class: PermitClass::Direct,
    };
    assert_eq!(
        permits.request(local),
        PermitRequestDisposition::Queued(local.id)
    );
    let drain = plan_completion_drain(
        &omega,
        &permits,
        vec![ExecutionCompletion {
            permit: old,
            completion: Completion {
                capability: capability.id,
                result: WorkResult::Verified,
            },
        }],
    )
    .expect("the completed retained work is immediately classifiable");
    let released = match drain.apply(&mut omega) {
        CompletionDrainApplyDisposition::Applied { released, .. } => released,
        other => panic!("expected the retained permit token back, got {other:?}"),
    };
    let [(old_token, observed_capability)] = released
        .try_into()
        .unwrap_or_else(|_| panic!("one completion returns one permit token"));
    assert_eq!(observed_capability, capability.id);
    let local_token = match permits.release(old_token.into()) {
        PermitReleaseDisposition::Released {
            next: Some(PermitGrant::Direct(token)),
            ..
        } => {
            assert_eq!(token.request(), local);
            token
        }
        other => panic!("expected FIFO Local handoff, got {other:?}"),
    };
    let plan = plan_compute_exchange(&omega, vec![capability.id], RetainedPermitGrant::empty())
        .expect("completion settlement never waits for a replacement checkout");
    assert!(plan.assigned.is_empty());
    assert_eq!(plan.batch.sequential_apply_count, 1);
    assert!(matches!(
        permits.release(local_token.into()),
        PermitReleaseDisposition::Released { next: None, .. }
    ));
}

#[test]
fn model_finished_result_holds_its_worker_slot_until_the_exchange_settles_it() {
    let mut limits = ModelLimits::small();
    limits.compute_permits = 1;
    let mut omega = Omega::new(
        limits.validate().expect("one compute permit is valid"),
        ViewId(1),
        RulesId(1),
    );
    let first = Transaction::independent(1, 1, 10, 20);
    let second = Transaction::independent(2, 2, 11, 21);
    let capability = computing_verify_chain(&mut omega, first);
    assert!(matches!(
        omega.kernel_step(KernelCommand::Admit(proposal(second.clone()))),
        KernelStep::AuthorityCommit { .. }
    ));
    assert_eq!(
        omega.kernel_step(KernelCommand::FinishExecution(Completion {
            capability: capability.id,
            result: WorkResult::Verified,
        })),
        KernelStep::NoAuthorityCommit(KernelDisposition::Finished(capability.id))
    );
    assert_eq!(omega.linear.free_compute_permits, 1);
    assert_eq!(
        omega.kernel_step(KernelCommand::Checkout),
        KernelStep::NoAuthorityCommit(KernelDisposition::Idle),
        "a released semaphore token does not manufacture a second worker slot"
    );

    let mut permits =
        FairPermitScheduler::new(PermitDomain(1), 1, 1).expect("valid fair permit fixture");
    let (_, lease) = granted_retained(&mut permits, 1);
    let plan = plan_compute_exchange(&omega, vec![capability.id], retained_grant([lease]))
        .expect("settlement frees the same slot before canonical checkout");
    assert_eq!(plan.assigned.len(), 1);
    assert_eq!(plan.assigned[0].1.transaction, second.id);
    assert_eq!(plan.batch.sequential_apply_count, 2);
    assert!(matches!(
        plan.apply(&mut omega),
        ComputeExchangeApplyDisposition::Applied {
            ref assignments,
            ref unused_grants,
            ..
        } if assignments.len() == 1 && unused_grants.is_empty()
    ));
    assert_eq!(omega.linear.work.len(), 1);
    assert!(omega.linear.finished_work.is_empty());
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_completion_drain_never_waits_for_a_slow_batch_peer() {
    let mut omega = model();
    let fast = Transaction::independent(1, 1, 10, 20);
    let slow = Transaction::independent(2, 2, 11, 21);
    let fast_capability = computing_verify_chain(&mut omega, fast.clone());
    let slow_capability = computing_verify_chain(&mut omega, slow.clone());
    let mut permits =
        FairPermitScheduler::new(PermitDomain(1), 2, 2).expect("valid fair permit fixture");
    let (_, fast_lease) = granted_retained(&mut permits, 1);
    let (_, _slow_lease) = granted_retained(&mut permits, 2);

    let drain = plan_completion_drain(
        &omega,
        &permits,
        vec![ExecutionCompletion {
            permit: fast_lease,
            completion: Completion {
                capability: fast_capability.id,
                result: WorkResult::Verified,
            },
        }],
    )
    .expect("one available completion is a complete drain cut");
    assert_eq!(drain.batch.sequential_apply_count, 0);
    let released = match drain.apply(&mut omega) {
        CompletionDrainApplyDisposition::Applied {
            finished, released, ..
        } => {
            assert_eq!(finished, vec![fast_capability.id]);
            released
        }
        other => panic!("expected immediate drain progress, got {other:?}"),
    };
    let [(fast_token, observed_capability)] = released
        .try_into()
        .unwrap_or_else(|_| panic!("one fast completion returns one permit token"));
    assert_eq!(observed_capability, fast_capability.id);
    assert!(matches!(
        permits.release(fast_token.into()),
        PermitReleaseDisposition::Released { next: None, .. }
    ));
    assert!(omega.linear.finished_work.contains_key(&fast_capability.id));
    assert!(omega.linear.work.contains_key(&slow_capability.id));

    let exchange = plan_compute_exchange(
        &omega,
        vec![fast_capability.id],
        RetainedPermitGrant::empty(),
    )
    .expect("the fast completion settles while the slow worker is still active");
    assert!(matches!(
        exchange.apply(&mut omega),
        ComputeExchangeApplyDisposition::Applied { .. }
    ));
    assert!(matches!(
        omega.authority.owners[&fast.id].location,
        OwnerLocation::Retained(super::state::RetainedOwner {
            phase: super::state::RetainedPhase::Ready(_),
            ..
        })
    ));
    assert!(matches!(
        omega.authority.owners[&slow.id].location,
        OwnerLocation::Retained(super::state::RetainedOwner {
            phase: super::state::RetainedPhase::Computing(_),
            ..
        })
    ));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_completion_drain_is_canonical_across_arrival_order() {
    let mut omega = model();
    let first = computing_verify_chain(&mut omega, Transaction::independent(1, 1, 10, 20));
    let second = computing_verify_chain(&mut omega, Transaction::independent(2, 2, 11, 21));
    let mut forward_permits =
        FairPermitScheduler::new(PermitDomain(1), 2, 2).expect("valid fair permit fixture");
    let (_, forward_first) = granted_retained(&mut forward_permits, 1);
    let (_, forward_second) = granted_retained(&mut forward_permits, 2);
    let completion = |permit, capability| ExecutionCompletion {
        permit,
        completion: Completion {
            capability,
            result: WorkResult::Verified,
        },
    };
    let forward = plan_completion_drain(
        &omega,
        &forward_permits,
        vec![
            completion(forward_first, first.id),
            completion(forward_second, second.id),
        ],
    )
    .expect("forward arrival is representable");
    let mut reverse_permits =
        FairPermitScheduler::new(PermitDomain(2), 2, 2).expect("valid fair permit fixture");
    let (_, reverse_first) = granted_retained(&mut reverse_permits, 1);
    let (_, reverse_second) = granted_retained(&mut reverse_permits, 2);
    let reverse = plan_completion_drain(
        &omega,
        &reverse_permits,
        vec![
            completion(reverse_second, second.id),
            completion(reverse_first, first.id),
        ],
    )
    .expect("reverse arrival is representable");
    assert_eq!(forward.batch.planned_state(), reverse.batch.planned_state());
    assert_eq!(forward.batch.dispositions, reverse.batch.dispositions);
}

#[test]
fn model_stale_completion_drain_returns_the_exact_execution_capability() {
    let mut limits = ModelLimits::small();
    limits.compute_permits = 1;
    let mut omega = Omega::new(
        limits.validate().expect("one compute permit is valid"),
        ViewId(1),
        RulesId(1),
    );
    let capability = computing_verify_chain(&mut omega, Transaction::independent(1, 1, 10, 20));
    let mut permits =
        FairPermitScheduler::new(PermitDomain(1), 1, 1).expect("valid fair permit fixture");
    let (_, token) = granted_retained(&mut permits, 1);
    let request_id = token.request().id;
    let plan = plan_completion_drain(
        &omega,
        &permits,
        vec![ExecutionCompletion {
            permit: token,
            completion: Completion {
                capability: capability.id,
                result: WorkResult::Verified,
            },
        }],
    )
    .expect("the original completion cut is valid");
    assert!(matches!(
        omega.kernel_step(KernelCommand::Admit(proposal(Transaction::independent(
            2, 2, 11, 21,
        )))),
        KernelStep::AuthorityCommit { .. }
    ));
    let changed = omega.clone();
    let CompletionDrainApplyDisposition::Stale { completions } = plan.apply(&mut omega) else {
        panic!("the changed authority cut must return the completion token");
    };
    let [returned] = completions
        .try_into()
        .unwrap_or_else(|_| panic!("one stale completion returns one token"));
    assert_eq!(returned.permit.request().id, request_id);
    assert_eq!(returned.completion.capability, capability.id);
    assert_eq!(omega, changed);
    assert_eq!(
        permits.active_request(request_id),
        Some(PermitRequest {
            id: PermitRequestId(1),
            class: PermitClass::Retained,
        })
    );
}

#[test]
fn model_completion_drain_rejections_return_every_linear_token() {
    let completion = |permit, capability| ExecutionCompletion {
        permit,
        completion: Completion {
            capability,
            result: WorkResult::Verified,
        },
    };

    let mut omega = model();
    let first = computing_verify_chain(&mut omega, Transaction::independent(1, 1, 10, 20));
    let second = computing_verify_chain(&mut omega, Transaction::independent(2, 2, 11, 21));
    let mut permits =
        FairPermitScheduler::new(PermitDomain(1), 3, 3).expect("valid fair permit fixture");
    let (_, first_token) = granted_retained(&mut permits, 1);
    let (_, second_token) = granted_retained(&mut permits, 2);
    let (_, third_token) = granted_retained(&mut permits, 3);
    let failure = plan_completion_drain(
        &omega,
        &permits,
        vec![
            completion(first_token, first.id),
            completion(second_token, second.id),
            completion(third_token, super::state::CapabilityId(u16::MAX)),
        ],
    )
    .expect_err("a completion wave cannot exceed the worker-slot bound");
    assert_eq!(failure.error, BatchPlanError::CompletionBatchBound);
    assert_eq!(failure.completions.len(), 3);
    for execution in failure.completions {
        assert!(matches!(
            permits.release(execution.permit.into()),
            PermitReleaseDisposition::Released { next: None, .. }
        ));
    }

    let mut permits =
        FairPermitScheduler::new(PermitDomain(2), 2, 2).expect("valid fair permit fixture");
    let (_, first_token) = granted_retained(&mut permits, 1);
    let (_, second_token) = granted_retained(&mut permits, 2);
    let failure = plan_completion_drain(
        &omega,
        &permits,
        vec![
            completion(first_token, first.id),
            completion(second_token, first.id),
        ],
    )
    .expect_err("one capability cannot settle twice in one wave");
    assert_eq!(failure.error, BatchPlanError::DuplicateCapability(first.id));
    assert_eq!(failure.completions.len(), 2);
    for execution in failure.completions {
        assert!(matches!(
            permits.release(execution.permit.into()),
            PermitReleaseDisposition::Released { next: None, .. }
        ));
    }
}

#[test]
fn model_foreign_scheduler_and_plan_rejections_return_exact_linear_tokens() {
    let mut omega = model();
    let capability = computing_verify_chain(&mut omega, Transaction::independent(1, 1, 10, 20));
    let mut owner = FairPermitScheduler::new(PermitDomain(1), 1, 1).expect("valid owner scheduler");
    let foreign = FairPermitScheduler::new(PermitDomain(2), 1, 1).expect("valid foreign scheduler");
    let (_, token) = granted_retained(&mut owner, 1);
    let request = token.request().id;
    let failure = plan_completion_drain(
        &omega,
        &foreign,
        vec![ExecutionCompletion {
            permit: token,
            completion: Completion {
                capability: capability.id,
                result: WorkResult::Verified,
            },
        }],
    )
    .expect_err("a foreign scheduler cannot validate the completion token");
    assert_eq!(failure.error, BatchPlanError::InvalidPermitToken(request));
    let [returned] = failure
        .completions
        .try_into()
        .unwrap_or_else(|_| panic!("one rejected completion returns one exact token"));
    assert!(owner.owns_retained(&returned.permit));
    assert!(matches!(
        owner.release(returned.permit.into()),
        PermitReleaseDisposition::Released { next: None, .. }
    ));

    let mut owner = FairPermitScheduler::new(PermitDomain(3), 1, 1).expect("valid owner scheduler");
    let (_, token) = granted_retained(&mut owner, 2);
    let request = token.request().id;
    let failure = plan_compute_exchange(
        &omega,
        vec![super::state::CapabilityId(u16::MAX)],
        retained_grant([token]),
    )
    .expect_err("missing finished work is a mutation-free plan rejection");
    assert_eq!(
        failure.error,
        BatchPlanError::MissingFinishedCapability(super::state::CapabilityId(u16::MAX))
    );
    assert_eq!(failure.finished, vec![super::state::CapabilityId(u16::MAX)]);
    let [token] = failure
        .grants
        .into_tokens()
        .try_into()
        .unwrap_or_else(|_| panic!("one rejected exchange returns one exact grant"));
    assert_eq!(token.request().id, request);
    assert!(matches!(
        owner.release(token.into()),
        PermitReleaseDisposition::Released { next: None, .. }
    ));
}

#[test]
fn model_foreign_release_and_mixed_domain_batch_rejection_return_exact_tokens() {
    let mut first = FairPermitScheduler::new(PermitDomain(1), 1, 1).expect("valid first scheduler");
    let mut second =
        FairPermitScheduler::new(PermitDomain(2), 1, 1).expect("valid second scheduler");
    let (_, first_token) = granted_retained(&mut first, 1);
    let (_, second_token) = granted_retained(&mut second, 2);
    let first_token = match second.release(first_token.into()) {
        PermitReleaseDisposition::Rejected {
            error: PermitReleaseError::ForeignOrStale(request),
            grant: PermitGrant::Retained(token),
        } => {
            assert_eq!(request, token.request().id);
            token
        }
        other => panic!("a foreign release must return the exact token, got {other:?}"),
    };
    assert!(second.owns_retained(&second_token));
    assert_eq!(second.check_invariants(), Ok(()));
    assert!(first.owns_retained(&first_token));

    let error = RetainedPermitGrant::try_from_tokens([second_token, first_token])
        .expect_err("one compute batch cannot span scheduler domains");
    assert_eq!(
        error.kind,
        RetainedPermitGrantErrorKind::MixedDomains {
            expected: PermitDomain(1),
            observed: PermitDomain(2),
        }
    );
    let mut tokens = error.into_tokens();
    assert_eq!(tokens.len(), 2, "the rejection returns both exact tokens");
    for token in tokens.drain(..) {
        match token.identity().0 {
            PermitDomain(1) => assert!(matches!(
                first.release(token.into()),
                PermitReleaseDisposition::Released { next: None, .. }
            )),
            PermitDomain(2) => assert!(matches!(
                second.release(token.into()),
                PermitReleaseDisposition::Released { next: None, .. }
            )),
            domain => panic!("unexpected scheduler domain {domain:?}"),
        }
    }
    assert_eq!(first.check_invariants(), Ok(()));
    assert_eq!(second.check_invariants(), Ok(()));
}

#[test]
fn model_duplicate_permit_identity_is_rejected_without_token_loss() {
    // Deliberately remove the unique-domain startup premise: two schedulers
    // share a domain and grant the same request identity. The batch boundary
    // must reject the alias instead of silently consuming either affine token.
    let mut first = FairPermitScheduler::new(PermitDomain(7), 1, 1).expect("valid first scheduler");
    let mut second =
        FairPermitScheduler::new(PermitDomain(7), 1, 1).expect("valid second scheduler");
    let (_, first_token) = granted_retained(&mut first, 1);
    let (_, second_token) = granted_retained(&mut second, 1);
    let duplicate_request = first_token.request().id;
    assert_eq!(duplicate_request, second_token.request().id);

    let error = RetainedPermitGrant::try_from_tokens([second_token, first_token])
        .expect_err("duplicate scheduler identities cannot enter one grant batch");
    assert_eq!(
        error.kind,
        RetainedPermitGrantErrorKind::DuplicateIdentity {
            request: duplicate_request,
        }
    );
    let mut tokens = error.into_tokens().into_iter();
    let first_token = tokens.next().expect("the first token is returned");
    let second_token = tokens.next().expect("the second token is returned");
    assert!(tokens.next().is_none());
    assert!(matches!(
        first.release(first_token.into()),
        PermitReleaseDisposition::Released { next: None, .. }
    ));
    assert!(matches!(
        second.release(second_token.into()),
        PermitReleaseDisposition::Released { next: None, .. }
    ));
    assert_eq!(first.check_invariants(), Ok(()));
    assert_eq!(second.check_invariants(), Ok(()));
}

#[test]
fn model_retained_acquirer_queues_once_then_only_fills_immediately_available_slots() {
    let mut scheduler =
        FairPermitScheduler::new(PermitDomain(1), 1, 4).expect("valid fair permit fixture");
    let (_, running) = granted_retained(&mut scheduler, 1);
    let local = PermitRequest {
        id: PermitRequestId(2),
        class: PermitClass::Direct,
    };
    assert_eq!(
        scheduler.request(local),
        PermitRequestDisposition::Queued(local.id)
    );

    let mut acquirer = RetainedPermitAcquirer::default();
    assert_eq!(
        acquirer.acquire(
            &mut scheduler,
            PermitRequestId(3),
            [PermitRequestId(4), PermitRequestId(5)],
        ),
        RetainedAcquireDisposition::Waiting(PermitRequestId(3))
    );
    assert_eq!(scheduler.waiting_position(local.id), Some(0));
    assert_eq!(scheduler.waiting_position(PermitRequestId(3)), Some(1));
    assert_eq!(scheduler.waiting_position(PermitRequestId(4)), None);
    assert_eq!(
        acquirer.acquire(&mut scheduler, PermitRequestId(4), [PermitRequestId(5)]),
        RetainedAcquireDisposition::Busy(PermitRequestId(3)),
        "a second coordinator waiter cannot be created while the first is pending"
    );

    let local_token = match scheduler.release(running.into()) {
        PermitReleaseDisposition::Released {
            next: Some(PermitGrant::Direct(token)),
            ..
        } => {
            assert_eq!(token.request(), local);
            token
        }
        other => panic!("expected the older Local waiter, got {other:?}"),
    };
    assert_eq!(
        acquirer.resume(&mut scheduler, None, [PermitRequestId(4)]),
        RetainedAcquireDisposition::Waiting(PermitRequestId(3))
    );
    let coordinator_token = match scheduler.release(local_token.into()) {
        PermitReleaseDisposition::Released {
            next: Some(PermitGrant::Retained(token)),
            ..
        } => {
            assert_eq!(token.request().id, PermitRequestId(3));
            token
        }
        other => panic!("expected the single coordinator waiter, got {other:?}"),
    };
    let acquired = acquirer.resume(
        &mut scheduler,
        Some(coordinator_token.into()),
        [PermitRequestId(4), PermitRequestId(5)],
    );
    let RetainedAcquireDisposition::Granted { grants, stopped_by } = acquired else {
        panic!("the delivered coordinator permit must form one bounded grant");
    };
    assert_eq!(
        stopped_by,
        Some(RetainedAcquireStop::NoImmediatePermit(PermitRequestId(4)))
    );
    let [coordinator_token] = grants
        .into_tokens()
        .try_into()
        .unwrap_or_else(|_| panic!("one delivered permit yields one grant"));
    assert!(matches!(
        scheduler.release(coordinator_token.into()),
        PermitReleaseDisposition::Released { next: None, .. }
    ));
    assert_eq!(scheduler.waiting_position(PermitRequestId(4)), None);
    assert_eq!(scheduler.waiting_position(PermitRequestId(5)), None);
    assert_eq!(scheduler.check_invariants(), Ok(()));
}
