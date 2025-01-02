use std::borrow::Cow;

use ckb_logger::debug;
use ckb_types::{packed, prelude::*};
use p2p::{
    multiaddr::{Multiaddr, Protocol},
    service::ServiceControl,
    utils::extract_peer_id,
};

use super::super::{Penetration, Status, StatusCode};
use crate::{network::ServiceControlExt as _, PeerId, SupportProtocols};

pub(crate) struct ConnectionRequestDeliveredProcess<'a> {
    message: packed::ConnectionRequestDeliveredReader<'a>,
    protocol: &'a Penetration,
    p2p_control: &'a ServiceControl,
}

impl<'a> ConnectionRequestDeliveredProcess<'a> {
    pub(crate) fn new(
        message: packed::ConnectionRequestDeliveredReader<'a>,
        protocol: &'a Penetration,
        p2p_control: &'a ServiceControl,
    ) -> Self {
        Self {
            message,
            protocol,
            p2p_control,
        }
    }

    pub(crate) fn execute(self) -> Status {
        let route = self.message.route();
        if let Some(next_peer_id_data) = route.iter().last() {
            // Forward the message to the next peer if it's still connected.
            let next_peer_id =
                if let Ok(peer_id) = PeerId::from_bytes(next_peer_id_data.raw_data().to_vec()) {
                    peer_id
                } else {
                    return StatusCode::InvalidRoute.with_context("the last peer id is invalid");
                };
            if let Some(next_peer) = self
                .protocol
                .network_state
                .peer_registry
                .read()
                .get_key_by_peer_id(&next_peer_id)
            {
                let message = self.message.to_entity();
                let new_route = packed::BytesVec::new_builder()
                    .extend(message.route().into_iter().take(route.len() - 1))
                    .build();
                let content = message.as_builder().route(new_route).build();
                let new_message = packed::PenetrationMessage::new_builder()
                    .set(content)
                    .build()
                    .as_bytes();
                let proto_id = SupportProtocols::Penetration.protocol_id();
                debug!(
                    "forward the delivery to next peer {} (id: {})",
                    next_peer, next_peer_id
                );
                if let Err(error) = self
                    .p2p_control
                    .try_forward(next_peer, proto_id, new_message)
                {
                    StatusCode::ForwardError.with_context(error)
                } else {
                    Status::ok()
                }
            } else {
                StatusCode::Ignore.with_context("the next peer in the route is disconnected")
            }
        } else {
            // Current node should be the `from` target.
            let from_peer_id =
                if let Ok(peer_id) = PeerId::from_bytes(self.message.from().raw_data().to_vec()) {
                    peer_id
                } else {
                    return StatusCode::InvalidFromPeerId.into();
                };
            let self_peer_id = self.protocol.network_state.local_peer_id();
            if self_peer_id != &from_peer_id {
                return StatusCode::Ignore.with_context("the destination of route is not self");
            }
            let to_peer_id =
                if let Ok(peer_id) = PeerId::from_bytes(self.message.to().raw_data().to_vec()) {
                    peer_id
                } else {
                    return StatusCode::InvalidToPeerId.into();
                };
            for listen_addr in self.message.listen_addrs().iter() {
                if let Ok(mut addr) = Multiaddr::try_from(listen_addr.bytes().raw_data().to_vec()) {
                    if let Some(peer_id) = extract_peer_id(&addr) {
                        if peer_id != to_peer_id {
                            continue;
                        }
                    } else {
                        addr.push(Protocol::P2P(Cow::Borrowed(to_peer_id.as_bytes())));
                    }
                    debug!("try dial {addr} ...");
                    self.protocol.network_state.add_node(self.p2p_control, addr);
                }
            }
            Status::ok()
        }
    }
}
