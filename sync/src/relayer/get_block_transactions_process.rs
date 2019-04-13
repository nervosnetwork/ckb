use crate::relayer::Relayer;
use ckb_network::{CKBProtocolContext, SessionId};
use ckb_protocol::{cast, GetBlockTransactions, RelayMessage};
use ckb_shared::store::ChainStore;
use failure::Error as FailureError;
use flatbuffers::FlatBufferBuilder;
use log::{debug, warn};
use std::convert::TryInto;

pub struct GetBlockTransactionsProcess<'a, CS> {
    message: &'a GetBlockTransactions<'a>,
    relayer: &'a Relayer<CS>,
    peer: SessionId,
    nc: &'a mut CKBProtocolContext,
}

impl<'a, CS: ChainStore> GetBlockTransactionsProcess<'a, CS> {
    pub fn new(
        message: &'a GetBlockTransactions,
        relayer: &'a Relayer<CS>,
        peer: SessionId,
        nc: &'a mut CKBProtocolContext,
    ) -> Self {
        GetBlockTransactionsProcess {
            message,
            nc,
            peer,
            relayer,
        }
    }

    pub fn execute(self) -> Result<(), FailureError> {
        let hash = cast!(self.message.hash())?.try_into()?;
        debug!(target: "relay", "get_block_transactions {:?}", hash);

        let indexes = cast!(self.message.indexes())?;

        if let Some(block) = self.relayer.get_block(&hash) {
            let transactions = indexes
                .safe_slice()
                .iter()
                .filter_map(|i| block.commit_transactions().get(*i as usize).cloned())
                .map(Into::into)
                .collect::<Vec<_>>();
            let fbb = &mut FlatBufferBuilder::new();
            let message = RelayMessage::build_block_transactions(fbb, &hash, &transactions);
            fbb.finish(message, None);

            let ret = self.nc.send(self.peer, fbb.finished_data().to_vec());
            if ret.is_err() {
                warn!(target: "relay", "GetBlockTransactionsProcess response error {:?}", ret);
            }
        }

        Ok(())
    }
}
