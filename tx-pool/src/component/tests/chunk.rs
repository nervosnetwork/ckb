use crate::component::tests::util::build_tx;
use crate::component::verify_queue::{Entry, VerifyQueue};
use ckb_types::core::TransactionBuilder;
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
    let (queue_tx, mut queue_rx) = watch::channel(0_usize);
    let (exit_tx, mut exit_rx) = watch::channel(());
    let mut queue = VerifyQueue::new(queue_tx);
    let count = tokio::spawn(async move {
        let mut counts = vec![];
        loop {
            select! {
                _ = queue_rx.changed() => {
                    let value = queue_rx.borrow().to_owned();
                    counts.push(value);
                }
                _ = exit_rx.changed() => {
                    break;
                }
            }
        }
        counts
    });

    assert!(queue.add_tx(tx.clone(), None).unwrap());
    sleep(std::time::Duration::from_millis(100)).await;

    assert!(!queue.add_tx(tx.clone(), None).unwrap());

    assert_eq!(queue.pop_first().as_ref(), Some(&entry));
    assert!(!queue.contains_key(&id));

    assert!(queue.add_tx(tx.clone(), None).unwrap());
    sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(queue.pop_first().as_ref(), Some(&entry));

    assert!(queue.add_tx(tx.clone(), None).unwrap());
    sleep(std::time::Duration::from_millis(100)).await;

    assert!(queue.add_tx(tx2.clone(), None).unwrap());
    sleep(std::time::Duration::from_millis(100)).await;

    exit_tx.send(()).unwrap();
    let counts = count.await.unwrap();
    assert_eq!(counts, vec![1, 1, 1, 2]);

    queue.clear();
    assert!(!queue.contains_key(&id));
}
