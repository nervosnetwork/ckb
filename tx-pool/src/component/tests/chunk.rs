use ckb_types::core::TransactionBuilder;
use tokio::sync::watch;

use crate::verify_queue::{Entry, VerifyQueue};

#[test]
fn basic() {
    let tx = TransactionBuilder::default().build();
    let entry = Entry {
        tx: tx.clone(),
        remote: None,
    };
    let id = tx.proposal_short_id();
    let (queue_tx, _queue_rx) = watch::channel(0 as usize);
    let mut queue = VerifyQueue::new(queue_tx);

    assert!(queue.add_tx(tx.clone(), None));
    assert_eq!(queue.pop_first().as_ref(), Some(&entry));
    assert!(!queue.contains_key(&id));
    assert!(queue.add_tx(tx, None));

    queue.clear();
    assert!(!queue.contains_key(&id));
}
