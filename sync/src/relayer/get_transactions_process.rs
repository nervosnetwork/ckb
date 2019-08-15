use crate::relayer::Relayer;
use ckb_logger::{debug_target, trace_target};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{packed, prelude::*};
use failure::Error as FailureError;
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
        let tx_hashes = self.message.tx_hashes();

        trace_target!(
            crate::LOG_TARGET_RELAY,
            "{} request transactions({})",
            self.peer,
            tx_hashes
        );

        let transactions: Vec<_> = {
            let state = self.relayer.shared.lock_chain_state();

            tx_hashes
                .iter()
                .filter_map(|tx_hash| {
                    let entry_opt = {
                        let short_id =
                            packed::ProposalShortId::from_tx_hash(&tx_hash.to_entity().unpack());
                        state
                            .get_tx_with_cycles_from_pool(&short_id)
                            .and_then(|(tx, cycles)| cycles.map(|cycles| (tx, cycles)))
                    };

                    if let Some((tx, cycles)) = entry_opt {
                        let content = packed::RelayTransaction::new_builder()
                            .cycles(cycles.pack())
                            .transaction(tx.data())
                            .build();
                        Some(content)
                    } else {
                        debug_target!(
                            crate::LOG_TARGET_RELAY,
                            "{} request transaction({}), but not found or without cycles",
                            self.peer,
                            tx_hash,
                        );
                        None
                    }
                })
                .collect()
        };

        if !transactions.is_empty() {
            let txs = packed::RelayTransactions::new_builder()
                .transactions(transactions.pack())
                .build();
            let message = packed::RelayMessage::new_builder().set(txs).build();
            let data = message.as_slice().into();
            if let Err(err) = self.nc.send_message_to(self.peer, data) {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "relayer send Transactions error: {:?}",
                    err,
                );
            }
        }
        Ok(())
    }
}
