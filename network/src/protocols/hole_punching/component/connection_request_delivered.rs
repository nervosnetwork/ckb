use std::{borrow::Cow, net::SocketAddr};

use ckb_logger::debug;
use ckb_systemtime::unix_time_as_millis;
use ckb_types::{packed, prelude::*};
use futures::future::select_ok;
use p2p::{
    multiaddr::{Multiaddr, Protocol},
    runtime,
    service::{RawSessionInfo, ServiceAsyncControl, TargetProtocol},
    utils::{TransportType, extract_peer_id, find_type},
};

use crate::{
    PeerId, PeerIndex,
    protocols::{
        SupportProtocols,
        hole_punching::{
            ADDRS_COUNT_LIMIT, HolePunching, MAX_HOPS,
            component::{forward_delivered, init_sync, try_nat_traversal},
            status::{Status, StatusCode},
        },
    },
};

struct DeliverdContent {
    from: PeerId,
    to: PeerId,
    route: Vec<PeerId>,
    listen_addrs: Vec<Multiaddr>,
    sync_route: Vec<PeerId>,
}

impl TryFrom<&packed::ConnectionRequestDeliveredReader<'_>> for DeliverdContent {
    type Error = Status;

    fn try_from(value: &packed::ConnectionRequestDeliveredReader<'_>) -> Result<Self, Self::Error> {
        let from = PeerId::from_bytes(value.from().raw_data().to_vec()).map_err(|_| {
            StatusCode::InvalidFromPeerId.with_context("the from peer id is invalid")
        })?;
        let to = PeerId::from_bytes(value.to().raw_data().to_vec())
            .map_err(|_| StatusCode::InvalidToPeerId.with_context("the to peer id is invalid"))?;
        let route = value
            .route()
            .iter()
            .map(|peer_id| {
                PeerId::from_bytes(peer_id.raw_data().to_vec())
                    .map_err(|_| StatusCode::InvalidRoute)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let listen_addrs = value
            .listen_addrs()
            .iter()
            .map(
                |raw| match Multiaddr::try_from(raw.bytes().raw_data().to_vec()) {
                    Ok(mut addr) => {
                        if let Some(peer_id) = extract_peer_id(&addr) {
                            if peer_id != to {
                                return Err(StatusCode::InvalidListenAddrLen
                                    .with_context("peer id in listen address is invalid"));
                            }
                        } else {
                            addr.push(Protocol::P2P(Cow::Borrowed(to.as_bytes())));
                        }
                        Ok(addr)
                    }
                    Err(_) => Err(StatusCode::InvalidListenAddrLen
                        .with_context("the listen address is invalid")),
                },
            )
            .collect::<Result<Vec<_>, _>>()?;

        let sync_route = value
            .sync_route()
            .iter()
            .map(|peer_id| {
                PeerId::from_bytes(peer_id.raw_data().to_vec())
                    .map_err(|_| StatusCode::InvalidRoute)
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(DeliverdContent {
            from,
            to,
            route,
            listen_addrs,
            sync_route,
        })
    }
}

pub struct ConnectionRequestDeliveredProcess<'a> {
    message: packed::ConnectionRequestDeliveredReader<'a>,
    protocol: &'a HolePunching,
    p2p_control: &'a ServiceAsyncControl,
    peer: PeerIndex,
    bind_addr: Option<SocketAddr>,
    msg_item_id: u32,
}

impl<'a> ConnectionRequestDeliveredProcess<'a> {
    pub(crate) fn new(
        message: packed::ConnectionRequestDeliveredReader<'a>,
        protocol: &'a HolePunching,
        p2p_control: &'a ServiceAsyncControl,
        peer: PeerIndex,
        bind_addr: Option<SocketAddr>,
        msg_item_id: u32,
    ) -> Self {
        Self {
            message,
            protocol,
            p2p_control,
            bind_addr,
            peer,
            msg_item_id,
        }
    }

    pub(crate) async fn execute(self) -> Status {
        let content = match DeliverdContent::try_from(&self.message) {
            Ok(content) => content,
            Err(status) => return status,
        };
        if content.listen_addrs.len() > ADDRS_COUNT_LIMIT || content.listen_addrs.is_empty() {
            return StatusCode::InvalidListenAddrLen
                .with_context("the listen address count is too large or empty");
        }

        if content.route.len() > MAX_HOPS as usize || content.sync_route.len() > MAX_HOPS as usize {
            return StatusCode::InvalidRoute.with_context("the route length is too long");
        }

        if self
            .protocol
            .forward_rate_limiter
            .check_key(&(content.from.clone(), content.to.clone(), self.msg_item_id))
            .is_err()
        {
            debug!(
                "from: {}, to {}, item_name: {}, rate limit is reached",
                content.from, content.to, "ConnectionRequestDelivered",
            );
            return StatusCode::TooManyRequests.with_context("ConnectionRequestDelivered");
        }

        match content.route.last() {
            Some(next_peer_id) => self.forward_delivered(next_peer_id).await,
            None => {
                let self_peer_id = self.protocol.network_state.local_peer_id();
                if self_peer_id != &content.from {
                    // forward the message to the `from` peer
                    self.forward_delivered(&content.from).await
                } else {
                    // the current peer is the target peer, respond the sync back
                    let request_start = self.protocol.inflight_requests.write().remove(&content.to);

                    match request_start {
                        Some(start) => {
                            let res = self.respond_sync(content.from).await;
                            if !res.is_ok() {
                                return res;
                            }
                            let now = unix_time_as_millis();
                            let ttl = now - start;

                            self.try_nat_traversal(ttl, content.listen_addrs);

                            Status::ok()
                        }
                        None => StatusCode::Ignore.with_context("the request is not in flight"),
                    }
                }
            }
        }
    }

    async fn forward_delivered(&self, peer_id: &PeerId) -> Status {
        let target_sid = self
            .protocol
            .network_state
            .peer_registry
            .read()
            .get_key_by_peer_id(peer_id);
        match target_sid {
            Some(next_peer) => {
                let content = forward_delivered(self.message);
                let new_message = packed::HolePunchingMessage::new_builder()
                    .set(content)
                    .build()
                    .as_bytes();
                let proto_id = SupportProtocols::HolePunching.protocol_id();
                debug!(
                    "forward the delivery to next peer {} (id: {})",
                    next_peer, peer_id
                );
                if let Err(error) = self
                    .p2p_control
                    .send_message_to(next_peer, proto_id, new_message)
                    .await
                {
                    StatusCode::ForwardError.with_context(error)
                } else {
                    Status::ok()
                }
            }
            None => StatusCode::Ignore.with_context("the next peer in the route is disconnected"),
        }
    }

    async fn respond_sync(&self, from_peer_id: PeerId) -> Status {
        let content = init_sync(self.message);
        let new_message = packed::HolePunchingMessage::new_builder()
            .set(content)
            .build()
            .as_bytes();
        let proto_id = SupportProtocols::HolePunching.protocol_id();
        debug!(
            "current peer is the target peer {}, respond the sync back",
            from_peer_id
        );
        if let Err(error) = self
            .p2p_control
            .send_message_to(self.peer, proto_id, new_message)
            .await
        {
            StatusCode::ForwardError.with_context(error)
        } else {
            Status::ok()
        }
    }

    fn try_nat_traversal(&self, ttl: u64, remote_addrs: Vec<Multiaddr>) {
        let tasks = remote_addrs
            .into_iter()
            .filter_map(|listen_addr| match find_type(&listen_addr) {
                TransportType::Tcp => {
                    if listen_addr
                        .iter()
                        .any(|p| matches!(p, Protocol::Ip4(_) | Protocol::Ip6(_)))
                    {
                        Some(Box::pin(try_nat_traversal(self.bind_addr, listen_addr)))
                    } else {
                        None
                    }
                }
                TransportType::Memory
                | TransportType::Onion
                | TransportType::Ws
                | TransportType::Wss
                | TransportType::Tls => None,
            })
            .collect::<Vec<_>>();

        if tasks.is_empty() {
            return;
        }

        debug!("start NAT traversal");

        let control = self.p2p_control.clone();

        runtime::spawn(async move {
            runtime::delay_for(std::time::Duration::from_millis(ttl / 2)).await;
            if let Ok(((stream, addr), _)) = select_ok(tasks).await {
                debug!("NAT traversal success, addr: {:?}", addr);
                let _ignore = control
                    .raw_session(
                        stream,
                        addr,
                        RawSessionInfo::outbound(TargetProtocol::Single(
                            SupportProtocols::Identify.protocol_id(),
                        )),
                    )
                    .await;
            }
        });
    }
}
