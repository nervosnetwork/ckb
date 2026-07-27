//! Cross-crate lifecycle support for synchronous tests.
//!
//! Production intentionally detaches the tx-pool dispatcher from
//! `TxPoolServiceBuilder::start`. A short-lived test process instead needs an
//! explicit owner so the effect journal and workers finish before its runtime
//! and database are destroyed.

use crate::service::{TxPoolServiceBuilder, TxVerificationResult};
use ckb_async_runtime::Handle;
use ckb_channel::Receiver;
use ckb_network::NetworkController;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const TEST_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Blocking-test owner for the tx-pool service and its required relay-result
/// consumer.
///
/// This type is available only through the `internal` feature. Dropping it on
/// a synchronous test thread performs the same bounded journal/worker
/// quiescence as production shutdown before joining the relay sink.
pub struct BlockingTxPoolTestScope {
    signal: CancellationToken,
    runtime: Handle,
    dispatcher: Option<JoinHandle<()>>,
    relay_sink: Option<std::thread::JoinHandle<()>>,
}

impl Drop for BlockingTxPoolTestScope {
    fn drop(&mut self) {
        self.signal.cancel();
        let Some(mut dispatcher) = self.dispatcher.take() else {
            return;
        };
        let clean = self.runtime.block_on(async {
            match tokio::time::timeout(TEST_SHUTDOWN_TIMEOUT, &mut dispatcher).await {
                Ok(Ok(())) => true,
                Ok(Err(_)) => false,
                Err(_) => {
                    dispatcher.abort();
                    false
                }
            }
        });
        if clean && let Some(relay_sink) = self.relay_sink.take() {
            let _ = relay_sink.join();
        }
    }
}

/// Start a tx-pool service for a synchronous cross-crate test.
///
/// The relay receiver is not optional: dropping it is a production-significant
/// endpoint failure that correctly closes the effect journal. Tests without a
/// relayer use the owned sink instead of weakening that failure policy.
pub fn start_blocking_test_service(
    builder: TxPoolServiceBuilder,
    network: NetworkController,
    relay_results: Receiver<TxVerificationResult>,
) -> std::io::Result<BlockingTxPoolTestScope> {
    let relay_sink = std::thread::Builder::new()
        .name("tx-pool-test-relay-sink".to_owned())
        .spawn(move || while relay_results.recv().is_ok() {})?;
    let signal = builder.signal_receiver.clone();
    let runtime = builder.handle.clone();
    let dispatcher = builder.start_with_handle(network);
    Ok(BlockingTxPoolTestScope {
        signal,
        runtime,
        dispatcher: Some(dispatcher),
        relay_sink: Some(relay_sink),
    })
}
