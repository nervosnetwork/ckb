use crate::network::async_disconnect_with_message;
use crate::NetworkState;
use ckb_logger::debug;
use p2p::{
    async_trait,
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

#[async_trait]
impl ServiceProtocol for Feeler {
    async fn init(&mut self, _context: &mut ProtocolContext) {}

    async fn connected(&mut self, context: ProtocolContextMutRef<'_>, _version: &str) {
        let session = context.session;
        if context.session.ty.is_outbound() {
            self.network_state.with_peer_store_mut(|peer_store| {
                peer_store.add_outbound_addr(session.address.clone());
            });
        }

        debug!("peer={} FeelerProtocol.connected", session.address);
        if let Err(err) =
            async_disconnect_with_message(context.control(), session.id, "feeler connection").await
        {
            debug!("Disconnect failed {:?}, error: {:?}", session.id, err);
        }
    }

    async fn disconnected(&mut self, context: ProtocolContextMutRef<'_>) {
        let session = context.session;
        self.network_state.with_peer_registry_mut(|reg| {
            reg.remove_feeler(&session.address);
        });
        debug!("peer={} FeelerProtocol.disconnected", session.address);
    }
}
