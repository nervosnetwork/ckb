use crate::{
    NetworkState,
    network::TransportType,
    peer_store::{PeerStore, types::AddrInfo},
};
use ckb_logger::trace;
use ckb_systemtime::unix_time_as_millis;
use futures::{Future, StreamExt};
use p2p::runtime::{Interval, MissedTickBehavior};
use p2p::{
    multiaddr::{Multiaddr, Protocol},
    service::ServiceControl,
};
use rand::prelude::IteratorRandom;
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

const FEELER_CONNECTION_COUNT: usize = 10;

/// Ensure that the outbound of the current node reaches the expected upper limit as much as possible
/// Periodically detect and verify data in the peer store
/// Keep the whitelist nodes connected as much as possible
/// Periodically detection finds that the observed addresses are all valid
///
/// A single service instance handles all dialable transports. The dial budget
/// of each tick (outbound slots, feeler count) is shared across transports so
/// that enabling additional transports (e.g. QUIC) does not multiply the
/// number of dial attempts.
pub struct OutboundPeerService {
    network_state: Arc<NetworkState>,
    p2p_control: ServiceControl,
    interval: Option<Interval>,
    try_connect_interval: Duration,
    try_identify_count: u8,
    /// Transports this node can dial, primary transport first.
    transport_types: Vec<TransportType>,
    update_outbound_connected_count: u8,
}

impl OutboundPeerService {
    pub fn new(
        network_state: Arc<NetworkState>,
        p2p_control: ServiceControl,
        try_connect_interval: Duration,
        transport_types: Vec<TransportType>,
    ) -> Self {
        debug_assert!(!transport_types.is_empty());
        OutboundPeerService {
            network_state,
            p2p_control,
            interval: None,
            try_connect_interval,
            try_identify_count: 0,
            update_outbound_connected_count: 0,
            transport_types,
        }
    }

    fn primary_transport_type(&self) -> TransportType {
        self.transport_types
            .first()
            .copied()
            .unwrap_or(TransportType::Tcp)
    }

    /// Whether the given peer address is dialable by the given transport.
    ///
    /// A QUIC address always carries an explicit `quic-v1` protocol component,
    /// so it is only matched by the QUIC transport. All TCP-based transports
    /// (TCP/WS/WSS) explicitly exclude QUIC addresses to avoid dialing a UDP
    /// address over a TCP-based transport.
    fn addr_matches_transport(transport_type: TransportType, peer_addr: &AddrInfo) -> bool {
        let is_quic = peer_addr.addr.iter().any(|p| matches!(p, Protocol::QuicV1));
        match transport_type {
            TransportType::Tcp => !is_quic,
            TransportType::Ws => {
                !is_quic
                    && peer_addr.addr.iter().any(|p| {
                        matches!(p, Protocol::Dns4(_) | Protocol::Dns6(_) | Protocol::Tcp(_))
                    })
            }
            TransportType::Wss => {
                !is_quic
                    && peer_addr
                        .addr
                        .iter()
                        .any(|p| matches!(p, Protocol::Dns4(_) | Protocol::Dns6(_)))
            }
            TransportType::QuicV1 => is_quic,
        }
    }

    /// Complete a bare peer address with the transport-specific protocol
    /// component so that p2p dials it over the intended transport. QUIC and TCP
    /// addresses already carry their transport information, so nothing is added.
    fn complete_dial_addr(transport_type: TransportType, mut addr: Multiaddr) -> Multiaddr {
        match transport_type {
            TransportType::Tcp | TransportType::QuicV1 => (),
            TransportType::Ws => addr.push(Protocol::Ws),
            TransportType::Wss => addr.push(Protocol::Wss),
        }
        addr
    }

    /// Fetch up to `count` addresses from the peer store, sharing the budget
    /// across all dialable transports (primary transport first). The returned
    /// addresses are already completed for their matching transport, and are
    /// marked as tried in the peer store.
    fn fetch_dial_addrs<F>(&self, count: usize, mut fetch: F) -> Vec<Multiaddr>
    where
        F: FnMut(&mut PeerStore, usize, TransportType) -> Vec<AddrInfo>,
    {
        let now_ms = unix_time_as_millis();
        let mut remain = count;
        let mut addrs = Vec::with_capacity(count);
        for &transport_type in &self.transport_types {
            if remain == 0 {
                break;
            }
            let paddrs = self.network_state.with_peer_store_mut(|peer_store| {
                let paddrs = fetch(peer_store, remain, transport_type);
                for paddr in paddrs.iter() {
                    // mark addr as tried
                    if let Some(paddr) = peer_store.mut_addr_manager().get_mut(&paddr.addr) {
                        paddr.mark_tried(now_ms);
                    }
                }
                paddrs
            });
            remain = remain.saturating_sub(paddrs.len());
            addrs.extend(
                paddrs
                    .into_iter()
                    .map(|info| Self::complete_dial_addr(transport_type, info.addr)),
            );
        }
        addrs
    }

    fn dial_feeler(&mut self) {
        let attempt_addrs = self.fetch_dial_addrs(
            FEELER_CONNECTION_COUNT,
            |peer_store, count, transport_type| {
                peer_store.fetch_addrs_to_feeler(count, |peer_addr: &AddrInfo| {
                    Self::addr_matches_transport(transport_type, peer_addr)
                })
            },
        );

        trace!(
            "feeler dial count={}, attempt_addrs: {:?}",
            attempt_addrs.len(),
            attempt_addrs,
        );

        for addr in attempt_addrs {
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

        let required_flags = self.network_state.required_flags;
        let fetch = |peer_store: &mut PeerStore, number: usize, transport_type: TransportType| {
            peer_store.fetch_addrs_to_attempt(number, required_flags, |peer_addr: &AddrInfo| {
                Self::addr_matches_transport(transport_type, peer_addr)
            })
        };

        let peers: Vec<Multiaddr> = if self.try_identify_count > 3 {
            self.try_identify_count = 0;
            let len = self.network_state.bootnodes.len();
            if len < count {
                let mut peers = self.fetch_dial_addrs(count - len, fetch);
                // Bootnode addresses already carry their transport information.
                peers.extend(self.network_state.bootnodes.iter().cloned());
                peers
            } else {
                self.network_state
                    .bootnodes
                    .iter()
                    .choose_multiple(&mut rand::thread_rng(), count)
                    .into_iter()
                    .cloned()
                    .collect()
            }
        } else {
            self.fetch_dial_addrs(count, fetch)
        };

        trace!(
            "identify dial count={}, attempt_peers: {:?}",
            peers.len(),
            peers
        );

        for addr in peers {
            self.network_state.dial_identify(&self.p2p_control, addr);
        }
    }

    fn try_dial_whitelist(&self) {
        // Whitelist addresses already determine their own transport (a QUIC
        // whitelist address carries `/quic-v1`, a WSS one carries `/wss`, ...);
        // only bare TCP addresses need completion for WS/WSS-primary nodes.
        // Dial each address exactly once to avoid duplicate dial attempts.
        let primary = self.primary_transport_type();
        for addr in self.network_state.config.whitelist_peers() {
            let addr = if addr.iter().any(|p| {
                matches!(
                    p,
                    Protocol::QuicV1 | Protocol::Ws | Protocol::Wss | Protocol::Onion3(_)
                )
            }) {
                addr
            } else {
                Self::complete_dial_addr(primary, addr)
            };
            self.network_state.dial_identify(&self.p2p_control, addr);
        }
    }

    fn update_outbound_connected_ms(&mut self) {
        if self.update_outbound_connected_count > 10 {
            let connected_outbounds: Vec<p2p::multiaddr::Multiaddr> =
                self.network_state.with_peer_registry(|re| {
                    re.peers()
                        .values()
                        .filter_map(|p| {
                            if p.is_outbound() {
                                Some(p.connected_addr.clone())
                            } else {
                                None
                            }
                        })
                        .collect()
                });

            self.network_state.with_peer_store_mut(|p| {
                for addr in connected_outbounds {
                    p.update_outbound_addr_last_connected_ms(addr)
                }
            });
            self.update_outbound_connected_count = 0;
        } else {
            self.update_outbound_connected_count += 1;
        }
    }
}

impl Future for OutboundPeerService {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.interval.is_none() {
            self.interval = {
                let mut interval =
                    Interval::new_at(self.try_connect_interval, self.try_connect_interval);
                // The outbound service does not need to urgently compensate for the missed wake,
                // just skip behavior is enough
                interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
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
            // keep whitelist peer on connected
            self.try_dial_whitelist();
            // ensure feeler work at any time
            self.dial_feeler();
            // keep outbound peer is enough
            self.try_dial_peers();
            // Keep connected nodes up to date in the peer store
            self.update_outbound_connected_ms();
        }
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::{AddrInfo, OutboundPeerService, TransportType};
    use p2p::multiaddr::Multiaddr;

    fn addr_info(addr: &str) -> AddrInfo {
        AddrInfo::new(addr.parse().unwrap(), 0, 0, 0)
    }

    #[test]
    fn transport_matching() {
        let tcp = addr_info("/ip4/127.0.0.1/tcp/8115");
        let dns_tcp = addr_info("/dns4/example.com/tcp/8115");
        let quic = addr_info("/ip4/127.0.0.1/udp/8115/quic-v1");
        let dns_quic = addr_info("/dns4/example.com/udp/8115/quic-v1");

        let matches = OutboundPeerService::addr_matches_transport;

        assert!(matches(TransportType::Tcp, &tcp));
        assert!(matches(TransportType::Ws, &tcp));
        assert!(!matches(TransportType::QuicV1, &tcp));

        assert!(matches(TransportType::QuicV1, &quic));
        assert!(!matches(TransportType::Tcp, &quic));
        assert!(!matches(TransportType::Ws, &quic));
        assert!(!matches(TransportType::Wss, &quic));

        // A DNS-based QUIC address contains a Dns4 component; it must still
        // only be matched by the QUIC transport, never by WS/WSS (which would
        // append `/ws` to it and produce an undialable address).
        assert!(matches(TransportType::QuicV1, &dns_quic));
        assert!(!matches(TransportType::Ws, &dns_quic));
        assert!(!matches(TransportType::Wss, &dns_quic));

        assert!(matches(TransportType::Ws, &dns_tcp));
        assert!(matches(TransportType::Wss, &dns_tcp));
    }

    #[test]
    fn complete_dial_addr_keeps_quic_untouched() {
        let quic: Multiaddr = "/ip4/127.0.0.1/udp/8115/quic-v1".parse().unwrap();
        assert_eq!(
            OutboundPeerService::complete_dial_addr(TransportType::QuicV1, quic.clone()),
            quic
        );

        let tcp: Multiaddr = "/ip4/127.0.0.1/tcp/8115".parse().unwrap();
        let ws: Multiaddr = "/ip4/127.0.0.1/tcp/8115/ws".parse().unwrap();
        assert_eq!(
            OutboundPeerService::complete_dial_addr(TransportType::Ws, tcp),
            ws
        );
    }
}
