//! An overlay to index the pending txs in the ckb tx pool

use ckb_async_runtime::{
    tokio::{self, task::JoinHandle},
    Handle,
};
use ckb_logger::info;
use ckb_notify::NotifyController;
use ckb_stop_handler::{new_tokio_exit_rx, CancellationToken};
use ckb_types::{core::TransactionView, packed::OutPoint};

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

const SUBSCRIBER_NAME: &str = "Indexer";

/// An overlay to index the pending txs in the ckb tx pool,
/// currently only supports removals of dead cells from the pending txs
#[derive(Default)]
pub struct Pool {
    dead_cells: HashSet<OutPoint>,
}

impl Pool {
    /// the tx has been committed in a block, it should be removed from pending dead cells
    pub fn transaction_committed(&mut self, tx: &TransactionView) {
        for input in tx.inputs() {
            self.dead_cells.remove(&input.previous_output());
        }
    }

    /// the tx has been rejected for some reason, it should be removed from pending dead cells
    pub fn transaction_rejected(&mut self, tx: &TransactionView) {
        for input in tx.inputs() {
            self.dead_cells.remove(&input.previous_output());
        }
    }

    /// a new tx is submitted to the pool, mark its inputs as dead cells
    pub fn new_transaction(&mut self, tx: &TransactionView) {
        for input in tx.inputs() {
            self.dead_cells.insert(input.previous_output());
        }
    }

    /// Return wether out_point referred cell consumed by pooled transaction
    pub fn is_consumed_by_pool_tx(&self, out_point: &OutPoint) -> bool {
        self.dead_cells.contains(out_point)
    }

    /// the txs has been committed in a block, it should be removed from pending dead cells
    pub fn transactions_committed(&mut self, txs: &[TransactionView]) {
        for tx in txs {
            self.transaction_committed(tx);
        }
    }

    /// return all dead cells
    pub fn dead_cells(&self) -> impl Iterator<Item = &OutPoint> {
        self.dead_cells.iter()
    }
}

/// Pool service
#[derive(Clone)]
pub struct PoolService {
    pool: Option<Arc<RwLock<Pool>>>,
    async_handle: Handle,
    is_index_tx_pool_called: bool,
}

impl PoolService {
    /// Construct new Pool service instance
    pub fn new(index_tx_pool: bool, async_handle: Handle) -> Self {
        let pool = if index_tx_pool {
            Some(Arc::new(RwLock::new(Pool::default())))
        } else {
            None
        };

        Self {
            pool,
            async_handle,
            is_index_tx_pool_called: false,
        }
    }

    /// Get the inner pool
    pub fn pool(&self) -> Option<Arc<RwLock<Pool>>> {
        self.pool.clone()
    }

    /// Processes that handle index pool transaction and expect to be spawned to run in tokio runtime
    pub fn index_tx_pool(
        &mut self,
        notify_controller: NotifyController,
        check_index_tx_pool_ready: JoinHandle<()>,
    ) {
        if self.is_index_tx_pool_called {
            return;
        }
        self.is_index_tx_pool_called = true;

        let service = self.clone();
        let stop: CancellationToken = new_tokio_exit_rx();

        self.async_handle.spawn(async move {
            let _check_index_tx_pool_ready = check_index_tx_pool_ready.await;
            if stop.is_cancelled() {
                info!("Indexer received exit signal, cancel subscribe_new_transaction task, exit now");
                return;
            }

            info!("check_index_tx_pool_ready finished");

            let mut new_transaction_receiver = notify_controller
                .subscribe_new_transaction(SUBSCRIBER_NAME.to_string())
                .await;
            let mut reject_transaction_receiver = notify_controller
                .subscribe_reject_transaction(SUBSCRIBER_NAME.to_string())
                .await;

            loop {
                tokio::select! {
                    Some(tx_entry) = new_transaction_receiver.recv() => {
                        if let Some(pool) = service.pool.as_ref() {
                            pool.write().expect("acquire lock").new_transaction(&tx_entry.transaction);
                        }
                    }
                    Some((tx_entry, _reject)) = reject_transaction_receiver.recv() => {
                        if let Some(pool) = service.pool.as_ref() {
                            pool.write()
                            .expect("acquire lock")
                            .transaction_rejected(&tx_entry.transaction);
                        }
                    }
                    _ = stop.cancelled() => {
                        info!("index_tx_pool received exit signal, exit now");
                        break
                    },
                    else => break,
                }
            }
        });
    }
}
