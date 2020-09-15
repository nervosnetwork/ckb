use ckb_logger::{debug, info};
use p2p::{
    bytes::Bytes,
    context::{ProtocolContext, ProtocolContextMutRef},
    traits::ServiceProtocol,
};

use crate::NetworkState;
use std::sync::Arc;

// A protocol for just receive string message and log it use debug level.
pub(crate) struct DisconnectMessageProtocol(Arc<NetworkState>);

impl DisconnectMessageProtocol {
    pub(crate) fn new(network_state: Arc<NetworkState>) -> Self {
        DisconnectMessageProtocol(network_state)
    }
}

impl ServiceProtocol for DisconnectMessageProtocol {
    fn init(&mut self, _context: &mut ProtocolContext) {}

    fn received(&mut self, context: ProtocolContextMutRef, data: Bytes) {
        let session_id = context.session.id;
        if let Ok(message) = String::from_utf8(data.to_vec()) {
            info!(
                "Received disconnect message from peer={}: {}",
                session_id, message
            );
        } else {
            // Maybe punish this peer later (also when send us too large message)
            debug!(
                "[WARNING]: peer {} send us a malformed disconnect message!",
                session_id
            );
        }
        if let Err(err) = context.disconnect(session_id) {
            debug!("Disconnect {:?} failed, error: {:?}", session_id, err);
        }
    }

    fn connected(&mut self, context: ProtocolContextMutRef, version: &str) {
        debug!(
            "DisconnectMessageProtocol connected, peer={}",
            context.session.id
        );
        self.0.with_peer_registry_mut(|reg| {
            reg.get_peer_mut(context.session.id).map(|peer| {
                peer.protocols.insert(context.proto_id, version.to_owned());
            })
        });
    }

    fn disconnected(&mut self, context: ProtocolContextMutRef) {
        debug!(
            "DisconnectMessageProtocol disconnected, peer={}",
            context.session.id
        );
        self.0.with_peer_registry_mut(|reg| {
            let _ = reg.get_peer_mut(context.session.id).map(|peer| {
                peer.protocols.remove(&context.proto_id);
            });
        });
    }
}
