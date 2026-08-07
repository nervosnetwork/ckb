use super::topology::{
    CachePublicationInput, CachePublicationTopology, CompleteTopology, CompleteTopologyGap,
    ExchangePermitAcquisition, ExchangePermitState, ExecutionTopology, IndependentWaveInput,
    OrderedBoundaryInput, OrderedBoundaryTopology, ProducerResidencyBound, QueryScratch,
    QueryScratchStep, QueryTopology, QueryTopologyInput, RetainedIngressTopology,
    exchange_permit_acquisition, retained_ingress_surface,
};

fn workload(owners: u32, workers: u32) -> IndependentWaveInput {
    IndependentWaveInput {
        owners,
        retained_worker_slots: workers,
        mutation_batch_limit: u32::try_from(crate::constants::MAX_POOL_MUTATION_CANDIDATES)
            .expect("the production mutation bound fits the model domain"),
        current_ready_batch_limit: u32::try_from(crate::constants::MAX_READY_BATCH)
            .expect("the production Ready bound fits the model domain"),
    }
}

#[test]
fn model_one_available_wave_falsifies_current_and_self_fused_serial_cut_targets() {
    let input = workload(8, 8);
    let current = input
        .compile(ExecutionTopology::CurrentUak)
        .expect("the current barrier workload is bounded");
    let self_fused = input
        .compile(ExecutionTopology::SelfFusedWorkers)
        .expect("the self-fused barrier workload is bounded");
    let exchange = input
        .compile(ExecutionTopology::BoundedSemanticExchange)
        .expect("the semantic exchange barrier workload is bounded");

    assert_eq!(current.total, 26);
    assert_eq!(self_fused.total, current.total);
    assert_eq!(exchange.total, 5);
    assert_eq!(exchange.ingress, 1);
    assert_eq!(exchange.compute, 2);
    assert_eq!(exchange.ready_membership, 1);
    assert_eq!(exchange.effect_settlement, 1);

    let prompt = workload(1, 8);
    assert_eq!(
        prompt
            .compile(ExecutionTopology::CurrentUak)
            .expect("one current owner is bounded")
            .total,
        5
    );
    assert_eq!(
        prompt
            .compile(ExecutionTopology::BoundedSemanticExchange)
            .expect("one exchanged owner is bounded")
            .total,
        5,
        "the no-timer exchange does not tax prompt single-item latency"
    );
}

#[test]
fn model_bounded_exchange_is_wave_amortization_not_asymptotic_magic() {
    let first = workload(64, 8)
        .compile(ExecutionTopology::BoundedSemanticExchange)
        .expect("the first fixed-width workload is bounded");
    let second = workload(128, 8)
        .compile(ExecutionTopology::BoundedSemanticExchange)
        .expect("the doubled fixed-width workload is bounded");
    assert_eq!(first.total, 26);
    assert_eq!(second.total, 51);
    assert!(second.total < first.total * 2);
    assert!(second.total > first.total);

    let current = workload(64, 8)
        .compile(ExecutionTopology::CurrentUak)
        .expect("the current fixed-width workload is bounded");
    let self_fused = workload(64, 8)
        .compile(ExecutionTopology::SelfFusedWorkers)
        .expect("the self-fused fixed-width workload is bounded");
    assert_eq!(current.total, 208);
    assert_eq!(self_fused.total, 152);
    assert!(first.total < self_fused.total);
}

#[test]
fn model_exchange_cost_names_its_task_channel_and_failure_price() {
    let input = workload(32, 8);
    let current = input
        .surface(ExecutionTopology::CurrentUak)
        .expect("the current topology has retained workers");
    let self_fused = input
        .surface(ExecutionTopology::SelfFusedWorkers)
        .expect("the self-fused topology has retained workers");
    let exchange = input
        .surface(ExecutionTopology::BoundedSemanticExchange)
        .expect("the exchange topology has retained workers");

    assert_eq!(current, self_fused);
    assert_eq!(current.compute_mutation_callers, 8);
    assert_eq!(exchange.compute_mutation_callers, 1);
    assert_eq!(exchange.compute_tasks, current.compute_tasks + 1);
    assert_eq!(exchange.transient_channel_slots, 16);
    assert_eq!(
        exchange.linear_capability_bound,
        current.linear_capability_bound
    );
    assert_eq!(exchange.added_join_edges, 1);
    assert!(!current.amortizes_one_available_wave);
    assert!(exchange.amortizes_one_available_wave);
}

#[test]
fn model_finished_exchange_never_waits_for_a_fair_permit_before_settlement() {
    assert_eq!(
        exchange_permit_acquisition(ExchangePermitState::FinishedCapabilityPresent),
        ExchangePermitAcquisition::ImmediateOnly
    );
    assert_eq!(
        exchange_permit_acquisition(ExchangePermitState::IdleFill),
        ExchangePermitAcquisition::MayQueueOne
    );
}

#[test]
fn model_existing_dispatcher_is_the_zero_topology_cost_ingress_combiner() {
    let per_request = retained_ingress_surface(RetainedIngressTopology::PerRequest);
    let dispatcher = retained_ingress_surface(RetainedIngressTopology::ExistingDispatcherDrain);
    let actor = retained_ingress_surface(RetainedIngressTopology::DedicatedIngressActor);
    assert!(!per_request.batches_immediately_available_requests);
    assert!(dispatcher.batches_immediately_available_requests);
    assert_eq!(dispatcher.added_tasks, 0);
    assert_eq!(dispatcher.added_channels, 0);
    assert!(!dispatcher.timer_or_fill_wait);
    assert!(dispatcher.exact_per_request_completion);
    assert_eq!(actor.added_tasks, 1);
    assert_eq!(actor.added_channels, 1);
}

#[test]
fn model_prepared_query_scratch_is_the_only_zero_duplicate_root_fix() {
    let input = QueryTopologyInput {
        concurrent_requests: 16,
        owner_rows: 10_000,
    };
    let current = input
        .compile(QueryTopology::CurrentGuarded)
        .expect("the current query cost is representable");
    let semaphore = input
        .compile(QueryTopology::SemaphoreOnly { permits: 1 })
        .expect("one query permit is valid");
    let scratch = input
        .compile(QueryTopology::PreparedScratch { permits: 1 })
        .expect("one scratch capture permit is valid");
    let projection = input
        .compile(QueryTopology::ResidentProjection)
        .expect("the resident projection cost is representable");

    assert_eq!(current.concurrent_guard_scans, 16);
    assert!(current.allocates_under_guard && current.sorts_under_guard);
    assert_eq!(semaphore.concurrent_guard_scans, 1);
    assert!(semaphore.allocates_under_guard && semaphore.sorts_under_guard);
    assert_eq!(scratch.concurrent_guard_scans, 1);
    assert!(!scratch.allocates_under_guard && !scratch.sorts_under_guard);
    assert!(scratch.bounded_capture_admission);
    assert_eq!(scratch.duplicate_resident_rows, 0);
    assert!(!scratch.per_apply_projection_work);
    assert_eq!(projection.duplicate_resident_rows, 10_000);
    assert!(projection.per_apply_projection_work);
}

#[test]
fn model_query_cost_uses_the_full_declared_u64_domain() {
    let input = QueryTopologyInput {
        concurrent_requests: u32::MAX,
        owner_rows: u32::MAX,
    };
    let cost = input
        .compile(QueryTopology::CurrentGuarded)
        .expect("the declared u64 row-visit domain is representable");
    assert_eq!(
        cost.authority_row_visits,
        u64::from(u32::MAX) * u64::from(u32::MAX)
    );
}

#[test]
fn model_query_scratch_growth_has_a_finite_strict_rank() {
    let first = QueryScratch {
        capacity: 0,
        max_capacity: 16_384,
    };
    let QueryScratchStep::Grow(second) = first.prepare(10_000, true) else {
        panic!("the first lock-external preparation must grow")
    };
    assert!(second.remaining_rank() < first.remaining_rank());
    assert_eq!(second.prepare(10_000, true), QueryScratchStep::Ready);
    assert_eq!(
        second.prepare(20_000, true),
        QueryScratchStep::RequestExceedsBound
    );
    assert_eq!(
        QueryScratch {
            capacity: 0,
            max_capacity: 16_384,
        }
        .prepare(10_000, false),
        QueryScratchStep::OrdinaryUnavailable
    );
}

#[test]
fn model_bounded_cache_writer_retains_worker_isolation_with_one_named_cost() {
    let input = CachePublicationInput {
        updates: 64,
        channel_updates: 1_024,
    };
    let inline = input.compile(CachePublicationTopology::InlineTryWrite);
    let writer = input.compile(CachePublicationTopology::BoundedWriter);

    assert_eq!(inline.persistent_tasks, 0);
    assert_eq!(inline.write_lock_attempts, 64);
    assert!(!inline.accepted_update_has_releaser);
    assert_eq!(writer.persistent_tasks, 1);
    assert_eq!(writer.resident_updates, 1_024);
    assert_eq!(writer.write_lock_attempts, 64);
    assert!(writer.accepted_update_has_releaser);
    assert!(!writer.worker_waits_for_cache);
}

#[test]
fn model_typed_ordered_boundary_bounds_external_admin_residency_without_dropping_reorg() {
    let input = OrderedBoundaryInput {
        trusted_reorg_publishers: 1,
    };
    let shared = input
        .compile(OrderedBoundaryTopology::SharedReliableSender)
        .expect("the shared boundary is representable");
    let split = input
        .compile(OrderedBoundaryTopology::TypedReorgAndBoundedAdmin)
        .expect("one trusted and one administrative capability are bounded");
    assert_eq!(
        shared.waiting_payloads,
        ProducerResidencyBound::UnboundedByProtocol
    );
    assert!(!shared.excess_admin_is_fail_fast);
    assert!(shared.reorg_is_lossless);
    assert_eq!(split.waiting_payloads, ProducerResidencyBound::Bounded(2));
    assert!(split.excess_admin_is_fail_fast);
    assert!(split.accepted_admin_preserves_order);
    assert!(split.added_admin_gate);
    assert!(split.reorg_is_lossless);
}

#[test]
fn model_complete_topology_selection_rejects_partial_fixes_without_stitching_exceptions() {
    let execution = workload(32, 8);
    let query = QueryTopologyInput {
        concurrent_requests: 16,
        owner_rows: 10_000,
    };
    let cache = CachePublicationInput {
        updates: 64,
        channel_updates: 1_024,
    };
    let ordered = OrderedBoundaryInput {
        trusted_reorg_publishers: 1,
    };
    let current = CompleteTopology {
        execution: ExecutionTopology::CurrentUak,
        ingress: RetainedIngressTopology::PerRequest,
        query: QueryTopology::CurrentGuarded,
        cache: CachePublicationTopology::BoundedWriter,
        ordered: OrderedBoundaryTopology::SharedReliableSender,
    };
    let self_fused = CompleteTopology {
        execution: ExecutionTopology::SelfFusedWorkers,
        ..current
    };
    let selected = CompleteTopology {
        execution: ExecutionTopology::BoundedSemanticExchange,
        ingress: RetainedIngressTopology::ExistingDispatcherDrain,
        query: QueryTopology::PreparedScratch { permits: 1 },
        cache: CachePublicationTopology::BoundedWriter,
        ordered: OrderedBoundaryTopology::TypedReorgAndBoundedAdmin,
    };

    let current_gaps = current
        .gaps(execution, query, cache, ordered)
        .expect("the current complete topology is representable");
    assert!(current_gaps.contains(&CompleteTopologyGap::PerOwnerAvailableWaveCuts));
    assert!(current_gaps.contains(&CompleteTopologyGap::PerRequestRetainedIngress));
    assert!(current_gaps.contains(&CompleteTopologyGap::GuardHeldFallibleQueryWork));
    assert!(current_gaps.contains(&CompleteTopologyGap::UnboundedOrderedProducerResidency));
    assert_eq!(
        self_fused
            .gaps(execution, query, cache, ordered)
            .expect("the self-fused complete topology is representable"),
        current_gaps,
        "worker-local fusion does not close a whole-system architecture gap"
    );
    assert_eq!(
        selected
            .gaps(execution, query, cache, ordered)
            .expect("the selected complete topology is representable"),
        Vec::new()
    );
}
