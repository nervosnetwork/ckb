use crate::component::tests::util::build_tx;
use crate::component::verify_queue::{Entry, VerifyQueue};
use ckb_network::SessionId;
use ckb_types::H256;
use ckb_types::core::TransactionBuilder;
use ckb_types::prelude::Pack;
use tokio::select;
use tokio::sync::watch;
use tokio::time::sleep;

const MAX_TX_VERIFY_CYCLES: u64 = 70_000_000;
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
    let mut queue = VerifyQueue::new(MAX_TX_VERIFY_CYCLES);
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

    assert!(queue.add_tx(tx.clone(), None).unwrap());
    sleep(std::time::Duration::from_millis(100)).await;

    assert!(!queue.add_tx(tx.clone(), None).unwrap());

    assert_eq!(queue.pop_front(false).as_ref(), Some(&entry));
    assert!(!queue.contains_key(&id));

    assert!(queue.add_tx(tx.clone(), None).unwrap());
    sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(queue.pop_front(false).as_ref(), Some(&entry));

    assert!(queue.add_tx(tx.clone(), None).unwrap());
    sleep(std::time::Duration::from_millis(100)).await;

    assert!(queue.add_tx(tx2.clone(), None).unwrap());
    sleep(std::time::Duration::from_millis(100)).await;

    exit_tx.send(()).unwrap();
    let counts = count.await.unwrap();
    assert_eq!(counts, 4);

    let cur = queue.pop_front(false);
    assert_eq!(cur.unwrap().tx, tx);

    assert!(!queue.is_empty());
    let cur = queue.pop_front(false);
    assert_eq!(cur.unwrap().tx, tx2);

    assert!(queue.is_empty());

    queue.clear();
    assert!(!queue.contains_key(&id));
}

#[tokio::test]
async fn test_verify_different_cycles() {
    let (exit_tx, mut exit_rx) = watch::channel(());
    let mut queue = VerifyQueue::new(MAX_TX_VERIFY_CYCLES);
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

    let remote = |cycles| Some((cycles, SessionId::default()));

    let tx0 = build_tx(vec![(&H256([0; 32]).pack(), 0)], 1);
    assert!(queue.add_tx(tx0.clone(), remote(1001)).unwrap());
    sleep(std::time::Duration::from_millis(100)).await;

    let tx1 = build_tx(vec![(&H256([1; 32]).pack(), 0)], 1);
    assert!(
        queue
            .add_tx(tx1.clone(), remote(MAX_TX_VERIFY_CYCLES + 1))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;

    let tx2 = build_tx(vec![(&H256([2; 32]).pack(), 0)], 1);
    assert!(queue.add_tx(tx2.clone(), remote(1001)).unwrap());
    sleep(std::time::Duration::from_millis(100)).await;
    // now queue should be sorted by time (tx1, tx2)

    let tx3 = build_tx(vec![(&H256([3; 32]).pack(), 0)], 1);
    assert!(queue.add_tx(tx3.clone(), remote(1001)).unwrap());
    sleep(std::time::Duration::from_millis(100)).await;

    let tx_size_sum = [&tx0, &tx1, &tx2, &tx3]
        .iter()
        .map(|tx| tx.data().serialized_size_in_block())
        .sum::<usize>();

    assert_eq!(queue.total_tx_size(), tx_size_sum);

    // tx0 should be the first tx in the queue
    let cur = queue.pop_front(true);
    assert_eq!(cur.unwrap().tx, tx0);

    let cur = queue.pop_front(true);
    assert_eq!(cur.unwrap().tx, tx2);

    let cur = queue.pop_front(true);
    assert_eq!(cur.unwrap().tx, tx3);

    // now there is no small cycle tx
    let cur = queue.pop_front(true);
    assert!(cur.is_none());

    // pop the tx with the large cycle
    let cur = queue.pop_front(false);
    assert_eq!(cur.unwrap().tx, tx1);

    let cur = queue.pop_front(false);
    assert!(cur.is_none());

    exit_tx.send(()).unwrap();
    let counts = count.await.unwrap();
    assert_eq!(counts, 4);
    assert_eq!(queue.total_tx_size(), 0);
}

#[tokio::test]
async fn verify_queue_remove() {
    let entry1 = Entry {
        tx: TransactionBuilder::default()
            .set_outputs_data(vec![Default::default()])
            .build(),
        remote: Some((1, SessionId::new(1))),
    };
    let entry1_id = entry1.tx.proposal_short_id();
    eprintln!("entry1_id: {:?}", entry1_id);
    let entry2 = Entry {
        tx: TransactionBuilder::default()
            .set_cell_deps(vec![Default::default(), Default::default()])
            .build(),
        remote: Some((2, SessionId::new(2))),
    };
    let entry2_id = entry2.tx.proposal_short_id();
    eprintln!("entry2_id: {:?}", entry2_id);
    let entry3 = Entry {
        tx: TransactionBuilder::default().build(),
        remote: None,
    };
    let entry3_id = entry3.tx.proposal_short_id();
    eprintln!("entry3_id: {:?}", entry3_id);

    let entry4 = Entry {
        tx: TransactionBuilder::default()
            .set_cell_deps(vec![
                Default::default(),
                Default::default(),
                Default::default(),
            ])
            .build(),
        remote: Some((4, SessionId::new(1))),
    };
    let entry4_id = entry4.tx.proposal_short_id();

    let mut queue = VerifyQueue::new(MAX_TX_VERIFY_CYCLES);

    assert!(
        queue
            .add_tx(entry1.tx.clone(), entry1.remote)
            .unwrap()
    );
    assert!(
        queue
            .add_tx(entry2.tx.clone(), entry2.remote)
            .unwrap()
    );
    assert!(
        queue
            .add_tx(entry3.tx.clone(), entry3.remote)
            .unwrap()
    );
    assert!(
        queue
            .add_tx(entry4.tx.clone(), entry4.remote)
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;

    assert!(queue.contains_key(&entry1_id));
    assert!(queue.contains_key(&entry2_id));
    assert!(queue.contains_key(&entry3_id));
    assert!(queue.contains_key(&entry4_id));

    queue.remove_txs_by_peer(&SessionId::new(1));

    assert!(!queue.contains_key(&entry1_id));
    assert!(!queue.contains_key(&entry4_id));
    assert!(queue.contains_key(&entry2_id));
    assert!(queue.contains_key(&entry3_id));
}
