use crate::relayer::Relayer;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{cast, GetBlockTransactions, RelayMessage};
use ckb_shared::store::ChainStore;
use failure::Error as FailureError;
use flatbuffers::FlatBufferBuilder;
use log::debug;
use std::convert::TryInto;

pub struct GetBlockTransactionsProcess<'a, CS> {
    message: &'a GetBlockTransactions<'a>,
    relayer: &'a Relayer<CS>,
    nc: &'a CKBProtocolContext,
    peer: PeerIndex,
}

impl<'a, CS: ChainStore> GetBlockTransactionsProcess<'a, CS> {
    pub fn new(
        message: &'a GetBlockTransactions,
        relayer: &'a Relayer<CS>,
        nc: &'a CKBProtocolContext,
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
        debug!(target: "relay", "get_block_transactions {:?}", block_hash);

        let indexes = cast!(self.message.indexes())?;

        if let Some(block) = self.relayer.shared.get_block(&block_hash) {
            let transactions = indexes
                .safe_slice()
                .iter()
                .filter_map(|i| block.transactions().get(*i as usize).cloned())
                .map(Into::into)
                .collect::<Vec<_>>();
            let fbb = &mut FlatBufferBuilder::new();
            let message = RelayMessage::build_block_transactions(fbb, &block_hash, &transactions);
            fbb.finish(message, None);

            self.nc
                .send_message_to(self.peer, fbb.finished_data().to_vec());
        }

        Ok(())
    }
}
