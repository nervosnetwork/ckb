use crate::relayer::Relayer;
use ckb_core::transaction::ProposalShortId;
use ckb_logger::{debug_target, trace_target};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{GetRelayTransaction as FbsGetRelayTransaction, RelayMessage};
use ckb_store::ChainStore;
use failure::Error as FailureError;
use flatbuffers::FlatBufferBuilder;
use std::convert::TryInto;
use std::sync::Arc;

pub struct GetTransactionProcess<'a, CS> {
    message: &'a FbsGetRelayTransaction<'a>,
    relayer: &'a Relayer<CS>,
    nc: Arc<dyn CKBProtocolContext>,
    peer: PeerIndex,
}

impl<'a, CS: ChainStore> GetTransactionProcess<'a, CS> {
    pub fn new(
        message: &'a FbsGetRelayTransaction,
        relayer: &'a Relayer<CS>,
        nc: Arc<dyn CKBProtocolContext>,
        peer: PeerIndex,
    ) -> Self {
        GetTransactionProcess {
            message,
            relayer,
            nc,
            peer,
        }
    }

    pub fn execute(self) -> Result<(), FailureError> {
        let tx_hash = (*self.message).try_into()?;
        trace_target!(
            crate::LOG_TARGET_RELAY,
            "{} request transaction({:#x})",
            self.peer,
            tx_hash
        );
        let entry_opt = {
            let short_id = ProposalShortId::from_tx_hash(&tx_hash);
            self.relayer
                .shared
                .lock_chain_state()
                .get_tx_with_cycles_from_pool(&short_id)
                .and_then(|(tx, cycles)| cycles.map(|cycles| (tx, cycles)))
        };
        if let Some((tx, cycles)) = entry_opt {
            let fbb = &mut FlatBufferBuilder::new();
            let message = RelayMessage::build_transaction(fbb, &tx, cycles);
            fbb.finish(message, None);
            let data = fbb.finished_data().into();
            if let Err(err) = self.nc.send_message_to(self.peer, data) {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "relayer send Transaction error: {:?}",
                    err,
                );
            }
        } else {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "{} request transaction({:#x}), but not found or without cycles",
                self.peer,
                tx_hash,
            );
        }
        Ok(())
    }
}
