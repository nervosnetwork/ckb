use super::{
    composition::{ComputeExchangeApplyDisposition, RetainedPermitGrant, plan_compute_exchange},
    kernel::{Admission, Completion, KernelCommand, KernelDisposition, KernelStep, WorkResult},
    permit::{
        FairPermitScheduler, PermitClass, PermitDomain, PermitGrant, PermitReleaseDisposition,
        PermitRequest, PermitRequestDisposition, PermitRequestId, RetainedWorkerGrantBatch,
        RetainedWorkerRole, RetainedWorkerSlot, WorkerGrantBatchErrorKind, WorkerSlotId,
    },
    scheduler_quotient::{SchedulerOwner, SchedulerQuotient, SchedulerVerifyOrder},
    state::{
        ModelLimits, MonotonicTick, Omega, PeerId, RemoteDeadline, RemoteResidency,
        ResolvedEvidence, RetainedSource, RulesId, Transaction, VerifyCapability, VerifyCycleClass,
        ViewId, WorkPermit,
    },
};

fn model(compute_permits: u16) -> Omega {
    let mut limits = ModelLimits::small();
    limits.compute_permits = compute_permits;
    Omega::new(
        limits
            .validate()
            .expect("the scheduler fixture uses valid bounded limits"),
        ViewId(1),
        RulesId(1),
    )
}

fn proposal(transaction: Transaction) -> Admission {
    Admission {
        transaction,
        source: RetainedSource::Proposal,
        observed_at: MonotonicTick(1),
    }
}

fn remote(transaction: Transaction, peer: u8) -> Admission {
    Admission {
        transaction,
        source: RetainedSource::Remote(RemoteResidency {
            peer: PeerId(peer),
            expires_at: RemoteDeadline(100),
        }),
        observed_at: MonotonicTick(1),
    }
}

fn admit(omega: &mut Omega, admission: Admission) {
    assert!(matches!(
        omega.kernel_step(KernelCommand::Admit(admission)),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::Retained(_),
            ..
        }
    ));
}

fn queue_verify_wave(omega: &mut Omega, admissions: Vec<(Admission, VerifyCycleClass)>) {
    let mut resolving = Vec::with_capacity(admissions.len());
    for (admission, verify_class) in admissions {
        let transaction = admission.transaction.clone();
        admit(omega, admission);
        let capability = match omega.kernel_step(KernelCommand::Checkout) {
            KernelStep::AuthorityCommit {
                disposition: KernelDisposition::CheckedOut(capability),
                ..
            } => capability,
            other => panic!("expected exact resolve checkout, got {other:?}"),
        };
        assert_eq!(capability.transaction, transaction.id);
        resolving.push((transaction, capability, verify_class));
    }
    for (transaction, capability, verify_class) in resolving {
        let evidence = ResolvedEvidence::for_transaction(
            &transaction,
            omega.authority.chain,
            omega.authority.rules,
        )
        .expect("direct transaction has no dep-group expansion")
        .with_verify_class(verify_class);
        assert!(matches!(
            omega.kernel_step(KernelCommand::Complete(Completion {
                capability: capability.id,
                result: WorkResult::Resolved(evidence),
            })),
            KernelStep::AuthorityCommit {
                disposition: KernelDisposition::Continued(id),
                ..
            } if id == transaction.id
        ));
    }
}

fn grant_batch(
    scheduler: &mut FairPermitScheduler,
    slots: Vec<RetainedWorkerSlot>,
) -> RetainedWorkerGrantBatch {
    grant_batch_from(scheduler, slots, 1)
}

fn grant_batch_from(
    scheduler: &mut FairPermitScheduler,
    slots: Vec<RetainedWorkerSlot>,
    first_request: u8,
) -> RetainedWorkerGrantBatch {
    let tokens = (0..slots.len())
        .map(|index| {
            let request = PermitRequest {
                id: PermitRequestId(
                    first_request
                        .checked_add(
                            u8::try_from(index).expect("the bounded request offset fits u8"),
                        )
                        .expect("the bounded request id does not overflow"),
                ),
                class: PermitClass::Retained,
            };
            match scheduler.request(request) {
                PermitRequestDisposition::Granted {
                    grant: PermitGrant::Retained(token),
                } => token,
                other => panic!("expected retained grant, got {other:?}"),
            }
        })
        .collect::<Vec<_>>();
    let permits = RetainedPermitGrant::try_from_tokens(tokens)
        .unwrap_or_else(|error| panic!("one scheduler owns the grant batch: {error:?}"));
    RetainedWorkerGrantBatch::bind(permits, slots)
        .unwrap_or_else(|error| panic!("the worker topology is valid: {error:?}"))
}

#[test]
fn model_scheduler_wave_binds_canonical_worker_roles_to_distinct_fair_owners() {
    let mut omega = model(3);
    let trusted = Transaction::independent(1, 1, 10, 20);
    let remote_a_first = Transaction::independent(2, 2, 11, 21);
    let remote_a_second = Transaction::independent(3, 3, 12, 22);
    let remote_b = Transaction::independent(4, 4, 13, 23);
    admit(&mut omega, proposal(trusted.clone()));
    admit(&mut omega, remote(remote_a_first.clone(), 1));
    admit(&mut omega, remote(remote_a_second, 1));
    admit(&mut omega, remote(remote_b.clone(), 2));

    let mut permits =
        FairPermitScheduler::new(PermitDomain(1), 3, 3).expect("three retained permits are valid");
    let grants = grant_batch(
        &mut permits,
        vec![
            RetainedWorkerSlot::new(
                WorkerSlotId(8),
                RetainedWorkerRole::Verifier(VerifyCapability::Any),
            ),
            RetainedWorkerSlot::new(WorkerSlotId(3), RetainedWorkerRole::OrderedResolve),
            RetainedWorkerSlot::new(
                WorkerSlotId(5),
                RetainedWorkerRole::Verifier(VerifyCapability::SmallCycleOnly),
            ),
        ],
    );
    let mut scheduler = SchedulerQuotient::default();
    let wave = scheduler.plan_wave(&omega, grants);
    let observed = wave
        .assignments()
        .iter()
        .map(|(grant, assignment)| {
            (
                grant.slot().role(),
                assignment.transaction(),
                assignment.permit(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        vec![
            (
                RetainedWorkerRole::OrderedResolve,
                trusted.id,
                WorkPermit::ResolveOnly,
            ),
            (
                RetainedWorkerRole::Verifier(VerifyCapability::SmallCycleOnly),
                remote_a_first.id,
                WorkPermit::ResolveThenVerify(VerifyCapability::SmallCycleOnly),
            ),
            (
                RetainedWorkerRole::Verifier(VerifyCapability::Any),
                remote_b.id,
                WorkPermit::ResolveThenVerify(VerifyCapability::Any),
            ),
        ]
    );

    let (cursor, assigned, idle) = wave.into_parts();
    assert!(idle.is_empty());
    assert!(cursor.apply(&mut scheduler));
    assert_eq!(
        scheduler.cursors(),
        (Some(SchedulerOwner::Remote(PeerId(2))), None)
    );
    for (grant, _) in assigned {
        assert!(matches!(
            permits.release(grant.into_permit().into()),
            PermitReleaseDisposition::Released { next: None, .. }
        ));
    }
}

#[test]
fn model_scheduler_wave_preserves_verify_capability_and_configured_order() {
    let mut omega = model(2);
    let small = Transaction::independent(1, 1, 10, 20);
    let large = Transaction::independent(2, 2, 11, 21);
    queue_verify_wave(
        &mut omega,
        vec![
            (remote(small.clone(), 1), VerifyCycleClass::Small),
            (remote(large.clone(), 2), VerifyCycleClass::Large),
        ],
    );

    let mut permits =
        FairPermitScheduler::new(PermitDomain(1), 2, 2).expect("two retained permits are valid");
    let grants = grant_batch(
        &mut permits,
        vec![
            RetainedWorkerSlot::new(
                WorkerSlotId(2),
                RetainedWorkerRole::Verifier(VerifyCapability::Any),
            ),
            RetainedWorkerSlot::new(
                WorkerSlotId(1),
                RetainedWorkerRole::Verifier(VerifyCapability::SmallCycleOnly),
            ),
        ],
    );
    let wave = SchedulerQuotient::default().plan_wave(&omega, grants);
    let observed = wave
        .assignments()
        .iter()
        .map(|(grant, assignment)| {
            (
                grant.slot().role(),
                assignment.transaction(),
                assignment.permit(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        vec![
            (
                RetainedWorkerRole::Verifier(VerifyCapability::SmallCycleOnly),
                small.id,
                WorkPermit::VerifyOnly(VerifyCapability::SmallCycleOnly),
            ),
            (
                RetainedWorkerRole::Verifier(VerifyCapability::Any),
                large.id,
                WorkPermit::VerifyOnly(VerifyCapability::Any),
            ),
        ]
    );
    let (_, assigned, idle) = wave.into_parts();
    assert!(idle.is_empty());
    for (grant, _) in assigned {
        assert!(matches!(
            permits.release(grant.into_permit().into()),
            PermitReleaseDisposition::Released { next: None, .. }
        ));
    }

    let mut same_owner = model(2);
    let earlier = Transaction::independent(3, 3, 12, 22).with_fee(10);
    let later = Transaction::independent(4, 4, 13, 23).with_fee(100);
    queue_verify_wave(
        &mut same_owner,
        vec![
            (remote(earlier.clone(), 1), VerifyCycleClass::Small),
            (remote(later.clone(), 1), VerifyCycleClass::Small),
        ],
    );
    let selected = |order| {
        let mut permits = FairPermitScheduler::new(PermitDomain(order), 1, 1)
            .expect("one retained permit is valid");
        let grants = grant_batch(
            &mut permits,
            vec![RetainedWorkerSlot::new(
                WorkerSlotId(1),
                RetainedWorkerRole::Verifier(VerifyCapability::Any),
            )],
        );
        let wave = SchedulerQuotient::new(if order == 1 {
            SchedulerVerifyOrder::Arrival
        } else {
            SchedulerVerifyOrder::FeeRate
        })
        .plan_wave(&same_owner, grants);
        wave.assignments()[0].1.transaction()
    };
    assert_eq!(selected(1), earlier.id);
    assert_eq!(selected(2), later.id);
}

#[test]
fn model_scheduler_small_capability_excludes_large_checkout_equivalence_premise() {
    let mut transaction_id = 20u8;
    for capability in [VerifyCapability::SmallCycleOnly, VerifyCapability::Any] {
        for class in [VerifyCycleClass::Small, VerifyCycleClass::Large] {
            transaction_id += 1;
            let mut omega = model(1);
            let transaction = Transaction::independent(transaction_id, 1, 10, 20);
            queue_verify_wave(&mut omega, vec![(remote(transaction.clone(), 1), class)]);

            let mut permits = FairPermitScheduler::new(PermitDomain(1), 1, 1)
                .expect("one retained permit is valid");
            let grants = grant_batch(
                &mut permits,
                vec![RetainedWorkerSlot::new(
                    WorkerSlotId(1),
                    RetainedWorkerRole::Verifier(capability),
                )],
            );
            let wave = SchedulerQuotient::default().plan_wave(&omega, grants);
            let (_, assigned, idle) = wave.into_parts();
            assert_eq!(
                assigned.len(),
                usize::from(capability.permits(class)),
                "capability={capability:?}, class={class:?}"
            );
            assert_eq!(
                idle.len(),
                usize::from(!capability.permits(class)),
                "capability={capability:?}, class={class:?}"
            );
            if let Some((grant, assignment)) = assigned.into_iter().next() {
                assert_eq!(assignment.transaction(), transaction.id);
                assert_eq!(assignment.permit(), WorkPermit::VerifyOnly(capability));
                assert!(matches!(
                    permits.release(grant.into_permit().into()),
                    PermitReleaseDisposition::Released { next: None, .. }
                ));
            }
            if let Some(grant) = idle.into_iter().next() {
                assert!(matches!(
                    permits.release(grant.into_permit().into()),
                    PermitReleaseDisposition::Released { next: None, .. }
                ));
            }
        }
    }
}

#[test]
fn model_worker_grant_binding_rejects_ambiguous_topology_without_losing_resources() {
    let check = |domain, slots: Vec<RetainedWorkerSlot>, expected| {
        let permit_count = if matches!(expected, WorkerGrantBatchErrorKind::CountMismatch { .. }) {
            1
        } else {
            slots.len()
        };
        let permit_count_u16 =
            u16::try_from(permit_count).expect("the bounded permit count fits u16");
        let mut permits =
            FairPermitScheduler::new(PermitDomain(domain), permit_count_u16, permit_count_u16)
                .expect("the bounded permit scheduler is valid");
        let tokens = (0..permit_count)
            .map(|index| {
                let request = PermitRequest {
                    id: PermitRequestId(u8::try_from(index + 1).expect("the request id fits u8")),
                    class: PermitClass::Retained,
                };
                match permits.request(request) {
                    PermitRequestDisposition::Granted {
                        grant: PermitGrant::Retained(token),
                    } => token,
                    other => panic!("expected retained grant, got {other:?}"),
                }
            })
            .collect::<Vec<_>>();
        let grant = RetainedPermitGrant::try_from_tokens(tokens)
            .unwrap_or_else(|error| panic!("one scheduler owns every permit: {error:?}"));
        let error = RetainedWorkerGrantBatch::bind(grant, slots)
            .expect_err("the ambiguous worker topology must be rejected");
        assert_eq!(error.kind, expected);
        let (grant, returned_slots) = error.into_parts();
        assert_eq!(returned_slots.len(), if permit_count == 1 { 0 } else { 2 });
        for token in grant.into_tokens() {
            assert!(matches!(
                permits.release(token.into()),
                PermitReleaseDisposition::Released { next: None, .. }
            ));
        }
    };

    check(
        1,
        Vec::new(),
        WorkerGrantBatchErrorKind::CountMismatch {
            permits: 1,
            slots: 0,
        },
    );
    check(
        2,
        vec![
            RetainedWorkerSlot::new(
                WorkerSlotId(1),
                RetainedWorkerRole::Verifier(VerifyCapability::Any),
            ),
            RetainedWorkerSlot::new(
                WorkerSlotId(1),
                RetainedWorkerRole::Verifier(VerifyCapability::SmallCycleOnly),
            ),
        ],
        WorkerGrantBatchErrorKind::DuplicateWorkerSlot(WorkerSlotId(1)),
    );
    check(
        3,
        vec![
            RetainedWorkerSlot::new(WorkerSlotId(1), RetainedWorkerRole::OrderedResolve),
            RetainedWorkerSlot::new(WorkerSlotId(2), RetainedWorkerRole::OrderedResolve),
        ],
        WorkerGrantBatchErrorKind::MultipleOrderedResolvers,
    );
}

#[test]
fn model_compute_exchange_rejects_a_stale_scheduler_cut_and_returns_its_grant() {
    let mut omega = model(1);
    let transaction = Transaction::independent(1, 1, 10, 20);
    admit(&mut omega, proposal(transaction));
    let before = omega.clone();
    let mut permits =
        FairPermitScheduler::new(PermitDomain(1), 1, 1).expect("one retained permit is valid");
    let grants = grant_batch(
        &mut permits,
        vec![RetainedWorkerSlot::new(
            WorkerSlotId(1),
            RetainedWorkerRole::OrderedResolve,
        )],
    );
    let expected_scheduler = SchedulerQuotient::default();
    let plan = plan_compute_exchange(&omega, &expected_scheduler, Vec::new(), grants)
        .expect("the original authority and scheduler cuts plan");
    let mut changed_scheduler = SchedulerQuotient::new(SchedulerVerifyOrder::FeeRate);
    let ComputeExchangeApplyDisposition::Stale { grants } =
        plan.apply(&mut omega, &mut changed_scheduler)
    else {
        panic!("a changed scheduler cut must return the exact grant");
    };
    assert_eq!(omega, before);
    let [grant] = grants
        .into_grants()
        .try_into()
        .unwrap_or_else(|_| panic!("one stale worker grant must be returned"));
    assert!(matches!(
        permits.release(grant.into_permit().into()),
        PermitReleaseDisposition::Released { next: None, .. }
    ));
}

#[test]
fn model_scheduler_cursor_persists_across_committed_compute_waves() {
    let mut omega = model(2);
    let trusted = Transaction::independent(1, 1, 10, 20);
    let remote_tx = Transaction::independent(2, 2, 11, 21);
    admit(&mut omega, proposal(trusted.clone()));
    admit(&mut omega, remote(remote_tx.clone(), 1));
    let mut permits =
        FairPermitScheduler::new(PermitDomain(1), 2, 2).expect("two retained permits are valid");
    let mut scheduler = SchedulerQuotient::default();

    let first = plan_compute_exchange(
        &omega,
        &scheduler,
        Vec::new(),
        grant_batch(
            &mut permits,
            vec![RetainedWorkerSlot::new(
                WorkerSlotId(1),
                RetainedWorkerRole::OrderedResolve,
            )],
        ),
    )
    .expect("the first wave plans");
    let first_assignments = match first.apply(&mut omega, &mut scheduler) {
        ComputeExchangeApplyDisposition::Applied { assignments, .. } => assignments,
        other => panic!("the first wave must apply, got {other:?}"),
    };
    assert_eq!(first_assignments[0].1.transaction, trusted.id);

    let second = plan_compute_exchange(
        &omega,
        &scheduler,
        Vec::new(),
        grant_batch_from(
            &mut permits,
            vec![RetainedWorkerSlot::new(
                WorkerSlotId(2),
                RetainedWorkerRole::OrderedResolve,
            )],
            2,
        ),
    )
    .expect("the second wave plans from the committed cursor");
    let second_assignments = match second.apply(&mut omega, &mut scheduler) {
        ComputeExchangeApplyDisposition::Applied { assignments, .. } => assignments,
        other => panic!("the second wave must apply, got {other:?}"),
    };
    assert_eq!(second_assignments[0].1.transaction, remote_tx.id);
    assert_eq!(
        scheduler.cursors(),
        (Some(SchedulerOwner::Remote(PeerId(1))), None)
    );
    assert_eq!(omega.check_invariants(), Ok(()));
}
