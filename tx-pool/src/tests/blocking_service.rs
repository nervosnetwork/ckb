//! Cross-crate lifecycle support for synchronous tests.
//!
//! Production intentionally detaches the tx-pool dispatcher from
//! `TxPoolServiceBuilder::start`. A short-lived test process instead needs an
//! explicit owner so the effect journal and workers finish before its runtime
//! and database are destroyed.

use crate::service::{TxPoolServiceBuilder, TxVerificationResultReceiver};
use ckb_async_runtime::Handle;
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
    relay_results: Option<TxVerificationResultReceiver>,
}

impl BlockingTxPoolTestScope {
    /// Move the sole relay-result capability to an external test consumer.
    ///
    /// The returned receiver must remain alive until this scope has quiesced
    /// the dispatcher. Cross-crate fixtures retain it in the same aggregate
    /// owner, so endpoint lifetime and database lifetime remain ordered.
    pub fn take_relay_results(&mut self) -> Option<TxVerificationResultReceiver> {
        self.relay_results.take()
    }
}

impl Drop for BlockingTxPoolTestScope {
    fn drop(&mut self) {
        self.signal.cancel();
        let Some(mut dispatcher) = self.dispatcher.take() else {
            return;
        };
        self.runtime.block_on(async {
            if tokio::time::timeout(TEST_SHUTDOWN_TIMEOUT, &mut dispatcher)
                .await
                .is_err()
            {
                dispatcher.abort();
            }
        });
    }
}

/// Start a tx-pool service for a synchronous cross-crate test.
///
/// The relay receiver is not optional: dropping it is a production-significant
/// endpoint failure. The current relay mailbox is nonblocking and reconciles
/// overflow to a bounded reset, so a test which does not inspect relay outcomes
/// needs only retain the sole receiver for the service lifetime; no polling or
/// forwarding thread is necessary.
pub fn start_blocking_test_service(
    builder: TxPoolServiceBuilder,
    network: NetworkController,
    relay_results: TxVerificationResultReceiver,
) -> BlockingTxPoolTestScope {
    let signal = builder.signal_receiver.clone();
    let runtime = builder.handle.clone();
    let dispatcher = builder.start_with_handle(network);
    BlockingTxPoolTestScope {
        signal,
        runtime,
        dispatcher: Some(dispatcher),
        relay_results: Some(relay_results),
    }
}
