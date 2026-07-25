use super::BackgroundWorkerHandles;
use crate::network::DummyTxPoolNetwork;
use crate::service::TxVerificationResult;
use crate::service::effects::{EffectEndpoints, EffectQueue, run_effect_publisher};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn finished() -> tokio::task::JoinHandle<()> {
    tokio::spawn(async {})
}

fn handles(
    effects: tokio::task::JoinHandle<()>,
    verify_cache: tokio::task::JoinHandle<()>,
) -> BackgroundWorkerHandles {
    BackgroundWorkerHandles {
        effects,
        verify_cache,
        maintenance: finished(),
        commit: finished(),
        pre_check: vec![finished()],
        verify_mgr: finished(),
        resolver: finished(),
        block_assembler: Some(finished()),
        reorg: Some(finished()),
    }
}

#[tokio::test]
async fn panicked_state_worker_makes_shutdown_ineligible_for_persistence() {
    let queue = Arc::new(EffectQueue::new(8, 1_000_000).unwrap());
    let (relay_tx, _relay_rx) = ckb_channel::bounded::<TxVerificationResult>(8);
    let publisher = tokio::spawn(run_effect_publisher(
        Arc::clone(&queue),
        EffectEndpoints {
            network: Arc::new(DummyTxPoolNetwork),
            tx_relay_sender: relay_tx,
            failure_cancel: CancellationToken::new(),
        },
    ));
    let panicked = tokio::spawn(async { panic!("injected state-worker failure") });

    assert!(
        !handles(publisher, panicked)
            .quiesce(Duration::from_secs(1), &queue)
            .await
    );
}

#[tokio::test]
async fn panicked_effect_publisher_makes_shutdown_ineligible_for_persistence() {
    let queue = Arc::new(EffectQueue::new(8, 1_000_000).unwrap());
    let publisher = tokio::spawn(async { panic!("injected effect-publisher failure") });

    assert!(
        !handles(publisher, finished())
            .quiesce(Duration::from_secs(1), &queue)
            .await
    );
}
