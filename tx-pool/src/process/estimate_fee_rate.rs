use crate::pool::TxPool;
use crate::FeeRate;
use futures::future::Future;
use tokio::prelude::{Async, Poll};
use tokio::sync::lock::Lock;

pub struct EstimateFeeRateProcess {
    pub tx_pool: Lock<TxPool>,
    pub expect_confirm_blocks: usize,
}

impl EstimateFeeRateProcess {
    pub fn new(tx_pool: Lock<TxPool>, expect_confirm_blocks: usize) -> EstimateFeeRateProcess {
        EstimateFeeRateProcess {
            tx_pool,
            expect_confirm_blocks,
        }
    }
}

impl Future for EstimateFeeRateProcess {
    type Item = FeeRate;
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.tx_pool.poll_lock() {
            Async::Ready(guard) => {
                let fee_rate = guard.fee_estimator.estimate(self.expect_confirm_blocks);
                Ok(Async::Ready(fee_rate))
            }
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}
