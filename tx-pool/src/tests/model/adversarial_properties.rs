use super::{
    adversarial::{
        AdversarialShape, HostileAction, HostileTraceGenerator, HostileTraceLimits, HostileTxKey,
        M2RootPremise, PremiseViolation, QuantitativeInput, QuantitativeLimits,
        WorkAmplificationAudit, WorkRecordDisposition, adversarial_cohort, bounded_permutations,
        canonical_headers, canonical_proposals, expanded_key_count,
        shortest_premise_counterexample,
    },
    composition::{
        CohortClass, CouplingReason, OrderedBatchFamily, RelationKind, analyze_ready_prefix,
        plan_completion_drain, plan_ordered_batch,
    },
    kernel::{Admission, Completion, KernelCommand, KernelDisposition, KernelStep, WorkResult},
    permit::{
        FairPermitScheduler, PermitClass, PermitDomain, PermitGrant, PermitRequest,
        PermitRequestDisposition, PermitRequestId,
    },
    state::{
        ChainView, EffectClass, EvidenceContext, LogicalEffect, ModelLimits, Omega, RemoteDeadline,
        RemoteResidency, ResolvedEvidence, RetainedSource, RulesId, Transaction, TxId, ViewId,
        WitnessId, WorkKind,
    },
};

fn model() -> Omega {
    Omega::new(
        ModelLimits::small()
            .validate()
            .expect("the adversarial fixture uses valid bounded limits"),
        ViewId(1),
        RulesId(1),
    )
}

fn proposal(transaction: Transaction) -> KernelCommand {
    KernelCommand::Admit(Admission {
        transaction,
        source: RetainedSource::Proposal,
        observed_at: super::state::MonotonicTick(1),
    })
}

fn checkout(omega: &mut Omega) -> super::state::CapabilityId {
    match omega.kernel_step(KernelCommand::Checkout) {
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::CheckedOut(capability),
            ..
        } => capability.id,
        other => panic!("expected checkout, got {other:?}"),
    }
}

fn make_ready(omega: &mut Omega, transaction: Transaction) {
    assert!(matches!(
        omega.kernel_step(proposal(transaction.clone())),
        KernelStep::AuthorityCommit { .. }
    ));
    let resolve = checkout(omega);
    let evidence = ResolvedEvidence::for_transaction(
        &transaction,
        omega.authority.chain,
        omega.authority.rules,
    );
    assert!(matches!(
        omega.kernel_step(KernelCommand::Complete(Completion {
            capability: resolve,
            result: WorkResult::Resolved(evidence),
        })),
        KernelStep::AuthorityCommit { .. }
    ));
    let verify = checkout(omega);
    assert!(matches!(
        omega.kernel_step(KernelCommand::Complete(Completion {
            capability: verify,
            result: WorkResult::Verified,
        })),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::Ready(_),
            ..
        }
    ));
}

fn accept(omega: &mut Omega, transaction: Transaction) {
    make_ready(omega, transaction);
    assert!(matches!(
        omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 1 }),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::Accepted(_) | KernelDisposition::AcceptedBatch(_),
            ..
        }
    ));
}

#[test]
fn model_generated_cell_shapes_route_only_the_exact_independent_prefix() {
    let cases = [
        (AdversarialShape::Independent(2), 2, None),
        (
            AdversarialShape::SharedInput(2),
            1,
            Some(RelationKind::SharedInput),
        ),
        (AdversarialShape::SharedHeaderRead(2), 2, None),
        (
            AdversarialShape::DeepChain(2),
            1,
            Some(RelationKind::CandidateProducesInput),
        ),
        (
            AdversarialShape::ReadFanout(2),
            1,
            Some(RelationKind::CandidateProducesRead),
        ),
        (
            AdversarialShape::ConditionalReadWrite,
            1,
            Some(RelationKind::CandidateSpendsRead),
        ),
        (
            AdversarialShape::RbfReplacement,
            1,
            Some(RelationKind::SharedInput),
        ),
    ];
    for (shape, expected_prefix, relation) in cases {
        let transactions = adversarial_cohort(shape).expect("the bounded shape is constructible");
        let mut omega = model();
        for transaction in transactions.clone() {
            make_ready(&mut omega, transaction);
        }
        let analysis = analyze_ready_prefix(&omega, transactions.len());
        assert_eq!(analysis.prefix.len(), expected_prefix, "shape {shape:?}");
        match relation {
            None => assert_eq!(analysis.class, CohortClass::IndependentComposable),
            Some(expected) => assert!(
                matches!(
                    analysis.stopped_by,
                    Some(CouplingReason::CandidateRelation { kind, .. }) if kind == expected
                ),
                "shape {shape:?}: {:?}",
                analysis.stopped_by
            ),
        }
        assert!(analysis.cost.linear_key_bound().is_some());
        assert!(
            expanded_key_count(&transactions).is_some(),
            "shape accounting must remain representable"
        );
    }
}

#[test]
fn model_generated_short_id_collision_is_not_confused_with_full_identity() {
    let transactions =
        adversarial_cohort(AdversarialShape::ProposalCollision).expect("bounded collision shape");
    assert_eq!(canonical_proposals(&transactions).len(), 1);
    assert_ne!(transactions[0].id, transactions[1].id);
    assert_ne!(transactions[0].witness, transactions[1].witness);

    let mut omega = model();
    let plan = plan_ordered_batch(
        &omega,
        OrderedBatchFamily::RetainedIngress,
        transactions.into_iter().map(proposal).collect(),
    )
    .expect("the collision has a deterministic canonical ingress outcome");
    assert!(matches!(
        plan.dispositions.as_slice(),
        [
            KernelDisposition::Retained(_),
            KernelDisposition::ProposalCollision(_)
        ]
    ));
    assert!(matches!(
        plan.apply(&mut omega),
        super::composition::BatchApplyDisposition::Applied { .. }
    ));
    assert_eq!(omega.authority.owners.len(), 1);
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_shared_header_reads_are_explicit_and_commutative() {
    let transactions = adversarial_cohort(AdversarialShape::SharedHeaderRead(3))
        .expect("the header fanout is bounded");
    assert_eq!(
        canonical_headers(&transactions),
        [super::state::HeaderId(7)].into()
    );
}

#[test]
fn model_rbf_is_coupled_to_the_accepted_victim_and_never_uses_the_independent_lane() {
    let transactions = adversarial_cohort(AdversarialShape::RbfReplacement)
        .expect("the replacement pair is bounded");
    let [original, replacement] = transactions.as_slice() else {
        panic!("the replacement shape must contain exactly two members");
    };
    let mut omega = model();
    accept(&mut omega, original.clone());
    make_ready(&mut omega, replacement.clone());
    let analysis = analyze_ready_prefix(&omega, 1);
    assert!(analysis.prefix.is_empty());
    assert!(matches!(
        analysis.class,
        CohortClass::Coupled(CouplingReason::AcceptedRelation {
            candidate,
            accepted,
            kind: RelationKind::SharedInput,
            ..
        }) if candidate == replacement.id && accepted == original.id
    ));
    assert!(matches!(
        omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 2 }),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::ReplacementAccepted { winner, .. },
            ..
        } if winner == replacement.id
    ));
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_full_effect_partition_stops_ready_before_any_authority_mutation() {
    let mut omega = model();
    let transaction = Transaction::independent(1, 1, 10, 20);
    make_ready(&mut omega, transaction.clone());
    while omega.append_effect_fixture(
        EffectClass::Trusted,
        vec![LogicalEffect::IngressReleased(TxId(200))],
    ) {}
    assert_eq!(omega.check_invariants(), Ok(()));
    let before = omega.clone();
    let analysis = analyze_ready_prefix(&omega, 1);
    assert!(analysis.prefix.is_empty());
    assert!(matches!(
        analysis.class,
        CohortClass::Coupled(CouplingReason::EffectCapacity(id)) if id == transaction.id
    ));
    assert_eq!(omega, before, "classification is a read-only Plan");
}

#[test]
fn model_deep_chain_and_fanout_key_work_is_linear_at_configured_scale() {
    for width in [1u8, 8, 64, 100] {
        let chain = adversarial_cohort(AdversarialShape::DeepChain(width))
            .expect("the configured chain identifiers fit");
        assert_eq!(chain.len(), usize::from(width));
        assert_eq!(expanded_key_count(&chain), Some(u32::from(width) * 2));

        let fanout = adversarial_cohort(AdversarialShape::ReadFanout(width))
            .expect("the configured fanout identifiers fit");
        assert_eq!(fanout.len(), usize::from(width) + 1);
        assert_eq!(expanded_key_count(&fanout), Some(2 + u32::from(width) * 3));
        assert_eq!(canonical_headers(&fanout).len(), 0);
    }
}

#[test]
fn model_completion_plan_is_equal_for_every_bounded_worker_permutation() {
    let transactions = adversarial_cohort(AdversarialShape::Independent(3))
        .expect("three independent workers fit the identifier model");
    let mut limits = ModelLimits::small();
    limits.compute_permits = 3;
    limits.owners.entries = 6;
    limits.retained.entries = 6;
    let largest_batch_effects = limits
        .owners
        .entries
        .checked_add(1)
        .expect("the enlarged owner partition has a representable effect bound");
    limits.effects.remote_bound.effects = largest_batch_effects;
    limits.effects.trusted_bound.effects = largest_batch_effects;
    limits.effects.critical_bound.effects = largest_batch_effects;
    let mut omega = Omega::new(
        limits
            .validate()
            .expect("three workers fit the resource fixture"),
        ViewId(1),
        RulesId(1),
    );
    let mut capabilities = Vec::new();
    for transaction in transactions {
        assert!(matches!(
            omega.kernel_step(proposal(transaction.clone())),
            KernelStep::AuthorityCommit { .. }
        ));
        let resolve = checkout(&mut omega);
        let evidence = ResolvedEvidence::for_transaction(
            &transaction,
            omega.authority.chain,
            omega.authority.rules,
        );
        assert!(matches!(
            omega.kernel_step(KernelCommand::Complete(Completion {
                capability: resolve,
                result: WorkResult::Resolved(evidence),
            })),
            KernelStep::AuthorityCommit { .. }
        ));
        capabilities.push(checkout(&mut omega));
    }
    let permutations =
        bounded_permutations(&[0usize, 1, 2], 3).expect("3! is an admitted proof bound");
    assert_eq!(permutations.len(), 6);
    let mut expected = None;
    for permutation in permutations {
        let mut scheduler =
            FairPermitScheduler::new(PermitDomain(1), 3, 3).expect("valid permit fixture");
        let mut tokens = Vec::new();
        for id in 1..=3 {
            let request = PermitRequest {
                id: PermitRequestId(id),
                class: PermitClass::Retained,
            };
            let PermitRequestDisposition::Granted {
                grant: PermitGrant::Retained(token),
            } = scheduler.request(request)
            else {
                panic!("every worker must receive its initial permit");
            };
            tokens.push(Some(token));
        }
        let completions = permutation
            .into_iter()
            .map(|index| {
                let permit = tokens[index]
                    .take()
                    .unwrap_or_else(|| panic!("a permutation uses every token exactly once"));
                super::composition::ExecutionCompletion {
                    permit,
                    completion: Completion {
                        capability: capabilities[index],
                        result: WorkResult::Verified,
                    },
                }
            })
            .collect();
        let plan = plan_completion_drain(&omega, &scheduler, completions)
            .expect("every arrival permutation is canonicalized");
        let observation = (
            plan.batch.planned_state().clone(),
            plan.batch.dispositions.clone(),
        );
        if let Some(expected) = &expected {
            assert_eq!(&observation, expected);
        } else {
            expected = Some(observation);
        }
    }
}

#[test]
fn model_quantitative_equation_separates_linear_work_from_core_wave_applies() {
    let limits = QuantitativeLimits {
        mutation_batch: 100,
        worker_slots: 64,
        external_records: 512,
        external_bytes: 1 << 20,
    };
    let input = QuantitativeInput {
        ingress_items: 100,
        completions: 64,
        grants: 64,
        ready_items: 64,
        accepted_owners_scanned: 1_000,
        accepted_edges_scanned: 3_000,
        cell_keys: 256,
        header_keys: 64,
        pool_edges: 0,
        index_operations: 320,
        candidate_scratch_entries: 320,
        coupled_members: 100,
        coupled_edges: 400,
        wake_edges: 400,
        stale_capabilities: 64,
        effect_records: 100,
        effect_batches: 25,
        effect_bytes: 20_000,
        relay_records: 100,
        relay_bytes: 40_000,
        detached_endpoint_calls: 4,
        detached_endpoint_bytes: 4_000,
    };
    let bound = input
        .compile(limits)
        .expect("the configured-scale bound fits");
    assert_eq!(bound.transient_items, 292);
    assert_eq!(bound.key_edge_operations, 3_640);
    assert_eq!(bound.component_operations, 500);
    assert_eq!(bound.scratch_entries, 5_112);
    assert_eq!(bound.linear_drain_steps, 64);
    assert_eq!(bound.completion_order_items, 64);
    assert_eq!(bound.completion_pair_space, 2_016);
    assert_eq!(bound.core_authority_applies, 3);
    assert_eq!(bound.effect_settlement_applies, 25);
    assert_eq!(bound.authority_apply_upper, 28);
    assert_eq!(bound.wake_operations, 400);
    assert_eq!(bound.stale_retirements, 64);
    assert_eq!(bound.external_backlog_records, 204);
    assert_eq!(bound.external_backlog_bytes, 64_000);

    let half = QuantitativeInput {
        ingress_items: 50,
        completions: 32,
        grants: 32,
        ready_items: 32,
        accepted_owners_scanned: 500,
        accepted_edges_scanned: 1_500,
        cell_keys: 128,
        header_keys: 32,
        index_operations: 160,
        candidate_scratch_entries: 160,
        coupled_members: 50,
        coupled_edges: 200,
        wake_edges: 200,
        stale_capabilities: 32,
        effect_records: 50,
        effect_batches: 13,
        effect_bytes: 10_000,
        relay_records: 50,
        relay_bytes: 20_000,
        detached_endpoint_calls: 2,
        detached_endpoint_bytes: 2_000,
        ..QuantitativeInput::default()
    }
    .compile(limits)
    .expect("the half-scale bound fits");
    assert_eq!(bound.transient_items, half.transient_items * 2);
    assert_eq!(bound.key_edge_operations, half.key_edge_operations * 2);
    assert_eq!(bound.component_operations, half.component_operations * 2);
    assert_eq!(bound.linear_drain_steps, half.linear_drain_steps * 2);
    assert_eq!(
        bound.completion_order_items,
        half.completion_order_items * 2
    );
    assert_eq!(bound.core_authority_applies, half.core_authority_applies);
    assert_eq!(bound.effect_settlement_applies, 25);
    assert_eq!(half.effect_settlement_applies, 13);
    assert_eq!(bound.authority_apply_upper, 28);
    assert_eq!(half.authority_apply_upper, 16);
}

#[test]
fn model_ready_composition_cost_is_consumed_without_a_hand_copied_projection() {
    let mut omega = model();
    for transaction in adversarial_cohort(AdversarialShape::Independent(2))
        .expect("the independent composition is bounded")
    {
        make_ready(&mut omega, transaction);
    }
    let analysis = analyze_ready_prefix(&omega, 2);
    assert_eq!(analysis.prefix.len(), 2);
    let input = QuantitativeInput {
        effect_records: 2,
        effect_batches: 1,
        effect_bytes: 32,
        ..QuantitativeInput::default()
    }
    .with_ready_composition(analysis.cost);
    let bound = input
        .compile(QuantitativeLimits {
            mutation_batch: 2,
            worker_slots: 2,
            external_records: 2,
            external_bytes: 32,
        })
        .expect("the exact composition cost fits its declared bounds");
    assert_eq!(bound.transient_items, 2);
    assert_eq!(
        bound.key_edge_operations,
        u64::from(
            analysis
                .cost
                .linear_key_bound()
                .expect("the key count is representable")
                + analysis.cost.index_operations
        )
    );
    assert_eq!(bound.core_authority_applies, 1);
    assert_eq!(bound.effect_settlement_applies, 1);
    assert_eq!(bound.authority_apply_upper, 2);
}

#[test]
fn model_quantitative_equation_rejects_every_configured_bound_overrun() {
    let limits = QuantitativeLimits {
        mutation_batch: 2,
        worker_slots: 2,
        external_records: 4,
        external_bytes: 64,
    };
    for input in [
        QuantitativeInput {
            ingress_items: 3,
            ..QuantitativeInput::default()
        },
        QuantitativeInput {
            completions: 3,
            ..QuantitativeInput::default()
        },
        QuantitativeInput {
            ready_items: 3,
            ..QuantitativeInput::default()
        },
        QuantitativeInput {
            stale_capabilities: 3,
            ..QuantitativeInput::default()
        },
        QuantitativeInput {
            effect_records: 5,
            effect_batches: 5,
            ..QuantitativeInput::default()
        },
        QuantitativeInput {
            effect_records: 2,
            effect_batches: 3,
            ..QuantitativeInput::default()
        },
        QuantitativeInput {
            effect_records: 1,
            ..QuantitativeInput::default()
        },
        QuantitativeInput {
            relay_bytes: 65,
            ..QuantitativeInput::default()
        },
    ] {
        assert_eq!(input.compile(limits), None, "input {input:?}");
    }
}

#[test]
fn model_ready_batch_bound_is_independent_of_the_current_worker_wave_width() {
    let bound = QuantitativeInput {
        ready_items: 4,
        effect_records: 4,
        effect_batches: 1,
        ..QuantitativeInput::default()
    }
    .compile(QuantitativeLimits {
        mutation_batch: 4,
        worker_slots: 2,
        external_records: 4,
        external_bytes: 0,
    })
    .expect("verified Ready backlog can span multiple worker waves");
    assert_eq!(bound.core_authority_applies, 1);
    assert_eq!(bound.effect_settlement_applies, 1);
}

#[test]
fn model_current_retained_path_separates_per_item_from_ready_batch_applies() {
    use super::adversarial::{CurrentRetainedPathCost, CurrentRetainedPathInput};

    let ready_batch_limit = u32::try_from(crate::constants::MAX_READY_BATCH)
        .expect("production Ready batch limit fits the model domain");
    assert_eq!(
        CurrentRetainedPathInput {
            items: 1,
            ready_applies: 1,
            ready_batch_limit,
        }
        .compile(),
        Some(CurrentRetainedPathCost {
            admission_applies: 1,
            checkout_applies: 1,
            completion_applies: 1,
            membership_applies: 1,
            effect_settlement_applies: 1,
            total_applies: 5,
        })
    );

    let fully_coalesced = CurrentRetainedPathInput {
        items: ready_batch_limit,
        ready_applies: 1,
        ready_batch_limit,
    }
    .compile()
    .expect("one full Ready slice is representable");
    assert_eq!(
        fully_coalesced.total_applies,
        ready_batch_limit
            .checked_mul(3)
            .and_then(|value| value.checked_add(2))
            .expect("production Ready batch limit has a representable cost")
    );

    let prompt = CurrentRetainedPathInput {
        items: ready_batch_limit,
        ready_applies: ready_batch_limit,
        ready_batch_limit,
    }
    .compile()
    .expect("one prompt Ready slice per owner is representable");
    assert_eq!(
        prompt.total_applies,
        ready_batch_limit
            .checked_mul(5)
            .expect("production Ready batch limit has a representable prompt cost")
    );

    assert_eq!(
        CurrentRetainedPathInput {
            items: ready_batch_limit + 1,
            ready_applies: 1,
            ready_batch_limit,
        }
        .compile(),
        None,
        "one Apply cannot exceed the current Ready batch limit"
    );
    assert_eq!(
        CurrentRetainedPathInput {
            items: 2,
            ready_applies: 3,
            ready_batch_limit,
        }
        .compile(),
        None,
        "a non-empty Ready Apply cannot outnumber its owners"
    );
}

#[test]
fn model_same_cut_revalidation_is_no_progress_but_new_revision_is_new_evidence() {
    let mut audit = WorkAmplificationAudit::new(2, 3).expect("non-zero audit bounds");
    let context = EvidenceContext {
        chain: ChainView::initial(ViewId(1)),
        rules: RulesId(1),
        witness: WitnessId(1),
    };
    assert_eq!(
        audit.record(TxId(1), context, WorkKind::Resolve),
        WorkRecordDisposition::Recorded
    );
    assert_eq!(
        audit.record(TxId(1), context, WorkKind::Resolve),
        WorkRecordDisposition::DuplicateCut
    );
    assert_eq!(
        audit.record(TxId(1), context, WorkKind::Verify),
        WorkRecordDisposition::Recorded
    );
    let later = EvidenceContext {
        chain: context
            .chain
            .advance(context.chain.tip)
            .expect("one revision remains"),
        ..context
    };
    assert_eq!(
        audit.record(TxId(1), later, WorkKind::Resolve),
        WorkRecordDisposition::Recorded
    );
    assert_eq!(
        audit.record(TxId(1), later, WorkKind::Verify),
        WorkRecordDisposition::EvidenceCutBound
    );
    assert_eq!(audit.total_attempts(), 3);
}

#[test]
fn model_every_m2_root_premise_has_a_typed_minimum_counterexample() {
    let counterexamples = M2RootPremise::ALL
        .into_iter()
        .map(|premise| {
            shortest_premise_counterexample(premise)
                .expect("every declared M2 premise must own a counterexample")
        })
        .collect::<Vec<_>>();
    assert_eq!(counterexamples.len(), M2RootPremise::ALL.len());
    assert_eq!(
        counterexamples
            .iter()
            .map(|counterexample| counterexample.premise)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        M2RootPremise::ALL.len()
    );
    for counterexample in counterexamples {
        match counterexample.violation {
            PremiseViolation::ProposalAlias { first, second, .. } => {
                assert_ne!(first, second);
                assert_eq!(counterexample.semantic_members, 2);
            }
            PremiseViolation::SharedInput { first, second, .. } => {
                assert_ne!(first, second);
                assert_eq!(counterexample.semantic_members, 2);
            }
            PremiseViolation::CausalCell {
                producer, consumer, ..
            } => {
                assert_ne!(producer, consumer);
                assert_eq!(counterexample.semantic_members, 2);
            }
            PremiseViolation::ReadSpend {
                reader, spender, ..
            } => {
                assert_ne!(reader, spender);
                assert_eq!(counterexample.semantic_members, 2);
            }
            PremiseViolation::PoolOrigin { parent, child, .. } => {
                assert_ne!(parent, child);
                assert_eq!(counterexample.semantic_members, 2);
            }
            PremiseViolation::StaleCut { planned, current } => {
                assert!(planned < current);
                assert_eq!(counterexample.semantic_members, 1);
            }
            PremiseViolation::WorkerSlotOverrun {
                slots,
                executing,
                finished,
            } => {
                assert!(executing + finished > slots);
                assert_eq!(counterexample.semantic_members, 2);
            }
            PremiseViolation::ResourceOverrun { limit, observed }
            | PremiseViolation::DetachedCallOverrun { limit, observed } => {
                assert!(observed > limit);
            }
            PremiseViolation::FairnessInversion {
                older_ticket,
                selected_newer_ticket,
            } => {
                assert!(older_ticket < selected_newer_ticket);
                assert_eq!(counterexample.semantic_members, 2);
            }
            PremiseViolation::SameEvidenceCut { .. } => {
                assert_eq!(counterexample.semantic_members, 1);
            }
        }
    }
}

#[test]
fn model_disjoint_peer_work_is_unchanged_by_an_unrelated_peer_ban() {
    let first = Transaction::independent(1, 1, 10, 20);
    let second = Transaction::independent(2, 2, 11, 21);
    let mut omega = model();
    for (transaction, peer) in [(first.clone(), 7), (second.clone(), 8)] {
        assert!(matches!(
            omega.kernel_step(KernelCommand::Admit(Admission {
                transaction,
                source: RetainedSource::Remote(RemoteResidency::new(
                    super::state::PeerId(peer),
                    RemoteDeadline(100),
                )),
                observed_at: super::state::MonotonicTick(1),
            })),
            KernelStep::AuthorityCommit { .. }
        ));
    }
    let second_before = omega.authority.owners[&second.id].clone();
    assert!(matches!(
        omega.kernel_step(KernelCommand::BanPeer {
            peer: super::state::PeerId(7),
            observed_at: super::state::MonotonicTick(2),
        }),
        KernelStep::AuthorityCommit { .. }
    ));
    assert!(!omega.authority.owners.contains_key(&first.id));
    assert_eq!(omega.authority.owners[&second.id], second_before);
    assert_eq!(omega.check_invariants(), Ok(()));
}

#[test]
fn model_independent_hostile_trace_search_preserves_every_commit_contract() {
    let transactions = adversarial_cohort(AdversarialShape::Independent(2))
        .expect("the hostile universe is bounded");
    let generator = HostileTraceGenerator::new(
        transactions,
        [super::state::PeerId(7), super::state::PeerId(8)],
        HostileTraceLimits {
            depth: 4,
            states: 8_000,
        },
    )
    .expect("the hostile universe is non-empty");
    let report = generator
        .explore(model())
        .expect("every independently generated hostile schedule is total");
    assert!(report.unique_states > 100);
    assert!(report.transitions > report.unique_states);
    assert!(report.authority_commits > 0);
    assert!(report.no_authority_commits > 0);
    assert!(report.environment_steps > 0);
    assert_eq!(report.deepest_trace, 4);
}

#[test]
fn model_hostile_universe_preserves_same_raw_distinct_witness_variants() {
    let original = Transaction::independent(1, 1, 10, 20);
    let variant = Transaction::independent(1, 9, 10, 20);
    let generator = HostileTraceGenerator::new(
        [original, variant],
        [super::state::PeerId(7)],
        HostileTraceLimits {
            depth: 2,
            states: 1_000,
        },
    )
    .expect("distinct verification identities share one raw identity legally");
    assert_eq!(
        generator.transaction_keys(),
        [
            HostileTxKey {
                raw: TxId(1),
                witness: WitnessId(1),
            },
            HostileTxKey {
                raw: TxId(1),
                witness: WitnessId(9),
            },
        ]
        .into_iter()
        .collect()
    );
    let report = generator
        .explore(model())
        .expect("the variant schedules remain ordinary outcomes");
    assert!(report.authority_commits > 0);
    assert!(report.no_authority_commits > 0);
}

#[test]
fn model_hostile_trace_search_finds_the_shortest_same_tip_new_revision_schedule() {
    let generator = HostileTraceGenerator::new(
        [Transaction::independent(1, 1, 10, 20)],
        [super::state::PeerId(7)],
        HostileTraceLimits {
            depth: 3,
            states: 2_000,
        },
    )
    .expect("the hostile universe is non-empty");
    let trace = generator
        .shortest_trace_to(model(), |state| {
            state.omega().authority.chain.tip == ViewId(1)
                && state.omega().authority.chain.revision.0 == 2
        })
        .expect("the repeated-view search remains bounded")
        .expect("T -> T' -> T is reachable");
    assert_eq!(
        trace,
        vec![HostileAction::AdvanceChain, HostileAction::AdvanceChain]
    );
}

#[test]
fn model_hostile_trace_makes_wall_and_monotonic_clock_domains_explicit() {
    let generator = HostileTraceGenerator::new(
        [Transaction::independent(1, 1, 10, 20)],
        [super::state::PeerId(7)],
        HostileTraceLimits {
            depth: 2,
            states: 2_000,
        },
    )
    .expect("the hostile universe is non-empty");

    let rollback = generator
        .shortest_trace_to(model(), |state| state.wall_clock() == 0)
        .expect("the wall-clock search remains bounded")
        .expect("wall-clock rollback is a legal environment observation");
    assert_eq!(rollback, vec![HostileAction::AdvanceWallClock { to: 0 }]);

    let monotonic = generator
        .shortest_trace_to(model(), |state| {
            state.monotonic_clock() == super::state::MonotonicTick(2)
        })
        .expect("the monotonic search remains bounded")
        .expect("forward monotonic progress is searchable");
    assert_eq!(
        monotonic,
        vec![HostileAction::AdvanceMonotonic {
            to: super::state::MonotonicTick(2),
        }]
    );

    let regression = generator
        .shortest_trace_to(model(), |state| {
            state.monotonic_clock() < super::state::MonotonicTick(1)
        })
        .expect("the monotonic regression search remains bounded");
    assert_eq!(regression, None, "process-monotonic time never regresses");
}
