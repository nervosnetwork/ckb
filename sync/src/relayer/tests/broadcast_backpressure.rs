use super::helper::{MockProtocolContext, build_chain, gen_block};
use crate::relayer::schedule_build_and_broadcast_compact_block;
use ckb_network::{CKBProtocolContext, SupportProtocols};
use std::{
    sync::{Arc, mpsc},
    time::Duration,
};

#[test]
fn compact_block_broadcast_backpressure_does_not_block_caller() {
    let (_chain, relayer, _) = build_chain(1);
    let parent = relayer.shared.active_chain().tip_header();
    let block = Arc::new(gen_block(&parent, relayer.shared().shared(), 0, 0, None));

    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::channel();
    let context: Arc<dyn CKBProtocolContext + Sync> =
        Arc::new(MockProtocolContext::with_blocked_broadcast(
            SupportProtocols::RelayV3,
            entered_tx,
            release_rx,
        ));
    let shared = relayer.shared().shared().clone();
    let (done_tx, done_rx) = mpsc::channel();

    let worker = std::thread::spawn(move || {
        schedule_build_and_broadcast_compact_block(context, shared, 1.into(), block);
        done_tx.send(()).unwrap();
    });

    done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("verification callback remained blocked by P2P backpressure");
    entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("broadcast future was not polled");

    release_tx.send(()).unwrap();
    worker.join().unwrap();
}
