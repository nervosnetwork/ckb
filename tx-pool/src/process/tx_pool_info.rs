use crate::pool::{TxPool, TxPoolInfo};
use futures::future::Future;
use tokio::prelude::{Async, Poll};
use tokio::sync::lock::Lock;

pub struct TxPoolInfoProcess {
    pub tx_pool: Lock<TxPool>,
}

impl Future for TxPoolInfoProcess {
    type Item = TxPoolInfo;
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.tx_pool.poll_lock() {
            Async::Ready(guard) => Ok(Async::Ready(guard.info())),
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}
