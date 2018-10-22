use bigint::H256;
use ckb_chain::chain::ChainProvider;
use ckb_protocol::{GetBlockTransactions, RelayMessage};
use flatbuffers::FlatBufferBuilder;
use network::{CKBProtocolContext, PeerIndex};
use relayer::Relayer;

pub struct GetBlockTransactionsProcess<'a, C: 'a> {
    message: &'a GetBlockTransactions<'a>,
    relayer: &'a Relayer<C>,
    peer: PeerIndex,
    nc: &'a CKBProtocolContext,
}

impl<'a, C> GetBlockTransactionsProcess<'a, C>
where
    C: ChainProvider + 'static,
{
    pub fn new(
        message: &'a GetBlockTransactions,
        relayer: &'a Relayer<C>,
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
        let hash = H256::from_slice(self.message.hash().and_then(|bytes| bytes.seq()).unwrap());
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
