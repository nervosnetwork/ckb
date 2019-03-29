use crate::NetworkState;
use log::debug;
use std::sync::Arc;
use std::time::Duration;

use p2p::{context::ProtocolContext, traits::ServiceProtocol};

const OUTBOUND_INTERVAL_TOKEN: u64 = 0;

pub(crate) struct OutboundPeerProtocol {
    // Try connect interval
    interval: Duration,
    network_state: Arc<NetworkState>,
}

impl OutboundPeerProtocol {
    pub(crate) fn new(network_state: Arc<NetworkState>, interval: Duration) -> Self {
        debug!(target: "network", "outbound peer service start, interval: {:?}", interval);
        OutboundPeerProtocol {
            network_state,
            interval,
        }
    }
}

impl ServiceProtocol for OutboundPeerProtocol {
    fn init(&mut self, context: &mut ProtocolContext) {
        let proto_id = context.proto_id;
        context.set_service_notify(proto_id, self.interval, OUTBOUND_INTERVAL_TOKEN);
    }

    fn notify(&mut self, context: &mut ProtocolContext, _token: u64) {
        let connection_status = self.network_state.connection_status();
        let new_outbound =
            (connection_status.max_outbound - connection_status.unreserved_outbound) as usize;
        if new_outbound > 0 {
            let attempt_peers = self
                .network_state
                .peer_store()
                .read()
                .peers_to_attempt(new_outbound as u32);
            for (peer_id, addr) in attempt_peers
                .into_iter()
                .filter(|(peer_id, _addr)| self.network_state.local_peer_id() != peer_id)
            {
                self.network_state.dial(context.control(), &peer_id, addr);
            }
        }
    }
}
