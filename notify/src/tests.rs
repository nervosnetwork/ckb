use super::*;
use ckb_types::core::{Capacity, TransactionBuilder};
use std::sync::Arc;

fn service() -> NotifyService {
    NotifyService::new(
        NotifyConfig::default(),
        Handle::new(tokio::runtime::Handle::current(), None),
    )
}

fn entry(timestamp: u64) -> PoolTransactionEntry {
    PoolTransactionEntry {
        transaction: TransactionBuilder::default().build(),
        cycles: 0,
        size: 0,
        fee: Capacity::zero(),
        timestamp,
    }
}

#[tokio::test]
async fn transaction_ingress_is_bounded_and_recovers_without_deferred_sends() {
    let mut controller = service().start();
    let (pending_tx, mut pending_rx) = mpsc::channel(1);
    let (proposed_tx, mut proposed_rx) = mpsc::channel(1);
    let (reject_tx, mut reject_rx) = mpsc::channel(1);
    controller.new_transaction_notifier = pending_tx;
    controller.proposed_transaction_notifier = proposed_tx;
    controller.reject_transaction_notifier = reject_tx;

    for timestamp in [1, 2] {
        controller.notify_new_transaction(entry(timestamp));
        controller.notify_proposed_transaction(entry(timestamp));
        controller
            .notify_reject_transaction(entry(timestamp), Reject::ExceededMaximumAncestorsCount);
    }
    // No task has been polled: handoff is already complete and overflow omitted.
    assert_eq!(pending_rx.try_recv().unwrap().timestamp, 1);
    assert_eq!(proposed_rx.try_recv().unwrap().timestamp, 1);
    assert_eq!(reject_rx.try_recv().unwrap().0.timestamp, 1);
    controller.notify_new_transaction(entry(3));
    controller.notify_proposed_transaction(entry(3));
    controller.notify_reject_transaction(entry(3), Reject::ExceededMaximumAncestorsCount);
    assert_eq!(pending_rx.try_recv().unwrap().timestamp, 3);
    assert_eq!(proposed_rx.try_recv().unwrap().timestamp, 3);
    assert_eq!(reject_rx.try_recv().unwrap().0.timestamp, 3);
    tokio::task::yield_now().await;
    assert!(pending_rx.try_recv().is_err());
    assert!(proposed_rx.try_recv().is_err());
    assert!(reject_rx.try_recv().is_err());
}

#[tokio::test]
async fn full_subscribers_drop_immediately_and_resume_without_delaying_fast_subscribers() {
    let mut service = NotifyService::new(
        NotifyConfig {
            notify_tx_timeout: Some(7_000),
            ..NotifyConfig::default()
        },
        Handle::new(tokio::runtime::Handle::current(), None),
    );
    let (pending_tx, mut pending_rx) = mpsc::channel(1);
    let (proposed_tx, mut proposed_rx) = mpsc::channel(1);
    let (reject_tx, mut reject_rx) = mpsc::channel(1);
    pending_tx.try_send(entry(0)).unwrap();
    proposed_tx.try_send(entry(0)).unwrap();
    reject_tx
        .try_send((entry(0), Reject::ExceededMaximumAncestorsCount))
        .unwrap();
    service
        .new_transaction_subscribers
        .insert("slow".into(), pending_tx);
    service
        .proposed_transaction_subscribers
        .insert("slow".into(), proposed_tx);
    service
        .reject_transaction_subscribers
        .insert("slow".into(), reject_tx);
    let (fast_pending, mut fast_pending_rx) = mpsc::channel(1);
    let (fast_proposed, mut fast_proposed_rx) = mpsc::channel(1);
    let (fast_reject, mut fast_reject_rx) = mpsc::channel(1);
    service
        .new_transaction_subscribers
        .insert("fast".into(), fast_pending);
    service
        .proposed_transaction_subscribers
        .insert("fast".into(), fast_proposed);
    service
        .reject_transaction_subscribers
        .insert("fast".into(), fast_reject);

    for timestamp in [1, 2] {
        service.handle_notify_new_transaction(entry(timestamp));
        service.handle_notify_proposed_transaction(entry(timestamp));
        service.handle_notify_reject_transaction((
            entry(timestamp),
            Reject::ExceededMaximumAncestorsCount,
        ));
        assert_eq!(fast_pending_rx.try_recv().unwrap().timestamp, timestamp);
        assert_eq!(fast_proposed_rx.try_recv().unwrap().timestamp, timestamp);
        assert_eq!(fast_reject_rx.try_recv().unwrap().0.timestamp, timestamp);
        let expected = if timestamp == 1 { 0 } else { 2 };
        assert_eq!(pending_rx.try_recv().unwrap().timestamp, expected);
        assert_eq!(proposed_rx.try_recv().unwrap().timestamp, expected);
        assert_eq!(reject_rx.try_recv().unwrap().0.timestamp, expected);
        // Even an old nonzero timeout cannot retain or replay the refused event.
        tokio::task::yield_now().await;
        assert!(pending_rx.try_recv().is_err());
        assert!(proposed_rx.try_recv().is_err());
        assert!(reject_rx.try_recv().is_err());
    }
}

#[test]
fn full_and_closed_transaction_handoffs_release_their_payload_ownership() {
    let (sender, mut receiver) = mpsc::channel(1);
    let queued = Arc::new(vec![0u8; 1024]);
    let queued_owner = Arc::downgrade(&queued);
    try_notify_transaction(&sender, queued);
    assert!(queued_owner.upgrade().is_some());

    let refused = Arc::new(vec![1u8; 1024]);
    let refused_owner = Arc::downgrade(&refused);
    try_notify_transaction(&sender, refused);
    assert!(
        refused_owner.upgrade().is_none(),
        "full send retains no payload"
    );
    drop(receiver.try_recv().unwrap());
    assert!(
        queued_owner.upgrade().is_none(),
        "consumption releases the queued payload"
    );

    receiver.close();
    let closed = Arc::new(vec![2u8; 1024]);
    let closed_owner = Arc::downgrade(&closed);
    try_notify_transaction(&sender, closed);
    assert!(
        closed_owner.upgrade().is_none(),
        "closed send retains no payload"
    );
}
