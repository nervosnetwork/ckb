use crate::{
    peer_store::{types::AddrInfo, PeerStore},
    NetworkState,
};
use ckb_logger::trace;
use faketime::unix_time_as_millis;
use futures::Future;
use p2p::{multiaddr::MultiAddr, service::ServiceControl};
use rand::prelude::IteratorRandom;
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::{Interval, MissedTickBehavior};

const FEELER_CONNECTION_COUNT: usize = 10;

/// Ensure that the outbound of the current node reaches the expected upper limit as much as possible
/// Periodically detect and verify data in the peer store
/// Keep the whitelist nodes connected as much as possible
/// Periodically detection finds that the observed addresses are all valid
pub struct OutboundPeerService {
    network_state: Arc<NetworkState>,
    p2p_control: ServiceControl,
    interval: Option<Interval>,
    try_connect_interval: Duration,
    try_identify_count: u8,
}

impl OutboundPeerService {
    pub fn new(
        network_state: Arc<NetworkState>,
        p2p_control: ServiceControl,
        try_connect_interval: Duration,
    ) -> Self {
        OutboundPeerService {
            network_state,
            p2p_control,
            interval: None,
            try_connect_interval,
            try_identify_count: 0,
        }
    }

    fn dial_feeler(&mut self) {
        let now_ms = unix_time_as_millis();
        let attempt_peers = self.network_state.with_peer_store_mut(|peer_store| {
            let paddrs = peer_store.fetch_addrs_to_feeler(FEELER_CONNECTION_COUNT);
            for paddr in paddrs.iter() {
                // mark addr as tried
                if let Some(paddr) = peer_store.mut_addr_manager().get_mut(&paddr.addr) {
                    paddr.mark_tried(now_ms);
                }
            }
            paddrs
        });

        trace!(
            "feeler dial count={}, attempt_peers: {:?}",
            attempt_peers.len(),
            attempt_peers,
        );

        for addr in attempt_peers.into_iter().map(|info| info.addr) {
            self.network_state.dial_feeler(&self.p2p_control, addr);
        }
    }

    fn try_dial_peers(&mut self) {
        let status = self.network_state.connection_status();
        let count = status
            .max_outbound
            .saturating_sub(status.non_whitelist_outbound) as usize;
        if count == 0 {
            self.try_identify_count = 0;
            return;
        }
        self.try_identify_count += 1;

        let target = &self.network_state.required_flags;

        let f = |peer_store: &mut PeerStore, number: usize, now_ms: u64| -> Vec<AddrInfo> {
            let paddrs = peer_store.fetch_addrs_to_attempt(number, *target);
            for paddr in paddrs.iter() {
                // mark addr as tried
                if let Some(paddr) = peer_store.mut_addr_manager().get_mut(&paddr.addr) {
                    paddr.mark_tried(now_ms);
                }
            }
            paddrs
        };

        let peers: Box<dyn Iterator<Item = MultiAddr>> = if self.try_identify_count > 3 {
            self.try_identify_count = 0;
            let len = self.network_state.bootnodes.len();
            if len < count {
                let now_ms = unix_time_as_millis();
                let attempt_peers = self
                    .network_state
                    .with_peer_store_mut(|peer_store| f(peer_store, count - len, now_ms));

                Box::new(
                    attempt_peers
                        .into_iter()
                        .map(|info| info.addr)
                        .chain(self.network_state.bootnodes.iter().cloned()),
                )
            } else {
                Box::new(
                    self.network_state
                        .bootnodes
                        .iter()
                        .choose_multiple(&mut rand::thread_rng(), count)
                        .into_iter()
                        .cloned(),
                )
            }
        } else {
            let now_ms = unix_time_as_millis();
            let attempt_peers = self
                .network_state
                .with_peer_store_mut(|peer_store| f(peer_store, count, now_ms));

            trace!(
                "identify dial count={}, attempt_peers: {:?}",
                attempt_peers.len(),
                attempt_peers,
            );

            Box::new(attempt_peers.into_iter().map(|info| info.addr))
        };

        for addr in peers {
            self.network_state.dial_identify(&self.p2p_control, addr);
        }
    }

    fn try_dial_whitelist(&self) {
        for addr in self.network_state.config.whitelist_peers() {
            self.network_state.dial_identify(&self.p2p_control, addr);
        }
    }

    fn try_dial_observed(&self) {
        self.network_state
            .try_dial_observed_addrs(&self.p2p_control);
    }
}

impl Future for OutboundPeerService {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.interval.is_none() {
            self.interval = {
                let mut interval = tokio::time::interval(self.try_connect_interval);
                // The outbound service does not need to urgently compensate for the missed wake,
                // just skip behavior is enough
                interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
                Some(interval)
            }
        }
        while self.interval.as_mut().unwrap().poll_tick(cx).is_ready() {
            // keep whitelist peer on connected
            self.try_dial_whitelist();
            // ensure feeler work at any time
            self.dial_feeler();
            // keep outbound peer is enough
            self.try_dial_peers();
            // try dial observed addrs
            self.try_dial_observed();
        }
        Poll::Pending
    }
}
