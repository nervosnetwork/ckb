use crate::NetworkState;
use ckb_logger::debug;
use futures::Future;
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

const DEFAULT_DUMP_INTERVAL: Duration = Duration::from_secs(3600); // 1 hour

/// Save current peer store data regularly
pub struct DumpPeerStoreService {
    network_state: Arc<NetworkState>,
    #[cfg(not(target_family = "wasm"))]
    interval: Option<tokio::time::Interval>,
    #[cfg(target_family = "wasm")]
    interval: Option<p2p::runtime::Interval>,
}

impl DumpPeerStoreService {
    pub fn new(network_state: Arc<NetworkState>) -> Self {
        DumpPeerStoreService {
            network_state,
            interval: None,
        }
    }

    #[cfg(not(target_family = "wasm"))]
    fn dump_peer_store(&self) {
        let path = self.network_state.config.peer_store_path();
        self.network_state.with_peer_store_mut(|peer_store| {
            if let Err(err) = peer_store.dump_to_dir(&path) {
                ckb_logger::warn!("Dump peer store error, path: {:?} error: {}", path, err);
            } else {
                debug!("Dump peer store to {:?}", path);
            }
        });
    }

    #[cfg(target_family = "wasm")]
    fn dump_peer_store(&self) {
        let config = &self.network_state.config;
        self.network_state
            .with_peer_store_mut(|peer_store| peer_store.dump_with_config(config));
    }
}

impl Drop for DumpPeerStoreService {
    fn drop(&mut self) {
        debug!("Dump peer store before exiting");
        self.dump_peer_store();
    }
}

impl Future for DumpPeerStoreService {
    type Output = ();

    #[cfg(not(target_family = "wasm"))]
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.interval.is_none() {
            self.interval = {
                let mut interval = tokio::time::interval_at(
                    tokio::time::Instant::now() + DEFAULT_DUMP_INTERVAL,
                    DEFAULT_DUMP_INTERVAL,
                );
                // The dump peer store service does not need to urgently compensate for the missed wake,
                // just delay behavior is enough
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                Some(interval)
            }
        }
        while self.interval.as_mut().unwrap().poll_tick(cx).is_ready() {
            self.dump_peer_store()
        }
        Poll::Pending
    }

    #[cfg(target_family = "wasm")]
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        use futures::StreamExt;
        if self.interval.is_none() {
            self.interval = {
                let mut interval = p2p::runtime::Interval::new(DEFAULT_DUMP_INTERVAL);
                // The outbound service does not need to urgently compensate for the missed wake,
                // just skip behavior is enough
                interval.set_missed_tick_behavior(p2p::runtime::MissedTickBehavior::Skip);
                Some(interval)
            }
        }
        while self
            .interval
            .as_mut()
            .unwrap()
            .poll_next_unpin(cx)
            .is_ready()
        {
            self.dump_peer_store()
        }
        Poll::Pending
    }
}
