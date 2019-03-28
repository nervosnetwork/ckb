use crate::Network;
use futures::{Async, Stream};
use log::{debug, warn};
use std::sync::Arc;
use std::time::Duration;
use std::usize;
use tokio::timer::Interval;

const FEELER_CONNECTION_COUNT: u32 = 5;

pub struct OutboundPeerService {
    pub stream_interval: Interval,
    pub network: Arc<Network>,
}

impl OutboundPeerService {
    pub fn new(network: Arc<Network>, try_connect_interval: Duration) -> Self {
        debug!(target: "network", "outbound peer service start, interval: {:?}", try_connect_interval);
        OutboundPeerService {
            network,
            stream_interval: Interval::new_interval(try_connect_interval),
        }
    }

    fn attempt_dial_peers(&mut self, count: u32) {
        let attempt_peers = self.network.peer_store().read().peers_to_attempt(count);
        for (peer_id, addr) in attempt_peers
            .into_iter()
            .filter(|(peer_id, _addr)| self.network.local_peer_id() != peer_id)
        {
            let _ = self.network.dial(&peer_id, addr);
        }
    }

    fn feeler_peers(&mut self, count: u32) {
        let peers = self.network.peer_store().read().peers_to_feeler(count);
        for (peer_id, addr) in peers
            .into_iter()
            .filter(|(peer_id, _addr)| self.network.local_peer_id() != peer_id)
        {
            let _ = self.network.dial_feeler(&peer_id, addr);
        }
    }
}

impl Stream for OutboundPeerService {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Result<Async<Option<Self::Item>>, Self::Error> {
        match try_ready!(self.stream_interval.poll().map_err(|_| ())) {
            Some(_tick) => {
                let connection_status = self.network.connection_status();
                let new_outbound = (connection_status.max_outbound
                    - connection_status.unreserved_outbound)
                    as usize;
                if new_outbound > 0 {
                    // dial peers
                    self.attempt_dial_peers(new_outbound as u32);
                } else {
                    // feeler peers
                    self.feeler_peers(FEELER_CONNECTION_COUNT);
                }
            }
            None => {
                warn!(target: "network", "ckb outbound peer service stopped");
                return Ok(Async::Ready(None));
            }
        }
        Ok(Async::Ready(Some(())))
    }
}
