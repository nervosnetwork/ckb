use crate::protocols::BackgroundService;
use crate::NetworkState;
use futures::{try_ready, Async, Stream};
use log::{debug, trace, warn};
use p2p::service::ServiceControl;
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::usize;
use tokio::timer::Interval;

const FEELER_CONNECTION_COUNT: u32 = 5;

pub struct OutboundPeerService {
    pub stream_interval: Interval,
    pub p2p_control: ServiceControl,
}

impl OutboundPeerService {
    pub fn new(p2p_control: ServiceControl, try_connect_interval: Duration) -> Self {
        debug!(target: "network", "outbound peer service start, interval: {:?}", try_connect_interval);
        OutboundPeerService {
            p2p_control,
            stream_interval: Interval::new_interval(try_connect_interval),
        }
    }

    fn attempt_dial_peers(&mut self, network_state: &NetworkState, count: u32) {
        let attempt_peers = network_state.peer_store().peers_to_attempt(count + 5);
        let mut p2p_control = self.p2p_control.clone();
        trace!(target: "network", "count={}, attempt_peers: {:?}", count, attempt_peers);
        for (peer_id, addr) in attempt_peers
            .into_iter()
            .filter(|(peer_id, _addr)| {
                network_state.local_peer_id() != peer_id
                    && network_state
                        .failed_dials
                        .get(peer_id)
                        .map(|last_dial| {
                            // Dial after 5 minutes when last failed
                            Instant::now() - *last_dial > Duration::from_secs(300)
                        })
                        .unwrap_or(true)
            })
            .take(count as usize)
        {
            debug!(target: "network", "dial attempt peer: {:?}", addr);
            network_state.dial_all(&mut p2p_control, &peer_id, addr);
        }
    }

    fn feeler_peers(&mut self, network_state: &NetworkState, count: u32) {
        let peers = network_state.peer_store().peers_to_feeler(count);
        let mut p2p_control = self.p2p_control.clone();
        for (peer_id, addr) in peers
            .into_iter()
            .filter(|(peer_id, _addr)| network_state.local_peer_id() != peer_id)
        {
            debug!(target: "network", "dial feeler peer: {:?}", addr);
            network_state.dial_feeler(&mut p2p_control, &peer_id, addr);
        }
    }
}

impl BackgroundService for OutboundPeerService {
    fn poll(&mut self, network_state: &mut NetworkState) -> Result<bool, ()> {
        match self.stream_interval.poll().map_err(|_| ()) {
            Ok(Async::Ready(Some(_tick))) => {
                let connection_status = network_state.connection_status();
                let new_outbound = (connection_status.max_outbound
                    - connection_status.unreserved_outbound)
                    as usize;
                if new_outbound > 0 {
                    // dial peers
                    self.attempt_dial_peers(network_state, new_outbound as u32);
                } else {
                    // feeler peers
                    self.feeler_peers(network_state, FEELER_CONNECTION_COUNT);
                }
                Ok(true)
            }
            Ok(Async::Ready(None)) => {
                debug!(target: "network", "ckb outbound peer service stopped");
                Ok(false)
            }
            Ok(Async::NotReady) => Ok(false),
            Err(err) => {
                warn!(target: "network", "ckb outbound peer service stopped error: {:?}", err);
                Err(())
            }
        }
    }
}
