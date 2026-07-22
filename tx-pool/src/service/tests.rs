use super::*;
use ckb_async_runtime::new_background_runtime;
use ckb_error::AnyError;
use std::future::Future;

fn relay_state() -> RelayState {
    let (tx_relay_sender, _relay_rx) = ckb_channel::bounded(1);
    let (block_assembler_sender, _assembler_rx) = mpsc::channel(1);
    RelayState {
        network: Arc::new(crate::network::DummyTxPoolNetwork),
        tx_relay_sender,
        block_assembler_sender,
        block_assembler_dirty: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        callbacks: Arc::new(Callbacks::new()),
        banned_peers: Default::default(),
    }
}

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
        signal: new_tokio_exit_rx(),
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

#[test]
fn block_assembler_dirty_journal_is_level_triggered_and_coalesced() {
    let relay = relay_state();
    relay.mark_block_assembler_dirty(&BlockAssemblerMessage::Pending);
    relay.mark_block_assembler_dirty(&BlockAssemblerMessage::Pending);
    relay.mark_block_assembler_dirty(&BlockAssemblerMessage::Proposed);

    assert_eq!(
        relay.take_block_assembler_dirty(),
        vec![
            BlockAssemblerMessage::Pending,
            BlockAssemblerMessage::Proposed
        ]
    );
    assert!(relay.take_block_assembler_dirty().is_empty());
}

#[test]
fn pipeline_epoch_exhaustion_is_fail_closed_without_wraparound() {
    let epoch = PipelineEpoch::default();
    epoch.set_for_test(u64::MAX - 1);
    assert_eq!(epoch.advance(), Some(u64::MAX));
    assert!(epoch.is_current(u64::MAX));

    assert_eq!(epoch.advance(), None);
    assert_eq!(epoch.current(), None);
    assert!(!epoch.is_current(u64::MAX));
    assert!(!epoch.is_current(0));
}
