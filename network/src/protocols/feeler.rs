use crate::{CKBProtocolContext, CKBProtocolHandler, SessionId};
use bytes::Bytes;
use log::info;

/// Feeler
/// Currently do nothing, CKBProtocol auto refresh peer_store after connected.
pub struct Feeler {}

//TODO
//1. report bad behaviours
//2. set peer feeler flag
impl CKBProtocolHandler for Feeler {
    fn initialize(&self, _nc: &mut dyn CKBProtocolContext) {}

    fn received(&self, _nc: &mut dyn CKBProtocolContext, _peer: SessionId, _data: Bytes) {}

    fn connected(&self, nc: &mut dyn CKBProtocolContext, peer: SessionId) {
        info!(target: "feeler", "peer={} FeelerProtocol.connected", peer);
        nc.disconnect(peer);
    }

    fn disconnected(&self, _nc: &mut dyn CKBProtocolContext, peer: SessionId) {
        info!(target: "relay", "peer={} FeelerProtocol.disconnected", peer);
    }
}
