use crate::component::tests::util::build_tx;
use crate::component::verify_queue::SortKey;
use crate::component::verify_queue::{Entry, VerifyQueue};
use ckb_types::core::FeeRate;
use ckb_types::core::{Capacity, TransactionBuilder};
use ckb_types::prelude::Pack;
use ckb_types::H256;
use tokio::select;
use tokio::sync::watch;
use tokio::time::sleep;

#[tokio::test]
async fn verify_queue_basic() {
    let tx = TransactionBuilder::default().build();
    let entry = Entry {
        tx: tx.clone(),
        remote: None,
    };
    let tx2 = build_tx(vec![(&tx.hash(), 0)], 1);

    let id = tx.proposal_short_id();
    let (exit_tx, mut exit_rx) = watch::channel(());
    let mut queue = VerifyQueue::new();
    let queue_rx = queue.subscribe();
    let count = tokio::spawn(async move {
        let mut count = 0;
        loop {
            select! {
                _ = queue_rx.notified() => {
                    count += 1;
                }
                _ = exit_rx.changed() => {
                    break;
                }
            }
        }
        count
    });

    assert!(queue
        .add_tx(tx.clone(), Capacity::shannons(1_u64), 2, None)
        .unwrap());
    sleep(std::time::Duration::from_millis(100)).await;

    assert!(!queue
        .add_tx(tx.clone(), Capacity::shannons(1_u64), 2, None)
        .unwrap());

    assert_eq!(queue.pop_first().as_ref(), Some(&entry));
    assert!(!queue.contains_key(&id));

    assert!(queue
        .add_tx(tx.clone(), Capacity::shannons(1_u64), 2, None)
        .unwrap());
    sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(queue.pop_first().as_ref(), Some(&entry));

    assert!(queue
        .add_tx(tx.clone(), Capacity::shannons(1_u64), 2, None)
        .unwrap());
    sleep(std::time::Duration::from_millis(100)).await;

    assert!(queue
        .add_tx(tx2.clone(), Capacity::shannons(1_u64), 2, None)
        .unwrap());
    sleep(std::time::Duration::from_millis(100)).await;

    exit_tx.send(()).unwrap();
    let counts = count.await.unwrap();
    assert_eq!(counts, 4);

    let cur = queue.pop_first();
    assert_eq!(cur.unwrap().tx, tx);

    assert!(!queue.is_empty());
    let cur = queue.pop_first();
    assert_eq!(cur.unwrap().tx, tx2);

    assert!(queue.is_empty());

    queue.clear();
    assert!(!queue.contains_key(&id));
}

#[test]

fn test_verify_sort_key() {
    let key1 = SortKey {
        added_time: 10,
        fee_rate: FeeRate(10),
    };
    let key2 = SortKey {
        added_time: 10,
        fee_rate: FeeRate(10),
    };
    let key3 = SortKey {
        added_time: 10,
        fee_rate: FeeRate(20),
    };
    let key4 = SortKey {
        added_time: 20,
        fee_rate: FeeRate(10),
    };

    assert!(key1 == key2);
    assert!(key1 > key3);
    assert!(key1 < key4);
}

#[tokio::test]

async fn test_verify_order() {
    let (exit_tx, mut exit_rx) = watch::channel(());
    let mut queue = VerifyQueue::new();
    let queue_rx = queue.subscribe();
    let count = tokio::spawn(async move {
        let mut count = 0;
        loop {
            select! {
                _ = queue_rx.notified() => {
                    count += 1;
                }
                _ = exit_rx.changed() => {
                    break;
                }
            }
        }
        count
    });

    let tx1 = build_tx(vec![(&H256([1; 32]).pack(), 0)], 1);
    assert!(queue
        .add_tx(tx1.clone(), Capacity::shannons(10_u64), 20, None)
        .unwrap());
    sleep(std::time::Duration::from_millis(100)).await;
    // tx1 should be the only tx in the queue

    let tx2 = build_tx(vec![(&H256([2; 32]).pack(), 0)], 1);
    assert!(queue
        .add_tx(tx2.clone(), Capacity::shannons(20_u64), 20, None)
        .unwrap());
    sleep(std::time::Duration::from_millis(100)).await;
    // now queue should be sorted by fee_rate (tx2, tx1), tx2 with higher fee, same size

    // tx2 should be the first tx in the queue
    let cur = queue.pop_first();
    assert_eq!(cur.unwrap().tx, tx2);

    let tx3 = build_tx(vec![(&H256([3; 32]).pack(), 0)], 1);
    assert!(queue
        .add_tx(tx3.clone(), Capacity::shannons(10_u64), 20, None)
        .unwrap());
    sleep(std::time::Duration::from_millis(100)).await;
    // now queue should be sorted by fee_rate (tx1, tx3), tx3 with same fee rate, but comes later

    // tx1 should be the first tx in the queue
    let cur = queue.pop_first();
    assert_eq!(cur.unwrap().tx, tx1);

    let tx4 = build_tx(vec![(&H256([4; 32]).pack(), 0)], 1);
    assert!(queue
        .add_tx(tx4.clone(), Capacity::shannons(10_u64), 19, None)
        .unwrap());
    sleep(std::time::Duration::from_millis(100)).await;
    // now queue should be sorted by fee_rate (tx4, tx3), tx4 is with smaller size, so fee rate is higher

    let cur = queue.pop_first();
    assert_eq!(cur.unwrap().tx, tx4);

    let cur = queue.pop_first();
    assert_eq!(cur.unwrap().tx, tx3);

    let cur = queue.pop_first();
    assert_eq!(cur, None);

    exit_tx.send(()).unwrap();
    assert_eq!(count.await.unwrap(), 4_usize);
}
