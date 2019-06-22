pub(crate) mod discovery;
pub(crate) mod feeler;
pub(crate) mod identify;
pub(crate) mod ping;
#[cfg(test)]
mod test;

use crate::Error;
use ckb_logger::trace;
use futures::Future;
use p2p::{
    builder::MetaBuilder,
    bytes::Bytes,
    context::{ProtocolContext, ProtocolContextMutRef},
    service::{ProtocolHandle, ProtocolMeta, ServiceControl, TargetSession},
    traits::ServiceProtocol,
    ProtocolId, SessionId,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::codec::length_delimited;

pub type PeerIndex = SessionId;

use crate::{
    compress::{compress, decompress},
    Behaviour, NetworkState, Peer, PeerRegistry, ProtocolVersion, MAX_FRAME_LENGTH,
};

pub trait CKBProtocolContext: Send {
    // Interact with underlying p2p service
    fn set_notify(&self, interval: Duration, token: u64) -> Result<(), Error>;
    fn quick_send_message(
        &self,
        proto_id: ProtocolId,
        peer_index: PeerIndex,
        data: Bytes,
    ) -> Result<(), Error>;
    fn quick_send_message_to(&self, peer_index: PeerIndex, data: Bytes) -> Result<(), Error>;
    fn quick_filter_broadcast(&self, target: TargetSession, data: Bytes) -> Result<(), Error>;
    // spawn a future task
    fn future_task(
        &self,
        task: Box<Future<Item = (), Error = ()> + 'static + Send>,
    ) -> Result<(), Error>;
    fn send_message(
        &self,
        proto_id: ProtocolId,
        peer_index: PeerIndex,
        data: Bytes,
    ) -> Result<(), Error>;
    fn send_message_to(&self, peer_index: PeerIndex, data: Bytes) -> Result<(), Error>;
    // TODO allow broadcast to target ProtocolId
    fn filter_broadcast(&self, target: TargetSession, data: Bytes) -> Result<(), Error>;
    fn disconnect(&self, peer_index: PeerIndex) -> Result<(), Error>;
    // Interact with NetworkState
    fn get_peer(&self, peer_index: PeerIndex) -> Option<Peer>;
    fn connected_peers(&self) -> Vec<PeerIndex>;
    fn report_peer(&self, peer_index: PeerIndex, behaviour: Behaviour);
    fn ban_peer(&self, peer_index: PeerIndex, timeout: Duration);
    // Other methods
    fn protocol_id(&self) -> ProtocolId;
}

pub trait CKBProtocolHandler: Sync + Send {
    fn init(&mut self, nc: Arc<dyn CKBProtocolContext + Sync>);
    /// Called when opening protocol
    fn connected(
        &mut self,
        _nc: Arc<dyn CKBProtocolContext + Sync>,
        _peer_index: PeerIndex,
        _version: &str,
    ) {
    }
    /// Called when closing protocol
    fn disconnected(&mut self, _nc: Arc<dyn CKBProtocolContext + Sync>, _peer_index: PeerIndex) {}
    /// Called when the corresponding protocol message is received
    fn received(
        &mut self,
        _nc: Arc<dyn CKBProtocolContext + Sync>,
        _peer_index: PeerIndex,
        _data: bytes::Bytes,
    ) {
    }
    /// Called when the Service receives the notify task
    fn notify(&mut self, _nc: Arc<dyn CKBProtocolContext + Sync>, _token: u64) {}
    /// Behave like `Stream::poll`, but nothing output
    fn poll(&mut self, _nc: Arc<dyn CKBProtocolContext + Sync>) {}
}

pub struct CKBProtocol {
    id: ProtocolId,
    // for example: b"/ckb/"
    protocol_name: String,
    // supported version, used to check protocol version
    supported_versions: Vec<ProtocolVersion>,
    handler: Box<Fn() -> Box<dyn CKBProtocolHandler + Send + 'static> + Send + 'static>,
    network_state: Arc<NetworkState>,
}

impl CKBProtocol {
    pub fn new<F: Fn() -> Box<dyn CKBProtocolHandler + Send + 'static> + Send + 'static>(
        protocol_name: String,
        id: ProtocolId,
        versions: &[ProtocolVersion],
        handler: F,
        network_state: Arc<NetworkState>,
    ) -> Self {
        CKBProtocol {
            id,
            network_state,
            handler: Box::new(handler),
            protocol_name: format!("/ckb/{}/", protocol_name).to_string(),
            supported_versions: {
                let mut versions: Vec<_> = versions.to_vec();
                versions.sort_by(|a, b| b.cmp(a));
                versions.to_vec()
            },
        }
    }

    pub fn id(&self) -> ProtocolId {
        self.id
    }

    pub fn protocol_name(&self) -> String {
        self.protocol_name.clone()
    }

    pub fn match_version(&self, version: ProtocolVersion) -> bool {
        self.supported_versions.contains(&version)
    }

    pub fn build(self) -> ProtocolMeta {
        let protocol_name = self.protocol_name();
        let supported_versions = self
            .supported_versions
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        MetaBuilder::default()
            .id(self.id)
            .name(move |_| protocol_name.clone())
            .codec(|| {
                Box::new(
                    length_delimited::Builder::new()
                        .max_frame_length(MAX_FRAME_LENGTH)
                        .new_codec(),
                )
            })
            .support_versions(supported_versions)
            .service_handle(move || {
                ProtocolHandle::Both(Box::new(CKBHandler {
                    proto_id: self.id,
                    network_state: Arc::clone(&self.network_state),
                    handler: (self.handler)(),
                }))
            })
            .before_send(compress)
            .before_receive(|| Some(Box::new(decompress)))
            .build()
    }
}

struct CKBHandler {
    proto_id: ProtocolId,
    network_state: Arc<NetworkState>,
    handler: Box<dyn CKBProtocolHandler>,
}

// Just proxy to inner handler, this struct exists for convenient unit test.
impl ServiceProtocol for CKBHandler {
    fn init(&mut self, context: &mut ProtocolContext) {
        let nc = DefaultCKBProtocolContext {
            proto_id: self.proto_id,
            network_state: Arc::clone(&self.network_state),
            p2p_control: context.control().to_owned(),
        };
        nc.set_notify(Duration::from_secs(6), std::u64::MAX)
            .expect("set_notify at init should be ok");
        self.handler.init(Arc::new(nc));
    }

    fn connected(&mut self, context: ProtocolContextMutRef, version: &str) {
        let nc = DefaultCKBProtocolContext {
            proto_id: self.proto_id,
            network_state: Arc::clone(&self.network_state),
            p2p_control: context.control().to_owned(),
        };
        let peer_index = context.session.id;
        self.handler.connected(Arc::new(nc), peer_index, version);
    }

    fn disconnected(&mut self, context: ProtocolContextMutRef) {
        let nc = DefaultCKBProtocolContext {
            proto_id: self.proto_id,
            network_state: Arc::clone(&self.network_state),
            p2p_control: context.control().to_owned(),
        };
        let peer_index = context.session.id;
        self.handler.disconnected(Arc::new(nc), peer_index);
    }

    fn received(&mut self, context: ProtocolContextMutRef, data: bytes::Bytes) {
        trace!(
            "[received message]: {}, {}, length={}",
            self.proto_id,
            context.session.id,
            data.len()
        );
        let nc = DefaultCKBProtocolContext {
            proto_id: self.proto_id,
            network_state: Arc::clone(&self.network_state),
            p2p_control: context.control().to_owned(),
        };
        let peer_index = context.session.id;
        self.handler.received(Arc::new(nc), peer_index, data);
    }

    fn notify(&mut self, context: &mut ProtocolContext, token: u64) {
        if token == std::u64::MAX {
            trace!("protocol handler heart beat {}", self.proto_id);
        } else {
            let nc = DefaultCKBProtocolContext {
                proto_id: self.proto_id,
                network_state: Arc::clone(&self.network_state),
                p2p_control: context.control().to_owned(),
            };
            self.handler.notify(Arc::new(nc), token);
        }
    }

    fn poll(&mut self, context: &mut ProtocolContext) {
        let nc = DefaultCKBProtocolContext {
            proto_id: self.proto_id,
            network_state: Arc::clone(&self.network_state),
            p2p_control: context.control().to_owned(),
        };
        self.handler.poll(Arc::new(nc));
    }
}

struct DefaultCKBProtocolContext {
    proto_id: ProtocolId,
    network_state: Arc<NetworkState>,
    p2p_control: ServiceControl,
}

impl CKBProtocolContext for DefaultCKBProtocolContext {
    fn set_notify(&self, interval: Duration, token: u64) -> Result<(), Error> {
        self.p2p_control
            .set_service_notify(self.proto_id, interval, token)?;
        Ok(())
    }
    fn quick_send_message(
        &self,
        proto_id: ProtocolId,
        peer_index: PeerIndex,
        data: Bytes,
    ) -> Result<(), Error> {
        trace!(
            "[send message]: {}, to={}, length={}",
            proto_id,
            peer_index,
            data.len()
        );
        self.p2p_control
            .quick_send_message_to(peer_index, proto_id, data)?;
        Ok(())
    }
    fn quick_send_message_to(&self, peer_index: PeerIndex, data: Bytes) -> Result<(), Error> {
        trace!(
            "[send message to]: {}, to={}, length={}",
            self.proto_id,
            peer_index,
            data.len()
        );
        self.p2p_control
            .quick_send_message_to(peer_index, self.proto_id, data)?;
        Ok(())
    }
    fn quick_filter_broadcast(&self, target: TargetSession, data: Bytes) -> Result<(), Error> {
        self.p2p_control
            .quick_filter_broadcast(target, self.proto_id, data)?;
        Ok(())
    }
    fn future_task(
        &self,
        task: Box<Future<Item = (), Error = ()> + 'static + Send>,
    ) -> Result<(), Error> {
        self.p2p_control.future_task(task)?;
        Ok(())
    }
    fn send_message(
        &self,
        proto_id: ProtocolId,
        peer_index: PeerIndex,
        data: Bytes,
    ) -> Result<(), Error> {
        trace!(
            "[send message]: {}, to={}, length={}",
            proto_id,
            peer_index,
            data.len()
        );
        self.p2p_control
            .send_message_to(peer_index, proto_id, data)?;
        Ok(())
    }
    fn send_message_to(&self, peer_index: PeerIndex, data: Bytes) -> Result<(), Error> {
        trace!(
            "[send message to]: {}, to={}, length={}",
            self.proto_id,
            peer_index,
            data.len()
        );
        self.p2p_control
            .send_message_to(peer_index, self.proto_id, data)?;
        Ok(())
    }
    fn filter_broadcast(&self, target: TargetSession, data: Bytes) -> Result<(), Error> {
        self.p2p_control
            .filter_broadcast(target, self.proto_id, data)?;
        Ok(())
    }
    fn disconnect(&self, peer_index: PeerIndex) -> Result<(), Error> {
        self.p2p_control.disconnect(peer_index)?;
        Ok(())
    }

    fn get_peer(&self, peer_index: PeerIndex) -> Option<Peer> {
        self.network_state
            .with_peer_registry(|reg| reg.get_peer(peer_index).cloned())
    }
    fn connected_peers(&self) -> Vec<PeerIndex> {
        self.network_state
            .with_peer_registry(PeerRegistry::connected_peers)
    }
    fn report_peer(&self, peer_index: PeerIndex, behaviour: Behaviour) {
        self.network_state
            .report_session(&self.p2p_control, peer_index, behaviour);
    }
    fn ban_peer(&self, peer_index: PeerIndex, timeout: Duration) {
        self.network_state
            .ban_session(&self.p2p_control, peer_index, timeout);
    }

    fn protocol_id(&self) -> ProtocolId {
        self.proto_id
    }
}
