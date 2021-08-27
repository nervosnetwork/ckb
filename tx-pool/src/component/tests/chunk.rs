use ckb_types::core::TransactionBuilder;

use crate::component::chunk::{ChunkQueue, Entry};

#[test]
fn basic() {
    let tx = TransactionBuilder::default().build();
    let entry = Entry {
        tx: tx.clone(),
        remote: None,
    };
    let id = tx.proposal_short_id();
    let mut queue = ChunkQueue::new();

    assert!(queue.add_tx(tx.clone()));
    assert_eq!(queue.pop_front().as_ref(), Some(&entry));
    assert!(queue.contains_key(&id));
    assert!(!queue.add_tx(tx));

    queue.clean_front();
    assert!(!queue.contains_key(&id));
}
