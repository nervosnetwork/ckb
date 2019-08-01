use crate::relayer::compact_block::GetRelayTransactions;
use crate::relayer::Relayer;
use ckb_core::transaction::ProposalShortId;
use ckb_logger::{debug_target, trace_target};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{GetRelayTransactions as FbsGetRelayTransactions, RelayMessage};
use ckb_store::ChainStore;
use failure::Error as FailureError;
use flatbuffers::FlatBufferBuilder;
use std::convert::TryInto;
use std::sync::Arc;

pub struct GetTransactionsProcess<'a, CS> {
    message: &'a FbsGetRelayTransactions<'a>,
    relayer: &'a Relayer<CS>,
    nc: Arc<dyn CKBProtocolContext>,
    peer: PeerIndex,
}

impl<'a, CS: ChainStore> GetTransactionsProcess<'a, CS> {
    pub fn new(
        message: &'a FbsGetRelayTransactions,
        relayer: &'a Relayer<CS>,
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
        let get_relay_tx: GetRelayTransactions = (*self.message).try_into()?;

        trace_target!(
            crate::LOG_TARGET_RELAY,
            "{} request transactions({:#?})",
            self.peer,
            get_relay_tx.hashes
        );

        let mut transactions = Vec::with_capacity(get_relay_tx.hashes.len());
        {
            let state = self.relayer.shared.lock_chain_state();

            for tx_hash in get_relay_tx.hashes {
                let entry_opt = {
                    let short_id = ProposalShortId::from_tx_hash(&tx_hash);
                    state
                        .get_tx_with_cycles_from_pool(&short_id)
                        .and_then(|(tx, cycles)| cycles.map(|cycles| (tx, cycles)))
                };

                if let Some((tx, cycles)) = entry_opt {
                    transactions.push((tx, cycles));
                } else {
                    debug_target!(
                        crate::LOG_TARGET_RELAY,
                        "{} request transaction({:#x}), but not found or without cycles",
                        self.peer,
                        tx_hash,
                    );
                }
            }
        }

        if !transactions.is_empty() {
            let fbb = &mut FlatBufferBuilder::new();
            let message = RelayMessage::build_transactions(fbb, &transactions);
            fbb.finish(message, None);
            let data = fbb.finished_data().into();
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
