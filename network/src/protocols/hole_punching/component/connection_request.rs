use std::{borrow::Cow, net::SocketAddr, time::Duration};

use ckb_logger::debug;
use ckb_systemtime::Instant;
use ckb_types::{packed, prelude::*};
use futures::future::select_ok;
use p2p::{
    multiaddr::{Multiaddr, Protocol},
    runtime,
    service::{RawSessionInfo, ServiceAsyncControl, TargetProtocol, TargetSession},
    utils::{TransportType, extract_peer_id, find_type, socketaddr_to_multiaddr},
};

use crate::{
    PeerId, PeerIndex,
    protocols::{
        SupportProtocols,
        hole_punching::{
            ADDRS_COUNT_LIMIT, HolePunching, MAX_TTL, PENETRATED_INTERVAL,
            component::try_nat_traversal,
            status::{Status, StatusCode},
        },
    },
};

pub(crate) struct ConnectionRequestProcess<'a> {
    message: packed::ConnectionRequestReader<'a>,
    protocol: &'a HolePunching,
    peer: PeerIndex,
    p2p_control: &'a ServiceAsyncControl,
    bind_addr: Option<SocketAddr>,
}

impl<'a> ConnectionRequestProcess<'a> {
    pub(crate) fn new(
        message: packed::ConnectionRequestReader<'a>,
        protocol: &'a HolePunching,
        peer: PeerIndex,
        p2p_control: &'a ServiceAsyncControl,
        bind_addr: Option<SocketAddr>,
    ) -> Self {
        Self {
            message,
            protocol,
            peer,
            p2p_control,
            bind_addr,
        }
    }

    pub(crate) async fn execute(self) -> Status {
        if self.message.listen_addrs().len() > ADDRS_COUNT_LIMIT {
            return StatusCode::InvalidListenAddrLen
                .with_context("the listen address count is too large");
        }
        let ttl: u8 = self.message.ttl().into();
        if ttl > MAX_TTL {
            return StatusCode::InvalidMaxTTL.into();
        }

        let self_peer_id = self.protocol.network_state.local_peer_id();
        for peer_id_bytes in self.message.route().iter() {
            match PeerId::from_bytes(peer_id_bytes.raw_data().to_vec()) {
                Ok(peer_id) => {
                    if self_peer_id == &peer_id {
                        return StatusCode::Ignore.with_context("the message is passed, ignore it");
                    }
                }
                Err(_) => {
                    return StatusCode::InvalidRoute.into();
                }
            }
        }

        let from_peer_id = match PeerId::from_bytes(self.message.from().raw_data().to_vec()) {
            Ok(peer_id) => {
                if self_peer_id == &peer_id {
                    return StatusCode::Ignore.with_context("the message is passed, ignore it");
                }
                peer_id
            }
            Err(_) => {
                return StatusCode::InvalidFromPeerId.into();
            }
        };
        let to_peer_id = match PeerId::from_bytes(self.message.to().raw_data().to_vec()) {
            Ok(peer_id) => peer_id,
            Err(_) => {
                return StatusCode::InvalidToPeerId.into();
            }
        };

        if self_peer_id == &to_peer_id {
            if let Some(t) = self.protocol.finished.read().unwrap().get(&from_peer_id) {
                if t.elapsed() < PENETRATED_INTERVAL {
                    return StatusCode::Ignore
                        .with_context("a same message is already replied in a moment ago");
                }
            }
            let listen_addrs = {
                let public_addr = self.protocol.network_state.public_addrs(ADDRS_COUNT_LIMIT);
                if public_addr.len() < ADDRS_COUNT_LIMIT {
                    let observed_addrs = self
                        .protocol
                        .network_state
                        .observed_addrs(ADDRS_COUNT_LIMIT - public_addr.len());
                    let iter = public_addr
                        .iter()
                        .chain(observed_addrs.iter())
                        .map(Multiaddr::to_vec)
                        .map(|v| packed::Address::new_builder().bytes(v.pack()).build());
                    packed::AddressVec::new_builder().extend(iter).build()
                } else {
                    let iter = public_addr
                        .iter()
                        .map(Multiaddr::to_vec)
                        .map(|v| packed::Address::new_builder().bytes(v.pack()).build());
                    packed::AddressVec::new_builder().extend(iter).build()
                }
            };
            let message = self.message.to_entity();
            let new_route = packed::BytesVec::new_builder()
                .extend(message.route().into_iter().take(message.route().len() - 1))
                .build();
            let content = packed::ConnectionRequestDelivered::new_builder()
                .from(message.from())
                .to(message.to())
                .route(new_route)
                .listen_addrs(listen_addrs)
                .build();
            let new_message = packed::HolePunchingMessage::new_builder()
                .set(content)
                .build()
                .as_bytes();
            let proto_id = SupportProtocols::HolePunching.protocol_id();

            debug!(
                "current peer is the target peer {}, send a response back",
                to_peer_id
            );

            if let Err(error) = self
                .p2p_control
                .send_message_to(self.peer, proto_id, new_message)
                .await
            {
                return StatusCode::ForwardError.with_context(error);
            }

            // drop the lock here
            {
                let mut finished = self.protocol.finished.write().unwrap();
                let now = Instant::now();
                finished.retain(|_, t| (now - *t) < Duration::from_secs(5 * 60));
                finished.insert(from_peer_id.clone(), Instant::now());
            }

            let mut tasks = Vec::new();
            let control: ServiceAsyncControl = self.p2p_control.clone();
            for listen_addr in self.message.listen_addrs().iter() {
                match Multiaddr::try_from(listen_addr.bytes().raw_data().to_vec()) {
                    Ok(mut addr) => {
                        if let Some(peer_id) = extract_peer_id(&addr) {
                            if peer_id != from_peer_id {
                                continue;
                            }
                        } else {
                            addr.push(Protocol::P2P(Cow::Borrowed(from_peer_id.as_bytes())));
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

            let listen_addr = self
                .protocol
                .network_state
                .config
                .listen_addresses
                .first()
                .cloned()
                .unwrap_or(socketaddr_to_multiaddr(SocketAddr::from((
                    [0, 0, 0, 0],
                    8115,
                ))));
            runtime::spawn(async move {
                if let Ok(((stream, addr), _)) = select_ok(tasks).await {
                    let _ignore = control
                        .raw_session(stream, addr, RawSessionInfo::inbound(listen_addr))
                        .await;
                }
            });
            Status::ok()
        } else if ttl == 0u8 {
            StatusCode::ReachedMaxTTL.into()
        } else {
            let message = self.message.to_entity();
            let new_route = message
                .route()
                .as_builder()
                .push(self_peer_id.as_bytes().pack())
                .build();
            let content = message
                .as_builder()
                .ttl((ttl - 1).into())
                .route(new_route)
                .build();
            let new_message = packed::HolePunchingMessage::new_builder()
                .set(content)
                .build()
                .as_bytes();
            let proto_id = SupportProtocols::HolePunching.protocol_id();

            let target_sid = self
                .protocol
                .network_state
                .peer_registry
                .read()
                .get_key_by_peer_id(&to_peer_id);

            match target_sid {
                Some(to_peer) => {
                    debug!(
                        "target peer {} is found, forward the request to it",
                        to_peer_id
                    );
                    if let Err(error) = self
                        .p2p_control
                        .send_message_to(to_peer, proto_id, new_message)
                        .await
                    {
                        StatusCode::ForwardError.with_context(error)
                    } else {
                        Status::ok()
                    }
                }
                None => {
                    debug!(
                        "target peer {} is not found, broadcast the request to more peers",
                        to_peer_id
                    );
                    let sid = self.peer;
                    if let Err(error) = self
                        .p2p_control
                        .filter_broadcast(
                            TargetSession::Filter(Box::new(move |id| id != &sid)),
                            proto_id,
                            new_message,
                        )
                        .await
                    {
                        StatusCode::BroadcastError.with_context(error)
                    } else {
                        Status::ok()
                    }
                }
            }
        }
    }
}
