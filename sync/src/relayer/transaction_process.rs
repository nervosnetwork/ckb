use ckb_protocol::Transaction;
use ckb_shared::index::ChainIndex;
use network::{CKBProtocolContext, PeerIndex};
use relayer::Relayer;

// TODO PENDING remove this attribute later
#[allow(dead_code)]
pub struct TransactionProcess<'a, CI: ChainIndex + 'a> {
    message: &'a Transaction<'a>,
    relayer: &'a Relayer<CI>,
    peer: PeerIndex,
    nc: &'a CKBProtocolContext,
}

impl<'a, CI> TransactionProcess<'a, CI>
where
    CI: ChainIndex + 'static,
{
    pub fn new(
        message: &'a Transaction,
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
        let tx = (*self.message).into();
        let _ = self.relayer.tx_pool.add_transaction(tx);
        // TODO PENDING new api NetworkContext#connected_peers
        // for peer_id in self.nc.connected_peers() {
        //     let data = builde_transaction(indexed_tx);
        //     self.nc.send(peer_id, 0, data.to_vec());
        // }
    }
}
