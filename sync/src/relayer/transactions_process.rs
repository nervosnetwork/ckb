use crate::Status;
use crate::relayer::Relayer;
use crate::types::SyncShared;
use ckb_logger::error;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{
    core::{Cycle, TransactionView},
    packed,
    prelude::*,
};
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_BAN_TIME: Duration = Duration::from_secs(3600 * 24 * 3);

pub(super) struct KnownRemoteBatch {
    shared: Arc<SyncShared>,
    hashes: Vec<packed::Byte32>,
    completed: usize,
}

impl KnownRemoteBatch {
    pub(super) fn new(shared: Arc<SyncShared>, hashes: Vec<packed::Byte32>) -> Self {
        Self {
            shared,
            hashes,
            completed: 0,
        }
    }

    pub(super) fn complete_prefix(&mut self, completed: usize) {
        self.completed = completed;
    }
}

impl Drop for KnownRemoteBatch {
    fn drop(&mut self) {
        for hash in self.hashes.iter().skip(self.completed) {
            self.shared.state().remove_from_known_txs(hash);
        }
    }
}

pub struct TransactionsProcess<'a> {
    message: packed::RelayTransactionsReader<'a>,
    relayer: &'a Relayer,
    nc: Arc<dyn CKBProtocolContext + Sync>,
    peer: PeerIndex,
}

impl<'a> TransactionsProcess<'a> {
    pub fn new(
        message: packed::RelayTransactionsReader<'a>,
        relayer: &'a Relayer,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer: PeerIndex,
    ) -> Self {
        TransactionsProcess {
            message,
            relayer,
            nc,
            peer,
        }
    }

    pub fn execute(self) -> Status {
        let shared_state = self.relayer.shared().state();
        let txs: Vec<(TransactionView, Cycle)> = {
            // ignore the tx if it's already known or it has never been requested before
            let mut tx_filter = shared_state.tx_filter();
            tx_filter.remove_expired();
            let unknown_tx_hashes = shared_state.unknown_tx_hashes();

            self.message
                .transactions()
                .iter()
                .map(|tx| (tx.transaction().to_entity().into_view(), tx.cycles().into()))
                .filter(|(tx, _)| {
                    !tx_filter.contains(&tx.hash())
                        && unknown_tx_hashes
                            .get_priority(&tx.hash())
                            .map(|priority| priority.requesting_peer() == Some(self.peer))
                            .unwrap_or_default()
                })
                .collect()
        };

        if txs.is_empty() {
            return Status::ok();
        }

        let max_block_cycles = self.relayer.shared().consensus().max_block_cycles();
        if txs
            .iter()
            .any(|(_, declared_cycles)| declared_cycles > &max_block_cycles)
        {
            self.nc.ban_peer(
                self.peer,
                DEFAULT_BAN_TIME,
                String::from("relay declared cycles greater than max_block_cycles"),
            );
            return Status::ok();
        }

        shared_state.mark_as_known_txs(txs.iter().map(|(tx, _)| tx.hash()));

        let tx_pool = self.relayer.shared.shared().tx_pool_controller().clone();
        let peer = self.peer;
        let mut known = KnownRemoteBatch::new(
            Arc::clone(self.relayer.shared()),
            txs.iter().map(|(tx, _)| tx.hash()).collect(),
        );
        let admission = match Arc::clone(&self.relayer.remote_batch_admission).try_acquire_owned() {
            Ok(admission) => admission,
            Err(_) => {
                error!("remote transaction batch admission is at capacity");
                return Status::ok();
            }
        };
        let response = match tx_pool.submit_remote_txs(txs, peer) {
            Ok(response) => response,
            Err(error) => {
                error!("submit remote transaction batch error {error}");
                return Status::ok();
            }
        };
        self.relayer
            .shared
            .shared()
            .async_handle()
            .spawn(async move {
                let _admission = admission;
                let (completed, error) = match response.await {
                    Ok(outcome) => {
                        let (_, completed, error) = outcome.into_parts();
                        (completed, error)
                    }
                    Err(error) => (0, Some(error)),
                };
                known.complete_prefix(completed);
                if let Some(error) = error {
                    error!("submit remote transaction batch error {error}");
                }
            });

        Status::ok()
    }
}
