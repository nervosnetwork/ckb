use std::time::{Duration, Instant};

use ckb_logger::debug;
use p2p::{
    context::{ProtocolContext, ProtocolContextMutRef},
    multiaddr::{Multiaddr, Protocol},
    utils::multiaddr_to_socketaddr,
    SessionId,
};

use super::{
    addr::AddrKnown,
    protocol::{encode, DiscoveryMessage, Node, Nodes},
    MAX_ADDR_TO_SEND,
};

// FIXME: should be a more high level version number

const VERSION: u32 = 0;

pub struct SessionState {
    // received pending messages
    pub(crate) addr_known: AddrKnown,
    // FIXME: Remote listen address, resolved by id protocol
    pub(crate) remote_addr: RemoteAddress,
    last_announce: Option<Instant>,
    pub(crate) announce_multiaddrs: Vec<Multiaddr>,
    pub(crate) received_get_nodes: bool,
    pub(crate) received_nodes: bool,
}

impl SessionState {
    pub(crate) fn new(context: ProtocolContextMutRef) -> SessionState {
        let mut addr_known = AddrKnown::default();
        let remote_addr = if context.session.ty.is_outbound() {
            let port = context
                .listens()
                .iter()
                .filter_map(|address| multiaddr_to_socketaddr(address))
                .map(|addr| addr.port())
                .next();

            let msg = encode(DiscoveryMessage::GetNodes {
                version: VERSION,
                count: MAX_ADDR_TO_SEND as u32,
                listen_port: port,
            });

            if context.send_message(msg).is_err() {
                debug!("{:?} send discovery msg GetNode fail", context.session.id)
            }

            addr_known.insert(&context.session.address);

            RemoteAddress::Listen(context.session.address.clone())
        } else {
            RemoteAddress::Init(context.session.address.clone())
        };

        SessionState {
            last_announce: None,
            addr_known,
            remote_addr,
            announce_multiaddrs: Vec::new(),
            received_get_nodes: false,
            received_nodes: false,
        }
    }

    pub(crate) fn check_timer(&mut self, now: Instant, interval: Duration) -> Option<&Multiaddr> {
        if self
            .last_announce
            .map(|time| now - time > interval)
            .unwrap_or(true)
        {
            self.last_announce = Some(now);
            if let RemoteAddress::Listen(addr) = &self.remote_addr {
                Some(addr)
            } else {
                None
            }
        } else {
            None
        }
    }

    pub(crate) fn send_messages(&mut self, cx: &mut ProtocolContext, id: SessionId) {
        if !self.announce_multiaddrs.is_empty() {
            let items = self
                .announce_multiaddrs
                .drain(..)
                .map(|addr| Node {
                    addresses: vec![addr],
                })
                .collect::<Vec<_>>();
            let nodes = Nodes {
                announce: true,
                items,
            };
            let msg = encode(DiscoveryMessage::Nodes(nodes));
            if cx.send_message_to(id, cx.proto_id, msg).is_err() {
                debug!("{:?} send discovery msg Nodes fail", id)
            }
        }
    }
}

#[derive(Eq, PartialEq, Hash, Debug, Clone)]
pub(crate) enum RemoteAddress {
    /// Inbound init remote address
    Init(Multiaddr),
    /// Outbound init remote address or Inbound listen address
    Listen(Multiaddr),
}

impl RemoteAddress {
    pub(crate) fn to_inner(&self) -> &Multiaddr {
        match self {
            RemoteAddress::Init(ref addr) | RemoteAddress::Listen(ref addr) => addr,
        }
    }

    pub(crate) fn update_port(&mut self, port: u16) {
        if let RemoteAddress::Init(ref addr) = self {
            let addr = addr
                .into_iter()
                .map(|proto| {
                    match proto {
                        // TODO: other transport, UDP for example
                        Protocol::TCP(_) => Protocol::TCP(port),
                        value => value,
                    }
                })
                .collect();
            *self = RemoteAddress::Listen(addr);
        }
    }
}
