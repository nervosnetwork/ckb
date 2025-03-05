use crate::relayer::{MAX_RELAY_TXS_BYTES_PER_BATCH, MAX_RELAY_TXS_NUM_PER_BATCH, Relayer};
use crate::utils::send_message_to;
use crate::{Status, StatusCode, attempt};
use ckb_logger::{debug_target, trace_target};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{packed, prelude::*};
use std::collections::HashSet;
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

    pub fn execute(self) -> Status {
        let message_len = self.message.tx_hashes().len();
        {
            if message_len > MAX_RELAY_TXS_NUM_PER_BATCH {
                return StatusCode::ProtocolMessageIsMalformed.with_context(format!(
                    "TxHashes count({message_len}) > MAX_RELAY_TXS_NUM_PER_BATCH({MAX_RELAY_TXS_NUM_PER_BATCH})",
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

            let tx_hashes_set: HashSet<_> = tx_hashes
                .iter()
                .map(|tx_hash| packed::ProposalShortId::from_tx_hash(&tx_hash.to_entity()))
                .collect();

            if message_len != tx_hashes_set.len() {
                return StatusCode::RequestDuplicate.with_context("Request duplicate transaction");
            }

            let fetch_txs_with_cycles = tx_pool.fetch_txs_with_cycles(tx_hashes_set);

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
                    self.send_relay_transactions(std::mem::take(&mut relay_txs));
                    relay_bytes = tx.total_size();
                } else {
                    relay_bytes += tx.total_size();
                }
                relay_txs.push(tx);
            }
            if !relay_txs.is_empty() {
                attempt!(self.send_relay_transactions(relay_txs));
            }
        }
        Status::ok()
    }

    fn send_relay_transactions(&self, txs: Vec<packed::RelayTransaction>) -> Status {
        let message = packed::RelayMessage::new_builder()
            .set(
                packed::RelayTransactions::new_builder()
                    .transactions(packed::RelayTransactionVec::new_builder().set(txs).build())
                    .build(),
            )
            .build();
        send_message_to(self.nc.as_ref(), self.peer, &message)
    }
}
