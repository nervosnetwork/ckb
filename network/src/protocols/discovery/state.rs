use std::time::{Duration, Instant};

use ckb_logger::debug;
use p2p::{
    bytes::BytesMut,
    context::{ProtocolContext, ProtocolContextMutRef},
    multiaddr::{Multiaddr, Protocol},
    utils::multiaddr_to_socketaddr,
    SessionId,
};

use tokio_util::codec::Encoder;

use super::{
    addr::{AddrKnown, RawAddr, DEFAULT_MAX_KNOWN},
    protocol::{DiscoveryCodec, DiscoveryMessage, Node, Nodes},
    MAX_ADDR_TO_SEND,
};

// FIXME: should be a more high level version number

const VERSION: u32 = 0;

pub struct SessionState {
    // received pending messages
    pub(crate) addr_known: AddrKnown<RawAddr>,
    // FIXME: Remote listen address, resolved by id protocol
    pub(crate) remote_addr: RemoteAddress,
    pub(crate) announce: bool,
    pub(crate) last_announce: Option<Instant>,
    pub(crate) announce_multiaddrs: Vec<Multiaddr>,
    pub(crate) received_get_nodes: bool,
    pub(crate) received_nodes: bool,
}

impl SessionState {
    pub(crate) fn new(context: ProtocolContextMutRef, codec: &mut DiscoveryCodec) -> SessionState {
        let mut addr_known = AddrKnown::new(DEFAULT_MAX_KNOWN);
        let remote_addr = if context.session.ty.is_outbound() {
            let port = context
                .listens()
                .iter()
                .filter_map(|address| multiaddr_to_socketaddr(address))
                .map(|addr| addr.port())
                .next();

            let mut msg = BytesMut::new();
            codec
                .encode(
                    DiscoveryMessage::GetNodes {
                        version: VERSION,
                        count: MAX_ADDR_TO_SEND as u32,
                        listen_port: port,
                    },
                    &mut msg,
                )
                .expect("encode must be success");

            if context.send_message(msg.freeze()).is_err() {
                debug!("{:?} send discovery msg GetNode fail", context.session.id)
            }

            if let Some(addr) = multiaddr_to_socketaddr(&context.session.address) {
                addr_known.insert(RawAddr::from(addr));
            }

            RemoteAddress::Listen(context.session.address.clone())
        } else {
            RemoteAddress::Init(context.session.address.clone())
        };

        SessionState {
            last_announce: None,
            addr_known,
            remote_addr,
            announce: false,
            announce_multiaddrs: Vec::new(),
            received_get_nodes: false,
            received_nodes: false,
        }
    }

    pub(crate) fn remote_raw_addr(&self) -> Option<RawAddr> {
        multiaddr_to_socketaddr(self.remote_addr.to_inner()).map(RawAddr::from)
    }

    pub(crate) fn check_timer(&mut self, now: Instant, interval: Duration) {
        if self
            .last_announce
            .map(|time| now - time > interval)
            .unwrap_or(true)
        {
            self.announce = true;
        }
    }

    pub(crate) fn send_messages(
        &mut self,
        cx: &mut ProtocolContext,
        id: SessionId,
        codec: &mut DiscoveryCodec,
    ) {
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
            let mut msg = BytesMut::new();
            codec
                .encode(DiscoveryMessage::Nodes(nodes), &mut msg)
                .expect("encode must be success");
            if cx.send_message_to(id, cx.proto_id, msg.freeze()).is_err() {
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
    fn to_inner(&self) -> &Multiaddr {
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
