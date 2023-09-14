use crate::network::async_disconnect_with_message;
use crate::{Flags, NetworkState};
use ckb_logger::debug;
use p2p::{
    async_trait,
    context::{ProtocolContext, ProtocolContextMutRef},
    traits::ServiceProtocol,
};
use std::sync::{atomic::Ordering, Arc};

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

    async fn connected(&mut self, context: ProtocolContextMutRef<'_>, version: &str) {
        let session = context.session;
        if self.network_state.ckb2023.load(Ordering::SeqCst) && version != "3" {
            self.network_state
                .peer_store
                .lock()
                .mut_addr_manager()
                .remove(&session.address);
        } else if context.session.ty.is_outbound() {
            let flags = self.network_state.with_peer_registry(|reg| {
                if let Some(p) = reg.get_peer(session.id) {
                    p.identify_info
                        .as_ref()
                        .map(|i| i.flags)
                        .unwrap_or(Flags::COMPATIBILITY)
                } else {
                    Flags::COMPATIBILITY
                }
            });
            self.network_state.with_peer_store_mut(|peer_store| {
                peer_store.add_outbound_addr(session.address.clone(), flags);
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
