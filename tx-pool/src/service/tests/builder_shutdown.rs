use super::{BackgroundWorkerHandles, MessageHandlerGuard, service_cancellation_token};
use crate::network::DummyTxPoolNetwork;
use crate::service::TxVerificationResult;
use crate::service::effects::{EffectEndpoints, EffectJournal, run_effect_publisher};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
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
        verify: vec![finished()],
        resolver: finished(),
        block_assembler: Some(finished()),
        reorg: Some(finished()),
    }
}

#[test]
fn unwinding_message_handler_guard_requests_fail_stop() {
    let shutdown = CancellationToken::new();
    let failed = Arc::new(AtomicBool::new(false));
    drop(MessageHandlerGuard::new(
        shutdown.clone(),
        Arc::clone(&failed),
    ));
    assert!(shutdown.is_cancelled());
    assert!(failed.load(Ordering::Acquire));
}

#[test]
fn completed_message_handler_guard_does_not_cancel_service() {
    let shutdown = CancellationToken::new();
    let failed = Arc::new(AtomicBool::new(false));
    let mut guard = MessageHandlerGuard::new(shutdown.clone(), Arc::clone(&failed));
    guard.complete();
    drop(guard);
    assert!(!shutdown.is_cancelled());
    assert!(!failed.load(Ordering::Acquire));
}

#[test]
fn service_cancellation_is_scoped_under_process_exit() {
    let process_exit = CancellationToken::new();
    let first = service_cancellation_token(&process_exit);
    let second = service_cancellation_token(&process_exit);

    first.cancel();
    assert!(first.is_cancelled());
    assert!(!second.is_cancelled());
    assert!(!process_exit.is_cancelled());

    process_exit.cancel();
    assert!(second.is_cancelled());
}

#[tokio::test]
async fn panicked_state_worker_makes_shutdown_ineligible_for_persistence() {
    let queue = Arc::new(EffectJournal::new(8, 1_000_000).unwrap());
    let (relay_tx, _relay_rx) = ckb_channel::bounded::<TxVerificationResult>(8);
    let publisher = tokio::spawn(run_effect_publisher(
        Arc::clone(&queue),
        EffectEndpoints {
            network: Arc::new(DummyTxPoolNetwork),
            tx_relay_sender: relay_tx,
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
    let queue = Arc::new(EffectJournal::new(8, 1_000_000).unwrap());
    let publisher = tokio::spawn(async { panic!("injected effect-publisher failure") });

    assert!(
        !handles(publisher, finished())
            .quiesce(Duration::from_secs(1), &queue)
            .await
    );
}
