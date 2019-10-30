use crate::relayer::{Relayer, MAX_RELAY_TXS_BYTES_PER_BATCH, MAX_RELAY_TXS_NUM_PER_BATCH};
use ckb_logger::{debug_target, trace_target, warn};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{packed, prelude::*};
use failure::{err_msg, Error as FailureError};
use std::sync::Arc;

pub struct GetTransactionsProcess<'a> {
    message: packed::GetRelayTransactionsReader<'a>,
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
        GetTransactionsProcess {
            message,
            relayer,
            nc,
            peer,
        }
    }

    pub fn execute(self) -> Result<(), FailureError> {
        {
            let get_transactions = self.message;
            if get_transactions.tx_hashes().len() > MAX_RELAY_TXS_NUM_PER_BATCH {
                warn!("Peer {} sends us an invalid message, GetTransactions tx_hashes size ({}) is greater than MAX_RELAY_TXS_NUM_PER_BATCH ({})",
                    self.peer, get_transactions.tx_hashes().len(), MAX_RELAY_TXS_NUM_PER_BATCH);
                return Err(err_msg(
                    "GetTransactions tx_hashes size is greater than MAX_RELAY_TXS_NUM_PER_BATCH"
                        .to_owned(),
                ));
            }
        }

        let tx_hashes = self.message.tx_hashes();

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
                    .iter()
                    .map(|tx_hash| packed::ProposalShortId::from_tx_hash(&tx_hash.to_entity()))
                    .collect(),
            );

            if let Err(e) = fetch_txs_with_cycles {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "relayer tx_pool_controller send fetch_txs_with_cycles error: {:?}",
                    e,
                );
                return Ok(());
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
                self.send_relay_transactions(relay_txs);
            }
        }
        Ok(())
    }

    fn send_relay_transactions(&self, txs: Vec<packed::RelayTransaction>) {
        let message = packed::RelayMessage::new_builder()
            .set(
                packed::RelayTransactions::new_builder()
                    .transactions(packed::RelayTransactionVec::new_builder().set(txs).build())
                    .build(),
            )
            .build();
        let data = message.as_slice().into();
        if let Err(err) = self.nc.send_message_to(self.peer, data) {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "relayer send Transactions error: {:?}",
                err,
            );
        }
    }
}
