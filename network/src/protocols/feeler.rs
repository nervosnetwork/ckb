use crate::network::disconnect_with_message;
use crate::NetworkState;
use ckb_logger::debug;
use p2p::{
    context::{ProtocolContext, ProtocolContextMutRef},
    traits::ServiceProtocol,
};
use std::sync::Arc;

/// Feeler
/// Currently do nothing, CKBProtocol auto refresh peer_store after connected.
pub(crate) struct Feeler {
    network_state: Arc<NetworkState>,
}

impl Feeler {
    pub(crate) fn new(network_state: Arc<NetworkState>) -> Self {
        Feeler { network_state }
    }
}

impl ServiceProtocol for Feeler {
    fn init(&mut self, _context: &mut ProtocolContext) {}

    fn connected(&mut self, context: ProtocolContextMutRef, _version: &str) {
        let session = context.session;
        if context.session.ty.is_outbound() {
            self.network_state.with_peer_store_mut(|peer_store| {
                peer_store.add_outbound_addr(session.address.clone());
            });
        }

        debug!("peer={} FeelerProtocol.connected", session.address);
        if let Err(err) =
            disconnect_with_message(context.control(), session.id, "feeler connection")
        {
            debug!("Disconnect failed {:?}, error: {:?}", session.id, err);
        }
    }

    fn disconnected(&mut self, context: ProtocolContextMutRef) {
        let session = context.session;
        self.network_state.with_peer_registry_mut(|reg| {
            reg.remove_feeler(&session.address);
        });
        debug!("peer={} FeelerProtocol.disconnected", session.address);
    }
}
