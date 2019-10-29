use crate::pool::TxPool;
use crate::FeeRate;
use ckb_types::packed::Byte32;
use futures::future::Future;
use tokio::prelude::{Async, Poll};
use tokio::sync::lock::Lock;

pub struct EstimatorTrackTxProcess {
    pub tx_pool: Lock<TxPool>,
    pub tx_hash: Byte32,
    pub fee_rate: FeeRate,
    pub height: u64,
}

impl EstimatorTrackTxProcess {
    pub fn new(
        tx_pool: Lock<TxPool>,
        tx_hash: Byte32,
        fee_rate: FeeRate,
        height: u64,
    ) -> EstimatorTrackTxProcess {
        EstimatorTrackTxProcess {
            tx_pool,
            tx_hash,
            fee_rate,
            height,
        }
    }
}

impl Future for EstimatorTrackTxProcess {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.tx_pool.poll_lock() {
            Async::Ready(mut guard) => {
                guard
                    .fee_estimator
                    .track_tx(self.tx_hash.clone(), self.fee_rate, self.height);
                Ok(Async::Ready(()))
            }
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}
