use crate::pool::TxPool;
use ckb_types::packed::Byte32;
use futures::future::Future;
use tokio::prelude::{Async, Poll};
use tokio::sync::lock::Lock;

pub struct EstimatorProcessBlockProcess {
    pub tx_pool: Lock<TxPool>,
    pub height: u64,
    pub txs: Vec<Byte32>,
}

impl EstimatorProcessBlockProcess {
    pub fn new(
        tx_pool: Lock<TxPool>,
        height: u64,
        txs: Vec<Byte32>,
    ) -> EstimatorProcessBlockProcess {
        EstimatorProcessBlockProcess {
            tx_pool,
            height,
            txs,
        }
    }
}

impl Future for EstimatorProcessBlockProcess {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.tx_pool.poll_lock() {
            Async::Ready(mut guard) => {
                guard
                    .fee_estimator
                    .process_block(self.height, self.txs.iter().cloned());
                Ok(Async::Ready(()))
            }
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}
