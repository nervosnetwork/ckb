use super::*;
use ckb_async_runtime::new_background_runtime;
use std::future::Future;

fn full_controller() -> TxPoolController {
    let (sender, _receiver) = mpsc::channel(1);
    let (reorg_sender, _reorg_receiver) = mpsc::channel(1);
    let (chunk_tx, _chunk_rx) = watch::channel(ChunkCommand::Resume);

    assert!(
        sender
            .try_send(Message::NotifyTxs(Notify::new(Vec::new())))
            .is_ok()
    );

    TxPoolController {
        sender,
        reorg_sender,
        chunk_tx: Arc::new(chunk_tx),
        handle: new_background_runtime(),
        started: Arc::new(AtomicBool::new(true)),
    }
}

async fn assert_fast_error<F, T>(future: F)
where
    F: Future<Output = Result<T, AnyError>>,
{
    let result = tokio::time::timeout(Duration::from_millis(100), future)
        .await
        .expect("tx-pool controller call should not wait for channel capacity");
    assert!(result.is_err());
}

#[tokio::test]
async fn async_network_controller_calls_fail_fast_when_channel_is_full() {
    let controller = full_controller();

    assert_fast_error(controller.notify_txs_async(Vec::new())).await;
    assert_fast_error(controller.fresh_proposals_filter(Vec::new())).await;
    assert_fast_error(controller.fetch_txs(HashSet::new())).await;
    assert_fast_error(controller.fetch_txs_with_cycles(HashSet::new())).await;
}
