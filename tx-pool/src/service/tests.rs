use super::*;
use ckb_async_runtime::new_background_runtime;
use ckb_error::AnyError;
use ckb_stop_handler::new_tokio_exit_rx;
use std::future::Future;
use std::time::Duration;

impl PipelineEpoch {
    pub(crate) fn set_for_test(&self, value: u64) {
        self.value.store(value, Ordering::Release);
        self.exhausted.store(false, Ordering::Release);
    }
}

impl BannedPeerSet {
    pub(crate) fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}

#[test]
fn banned_peer_fence_is_bounded_and_expires_entries() {
    let peers = BannedPeerSet::new(2);
    let first = ckb_network::PeerIndex::from(1);
    let second = ckb_network::PeerIndex::from(2);
    let third = ckb_network::PeerIndex::from(3);
    peers.record(first, std::time::Duration::from_secs(60));
    peers.record(second, std::time::Duration::from_secs(60));
    peers.record(third, std::time::Duration::from_secs(60));
    assert_eq!(peers.len(), 2);
    assert!(!peers.contains(first), "oldest transient marker is evicted");
    assert!(peers.contains(second));
    assert!(peers.contains(third));

    peers.record(second, std::time::Duration::ZERO);
    assert!(!peers.contains(second));
    assert_eq!(peers.len(), 1);
}

fn relay_state() -> RelayState {
    let (tx_relay_sender, _relay_rx) = ckb_channel::bounded(1);
    let (block_assembler_sender, _assembler_rx) = mpsc::channel(1);
    RelayState {
        network: Arc::new(crate::network::DummyTxPoolNetwork),
        tx_relay_sender,
        block_assembler_sender,
        block_assembler_dirty: Arc::new(Default::default()),
        block_assembler_reset: Arc::new(Default::default()),
        callbacks: Arc::new(Callbacks::new()),
        effects: Arc::new(crate::service::effects::EffectJournal::new(16, 1_000_000).unwrap()),
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
    relay.mark_block_assembler_dirty(&BlockAssemblerMessage::Uncle);

    let loaded = relay.load_block_assembler_dirty();
    assert_eq!(
        loaded
            .iter()
            .map(|(message, _)| message.clone())
            .collect::<Vec<_>>(),
        vec![
            BlockAssemblerMessage::Pending,
            BlockAssemblerMessage::Proposed,
            BlockAssemblerMessage::Uncle,
        ]
    );

    // Loading does not consume authority, and a producer racing with an old
    // completion installs a generation that the stale acknowledgement cannot
    // clear.
    relay.mark_block_assembler_dirty(&BlockAssemblerMessage::Pending);
    for (message, generation) in loaded {
        relay.complete_block_assembler_dirty(&message, generation);
    }
    let remaining = relay.load_block_assembler_dirty();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].0, BlockAssemblerMessage::Pending);
    relay.complete_block_assembler_dirty(&remaining[0].0, remaining[0].1);
    assert!(relay.load_block_assembler_dirty().is_empty());
}

#[test]
fn full_rebuild_reissues_both_optimistic_delta_generations() {
    let relay = relay_state();
    relay.mark_block_assembler_dirty(&BlockAssemblerMessage::Pending);
    let stale = relay
        .load_block_assembler_dirty()
        .into_iter()
        .find(|(message, _)| *message == BlockAssemblerMessage::Pending)
        .expect("pending generation exists");

    // Model a partial update that completed and acknowledged immediately
    // before the high-priority full writer swapped its older calculation.
    relay.complete_block_assembler_dirty(&stale.0, stale.1);
    assert!(relay.load_block_assembler_dirty().is_empty());

    relay.mark_block_assembler_full_reconcile();
    let messages = relay
        .load_block_assembler_dirty()
        .into_iter()
        .map(|(message, _)| message)
        .collect::<Vec<_>>();
    assert_eq!(
        messages,
        vec![
            BlockAssemblerMessage::Pending,
            BlockAssemblerMessage::Proposed
        ]
    );
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
