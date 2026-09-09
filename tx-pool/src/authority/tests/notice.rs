use super::*;
use crate::{authority::tests::common::*, network::DummyTxPoolNetwork};
use std::future::Future;

fn fixture(
    callbacks: Callbacks,
) -> (
    Arc<Outbox>,
    Endpoints,
    super::super::relay::AuthorityRelayReceiver,
) {
    let store = store();
    let (sink, receiver) =
        super::super::relay::production_authority_relay_mailbox(1024, 128).unwrap();
    let endpoints = Endpoints::new(
        Arc::new(DummyTxPoolNetwork),
        sink,
        Arc::new(callbacks),
        None,
        FeeEstimator::new_dummy(),
    );
    (Arc::clone(&store.outbox), endpoints, receiver)
}
fn effect(nonce: u32) -> Effect {
    Effect {
        relay: Some(TxVerificationResult::Reject {
            tx_hash: tx(nonce).hash(),
        }),
        ..Effect::default()
    }
}
fn callback_effect(nonce: u32) -> Effect {
    let store = store();
    let hash = accept(&store, output_tx(nonce), 1, 1, Status::Pending);
    let owner = store.point(&hash).1.unwrap();
    let selected =
        super::super::membership::snapshot(&owner, Default::default(), Default::default()).unwrap();
    Effect {
        callback: Some(CallbackEvent::Pending(selected)),
        ..effect(nonce)
    }
}
fn append(outbox: &Arc<Outbox>, effects: Vec<Effect>) -> Arc<Batch> {
    outbox
        .reserve(effects, Class::Trusted)
        .unwrap()
        .unwrap()
        .append()
}
fn poll_pending(future: std::pin::Pin<&mut impl Future>) {
    assert!(
        future
            .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
            .is_pending()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn later_activation_cannot_overtake_committed_fifo_head() {
    let (outbox, endpoints, receiver) = fixture(Callbacks::new());
    let first = append(&outbox, vec![effect(1), effect(2)]);
    let second = append(&outbox, vec![effect(3)]);
    second.activate(&outbox);
    outbox.close();
    let mut publisher = Box::pin(Arc::clone(&outbox).run(endpoints));
    poll_pending(publisher.as_mut());
    assert!(receiver.try_recv().is_none());
    assert!(!second.published.load(Ordering::Acquire));
    first.activate(&outbox);
    publisher.await.unwrap();
    first.wait(&outbox).await.unwrap();
    second.wait(&outbox).await.unwrap();
    for nonce in 1..=3 {
        assert!(
            matches!(receiver.try_recv(), Some(TxVerificationResult::Reject { tx_hash }) if tx_hash == tx(nonce).hash())
        );
    }
    assert!(receiver.try_recv().is_none());
    assert!(outbox.drained());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_reservation_creates_no_sequence_gap_and_returns_all_capacity() {
    let (outbox, endpoints, receiver) = fixture(Callbacks::new());
    let reservation = outbox.reserve(vec![effect(1)], Class::Remote).unwrap();
    assert_eq!(outbox.state.lock().usage[2].items, 1);
    drop(reservation);
    assert_eq!(outbox.state.lock().usage[2].items, 0);
    let committed = append(&outbox, vec![effect(2)]);
    committed.activate(&outbox);
    outbox.close();
    Arc::clone(&outbox).run(endpoints).await.unwrap();
    assert!(
        matches!(receiver.try_recv(), Some(TxVerificationResult::Reject { tx_hash }) if tx_hash == tx(2).hash())
    );
    assert!(outbox.drained());
}

#[test]
fn ordinary_saturation_preserves_trusted_and_critical_headroom() {
    let (outbox, _, _) = fixture(Callbacks::new());
    let mut remote = Vec::new();
    while let Ok(Some(reservation)) = outbox.reserve(vec![effect(1)], Class::Remote) {
        remote.push(reservation);
    }
    assert_eq!(
        remote.len(),
        crate::constants::EFFECT_JOURNAL_REMOTE_MAX_BATCHES
    );
    let mut trusted = Vec::new();
    while let Ok(Some(reservation)) = outbox.reserve(vec![effect(1)], Class::Trusted) {
        trusted.push(reservation);
    }
    assert_eq!(
        trusted.len(),
        crate::constants::EFFECT_TRUSTED_HEADROOM_BATCHES
    );
    let critical = outbox
        .reserve(vec![Effect::reset()], Class::Critical)
        .unwrap()
        .unwrap();
    assert!(matches!(
        outbox.reserve(vec![effect(1)], Class::Critical),
        Err(Error::Full(_))
    ));
    drop((remote, trusted, critical));
    assert_eq!(outbox.state.lock().usage[2].items, 0);
    assert!(!outbox.faulted.load(Ordering::Acquire));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn older_acknowledgement_cannot_erase_later_pending_rejection() {
    let (outbox, endpoints, _) = fixture(Callbacks::new());
    let hash = tx(1).hash();
    let rejected = |reason: &str| {
        Effect::rejected(
            &hash,
            Reject::Malformed(reason.into(), String::new()),
            None,
            false,
        )
        .unwrap()
    };
    let first = append(&outbox, vec![rejected("first")]);
    let second = append(&outbox, vec![rejected("second")]);
    first.activate(&outbox);
    let mut publisher = Box::pin(Arc::clone(&outbox).run(endpoints));
    poll_pending(publisher.as_mut());
    assert!(first.published.load(Ordering::Acquire));
    assert!(
        matches!(outbox.pending_reject(&hash), Some(Reject::Malformed(reason, _)) if reason == "second")
    );
    second.activate(&outbox);
    outbox.close();
    publisher.await.unwrap();
    assert!(outbox.pending_reject(&hash).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publisher_cancellation_faults_generation_and_releases_waiters_with_failure() {
    let (outbox, endpoints, _) = fixture(Callbacks::new());
    let batch = append(&outbox, vec![effect(1)]);
    let mut publisher = Box::pin(Arc::clone(&outbox).run(endpoints));
    poll_pending(publisher.as_mut());
    drop(publisher);
    assert!(matches!(batch.wait(&outbox).await, Err(Error::Fault(_))));
    assert!(!batch.published.load(Ordering::Acquire));
    assert_eq!(outbox.state.lock().usage[2].items, 1);
    assert!(!outbox.drained());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn callback_failure_disables_all_callback_kinds_while_later_relay_drains() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut callbacks = Callbacks::new();
    let observed = Arc::clone(&calls);
    callbacks.register_pending(Box::new(move |_| {
        observed.fetch_add(1, Ordering::AcqRel);
        panic!("injected callback failure");
    }));
    let other_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = Arc::clone(&other_calls);
    callbacks.register_proposed(Box::new(move |_| {
        observed.fetch_add(1, Ordering::AcqRel);
    }));
    let observed = Arc::clone(&other_calls);
    callbacks.register_reject(Box::new(move |_, _| {
        observed.fetch_add(1, Ordering::AcqRel);
    }));
    let (outbox, endpoints, receiver) = fixture(callbacks);
    let store = store();
    let owner = accept(&store, output_tx(33), 1, 1, Status::Pending);
    let owner = store.point(&owner).1.unwrap();
    let selected =
        super::super::membership::snapshot(&owner, Default::default(), Default::default()).unwrap();
    for nonce in 1..=2 {
        let mut event = effect(nonce);
        event.callback = Some(CallbackEvent::Pending(selected.clone()));
        append(&outbox, vec![event]).activate(&outbox);
    }
    for (nonce, callback) in [
        (3, CallbackEvent::Proposed(selected.clone())),
        (4, CallbackEvent::Reject(selected, Reject::Expiry(0))),
    ] {
        let mut event = effect(nonce);
        event.callback = Some(callback);
        append(&outbox, vec![event]).activate(&outbox);
    }
    outbox.close();
    Arc::clone(&outbox).run(endpoints).await.unwrap();
    assert_eq!(calls.load(Ordering::Acquire), 1);
    assert_eq!(other_calls.load(Ordering::Acquire), 0);
    for nonce in 1..=4 {
        assert!(matches!(
            receiver.try_recv(),
            Some(TxVerificationResult::Reject { tx_hash }) if tx_hash == tx(nonce).hash()
        ));
    }
    assert!(receiver.try_recv().is_none());
    assert!(outbox.drained());
    assert!(!outbox.faulted.load(Ordering::Acquire));
}

#[tokio::test(start_paused = true)]
async fn recent_write_errors_throttle_new_attempts_and_unwind_disables_them() {
    use std::cell::RefCell;
    let calls = RefCell::new(Vec::new());
    let mut writes = RecentWrites::default();
    writes.write(|| {
        calls.borrow_mut().push(1);
        Err(ckb_error::OtherError::new("controlled write error").into())
    });
    writes.write(|| {
        calls.borrow_mut().push(2);
        Ok(())
    });
    tokio::time::advance(Duration::from_millis(999)).await;
    writes.write(|| {
        calls.borrow_mut().push(3);
        Ok(())
    });
    assert_eq!(*calls.borrow(), [1]);
    tokio::time::advance(Duration::from_millis(1)).await;
    writes.write(|| {
        calls.borrow_mut().push(4);
        Ok(())
    });
    writes.write(|| {
        calls.borrow_mut().push(5);
        Ok(())
    });
    assert_eq!(*calls.borrow(), [1, 4, 5]);
    assert!(matches!(writes, RecentWrites::Available));

    writes.write(|| {
        calls.borrow_mut().push(6);
        panic!("controlled endpoint unwind");
    });
    tokio::time::advance(Duration::from_secs(2)).await;
    writes.write(|| {
        calls.borrow_mut().push(7);
        Ok(())
    });
    assert_eq!(*calls.borrow(), [1, 4, 5, 6]);
    assert!(matches!(writes, RecentWrites::Disabled));
}

#[tokio::test(start_paused = true)]
async fn recent_write_cooldown_settles_batches_and_later_records_without_replay() {
    let directory = tempfile::tempdir().unwrap();
    let recent = Arc::new(RecentReject::new(directory.path(), 100, -1).unwrap());
    let (outbox, mut endpoints, receiver) = fixture(Callbacks::new());
    endpoints.recent = Some(Arc::clone(&recent));
    endpoints
        .recent_writes
        .write(|| Err(ckb_error::OtherError::new("controlled write error").into()));
    let first_hash = tx(805).hash();
    let second_hash = tx(806).hash();
    let first = append(
        &outbox,
        vec![Effect::rejected(&first_hash, Reject::Expiry(0), None, true).unwrap()],
    );
    first.activate(&outbox);
    assert!(outbox.pending_reject(&first_hash).is_some());
    let mut publisher = Box::pin(Arc::clone(&outbox).run(endpoints));
    poll_pending(publisher.as_mut());
    assert!(first.published.load(Ordering::Acquire));
    assert!(outbox.pending_reject(&first_hash).is_none());
    assert!(recent.get(&first_hash).unwrap().is_none());
    assert!(matches!(
        receiver.try_recv(),
        Some(TxVerificationResult::Reject { tx_hash }) if tx_hash == first_hash
    ));

    tokio::time::advance(Duration::from_secs(1)).await;
    let second = append(
        &outbox,
        vec![Effect::rejected(&second_hash, Reject::Expiry(0), None, true).unwrap()],
    );
    second.activate(&outbox);
    outbox.close();
    publisher.await.unwrap();
    second.wait(&outbox).await.unwrap();
    assert!(recent.get(&second_hash).unwrap().is_some());
    assert!(recent.get(&first_hash).unwrap().is_none());
    assert!(outbox.pending_reject(&second_hash).is_none());
    assert!(matches!(
        receiver.try_recv(),
        Some(TxVerificationResult::Reject { tx_hash }) if tx_hash == second_hash
    ));
    assert!(receiver.try_recv().is_none());
    assert!(outbox.drained());
    assert!(!outbox.faulted.load(Ordering::Acquire));
}

#[test]
fn rejection_diagnostics_keep_policy_public_shape_and_bounded_owned_strings() {
    use ckb_error::ErrorKind;
    use ckb_types::core::error::TransactionError;
    let cases = [
        Reject::Verification(TransactionError::InvalidSince { index: 0 }.into()),
        Reject::Verification(
            ckb_script::ScriptError::Other("bounded script fixture".to_owned())
                .output_type_script(0)
                .into(),
        ),
        Reject::Verification(
            ErrorKind::Transaction.because(ErrorKind::Script.other("nested fixture")),
        ),
    ];
    for original in cases {
        let expected = original.to_string();
        let public = PoolTransactionReject::from(original.clone());
        let bounded = bound_reject_diagnostic(original);
        assert_eq!(bounded.to_string(), expected);
        assert_eq!(PoolTransactionReject::from(bounded), public);
    }
    let mut text = String::with_capacity(MAX_TX_POOL_REJECT_DESCRIPTION_BYTES * 4);
    text.push_str("short transient diagnostic");
    let Reject::Full(text) = bound_reject_diagnostic(Reject::Full(text)) else {
        panic!("same variant")
    };
    assert_eq!(text, "short transient diagnostic");
    assert!(text.capacity() <= MAX_TX_POOL_REJECT_DESCRIPTION_BYTES);
    let rejection = bound_reject_diagnostic(Reject::RBFRejected(
        "界".repeat(MAX_TX_POOL_REJECT_DESCRIPTION_BYTES),
    ));
    assert!(rejection.should_recorded());
    let Reject::RBFRejected(text) = rejection else {
        panic!("same RBF variant")
    };
    assert!(text.len() <= MAX_DYNAMIC_REJECT_TEXT_BYTES);
}

#[test]
fn excessive_verification_time_sends_negative_relay_without_persistent_rejection() {
    let effect = Effect::rejected(&tx(10).hash(), Reject::ExcessiveVerifyTime, None, true).unwrap();
    assert!(effect.recent.is_none());
    assert!(matches!(
        effect.relay,
        Some(TxVerificationResult::Reject { .. })
    ));
    assert!(matches!(
        PoolTransactionReject::from(Reject::ExcessiveVerifyTime),
        PoolTransactionReject::ExcessiveVerifyTime(_)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_rejection_in_one_batch_exposes_the_last_value_until_whole_batch_acknowledgement()
{
    let (outbox, endpoints, _) = fixture(Callbacks::new());
    let hash = tx(800).hash();
    let events = ["first", "second"].map(|reason| {
        Effect::rejected(
            &hash,
            Reject::Malformed(reason.into(), String::new()),
            None,
            false,
        )
        .unwrap()
    });
    let batch = append(&outbox, events.into());
    assert!(
        matches!(outbox.pending_reject(&hash), Some(Reject::Malformed(reason, _)) if reason == "second")
    );
    batch.activate(&outbox);
    outbox.close();
    Arc::clone(&outbox).run(endpoints).await.unwrap();
    assert!(outbox.pending_reject(&hash).is_none());
    assert!(outbox.drained());
}

#[test]
fn largest_admission_and_missing_parent_notice_shapes_fit_their_reserved_regions() {
    let store = store();
    let outbox = &store.outbox;
    let hash = accept(&store, output_tx(801), 1000, 1, Status::Pending);
    let owner = store.point(&hash).1.unwrap();
    let selected =
        super::super::membership::snapshot(&owner, Default::default(), Default::default()).unwrap();
    let mut events = Vec::with_capacity(outbox.batch_effects[0]);
    for _ in 1..outbox.batch_effects[0] {
        events.push(
            Effect::rejected(
                &hash,
                Reject::Malformed(
                    "x".repeat(MAX_TX_POOL_REJECT_DESCRIPTION_BYTES),
                    "y".repeat(MAX_TX_POOL_REJECT_DESCRIPTION_BYTES),
                ),
                Some(selected.clone()),
                true,
            )
            .unwrap(),
        );
    }
    events.push(Effect::accepted(selected, Status::Pending, None));
    for class in [Class::Remote, Class::Trusted] {
        let reservation = outbox.reserve(events.clone(), class).unwrap().unwrap();
        drop(reservation);
    }
    let parents = (0..store.budget.limits.per_job.edges)
        .map(|n| tx(n as u32).hash())
        .collect();
    let request = Effect {
        relay: Some(TxVerificationResult::UnknownParents {
            peer: 1.into(),
            parents,
        }),
        ..Effect::default()
    };
    let reservation = outbox
        .reserve(vec![request], Class::Remote)
        .unwrap()
        .unwrap();
    drop(reservation);
    let mut too_many = vec![effect(802); outbox.batch_effects[0] + 1];
    too_many.shrink_to_fit();
    assert!(matches!(
        outbox.reserve(too_many, Class::Remote),
        Err(Error::Full(FullReason::Other("indivisible notice batch")))
    ));
    assert!(!store.is_faulted());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn publisher_abort_keeps_a_running_callback_and_its_batch_owned_until_return() {
    let (entered, mut events) = tokio::sync::mpsc::unbounded_channel();
    let (release, waiting) = std::sync::mpsc::channel();
    let waiting = std::sync::Mutex::new(waiting);
    let mut callbacks = Callbacks::new();
    callbacks.register_pending(Box::new(move |_| {
        entered.send(()).unwrap();
        waiting
            .lock()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
    }));
    let (outbox, endpoints, _) = fixture(callbacks);
    let store = store();
    let hash = accept(&store, output_tx(803), 1000, 1, Status::Pending);
    let owner = store.point(&hash).1.unwrap();
    let selected =
        super::super::membership::snapshot(&owner, Default::default(), Default::default()).unwrap();
    let batch = append(
        &outbox,
        vec![Effect::accepted(selected, Status::Pending, None)],
    );
    batch.activate(&outbox);
    let publisher = tokio::spawn(Arc::clone(&outbox).run(endpoints));
    tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    publisher.abort();
    assert!(!publisher.is_finished());
    assert!(!batch.published.load(Ordering::Acquire));
    assert_eq!(outbox.state.lock().usage[2].items, 1);
    release.send(()).unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), publisher)
        .await
        .unwrap();
    assert!(result.unwrap_err().is_cancelled());
    assert!(batch.published.load(Ordering::Acquire));
    assert_eq!(outbox.state.lock().usage[2].items, 0);
    assert!(outbox.faulted.load(Ordering::Acquire));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unregistered_callbacks_do_not_cross_the_blocking_boundary() {
    let (_, mut endpoints, _) = fixture(Callbacks::new());
    let store = store();
    let hash = accept(&store, output_tx(804), 1, 1, Status::Pending);
    let owner = store.point(&hash).1.unwrap();
    let selected =
        super::super::membership::snapshot(&owner, Default::default(), Default::default()).unwrap();
    // Tokio forbids block_in_place inside a LocalSet, even on this multi-thread runtime.
    tokio::task::LocalSet::new()
        .run_until(async {
            for callback in [
                CallbackEvent::Pending(selected.clone()),
                CallbackEvent::Proposed(selected.clone()),
                CallbackEvent::Reject(selected, Reject::Full("fixture".into())),
            ] {
                endpoints.publish(&Effect {
                    callback: Some(callback),
                    ..Effect::default()
                });
            }
        })
        .await;
    assert!(!endpoints.callbacks_disabled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publication_selection_excludes_later_activation_and_append() {
    let (entered, mut events) = tokio::sync::mpsc::unbounded_channel();
    let (release, waiting) = std::sync::mpsc::channel();
    let waiting = std::sync::Mutex::new(waiting);
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let mut callbacks = Callbacks::new();
    callbacks.register_pending(Box::new(move |_| {
        if calls.fetch_add(1, Ordering::AcqRel) == 0 {
            entered.send(()).unwrap();
            waiting
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        }
    }));
    let (outbox, mut endpoints, receiver) = fixture(callbacks);
    let first = append(&outbox, vec![callback_effect(805)]);
    let second = append(&outbox, vec![callback_effect(806)]);
    let third = append(&outbox, vec![effect(807)]);
    let fourth = append(&outbox, vec![effect(808)]);
    first.activate(&outbox);
    second.activate(&outbox);
    fourth.activate(&outbox);
    let publishing = Arc::clone(&outbox);
    let operation = tokio::spawn(async move {
        let result = publishing.publish_ready(&mut endpoints);
        (result, endpoints)
    });
    tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    third.activate(&outbox);
    let fifth = append(&outbox, vec![effect(809)]);
    fifth.activate(&outbox);
    outbox.close();
    release.send(()).unwrap();
    let (result, endpoints) = operation.await.unwrap();
    assert!(result.unwrap());
    assert!(first.published.load(Ordering::Acquire));
    assert!(second.published.load(Ordering::Acquire));
    for batch in [&third, &fourth, &fifth] {
        assert!(!batch.published.load(Ordering::Acquire));
    }
    Arc::clone(&outbox).run(endpoints).await.unwrap();
    for nonce in 805..=809 {
        assert!(
            matches!(receiver.try_recv(), Some(TxVerificationResult::Reject { tx_hash }) if tx_hash == tx(nonce).hash())
        );
    }
    assert!(receiver.try_recv().is_none());
    assert!(outbox.drained());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn later_callback_observes_prior_batch_settled_and_released() {
    for cancel in [false, true] {
        let (entered, mut events) = tokio::sync::mpsc::unbounded_channel();
        let (release, waiting) = std::sync::mpsc::channel();
        let waiting = std::sync::Mutex::new(waiting);
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let mut callbacks = Callbacks::new();
        callbacks.register_pending(Box::new(move |_| {
            if calls.fetch_add(1, Ordering::AcqRel) == 1 {
                entered.send(()).unwrap();
                waiting
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }
        }));
        let (outbox, endpoints, _) = fixture(callbacks);
        let first = append(&outbox, vec![callback_effect(809)]);
        let second = append(&outbox, vec![callback_effect(810)]);
        first.activate(&outbox);
        second.activate(&outbox);
        if !cancel {
            outbox.close();
        }
        let publisher = tokio::spawn(Arc::clone(&outbox).run(endpoints));
        tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap();
        first.wait(&outbox).await.unwrap();
        assert_eq!(outbox.state.lock().usage[2].items, 1);
        assert_eq!(Arc::strong_count(&first), 1);
        let released = Arc::downgrade(&first);
        drop(first);
        assert!(released.upgrade().is_none());
        assert!(!second.published.load(Ordering::Acquire));
        if cancel {
            publisher.abort();
            assert!(!publisher.is_finished());
        }
        release.send(()).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), publisher)
            .await
            .unwrap();
        if cancel {
            assert!(result.unwrap_err().is_cancelled());
            assert!(outbox.faulted.load(Ordering::Acquire));
            outbox.close();
        } else {
            result.unwrap().unwrap();
            assert!(!outbox.faulted.load(Ordering::Acquire));
        }
        assert!(second.published.load(Ordering::Acquire));
        assert!(outbox.drained());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unregistered_callback_prefix_is_bounded_without_a_blocking_boundary() {
    let (outbox, mut endpoints, receiver) = fixture(Callbacks::new());
    let callback = callback_effect(811).callback;
    let batches = (0..PUBLISH_BATCH_LIMIT + 1)
        .map(|index| {
            let batch = append(
                &outbox,
                vec![Effect {
                    callback: callback.clone(),
                    ..effect(812 + u32::try_from(index).unwrap())
                }],
            );
            batch.activate(&outbox);
            batch
        })
        .collect::<Vec<_>>();
    outbox.close();
    // An unconditional outer block_in_place would panic inside this LocalSet.
    tokio::task::LocalSet::new()
        .run_until(async {
            assert!(outbox.publish_ready(&mut endpoints).unwrap());
            assert!(
                batches[..PUBLISH_BATCH_LIMIT]
                    .iter()
                    .all(|batch| batch.published.load(Ordering::Acquire))
            );
            assert!(
                !batches[PUBLISH_BATCH_LIMIT]
                    .published
                    .load(Ordering::Acquire)
            );
            assert_eq!(outbox.state.lock().usage[2].items, 1);
            assert!(outbox.publish_ready(&mut endpoints).unwrap());
            assert!(!outbox.publish_ready(&mut endpoints).unwrap());
        })
        .await;
    for index in 0..PUBLISH_BATCH_LIMIT + 1 {
        assert!(
            matches!(receiver.try_recv(), Some(TxVerificationResult::Reject { tx_hash }) if tx_hash == tx(812 + u32::try_from(index).unwrap()).hash())
        );
    }
    assert!(receiver.try_recv().is_none());
    assert!(outbox.drained());
}
