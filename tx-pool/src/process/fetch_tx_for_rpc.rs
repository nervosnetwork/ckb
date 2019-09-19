use crate::pool::TxPool;
use ckb_types::core::TransactionView;
use ckb_types::packed::ProposalShortId;
use futures::future::Future;
use tokio::prelude::{Async, Poll};
use tokio::sync::lock::Lock;

pub struct FetchTxRPCProcess {
    pub tx_pool: Lock<TxPool>,
    pub proposal_id: Option<ProposalShortId>,
}

impl FetchTxRPCProcess {
    pub fn new(tx_pool: Lock<TxPool>, proposal_id: ProposalShortId) -> FetchTxRPCProcess {
        FetchTxRPCProcess {
            tx_pool,
            proposal_id: Some(proposal_id),
        }
    }
}

impl Future for FetchTxRPCProcess {
    type Item = Option<(bool, TransactionView)>;
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.tx_pool.poll_lock() {
            Async::Ready(guard) => {
                let id = self.proposal_id.take().expect("cannot poll twice");
                let ret = guard
                    .proposed()
                    .get(&id)
                    .map(|entry| (true, entry.transaction.clone()))
                    .or_else(|| guard.get_tx_without_conflict(&id).map(|tx| (false, tx)));
                Ok(Async::Ready(ret))
            }
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}
