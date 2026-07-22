use crate::component::lifecycle_store::{
    LifecycleBatchOp, LifecycleBatchResult, LifecycleError, LifecycleLimits, LifecycleLocation,
    LifecycleLocationKind, LifecycleStore, LifecycleTransition, PipelineStage, PoolStage,
    Residency, TerminalOutcome,
};
use ckb_network::PeerIndex;
use ckb_types::packed::{Byte32, ProposalShortId};

#[derive(Debug, PartialEq, Eq)]
struct Payload(&'static str);

fn hash(seed: u8) -> Byte32 {
    Byte32::new([seed; 32])
}

fn short(seed: u8) -> ProposalShortId {
    ProposalShortId::new([seed; 10])
}

fn limits(global: Residency, per_peer: Option<Residency>) -> LifecycleLimits {
    LifecycleLimits::new(global, per_peer)
}

fn roomy_store() -> LifecycleStore<Payload> {
    LifecycleStore::new(limits(
        Residency::new(100, 10_000),
        Some(Residency::new(10, 1_000)),
    ))
}

#[test]
fn one_owner_flows_through_every_stage_with_consistent_indexes() {
    let mut store = roomy_store();
    let tx_hash = hash(1);
    let peer: PeerIndex = 7.into();
    store
        .admit(
            tx_hash.clone(),
            short(1),
            Payload("raw"),
            LifecycleLocation::Queued(PipelineStage::PreCheck),
            Some(peer),
            10,
        )
        .unwrap();
    assert_eq!(store.usage(), Residency::new(1, 10));
    assert_eq!(store.peer_usage(peer), Residency::new(1, 10));
    assert_eq!(store.location_len(LifecycleLocationKind::QueuedPreCheck), 1);
    store.audit().unwrap();

    let precheck = store.checkout(&tx_hash, PipelineStage::PreCheck).unwrap();
    assert_eq!(*precheck.payload, Payload("raw"));
    store
        .complete(
            &precheck,
            LifecycleLocation::Queued(PipelineStage::Resolve),
            Some((Payload("prechecked"), 20)),
        )
        .unwrap();
    assert_eq!(store.usage(), Residency::new(1, 20));
    assert_eq!(store.peer_usage(peer), Residency::new(1, 20));

    let resolve = store.checkout(&tx_hash, PipelineStage::Resolve).unwrap();
    let waiting_version = store
        .complete(&resolve, LifecycleLocation::WaitingParents, None)
        .unwrap();
    store
        .transition(
            &tx_hash,
            waiting_version,
            &LifecycleLocation::WaitingParents,
            LifecycleLocation::Queued(PipelineStage::Resolve),
        )
        .unwrap();
    let resolve = store.checkout(&tx_hash, PipelineStage::Resolve).unwrap();
    store
        .complete(
            &resolve,
            LifecycleLocation::Queued(PipelineStage::Verify),
            Some((Payload("resolved"), 40)),
        )
        .unwrap();
    assert_eq!(store.usage(), Residency::new(1, 40));

    let verify = store.checkout(&tx_hash, PipelineStage::Verify).unwrap();
    let mut version = store
        .complete(&verify, LifecycleLocation::ReadyToCommit, None)
        .unwrap();
    version = store
        .transition(
            &tx_hash,
            version,
            &LifecycleLocation::ReadyToCommit,
            LifecycleLocation::Committing,
        )
        .unwrap();
    version = store
        .transition(
            &tx_hash,
            version,
            &LifecycleLocation::Committing,
            LifecycleLocation::Pool(PoolStage::Pending),
        )
        .unwrap();
    version = store
        .transition(
            &tx_hash,
            version,
            &LifecycleLocation::Pool(PoolStage::Pending),
            LifecycleLocation::Pool(PoolStage::Gap),
        )
        .unwrap();
    version = store
        .transition(
            &tx_hash,
            version,
            &LifecycleLocation::Pool(PoolStage::Gap),
            LifecycleLocation::Pool(PoolStage::Proposed),
        )
        .unwrap();

    assert_eq!(
        store.view(&tx_hash).unwrap().location,
        LifecycleLocation::Pool(PoolStage::Proposed)
    );
    assert_eq!(store.location_len(LifecycleLocationKind::PoolProposed), 1);
    assert_eq!(*store.payload(&tx_hash).unwrap(), Payload("resolved"));
    store.audit().unwrap();

    let terminal = store
        .terminalize(
            &tx_hash,
            version,
            &LifecycleLocation::Pool(PoolStage::Proposed),
            TerminalOutcome::Committed,
        )
        .unwrap();
    assert_eq!(*terminal.payload, Payload("resolved"));
    assert_eq!(terminal.outcome, TerminalOutcome::Committed);
    assert!(store.is_empty());
    assert_eq!(store.usage(), Residency::default());
    assert_eq!(store.peer_usage(peer), Residency::default());
    store.audit().unwrap();
}

#[test]
fn hash_and_short_id_uniqueness_are_both_authoritative() {
    let mut store = roomy_store();
    let first_hash = hash(2);
    let first_short = short(2);
    store
        .admit(
            first_hash.clone(),
            first_short.clone(),
            Payload("first"),
            LifecycleLocation::Queued(PipelineStage::PreCheck),
            None,
            10,
        )
        .unwrap();

    assert_eq!(
        store.admit(
            first_hash.clone(),
            short(3),
            Payload("duplicate hash"),
            LifecycleLocation::Queued(PipelineStage::PreCheck),
            None,
            10,
        ),
        Err(LifecycleError::DuplicateHash(first_hash.clone()))
    );
    assert_eq!(
        store.admit(
            hash(3),
            first_short.clone(),
            Payload("short collision"),
            LifecycleLocation::Queued(PipelineStage::PreCheck),
            None,
            10,
        ),
        Err(LifecycleError::ShortIdCollision {
            short_id: first_short.clone(),
            existing_hash: first_hash.clone(),
        })
    );
    assert_eq!(store.hash_by_short_id(&first_short), Some(&first_hash));
    assert_eq!(store.len(), 1);
    assert_eq!(store.usage(), Residency::new(1, 10));
    store.audit().unwrap();
}

#[test]
fn global_and_per_peer_budgets_follow_active_and_waiting_entries() {
    let mut store =
        LifecycleStore::new(limits(Residency::new(2, 100), Some(Residency::new(2, 60))));
    let peer_a: PeerIndex = 1.into();
    let peer_b: PeerIndex = 2.into();
    let hash_a = hash(10);

    store
        .admit(
            hash_a.clone(),
            short(10),
            Payload("a"),
            LifecycleLocation::Queued(PipelineStage::PreCheck),
            Some(peer_a),
            50,
        )
        .unwrap();
    let lease = store.checkout(&hash_a, PipelineStage::PreCheck).unwrap();
    store
        .complete(&lease, LifecycleLocation::WaitingParents, None)
        .unwrap();

    assert_eq!(
        store.admit(
            hash(11),
            short(11),
            Payload("same peer"),
            LifecycleLocation::Queued(PipelineStage::PreCheck),
            Some(peer_a),
            11,
        ),
        Err(LifecycleError::PeerBudgetExceeded(peer_a))
    );
    store
        .admit(
            hash(12),
            short(12),
            Payload("other peer"),
            LifecycleLocation::Queued(PipelineStage::PreCheck),
            Some(peer_b),
            50,
        )
        .unwrap();
    assert_eq!(store.usage(), Residency::new(2, 100));
    assert_eq!(
        store.admit(
            hash(13),
            short(13),
            Payload("global overflow"),
            LifecycleLocation::Queued(PipelineStage::PreCheck),
            None,
            0,
        ),
        Err(LifecycleError::GlobalBudgetExceeded)
    );
    store.audit().unwrap();
}

#[test]
fn failed_payload_recharge_is_transactional() {
    let mut store = LifecycleStore::new(limits(Residency::new(1, 50), None));
    let tx_hash = hash(20);
    store
        .admit(
            tx_hash.clone(),
            short(20),
            Payload("raw"),
            LifecycleLocation::Queued(PipelineStage::PreCheck),
            None,
            20,
        )
        .unwrap();
    let lease = store.checkout(&tx_hash, PipelineStage::PreCheck).unwrap();
    let before = store.view(&tx_hash).unwrap();

    assert_eq!(
        store.complete(
            &lease,
            LifecycleLocation::Queued(PipelineStage::Resolve),
            Some((Payload("too large"), 51)),
        ),
        Err(LifecycleError::GlobalBudgetExceeded)
    );
    assert_eq!(store.view(&tx_hash).unwrap(), before);
    assert_eq!(*store.payload(&tx_hash).unwrap(), Payload("raw"));
    assert_eq!(store.usage(), Residency::new(1, 20));
    store.audit().unwrap();
}

#[test]
fn stale_worker_cannot_complete_a_re_admitted_hash() {
    let mut store = roomy_store();
    let tx_hash = hash(30);
    store
        .admit(
            tx_hash.clone(),
            short(30),
            Payload("old"),
            LifecycleLocation::Queued(PipelineStage::PreCheck),
            None,
            10,
        )
        .unwrap();
    let old_lease = store.checkout(&tx_hash, PipelineStage::PreCheck).unwrap();
    store
        .force_remove(&tx_hash, TerminalOutcome::Removed)
        .unwrap();

    store
        .admit(
            tx_hash.clone(),
            short(30),
            Payload("new"),
            LifecycleLocation::Queued(PipelineStage::PreCheck),
            None,
            10,
        )
        .unwrap();
    let new_lease = store.checkout(&tx_hash, PipelineStage::PreCheck).unwrap();
    let new_view = store.view(&tx_hash).unwrap();

    assert!(matches!(
        store.complete(
            &old_lease,
            LifecycleLocation::Queued(PipelineStage::Resolve),
            None,
        ),
        Err(LifecycleError::IncarnationMismatch { .. })
    ));
    assert_eq!(store.view(&tx_hash).unwrap(), new_view);
    assert_eq!(*store.payload(&tx_hash).unwrap(), Payload("new"));

    store
        .complete(
            &new_lease,
            LifecycleLocation::Queued(PipelineStage::Resolve),
            None,
        )
        .unwrap();
    store.audit().unwrap();
}

#[test]
fn illegal_transition_leaves_state_and_indexes_unchanged() {
    let mut store = roomy_store();
    let tx_hash = hash(40);
    let version = store
        .admit(
            tx_hash.clone(),
            short(40),
            Payload("raw"),
            LifecycleLocation::Queued(PipelineStage::PreCheck),
            None,
            10,
        )
        .unwrap();
    let before = store.view(&tx_hash).unwrap();

    assert_eq!(
        store.transition(
            &tx_hash,
            version,
            &LifecycleLocation::Queued(PipelineStage::PreCheck),
            LifecycleLocation::Pool(PoolStage::Pending),
        ),
        Err(LifecycleError::IllegalTransition {
            from: LifecycleLocation::Queued(PipelineStage::PreCheck),
            to: LifecycleLocation::Pool(PoolStage::Pending),
        })
    );
    assert_eq!(store.view(&tx_hash).unwrap(), before);
    assert_eq!(store.location_len(LifecycleLocationKind::QueuedPreCheck), 1);
    store.audit().unwrap();
}

#[test]
fn batch_transition_is_all_or_nothing() {
    let mut store = roomy_store();
    let hash_a = hash(50);
    let hash_b = hash(51);
    let version_a = store
        .admit(
            hash_a.clone(),
            short(50),
            Payload("a"),
            LifecycleLocation::Pool(PoolStage::Pending),
            None,
            10,
        )
        .unwrap();
    let version_b = store
        .admit(
            hash_b.clone(),
            short(51),
            Payload("b"),
            LifecycleLocation::Pool(PoolStage::Pending),
            None,
            10,
        )
        .unwrap();

    let stale_b = crate::component::lifecycle_store::LifecycleVersion {
        revision: version_b.revision + 1,
        ..version_b
    };
    let invalid_batch = [
        LifecycleTransition {
            hash: hash_a.clone(),
            version: version_a,
            expected: LifecycleLocation::Pool(PoolStage::Pending),
            next: LifecycleLocation::Pool(PoolStage::Gap),
        },
        LifecycleTransition {
            hash: hash_b.clone(),
            version: stale_b,
            expected: LifecycleLocation::Pool(PoolStage::Pending),
            next: LifecycleLocation::Pool(PoolStage::Gap),
        },
    ];
    assert!(matches!(
        store.transition_batch(&invalid_batch),
        Err(LifecycleError::RevisionMismatch { .. })
    ));
    assert_eq!(
        store.view(&hash_a).unwrap().location,
        LifecycleLocation::Pool(PoolStage::Pending)
    );
    assert_eq!(
        store.view(&hash_b).unwrap().location,
        LifecycleLocation::Pool(PoolStage::Pending)
    );

    let valid_batch = [
        LifecycleTransition {
            hash: hash_a.clone(),
            version: version_a,
            expected: LifecycleLocation::Pool(PoolStage::Pending),
            next: LifecycleLocation::Pool(PoolStage::Gap),
        },
        LifecycleTransition {
            hash: hash_b.clone(),
            version: version_b,
            expected: LifecycleLocation::Pool(PoolStage::Pending),
            next: LifecycleLocation::Pool(PoolStage::Gap),
        },
    ];
    let next_versions = store.transition_batch(&valid_batch).unwrap();
    assert_eq!(next_versions.len(), 2);
    assert_eq!(store.location_len(LifecycleLocationKind::PoolPending), 0);
    assert_eq!(store.location_len(LifecycleLocationKind::PoolGap), 2);

    let duplicate_batch = [
        LifecycleTransition {
            hash: hash_a.clone(),
            version: next_versions[0],
            expected: LifecycleLocation::Pool(PoolStage::Gap),
            next: LifecycleLocation::Pool(PoolStage::Proposed),
        },
        LifecycleTransition {
            hash: hash_a.clone(),
            version: next_versions[0],
            expected: LifecycleLocation::Pool(PoolStage::Gap),
            next: LifecycleLocation::Pool(PoolStage::Proposed),
        },
    ];
    assert_eq!(
        store.transition_batch(&duplicate_batch),
        Err(LifecycleError::DuplicateBatchEntry(hash_a.clone()))
    );
    assert_eq!(
        store.view(&hash_a).unwrap().location,
        LifecycleLocation::Pool(PoolStage::Gap)
    );
    store.audit().unwrap();
}

#[test]
fn rbf_winner_and_victims_commit_in_one_lifecycle_batch() {
    let mut store = roomy_store();
    let winner_hash = hash(55);
    let victim_a_hash = hash(56);
    let victim_b_hash = hash(57);

    store
        .admit(
            winner_hash.clone(),
            short(55),
            Payload("winner"),
            LifecycleLocation::Queued(PipelineStage::Verify),
            None,
            10,
        )
        .unwrap();
    let winner_lease = store.checkout(&winner_hash, PipelineStage::Verify).unwrap();
    let winner_ready = store
        .complete(&winner_lease, LifecycleLocation::ReadyToCommit, None)
        .unwrap();
    let winner_committing = store
        .transition(
            &winner_hash,
            winner_ready,
            &LifecycleLocation::ReadyToCommit,
            LifecycleLocation::Committing,
        )
        .unwrap();

    let victim_a_version = store
        .admit(
            victim_a_hash.clone(),
            short(56),
            Payload("victim-a"),
            LifecycleLocation::Pool(PoolStage::Pending),
            None,
            10,
        )
        .unwrap();
    let victim_b_version = store
        .admit(
            victim_b_hash.clone(),
            short(57),
            Payload("victim-b"),
            LifecycleLocation::Pool(PoolStage::Proposed),
            None,
            10,
        )
        .unwrap();

    let operations = [
        LifecycleBatchOp::Transition(LifecycleTransition {
            hash: winner_hash.clone(),
            version: winner_committing,
            expected: LifecycleLocation::Committing,
            next: LifecycleLocation::Pool(PoolStage::Pending),
        }),
        LifecycleBatchOp::Terminalize {
            hash: victim_a_hash.clone(),
            version: victim_a_version,
            expected: LifecycleLocation::Pool(PoolStage::Pending),
            outcome: TerminalOutcome::Rejected,
        },
        LifecycleBatchOp::Terminalize {
            hash: victim_b_hash.clone(),
            version: victim_b_version,
            expected: LifecycleLocation::Pool(PoolStage::Proposed),
            outcome: TerminalOutcome::Rejected,
        },
    ];
    let results = store.apply_batch(&operations).unwrap();

    assert_eq!(results.len(), 3);
    assert!(matches!(
        &results[0],
        LifecycleBatchResult::Transitioned { hash, .. } if hash == &winner_hash
    ));
    for (result, expected_hash) in results[1..].iter().zip([&victim_a_hash, &victim_b_hash]) {
        assert!(matches!(
            result,
            LifecycleBatchResult::Terminalized(entry)
                if &entry.hash == expected_hash
                    && entry.outcome == TerminalOutcome::Rejected
        ));
    }
    assert_eq!(store.len(), 1);
    assert_eq!(
        store.view(&winner_hash).unwrap().location,
        LifecycleLocation::Pool(PoolStage::Pending)
    );
    assert!(store.view(&victim_a_hash).is_none());
    assert!(store.view(&victim_b_hash).is_none());
    assert_eq!(store.usage(), Residency::new(1, 10));
    store.audit().unwrap();
}

#[test]
fn clear_releases_every_budget_and_index() {
    let mut store = roomy_store();
    let peer: PeerIndex = 9.into();
    for seed in 60..63 {
        store
            .admit(
                hash(seed),
                short(seed),
                Payload("entry"),
                LifecycleLocation::Queued(PipelineStage::PreCheck),
                Some(peer),
                10,
            )
            .unwrap();
    }
    let removed = store.clear();
    assert_eq!(removed.len(), 3);
    assert!(
        removed
            .iter()
            .all(|entry| entry.outcome == TerminalOutcome::Cleared)
    );
    assert!(store.is_empty());
    assert_eq!(store.usage(), Residency::default());
    assert_eq!(store.peer_usage(peer), Residency::default());
    assert_eq!(store.location_len(LifecycleLocationKind::QueuedPreCheck), 0);
    store.audit().unwrap();
}
