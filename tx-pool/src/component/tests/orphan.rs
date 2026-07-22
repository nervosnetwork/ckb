use crate::component::tests::util::build_tx;
use crate::component::waiting_room::{WaitReason, WaitingRoom};
use crate::tx_source::TxSource;
use ckb_types::core::TransactionView;
use ckb_types::packed::{Byte32, OutPoint};
use ckb_types::prelude::*;

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

/// A remote child whose parent is merely *in flight* must not be rejected
/// and must not sit in the ordered queue forever: it is routed to the
/// ordered resolve queue first (dependent on an in-flight output), and
/// when the resolver still cannot resolve it, it parks in the orphan pool
/// (event-driven recovery — the delayed bounded retry is a local-only
/// path). Once the parent is accepted, the orphan is re-classified and
/// committed. This locks down the boundary between the ordered resolve
/// queue and the orphan pool for remote transactions.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_orphan_parks_and_recovers_after_parent_lands() {
    use crate::component::pipeline_queue::PipelineQueue;
    use crate::component::pool_map::Status;
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::component::tests::util::MOCK_CYCLES;
    use crate::resolve_mgr::OrderedResolver;
    use crate::resolved_tx::ResolvedTx;
    use ckb_script::ChunkCommand;
    use ckb_stop_handler::CancellationToken;
    use ckb_types::core::{Capacity, cell::ResolvedTransaction};
    use std::sync::Arc;
    use tokio::sync::watch;

    let h = harness(1).workers(WorkerSet::None).build();
    let service = h.service;
    let funding = h.out_points[0].clone();

    let parent = build_resolvable_tx(&funding, 4_000);
    let child = build_resolvable_tx(&OutPoint::new(parent.hash(), 0), 3_000);
    let parent_id = parent.proposal_short_id();
    let child_id = child.proposal_short_id();

    // The parent is in flight: resolved and parked in the verify queue,
    // not yet in the pool.
    let snapshot = service.pool.tx_pool.read().await.cloned_snapshot();
    let parent_resolved = ResolvedTx {
        tx: parent.clone(),
        rtx: Arc::new(ResolvedTransaction::dummy_resolve(parent.clone())),
        status: Status::Pending,
        fee: Capacity::zero(),
        tx_size: parent.data().serialized_size_in_block(),
        pre_resolve_tip: Default::default(),
        snapshot,
        source: TxSource::Local,
    };
    {
        let mut verify = service.pipeline.queues.verify_queue.write().await;
        verify.add_tx(parent_resolved.clone()).unwrap();
    }

    // Submit the remote child: it depends on the in-flight parent, so it
    // routes to the ordered resolve queue — not the orphan pool.
    service
        .submit_remote_tx(
            child.clone(),
            TxSource::Remote {
                cycles: 0,
                peer: 1.into(),
            },
        )
        .await
        .unwrap();
    {
        let ordered = service.pipeline.queues.ordered_resolve_queue.read().await;
        assert!(
            ordered.contains_key(&child_id),
            "a dependent remote child must route to the ordered resolve queue"
        );
        let room = service.pipeline.waiting_room.read().await;
        assert!(
            room.get(&child_id).is_none(),
            "must not enter the orphan pool while the parent is merely in flight"
        );
    }

    // The ordered resolver pops the child while the parent is still not
    // committed: a remote orphan parks in the orphan pool (event-driven,
    // no delayed retry — that path is local-only).
    let signal = CancellationToken::new();
    let (chunk_tx, _chunk_rx) = watch::channel(ChunkCommand::Resume);
    let resolver =
        OrderedResolver::new(service.clone(), chunk_tx.subscribe(), signal.child_token());
    let (exit_tx, mut exit_rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = resolver.start(exit_tx);
    tokio::spawn(async move {
        if let Some((_, crate::resolve_mgr::ResolveExit::Panicked { message })) =
            exit_rx.recv().await
        {
            panic!("tx-pool ordered resolver panicked: {message}");
        }
        let _ = handle.await;
    });

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let ordered = service.pipeline.queues.ordered_resolve_queue.read().await;
            if !ordered.contains_key(&child_id) {
                break;
            }
            drop(ordered);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the resolver must pop the child");

    {
        let room = service.pipeline.waiting_room.read().await;
        let entry = room
            .get(&child_id)
            .expect("the remote child must park in the orphan pool");
        assert!(
            matches!(entry.reason, WaitReason::ParentsMissing { .. }),
            "remote child must park as ParentsMissing, got {:?}",
            entry.reason
        );
    }

    // The parent is accepted (driven through the verify queue and submit).
    {
        let mut verify = service.pipeline.queues.verify_queue.write().await;
        assert_eq!(
            verify.pop_front(false).map(|r| r.tx.proposal_short_id()),
            Some(parent_id.clone())
        );
    }
    service
        .submit_entry(parent_resolved, MOCK_CYCLES)
        .await
        .unwrap();
    {
        let pool = service.pool.tx_pool.read().await;
        assert!(pool.contains_proposal_id(&parent_id));
    }

    // The parent landing wakes the orphan: it is re-classified into the
    // verify queue and can be committed.
    service.process_orphan_tx(&parent).await;
    let child_resolved = {
        let mut verify = service.pipeline.queues.verify_queue.write().await;
        verify
            .pop_front(false)
            .expect("the recovered child must enter the verify queue")
    };
    service
        .submit_entry(child_resolved, MOCK_CYCLES)
        .await
        .unwrap();

    let pool = service.pool.tx_pool.read().await;
    assert!(
        pool.contains_proposal_id(&child_id),
        "the recovered remote child must be committed"
    );
    let room = service.pipeline.waiting_room.read().await;
    assert!(
        room.get(&child_id).is_none(),
        "the orphan entry must be gone after recovery"
    );
}

/// Build a transaction that passes full contextual verification on the
/// harness chain: it spends a real cell secured by the always-success
/// script and carries the always-success lock on its own output, so its
/// children can spend it too.
fn build_resolvable_tx(
    input: &ckb_types::packed::OutPoint,
    output_capacity: usize,
) -> TransactionView {
    let (_, _, always_success_script) = ckb_test_chain_utils::always_success_cell();
    ckb_types::core::TransactionBuilder::default()
        .cell_dep(
            ckb_types::packed::CellDep::new_builder()
                .out_point(ckb_test_chain_utils::create_always_success_out_point())
                .build(),
        )
        .input(ckb_types::packed::CellInput::new(input.clone(), 0))
        .output(
            ckb_types::packed::CellOutput::new_builder()
                .capacity(ckb_types::core::Capacity::bytes(output_capacity).unwrap())
                .lock(always_success_script.clone())
                .build(),
        )
        .output_data(ckb_types::bytes::Bytes::default().pack())
        .build()
}
