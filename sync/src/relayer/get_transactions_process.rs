use crate::relayer::{Relayer, MAX_RELAY_TXS_BYTES_PER_BATCH, MAX_RELAY_TXS_NUM_PER_BATCH};
use crate::utils::send_relaytransactions;
use crate::{attempt, Status, StatusCode};
use ckb_logger::{debug_target, trace_target};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{packed, prelude::*};
use std::sync::Arc;

pub struct GetTransactionsProcess<'a> {
    get_relay_transactions: packed::GetRelayTransactions,
    relayer: &'a Relayer,
    nc: Arc<dyn CKBProtocolContext>,
    peer: PeerIndex,
}

impl<'a> GetTransactionsProcess<'a> {
    pub fn new(
        message: packed::GetRelayTransactionsReader<'a>,
        relayer: &'a Relayer,
        nc: Arc<dyn CKBProtocolContext>,
        peer: PeerIndex,
    ) -> Self {
        let get_relay_transactions = message.to_entity();
        GetTransactionsProcess {
            get_relay_transactions,
            relayer,
            nc,
            peer,
        }
    }

    pub fn execute(self) -> Status {
        {
            fail::fail_point!("recv_getrelaytransactions", |_| {
                let length = self.get_relay_transactions.tx_hashes().len();
                ckb_logger::debug!(
                    "[failpoint] recv_getrelaytransactions(len={}) from {}",
                    length,
                    self.peer
                );
                Status::ignored()
            })
        }

        {
            let get_transactions = &self.get_relay_transactions;
            if get_transactions.tx_hashes().len() > MAX_RELAY_TXS_NUM_PER_BATCH {
                return StatusCode::ProtocolMessageIsMalformed.with_context(format!(
                    "TxHashes count({}) > MAX_RELAY_TXS_NUM_PER_BATCH({})",
                    get_transactions.tx_hashes().len(),
                    MAX_RELAY_TXS_NUM_PER_BATCH,
                ));
            }
        }

        let tx_hashes = self.get_relay_transactions.tx_hashes();

        trace_target!(
            crate::LOG_TARGET_RELAY,
            "{} request transactions({})",
            self.peer,
            tx_hashes
        );

        let transactions: Vec<_> = {
            let tx_pool = self.relayer.shared.shared().tx_pool_controller();

            let fetch_txs_with_cycles = tx_pool.fetch_txs_with_cycles(
                tx_hashes
                    .into_iter()
                    .map(|tx_hash| packed::ProposalShortId::from_tx_hash(&tx_hash))
                    .collect(),
            );

            if let Err(e) = fetch_txs_with_cycles {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "relayer tx_pool_controller send fetch_txs_with_cycles error: {:?}",
                    e,
                );
                return Status::ok();
            };

            fetch_txs_with_cycles
                .unwrap()
                .into_iter()
                .map(|(_, (tx, cycles))| {
                    packed::RelayTransaction::new_builder()
                        .cycles(cycles.pack())
                        .transaction(tx.data())
                        .build()
                })
                .collect()
        };

        if !transactions.is_empty() {
            let mut relay_bytes = 0;
            let mut relay_txs = Vec::new();
            for tx in transactions {
                if relay_bytes + tx.total_size() > MAX_RELAY_TXS_BYTES_PER_BATCH {
                    self.send_relay_transactions(relay_txs.drain(..).collect());
                    relay_bytes = tx.total_size();
                    relay_txs.push(tx);
                } else {
                    relay_bytes += tx.total_size();
                    relay_txs.push(tx);
                }
            }
            if !relay_txs.is_empty() {
                attempt!(self.send_relay_transactions(relay_txs));
            }
        }
        Status::ok()
    }

    fn send_relay_transactions(&self, txs: Vec<packed::RelayTransaction>) -> Status {
        if let Err(err) = send_relaytransactions(self.nc.as_ref(), self.peer, txs) {
            return StatusCode::Network
                .with_context(format!("send_relaytransactions error: {:?}", err));
        }
        Status::ok()
    }
}
