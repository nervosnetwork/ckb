use super::{Queues, WorkStage};
use crate::authority::{
    model::{Phase, Source, Status},
    tests::common::{entry, queued, remote, store, tx, verified},
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
    let retired = queues.take();
    for stage in [WorkStage::Resolve, WorkStage::Verify] {
        assert!(queues.pop(stage, false, &store.budget).unwrap().is_none());
        assert!(retired.pop(stage, false, &store.budget).unwrap().is_some());
    }
    queues.insert(&resolving);
    let (fresh, _) = queues
        .pop(WorkStage::Resolve, false, &store.budget)
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&fresh, &resolving));
    assert!(!store.budget.faulted());
}

#[test]
fn both_phases_use_the_same_declared_cycle_boundary() {
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
                let store = store();
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
    let _occupied = store.budget.active(remote(1, 1)).unwrap();
    let (selected, memory) = queues
        .pop(WorkStage::Verify, true, &store.budget)
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&selected, &small));
    drop(memory);
    assert!(
        queues
            .pop(WorkStage::Verify, true, &store.budget)
            .unwrap()
            .is_none()
    );
    let (selected, _) = queues
        .pop(WorkStage::Verify, false, &store.budget)
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&selected, &large));
}
