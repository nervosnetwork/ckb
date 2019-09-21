use crate::pool::TxPool;
use ckb_types::packed::ProposalShortId;
use futures::future::Future;
use tokio::prelude::{Async, Poll};
use tokio::sync::lock::Lock;

pub struct FreshProposalsFilterProcess {
    pub tx_pool: Lock<TxPool>,
    pub proposals: Option<Vec<ProposalShortId>>,
}

impl FreshProposalsFilterProcess {
    pub fn new(
        tx_pool: Lock<TxPool>,
        proposals: Vec<ProposalShortId>,
    ) -> FreshProposalsFilterProcess {
        FreshProposalsFilterProcess {
            tx_pool,
            proposals: Some(proposals),
        }
    }
}

impl Future for FreshProposalsFilterProcess {
    type Item = Vec<ProposalShortId>;
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.tx_pool.poll_lock() {
            Async::Ready(guard) => {
                let mut proposals = self.proposals.take().expect("cannot poll twice");
                proposals.retain(|id| !guard.contains_proposal_id(&id));
                Ok(Async::Ready(proposals))
            }
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}
