use log::info;
use p2p::{
    context::{ProtocolContext, ProtocolContextMutRef},
    traits::ServiceProtocol,
};

/// Feeler
/// Currently do nothing, CKBProtocol auto refresh peer_store after connected.
pub struct Feeler {}

//TODO
//1. report bad behaviours
//2. set peer feeler flag
impl ServiceProtocol for Feeler {
    fn init(&mut self, _context: &mut ProtocolContext) {}

    fn connected(&mut self, context: ProtocolContextMutRef, _: &str) {
        let session = context.session;
        info!(target: "feeler", "peer={} FeelerProtocol.connected", session.address);
        context.disconnect(session.id);
    }

    fn disconnected(&mut self, context: ProtocolContextMutRef) {
        let session = context.session;
        info!(target: "relay", "peer={} FeelerProtocol.disconnected", session.address);
    }
}
