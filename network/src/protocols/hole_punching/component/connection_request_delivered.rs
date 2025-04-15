use std::{borrow::Cow, net::SocketAddr};

use ckb_logger::debug;
use ckb_types::{packed, prelude::*};
use futures::future::select_ok;
use p2p::{
    multiaddr::{Multiaddr, Protocol},
    runtime,
    service::{RawSessionInfo, ServiceAsyncControl, TargetProtocol},
    utils::{TransportType, extract_peer_id, find_type},
};

use crate::{
    PeerId,
    protocols::{
        SupportProtocols,
        hole_punching::{
            ADDRS_COUNT_LIMIT, HolePunching,
            component::try_nat_traversal,
            status::{Status, StatusCode},
        },
    },
};

pub struct ConnectionRequestDeliveredProcess<'a> {
    message: packed::ConnectionRequestDeliveredReader<'a>,
    protocol: &'a HolePunching,
    p2p_control: &'a ServiceAsyncControl,
    bind_addr: Option<SocketAddr>,
}

impl<'a> ConnectionRequestDeliveredProcess<'a> {
    pub(crate) fn new(
        message: packed::ConnectionRequestDeliveredReader<'a>,
        protocol: &'a HolePunching,
        p2p_control: &'a ServiceAsyncControl,
        bind_addr: Option<SocketAddr>,
    ) -> Self {
        Self {
            message,
            protocol,
            p2p_control,
            bind_addr,
        }
    }

    pub(crate) async fn execute(self) -> Status {
        if self.message.listen_addrs().len() > ADDRS_COUNT_LIMIT {
            return StatusCode::InvalidListenAddrLen
                .with_context("the listen address count is too large");
        }
        let route = self.message.route();
        match route.iter().last() {
            Some(next_peer_id_data) => {
                let next_peer_id = match PeerId::from_bytes(next_peer_id_data.raw_data().to_vec()) {
                    Ok(peer_id) => peer_id,
                    Err(_) => {
                        return StatusCode::InvalidRoute
                            .with_context("the last peer id is invalid");
                    }
                };

                let target_sid = self
                    .protocol
                    .network_state
                    .peer_registry
                    .read()
                    .get_key_by_peer_id(&next_peer_id);

                match target_sid {
                    Some(next_peer) => {
                        let message = self.message.to_entity();
                        let new_route = packed::BytesVec::new_builder()
                            .extend(message.route().into_iter().take(route.len() - 1))
                            .build();
                        let content = message.as_builder().route(new_route).build();
                        let new_message = packed::HolePunchingMessage::new_builder()
                            .set(content)
                            .build()
                            .as_bytes();
                        let proto_id = SupportProtocols::HolePunching.protocol_id();
                        debug!(
                            "forward the delivery to next peer {} (id: {})",
                            next_peer, next_peer_id
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
                    None => {
                        return StatusCode::Ignore
                            .with_context("the next peer in the route is disconnected");
                    }
                }
            }
            None => {
                // Current node should be the `from` target.
                let from_peer_id = match PeerId::from_bytes(self.message.from().raw_data().to_vec())
                {
                    Ok(peer_id) => peer_id,
                    Err(_) => return StatusCode::InvalidFromPeerId.into(),
                };

                let self_peer_id = self.protocol.network_state.local_peer_id();
                if self_peer_id != &from_peer_id {
                    return StatusCode::Ignore.with_context("the destination of route is not self");
                }

                let to_peer_id = match PeerId::from_bytes(self.message.to().raw_data().to_vec()) {
                    Ok(peer_id) => peer_id,
                    Err(_) => return StatusCode::InvalidToPeerId.into(),
                };

                let mut tasks = Vec::new();
                let control: ServiceAsyncControl = self.p2p_control.clone();
                for listen_addr in self.message.listen_addrs().iter() {
                    match Multiaddr::try_from(listen_addr.bytes().raw_data().to_vec()) {
                        Ok(mut addr) => {
                            if let Some(peer_id) = extract_peer_id(&addr) {
                                if peer_id != to_peer_id {
                                    continue;
                                }
                            } else {
                                addr.push(Protocol::P2P(Cow::Borrowed(to_peer_id.as_bytes())));
                            }
                            match find_type(&addr) {
                                TransportType::Memory => continue,
                                TransportType::Onion => continue,
                                TransportType::Ws => continue,
                                TransportType::Wss => continue,
                                TransportType::Tls => continue,
                                TransportType::Tcp => {
                                    if addr
                                        .iter()
                                        .any(|p| matches!(p, Protocol::Dns4(_) | Protocol::Dns6(_)))
                                    {
                                        let control = control.clone();
                                        // If the address contains DNS4 or DNS6, we just dial it directly
                                        // without NAT traversal
                                        runtime::spawn(async move {
                                            let _ignore = control
                                                .dial(
                                                    addr,
                                                    TargetProtocol::Single(
                                                        SupportProtocols::Identify.protocol_id(),
                                                    ),
                                                )
                                                .await;
                                        });
                                    } else {
                                        let task = try_nat_traversal(self.bind_addr, addr);
                                        tasks.push(Box::pin(task));
                                    }
                                }
                            }
                        }
                        Err(_) => {
                            continue;
                        }
                    }
                }

                runtime::spawn(async move {
                    if let Ok(((stream, addr), _)) = select_ok(tasks).await {
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
                Status::ok()
            }
        }
    }
}
