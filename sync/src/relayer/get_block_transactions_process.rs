use crate::relayer::Relayer;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{GetBlockTransactions, RelayMessage};
use ckb_shared::index::ChainIndex;
use flatbuffers::FlatBufferBuilder;
use log::debug;
use numext_fixed_hash::H256;

pub struct GetBlockTransactionsProcess<'a, CI: ChainIndex + 'a> {
    message: &'a GetBlockTransactions<'a>,
    relayer: &'a Relayer<CI>,
    peer: PeerIndex,
    nc: &'a CKBProtocolContext,
}

impl<'a, CI> GetBlockTransactionsProcess<'a, CI>
where
    CI: ChainIndex + 'static,
{
    pub fn new(
        message: &'a GetBlockTransactions,
        relayer: &'a Relayer<CI>,
        peer: PeerIndex,
        nc: &'a CKBProtocolContext,
    ) -> Self {
        GetBlockTransactionsProcess {
            message,
            nc,
            peer,
            relayer,
        }
    }

    pub fn execute(self) {
        let hash =
            H256::from_slice(self.message.hash().and_then(|bytes| bytes.seq()).unwrap()).unwrap();
        debug!(target: "relay", "get_block_transactions {:?}", hash);

        if let Some(block) = self.relayer.get_block(&hash) {
            let transactions = self
                .message
                .indexes()
                .unwrap()
                .safe_slice()
                .iter()
                .filter_map(|i| block.commit_transactions().get(*i as usize).cloned())
                .map(Into::into)
                .collect::<Vec<_>>();
            let fbb = &mut FlatBufferBuilder::new();
            let message = RelayMessage::build_block_transactions(fbb, &hash, &transactions);
            fbb.finish(message, None);

            let _ = self.nc.send(self.peer, fbb.finished_data().to_vec());
        }
    }
}
