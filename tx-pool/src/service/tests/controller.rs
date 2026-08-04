use super::*;
use crate::service::{AsyncRequest, Notify, NotifyTxBatch, RemoteTxSubmission};
use ckb_async_runtime::new_background_runtime;
use ckb_error::AnyError;
use std::{
    future::Future,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

fn full_controller() -> TxPoolController {
    let (sender, _receiver) = mpsc::channel(1);
    let (reorg_sender, _reorg_receiver) = mpsc::channel(1);
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
        reorg_sender,
        chunk_tx: Arc::new(chunk_tx),
        handle: new_background_runtime(),
        started: Arc::new(AtomicBool::new(true)),
        signal: CancellationToken::new(),
    }
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
        let (reorg_sender, _reorg_receiver) = mpsc::channel(1);
        let (chunk_tx, _chunk_rx) = watch::channel(ChunkCommand::Resume);
        let controller = TxPoolController {
            sender,
            reorg_sender,
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
