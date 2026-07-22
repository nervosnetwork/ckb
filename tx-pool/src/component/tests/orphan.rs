use crate::component::tests::util::build_tx;
use crate::component::waiting_room::{WaitReason, WaitingRoom};
use crate::tx_source::TxSource;
use ckb_types::core::TransactionView;
use ckb_types::packed::Byte32;

/// Park a transaction in the waiting room as a parents-missing orphan.
fn park(room: &mut WaitingRoom, tx: TransactionView) {
    let reason = WaitReason::ParentsMissing {
        parents: tx.unique_parents(),
    };
    room.wait(tx, TxSource::Local, reason);
}

#[test]
fn test_orphan() {
    let tx1 = build_tx(vec![(&Byte32::zero(), 1), (&Byte32::zero(), 2)], 1);
    let mut orphan = WaitingRoom::new();
    assert_eq!(orphan.len(), 0);
    assert!(!orphan.contains_key(&tx1.proposal_short_id()));

    park(&mut orphan, tx1.clone());
    assert_eq!(orphan.len(), 1);

    park(&mut orphan, tx1.clone());
    assert_eq!(orphan.len(), 1);

    let tx2 = build_tx(vec![(&tx1.hash(), 0)], 1);
    park(&mut orphan, tx2.clone());
    assert_eq!(orphan.len(), 2);

    orphan.remove(&tx1.proposal_short_id());
    assert_eq!(orphan.len(), 1);
    orphan.remove(&tx2.proposal_short_id());
    assert_eq!(orphan.len(), 0);
}

#[test]
fn test_orphan_allows_double_spends_of_unknown_input() {
    let parent = build_tx(vec![(&Byte32::zero(), 1)], 1);
    let parent_hash = parent.hash();
    let tx1 = build_tx(vec![(&parent_hash, 0)], 1);
    let tx2 = build_tx(vec![(&parent_hash, 0)], 2);
    let mut orphan = WaitingRoom::new();

    park(&mut orphan, tx1.clone());
    park(&mut orphan, tx2.clone());

    assert_eq!(orphan.len(), 2);
    let txs = orphan.find_by_parent(&parent);
    assert_eq!(txs.len(), 2);
    assert!(txs.contains(&&tx1.proposal_short_id()));
    assert!(txs.contains(&&tx2.proposal_short_id()));
}

#[test]
fn test_orphan_duplicated() {
    let tx1 = build_tx(vec![(&Byte32::zero(), 1), (&Byte32::zero(), 2)], 3);
    let mut orphan = WaitingRoom::new();

    let tx2 = build_tx(vec![(&tx1.hash(), 0)], 1);
    let tx3 = build_tx(vec![(&tx2.hash(), 0)], 1);
    let tx4 = build_tx(vec![(&tx3.hash(), 0), (&tx1.hash(), 1)], 1);
    let tx5 = build_tx(vec![(&tx1.hash(), 0)], 2);
    park(&mut orphan, tx1.clone());
    park(&mut orphan, tx2.clone());
    park(&mut orphan, tx3);
    park(&mut orphan, tx4.clone());
    park(&mut orphan, tx5.clone());
    assert_eq!(orphan.len(), 5);

    let txs = orphan.find_by_parent(&tx2);
    assert_eq!(txs.len(), 1);

    let txs = orphan.find_by_parent(&tx1);
    assert_eq!(txs.len(), 3);
    assert!(txs.contains(&&tx2.proposal_short_id()));
    assert!(txs.contains(&&tx4.proposal_short_id()));
    assert!(txs.contains(&&tx5.proposal_short_id()));

    orphan.remove(&tx4.proposal_short_id());
    let txs = orphan.find_by_parent(&tx1);
    assert_eq!(txs.len(), 2);
    assert!(txs.contains(&&tx2.proposal_short_id()));
    assert!(txs.contains(&&tx5.proposal_short_id()));
}

/// The orphan-insert path rechecks parent availability under the orphan
/// lock: a parent that landed in the window between the failed resolve and
/// the insert must route the tx straight into the pipeline instead of
/// parking it in the orphan pool until expiry.
#[tokio::test]
async fn orphan_insert_rechecks_parent_availability() {
    use crate::component::entry::TxEntry;
    use crate::component::pipeline_queue::PipelineQueue;
    use crate::component::pool_map::Status;
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::component::tests::util::{MOCK_CYCLES, MOCK_SIZE};
    use ckb_types::core::Capacity;

    let service = harness(2).workers(WorkerSet::None).build().service;
    let parent = build_tx(vec![(&Byte32::zero(), 7)], 1);
    let child = build_tx(vec![(&parent.hash(), 0)], 1);
    let child_id = child.proposal_short_id();

    // The parent lands in the pool *before* the orphan handling runs — the
    // race window that previously stranded the child in the orphan pool.
    {
        let mut tx_pool = service.pool.tx_pool.write().await;
        tx_pool
            .pool_map
            .add_entry(
                TxEntry::dummy_resolve(
                    parent.clone(),
                    MOCK_CYCLES,
                    Capacity::shannons(1_000),
                    MOCK_SIZE,
                ),
                Status::Pending,
            )
            .unwrap();
    }

    service
        .handle_missing_input_orphan(
            child.clone(),
            TxSource::Remote {
                cycles: 0,
                peer: 1.into(),
            },
            child.unique_parents(),
        )
        .await;

    assert!(
        !service
            .pipeline
            .waiting_room
            .read()
            .await
            .contains_key(&child_id),
        "must not be parked in the orphan pool when its parent is available"
    );
    let in_verify = service
        .pipeline
        .queues
        .verify_queue
        .read()
        .await
        .contains_key(&child_id);
    let in_ordered = service
        .pipeline
        .queues
        .ordered_resolve_queue
        .read()
        .await
        .contains_key(&child_id);
    assert!(
        in_verify || in_ordered,
        "must be routed into the pipeline instead"
    );
}

/// A proposal/local tx rejected with missing inputs goes to the orphan pool
/// just like a remote one — it must not be dropped with a recent_reject
/// record while its remote counterpart would have been parked.
#[tokio::test]
async fn proposal_tx_with_missing_input_goes_to_orphan() {
    use crate::component::tests::harness::{WorkerSet, harness};
    use ckb_types::core::error::OutPointError;
    use ckb_types::packed::OutPoint;

    let service = harness(2).workers(WorkerSet::None).build().service;
    let parent = build_tx(vec![(&Byte32::zero(), 9)], 1);
    let child = build_tx(vec![(&parent.hash(), 0)], 1);
    let child_id = child.proposal_short_id();

    // Simulate the reorg-detach outcome: the proposal tx failed
    // tip-revalidation because its parent is no longer known.
    let reject =
        crate::error::Reject::Resolve(OutPointError::Unknown(OutPoint::new(parent.hash(), 0)));
    service
        .after_process(child.clone(), TxSource::Proposal, &Err(reject))
        .await;

    assert!(
        service
            .pipeline
            .waiting_room
            .read()
            .await
            .contains_key(&child_id),
        "proposal tx with missing inputs must be parked in the orphan pool"
    );
}

/// `process_orphan_tx` must not reclassify orphans that still have
/// unavailable parents: each round trip would refresh the orphan's expiry
/// and re-notify the relayer.
#[tokio::test]
async fn process_orphan_tx_skips_still_unavailable_orphans() {
    use crate::component::tests::harness::{WorkerSet, harness};

    let service = harness(2).workers(WorkerSet::None).build().service;
    let parent_p = build_tx(vec![(&Byte32::zero(), 11)], 1);
    let parent_q = build_tx(vec![(&Byte32::zero(), 12)], 1);
    let child = build_tx(vec![(&parent_p.hash(), 0), (&parent_q.hash(), 0)], 1);
    let child_id = child.proposal_short_id();

    // Park the child (both parents missing).
    service
        .handle_missing_input_orphan(child.clone(), TxSource::Local, child.unique_parents())
        .await;
    assert!(
        service
            .pipeline
            .waiting_room
            .read()
            .await
            .contains_key(&child_id)
    );

    // P "lands" but Q is still missing: the child must stay put, not bounce
    // through the pipeline and back.
    service.process_orphan_tx(&parent_p).await;
    assert!(
        service
            .pipeline
            .waiting_room
            .read()
            .await
            .contains_key(&child_id),
        "orphan with a still-missing parent must not be reclassified"
    );
}

/// The waiting room must evict by total serialized bytes, not just by entry
/// count: a flood of max-size transactions stays within the byte budget.
#[test]
fn test_orphan_pool_evicts_by_total_bytes() {
    use ckb_types::bytes::Bytes;
    use ckb_types::core::TransactionBuilder;
    use ckb_types::packed::{CellInput, OutPoint};
    use ckb_types::prelude::*;

    let mut orphan = WaitingRoom::new();
    let one_mb = vec![0u8; 1_000_000];
    let mut saw_eviction = false;
    for i in 0..21u8 {
        let tx = TransactionBuilder::default()
            .input(CellInput::new(
                OutPoint::new(Byte32::new([i + 1; 32]), 0),
                0,
            ))
            .set_outputs_data(vec![Bytes::from(one_mb.clone()).pack()])
            .build();
        let (_retained, evicted) = {
            let reason = WaitReason::ParentsMissing {
                parents: tx.unique_parents(),
            };
            orphan.wait(tx, TxSource::Local, reason)
        };
        saw_eviction |= !evicted.is_empty();
    }
    assert!(saw_eviction, "the byte budget must evict once exceeded");
    assert!(
        orphan.len() < 21,
        "eviction must keep the room below the byte budget"
    );
}

/// An orphan spending several outputs of the same parent must be returned
/// only once per batch, not once per referenced output.
#[test]
fn test_orphan_find_by_parent_dedups_multiple_outputs_of_same_parent() {
    let parent = build_tx(vec![(&Byte32::zero(), 3)], 2);
    let child = build_tx(vec![(&parent.hash(), 0), (&parent.hash(), 1)], 1);
    let mut orphan = WaitingRoom::new();
    park(&mut orphan, child.clone());
    let txs = orphan.find_by_parent(&parent);
    assert_eq!(
        txs.len(),
        1,
        "one orphan per referenced parent, not per output"
    );
    assert!(txs.contains(&&child.proposal_short_id()));
}
