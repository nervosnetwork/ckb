use crate::relayer::Relayer;
use crate::Status;
use ckb_logger::error;
use ckb_network::PeerIndex;
use ckb_types::{
    core::{Cycle, TransactionView},
    packed,
    prelude::*,
};

pub struct TransactionsProcess<'a> {
    message: packed::RelayTransactionsReader<'a>,
    relayer: &'a Relayer,
    peer: PeerIndex,
}

impl<'a> TransactionsProcess<'a> {
    pub fn new(
        message: packed::RelayTransactionsReader<'a>,
        relayer: &'a Relayer,
        peer: PeerIndex,
    ) -> Self {
        TransactionsProcess {
            message,
            relayer,
            peer,
        }
    }

    pub fn execute(self) -> Status {
        let shared_state = self.relayer.shared().state();
        let txs: Vec<(TransactionView, Cycle)> = {
            let tx_filter = shared_state.tx_filter();

            self.message
                .transactions()
                .iter()
                .map(|tx| {
                    (
                        tx.transaction().to_entity().into_view(),
                        tx.cycles().unpack(),
                    )
                })
                .filter(|(tx, _)| !tx_filter.contains(&tx.hash()))
                .collect()
        };

        if txs.is_empty() {
            return Status::ok();
        }

        // Insert tx_hash into `already_known`
        // Remove tx_hash from `inflight_transactions`
        {
            shared_state.mark_as_known_txs(txs.iter().map(|(tx, _)| tx.hash()));
        }

        // Remove tx_hash from `tx_ask_for_set`
        {
            if let Some(mut peer_state) = shared_state.peers().state.get_mut(&self.peer) {
                for (tx, _) in txs.iter() {
                    peer_state.remove_ask_for_tx(&tx.hash());
                }
            }
        }

        let tx_pool = self.relayer.shared.shared().tx_pool_controller().clone();
        let peer = self.peer;
        self.relayer
            .shared
            .shared()
            .async_handle()
            .spawn(async move {
                for (tx, declared_cycle) in txs {
                    if let Err(e) = tx_pool
                        .submit_remote_tx(tx.clone(), declared_cycle, peer)
                        .await
                    {
                        error!("submit_tx error {}", e);
                    }
                }
            });

        Status::ok()
    }
}
