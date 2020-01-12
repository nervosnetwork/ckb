use crate::NetworkState;
use ckb_logger::{debug, warn};
use futures::{Async, Future, Stream};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::timer::Interval;

const DEFAULT_DUMP_INTERVAL: Duration = Duration::from_secs(3600); // 1 hour

pub struct DumpPeerStoreService {
    network_state: Arc<NetworkState>,
    interval: Interval,
}

impl DumpPeerStoreService {
    pub fn new(network_state: Arc<NetworkState>) -> Self {
        DumpPeerStoreService {
            network_state,
            interval: Interval::new(Instant::now(), DEFAULT_DUMP_INTERVAL),
        }
    }

    fn dump_peer_store(&self) {
        let path = self.network_state.config.peer_store_path();
        self.network_state.with_peer_store_mut(|peer_store| {
            if let Err(err) = peer_store.dump_to_dir(&path) {
                warn!("Dump peer store error, path: {:?} error: {}", path, err);
            } else {
                debug!("Dump peer store to {:?}", path);
            }
        });
    }
}

impl Drop for DumpPeerStoreService {
    fn drop(&mut self) {
        debug!("dump peer store before exit");
        self.dump_peer_store();
    }
}

impl Future for DumpPeerStoreService {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Result<Async<Self::Item>, Self::Error> {
        loop {
            match self.interval.poll() {
                Ok(Async::Ready(Some(_tick))) => {
                    self.dump_peer_store();
                }
                Ok(Async::Ready(None)) => {
                    warn!("ckb dump peer store service stopped");
                    return Ok(Async::Ready(()));
                }
                Ok(Async::NotReady) => {
                    return Ok(Async::NotReady);
                }
                Err(err) => {
                    warn!("dump peer store service stopped because: {:?}", err);
                    return Err(());
                }
            }
        }
    }
}
