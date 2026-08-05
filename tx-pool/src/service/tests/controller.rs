use super::*;
use crate::service::{AsyncRequest, ChainControl, Notify, NotifyTxBatch, RemoteTxSubmission};
use crate::test_support::genesis_snapshot;
use ckb_async_runtime::new_background_runtime;
use ckb_error::AnyError;
use std::{
    collections::{HashSet, VecDeque},
    future::Future,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

fn full_controller() -> TxPoolController {
    let (sender, _receiver) = mpsc::channel(1);
    let (chain_control_sender, _chain_control_receiver) = mpsc::channel(1);
    let (chunk_tx, _chunk_rx) = watch::channel(ChunkCommand::Resume);
    assert!(
        sender
            .try_send(Message::NotifyTxs(Notify::new(
                NotifyTxBatch::try_new(Vec::new()).expect("empty relay batch is valid"),
            )))
            .is_ok(),
        "fixture fills the bounded controller channel"
    );

    TxPoolController {
        sender,
        chain_control_sender,
        chunk_tx: Arc::new(chunk_tx),
        handle: new_background_runtime(),
        started: Arc::new(AtomicBool::new(true)),
        signal: CancellationToken::new(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authoritative_reorg_delivery_is_independent_of_rpc_readiness() {
    let (sender, _receiver) = mpsc::channel(1);
    let (chain_control_sender, mut chain_control_receiver) = mpsc::channel(1);
    let (chunk_tx, _chunk_rx) = watch::channel(ChunkCommand::Resume);
    let controller = TxPoolController {
        sender,
        chain_control_sender,
        chunk_tx: Arc::new(chunk_tx),
        handle: new_background_runtime(),
        started: Arc::new(AtomicBool::new(false)),
        signal: CancellationToken::new(),
    };
    let snapshot = genesis_snapshot();

    assert!(!controller.service_started());
    controller
        .update_tx_pool_for_reorg(
            VecDeque::new(),
            VecDeque::new(),
            HashSet::new(),
            Arc::clone(&snapshot),
        )
        .expect("the pre-start bounded channel retains the authoritative delta");

    let delivered = chain_control_receiver
        .try_recv()
        .expect("readiness cannot suppress an authoritative chain transition");
    let ChainControl::Reconcile(arguments) = delivered else {
        panic!("the ordered control must retain the exact chain transition");
    };
    assert!(arguments.0.is_empty());
    assert!(arguments.1.is_empty());
    assert!(arguments.2.is_empty());
    assert_eq!(arguments.3.tip_hash(), snapshot.tip_hash());
}

#[test]
fn closed_reorg_consumer_fails_without_waiting() {
    let (sender, _receiver) = mpsc::channel(1);
    let (chain_control_sender, chain_control_receiver) = mpsc::channel(1);
    let (chunk_tx, _chunk_rx) = watch::channel(ChunkCommand::Resume);
    drop(chain_control_receiver);
    let controller = TxPoolController {
        sender,
        chain_control_sender,
        chunk_tx: Arc::new(chunk_tx),
        handle: new_background_runtime(),
        started: Arc::new(AtomicBool::new(false)),
        signal: CancellationToken::new(),
    };

    let error = controller
        .update_tx_pool_for_reorg(
            VecDeque::new(),
            VecDeque::new(),
            HashSet::new(),
            genesis_snapshot(),
        )
        .expect_err("an explicitly disabled tx-pool has no chain consumer");
    assert!(error.to_string().contains("channel closed"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generation_clear_cannot_overtake_a_prior_chain_transition() {
    let (sender, _receiver) = mpsc::channel(1);
    let (chain_control_sender, mut chain_control_receiver) = mpsc::channel(1);
    let (chunk_tx, _chunk_rx) = watch::channel(ChunkCommand::Resume);
    let controller = TxPoolController {
        sender,
        chain_control_sender,
        chunk_tx: Arc::new(chunk_tx),
        handle: new_background_runtime(),
        started: Arc::new(AtomicBool::new(true)),
        signal: CancellationToken::new(),
    };
    let snapshot = genesis_snapshot();

    controller
        .update_tx_pool_for_reorg(
            VecDeque::new(),
            VecDeque::new(),
            HashSet::new(),
            Arc::clone(&snapshot),
        )
        .expect("the chain transition enters the ordered lane");

    let clear_controller = controller.clone();
    let clear_snapshot = Arc::clone(&snapshot);
    let clear = tokio::task::spawn_blocking(move || clear_controller.clear_pool(clear_snapshot));

    assert!(matches!(
        chain_control_receiver.recv().await,
        Some(ChainControl::Reconcile(_))
    ));
    let Some(ChainControl::ClearPool(Request { responder, .. })) =
        chain_control_receiver.recv().await
    else {
        panic!("clear_pool must follow the already-enqueued chain transition");
    };
    responder
        .send(())
        .expect("the synchronous clear caller retains its response");
    clear
        .await
        .expect("the clear caller task does not panic")
        .expect("the ordered clear receives its response");
}

async fn assert_fast_error<F, T>(future: F)
where
    F: Future<Output = Result<T, AnyError>>,
{
    let result = tokio::time::timeout(Duration::from_millis(100), future)
        .await
        .expect("a full controller channel must fail without waiting");
    assert!(result.is_err());
}

#[tokio::test]
async fn asynchronous_network_calls_fail_fast_when_the_controller_channel_is_full() {
    let controller = full_controller();

    assert_fast_error(controller.notify_txs_async(Vec::new())).await;
    assert_fast_error(controller.fresh_proposals_filter(Vec::new())).await;
    assert_fast_error(controller.fetch_txs(HashSet::new())).await;
    assert_fast_error(controller.fetch_txs_with_cycles(HashSet::new())).await;
}

#[test]
fn remote_submit_waits_without_blocking_a_current_thread_runtime() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime builds");
    runtime.block_on(async {
        let (sender, mut receiver) = mpsc::channel(1);
        let (chain_control_sender, _chain_control_receiver) = mpsc::channel(1);
        let (chunk_tx, _chunk_rx) = watch::channel(ChunkCommand::Resume);
        let controller = TxPoolController {
            sender,
            chain_control_sender,
            chunk_tx: Arc::new(chunk_tx),
            handle: new_background_runtime(),
            started: Arc::new(AtomicBool::new(true)),
            signal: CancellationToken::new(),
        };
        let transaction = ckb_types::core::TransactionBuilder::default().build();
        let expected = transaction.clone();
        let responder = tokio::spawn(async move {
            let Some(Message::SubmitRemoteTx(AsyncRequest {
                responder,
                arguments,
            })) = receiver.recv().await
            else {
                panic!("remote submission message missing");
            };
            let RemoteTxSubmission { transaction, .. } = arguments;
            assert_eq!(transaction, expected);
            responder.send(()).expect("test receiver remains present");
        });

        controller
            .submit_remote_tx(transaction, 0, ckb_network::PeerIndex::from(1))
            .await
            .expect("remote submission receives its response");
        responder.await.expect("responder task does not panic");
    });
}
