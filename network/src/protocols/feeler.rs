use crate::NetworkState;
use ckb_logger::{debug, info};
use p2p::{
    context::{ProtocolContext, ProtocolContextMutRef},
    secio::PublicKey,
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

//TODO
//1. report bad behaviours
//2. set peer feeler flag
impl ServiceProtocol for Feeler {
    fn init(&mut self, _context: &mut ProtocolContext) {}

    fn connected(&mut self, context: ProtocolContextMutRef, _: &str) {
        let session = context.session;
        let peer_id = session
            .remote_pubkey
            .as_ref()
            .map(PublicKey::peer_id)
            .expect("Secio must enabled");
        self.network_state.with_peer_store_mut(|peer_store| {
            peer_store.add_connected_peer(&peer_id, session.address.clone(), session.ty);
        });
        info!("peer={} FeelerProtocol.connected", session.address);
        if let Err(err) = context.disconnect(session.id) {
            debug!("Disconnect failed {:?}, error: {:?}", session.id, err);
        }
    }

    fn disconnected(&mut self, context: ProtocolContextMutRef) {
        let session = context.session;
        let peer_id = session
            .remote_pubkey
            .as_ref()
            .map(PublicKey::peer_id)
            .expect("Secio must enabled");
        self.network_state.with_peer_registry_mut(|reg| {
            reg.remove_feeler(&peer_id);
        });
        info!("peer={} FeelerProtocol.disconnected", session.address);
    }
}
