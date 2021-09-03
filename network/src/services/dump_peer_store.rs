use crate::NetworkState;
use ckb_logger::{debug, warn};
use futures::Future;
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::{Instant, Interval, MissedTickBehavior};

const DEFAULT_DUMP_INTERVAL: Duration = Duration::from_secs(3600); // 1 hour

/// Save current peer store data regularly
pub struct DumpPeerStoreService {
    network_state: Arc<NetworkState>,
    interval: Option<Interval>,
}

impl DumpPeerStoreService {
    pub fn new(network_state: Arc<NetworkState>) -> Self {
        DumpPeerStoreService {
            network_state,
            interval: None,
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
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.interval.is_none() {
            self.interval = {
                let mut interval = tokio::time::interval_at(
                    Instant::now() + DEFAULT_DUMP_INTERVAL,
                    DEFAULT_DUMP_INTERVAL,
                );
                // The dump peer store service does not need to urgently compensate for the missed wake,
                // just delay behavior is enough
                interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
                Some(interval)
            }
        }
        while self.interval.as_mut().unwrap().poll_tick(cx).is_ready() {
            self.dump_peer_store()
        }
        Poll::Pending
    }
}
