//! Regression test for the bounded in-flight retry of local orphans.
//!
//! A local orphan whose missing parents are all in flight is re-enqueued
//! with a short delay instead of being rejected. Previously that branch
//! never incremented the attempt counter, so a submitter keeping parents
//! in the pipeline indefinitely could make the ordered resolver retry the
//! orphan forever. The branch is now bounded by
//! `MAX_LOCAL_ORPHAN_IN_FLIGHT_ATTEMPTS` (30 in tests, 2400 in production).
//!
//! The normal path (an orphan resolving once its parent lands) is covered
//! by `pipeline_preserves_order_for_dependent_txs` in `tests/pipeline.rs`.

use crate::{
    component::{pipeline_queue::PipelineQueue, pool_map::Status},
    resolve_mgr::{MAX_LOCAL_ORPHAN_IN_FLIGHT_ATTEMPTS, OrderedResolver},
    resolved_tx::ResolvedTx,
    service::TxPoolService,
    tx_source::TxSource,
};
use ckb_script::ChunkCommand;
use ckb_stop_handler::CancellationToken;
use ckb_types::{
    core::{Capacity, TransactionView, cell::ResolvedTransaction},
    packed::Byte32,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

use super::harness::{WorkerSet, harness};
use super::util::build_tx;

fn service() -> TxPoolService {
    harness(2).workers(WorkerSet::None).build().service
}

/// Park `parent` in the verify queue forever (no verify manager runs in
/// this harness) so it never leaves the "in flight" state.
async fn park_parent_in_verify_queue(service: &TxPoolService, parent: &TransactionView) {
    let snapshot = service.tx_pool.read().await.cloned_snapshot();
    let resolved = ResolvedTx {
        tx: parent.clone(),
        rtx: Arc::new(ResolvedTransaction::dummy_resolve(parent.clone())),
        status: Status::Pending,
        fee: Capacity::zero(),
        tx_size: parent.data().serialized_size_in_block(),
        pre_resolve_tip: Default::default(),
        snapshot,
        source: TxSource::Local,
    };
    let mut verify = service.queues.verify_queue.write().await;
    verify.add_tx(resolved).unwrap();
}

fn start_ordered_resolver(service: &TxPoolService) -> CancellationToken {
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
    signal
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_orphan_with_stuck_parent_is_eventually_rejected() {
    let service = service();
    let parent = build_tx(vec![(&Byte32::zero(), 0)], 1);
    let child = build_tx(vec![(&parent.hash(), 0)], 1);
    let child_id = child.proposal_short_id();

    park_parent_in_verify_queue(&service, &parent).await;
    service
        .classify_and_enqueue_tx(child.clone(), TxSource::Local)
        .await
        .unwrap();
    {
        let ordered = service.queues.ordered_resolve_queue.read().await;
        assert!(ordered.contains_key(&child_id));
    }

    let _signal = start_ordered_resolver(&service);

    // The retry budget is 30 attempts at 50ms in tests (~1.5s). With the
    // delay-heap retry model the job stays *visible* in the queue during
    // every delay window, so "not in queue" now unambiguously means
    // "rejected" — no need to distinguish retry windows from rejection.
    let mut rejected = false;
    for _ in 0..200 {
        let gone = {
            let ordered = service.queues.ordered_resolve_queue.read().await;
            !ordered.contains_key(&child_id)
        };
        if gone {
            rejected = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        rejected,
        "orphan with a permanently in-flight parent must be rejected after \
         MAX_LOCAL_ORPHAN_IN_FLIGHT_ATTEMPTS ({MAX_LOCAL_ORPHAN_IN_FLIGHT_ATTEMPTS}) retries"
    );
    assert!(
        service
            .tx_pool
            .read()
            .await
            .pool_map
            .get_by_id(&child_id)
            .is_none(),
        "rejected orphan must not enter the pool"
    );
}

/// A job parked in the queue's delayed section (waiting for its in-flight
/// parent) must stay visible to administrative removal the whole time —
/// the previous spawn-sleep-re-enqueue model hid it inside a detached task
/// for the duration of the delay.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delayed_orphan_is_removable_by_hash() {
    let service = service();
    let parent = build_tx(vec![(&Byte32::zero(), 0)], 1);
    let child = build_tx(vec![(&parent.hash(), 0)], 1);
    let child_id = child.proposal_short_id();

    park_parent_in_verify_queue(&service, &parent).await;
    service
        .classify_and_enqueue_tx(child.clone(), TxSource::Local)
        .await
        .unwrap();

    let _signal = start_ordered_resolver(&service);

    // Wait for the child to complete at least one delayed re-enqueue cycle:
    // it must be back in the queue (visible) before we try to remove it.
    tokio::time::sleep(Duration::from_millis(120)).await;
    {
        let ordered = service.queues.ordered_resolve_queue.read().await;
        assert!(
            ordered.contains_key(&child_id),
            "delayed orphan must stay visible in the queue"
        );
    }

    assert!(service.remove_tx(child.hash()).await);
    let ordered = service.queues.ordered_resolve_queue.read().await;
    assert!(!ordered.contains_key(&child_id));
}
