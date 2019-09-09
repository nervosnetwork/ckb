use crate::pool::TxPool;
use ckb_types::core::TransactionView;
use ckb_types::packed::ProposalShortId;
use futures::future::Future;
use std::collections::HashMap;
use tokio::prelude::{Async, Poll};
use tokio::sync::lock::Lock;

pub struct FetchTxsProcess {
    pub tx_pool: Lock<TxPool>,
    pub short_ids: Option<Vec<ProposalShortId>>,
}

impl FetchTxsProcess {
    pub fn new(tx_pool: Lock<TxPool>, short_ids: Vec<ProposalShortId>) -> FetchTxsProcess {
        FetchTxsProcess {
            tx_pool,
            short_ids: Some(short_ids),
        }
    }
}

impl Future for FetchTxsProcess {
    type Item = HashMap<ProposalShortId, TransactionView>;
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.tx_pool.poll_lock() {
            Async::Ready(guard) => {
                let short_ids = self.short_ids.take().expect("cannot poll twice");
                let ret = short_ids
                    .into_iter()
                    .filter_map(|short_id| {
                        if let Some(tx) = guard.get_tx_from_pool_or_store(&short_id) {
                            Some((short_id, tx))
                        } else {
                            None
                        }
                    })
                    .collect();
                Ok(Async::Ready(ret))
            }
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}
