use crate::component::pipeline_coordinator::{
    CoordinatorError, CoordinatorFeeGate, CoordinatorLimits, CoordinatorLocation,
    CoordinatorMetadataCost, CoordinatorResidency, CoordinatorSource, PayloadPhase,
    PipelineCoordinator, QueueKind, RawStage, TerminalDisposition, TrustedSource,
};
use ckb_network::PeerIndex;
use ckb_types::packed::{Byte32, OutPoint, ProposalShortId};
use std::collections::HashSet;

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

fn roomy() -> PipelineCoordinator<Raw, Unverified, Verified> {
    PipelineCoordinator::new(CoordinatorLimits::new(
        CoordinatorResidency::new(100, 100_000),
        Some(CoordinatorResidency::new(20, 20_000)),
        16,
        16,
    ))
}

fn verify_candidate(
    coordinator: &mut PipelineCoordinator<Raw, Unverified, Verified>,
    seed: u8,
    conflict_inputs: HashSet<OutPoint>,
    fee: u64,
) -> Byte32 {
    let tx_hash = hash(seed);
    coordinator
        .admit_raw(
            tx_hash.clone(),
            short(seed),
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
        .complete_raw(&raw, Unverified("resolved"), 20)
        .unwrap();
    let verify = coordinator.checkout_verify().unwrap().unwrap();
    let candidate = CoordinatorFeeGate::new(0, 0)
        .validate(tx_hash.clone(), conflict_inputs, fee, 100)
        .unwrap();
    coordinator
        .complete_verification_candidate(&verify, Verified("proof"), 30, candidate)
        .unwrap();
    tx_hash
}

fn verify_plain(
    coordinator: &mut PipelineCoordinator<Raw, Unverified, Verified>,
    seed: u8,
) -> (
    Byte32,
    crate::component::pipeline_coordinator::CoordinatorVersion,
) {
    let tx_hash = hash(seed);
    coordinator
        .admit_raw(
            tx_hash.clone(),
            short(seed),
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
        .complete_raw(&raw, Unverified("resolved"), 20)
        .unwrap();
    let verify = coordinator.checkout_verify().unwrap().unwrap();
    let version = coordinator
        .complete_verification(&verify, Verified("proof"), 30)
        .unwrap();
    (tx_hash, version)
}

#[test]
fn accepted_pool_inputs_wake_only_after_the_final_input_is_free() {
    let mut coordinator = roomy();
    let (tx_hash, version) = verify_plain(&mut coordinator, 80);
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
    let limits = CoordinatorLimits::new(
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
    assert_eq!(coordinator.pool_input_freed(&shared, 1).unwrap().len(), 1);
    assert_eq!(coordinator.queue_len(QueueKind::Commit), 1);
    assert_eq!(coordinator.pool_input_freed(&shared, 1).unwrap().len(), 1);
    assert_eq!(coordinator.queue_len(QueueKind::Commit), 2);
    coordinator.audit().unwrap();

    let (third, third_version) = verify_plain(&mut coordinator, 85);
    let too_many = inputs([185, 186, 187]);
    assert_eq!(
        coordinator.wait_for_pool_inputs(&third, third_version, too_many),
        Err(CoordinatorError::PoolInputLimitExceeded)
    );
    assert_eq!(
        coordinator.view(&third).unwrap().location,
        CoordinatorLocation::ReadyToCommit
    );
    coordinator.audit().unwrap();
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
    let limits = CoordinatorLimits::new(
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
        .complete_raw(&raw, Unverified("resolved"), 20)
        .unwrap();
    assert_eq!(coordinator.usage(), CoordinatorResidency::new(1, 46));
    let verify = coordinator.checkout_verify().unwrap().unwrap();
    let candidate = CoordinatorFeeGate::new(0, 0)
        .validate(tx_hash.clone(), inputs([187, 188]), 100, 100)
        .unwrap();
    let version = coordinator
        .complete_verification_candidate(&verify, Verified("proof"), 30, candidate)
        .unwrap();
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

    let tight_limits = CoordinatorLimits::new(CoordinatorResidency::new(1, 35), None, 4, 4)
        .with_metadata_cost(metadata);
    let mut tight: PipelineCoordinator<Raw, Unverified, Verified> =
        PipelineCoordinator::new(tight_limits);
    assert_eq!(
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
    );
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
        [hash(96), hash(97), hash(95)]
    );
    coordinator.audit().unwrap();
}

#[test]
fn peer_rotation_and_active_caps_prevent_a_remote_fifo_prefix_monopoly() {
    let limits = CoordinatorLimits::new(
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
        .complete_raw(&first, Unverified("done"), 20)
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
        .complete_raw(&lease, Unverified("resolved"), 20)
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
        .complete_raw(&raw, Unverified("resolved"), 20)
        .unwrap();
    let verify = coordinator.checkout_verify().unwrap().unwrap();
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
                    let _ = coordinator.complete_raw(&lease, Unverified("state-machine"), 20);
                }
            }
            4 => {
                if let Some(lease) = raw_leases.pop() {
                    let _ = coordinator.requeue_raw(&lease);
                }
            }
            5 => {
                if let Ok(Some(lease)) = coordinator.checkout_verify() {
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
        .complete_raw(&raw, Unverified("resolved"), 20)
        .unwrap();
    let view = coordinator.view(&tx_hash).unwrap();
    assert_eq!(view.phase, PayloadPhase::Unverified);
    assert_eq!(view.location, CoordinatorLocation::VerifyQueued);
    assert_eq!(coordinator.usage(), CoordinatorResidency::new(1, 20));
    coordinator.audit().unwrap();

    let verify = coordinator.checkout_verify().unwrap().unwrap();
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
        .complete_raw(&raw, Unverified("resolved"), 50)
        .unwrap();
    let verify = coordinator.checkout_verify().unwrap().unwrap();

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
        coordinator.complete_raw(&old, Unverified("stale"), 20),
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
        PipelineCoordinator::new(CoordinatorLimits::new(
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
            None,
            10,
            set([parent.clone()]),
        ),
        Err(CoordinatorError::ParentFanoutLimitExceeded(hash)) if hash == parent
    ));
    assert_eq!(
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
            short_id: short(8),
            existing_hash: first,
        })
    );
    assert_eq!(coordinator.len(), 1);
    assert_eq!(coordinator.usage(), CoordinatorResidency::new(1, 10));
    coordinator.audit().unwrap();
}

#[test]
fn failed_phase_recharge_leaves_payload_location_and_queue_unchanged() {
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> = PipelineCoordinator::new(
        CoordinatorLimits::new(CoordinatorResidency::new(1, 15), None, 4, 4),
    );
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

    assert_eq!(
        coordinator.complete_raw(&lease, Unverified("too large"), 16),
        Err(CoordinatorError::GlobalBudgetExceeded)
    );
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
        .complete_raw(&raw, Unverified("resolved"), 20)
        .unwrap();
    let verify = coordinator.checkout_verify().unwrap().unwrap();
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
        .complete_raw(&raw, Unverified("unverified high fee"), 20)
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
        .complete_raw(&raw, Unverified("resolved"), 20)
        .unwrap();
    let _verify = coordinator.checkout_verify().unwrap().unwrap();

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
fn conflict_limits_fail_before_verified_state_or_indexes_change() {
    let mut coordinator: PipelineCoordinator<Raw, Unverified, Verified> = PipelineCoordinator::new(
        CoordinatorLimits::new(CoordinatorResidency::new(10, 1_000), None, 4, 4)
            .with_conflict_limits(1, 1, 1),
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
        .complete_raw(&raw, Unverified("resolved"), 20)
        .unwrap();
    let verify = coordinator.checkout_verify().unwrap().unwrap();
    let candidate = CoordinatorFeeGate::new(0, 0)
        .validate(second_hash.clone(), inputs([6]), 200, 100)
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
    let _owner = verify_candidate(
        &mut coordinator,
        41,
        HashSet::from([contested.clone()]),
        200,
    );
    let waiter = verify_candidate(&mut coordinator, 42, HashSet::from([contested]), 100);
    coordinator
        .set_revision_for_test(&waiter, u64::MAX)
        .unwrap();

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
