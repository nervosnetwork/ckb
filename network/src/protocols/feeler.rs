use crate::network::disconnect_with_message;
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
            if let Err(err) = peer_store.add_connected_peer(session.address.clone(), session.ty) {
                debug!(
                    "Failed to add connected peer to peer_store {:?} {:?} {:?}",
                    err, peer_id, session
                );
            }
        });
        info!("peer={} FeelerProtocol.connected", session.address);
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
        info!("peer={} FeelerProtocol.disconnected", session.address);
    }
}
