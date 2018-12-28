use crate::relayer::Relayer;
use ckb_core::transaction::Transaction;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{RelayMessage, Transaction as FbsTransaction};
use ckb_shared::index::ChainIndex;
use flatbuffers::FlatBufferBuilder;

pub struct TransactionProcess<'a, CI: ChainIndex + 'a> {
    message: &'a FbsTransaction<'a>,
    relayer: &'a Relayer<CI>,
    peer: PeerIndex,
    nc: &'a CKBProtocolContext,
}

impl<'a, CI> TransactionProcess<'a, CI>
where
    CI: ChainIndex + 'static,
{
    pub fn new(
        message: &'a FbsTransaction,
        relayer: &'a Relayer<CI>,
        peer: PeerIndex,
        nc: &'a CKBProtocolContext,
    ) -> Self {
        TransactionProcess {
            message,
            nc,
            relayer,
            peer,
        }
    }

    pub fn execute(self) {
        let tx: Transaction = (*self.message).into();
        if self.relayer.tx_pool.add_transaction(tx.clone()).is_ok() {
            let fbb = &mut FlatBufferBuilder::new();
            let message = RelayMessage::build_transaction(fbb, &tx);
            fbb.finish(message, None);

            for peer_id in self.nc.connected_peers() {
                if peer_id != self.peer
                    && self
                        .relayer
                        .peers()
                        .transaction_filters
                        .read()
                        .get(&peer_id)
                        .map_or(true, |filter| filter.contains(&tx))
                {
                    let _ = self.nc.send(peer_id, fbb.finished_data().to_vec());
                }
            }
        }
    }
}
