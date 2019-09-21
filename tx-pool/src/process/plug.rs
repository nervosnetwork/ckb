use crate::component::entry::TxEntry;
use crate::pool::TxPool;
use futures::future::Future;
use tokio::prelude::{Async, Poll};
use tokio::sync::lock::Lock;

pub enum PlugTarget {
    Pending,
    Proposed,
}

pub struct PlugEntryProcess {
    pub tx_pool: Lock<TxPool>,
    pub entries: Option<Vec<TxEntry>>,
    pub target: PlugTarget,
}

impl PlugEntryProcess {
    pub fn new(
        tx_pool: Lock<TxPool>,
        entries: Vec<TxEntry>,
        target: PlugTarget,
    ) -> PlugEntryProcess {
        PlugEntryProcess {
            tx_pool,
            target,
            entries: Some(entries),
        }
    }
}

impl Future for PlugEntryProcess {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.tx_pool.poll_lock() {
            Async::Ready(mut tx_pool) => {
                let entries = self.entries.take().expect("cannot execute twice");
                match self.target {
                    PlugTarget::Pending => {
                        for entry in entries {
                            tx_pool.add_pending(entry);
                        }
                    }
                    PlugTarget::Proposed => {
                        for entry in entries {
                            tx_pool.add_proposed(entry);
                        }
                    }
                }
                Ok(Async::Ready(()))
            }
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}
