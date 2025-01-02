use std::borrow::Cow;

use ckb_logger::debug;
use ckb_systemtime::Instant;
use ckb_types::{packed, prelude::*};
use p2p::{
    multiaddr::{Multiaddr, Protocol},
    service::ServiceControl,
    utils::extract_peer_id,
};

use super::super::{
    Penetration, Status, StatusCode, ADDRS_COUNT_LIMIT, MAX_TTL, PENETRATED_INTERVAL,
};
use crate::{network::ServiceControlExt as _, PeerId, PeerIndex, SupportProtocols};

pub(crate) struct ConnectionRequestProcess<'a> {
    message: packed::ConnectionRequestReader<'a>,
    protocol: &'a Penetration,
    peer: PeerIndex,
    p2p_control: &'a ServiceControl,
}

impl<'a> ConnectionRequestProcess<'a> {
    pub(crate) fn new(
        message: packed::ConnectionRequestReader<'a>,
        protocol: &'a Penetration,
        peer: PeerIndex,
        p2p_control: &'a ServiceControl,
    ) -> Self {
        Self {
            message,
            protocol,
            peer,
            p2p_control,
        }
    }

    pub(crate) fn execute(self) -> Status {
        let ttl: u8 = self.message.ttl().into();
        if ttl > MAX_TTL {
            return StatusCode::InvalidMaxTTL.into();
        }
        let self_peer_id = self.protocol.network_state.local_peer_id();
        for peer_id_bytes in self.message.route().iter() {
            if let Ok(passed_peer_id) = PeerId::from_bytes(peer_id_bytes.raw_data().to_vec()) {
                if self_peer_id == &passed_peer_id {
                    return StatusCode::Ignore.with_context("the message is passed, ignore it");
                }
            } else {
                return StatusCode::InvalidRoute.into();
            };
        }
        let from_peer_id =
            if let Ok(peer_id) = PeerId::from_bytes(self.message.from().raw_data().to_vec()) {
                peer_id
            } else {
                return StatusCode::InvalidFromPeerId.into();
            };
        if self_peer_id == &from_peer_id {
            return StatusCode::Ignore.with_context("the message is sent by self, ignore it");
        }
        let to_peer_id =
            if let Ok(peer_id) = PeerId::from_bytes(self.message.to().raw_data().to_vec()) {
                peer_id
            } else {
                return StatusCode::InvalidToPeerId.into();
            };
        if self_peer_id == &to_peer_id {
            if let Some(last_from) = self.protocol.from_addrs.read().get(&from_peer_id) {
                if Instant::now().saturating_duration_since(*last_from) < PENETRATED_INTERVAL {
                    return StatusCode::Ignore
                        .with_context("a same message is already replied in a moment ago");
                }
            }
            let listen_addrs = {
                let reader = self
                    .protocol
                    .network_state
                    .possible_addrs(ADDRS_COUNT_LIMIT);
                let iter = reader
                    .iter()
                    .map(Multiaddr::to_vec)
                    .map(|v| packed::Address::new_builder().bytes(v.pack()).build());
                packed::AddressVec::new_builder().extend(iter).build()
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
            let new_message = packed::PenetrationMessage::new_builder()
                .set(content)
                .build()
                .as_bytes();
            let proto_id = SupportProtocols::Penetration.protocol_id();
            debug!(
                "current peer is the target peer {}, send a response back",
                to_peer_id
            );
            if let Err(error) = self
                .p2p_control
                .try_forward(self.peer, proto_id, new_message)
            {
                return StatusCode::ForwardError.with_context(error);
            }
            for listen_addr in message.listen_addrs().into_iter() {
                if let Ok(mut addr) = Multiaddr::try_from(listen_addr.bytes().raw_data().to_vec()) {
                    if let Some(peer_id) = extract_peer_id(&addr) {
                        if peer_id != from_peer_id {
                            continue;
                        }
                    } else {
                        addr.push(Protocol::P2P(Cow::Borrowed(from_peer_id.as_bytes())));
                    }
                    debug!("try dial {addr} ...");
                    self.protocol
                        .from_addrs
                        .write()
                        .insert(from_peer_id.clone(), Instant::now());
                    self.protocol.network_state.add_node(self.p2p_control, addr);
                }
            }
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
            let new_message = packed::PenetrationMessage::new_builder()
                .set(content)
                .build()
                .as_bytes();
            let proto_id = SupportProtocols::Penetration.protocol_id();

            if let Some(to_peer) = self
                .protocol
                .network_state
                .peer_registry
                .read()
                .get_key_by_peer_id(&to_peer_id)
            {
                debug!(
                    "target peer {} is found, forward the request to it",
                    to_peer_id
                );
                if let Err(error) = self.p2p_control.try_forward(to_peer, proto_id, new_message) {
                    StatusCode::ForwardError.with_context(error)
                } else {
                    Status::ok()
                }
            } else {
                debug!(
                    "target peer {} is not found, broadcast the request to more peers",
                    to_peer_id
                );
                if let Err(error) =
                    self.p2p_control
                        .try_broadcast(false, None, proto_id, new_message)
                {
                    StatusCode::BroadcastError.with_context(error)
                } else {
                    Status::ok()
                }
            }
        }
    }
}
