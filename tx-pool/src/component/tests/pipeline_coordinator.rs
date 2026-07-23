use crate::component::pipeline_coordinator::{
    CoordinatorError, CoordinatorFeeGate, CoordinatorLimits, CoordinatorLocation,
    CoordinatorMetadataCost, CoordinatorReconciliationLimits, CoordinatorResidency,
    CoordinatorSource, CoordinatorVerifyOrdering, PayloadPhase, PipelineCoordinator, QueueKind,
    RawStage, TerminalDisposition, TrustedSource, VerifiedCandidate, VerifySchedule,
    VerifyWorkLease, WorkerCapability,
};
use ckb_network::PeerIndex;
use ckb_types::packed::{Byte32, OutPoint, ProposalShortId};
use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};

#[derive(Debug, PartialEq, Eq)]
struct Raw(&'static str);

#[derive(Debug, PartialEq, Eq)]
struct Unverified(&'static str);

#[derive(Debug, PartialEq, Eq)]
struct Verified(&'static str);

fn hash(seed: u8) -> Byte32 {
    Byte32::new([seed; 32])
}

fn short(seed: u8) -> ProposalShortId {
    ProposalShortId::new([seed; 10])
}

fn set<const N: usize>(items: [Byte32; N]) -> HashSet<Byte32> {
    HashSet::from(items)
}

fn input(seed: u8) -> OutPoint {
    OutPoint::new(hash(seed), 0)
}

fn inputs<const N: usize>(items: [u8; N]) -> HashSet<OutPoint> {
    items.into_iter().map(input).collect()
}

fn test_limits(
    global: CoordinatorResidency,
    per_peer: Option<CoordinatorResidency>,
    max_dependencies_per_entry: usize,
    max_dependents_per_parent: usize,
) -> CoordinatorLimits {
    crate::component::pipeline_coordinator::CoordinatorLimits::new(
        global,
        per_peer,
        max_dependencies_per_entry,
        max_dependents_per_parent,
        CoordinatorReconciliationLimits::new(125, 64),
    )
}

fn roomy() -> PipelineCoordinator<Raw, Unverified, Verified> {
    PipelineCoordinator::new(test_limits(
        CoordinatorResidency::new(100, 100_000),
        Some(CoordinatorResidency::new(20, 20_000)),
        16,
        16,
    ))
}

fn enqueue_verify(
    coordinator: &mut PipelineCoordinator<Raw, Unverified, Verified>,
    seed: u8,
    source: CoordinatorSource,
    schedule: VerifySchedule,
) {
    coordinator
        .admit_raw_sourced(
            hash(seed),
            short(seed),
            Raw("raw"),
            RawStage::Resolve,
            source,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    let raw = coordinator
        .checkout_raw(RawStage::Resolve)
        .unwrap()
        .unwrap();
    assert_eq!(raw.hash, hash(seed));
    coordinator
        .complete_raw(&raw, Unverified("resolved"), 20, schedule)
        .unwrap();
}

fn verify_candidate(
    coordinator: &mut PipelineCoordinator<Raw, Unverified, Verified>,
    seed: u8,
    conflict_inputs: HashSet<OutPoint>,
    fee: u64,
) -> Byte32 {
    let (tx_hash, verify, candidate) = begin_candidate(
        coordinator,
        seed,
        CoordinatorSource::Local,
        conflict_inputs,
        fee,
    );
    let (_, evicted) = coordinator
        .complete_verification_candidate(&verify, Verified("proof"), 30, candidate)
        .unwrap();
    assert!(evicted.is_empty());
    tx_hash
}

fn begin_candidate(
    coordinator: &mut PipelineCoordinator<Raw, Unverified, Verified>,
    seed: u8,
    source: CoordinatorSource,
    conflict_inputs: HashSet<OutPoint>,
    fee: u64,
) -> (Byte32, VerifyWorkLease<Unverified>, VerifiedCandidate) {
    let tx_hash = hash(seed);
    coordinator
        .admit_raw_sourced(
            tx_hash.clone(),
            short(seed),
            Raw("raw"),
            RawStage::PreCheck,
            source,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    let raw = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    coordinator
        .complete_raw(&raw, Unverified("resolved"), 20, VerifySchedule::default())
        .unwrap();
    let verify = coordinator
        .checkout_verify(WorkerCapability::Any)
        .unwrap()
        .unwrap();
    let candidate = CoordinatorFeeGate::new(0, 0)
        .validate(tx_hash.clone(), conflict_inputs, fee, 100)
        .unwrap();
    (tx_hash, verify, candidate)
}

fn verify_plain(
    coordinator: &mut PipelineCoordinator<Raw, Unverified, Verified>,
    seed: u8,
) -> (
    Byte32,
    crate::component::pipeline_coordinator::CoordinatorVersion,
) {
    verify_plain_sourced(coordinator, seed, CoordinatorSource::Local)
}

fn verify_plain_sourced(
    coordinator: &mut PipelineCoordinator<Raw, Unverified, Verified>,
    seed: u8,
    source: CoordinatorSource,
) -> (
    Byte32,
    crate::component::pipeline_coordinator::CoordinatorVersion,
) {
    let tx_hash = hash(seed);
    coordinator
        .admit_raw_sourced(
            tx_hash.clone(),
            short(seed),
            Raw("raw"),
            RawStage::PreCheck,
            source,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    let raw = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    coordinator
        .complete_raw(&raw, Unverified("resolved"), 20, VerifySchedule::default())
        .unwrap();
    let verify = coordinator
        .checkout_verify(WorkerCapability::Any)
        .unwrap()
        .unwrap();
    let version = coordinator
        .complete_verification(&verify, Verified("proof"), 30)
        .unwrap()
        .0;
    (tx_hash, version)
}

#[test]
fn accepted_pool_inputs_wake_only_after_the_final_input_is_free() {
    let mut coordinator = roomy();
    let (tx_hash, version) = verify_plain(&mut coordinator, 80);
    let before = coordinator.view(&tx_hash).unwrap();
    let usage = coordinator.usage();
    for fault_step in 1..=2 {
        coordinator.set_apply_fault_for_test(Some(fault_step));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = coordinator.wait_for_pool_inputs(&tx_hash, version, inputs([180, 181]));
        }));
        assert!(result.is_err(), "fault step {fault_step} was not reached");
        coordinator.set_apply_fault_for_test(None);
        assert_eq!(coordinator.view(&tx_hash).unwrap(), before);
        assert_eq!(coordinator.usage(), usage);
        coordinator.audit().unwrap();
    }
    coordinator
        .wait_for_pool_inputs(&tx_hash, version, inputs([180, 181]))
        .unwrap();
    assert_eq!(coordinator.queue_len(QueueKind::Commit), 0);
    assert_eq!(
        coordinator.view(&tx_hash).unwrap().location,
        CoordinatorLocation::WaitingPoolInputs {
            inputs: inputs([180, 181])
        }
    );
    coordinator.audit().unwrap();

    assert_eq!(
        coordinator.pool_input_freed(&input(180), 1).unwrap(),
        vec![tx_hash.clone()]
    );
    assert_eq!(
        coordinator.view(&tx_hash).unwrap().location,
        CoordinatorLocation::WaitingPoolInputs {
            inputs: inputs([181])
        }
    );
    assert_eq!(coordinator.queue_len(QueueKind::Commit), 0);
    coordinator.audit().unwrap();

    coordinator.pool_input_freed(&input(181), 1).unwrap();
    assert_eq!(
        coordinator.view(&tx_hash).unwrap().location,
        CoordinatorLocation::ReadyToCommit
    );
    assert_eq!(coordinator.queue_len(QueueKind::Commit), 1);
    coordinator.audit().unwrap();
}

#[test]
fn accepted_pool_wait_releases_speculative_claim_but_retains_verified_ranking() {
    let mut coordinator = roomy();
    let shared = input(182);
    let accepted = input(183);
    let strong = verify_candidate(&mut coordinator, 81, HashSet::from([shared.clone()]), 200);
    let strong_version = coordinator.view(&strong).unwrap().version;
    coordinator
        .wait_for_pool_inputs(&strong, strong_version, HashSet::from([accepted.clone()]))
        .unwrap();
    assert!(coordinator.active_conflict_owner(&shared).is_none());

    let weak = verify_candidate(&mut coordinator, 82, HashSet::from([shared.clone()]), 100);
    assert_eq!(coordinator.active_conflict_owner(&shared), Some(&weak));
    coordinator.pool_input_freed(&accepted, 1).unwrap();
    assert_eq!(coordinator.conflict_recheck_len(), 1);
    coordinator.drain_conflict_rechecks(1).unwrap();
    assert_eq!(coordinator.active_conflict_owner(&shared), Some(&strong));
    assert!(matches!(
        coordinator.view(&weak).unwrap().location,
        CoordinatorLocation::WaitingConflict { .. }
    ));
    coordinator.audit().unwrap();
}

#[test]
fn accepted_pool_input_limits_and_wake_slices_are_transactional() {
    let limits = test_limits(
        CoordinatorResidency::new(20, 20_000),
        Some(CoordinatorResidency::new(20, 20_000)),
        16,
        16,
    )
    .with_pool_input_limits(2, 2, 4);
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> =
        PipelineCoordinator::new(limits);
    let (first, first_version) = verify_plain(&mut coordinator, 83);
    let (second, second_version) = verify_plain(&mut coordinator, 84);
    let shared = input(184);
    coordinator
        .wait_for_pool_inputs(&first, first_version, HashSet::from([shared.clone()]))
        .unwrap();
    coordinator
        .wait_for_pool_inputs(&second, second_version, HashSet::from([shared.clone()]))
        .unwrap();
    let before = [first.clone(), second.clone()].map(|hash| coordinator.view(&hash).unwrap());
    let usage = coordinator.usage();
    for fault_step in 1..=2 {
        coordinator.set_apply_fault_for_test(Some(fault_step));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = coordinator.pool_input_freed(&shared, 2);
        }));
        assert!(result.is_err(), "fault step {fault_step} was not reached");
        coordinator.set_apply_fault_for_test(None);
        let after = [first.clone(), second.clone()].map(|hash| coordinator.view(&hash).unwrap());
        assert_eq!(after, before);
        assert_eq!(coordinator.usage(), usage);
        assert_eq!(coordinator.queue_len(QueueKind::Commit), 0);
        coordinator.audit().unwrap();
    }
    assert_eq!(coordinator.pool_input_freed(&shared, 1).unwrap().len(), 1);
    assert_eq!(coordinator.queue_len(QueueKind::Commit), 1);
    assert_eq!(coordinator.pool_input_freed(&shared, 1).unwrap().len(), 1);
    assert_eq!(coordinator.queue_len(QueueKind::Commit), 2);
    coordinator.audit().unwrap();

    let (third, third_version) = verify_plain(&mut coordinator, 85);
    let too_many = inputs([185, 186, 187]);
    assert!(matches!(
        coordinator.wait_for_pool_inputs(&third, third_version, too_many),
        Err(CoordinatorError::PoolInputLimitExceeded)
    ));
    assert_eq!(
        coordinator.view(&third).unwrap().location,
        CoordinatorLocation::ReadyToCommit
    );
    coordinator.audit().unwrap();
}

#[test]
fn stronger_verified_work_reconciles_accepted_input_waiter_capacity() {
    let limits = test_limits(CoordinatorResidency::new(20, 20_000), None, 4, 4)
        .with_conflict_limits(1, 4, 8)
        .with_pool_input_limits(1, 1, 4);
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> =
        PipelineCoordinator::new(limits);
    let accepted = input(141);
    let (weak, verify, candidate) = begin_candidate(
        &mut coordinator,
        139,
        CoordinatorSource::Local,
        HashSet::from([input(139)]),
        100,
    );
    let (weak_version, evicted) = coordinator
        .complete_verification_candidate(&verify, Verified("weak"), 30, candidate)
        .unwrap();
    assert!(evicted.is_empty());
    coordinator
        .wait_for_pool_inputs(&weak, weak_version, HashSet::from([accepted.clone()]))
        .unwrap();

    let (strong, verify, candidate) = begin_candidate(
        &mut coordinator,
        140,
        CoordinatorSource::Local,
        HashSet::from([input(140)]),
        200,
    );
    let (strong_version, evicted) = coordinator
        .complete_verification_candidate(&verify, Verified("strong"), 30, candidate)
        .unwrap();
    assert!(evicted.is_empty());
    let (_, evicted) = coordinator
        .wait_for_pool_inputs(&strong, strong_version, HashSet::from([accepted.clone()]))
        .unwrap();
    assert_eq!(evicted.len(), 1);
    assert_eq!(evicted[0].hash, weak);
    assert_eq!(evicted[0].disposition, TerminalDisposition::CapacityEvicted);
    assert!(matches!(
        coordinator.view(&strong).unwrap().location,
        CoordinatorLocation::WaitingPoolInputs { inputs } if inputs == HashSet::from([accepted])
    ));
    coordinator.audit().unwrap();
}

#[test]
fn plain_pool_waiter_ties_evict_the_newer_entry_deterministically() {
    let limits = test_limits(CoordinatorResidency::new(10, 1_000), None, 4, 4)
        .with_pool_input_limits(1, 2, 4);
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> =
        PipelineCoordinator::new(limits);
    let accepted = input(210);
    let (earlier, earlier_version) =
        verify_plain_sourced(&mut coordinator, 177, CoordinatorSource::Remote(55.into()));
    coordinator
        .wait_for_pool_inputs(&earlier, earlier_version, HashSet::from([accepted.clone()]))
        .unwrap();
    let (later, later_version) =
        verify_plain_sourced(&mut coordinator, 178, CoordinatorSource::Remote(56.into()));
    coordinator
        .wait_for_pool_inputs(&later, later_version, HashSet::from([accepted.clone()]))
        .unwrap();

    let (trusted, trusted_version) =
        verify_plain_sourced(&mut coordinator, 179, CoordinatorSource::Local);
    let (_, evicted) = coordinator
        .wait_for_pool_inputs(&trusted, trusted_version, HashSet::from([accepted]))
        .unwrap();
    assert_eq!(evicted.len(), 1);
    assert_eq!(evicted[0].hash, later);
    assert!(coordinator.view(&earlier).is_some());
    assert!(coordinator.view(&trusted).is_some());
    coordinator.audit().unwrap();
}

#[test]
fn accepted_input_capacity_reconciliation_rolls_back_every_apply_boundary() {
    for fault_step in 1..=5 {
        let limits = test_limits(CoordinatorResidency::new(20, 20_000), None, 4, 4)
            .with_conflict_limits(1, 4, 8)
            .with_pool_input_limits(1, 1, 4);
        let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> =
            PipelineCoordinator::new(limits);
        let accepted = input(144);
        let (weak, verify, candidate) = begin_candidate(
            &mut coordinator,
            142,
            CoordinatorSource::Local,
            HashSet::from([input(142)]),
            100,
        );
        let weak_version = coordinator
            .complete_verification_candidate(&verify, Verified("weak"), 30, candidate)
            .unwrap()
            .0;
        coordinator
            .wait_for_pool_inputs(&weak, weak_version, HashSet::from([accepted.clone()]))
            .unwrap();
        let (strong, verify, candidate) = begin_candidate(
            &mut coordinator,
            143,
            CoordinatorSource::Local,
            HashSet::from([input(143)]),
            200,
        );
        let strong_version = coordinator
            .complete_verification_candidate(&verify, Verified("strong"), 30, candidate)
            .unwrap()
            .0;
        let before = [weak.clone(), strong.clone()].map(|hash| coordinator.view(&hash).unwrap());
        let usage = coordinator.usage();

        coordinator.set_apply_fault_for_test(Some(fault_step));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = coordinator.wait_for_pool_inputs(
                &strong,
                strong_version,
                HashSet::from([accepted.clone()]),
            );
        }));
        assert!(result.is_err(), "fault step {fault_step} was not reached");
        coordinator.set_apply_fault_for_test(None);

        let after = [weak, strong].map(|hash| coordinator.view(&hash).unwrap());
        assert_eq!(after, before);
        assert_eq!(coordinator.usage(), usage);
        coordinator.audit().unwrap();
    }
}

#[test]
fn metadata_residency_is_charged_continuously_across_every_wait_state() {
    let metadata = CoordinatorMetadataCost {
        entry_bytes: 5,
        dependency_edge_bytes: 7,
        lifecycle_ticket_bytes: 3,
        deadline_ticket_bytes: 11,
        conflict_edge_bytes: 13,
        pool_input_edge_bytes: 17,
    };
    let limits = test_limits(
        CoordinatorResidency::new(10, 1_000),
        Some(CoordinatorResidency::new(10, 1_000)),
        4,
        4,
    )
    .with_metadata_cost(metadata);
    let mut coordinator = PipelineCoordinator::new(limits);
    let tx_hash = hash(86);
    let parent = hash(186);
    let peer: PeerIndex = 22.into();
    coordinator
        .admit_raw_sourced(
            tx_hash.clone(),
            short(86),
            Raw("raw"),
            RawStage::Resolve,
            CoordinatorSource::Remote(peer),
            Some(1_000),
            10,
            set([parent.clone()]),
        )
        .unwrap();
    assert_eq!(coordinator.usage(), CoordinatorResidency::new(1, 36));
    assert_eq!(
        coordinator.peer_usage(peer),
        CoordinatorResidency::new(1, 36)
    );

    let raw = coordinator
        .checkout_raw(RawStage::Resolve)
        .unwrap()
        .unwrap();
    coordinator
        .complete_raw(&raw, Unverified("resolved"), 20, VerifySchedule::default())
        .unwrap();
    assert_eq!(coordinator.usage(), CoordinatorResidency::new(1, 46));
    let verify = coordinator
        .checkout_verify(WorkerCapability::Any)
        .unwrap()
        .unwrap();
    let candidate = CoordinatorFeeGate::new(0, 0)
        .validate(tx_hash.clone(), inputs([187, 188]), 100, 100)
        .unwrap();
    let version = coordinator
        .complete_verification_candidate(&verify, Verified("proof"), 30, candidate)
        .unwrap()
        .0;
    assert_eq!(coordinator.usage(), CoordinatorResidency::new(1, 82));
    coordinator
        .wait_for_pool_inputs(&tx_hash, version, inputs([189, 190]))
        .unwrap();
    assert_eq!(coordinator.usage(), CoordinatorResidency::new(1, 116));
    coordinator.pool_input_freed(&input(189), 1).unwrap();
    assert_eq!(coordinator.usage(), CoordinatorResidency::new(1, 99));

    coordinator.parent_unavailable(&parent).unwrap();
    assert_eq!(coordinator.usage(), CoordinatorResidency::new(1, 36));
    assert_eq!(
        coordinator.peer_usage(peer),
        CoordinatorResidency::new(1, 36)
    );
    coordinator.audit().unwrap();

    let tight_limits =
        test_limits(CoordinatorResidency::new(1, 35), None, 4, 4).with_metadata_cost(metadata);
    let mut tight: PipelineCoordinator<Raw, Unverified, Verified> =
        PipelineCoordinator::new(tight_limits);
    assert!(matches!(
        tight.admit_raw_sourced(
            hash(87),
            short(87),
            Raw("raw"),
            RawStage::Resolve,
            CoordinatorSource::Local,
            Some(1_000),
            10,
            set([hash(191)]),
        ),
        Err(CoordinatorError::GlobalBudgetExceeded)
    ));
    assert!(tight.is_empty());
    tight.audit().unwrap();
}

#[test]
fn trusted_source_promotion_releases_remote_charge_and_preserves_priority() {
    let mut coordinator = roomy();
    let local = hash(90);
    let promoted = hash(91);
    let peer: PeerIndex = 19.into();
    coordinator
        .admit_raw(
            local.clone(),
            short(90),
            Raw("local"),
            RawStage::PreCheck,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    coordinator
        .admit_raw(
            promoted.clone(),
            short(91),
            Raw("remote"),
            RawStage::PreCheck,
            Some(peer),
            20,
            HashSet::new(),
        )
        .unwrap();
    assert_eq!(
        coordinator.peer_usage(peer),
        CoordinatorResidency::new(1, 20)
    );

    coordinator
        .promote_source(&promoted, TrustedSource::Proposal)
        .unwrap();
    let view = coordinator.view(&promoted).unwrap();
    assert_eq!(view.source, CoordinatorSource::Proposal);
    assert_eq!(view.peer, None);
    assert_eq!(
        coordinator.peer_usage(peer),
        CoordinatorResidency::default()
    );
    assert_eq!(coordinator.usage(), CoordinatorResidency::new(2, 30));
    assert_eq!(
        coordinator
            .checkout_raw(RawStage::PreCheck)
            .unwrap()
            .unwrap()
            .hash,
        promoted
    );
    assert_eq!(
        coordinator.promote_source(&promoted, TrustedSource::Local),
        Err(CoordinatorError::SourceDowngrade)
    );
    coordinator.audit().unwrap();
}

#[test]
fn proposal_priority_lane_is_fifo_and_does_not_starve_earlier_proposals() {
    let mut coordinator = roomy();
    for seed in [95, 96, 97] {
        coordinator
            .admit_raw(
                hash(seed),
                short(seed),
                Raw("queued"),
                RawStage::PreCheck,
                None,
                10,
                HashSet::new(),
            )
            .unwrap();
    }
    coordinator
        .promote_source(&hash(96), TrustedSource::Proposal)
        .unwrap();
    coordinator
        .promote_source(&hash(97), TrustedSource::Proposal)
        .unwrap();
    coordinator
        .promote_source(&hash(96), TrustedSource::Proposal)
        .unwrap();

    let first = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    let second = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    let third = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    assert_eq!(
        [first.hash, second.hash, third.hash],
        [hash(97), hash(96), hash(95)]
    );
    coordinator.audit().unwrap();
}

#[test]
fn configured_fifo_order_and_active_caps_prevent_a_remote_prefix_monopoly() {
    let limits = test_limits(
        CoordinatorResidency::new(20, 20_000),
        Some(CoordinatorResidency::new(20, 20_000)),
        4,
        4,
    )
    .with_active_limits(3, 1);
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> =
        PipelineCoordinator::new(limits);
    let first_peer: PeerIndex = 31.into();
    let second_peer: PeerIndex = 32.into();
    for (seed, peer) in [
        (101, Some(first_peer)),
        (102, Some(first_peer)),
        (103, Some(second_peer)),
        (104, None),
    ] {
        coordinator
            .admit_raw(
                hash(seed),
                short(seed),
                Raw("fair"),
                RawStage::PreCheck,
                peer,
                10,
                HashSet::new(),
            )
            .unwrap();
    }

    let first = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    let second = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    let third = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    assert_eq!(first.hash, hash(101));
    assert_eq!(second.hash, hash(103));
    assert_eq!(third.hash, hash(104));
    assert_eq!(coordinator.active_work(), 3);
    assert_eq!(coordinator.peer_active_work(first_peer), 1);
    assert_eq!(coordinator.peer_active_work(second_peer), 1);
    assert!(
        coordinator
            .checkout_raw(RawStage::PreCheck)
            .unwrap()
            .is_none()
    );

    coordinator
        .complete_raw(&first, Unverified("done"), 20, VerifySchedule::default())
        .unwrap();
    let fourth = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    assert_eq!(fourth.hash, hash(102));
    assert_eq!(coordinator.peer_active_work(first_peer), 1);
    coordinator.audit().unwrap();
}

#[test]
fn verify_fee_ordering_is_descending_with_proposal_priority_above_fee() {
    let limits = test_limits(CoordinatorResidency::new(20, 20_000), None, 4, 4)
        .with_verify_ordering(CoordinatorVerifyOrdering::FeeRate);
    let mut coordinator = PipelineCoordinator::new(limits);
    enqueue_verify(
        &mut coordinator,
        105,
        CoordinatorSource::Local,
        VerifySchedule::new(100, false),
    );
    enqueue_verify(
        &mut coordinator,
        106,
        CoordinatorSource::Local,
        VerifySchedule::new(300, false),
    );
    enqueue_verify(
        &mut coordinator,
        107,
        CoordinatorSource::Local,
        VerifySchedule::new(200, false),
    );
    enqueue_verify(
        &mut coordinator,
        108,
        CoordinatorSource::Proposal,
        VerifySchedule::new(1, false),
    );

    let order = [0; 4].map(|_| {
        coordinator
            .checkout_verify(WorkerCapability::Any)
            .unwrap()
            .unwrap()
            .hash
    });
    assert_eq!(order, [hash(108), hash(106), hash(107), hash(105)]);
    coordinator.audit().unwrap();
}

#[test]
fn worker_capability_filters_eligibility_without_changing_fee_order() {
    let limits = test_limits(CoordinatorResidency::new(20, 20_000), None, 4, 4)
        .with_verify_ordering(CoordinatorVerifyOrdering::FeeRate);
    let mut coordinator = PipelineCoordinator::new(limits);
    enqueue_verify(
        &mut coordinator,
        109,
        CoordinatorSource::Local,
        VerifySchedule::new(300, true),
    );
    enqueue_verify(
        &mut coordinator,
        110,
        CoordinatorSource::Local,
        VerifySchedule::new(200, false),
    );

    let small = coordinator
        .checkout_verify(WorkerCapability::SmallCycleOnly)
        .unwrap()
        .unwrap();
    assert_eq!(small.hash, hash(110));
    let any = coordinator
        .checkout_verify(WorkerCapability::Any)
        .unwrap()
        .unwrap();
    assert_eq!(any.hash, hash(109));
    coordinator.audit().unwrap();
}

#[test]
fn scheduling_order_rebuilds_from_authoritative_ticket_keys() {
    let limits = test_limits(CoordinatorResidency::new(20, 20_000), None, 4, 4)
        .with_verify_ordering(CoordinatorVerifyOrdering::FeeRate);
    let mut coordinator = PipelineCoordinator::new(limits);
    enqueue_verify(
        &mut coordinator,
        113,
        CoordinatorSource::Local,
        VerifySchedule::new(100, false),
    );
    enqueue_verify(
        &mut coordinator,
        114,
        CoordinatorSource::Local,
        VerifySchedule::new(300, false),
    );
    enqueue_verify(
        &mut coordinator,
        115,
        CoordinatorSource::Proposal,
        VerifySchedule::new(1, false),
    );

    coordinator.rebuild_derived_indexes_for_test().unwrap();
    coordinator.audit().unwrap();
    let order = [0; 3].map(|_| {
        coordinator
            .checkout_verify(WorkerCapability::Any)
            .unwrap()
            .unwrap()
            .hash
    });
    assert_eq!(order, [hash(115), hash(114), hash(113)]);
    coordinator.audit().unwrap();
}

#[test]
fn queue_sequence_exhaustion_fails_before_admission_or_phase_transition() {
    let mut coordinator = roomy();
    coordinator.set_next_queue_sequence_for_test(u64::MAX);
    assert!(matches!(
        coordinator.admit_raw(
            hash(111),
            short(111),
            Raw("raw"),
            RawStage::Resolve,
            None,
            10,
            HashSet::new(),
        ),
        Err(CoordinatorError::QueueSequenceExhausted)
    ));
    assert!(coordinator.is_empty());

    coordinator.set_next_queue_sequence_for_test(0);
    coordinator
        .admit_raw(
            hash(112),
            short(112),
            Raw("raw"),
            RawStage::Resolve,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    let raw = coordinator
        .checkout_raw(RawStage::Resolve)
        .unwrap()
        .unwrap();
    let before = coordinator.view(&hash(112)).unwrap();
    coordinator.set_next_queue_sequence_for_test(u64::MAX);
    assert!(matches!(
        coordinator.complete_raw(
            &raw,
            Unverified("resolved"),
            20,
            VerifySchedule::new(100, false),
        ),
        Err(CoordinatorError::QueueSequenceExhausted)
    ));
    assert_eq!(coordinator.view(&hash(112)).unwrap(), before);
    coordinator.audit().unwrap();
}

#[test]
fn active_source_promotion_does_not_invalidate_owned_work() {
    let mut coordinator = roomy();
    let tx_hash = hash(92);
    let peer: PeerIndex = 20.into();
    coordinator
        .admit_raw(
            tx_hash.clone(),
            short(92),
            Raw("remote"),
            RawStage::Resolve,
            Some(peer),
            10,
            HashSet::new(),
        )
        .unwrap();
    let lease = coordinator
        .checkout_raw(RawStage::Resolve)
        .unwrap()
        .unwrap();
    coordinator
        .promote_source(&tx_hash, TrustedSource::Proposal)
        .unwrap();
    coordinator
        .complete_raw(
            &lease,
            Unverified("resolved"),
            20,
            VerifySchedule::default(),
        )
        .unwrap();
    assert_eq!(
        coordinator.peer_usage(peer),
        CoordinatorResidency::default()
    );
    assert_eq!(
        coordinator.view(&tx_hash).unwrap().source,
        CoordinatorSource::Proposal
    );
    coordinator.audit().unwrap();
}

#[test]
fn expiry_survives_revision_changes_but_not_re_admission() {
    let mut coordinator = roomy();
    let tx_hash = hash(93);
    coordinator
        .admit_raw_sourced(
            tx_hash.clone(),
            short(93),
            Raw("first"),
            RawStage::PreCheck,
            CoordinatorSource::Remote(21.into()),
            Some(10),
            10,
            HashSet::new(),
        )
        .unwrap();
    let lease = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    coordinator.requeue_raw(&lease).unwrap();
    assert!(coordinator.expire_due(9, 1).unwrap().is_empty());
    let terminal = coordinator.expire_due(10, 1).unwrap();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].disposition, TerminalDisposition::Expired);
    assert_eq!(coordinator.deadline_len(), 0);

    coordinator
        .admit_raw_sourced(
            tx_hash.clone(),
            short(93),
            Raw("second"),
            RawStage::PreCheck,
            CoordinatorSource::Local,
            Some(20),
            10,
            HashSet::new(),
        )
        .unwrap();
    assert!(coordinator.expire_due(10, 1).unwrap().is_empty());
    assert_eq!(coordinator.expire_due(20, 1).unwrap().len(), 1);
    coordinator.audit().unwrap();
}

#[test]
fn expiry_and_administrative_remove_cannot_steal_a_committing_lease() {
    let mut coordinator = roomy();
    let tx_hash = hash(98);
    coordinator
        .admit_raw_sourced(
            tx_hash.clone(),
            short(98),
            Raw("raw"),
            RawStage::PreCheck,
            CoordinatorSource::Local,
            Some(10),
            10,
            HashSet::new(),
        )
        .unwrap();
    let raw = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    coordinator
        .complete_raw(&raw, Unverified("resolved"), 20, VerifySchedule::default())
        .unwrap();
    let verify = coordinator
        .checkout_verify(WorkerCapability::Any)
        .unwrap()
        .unwrap();
    coordinator
        .complete_verification(&verify, Verified("proof"), 30)
        .unwrap();
    let commit = coordinator.begin_next_commit().unwrap().unwrap();

    assert!(coordinator.expire_due(10, 1).unwrap().is_empty());
    assert!(matches!(
        coordinator.force_terminalize(&tx_hash, TerminalDisposition::Removed),
        Err(CoordinatorError::CommitInProgress(hash)) if hash == tx_hash
    ));
    coordinator.abort_commit(&commit).unwrap();
    assert_eq!(coordinator.expire_due(10, 1).unwrap().len(), 1);
    coordinator.audit().unwrap();
}

#[test]
fn expiry_batch_never_loses_an_earlier_terminal_on_unwind() {
    let mut coordinator = roomy();
    let hashes = [hash(228), hash(229)];
    for (seed, tx_hash) in [(228, hashes[0].clone()), (229, hashes[1].clone())] {
        coordinator
            .admit_raw_sourced(
                tx_hash,
                short(seed),
                Raw("expiring"),
                RawStage::Resolve,
                CoordinatorSource::Local,
                Some(10),
                10,
                HashSet::new(),
            )
            .unwrap();
    }
    let before = hashes
        .clone()
        .map(|tx_hash| coordinator.view(&tx_hash).unwrap());
    let usage = coordinator.usage();

    for fault_step in 1..=6 {
        coordinator.set_apply_fault_for_test(Some(fault_step));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = coordinator.expire_due(10, 2);
        }));
        assert!(result.is_err(), "fault step {fault_step} was not reached");
        coordinator.set_apply_fault_for_test(None);
        let after = hashes
            .clone()
            .map(|tx_hash| coordinator.view(&tx_hash).unwrap());
        assert_eq!(after, before);
        assert_eq!(coordinator.usage(), usage);
        assert_eq!(coordinator.deadline_len(), 2);
        coordinator.audit().unwrap();
    }

    assert_eq!(coordinator.expire_due(10, 2).unwrap().len(), 2);
    assert!(coordinator.is_empty());
    coordinator.audit().unwrap();
}

#[test]
fn deadline_tombstones_compact_under_remove_readmit_churn() {
    let mut coordinator = roomy();
    let tx_hash = hash(94);
    for _ in 0..100 {
        coordinator
            .admit_raw_sourced(
                tx_hash.clone(),
                short(94),
                Raw("churn"),
                RawStage::Resolve,
                CoordinatorSource::Local,
                Some(1_000),
                10,
                HashSet::new(),
            )
            .unwrap();
        coordinator
            .force_terminalize(&tx_hash, TerminalDisposition::Removed)
            .unwrap()
            .unwrap();
    }
    coordinator
        .admit_raw_sourced(
            tx_hash,
            short(94),
            Raw("live"),
            RawStage::Resolve,
            CoordinatorSource::Local,
            Some(1_000),
            10,
            HashSet::new(),
        )
        .unwrap();
    assert!(coordinator.physical_deadline_slots_for_test() <= 66);
    coordinator.audit().unwrap();
}

#[test]
fn deadline_tombstones_compact_under_commit_abort_churn() {
    let mut coordinator = roomy();
    let tx_hash = hash(95);
    coordinator
        .admit_raw_sourced(
            tx_hash,
            short(95),
            Raw("churn"),
            RawStage::PreCheck,
            CoordinatorSource::Local,
            Some(1_000),
            10,
            HashSet::new(),
        )
        .unwrap();
    let raw = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    coordinator
        .complete_raw(&raw, Unverified("resolved"), 20, VerifySchedule::default())
        .unwrap();
    let verify = coordinator
        .checkout_verify(WorkerCapability::Any)
        .unwrap()
        .unwrap();
    coordinator
        .complete_verification(&verify, Verified("proof"), 30)
        .unwrap();

    for _ in 0..100 {
        let commit = coordinator.begin_next_commit().unwrap().unwrap();
        coordinator.abort_commit(&commit).unwrap();
    }

    assert_eq!(coordinator.deadline_len(), 1);
    assert!(coordinator.physical_deadline_slots_for_test() <= 66);
    coordinator.audit().unwrap();
}

#[test]
fn deterministic_state_machine_audits_every_ownership_boundary() {
    let mut coordinator = roomy();
    let mut raw_leases = Vec::new();
    let mut verify_leases = Vec::new();
    let mut commit_leases = Vec::new();
    let mut random = 0x9e37_79b9_7f4a_7c15u64;
    let mut now = 0u64;

    for step in 0..4_000u64 {
        random = random
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let seed = ((random >> 32) as u8 % 32).saturating_add(1);
        let tx_hash = hash(seed);
        let action = random % 14;
        match action {
            0 => {
                if coordinator.view(&tx_hash).is_none() {
                    let source = if seed % 3 == 0 {
                        CoordinatorSource::Remote(PeerIndex::from((seed % 4 + 1) as usize))
                    } else {
                        CoordinatorSource::Local
                    };
                    let stage = if seed % 2 == 0 {
                        RawStage::PreCheck
                    } else {
                        RawStage::Resolve
                    };
                    let _ = coordinator.admit_raw_sourced(
                        tx_hash,
                        short(seed),
                        Raw("state-machine"),
                        stage,
                        source,
                        Some(now.saturating_add(50)),
                        10,
                        HashSet::new(),
                    );
                }
            }
            1 => {
                if let Ok(Some(lease)) = coordinator.checkout_raw(RawStage::PreCheck) {
                    raw_leases.push(lease);
                }
            }
            2 => {
                if let Ok(Some(lease)) = coordinator.checkout_raw(RawStage::Resolve) {
                    raw_leases.push(lease);
                }
            }
            3 => {
                if let Some(lease) = raw_leases.pop() {
                    let _ = coordinator.complete_raw(
                        &lease,
                        Unverified("state-machine"),
                        20,
                        VerifySchedule::default(),
                    );
                }
            }
            4 => {
                if let Some(lease) = raw_leases.pop() {
                    let _ = coordinator.requeue_raw(&lease);
                }
            }
            5 => {
                if let Ok(Some(lease)) = coordinator.checkout_verify(WorkerCapability::Any) {
                    verify_leases.push(lease);
                }
            }
            6 => {
                if let Some(lease) = verify_leases.pop() {
                    let _ =
                        coordinator.complete_verification(&lease, Verified("state-machine"), 30);
                }
            }
            7 => {
                if let Ok(Some(lease)) = coordinator.begin_next_commit() {
                    commit_leases.push(lease);
                }
            }
            8 => {
                if let Some(lease) = commit_leases.pop() {
                    let _ = coordinator.abort_commit(&lease);
                }
            }
            9 => {
                if let Some(lease) = commit_leases.pop() {
                    let _ = coordinator.commit_handoff(&lease);
                }
            }
            10 => {
                let promotion = if seed % 2 == 0 {
                    TrustedSource::Proposal
                } else {
                    TrustedSource::Local
                };
                let _ = coordinator.promote_source(&tx_hash, promotion);
            }
            11 => {
                let _ = coordinator.force_terminalize(&tx_hash, TerminalDisposition::Removed);
            }
            12 => {
                now = now.saturating_add(1);
                let _ = coordinator.expire_due(now, 3);
            }
            _ => {
                if step % 257 == 0 {
                    let _ = coordinator.clear();
                }
            }
        }
        assert_eq!(
            coordinator.audit(),
            Ok(()),
            "state-machine step {step}, action {action}, seed {seed}"
        );
    }
    coordinator.clear().unwrap();
    coordinator.audit().unwrap();
}

#[test]
fn one_entry_and_revision_own_every_payload_phase_until_commit_handoff() {
    let mut coordinator = roomy();
    let tx_hash = hash(1);
    let peer: PeerIndex = 7.into();
    coordinator
        .admit_raw(
            tx_hash.clone(),
            short(1),
            Raw("raw"),
            RawStage::PreCheck,
            Some(peer),
            10,
            HashSet::new(),
        )
        .unwrap();
    coordinator.audit().unwrap();
    assert_eq!(coordinator.queue_len(QueueKind::PreCheck), 1);

    let raw = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    assert_eq!(*raw.payload, Raw("raw"));
    coordinator
        .complete_raw(&raw, Unverified("resolved"), 20, VerifySchedule::default())
        .unwrap();
    let view = coordinator.view(&tx_hash).unwrap();
    assert_eq!(view.phase, PayloadPhase::Unverified);
    assert_eq!(view.location, CoordinatorLocation::VerifyQueued);
    assert_eq!(coordinator.usage(), CoordinatorResidency::new(1, 20));
    coordinator.audit().unwrap();

    let verify = coordinator
        .checkout_verify(WorkerCapability::Any)
        .unwrap()
        .unwrap();
    assert_eq!(*verify.payload, Unverified("resolved"));
    coordinator
        .complete_verification(&verify, Verified("proof"), 30)
        .unwrap();
    let commit = coordinator.begin_next_commit().unwrap().unwrap();
    assert_eq!(*commit.payload, Verified("proof"));
    coordinator.audit().unwrap();

    let handoff = coordinator.commit_handoff(&commit).unwrap();
    assert_eq!(handoff.hash, tx_hash);
    assert_eq!(*handoff.raw, Raw("raw"));
    assert_eq!(*handoff.verified, Verified("proof"));
    assert_eq!(handoff.peer, Some(peer));
    assert!(coordinator.is_empty());
    assert_eq!(coordinator.usage(), CoordinatorResidency::default());
    coordinator.audit().unwrap();
}

#[test]
fn administrative_terminal_api_cannot_express_commit_and_releases_all_indexes() {
    let mut coordinator = roomy();
    let tx_hash = hash(2);
    coordinator
        .admit_raw(
            tx_hash.clone(),
            short(2),
            Raw("raw"),
            RawStage::Resolve,
            None,
            10,
            set([hash(20)]),
        )
        .unwrap();

    let terminal = coordinator
        .force_terminalize(&tx_hash, TerminalDisposition::Removed)
        .unwrap()
        .unwrap();
    assert_eq!(terminal.disposition, TerminalDisposition::Removed);
    assert_eq!(*terminal.raw, Raw("raw"));
    assert!(terminal.later_phase.is_none());
    assert!(coordinator.hash_by_short_id(&short(2)).is_none());
    assert!(coordinator.is_empty());
    coordinator.audit().unwrap();
}

#[test]
fn parent_invalidation_demotes_payload_and_makes_active_verify_lease_stale() {
    let mut coordinator = roomy();
    let tx_hash = hash(3);
    let parent = hash(30);
    coordinator
        .admit_raw(
            tx_hash.clone(),
            short(3),
            Raw("raw"),
            RawStage::Resolve,
            None,
            10,
            set([parent.clone()]),
        )
        .unwrap();
    let raw = coordinator
        .checkout_raw(RawStage::Resolve)
        .unwrap()
        .unwrap();
    coordinator
        .complete_raw(&raw, Unverified("resolved"), 50, VerifySchedule::default())
        .unwrap();
    let verify = coordinator
        .checkout_verify(WorkerCapability::Any)
        .unwrap()
        .unwrap();

    assert_eq!(
        coordinator.parent_unavailable(&parent).unwrap(),
        vec![tx_hash.clone()]
    );
    let view = coordinator.view(&tx_hash).unwrap();
    assert_eq!(view.phase, PayloadPhase::Raw);
    assert_eq!(view.charge_bytes, 10);
    assert_eq!(
        view.location,
        CoordinatorLocation::WaitingParents {
            missing: set([parent.clone()])
        }
    );
    assert!(matches!(
        coordinator.complete_verification(&verify, Verified("stale"), 60),
        Err(CoordinatorError::RevisionMismatch { .. })
    ));

    let ready = coordinator.parent_available(&parent).unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(coordinator.queue_len(QueueKind::Resolve), 1);
    coordinator.audit().unwrap();
}

#[test]
fn definitive_parent_failure_is_fail_closed_and_drained_in_bounded_slices() {
    let mut coordinator = roomy();
    let parent = hash(200);
    let child = hash(201);
    let grandchild = hash(202);
    let great_grandchild = hash(203);

    coordinator
        .admit_raw(
            child.clone(),
            short(201),
            Raw("child"),
            RawStage::Resolve,
            None,
            10,
            set([parent.clone()]),
        )
        .unwrap();
    let child_raw = coordinator
        .checkout_raw(RawStage::Resolve)
        .unwrap()
        .unwrap();
    coordinator
        .complete_raw(
            &child_raw,
            Unverified("child"),
            20,
            VerifySchedule::default(),
        )
        .unwrap();
    let child_verify = coordinator
        .checkout_verify(WorkerCapability::Any)
        .unwrap()
        .unwrap();

    coordinator
        .admit_raw(
            grandchild.clone(),
            short(202),
            Raw("grandchild"),
            RawStage::Resolve,
            None,
            10,
            set([child.clone()]),
        )
        .unwrap();
    let grand_raw = coordinator
        .checkout_raw(RawStage::Resolve)
        .unwrap()
        .unwrap();
    coordinator
        .complete_raw(
            &grand_raw,
            Unverified("grandchild"),
            20,
            VerifySchedule::default(),
        )
        .unwrap();
    let grand_verify = coordinator
        .checkout_verify(WorkerCapability::Any)
        .unwrap()
        .unwrap();
    coordinator
        .complete_verification(&grand_verify, Verified("grandchild"), 30)
        .unwrap();
    let grand_commit = coordinator.begin_next_commit().unwrap().unwrap();

    coordinator
        .admit_raw(
            great_grandchild.clone(),
            short(203),
            Raw("great-grandchild"),
            RawStage::Resolve,
            None,
            10,
            set([grandchild.clone()]),
        )
        .unwrap();
    assert_eq!(coordinator.active_work(), 2);

    assert_eq!(
        coordinator.schedule_parent_failure(&parent).unwrap(),
        vec![child.clone()]
    );
    assert_eq!(coordinator.dependency_failure_len(), 1);
    assert_eq!(coordinator.active_work(), 1);
    assert!(matches!(
        coordinator.complete_verification(&child_verify, Verified("stale"), 30),
        Err(CoordinatorError::RevisionMismatch { .. })
    ));
    assert!(matches!(
        coordinator.commit_handoff(&grand_commit),
        Err(CoordinatorError::DependencyInvalidated {
            child: failed_child,
            parent: failed_parent,
        }) if failed_child == grandchild && failed_parent == child
    ));
    coordinator.audit().unwrap();

    let first = coordinator.drain_dependency_failures(1).unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].hash, child);
    assert_eq!(first[0].disposition, TerminalDisposition::DependencyFailed);
    assert_eq!(coordinator.active_work(), 0);
    assert!(matches!(
        coordinator.view(&grandchild).unwrap().location,
        CoordinatorLocation::Invalidated { .. }
    ));
    coordinator.audit().unwrap();

    let second = coordinator.drain_dependency_failures(1).unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].hash, grandchild);
    assert!(matches!(
        coordinator.view(&great_grandchild).unwrap().location,
        CoordinatorLocation::Invalidated { .. }
    ));
    coordinator.audit().unwrap();

    let third = coordinator.drain_dependency_failures(1).unwrap();
    assert_eq!(third.len(), 1);
    assert_eq!(third[0].hash, great_grandchild);
    assert!(coordinator.is_empty());
    coordinator.audit().unwrap();
}

#[test]
fn injected_multi_entry_unwind_restores_entries_and_rebuilds_indexes() {
    let mut coordinator = roomy();
    let parent = hash(204);
    let children = [hash(205), hash(206)];
    for (seed, child) in [(205, children[0].clone()), (206, children[1].clone())] {
        coordinator
            .admit_raw(
                child,
                short(seed),
                Raw("child"),
                RawStage::Resolve,
                Some(PeerIndex::from(seed as usize)),
                10,
                set([parent.clone()]),
            )
            .unwrap();
    }
    let before: Vec<_> = children
        .iter()
        .map(|child| coordinator.view(child).unwrap())
        .collect();
    let usage = coordinator.usage();
    let physical = coordinator.physical_queue_slots_for_test(QueueKind::Resolve);
    for fault_step in 1..=4 {
        coordinator.set_apply_fault_for_test(Some(fault_step));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = coordinator.schedule_parent_failure(&parent);
        }));
        assert!(result.is_err(), "fault step {fault_step} was not reached");
        coordinator.set_apply_fault_for_test(None);
        let after: Vec<_> = children
            .iter()
            .map(|child| coordinator.view(child).unwrap())
            .collect();
        assert_eq!(after, before);
        assert_eq!(coordinator.usage(), usage);
        assert_eq!(coordinator.dependency_failure_len(), 0);
        assert_eq!(coordinator.queue_len(QueueKind::Resolve), 2);
        assert_eq!(
            coordinator.physical_queue_slots_for_test(QueueKind::Resolve),
            physical
        );
        coordinator.audit().unwrap();
    }

    assert_eq!(
        coordinator.schedule_parent_failure(&parent).unwrap().len(),
        2
    );
    assert_eq!(coordinator.drain_dependency_failures(2).unwrap().len(), 2);
    coordinator.audit().unwrap();
}

#[test]
fn dependency_failure_batch_never_loses_an_earlier_terminal_on_unwind() {
    let mut coordinator = roomy();
    let parent = hash(221);
    let children = [hash(222), hash(223)];
    for (seed, child) in [(222, children[0].clone()), (223, children[1].clone())] {
        coordinator
            .admit_raw(
                child,
                short(seed),
                Raw("child"),
                RawStage::Resolve,
                None,
                10,
                set([parent.clone()]),
            )
            .unwrap();
    }
    coordinator.schedule_parent_failure(&parent).unwrap();
    let before = children
        .clone()
        .map(|child| coordinator.view(&child).unwrap());
    let usage = coordinator.usage();

    for fault_step in 1..=6 {
        coordinator.set_apply_fault_for_test(Some(fault_step));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = coordinator.drain_dependency_failures(2);
        }));
        assert!(result.is_err(), "fault step {fault_step} was not reached");
        coordinator.set_apply_fault_for_test(None);
        let after = children
            .clone()
            .map(|child| coordinator.view(&child).unwrap());
        assert_eq!(after, before);
        assert_eq!(coordinator.usage(), usage);
        assert_eq!(coordinator.dependency_failure_len(), 2);
        coordinator.audit().unwrap();
    }

    let terminal = coordinator.drain_dependency_failures(2).unwrap();
    assert_eq!(terminal.len(), 2);
    assert!(coordinator.is_empty());
    coordinator.audit().unwrap();
}

#[test]
fn dependency_maintenance_rebuild_preserves_authoritative_enqueue_order() {
    let mut coordinator = roomy();
    let earlier_parent = hash(20);
    let earlier_child = hash(21);
    let later_parent = hash(22);
    let later_child = hash(23);
    for (seed, child, parent) in [
        (21, earlier_child.clone(), earlier_parent.clone()),
        (23, later_child.clone(), later_parent.clone()),
    ] {
        coordinator
            .admit_raw(
                child,
                short(seed),
                Raw("child"),
                RawStage::Resolve,
                None,
                10,
                set([parent]),
            )
            .unwrap();
    }

    // Enqueue the larger hash first so a hash-map rebuild cannot accidentally
    // satisfy this assertion by sorting or iteration order.
    coordinator.schedule_parent_failure(&later_parent).unwrap();
    coordinator
        .schedule_parent_failure(&earlier_parent)
        .unwrap();
    coordinator.set_apply_fault_for_test(Some(1));
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _ = coordinator.drain_dependency_failures(2);
    }));
    assert!(result.is_err());
    coordinator.set_apply_fault_for_test(None);
    coordinator.audit().unwrap();

    let first = coordinator.drain_dependency_failures(1).unwrap();
    assert_eq!(first[0].hash, later_child);
    let second = coordinator.drain_dependency_failures(1).unwrap();
    assert_eq!(second[0].hash, earlier_child);
    coordinator.audit().unwrap();
}

#[test]
fn maintenance_sequence_exhaustion_fails_before_invalidation() {
    let mut coordinator = roomy();
    let parent = hash(24);
    let child = hash(25);
    coordinator
        .admit_raw(
            child.clone(),
            short(25),
            Raw("child"),
            RawStage::Resolve,
            None,
            10,
            set([parent.clone()]),
        )
        .unwrap();
    let before = coordinator.view(&child).unwrap();
    coordinator.set_next_maintenance_sequence_for_test(u64::MAX);

    assert_eq!(
        coordinator.schedule_parent_failure(&parent),
        Err(CoordinatorError::MaintenanceSequenceExhausted)
    );
    assert_eq!(coordinator.view(&child).unwrap(), before);
    assert_eq!(coordinator.dependency_failure_len(), 0);
    coordinator.audit().unwrap();
}

#[test]
fn maintenance_sequence_allocator_rolls_back_with_failed_transition() {
    let mut coordinator = roomy();
    let parent = hash(26);
    let child = hash(27);
    coordinator
        .admit_raw(
            child.clone(),
            short(27),
            Raw("child"),
            RawStage::Resolve,
            None,
            10,
            set([parent.clone()]),
        )
        .unwrap();
    coordinator.set_next_maintenance_sequence_for_test(u64::MAX - 1);
    coordinator.set_apply_fault_for_test(Some(1));
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _ = coordinator.schedule_parent_failure(&parent);
    }));
    assert!(result.is_err());
    coordinator.set_apply_fault_for_test(None);

    assert_eq!(coordinator.dependency_failure_len(), 0);
    assert_eq!(
        coordinator.schedule_parent_failure(&parent).unwrap(),
        vec![child]
    );
    coordinator.audit().unwrap();
}

#[test]
fn final_parent_wake_is_exactly_once_and_batch_preflight_is_atomic() {
    let mut coordinator = roomy();
    let parent = hash(40);
    let child_a = hash(4);
    let child_b = hash(5);
    for (child, seed) in [(child_a.clone(), 4), (child_b.clone(), 5)] {
        coordinator
            .admit_raw(
                child.clone(),
                short(seed),
                Raw("raw"),
                RawStage::Resolve,
                None,
                10,
                set([parent.clone()]),
            )
            .unwrap();
        let lease = coordinator
            .checkout_raw(RawStage::Resolve)
            .unwrap()
            .unwrap();
        coordinator
            .wait_for_parents(&lease, set([parent.clone()]))
            .unwrap();
    }
    coordinator
        .set_revision_for_test(&child_b, u64::MAX)
        .unwrap();

    assert_eq!(
        coordinator.parent_available(&parent),
        Err(CoordinatorError::RevisionExhausted(child_b.clone()))
    );
    for child in [&child_a, &child_b] {
        assert!(matches!(
            coordinator.view(child).unwrap().location,
            CoordinatorLocation::WaitingParents { ref missing }
                if missing == &set([parent.clone()])
        ));
    }
    assert_eq!(coordinator.queue_len(QueueKind::Resolve), 0);
    coordinator.audit().unwrap();
}

#[test]
fn every_dependency_batch_apply_boundary_is_all_old_or_all_new() {
    let parent = hash(230);
    let children = [hash(231), hash(232)];

    let mut unavailable = roomy();
    for (seed, child) in [(231, children[0].clone()), (232, children[1].clone())] {
        unavailable
            .admit_raw(
                child,
                short(seed),
                Raw("child"),
                RawStage::Resolve,
                None,
                10,
                set([parent.clone()]),
            )
            .unwrap();
    }
    let before = children
        .clone()
        .map(|child| unavailable.view(&child).unwrap());
    for fault_step in 1..=4 {
        unavailable.set_apply_fault_for_test(Some(fault_step));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = unavailable.parent_unavailable(&parent);
        }));
        assert!(result.is_err(), "fault step {fault_step} was not reached");
        unavailable.set_apply_fault_for_test(None);
        let after = children
            .clone()
            .map(|child| unavailable.view(&child).unwrap());
        assert_eq!(after, before);
        assert_eq!(unavailable.queue_len(QueueKind::Resolve), 2);
        unavailable.audit().unwrap();
    }
    assert_eq!(unavailable.parent_unavailable(&parent).unwrap().len(), 2);
    unavailable.audit().unwrap();

    let mut available = roomy();
    for (seed, child) in [(231, children[0].clone()), (232, children[1].clone())] {
        available
            .admit_raw(
                child,
                short(seed),
                Raw("child"),
                RawStage::Resolve,
                None,
                10,
                set([parent.clone()]),
            )
            .unwrap();
        let lease = available.checkout_raw(RawStage::Resolve).unwrap().unwrap();
        available
            .wait_for_parents(&lease, set([parent.clone()]))
            .unwrap();
    }
    let before = children
        .clone()
        .map(|child| available.view(&child).unwrap());
    for fault_step in 1..=2 {
        available.set_apply_fault_for_test(Some(fault_step));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = available.parent_available(&parent);
        }));
        assert!(result.is_err(), "fault step {fault_step} was not reached");
        available.set_apply_fault_for_test(None);
        let after = children
            .clone()
            .map(|child| available.view(&child).unwrap());
        assert_eq!(after, before);
        assert_eq!(available.queue_len(QueueKind::Resolve), 0);
        available.audit().unwrap();
    }
    assert_eq!(available.parent_available(&parent).unwrap().len(), 2);
    assert_eq!(available.queue_len(QueueKind::Resolve), 2);
    available.audit().unwrap();
}

#[test]
fn revision_exhaustion_does_not_consume_the_only_live_queue_ticket() {
    let mut coordinator = roomy();
    let tx_hash = hash(6);
    coordinator
        .admit_raw(
            tx_hash.clone(),
            short(6),
            Raw("raw"),
            RawStage::PreCheck,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    coordinator
        .set_revision_for_test(&tx_hash, u64::MAX)
        .unwrap();

    assert!(matches!(
        coordinator.checkout_raw(RawStage::PreCheck),
        Err(CoordinatorError::RevisionExhausted(hash)) if hash == tx_hash
    ));
    assert_eq!(coordinator.queue_len(QueueKind::PreCheck), 1);
    assert!(coordinator.physical_queue_slots_for_test(QueueKind::PreCheck) >= 1);
    coordinator.audit().unwrap();
}

#[test]
fn removed_and_readmitted_hash_rejects_the_old_worker_incarnation() {
    let mut coordinator = roomy();
    let tx_hash = hash(7);
    coordinator
        .admit_raw(
            tx_hash.clone(),
            short(7),
            Raw("old"),
            RawStage::PreCheck,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    let old = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    coordinator
        .force_terminalize(&tx_hash, TerminalDisposition::Cleared)
        .unwrap();
    coordinator
        .admit_raw(
            tx_hash.clone(),
            short(7),
            Raw("new"),
            RawStage::PreCheck,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();

    assert!(matches!(
        coordinator.complete_raw(&old, Unverified("stale"), 20, VerifySchedule::default()),
        Err(CoordinatorError::IncarnationMismatch { .. })
    ));
    let current = coordinator.view(&tx_hash).unwrap();
    assert_eq!(current.phase, PayloadPhase::Raw);
    assert_eq!(
        current.location,
        CoordinatorLocation::RawQueued(RawStage::PreCheck)
    );
    coordinator.audit().unwrap();
}

#[test]
fn identity_budget_and_fanout_failures_do_not_partially_admit() {
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> =
        PipelineCoordinator::new(test_limits(
            CoordinatorResidency::new(2, 20),
            Some(CoordinatorResidency::new(1, 10)),
            1,
            1,
        ));
    let peer: PeerIndex = 1.into();
    let parent = hash(80);
    let first = hash(8);
    coordinator
        .admit_raw(
            first.clone(),
            short(8),
            Raw("first"),
            RawStage::Resolve,
            Some(peer),
            10,
            set([parent.clone()]),
        )
        .unwrap();

    assert!(matches!(
        coordinator.admit_raw(
            hash(9),
            short(9),
            Raw("fanout"),
            RawStage::Resolve,
            Some(2.into()),
            10,
            set([parent.clone()]),
        ),
        Err(CoordinatorError::ParentFanoutLimitExceeded(hash)) if hash == parent
    ));
    assert!(matches!(
        coordinator.admit_raw(
            hash(10),
            short(8),
            Raw("collision"),
            RawStage::PreCheck,
            None,
            10,
            HashSet::new(),
        ),
        Err(CoordinatorError::ShortIdCollision {
            short_id,
            existing_hash,
        }) if short_id == short(8) && existing_hash == first
    ));
    assert_eq!(coordinator.len(), 1);
    assert_eq!(coordinator.usage(), CoordinatorResidency::new(1, 10));
    coordinator.audit().unwrap();
}

#[test]
fn transitive_dependency_cycle_is_rejected_before_admission() {
    let mut coordinator = roomy();
    let first = hash(175);
    let second = hash(176);
    coordinator
        .admit_raw(
            first.clone(),
            short(175),
            Raw("first"),
            RawStage::Resolve,
            None,
            10,
            HashSet::from([second.clone()]),
        )
        .unwrap();
    assert!(matches!(
        coordinator.admit_raw(
            second.clone(),
            short(176),
            Raw("cycle"),
            RawStage::Resolve,
            None,
            10,
            HashSet::from([first.clone()]),
        ),
        Err(CoordinatorError::DependencyCycle(hash)) if hash == second
    ));
    assert!(coordinator.view(&first).is_some());
    assert!(coordinator.view(&second).is_none());
    coordinator.audit().unwrap();
}

#[test]
fn stronger_source_reconciles_parent_capacity_with_explicit_causal_eviction() {
    let limits = test_limits(
        CoordinatorResidency::new(10, 10_000),
        Some(CoordinatorResidency::new(10, 10_000)),
        4,
        1,
    );
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> =
        PipelineCoordinator::new(limits);
    let parent = hash(116);
    let remote = hash(117);
    let remote_child = hash(118);
    coordinator
        .admit_raw(
            remote.clone(),
            short(117),
            Raw("remote"),
            RawStage::Resolve,
            Some(41.into()),
            10,
            set([parent.clone()]),
        )
        .unwrap();
    coordinator
        .admit_raw(
            remote_child.clone(),
            short(118),
            Raw("remote child"),
            RawStage::Resolve,
            Some(41.into()),
            10,
            set([remote.clone()]),
        )
        .unwrap();

    let local = hash(119);
    let (_, evicted) = coordinator
        .admit_raw_sourced(
            local.clone(),
            short(119),
            Raw("local"),
            RawStage::Resolve,
            CoordinatorSource::Local,
            None,
            10,
            set([parent.clone()]),
        )
        .unwrap();
    assert_eq!(evicted.len(), 1);
    assert_eq!(evicted[0].hash, remote);
    assert_eq!(evicted[0].disposition, TerminalDisposition::CapacityEvicted);
    assert!(coordinator.view(&local).is_some());
    assert!(matches!(
        coordinator.view(&remote_child).unwrap().location,
        CoordinatorLocation::Invalidated { cause } if cause == hash(117)
    ));

    let proposal = hash(120);
    let (_, evicted) = coordinator
        .admit_raw_sourced(
            proposal.clone(),
            short(120),
            Raw("proposal"),
            RawStage::Resolve,
            CoordinatorSource::Proposal,
            None,
            10,
            set([parent]),
        )
        .unwrap();
    assert_eq!(evicted.len(), 1);
    assert_eq!(evicted[0].hash, local);
    assert_eq!(evicted[0].disposition, TerminalDisposition::CapacityEvicted);
    assert!(coordinator.view(&proposal).is_some());
    coordinator.audit().unwrap();
}

#[test]
fn parent_capacity_reconciliation_is_all_old_or_all_new_on_unwind() {
    for fault_step in 1..=3 {
        let limits = test_limits(
            CoordinatorResidency::new(10, 10_000),
            Some(CoordinatorResidency::new(10, 10_000)),
            4,
            1,
        );
        let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> =
            PipelineCoordinator::new(limits);
        let parent = hash(121);
        let remote = hash(122);
        coordinator
            .admit_raw(
                remote.clone(),
                short(122),
                Raw("remote"),
                RawStage::Resolve,
                Some(42.into()),
                10,
                set([parent.clone()]),
            )
            .unwrap();
        let before = coordinator.view(&remote).unwrap();
        let usage = coordinator.usage();

        coordinator.set_apply_fault_for_test(Some(fault_step));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = coordinator.admit_raw_sourced(
                hash(123),
                short(123),
                Raw("proposal"),
                RawStage::Resolve,
                CoordinatorSource::Proposal,
                None,
                10,
                set([parent.clone()]),
            );
        }));
        assert!(result.is_err(), "fault step {fault_step} was not reached");
        coordinator.set_apply_fault_for_test(None);

        assert_eq!(coordinator.view(&remote).unwrap(), before);
        assert!(coordinator.view(&hash(123)).is_none());
        assert_eq!(coordinator.usage(), usage);
        coordinator.audit().unwrap();
    }
}

#[test]
fn global_admission_reconciliation_preserves_dependency_ancestors() {
    let limits = test_limits(
        CoordinatorResidency::new(3, 30),
        Some(CoordinatorResidency::new(3, 30)),
        4,
        4,
    );
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> =
        PipelineCoordinator::new(limits);
    let parent = hash(145);
    coordinator
        .admit_raw(
            parent.clone(),
            short(145),
            Raw("parent"),
            RawStage::Resolve,
            Some(44.into()),
            10,
            HashSet::new(),
        )
        .unwrap();
    for seed in [146, 147] {
        coordinator
            .admit_raw(
                hash(seed),
                short(seed),
                Raw("remote filler"),
                RawStage::Resolve,
                Some(44.into()),
                10,
                HashSet::new(),
            )
            .unwrap();
    }

    let child = hash(148);
    let (_, evicted) = coordinator
        .admit_raw_sourced(
            child.clone(),
            short(148),
            Raw("local child"),
            RawStage::Resolve,
            CoordinatorSource::Local,
            None,
            10,
            set([parent.clone()]),
        )
        .unwrap();
    assert_eq!(evicted.len(), 1);
    assert_eq!(evicted[0].hash, hash(147));
    assert!(coordinator.view(&parent).is_some());
    assert!(coordinator.view(&child).is_some());
    assert!(matches!(
        coordinator.admit_raw(
            hash(149),
            short(149),
            Raw("remote"),
            RawStage::Resolve,
            Some(45.into()),
            10,
            HashSet::new(),
        ),
        Err(CoordinatorError::GlobalBudgetExceeded)
    ));
    coordinator.audit().unwrap();
}

#[test]
fn global_recharge_reconciliation_covers_raw_and_plain_verified_phases() {
    let limits = test_limits(CoordinatorResidency::new(2, 25), None, 4, 4);
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> =
        PipelineCoordinator::new(limits);
    let local = hash(150);
    coordinator
        .admit_raw_sourced(
            local.clone(),
            short(150),
            Raw("local"),
            RawStage::Resolve,
            CoordinatorSource::Local,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    coordinator
        .admit_raw(
            hash(151),
            short(151),
            Raw("remote"),
            RawStage::Resolve,
            Some(46.into()),
            10,
            HashSet::new(),
        )
        .unwrap();
    let raw = coordinator
        .checkout_raw(RawStage::Resolve)
        .unwrap()
        .unwrap();
    assert_eq!(raw.hash, local);
    let (_, evicted) = coordinator
        .complete_raw(&raw, Unverified("resolved"), 20, VerifySchedule::default())
        .unwrap();
    assert_eq!(evicted.len(), 1);
    assert_eq!(evicted[0].hash, hash(151));

    coordinator
        .admit_raw(
            hash(152),
            short(152),
            Raw("remote filler"),
            RawStage::Resolve,
            Some(46.into()),
            5,
            HashSet::new(),
        )
        .unwrap();
    let verify = coordinator
        .checkout_verify(WorkerCapability::Any)
        .unwrap()
        .unwrap();
    let (_, evicted) = coordinator
        .complete_verification(&verify, Verified("proof"), 25)
        .unwrap();
    assert_eq!(evicted.len(), 1);
    assert_eq!(evicted[0].hash, hash(152));
    assert_eq!(coordinator.usage(), CoordinatorResidency::new(1, 25));
    coordinator.audit().unwrap();
}

#[test]
fn global_recharge_reconciliation_covers_candidate_and_pool_wait_metadata() {
    let metadata = CoordinatorMetadataCost {
        conflict_edge_bytes: 10,
        pool_input_edge_bytes: 10,
        ..CoordinatorMetadataCost::default()
    };
    let limits = test_limits(CoordinatorResidency::new(3, 60), None, 4, 4)
        .with_conflict_limits(1, 4, 8)
        .with_pool_input_limits(1, 4, 8)
        .with_metadata_cost(metadata);
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> =
        PipelineCoordinator::new(limits);
    let (candidate_hash, verify, candidate) = begin_candidate(
        &mut coordinator,
        153,
        CoordinatorSource::Local,
        HashSet::from([input(153)]),
        200,
    );
    coordinator
        .admit_raw(
            hash(154),
            short(154),
            Raw("remote filler"),
            RawStage::Resolve,
            Some(47.into()),
            40,
            HashSet::new(),
        )
        .unwrap();
    let (version, evicted) = coordinator
        .complete_verification_candidate(&verify, Verified("candidate"), 40, candidate)
        .unwrap();
    assert_eq!(evicted.len(), 1);
    assert_eq!(evicted[0].hash, hash(154));
    assert_eq!(coordinator.usage(), CoordinatorResidency::new(1, 50));

    coordinator
        .admit_raw(
            hash(155),
            short(155),
            Raw("remote filler"),
            RawStage::Resolve,
            Some(47.into()),
            10,
            HashSet::new(),
        )
        .unwrap();
    let (_, evicted) = coordinator
        .wait_for_pool_inputs(&candidate_hash, version, HashSet::from([input(156)]))
        .unwrap();
    assert_eq!(evicted.len(), 1);
    assert_eq!(evicted[0].hash, hash(155));
    assert_eq!(coordinator.usage(), CoordinatorResidency::new(1, 60));
    coordinator.audit().unwrap();
}

#[test]
fn global_recharge_reconciliation_rolls_back_after_every_apply_boundary() {
    for fault_step in 1..=3 {
        let limits = test_limits(CoordinatorResidency::new(2, 25), None, 4, 4);
        let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> =
            PipelineCoordinator::new(limits);
        let local = hash(157);
        let remote = hash(158);
        coordinator
            .admit_raw_sourced(
                local.clone(),
                short(157),
                Raw("local"),
                RawStage::Resolve,
                CoordinatorSource::Local,
                None,
                10,
                HashSet::new(),
            )
            .unwrap();
        coordinator
            .admit_raw(
                remote.clone(),
                short(158),
                Raw("remote"),
                RawStage::Resolve,
                Some(48.into()),
                10,
                HashSet::new(),
            )
            .unwrap();
        let lease = coordinator
            .checkout_raw(RawStage::Resolve)
            .unwrap()
            .unwrap();
        let before = [local.clone(), remote.clone()].map(|hash| coordinator.view(&hash).unwrap());
        let usage = coordinator.usage();

        coordinator.set_apply_fault_for_test(Some(fault_step));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = coordinator.complete_raw(
                &lease,
                Unverified("resolved"),
                20,
                VerifySchedule::default(),
            );
        }));
        assert!(result.is_err(), "fault step {fault_step} was not reached");
        coordinator.set_apply_fault_for_test(None);

        let after = [local, remote].map(|hash| coordinator.view(&hash).unwrap());
        assert_eq!(after, before);
        assert_eq!(coordinator.usage(), usage);
        coordinator.audit().unwrap();
    }
}

#[test]
fn global_conflict_edge_capacity_reconciles_disjoint_lower_priority_work() {
    let limits =
        test_limits(CoordinatorResidency::new(4, 400), None, 4, 4).with_conflict_limits(1, 4, 1);
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> =
        PipelineCoordinator::new(limits);
    let (remote, verify, candidate) = begin_candidate(
        &mut coordinator,
        159,
        CoordinatorSource::Remote(49.into()),
        HashSet::from([input(159)]),
        100,
    );
    coordinator
        .complete_verification_candidate(&verify, Verified("remote"), 30, candidate)
        .unwrap();

    let (local, verify, candidate) = begin_candidate(
        &mut coordinator,
        160,
        CoordinatorSource::Local,
        HashSet::from([input(160)]),
        100,
    );
    let (_, evicted) = coordinator
        .complete_verification_candidate(&verify, Verified("local"), 30, candidate)
        .unwrap();
    assert_eq!(evicted.len(), 1);
    assert_eq!(evicted[0].hash, remote);
    assert_eq!(evicted[0].disposition, TerminalDisposition::CapacityEvicted);
    assert!(coordinator.view(&local).is_some());
    assert_eq!(coordinator.conflict_edge_count(), 1);
    coordinator.audit().unwrap();
}

#[test]
fn global_pool_input_edge_capacity_reconciles_disjoint_lower_priority_work() {
    let limits = test_limits(CoordinatorResidency::new(4, 400), None, 4, 4)
        .with_conflict_limits(1, 4, 4)
        .with_pool_input_limits(1, 4, 1);
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> =
        PipelineCoordinator::new(limits);
    let (remote, verify, candidate) = begin_candidate(
        &mut coordinator,
        161,
        CoordinatorSource::Remote(50.into()),
        HashSet::from([input(161)]),
        100,
    );
    let remote_version = coordinator
        .complete_verification_candidate(&verify, Verified("remote"), 30, candidate)
        .unwrap()
        .0;
    coordinator
        .wait_for_pool_inputs(&remote, remote_version, HashSet::from([input(201)]))
        .unwrap();

    let (local, verify, candidate) = begin_candidate(
        &mut coordinator,
        162,
        CoordinatorSource::Local,
        HashSet::from([input(162)]),
        100,
    );
    let local_version = coordinator
        .complete_verification_candidate(&verify, Verified("local"), 30, candidate)
        .unwrap()
        .0;
    let (_, evicted) = coordinator
        .wait_for_pool_inputs(&local, local_version, HashSet::from([input(202)]))
        .unwrap();
    assert_eq!(evicted.len(), 1);
    assert_eq!(evicted[0].hash, remote);
    assert_eq!(evicted[0].disposition, TerminalDisposition::CapacityEvicted);
    assert!(matches!(
        coordinator.view(&local).unwrap().location,
        CoordinatorLocation::WaitingPoolInputs { .. }
    ));
    coordinator.audit().unwrap();
}

#[test]
fn capacity_reconciliation_never_selects_an_incoming_dependency_ancestor() {
    let limits =
        test_limits(CoordinatorResidency::new(4, 400), None, 4, 4).with_conflict_limits(1, 4, 1);
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> =
        PipelineCoordinator::new(limits);
    let (ancestor, verify, candidate) = begin_candidate(
        &mut coordinator,
        163,
        CoordinatorSource::Remote(51.into()),
        HashSet::from([input(163)]),
        100,
    );
    coordinator
        .complete_verification_candidate(&verify, Verified("ancestor"), 30, candidate)
        .unwrap();

    let child = hash(164);
    coordinator
        .admit_raw_sourced(
            child.clone(),
            short(164),
            Raw("child"),
            RawStage::PreCheck,
            CoordinatorSource::Local,
            None,
            10,
            HashSet::from([ancestor.clone()]),
        )
        .unwrap();
    let raw = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    coordinator
        .complete_raw(&raw, Unverified("child"), 20, VerifySchedule::default())
        .unwrap();
    let verify = coordinator
        .checkout_verify(WorkerCapability::Any)
        .unwrap()
        .unwrap();
    let candidate = CoordinatorFeeGate::new(0, 0)
        .validate(child.clone(), HashSet::from([input(164)]), 200, 100)
        .unwrap();
    let before = coordinator.view(&child).unwrap();
    assert!(matches!(
        coordinator.complete_verification_candidate(&verify, Verified("child"), 30, candidate),
        Err(CoordinatorError::ConflictEdgeLimitExceeded)
    ));
    assert_eq!(coordinator.view(&child).unwrap(), before);
    assert!(coordinator.view(&ancestor).is_some());
    coordinator.audit().unwrap();
}

#[test]
fn capacity_planning_limits_ancestor_walk_and_victim_batch_size() {
    let chain_limits = test_limits(CoordinatorResidency::new(5, 500), None, 4, 4)
        .with_capacity_reconciliation_limits(2, 4);
    let mut chain: PipelineCoordinator<Raw, Unverified, Verified> =
        PipelineCoordinator::new(chain_limits);
    chain
        .admit_raw(
            hash(165),
            short(165),
            Raw("root"),
            RawStage::Resolve,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    chain
        .admit_raw(
            hash(166),
            short(166),
            Raw("middle"),
            RawStage::Resolve,
            None,
            10,
            HashSet::from([hash(165)]),
        )
        .unwrap();
    chain
        .admit_raw(
            hash(167),
            short(167),
            Raw("leaf"),
            RawStage::Resolve,
            None,
            10,
            HashSet::from([hash(166)]),
        )
        .unwrap();
    assert!(matches!(
        chain.admit_raw(
            hash(168),
            short(168),
            Raw("too deep"),
            RawStage::Resolve,
            None,
            10,
            HashSet::from([hash(167)]),
        ),
        Err(CoordinatorError::DependencyAncestorLimitExceeded)
    ));
    assert!(chain.view(&hash(168)).is_none());
    chain.audit().unwrap();

    let victim_limits = test_limits(CoordinatorResidency::new(3, 25), None, 4, 4)
        .with_capacity_reconciliation_limits(4, 1);
    let mut victims: PipelineCoordinator<Raw, Unverified, Verified> =
        PipelineCoordinator::new(victim_limits);
    for seed in [169, 170] {
        victims
            .admit_raw(
                hash(seed),
                short(seed),
                Raw("remote"),
                RawStage::Resolve,
                Some(52.into()),
                10,
                HashSet::new(),
            )
            .unwrap();
    }
    assert!(matches!(
        victims.admit_raw_sourced(
            hash(171),
            short(171),
            Raw("large local"),
            RawStage::Resolve,
            CoordinatorSource::Local,
            None,
            20,
            HashSet::new(),
        ),
        Err(CoordinatorError::CapacityEvictionLimitExceeded)
    ));
    assert!(victims.view(&hash(169)).is_some());
    assert!(victims.view(&hash(170)).is_some());
    assert!(victims.view(&hash(171)).is_none());
    victims.audit().unwrap();
}

#[test]
fn impossible_peer_budget_fails_before_global_capacity_reconciliation() {
    let limits = test_limits(
        CoordinatorResidency::new(2, 20),
        Some(CoordinatorResidency::new(1, 15)),
        4,
        4,
    );
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> =
        PipelineCoordinator::new(limits);
    for (seed, peer) in [(172, 53), (173, 54)] {
        coordinator
            .admit_raw(
                hash(seed),
                short(seed),
                Raw("remote"),
                RawStage::Resolve,
                Some(PeerIndex::from(peer)),
                10,
                HashSet::new(),
            )
            .unwrap();
    }
    assert!(matches!(
        coordinator.admit_raw(
            hash(174),
            short(174),
            Raw("same peer"),
            RawStage::Resolve,
            Some(PeerIndex::from(53)),
            10,
            HashSet::new(),
        ),
        Err(CoordinatorError::PeerBudgetExceeded(peer)) if peer == PeerIndex::from(53)
    ));
    assert_eq!(coordinator.len(), 2);
    coordinator.audit().unwrap();
}

#[test]
fn failed_phase_recharge_leaves_payload_location_and_queue_unchanged() {
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> =
        PipelineCoordinator::new(test_limits(CoordinatorResidency::new(1, 15), None, 4, 4));
    let tx_hash = hash(11);
    coordinator
        .admit_raw(
            tx_hash.clone(),
            short(11),
            Raw("raw"),
            RawStage::PreCheck,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    let lease = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();

    assert!(matches!(
        coordinator.complete_raw(
            &lease,
            Unverified("too large"),
            16,
            VerifySchedule::default()
        ),
        Err(CoordinatorError::GlobalBudgetExceeded)
    ));
    let view = coordinator.view(&tx_hash).unwrap();
    assert_eq!(view.phase, PayloadPhase::Raw);
    assert_eq!(
        view.location,
        CoordinatorLocation::RawActive(RawStage::PreCheck)
    );
    assert_eq!(view.charge_bytes, 10);
    assert_eq!(coordinator.queue_len(QueueKind::Verify), 0);
    coordinator.audit().unwrap();
}

#[test]
fn abort_commit_requeues_once_and_makes_the_old_commit_lease_stale() {
    let mut coordinator = roomy();
    let tx_hash = hash(12);
    coordinator
        .admit_raw(
            tx_hash.clone(),
            short(12),
            Raw("raw"),
            RawStage::PreCheck,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    let raw = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    coordinator
        .complete_raw(&raw, Unverified("resolved"), 20, VerifySchedule::default())
        .unwrap();
    let verify = coordinator
        .checkout_verify(WorkerCapability::Any)
        .unwrap()
        .unwrap();
    coordinator
        .complete_verification(&verify, Verified("proof"), 30)
        .unwrap();
    let old_commit = coordinator.begin_next_commit().unwrap().unwrap();
    coordinator.abort_commit(&old_commit).unwrap();

    assert!(matches!(
        coordinator.commit_handoff(&old_commit),
        Err(CoordinatorError::RevisionMismatch { .. })
    ));
    assert_eq!(coordinator.queue_len(QueueKind::Commit), 1);
    let new_commit = coordinator.begin_next_commit().unwrap().unwrap();
    assert_ne!(old_commit.version, new_commit.version);
    coordinator.audit().unwrap();
}

#[test]
fn unverified_high_fee_work_cannot_own_or_preempt_a_conflict_domain() {
    let mut coordinator = roomy();
    let contested = input(1);
    let verified = verify_candidate(
        &mut coordinator,
        20,
        HashSet::from([contested.clone()]),
        100,
    );
    assert_eq!(
        coordinator.active_conflict_owner(&contested),
        Some(&verified)
    );

    let unverified_hash = hash(21);
    coordinator
        .admit_raw(
            unverified_hash.clone(),
            short(21),
            Raw("raw"),
            RawStage::PreCheck,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    let raw = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    coordinator
        .complete_raw(
            &raw,
            Unverified("unverified high fee"),
            20,
            VerifySchedule::default(),
        )
        .unwrap();

    assert_eq!(
        coordinator.active_conflict_owner(&contested),
        Some(&verified)
    );
    assert_eq!(
        coordinator.view(&unverified_hash).unwrap().location,
        CoordinatorLocation::VerifyQueued
    );
    coordinator.audit().unwrap();
}

#[test]
fn under_fee_candidate_cannot_become_verified_conflict_state() {
    let mut coordinator = roomy();
    let contested = input(9);
    let owner = verify_candidate(
        &mut coordinator,
        33,
        HashSet::from([contested.clone()]),
        1_000,
    );
    let candidate_hash = hash(34);
    coordinator
        .admit_raw(
            candidate_hash.clone(),
            short(34),
            Raw("raw"),
            RawStage::PreCheck,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    let raw = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    coordinator
        .complete_raw(&raw, Unverified("resolved"), 20, VerifySchedule::default())
        .unwrap();
    let _verify = coordinator
        .checkout_verify(WorkerCapability::Any)
        .unwrap()
        .unwrap();

    assert_eq!(
        CoordinatorFeeGate::new(2_000, 0).validate(
            candidate_hash.clone(),
            HashSet::from([contested.clone()]),
            1_999,
            100,
        ),
        Err(CoordinatorError::UnderReplacementFee {
            hash: candidate_hash.clone(),
            required: 2_000,
            actual: 1_999,
        })
    );
    assert_eq!(coordinator.active_conflict_owner(&contested), Some(&owner));
    assert_eq!(
        coordinator.view(&candidate_hash).unwrap().phase,
        PayloadPhase::Unverified
    );
    assert_eq!(coordinator.conflict_edge_count(), 1);
    coordinator.audit().unwrap();
}

#[test]
fn higher_verified_candidate_preempts_and_removal_rechecks_the_loser() {
    let mut coordinator = roomy();
    let contested = input(2);
    let low = verify_candidate(
        &mut coordinator,
        22,
        HashSet::from([contested.clone()]),
        100,
    );
    let high = verify_candidate(
        &mut coordinator,
        23,
        HashSet::from([contested.clone()]),
        200,
    );

    assert_eq!(coordinator.active_conflict_owner(&contested), Some(&high));
    assert!(matches!(
        coordinator.view(&low).unwrap().location,
        CoordinatorLocation::WaitingConflict { ref blockers }
            if blockers == &HashSet::from([high.clone()])
    ));
    coordinator.audit().unwrap();

    coordinator
        .force_terminalize(&high, TerminalDisposition::Rejected)
        .unwrap();
    assert_eq!(
        coordinator.view(&low).unwrap().location,
        CoordinatorLocation::ConflictRecheck
    );
    assert_eq!(coordinator.conflict_recheck_len(), 1);
    let activated = coordinator.drain_conflict_rechecks(1).unwrap();
    assert_eq!(activated.len(), 1);
    assert_eq!(coordinator.active_conflict_owner(&contested), Some(&low));
    assert_eq!(
        coordinator.view(&low).unwrap().location,
        CoordinatorLocation::ReadyToCommit
    );
    coordinator.audit().unwrap();
}

#[test]
fn verified_conflict_preemption_rolls_back_at_every_apply_boundary() {
    let mut coordinator = roomy();
    let contested = input(233);
    let low = verify_candidate(
        &mut coordinator,
        233,
        HashSet::from([contested.clone()]),
        100,
    );
    let high = hash(234);
    coordinator
        .admit_raw(
            high.clone(),
            short(234),
            Raw("high"),
            RawStage::PreCheck,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    let raw = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    coordinator
        .complete_raw(&raw, Unverified("high"), 20, VerifySchedule::default())
        .unwrap();
    let verify = coordinator
        .checkout_verify(WorkerCapability::Any)
        .unwrap()
        .unwrap();
    let before = [low.clone(), high.clone()].map(|hash| coordinator.view(&hash).unwrap());
    let usage = coordinator.usage();
    let active_work = coordinator.active_work();

    for fault_step in 1..=3 {
        let candidate = CoordinatorFeeGate::new(0, 0)
            .validate(high.clone(), HashSet::from([contested.clone()]), 200, 100)
            .unwrap();
        coordinator.set_apply_fault_for_test(Some(fault_step));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = coordinator.complete_verification_candidate(
                &verify,
                Verified("high"),
                30,
                candidate,
            );
        }));
        assert!(result.is_err(), "fault step {fault_step} was not reached");
        coordinator.set_apply_fault_for_test(None);
        let after = [low.clone(), high.clone()].map(|hash| coordinator.view(&hash).unwrap());
        assert_eq!(after, before);
        assert_eq!(coordinator.usage(), usage);
        assert_eq!(coordinator.active_work(), active_work);
        assert_eq!(coordinator.active_conflict_owner(&contested), Some(&low));
        coordinator.audit().unwrap();
    }

    let candidate = CoordinatorFeeGate::new(0, 0)
        .validate(high.clone(), HashSet::from([contested.clone()]), 200, 100)
        .unwrap();
    coordinator
        .complete_verification_candidate(&verify, Verified("high"), 30, candidate)
        .unwrap();
    assert_eq!(coordinator.active_conflict_owner(&contested), Some(&high));
    coordinator.audit().unwrap();
}

#[test]
fn exact_conflict_score_tie_keeps_the_earlier_verified_candidate() {
    let mut coordinator = roomy();
    let shared = input(210);
    let first = verify_candidate(&mut coordinator, 210, HashSet::from([shared.clone()]), 100);
    let second = verify_candidate(&mut coordinator, 211, HashSet::from([shared.clone()]), 100);
    assert_eq!(coordinator.active_conflict_owner(&shared), Some(&first));
    assert!(matches!(
        coordinator.view(&second).unwrap().location,
        CoordinatorLocation::WaitingConflict { .. }
    ));
    coordinator.audit().unwrap();
}

#[test]
fn conflict_preemption_never_disturbs_an_independent_input_domain() {
    let mut coordinator = roomy();
    let contested = input(212);
    let independent = input(213);
    let weak = verify_candidate(
        &mut coordinator,
        212,
        HashSet::from([contested.clone()]),
        100,
    );
    let other = verify_candidate(
        &mut coordinator,
        213,
        HashSet::from([independent.clone()]),
        100,
    );
    let strong = verify_candidate(
        &mut coordinator,
        214,
        HashSet::from([contested.clone()]),
        200,
    );
    assert_eq!(coordinator.active_conflict_owner(&contested), Some(&strong));
    assert_eq!(
        coordinator.active_conflict_owner(&independent),
        Some(&other)
    );
    assert!(matches!(
        coordinator.view(&weak).unwrap().location,
        CoordinatorLocation::WaitingConflict { .. }
    ));
    coordinator.audit().unwrap();
}

#[test]
fn multi_input_verified_candidate_is_all_or_none_and_committing_is_frozen() {
    let mut coordinator = roomy();
    let left_input = input(3);
    let right_input = input(4);
    let left = verify_candidate(
        &mut coordinator,
        24,
        HashSet::from([left_input.clone()]),
        100,
    );
    let right = verify_candidate(
        &mut coordinator,
        25,
        HashSet::from([right_input.clone()]),
        100,
    );
    let both = verify_candidate(
        &mut coordinator,
        26,
        HashSet::from([left_input.clone(), right_input.clone()]),
        200,
    );

    assert_eq!(coordinator.active_conflict_owner(&left_input), Some(&both));
    assert_eq!(coordinator.active_conflict_owner(&right_input), Some(&both));
    for loser in [&left, &right] {
        assert!(matches!(
            coordinator.view(loser).unwrap().location,
            CoordinatorLocation::WaitingConflict { ref blockers }
                if blockers == &HashSet::from([both.clone()])
        ));
    }

    let committing = coordinator.begin_next_commit().unwrap().unwrap();
    assert_eq!(committing.hash, both);
    let later = verify_candidate(
        &mut coordinator,
        27,
        HashSet::from([left_input.clone(), right_input.clone()]),
        300,
    );
    assert!(matches!(
        coordinator.view(&later).unwrap().location,
        CoordinatorLocation::WaitingConflict { ref blockers }
            if blockers == &HashSet::from([committing.hash.clone()])
    ));
    assert_eq!(
        coordinator.active_conflict_owner(&left_input),
        Some(&committing.hash)
    );
    assert_eq!(
        coordinator.active_conflict_owner(&right_input),
        Some(&committing.hash)
    );
    coordinator.audit().unwrap();
}

#[test]
fn preempted_blockers_move_their_old_waiters_to_bounded_recheck_work() {
    let mut coordinator = roomy();
    let contested = input(5);
    let middle = verify_candidate(
        &mut coordinator,
        28,
        HashSet::from([contested.clone()]),
        200,
    );
    let low = verify_candidate(
        &mut coordinator,
        29,
        HashSet::from([contested.clone()]),
        100,
    );
    assert!(matches!(
        coordinator.view(&low).unwrap().location,
        CoordinatorLocation::WaitingConflict { .. }
    ));

    let high = verify_candidate(
        &mut coordinator,
        30,
        HashSet::from([contested.clone()]),
        300,
    );
    assert_eq!(coordinator.active_conflict_owner(&contested), Some(&high));
    assert!(matches!(
        coordinator.view(&middle).unwrap().location,
        CoordinatorLocation::WaitingConflict { .. }
    ));
    assert_eq!(
        coordinator.view(&low).unwrap().location,
        CoordinatorLocation::ConflictRecheck
    );
    assert_eq!(coordinator.conflict_recheck_len(), 1);
    coordinator.audit().unwrap();

    assert!(coordinator.drain_conflict_rechecks(1).unwrap().is_empty());
    assert!(matches!(
        coordinator.view(&low).unwrap().location,
        CoordinatorLocation::WaitingConflict { ref blockers }
            if blockers == &HashSet::from([high])
    ));
    coordinator.audit().unwrap();
}

#[test]
fn injected_conflict_recheck_unwind_restores_the_entire_preemption() {
    let mut coordinator = roomy();
    let contested = input(215);
    let middle = verify_candidate(
        &mut coordinator,
        215,
        HashSet::from([contested.clone()]),
        200,
    );
    let low = verify_candidate(
        &mut coordinator,
        216,
        HashSet::from([contested.clone()]),
        100,
    );
    let high = verify_candidate(
        &mut coordinator,
        217,
        HashSet::from([contested.clone()]),
        300,
    );
    coordinator
        .force_terminalize(&high, TerminalDisposition::Rejected)
        .unwrap();
    let weak = verify_candidate(
        &mut coordinator,
        218,
        HashSet::from([contested.clone()]),
        50,
    );
    assert_eq!(coordinator.conflict_recheck_len(), 2);
    assert_eq!(coordinator.active_conflict_owner(&contested), Some(&weak));
    let before =
        [low.clone(), middle.clone(), weak.clone()].map(|hash| coordinator.view(&hash).unwrap());
    let usage = coordinator.usage();

    for fault_step in 1..=3 {
        coordinator.set_apply_fault_for_test(Some(fault_step));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = coordinator.drain_conflict_rechecks(1);
        }));
        assert!(result.is_err(), "fault step {fault_step} was not reached");
        coordinator.set_apply_fault_for_test(None);

        let after = [low.clone(), middle.clone(), weak.clone()]
            .map(|hash| coordinator.view(&hash).unwrap());
        assert_eq!(after, before);
        assert_eq!(coordinator.usage(), usage);
        assert_eq!(coordinator.conflict_recheck_len(), 2);
        assert_eq!(coordinator.active_conflict_owner(&contested), Some(&weak));
        assert_eq!(
            coordinator.physical_queue_slots_for_test(QueueKind::Commit),
            coordinator.queue_len(QueueKind::Commit)
        );
        coordinator.audit().unwrap();
    }

    assert_eq!(coordinator.drain_conflict_rechecks(1).unwrap().len(), 1);
    assert_eq!(coordinator.active_conflict_owner(&contested), Some(&low));
    assert!(matches!(
        coordinator.view(&weak).unwrap().location,
        CoordinatorLocation::WaitingConflict { .. }
    ));
    coordinator.audit().unwrap();
}

#[test]
fn conflict_recheck_batch_rolls_back_earlier_preemptions_on_late_unwind() {
    let mut coordinator = roomy();
    let contested = input(224);
    let middle = verify_candidate(
        &mut coordinator,
        224,
        HashSet::from([contested.clone()]),
        200,
    );
    let low = verify_candidate(
        &mut coordinator,
        225,
        HashSet::from([contested.clone()]),
        100,
    );
    let high = verify_candidate(
        &mut coordinator,
        226,
        HashSet::from([contested.clone()]),
        300,
    );
    coordinator
        .force_terminalize(&high, TerminalDisposition::Rejected)
        .unwrap();
    let weak = verify_candidate(
        &mut coordinator,
        227,
        HashSet::from([contested.clone()]),
        50,
    );
    let before =
        [low.clone(), middle.clone(), weak.clone()].map(|hash| coordinator.view(&hash).unwrap());
    let usage = coordinator.usage();
    assert_eq!(coordinator.conflict_recheck_len(), 2);
    assert_eq!(coordinator.active_conflict_owner(&contested), Some(&weak));

    coordinator.set_apply_fault_for_test(Some(4));
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _ = coordinator.drain_conflict_rechecks(2);
    }));
    assert!(result.is_err());
    coordinator.set_apply_fault_for_test(None);

    let after =
        [low.clone(), middle.clone(), weak.clone()].map(|hash| coordinator.view(&hash).unwrap());
    assert_eq!(after, before);
    assert_eq!(coordinator.usage(), usage);
    assert_eq!(coordinator.conflict_recheck_len(), 2);
    assert_eq!(coordinator.active_conflict_owner(&contested), Some(&weak));
    coordinator.audit().unwrap();

    assert_eq!(coordinator.drain_conflict_rechecks(2).unwrap().len(), 2);
    assert_eq!(coordinator.active_conflict_owner(&contested), Some(&middle));
    coordinator.audit().unwrap();
}

#[test]
fn conflict_maintenance_rebuild_preserves_authoritative_enqueue_order() {
    let mut coordinator = roomy();
    let earlier_input = input(212);
    let later_input = input(213);
    let earlier_owner = verify_candidate(
        &mut coordinator,
        10,
        HashSet::from([earlier_input.clone()]),
        200,
    );
    let earlier_waiter =
        verify_candidate(&mut coordinator, 11, HashSet::from([earlier_input]), 100);
    let later_owner = verify_candidate(
        &mut coordinator,
        12,
        HashSet::from([later_input.clone()]),
        200,
    );
    let later_waiter = verify_candidate(&mut coordinator, 13, HashSet::from([later_input]), 100);

    coordinator
        .force_terminalize(&later_owner, TerminalDisposition::Rejected)
        .unwrap();
    coordinator
        .force_terminalize(&earlier_owner, TerminalDisposition::Rejected)
        .unwrap();
    assert_eq!(coordinator.conflict_recheck_len(), 2);

    coordinator.set_apply_fault_for_test(Some(1));
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _ = coordinator.drain_conflict_rechecks(2);
    }));
    assert!(result.is_err());
    coordinator.set_apply_fault_for_test(None);
    coordinator.audit().unwrap();

    let first = coordinator.drain_conflict_rechecks(1).unwrap();
    assert_eq!(first[0].hash, later_waiter);
    let second = coordinator.drain_conflict_rechecks(1).unwrap();
    assert_eq!(second[0].hash, earlier_waiter);
    coordinator.audit().unwrap();
}

#[test]
fn conflict_limits_fail_before_verified_state_or_indexes_change() {
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> = PipelineCoordinator::new(
        test_limits(CoordinatorResidency::new(10, 1_000), None, 4, 4).with_conflict_limits(1, 1, 1),
    );
    let first = verify_candidate(&mut coordinator, 31, inputs([6]), 100);
    assert_eq!(coordinator.conflict_edge_count(), 1);

    let second_hash = hash(32);
    coordinator
        .admit_raw(
            second_hash.clone(),
            short(32),
            Raw("raw"),
            RawStage::PreCheck,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    let raw = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    coordinator
        .complete_raw(&raw, Unverified("resolved"), 20, VerifySchedule::default())
        .unwrap();
    let verify = coordinator
        .checkout_verify(WorkerCapability::Any)
        .unwrap()
        .unwrap();
    let candidate = CoordinatorFeeGate::new(0, 0)
        .validate(second_hash.clone(), inputs([6]), 100, 100)
        .unwrap();
    assert!(matches!(
        coordinator.complete_verification_candidate(&verify, Verified("proof"), 30, candidate),
        Err(CoordinatorError::ConflictEdgeLimitExceeded)
            | Err(CoordinatorError::ConflictCandidateLimitExceeded(_))
    ));

    let view = coordinator.view(&second_hash).unwrap();
    assert_eq!(view.phase, PayloadPhase::Unverified);
    assert_eq!(view.location, CoordinatorLocation::VerifyActive);
    assert_eq!(coordinator.active_conflict_owner(&input(6)), Some(&first));
    assert_eq!(coordinator.conflict_edge_count(), 1);
    coordinator.audit().unwrap();
}

#[test]
fn stronger_verified_candidate_reconciles_every_full_input_bucket_atomically() {
    let limits = test_limits(CoordinatorResidency::new(20, 20_000), None, 4, 4)
        .with_conflict_limits(2, 1, 4);
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> =
        PipelineCoordinator::new(limits);
    let first_input = input(130);
    let second_input = input(131);
    let first = verify_candidate(
        &mut coordinator,
        130,
        HashSet::from([first_input.clone()]),
        100,
    );
    let second = verify_candidate(
        &mut coordinator,
        131,
        HashSet::from([second_input.clone()]),
        100,
    );
    let (strong, verify, candidate) = begin_candidate(
        &mut coordinator,
        132,
        CoordinatorSource::Local,
        HashSet::from([first_input.clone(), second_input.clone()]),
        300,
    );

    let (_, evicted) = coordinator
        .complete_verification_candidate(&verify, Verified("strong"), 30, candidate)
        .unwrap();
    let evicted_hashes: Vec<_> = evicted.iter().map(|record| record.hash.clone()).collect();
    assert_eq!(evicted_hashes, vec![first, second]);
    assert!(
        evicted
            .iter()
            .all(|record| record.disposition == TerminalDisposition::CapacityEvicted)
    );
    assert_eq!(
        coordinator.active_conflict_owner(&first_input),
        Some(&strong)
    );
    assert_eq!(
        coordinator.active_conflict_owner(&second_input),
        Some(&strong)
    );
    assert_eq!(coordinator.conflict_edge_count(), 2);
    coordinator.audit().unwrap();
}

#[test]
fn source_priority_protects_verified_reconciliation_capacity() {
    let limits = test_limits(
        CoordinatorResidency::new(10, 10_000),
        Some(CoordinatorResidency::new(10, 10_000)),
        4,
        4,
    )
    .with_conflict_limits(1, 1, 2);
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> =
        PipelineCoordinator::new(limits);
    let contested = input(133);
    let (remote, verify, candidate) = begin_candidate(
        &mut coordinator,
        133,
        CoordinatorSource::Remote(43.into()),
        HashSet::from([contested.clone()]),
        300,
    );
    coordinator
        .complete_verification_candidate(&verify, Verified("remote"), 30, candidate)
        .unwrap();
    let (local, verify, candidate) = begin_candidate(
        &mut coordinator,
        134,
        CoordinatorSource::Local,
        HashSet::from([contested.clone()]),
        100,
    );
    let (_, evicted) = coordinator
        .complete_verification_candidate(&verify, Verified("local"), 30, candidate)
        .unwrap();
    assert_eq!(evicted.len(), 1);
    assert_eq!(evicted[0].hash, remote);
    assert_eq!(coordinator.active_conflict_owner(&contested), Some(&local));
    coordinator.audit().unwrap();
}

#[test]
fn committing_candidate_cannot_be_capacity_evicted() {
    let limits = test_limits(CoordinatorResidency::new(10, 10_000), None, 4, 4)
        .with_conflict_limits(1, 1, 2);
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> =
        PipelineCoordinator::new(limits);
    let contested = input(135);
    let owner = verify_candidate(
        &mut coordinator,
        135,
        HashSet::from([contested.clone()]),
        100,
    );
    let commit = coordinator.begin_next_commit().unwrap().unwrap();
    assert_eq!(commit.hash, owner);
    let (_, verify, candidate) = begin_candidate(
        &mut coordinator,
        136,
        CoordinatorSource::Proposal,
        HashSet::from([contested.clone()]),
        1_000,
    );
    assert!(matches!(
        coordinator.complete_verification_candidate(
            &verify,
            Verified("proposal"),
            30,
            candidate,
        ),
        Err(CoordinatorError::ConflictCandidateLimitExceeded(input)) if input == contested
    ));
    assert_eq!(coordinator.active_conflict_owner(&contested), Some(&owner));
    coordinator.audit().unwrap();
}

#[test]
fn conflict_capacity_reconciliation_rolls_back_every_apply_boundary() {
    for fault_step in 1..=5 {
        let limits = test_limits(CoordinatorResidency::new(10, 10_000), None, 4, 4)
            .with_conflict_limits(1, 1, 2);
        let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> =
            PipelineCoordinator::new(limits);
        let contested = input(137);
        let owner = verify_candidate(
            &mut coordinator,
            137,
            HashSet::from([contested.clone()]),
            100,
        );
        let (strong, verify, candidate) = begin_candidate(
            &mut coordinator,
            138,
            CoordinatorSource::Local,
            HashSet::from([contested.clone()]),
            200,
        );
        let before = [owner.clone(), strong.clone()].map(|hash| coordinator.view(&hash).unwrap());
        let usage = coordinator.usage();

        coordinator.set_apply_fault_for_test(Some(fault_step));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = coordinator.complete_verification_candidate(
                &verify,
                Verified("strong"),
                30,
                candidate,
            );
        }));
        assert!(result.is_err(), "fault step {fault_step} was not reached");
        coordinator.set_apply_fault_for_test(None);

        let after = [owner.clone(), strong].map(|hash| coordinator.view(&hash).unwrap());
        assert_eq!(after, before);
        assert_eq!(coordinator.usage(), usage);
        assert_eq!(coordinator.active_conflict_owner(&contested), Some(&owner));
        coordinator.audit().unwrap();
    }
}

#[test]
fn waiter_revision_exhaustion_cannot_half_remove_its_active_blocker() {
    let mut coordinator = roomy();
    let contested = input(10);
    let owner = verify_candidate(
        &mut coordinator,
        35,
        HashSet::from([contested.clone()]),
        200,
    );
    let waiter = verify_candidate(
        &mut coordinator,
        36,
        HashSet::from([contested.clone()]),
        100,
    );
    coordinator
        .set_revision_for_test(&waiter, u64::MAX)
        .unwrap();

    assert!(matches!(
        coordinator.force_terminalize(&owner, TerminalDisposition::Rejected),
        Err(CoordinatorError::RevisionExhausted(hash)) if hash == waiter
    ));
    assert_eq!(coordinator.active_conflict_owner(&contested), Some(&owner));
    assert_eq!(
        coordinator.view(&owner).unwrap().location,
        CoordinatorLocation::ReadyToCommit
    );
    assert!(matches!(
        coordinator.view(&waiter).unwrap().location,
        CoordinatorLocation::WaitingConflict { ref blockers }
            if blockers == &HashSet::from([owner])
    ));
    assert_eq!(coordinator.queue_len(QueueKind::Commit), 1);
    coordinator.audit().unwrap();
}

#[test]
fn every_conflict_owner_removal_apply_boundary_rolls_back_atomically() {
    let contested = input(219);
    for fault_step in 1..=3 {
        let mut coordinator = roomy();
        let owner = verify_candidate(
            &mut coordinator,
            219,
            HashSet::from([contested.clone()]),
            200,
        );
        let waiter = verify_candidate(
            &mut coordinator,
            220,
            HashSet::from([contested.clone()]),
            100,
        );
        let before = [owner.clone(), waiter.clone()].map(|hash| coordinator.view(&hash).unwrap());
        let usage = coordinator.usage();
        let commit_len = coordinator.queue_len(QueueKind::Commit);
        let recheck_len = coordinator.conflict_recheck_len();

        coordinator.set_apply_fault_for_test(Some(fault_step));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = coordinator.force_terminalize(&owner, TerminalDisposition::Rejected);
        }));
        assert!(result.is_err(), "fault step {fault_step} was not reached");
        coordinator.set_apply_fault_for_test(None);

        let after = [owner.clone(), waiter].map(|hash| coordinator.view(&hash).unwrap());
        assert_eq!(after, before);
        assert_eq!(coordinator.usage(), usage);
        assert_eq!(coordinator.queue_len(QueueKind::Commit), commit_len);
        assert_eq!(coordinator.conflict_recheck_len(), recheck_len);
        assert_eq!(coordinator.active_conflict_owner(&contested), Some(&owner));
        coordinator.audit().unwrap();
    }
}

#[test]
fn successful_candidate_handoff_rejects_current_direct_cohort_only() {
    let mut coordinator = roomy();
    let contested = input(11);
    let independent_input = input(12);
    let winner = verify_candidate(
        &mut coordinator,
        37,
        HashSet::from([contested.clone()]),
        300,
    );
    let loser = verify_candidate(
        &mut coordinator,
        38,
        HashSet::from([contested.clone()]),
        100,
    );
    let independent = verify_candidate(
        &mut coordinator,
        39,
        HashSet::from([independent_input.clone()]),
        50,
    );
    let committing = coordinator.begin_next_commit().unwrap().unwrap();
    assert_eq!(committing.hash, winner);
    let late_loser = verify_candidate(
        &mut coordinator,
        40,
        HashSet::from([contested.clone()]),
        400,
    );
    assert!(matches!(
        coordinator.view(&late_loser).unwrap().location,
        CoordinatorLocation::WaitingConflict { .. }
    ));
    assert!(matches!(
        coordinator.commit_handoff(&committing),
        Err(CoordinatorError::ConflictInvariant)
    ));

    let before = [
        winner.clone(),
        loser.clone(),
        late_loser.clone(),
        independent.clone(),
    ]
    .map(|hash| coordinator.view(&hash).unwrap());
    let usage = coordinator.usage();
    let conflict_edges = coordinator.conflict_edge_count();
    for fault_step in 1..=9 {
        coordinator.set_apply_fault_for_test(Some(fault_step));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = coordinator.commit_candidate_handoff(&committing);
        }));
        assert!(result.is_err(), "fault step {fault_step} was not reached");
        coordinator.set_apply_fault_for_test(None);
        let after = [
            winner.clone(),
            loser.clone(),
            late_loser.clone(),
            independent.clone(),
        ]
        .map(|hash| coordinator.view(&hash).unwrap());
        assert_eq!(after, before);
        assert_eq!(coordinator.usage(), usage);
        assert_eq!(coordinator.conflict_edge_count(), conflict_edges);
        assert_eq!(coordinator.active_work(), 1);
        coordinator.audit().unwrap();
    }

    let handoff = coordinator.commit_candidate_handoff(&committing).unwrap();
    assert_eq!(handoff.winner.hash, winner);
    let rejected: HashSet<_> = handoff
        .rejected
        .into_iter()
        .map(|record| {
            assert_eq!(record.disposition, TerminalDisposition::Rejected);
            record.hash
        })
        .collect();
    assert_eq!(rejected, HashSet::from([loser, late_loser]));
    assert!(coordinator.view(&independent).is_some());
    assert_eq!(
        coordinator.active_conflict_owner(&independent_input),
        Some(&independent)
    );
    assert_eq!(coordinator.conflict_edge_count(), 1);
    coordinator.audit().unwrap();
}

#[test]
fn clear_is_one_batch_and_does_not_revise_conflict_waiters() {
    let mut coordinator = roomy();
    let contested = input(13);
    let owner = verify_candidate(
        &mut coordinator,
        41,
        HashSet::from([contested.clone()]),
        200,
    );
    let waiter = verify_candidate(&mut coordinator, 42, HashSet::from([contested]), 100);
    coordinator
        .set_revision_for_test(&waiter, u64::MAX)
        .unwrap();

    let before = [owner.clone(), waiter.clone()].map(|hash| coordinator.view(&hash).unwrap());
    coordinator.set_apply_fault_for_test(Some(1));
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _ = coordinator.clear();
    }));
    assert!(result.is_err());
    coordinator.set_apply_fault_for_test(None);
    let after = [owner, waiter.clone()].map(|hash| coordinator.view(&hash).unwrap());
    assert_eq!(after, before);
    coordinator.audit().unwrap();

    let cleared = coordinator.clear().unwrap();
    assert_eq!(cleared.len(), 2);
    assert!(
        cleared
            .iter()
            .all(|record| record.disposition == TerminalDisposition::Cleared)
    );
    assert!(coordinator.is_empty());
    assert_eq!(coordinator.usage(), CoordinatorResidency::default());
    assert_eq!(coordinator.conflict_edge_count(), 0);
    assert_eq!(coordinator.conflict_recheck_len(), 0);
    coordinator.audit().unwrap();
}

#[test]
fn every_definitive_parent_exit_invalidates_dependents_in_the_same_transition() {
    let mut coordinator = roomy();
    let parent = hash(243);
    let child = hash(244);
    coordinator
        .admit_raw(
            parent.clone(),
            short(243),
            Raw("parent"),
            RawStage::PreCheck,
            None,
            10,
            HashSet::new(),
        )
        .unwrap();
    coordinator
        .admit_raw(
            child.clone(),
            short(244),
            Raw("child"),
            RawStage::Resolve,
            None,
            10,
            set([parent.clone()]),
        )
        .unwrap();
    let child_lease = coordinator
        .checkout_raw(RawStage::Resolve)
        .unwrap()
        .unwrap();
    coordinator
        .wait_for_parents(&child_lease, set([parent.clone()]))
        .unwrap();

    let removed = coordinator
        .force_terminalize(&parent, TerminalDisposition::Removed)
        .unwrap()
        .unwrap();
    assert_eq!(removed.hash, parent);
    assert_eq!(coordinator.dependency_failure_len(), 1);
    assert!(matches!(
        coordinator.view(&child).unwrap().location,
        CoordinatorLocation::Invalidated { cause } if cause == parent
    ));
    let failed = coordinator.drain_dependency_failures(1).unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].hash, child);
    assert_eq!(failed[0].disposition, TerminalDisposition::DependencyFailed);
    coordinator.audit().unwrap();
}

#[test]
fn later_parent_unavailability_cannot_resurrect_an_invalidated_child() {
    let mut coordinator = roomy();
    let failed_parent = hash(239);
    let other_parent = hash(240);
    let child = hash(241);
    coordinator
        .admit_raw(
            child.clone(),
            short(241),
            Raw("child"),
            RawStage::Resolve,
            None,
            10,
            set([failed_parent.clone(), other_parent.clone()]),
        )
        .unwrap();

    coordinator.schedule_parent_failure(&failed_parent).unwrap();
    let invalidated = coordinator.view(&child).unwrap();
    assert!(matches!(
        invalidated.location,
        CoordinatorLocation::Invalidated { ref cause } if cause == &failed_parent
    ));

    assert!(
        coordinator
            .parent_unavailable(&other_parent)
            .unwrap()
            .is_empty()
    );
    assert_eq!(coordinator.view(&child).unwrap(), invalidated);
    assert_eq!(coordinator.dependency_failure_len(), 1);
    coordinator.audit().unwrap();
}

#[test]
fn accepted_parent_handoff_wakes_waiting_children_atomically() {
    let mut coordinator = roomy();
    let (parent, _) = verify_plain(&mut coordinator, 245);
    let child = hash(246);
    coordinator
        .admit_raw(
            child.clone(),
            short(246),
            Raw("child"),
            RawStage::Resolve,
            None,
            10,
            set([parent.clone()]),
        )
        .unwrap();
    let child_lease = coordinator
        .checkout_raw(RawStage::Resolve)
        .unwrap()
        .unwrap();
    coordinator
        .wait_for_parents(&child_lease, set([parent.clone()]))
        .unwrap();
    let commit = coordinator.begin_next_commit().unwrap().unwrap();

    let handoff = coordinator.commit_handoff(&commit).unwrap();
    assert_eq!(handoff.hash, parent);
    assert_eq!(handoff.ready_children.len(), 1);
    assert_eq!(handoff.ready_children[0].hash, child);
    assert_eq!(
        coordinator.view(&child).unwrap().location,
        CoordinatorLocation::RawQueued(RawStage::Resolve)
    );
    coordinator.audit().unwrap();
}

#[test]
fn accepted_parent_handoff_rolls_back_child_wake_at_every_apply_boundary() {
    for fault_step in 1..=3 {
        let mut coordinator = roomy();
        let (parent, _) = verify_plain(&mut coordinator, 251);
        let child = hash(252);
        coordinator
            .admit_raw(
                child.clone(),
                short(252),
                Raw("child"),
                RawStage::Resolve,
                None,
                10,
                set([parent.clone()]),
            )
            .unwrap();
        let child_lease = coordinator
            .checkout_raw(RawStage::Resolve)
            .unwrap()
            .unwrap();
        coordinator
            .wait_for_parents(&child_lease, set([parent.clone()]))
            .unwrap();
        let commit = coordinator.begin_next_commit().unwrap().unwrap();
        let before = [parent.clone(), child.clone()].map(|hash| coordinator.view(&hash).unwrap());
        let usage = coordinator.usage();
        let resolve_len = coordinator.queue_len(QueueKind::Resolve);

        coordinator.set_apply_fault_for_test(Some(fault_step));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = coordinator.commit_handoff(&commit);
        }));
        assert!(result.is_err(), "fault step {fault_step} was not reached");
        coordinator.set_apply_fault_for_test(None);

        let after = [parent, child].map(|hash| coordinator.view(&hash).unwrap());
        assert_eq!(after, before);
        assert_eq!(coordinator.usage(), usage);
        assert_eq!(coordinator.queue_len(QueueKind::Resolve), resolve_len);
        assert_eq!(coordinator.active_work(), 1);
        coordinator.audit().unwrap();
    }
}

#[test]
fn rejected_conflict_parent_invalidates_its_child_during_winner_handoff() {
    let mut coordinator = roomy();
    let contested = input(253);
    let winner = verify_candidate(
        &mut coordinator,
        253,
        HashSet::from([contested.clone()]),
        300,
    );
    let loser = verify_candidate(&mut coordinator, 254, HashSet::from([contested]), 100);
    let child = hash(255);
    coordinator
        .admit_raw(
            child.clone(),
            short(255),
            Raw("child"),
            RawStage::Resolve,
            None,
            10,
            set([loser.clone()]),
        )
        .unwrap();
    let child_lease = coordinator
        .checkout_raw(RawStage::Resolve)
        .unwrap()
        .unwrap();
    coordinator
        .wait_for_parents(&child_lease, set([loser.clone()]))
        .unwrap();

    let committing = coordinator.begin_next_commit().unwrap().unwrap();
    assert_eq!(committing.hash, winner);
    let handoff = coordinator.commit_candidate_handoff(&committing).unwrap();
    assert_eq!(handoff.rejected.len(), 1);
    assert_eq!(handoff.rejected[0].hash, loser);
    assert!(matches!(
        coordinator.view(&child).unwrap().location,
        CoordinatorLocation::Invalidated { cause } if cause == loser
    ));
    assert_eq!(coordinator.dependency_failure_len(), 1);
    coordinator.audit().unwrap();
}

#[test]
fn expiring_parent_cannot_leave_a_schedulable_child() {
    let mut coordinator = roomy();
    let parent = hash(247);
    let child = hash(248);
    coordinator
        .admit_raw_sourced(
            parent.clone(),
            short(247),
            Raw("parent"),
            RawStage::PreCheck,
            CoordinatorSource::Local,
            Some(10),
            10,
            HashSet::new(),
        )
        .unwrap();
    coordinator
        .admit_raw(
            child.clone(),
            short(248),
            Raw("child"),
            RawStage::Resolve,
            None,
            10,
            set([parent.clone()]),
        )
        .unwrap();

    let expired = coordinator.expire_due(10, 1).unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].hash, parent);
    assert!(matches!(
        coordinator.view(&child).unwrap().location,
        CoordinatorLocation::Invalidated { cause } if cause == parent
    ));
    coordinator.audit().unwrap();
}

#[test]
fn committing_deadline_cannot_block_later_expirations() {
    let mut coordinator = roomy();
    let committing = hash(249);
    let later = hash(250);
    coordinator
        .admit_raw_sourced(
            committing.clone(),
            short(249),
            Raw("committing"),
            RawStage::PreCheck,
            CoordinatorSource::Local,
            Some(10),
            10,
            HashSet::new(),
        )
        .unwrap();
    let raw = coordinator
        .checkout_raw(RawStage::PreCheck)
        .unwrap()
        .unwrap();
    coordinator
        .complete_raw(&raw, Unverified("resolved"), 20, VerifySchedule::default())
        .unwrap();
    let verify = coordinator
        .checkout_verify(WorkerCapability::Any)
        .unwrap()
        .unwrap();
    coordinator
        .complete_verification(&verify, Verified("proof"), 30)
        .unwrap();
    coordinator
        .admit_raw_sourced(
            later.clone(),
            short(250),
            Raw("later"),
            RawStage::PreCheck,
            CoordinatorSource::Local,
            Some(10),
            10,
            HashSet::new(),
        )
        .unwrap();
    let lease = coordinator.begin_next_commit().unwrap().unwrap();
    assert_eq!(lease.hash, committing);

    let expired = coordinator.expire_due(10, 1).unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].hash, later);
    coordinator.abort_commit(&lease).unwrap();
    let expired = coordinator.expire_due(10, 1).unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].hash, committing);
    coordinator.audit().unwrap();
}
