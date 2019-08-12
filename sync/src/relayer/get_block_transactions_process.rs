use crate::relayer::Relayer;
use crate::{attempt, Status};
use ckb_logger::debug_target;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{cast, GetBlockTransactions, RelayMessage};
use ckb_store::ChainStore;
use flatbuffers::FlatBufferBuilder;
use numext_fixed_hash::H256;
use std::convert::TryInto;
use std::sync::Arc;

pub struct GetBlockTransactionsProcess<'a> {
    message: &'a GetBlockTransactions<'a>,
    relayer: &'a Relayer,
    nc: Arc<dyn CKBProtocolContext>,
    peer: PeerIndex,
}

impl<'a> GetBlockTransactionsProcess<'a> {
    pub fn new(
        message: &'a GetBlockTransactions,
        relayer: &'a Relayer,
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

    pub fn execute(self) -> Status {
        let block_hash = attempt!(TryInto::<H256>::try_into(attempt!(cast!(self
            .message
            .block_hash()))));
        let indexes = attempt!(cast!(self.message.indexes()));
        debug_target!(
            crate::LOG_TARGET_RELAY,
            "get_block_transactions {:x}",
            block_hash
        );

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

        Status::ok()
    }
}
