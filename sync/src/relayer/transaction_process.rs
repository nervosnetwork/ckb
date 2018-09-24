use ckb_chain::chain::ChainProvider;
use ckb_chain::PowEngine;
use ckb_protocol::Transaction;
use core::transaction::IndexedTransaction;
use network::{NetworkContext, PeerId};
use relayer::Relayer;

// TODO PENDING remove this attribute later
#[allow(dead_code)]
pub struct TransactionProcess<'a, C: 'a, P: 'a> {
    message: &'a Transaction<'a>,
    relayer: &'a Relayer<C, P>,
    peer: PeerId,
    nc: &'a NetworkContext,
}

impl<'a, C, P> TransactionProcess<'a, C, P>
where
    C: ChainProvider + 'a,
    P: PowEngine + 'a,
{
    pub fn new(
        message: &'a Transaction,
        relayer: &'a Relayer<C, P>,
        peer: PeerId,
        nc: &'a NetworkContext,
    ) -> Self {
        TransactionProcess {
            message,
            nc,
            relayer,
            peer,
        }
    }

    pub fn execute(self) {
        let tx: IndexedTransaction = (*self.message).into();
        let _ = self.relayer.tx_pool.add_transaction(tx);
        // TODO PENDING new api NetworkContext#connected_peers
        // for peer_id in self.nc.connected_peers() {
        //     let data = builde_transaction(indexed_tx);
        //     self.nc.send(peer_id, 0, data.to_vec());
        // }
    }
}
