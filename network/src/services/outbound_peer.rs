use crate::peer_store::types::PeerAddr;
use crate::NetworkState;
use ckb_logger::{trace, warn};
use faketime::unix_time_as_millis;
use futures::{Async, Future, Stream};
use p2p::service::ServiceControl;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::timer::Interval;

const FEELER_CONNECTION_COUNT: u32 = 5;

pub struct OutboundPeerService {
    network_state: Arc<NetworkState>,
    p2p_control: ServiceControl,
    interval: Interval,
    try_connect_interval: Duration,
    last_connect: Option<Instant>,
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
            interval: Interval::new(Instant::now(), Duration::from_secs(1)),
            try_connect_interval,
            last_connect: None,
        }
    }

    fn dial_peers(&mut self, is_feeler: bool, count: u32) {
        let now_ms = unix_time_as_millis();
        let attempt_peers = self.network_state.with_peer_store_mut(|peer_store| {
            // take extra 5 peers
            // in current implementation fetch peers may return less than count
            let extra_count = 5;
            let mut paddrs = if is_feeler {
                peer_store.peers_to_feeler(count + extra_count)
            } else {
                peer_store.peers_to_attempt(count + extra_count)
            };
            paddrs.truncate(count as usize);
            for paddr in &mut paddrs {
                // mark addr as tried
                paddr.mark_tried(now_ms);
                peer_store.update_peer_addr(&paddr);
            }
            paddrs
        });
        trace!(
            "count={}, attempt_peers: {:?} is_feeler: {}",
            count,
            attempt_peers,
            is_feeler
        );
        // keep whitelist peer on connected
        self.try_dial_whitelist();

        for paddr in attempt_peers {
            let PeerAddr { peer_id, addr, .. } = paddr;
            if is_feeler {
                self.network_state
                    .dial_feeler(&self.p2p_control, &peer_id, addr);
            } else {
                self.network_state
                    .dial_identify(&self.p2p_control, &peer_id, addr);
            }
        }
    }

    fn try_dial_whitelist(&self) {
        // This will never panic because network start has already been checked
        for (peer_id, addr) in self
            .network_state
            .config
            .whitelist_peers()
            .expect("address must be correct")
        {
            if self.network_state.query_session_id(&peer_id).is_none() {
                self.network_state
                    .dial_identify(&self.p2p_control, &peer_id, addr);
            }
        }
    }
}

impl Future for OutboundPeerService {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Result<Async<Self::Item>, Self::Error> {
        loop {
            match self.interval.poll() {
                Ok(Async::Ready(Some(_tick))) => {
                    let last_connect = self
                        .last_connect
                        .map(|time| time.elapsed())
                        .unwrap_or(Duration::from_secs(std::u64::MAX));
                    if last_connect > self.try_connect_interval {
                        let status = self.network_state.connection_status();
                        let new_outbound = status
                            .max_outbound
                            .saturating_sub(status.non_whitelist_outbound);
                        if self.network_state.config.whitelist_only {
                            self.try_dial_whitelist()
                        } else if new_outbound > 0 {
                            // dial peers
                            self.dial_peers(false, new_outbound as u32);
                        } else {
                            // feeler peers
                            self.dial_peers(true, FEELER_CONNECTION_COUNT);
                        }
                        self.last_connect = Some(Instant::now());
                    }
                }
                Ok(Async::Ready(None)) => {
                    warn!("ckb outbound peer service stopped");
                    return Ok(Async::Ready(()));
                }
                Ok(Async::NotReady) => {
                    return Ok(Async::NotReady);
                }
                Err(err) => {
                    warn!("outbound peer service stopped because: {:?}", err);
                    return Err(());
                }
            }
        }
    }
}
