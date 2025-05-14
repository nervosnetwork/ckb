use std::borrow::Cow;

use ckb_logger::debug;
use ckb_systemtime::unix_time_as_millis;
use ckb_types::{packed, prelude::*};
use p2p::{
    multiaddr::{Multiaddr, Protocol},
    service::{ServiceAsyncControl, TargetSession},
    utils::{TransportType, extract_peer_id, find_type},
};

use crate::{
    PeerId, PeerIndex,
    protocols::{
        SupportProtocols,
        hole_punching::{
            ADDRS_COUNT_LIMIT, HolePunching, MAX_TTL, PENETRATED_INTERVAL,
            component::{forward_request, init_delivered},
            status::{Status, StatusCode},
        },
    },
};

pub(crate) struct ConnectionRequestProcess<'a> {
    message: packed::ConnectionRequestReader<'a>,
    protocol: &'a HolePunching,
    peer: PeerIndex,
    p2p_control: &'a ServiceAsyncControl,
}

impl<'a> ConnectionRequestProcess<'a> {
    pub(crate) fn new(
        message: packed::ConnectionRequestReader<'a>,
        protocol: &'a HolePunching,
        peer: PeerIndex,
        p2p_control: &'a ServiceAsyncControl,
    ) -> Self {
        Self {
            message,
            protocol,
            peer,
            p2p_control,
        }
    }

    pub(crate) async fn execute(self) -> Status {
        if self.message.listen_addrs().len() > ADDRS_COUNT_LIMIT
            || self.message.listen_addrs().is_empty()
        {
            return StatusCode::InvalidListenAddrLen
                .with_context("the listen address count is too large or empty");
        }
        let ttl: u8 = self.message.ttl().into();
        if ttl > MAX_TTL {
            return StatusCode::InvalidMaxTTL.into();
        }
        if self.message.route().len() > 8 {
            return StatusCode::InvalidRoute.with_context("the route length is too long");
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
            self.respond_delivered(from_peer_id, &to_peer_id).await
        } else if ttl == 0u8 {
            StatusCode::ReachedMaxTTL.into()
        } else {
            self.forward_message(self_peer_id, &to_peer_id).await
        }
    }

    async fn respond_delivered(&self, from_peer_id: PeerId, to_peer_id: &PeerId) -> Status {
        if let Some((_, t)) = self.protocol.pending_delivered.read().get(&from_peer_id) {
            let now = unix_time_as_millis();
            if now - t < PENETRATED_INTERVAL {
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
        let content = init_delivered(self.message, listen_addrs);
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

        let remote_listens = self
            .message
            .listen_addrs()
            .iter()
            .filter_map(
                |raw| match Multiaddr::try_from(raw.bytes().raw_data().to_vec()) {
                    Ok(mut addr) => {
                        if let Some(peer_id) = extract_peer_id(&addr) {
                            if peer_id != from_peer_id {
                                return None;
                            }
                        } else {
                            addr.push(Protocol::P2P(Cow::Borrowed(from_peer_id.as_bytes())));
                        }

                        match find_type(&addr) {
                            TransportType::Memory
                            | TransportType::Onion
                            | TransportType::Ws
                            | TransportType::Wss
                            | TransportType::Tls => None,
                            TransportType::Tcp => Some(addr),
                        }
                    }
                    Err(_) => None,
                },
            )
            .collect();

        let mut pending_delivered = self.protocol.pending_delivered.write();
        let now = unix_time_as_millis();
        pending_delivered.insert(from_peer_id, (remote_listens, now));

        Status::ok()
    }

    async fn forward_message(&self, self_peer_id: &PeerId, to_peer_id: &PeerId) -> Status {
        let content = forward_request(self.message, self_peer_id);
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
            .get_key_by_peer_id(to_peer_id);

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
