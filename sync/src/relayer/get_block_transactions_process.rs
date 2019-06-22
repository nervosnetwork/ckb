use crate::relayer::Relayer;
use ckb_logger::debug_target;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{cast, GetBlockTransactions, RelayMessage};
use ckb_store::ChainStore;
use failure::Error as FailureError;
use flatbuffers::FlatBufferBuilder;
use std::convert::TryInto;
use std::sync::Arc;

pub struct GetBlockTransactionsProcess<'a, CS> {
    message: &'a GetBlockTransactions<'a>,
    relayer: &'a Relayer<CS>,
    nc: Arc<dyn CKBProtocolContext>,
    peer: PeerIndex,
}

impl<'a, CS: ChainStore> GetBlockTransactionsProcess<'a, CS> {
    pub fn new(
        message: &'a GetBlockTransactions,
        relayer: &'a Relayer<CS>,
        nc: Arc<dyn CKBProtocolContext>,
        peer: PeerIndex,
    ) -> Self {
        GetBlockTransactionsProcess {
            message,
            nc,
            relayer,
            peer,
        }
    }

    pub fn execute(self) -> Result<(), FailureError> {
        let block_hash = cast!(self.message.block_hash())?.try_into()?;
        debug_target!(
            crate::LOG_TARGET_RELAY,
            "get_block_transactions {:x}",
            block_hash
        );

        let indexes = cast!(self.message.indexes())?;

        if let Some(block) = self.relayer.shared.store().get_block(&block_hash) {
            let transactions = indexes
                .safe_slice()
                .iter()
                .filter_map(|i| block.transactions().get(*i as usize).cloned())
                .map(Into::into)
                .collect::<Vec<_>>();
            let fbb = &mut FlatBufferBuilder::new();
            let message = RelayMessage::build_block_transactions(fbb, &block_hash, &transactions);
            fbb.finish(message, None);

            if let Err(err) = self
                .nc
                .send_message_to(self.peer, fbb.finished_data().into())
            {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "relayer send BlockTransactions error: {:?}",
                    err
                );
            }
        }

        Ok(())
    }
}
