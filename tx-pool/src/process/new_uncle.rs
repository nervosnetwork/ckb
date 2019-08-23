use crate::block_assembler::BlockAssembler;
use ckb_types::core::UncleBlockView;
use faketime::unix_time_as_millis;
use futures::future::Future;
use std::sync::atomic::Ordering;
use tokio::prelude::{Async, Poll};
use tokio::sync::lock::Lock;

pub struct NewUncleProcess {
    pub block_assembler: Lock<BlockAssembler>,
    pub uncle: Option<UncleBlockView>,
}

impl NewUncleProcess {
    pub fn new(block_assembler: Lock<BlockAssembler>, uncle: UncleBlockView) -> NewUncleProcess {
        NewUncleProcess {
            block_assembler,
            uncle: Some(uncle),
        }
    }
}

impl Future for NewUncleProcess {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.block_assembler.poll_lock() {
            Async::Ready(mut guard) => {
                let uncle = self.uncle.take().expect("cannot poll twice");
                guard.candidate_uncles.insert(uncle);
                guard
                    .last_uncles_updated_at
                    .store(unix_time_as_millis(), Ordering::SeqCst);
                Ok(Async::Ready(()))
            }
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}
