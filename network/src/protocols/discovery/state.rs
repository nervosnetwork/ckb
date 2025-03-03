use ckb_systemtime::{Duration, Instant};

use ckb_logger::debug;
use p2p::{
    context::{ProtocolContext, ProtocolContextMutRef},
    multiaddr::{Multiaddr, Protocol},
    utils::multiaddr_to_socketaddr,
    SessionId,
};

use crate::Flags;

use super::{
    addr::AddrKnown,
    protocol::{encode, DiscoveryMessage, Node, Nodes},
    AddressManager, MAX_ADDR_TO_SEND,
};

// FIXME: should be a more high level version number

// default
#[allow(dead_code)]
const FIRST_VERSION: u32 = 0;
// enable reuse port
#[allow(dead_code)]
pub const REUSE_PORT_VERSION: u32 = 1;

pub struct SessionState {
    // received pending messages
    pub(crate) addr_known: AddrKnown,
    // FIXME: Remote listen address, resolved by id protocol
    pub(crate) remote_addr: RemoteAddress,
    last_announce: Option<Instant>,
    pub(crate) announce_multiaddrs: Vec<(Multiaddr, Flags)>,
    pub(crate) received_get_nodes: bool,
    pub(crate) received_nodes: bool,
}

impl SessionState {
    pub(crate) async fn new<M: AddressManager + Send>(
        context: ProtocolContextMutRef<'_>,
        addr_manager: &M,
    ) -> SessionState {
        let mut addr_known = AddrKnown::default();
        let remote_addr = if context.session.ty.is_outbound() {
            let port = context
                .listens()
                .iter()
                .flat_map(|address| {
                    // Verify self is a public node first
                    // if not, try to make public network nodes broadcast hole punching information
                    if addr_manager.is_valid_addr(address) {
                        multiaddr_to_socketaddr(address).map(|socket_addr| socket_addr.port())
                    } else {
                        None
                    }
                })
                .next();

            let msg = encode(DiscoveryMessage::GetNodes {
                #[cfg(target_os = "linux")]
                version: REUSE_PORT_VERSION,
                #[cfg(not(target_os = "linux"))]
                version: FIRST_VERSION,
                count: MAX_ADDR_TO_SEND as u32,
                listen_port: port,
                required_flags: addr_manager.required_flags(),
            });

            if context.send_message(msg).await.is_err() {
                debug!(
                    "{:?} sending discovery msg GetNode failed",
                    context.session.id
                )
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
            .map(|time| now.saturating_duration_since(time) > interval)
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

    pub(crate) async fn send_messages(&mut self, cx: &mut ProtocolContext, id: SessionId) {
        if !self.announce_multiaddrs.is_empty() {
            let items = self
                .announce_multiaddrs
                .drain(..)
                .map(|addr| Node {
                    addresses: vec![addr.0],
                    flags: addr.1,
                })
                .collect::<Vec<_>>();
            let msg = encode(DiscoveryMessage::Nodes(Nodes {
                announce: true,
                items,
            }));
            if cx.send_message_to(id, cx.proto_id, msg).await.is_err() {
                debug!("{:?} sending discovery msg Nodes failed", id)
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
            RemoteAddress::Init(addr) | RemoteAddress::Listen(addr) => addr,
        }
    }

    pub(crate) fn change_to_listen(&mut self) {
        if let RemoteAddress::Init(addr) = self {
            *self = RemoteAddress::Listen(addr.clone());
        }
    }

    pub(crate) fn update_port(&mut self, port: u16) {
        if let RemoteAddress::Init(addr) = self {
            let addr = addr
                .into_iter()
                .map(|proto| {
                    match proto {
                        // TODO: other transport, UDP for example
                        Protocol::Tcp(_) => Protocol::Tcp(port),
                        value => value,
                    }
                })
                .collect();
            *self = RemoteAddress::Listen(addr);
        }
    }
}
