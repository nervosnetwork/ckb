use crate::pool::TxPool;
use ckb_types::core::{Cycle, TransactionView};
use ckb_types::packed::ProposalShortId;
use futures::future::Future;
use tokio::prelude::{Async, Poll};
use tokio::sync::lock::Lock;

pub struct FetchTxsWithCyclesProcess {
    pub tx_pool: Lock<TxPool>,
    pub short_ids: Option<Vec<ProposalShortId>>,
}

impl FetchTxsWithCyclesProcess {
    pub fn new(
        tx_pool: Lock<TxPool>,
        short_ids: Vec<ProposalShortId>,
    ) -> FetchTxsWithCyclesProcess {
        FetchTxsWithCyclesProcess {
            tx_pool,
            short_ids: Some(short_ids),
        }
    }
}

impl Future for FetchTxsWithCyclesProcess {
    type Item = Vec<(ProposalShortId, (TransactionView, Cycle))>;
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.tx_pool.poll_lock() {
            Async::Ready(guard) => {
                let short_ids = self.short_ids.take().expect("cannot poll twice");
                let ret = short_ids
                    .into_iter()
                    .filter_map(|short_id| {
                        guard
                            .get_tx_with_cycles(&short_id)
                            .and_then(|(tx, cycles)| cycles.map(|cycles| (short_id, (tx, cycles))))
                    })
                    .collect();
                Ok(Async::Ready(ret))
            }
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}
