use crate::block_assembler::{CandidateUncles};
use ckb_types::core::UncleBlockView;
use faketime::unix_time_as_millis;
use futures::future::Future;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::prelude::{Async, Poll};
use tokio::sync::lock::Lock;

pub struct NewUncleProcess {
    pub candidate_uncles: Lock<CandidateUncles>,
    pub last_uncles_updated_at: Arc<AtomicU64>,
    pub uncle: Option<UncleBlockView>,
}

impl NewUncleProcess {
    pub fn new(
        candidate_uncles: Lock<CandidateUncles>,
        last_uncles_updated_at: Arc<AtomicU64>,
        uncle: UncleBlockView,
    ) -> NewUncleProcess {
        NewUncleProcess {
            candidate_uncles,
            last_uncles_updated_at,
            uncle: Some(uncle),
        }
    }
}

impl Future for NewUncleProcess {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.candidate_uncles.poll_lock() {
            Async::Ready(mut guard) => {
                let uncle = self.uncle.take().expect("cannot poll twice");
                guard.insert(uncle);
                self.last_uncles_updated_at
                    .store(unix_time_as_millis(), Ordering::SeqCst);
                Ok(Async::Ready(()))
            }
            Async::NotReady => Ok(Async::NotReady),
        }
    }
}
