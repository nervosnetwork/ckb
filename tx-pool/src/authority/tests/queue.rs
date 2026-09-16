use super::{Queues, WorkStage};
use crate::authority::{
    model::{Phase, Source, Status},
    tests::common::{
        config, entry, queued, remote, store, store_with_pipeline_limit, tx, verified,
    },
};
use ckb_app_config::VerifyOrdering;
use std::{sync::Arc, time::Duration};

#[test]
fn each_queue_progresses_while_the_other_lane_is_held() {
    for stage in [WorkStage::Resolve, WorkStage::Verify] {
        let store = store();
        let queues = Queues::new(VerifyOrdering::ArrivalTime, 100);
        let original = entry(&store, tx(200), Source::Local);
        let queued = if stage == WorkStage::Verify {
            original.with_phase(Phase::Verify(Arc::clone(
                verified(&store, &original, 1, 1, Status::Pending).resolved(),
            )))
        } else {
            original
        };
        std::thread::scope(|scope| {
            let held = if stage == WorkStage::Verify {
                queues.resolve.lock()
            } else {
                queues.verify.lock()
            };
            let (finished, received) = std::sync::mpsc::sync_channel(1);
            let queues = &queues;
            let store = &store;
            let task = scope.spawn(move || {
                queues.insert(&queued);
                let (selected, memory) = queues.pop(stage, false, &store.budget).unwrap().unwrap();
                assert!(Arc::ptr_eq(&selected, &queued));
                drop(memory);
                queues.insert(&queued);
                queues.remove(&queued);
                assert!(queues.pop(stage, false, &store.budget).unwrap().is_none());
                finished.send(()).unwrap();
            });
            // Completion while the other lane is still held establishes the
            // ordering; the timeout only bounds a regression's failure.
            let progress = received.recv_timeout(Duration::from_secs(5));
            drop(held);
            task.join().unwrap();
            progress.unwrap();
        });
        assert!(!store.budget.faulted());
    }
}

#[test]
fn clearing_detaches_both_queues_and_allows_fresh_work() {
    let store = store();
    let queues = Queues::new(VerifyOrdering::FeeRate, 100);
    let resolving = entry(&store, tx(201), Source::Local);
    let verifying = entry(&store, tx(202), Source::Local);
    let verifying = verifying.with_phase(Phase::Verify(Arc::clone(
        verified(&store, &verifying, 1, 1, Status::Pending).resolved(),
    )));
    queues.insert(&resolving);
    queues.insert(&verifying);
    assert_eq!(queues.queued_len(), 2);
    let retired = queues.take();
    assert_eq!(queues.queued_len(), 0);
    assert_eq!(retired.queued_len(), 2);
    for stage in [WorkStage::Resolve, WorkStage::Verify] {
        assert!(queues.pop(stage, false, &store.budget).unwrap().is_none());
        assert!(retired.pop(stage, false, &store.budget).unwrap().is_some());
    }
    assert_eq!(retired.queued_len(), 0);
    queues.insert(&resolving);
    assert_eq!(queues.queued_len(), 1);
    let (fresh, _) = queues
        .pop(WorkStage::Resolve, false, &store.budget)
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&fresh, &resolving));
    assert_eq!(queues.queued_len(), 0);
    assert!(!store.budget.faulted());
}

#[test]
fn both_phases_use_the_same_declared_cycle_boundary() {
    let snapshot = crate::test_support::genesis_snapshot();
    let configuration = config();
    let remote = |cycles| Source::Remote {
        peer: 1.into(),
        deadline: std::time::Instant::now() + Duration::from_secs(60),
        cycles,
    };
    for order in [VerifyOrdering::ArrivalTime, VerifyOrdering::FeeRate] {
        for (source, large) in [
            (remote(Some(99)), false),
            (remote(Some(100)), false),
            (remote(Some(101)), true),
            (Source::Local, false),
            (Source::Recovery, false),
            (Source::Proposal { remote: None }, false),
            (remote(None), false),
        ] {
            for stage in [WorkStage::Resolve, WorkStage::Verify] {
                let store =
                    store_with_pipeline_limit(Arc::clone(&snapshot), &configuration, 64_000_000);
                let queues = Queues::new(order, 100);
                let original = entry(&store, tx(203), source);
                let queued = if stage == WorkStage::Verify {
                    original.with_phase(Phase::Verify(Arc::clone(
                        verified(&store, &original, 1, 1, Status::Pending).resolved(),
                    )))
                } else {
                    original
                };
                queues.insert(&queued);
                let small = queues.pop(stage, true, &store.budget).unwrap();
                assert_eq!(small.is_none(), large);
                if large {
                    let (selected, _) = queues.pop(stage, false, &store.budget).unwrap().unwrap();
                    assert!(Arc::ptr_eq(&selected, &queued));
                } else {
                    assert!(Arc::ptr_eq(&small.unwrap().0, &queued));
                }
            }
        }
    }
}

#[test]
fn verification_queue_preserves_configured_arrival_and_fee_ordering() {
    for (order, expected) in [
        (VerifyOrdering::ArrivalTime, 30),
        (VerifyOrdering::FeeRate, 31),
    ] {
        let store = store();
        let queues = Queues::new(order, 100);
        let first = queued(&store, 30, Source::Local, 1);
        let second = queued(&store, 31, Source::Local, 100);
        queues.insert(&first);
        queues.insert(&second);
        let (selected, _) = queues
            .pop(WorkStage::Verify, false, &store.budget)
            .unwrap()
            .unwrap();
        assert_eq!(selected.hash(), tx(expected).hash());
    }
}

#[test]
fn fair_peer_selection_skips_an_ineligible_peer_and_preserves_the_small_lane() {
    let store = store();
    let queues = Queues::new(VerifyOrdering::ArrivalTime, 100);
    let blocked = queued(&store, 32, remote(1, 1), 1);
    let small = queued(&store, 33, remote(2, 1), 1);
    let large = queued(&store, 34, remote(3, 200), 1);
    queues.insert(&blocked);
    queues.insert(&small);
    queues.insert(&large);
    assert_eq!(queues.queued_len(), 3);
    let _occupied = store.budget.active(remote(1, 1)).unwrap();
    let (selected, memory) = queues
        .pop(WorkStage::Verify, true, &store.budget)
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&selected, &small));
    assert_eq!(queues.queued_len(), 2);
    drop(memory);
    assert!(
        queues
            .pop(WorkStage::Verify, true, &store.budget)
            .unwrap()
            .is_none()
    );
    assert_eq!(queues.queued_len(), 2);
    let (selected, _) = queues
        .pop(WorkStage::Verify, false, &store.budget)
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&selected, &large));
    assert_eq!(queues.queued_len(), 1);
}

#[test]
fn queue_count_follows_exact_replacement_removal_and_stale_collection() {
    for stage in [WorkStage::Resolve, WorkStage::Verify] {
        let store = store();
        let queues = Queues::new(VerifyOrdering::ArrivalTime, 100);
        let make = |nonce| {
            let original = entry(&store, tx(nonce), Source::Local);
            if stage == WorkStage::Verify {
                original.with_phase(Phase::Verify(Arc::clone(
                    verified(&store, &original, 1, 1, Status::Pending).resolved(),
                )))
            } else {
                original
            }
        };
        let original = make(8100);
        queues.insert(&original);
        queues.insert(&original);
        assert_eq!(queues.queued_len(), 1);
        let successor = original.with_phase(original.phase.clone());
        queues.insert(&successor);
        assert_eq!(queues.queued_len(), 1);
        queues.remove(&original);
        assert_eq!(queues.queued_len(), 1);
        let (selected, memory) = queues.pop(stage, false, &store.budget).unwrap().unwrap();
        assert!(Arc::ptr_eq(&selected, &successor));
        assert_eq!(queues.queued_len(), 0);
        drop(memory);
        queues.remove(&successor);
        assert_eq!(queues.queued_len(), 0);
        queues.insert(&successor);
        queues.remove(&successor);
        assert_eq!(queues.queued_len(), 0);

        let stale = make(8101);
        queues.insert(&stale);
        drop(stale);
        assert_eq!(queues.queued_len(), 1);
        assert!(queues.pop(stage, false, &store.budget).unwrap().is_none());
        assert_eq!(queues.queued_len(), 0);
        assert!(!store.budget.faulted());
    }
}

#[tokio::test]
async fn global_active_refusal_preserves_both_queues_and_observes_an_early_release() {
    use crate::authority::model::{Error, FullReason};
    for order in [VerifyOrdering::ArrivalTime, VerifyOrdering::FeeRate] {
        let store = store();
        let queues = Queues::new(order, 100);
        let owners: Vec<_> = (0..128)
            .flat_map(|peer| {
                let source = remote(peer, 1);
                let nonce = u32::try_from(peer).unwrap();
                let resolving = entry(&store, tx(9000 + nonce), source);
                let verifying = queued(&store, 9200 + nonce, source, 1);
                [resolving, verifying]
            })
            .collect();
        for owner in &owners {
            queues.insert(owner);
        }
        // Establish nonempty fairness cursors before filling shared capacity.
        for stage in [WorkStage::Resolve, WorkStage::Verify] {
            let (selected, permit) = queues.pop(stage, false, &store.budget).unwrap().unwrap();
            queues.insert(&selected);
            drop(permit);
        }
        let cursors = (queues.resolve.lock().cursor, queues.verify.lock().cursor);
        let mut occupied: Vec<_> = (0..16)
            .map_while(|_| store.budget.active(Source::Local).ok())
            .collect();
        assert!(occupied.len() < 16);
        assert!(matches!(
            store.budget.active(Source::Local),
            Err(Error::Full(FullReason::Active))
        ));
        let changed = store.budget.changed.notified();
        for stage in [WorkStage::Resolve, WorkStage::Verify] {
            assert!(matches!(
                queues.pop(stage, false, &store.budget),
                Err(Error::Full(FullReason::Active))
            ));
        }
        assert_eq!(queues.queued_len(), owners.len());
        assert_eq!(
            (queues.resolve.lock().cursor, queues.verify.lock().cursor),
            cursors
        );
        // Register-before-check must retain a release before the first poll.
        drop(occupied.pop().unwrap());
        tokio::time::timeout(Duration::from_secs(1), changed)
            .await
            .unwrap();
        for stage in [WorkStage::Resolve, WorkStage::Verify] {
            let (selected, permit) = queues.pop(stage, false, &store.budget).unwrap().unwrap();
            assert!(owners.iter().any(|owner| Arc::ptr_eq(owner, &selected)));
            drop(permit);
        }
        assert_eq!(queues.queued_len(), owners.len() - 2);
        drop(occupied);
        assert!(!store.budget.faulted());
    }
}
