pub(crate) mod disconnect_message;
pub(crate) mod discovery;
pub(crate) mod feeler;
pub(crate) mod identify;
pub(crate) mod ping;
pub(crate) mod support_protocols;

#[cfg(test)]
mod test;

use ckb_logger::{debug, trace};
use futures::{Future, FutureExt};
use p2p::{
    builder::MetaBuilder,
    bytes::Bytes,
    context::{ProtocolContext, ProtocolContextMutRef},
    service::{BlockingFlag, ProtocolHandle, ProtocolMeta, ServiceControl, TargetSession},
    traits::ServiceProtocol,
    ProtocolId, SessionId,
};
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio_util::codec::length_delimited;

/// TODO(doc): @driftluo
pub type PeerIndex = SessionId;
/// TODO(doc): @driftluo
pub type BoxedFutureTask = Pin<Box<dyn Future<Output = ()> + 'static + Send>>;

use crate::{
    compress::{compress, decompress},
    network::disconnect_with_message,
    Behaviour, Error, NetworkState, Peer, ProtocolVersion,
};

/// TODO(doc): @driftluo
pub trait CKBProtocolContext: Send {
    /// TODO(doc): @driftluo
    // Interact with underlying p2p service
    fn set_notify(&self, interval: Duration, token: u64) -> Result<(), Error>;
    /// TODO(doc): @driftluo
    fn remove_notify(&self, token: u64) -> Result<(), Error>;
    /// TODO(doc): @driftluo
    fn quick_send_message(
        &self,
        proto_id: ProtocolId,
        peer_index: PeerIndex,
        data: Bytes,
    ) -> Result<(), Error>;
    /// TODO(doc): @driftluo
    fn quick_send_message_to(&self, peer_index: PeerIndex, data: Bytes) -> Result<(), Error>;
    /// TODO(doc): @driftluo
    fn quick_filter_broadcast(&self, target: TargetSession, data: Bytes) -> Result<(), Error>;
    // spawn a future task, if `blocking` is true we use tokio_threadpool::blocking to handle the task.
    /// TODO(doc): @driftluo
    fn future_task(&self, task: BoxedFutureTask, blocking: bool) -> Result<(), Error>;
    /// TODO(doc): @driftluo
    fn send_message(
        &self,
        proto_id: ProtocolId,
        peer_index: PeerIndex,
        data: Bytes,
    ) -> Result<(), Error>;
    /// TODO(doc): @driftluo
    fn send_message_to(&self, peer_index: PeerIndex, data: Bytes) -> Result<(), Error>;
    /// TODO(doc): @driftluo
    // TODO allow broadcast to target ProtocolId
    fn filter_broadcast(&self, target: TargetSession, data: Bytes) -> Result<(), Error>;
    /// TODO(doc): @driftluo
    fn disconnect(&self, peer_index: PeerIndex, message: &str) -> Result<(), Error>;
    // Interact with NetworkState
    /// TODO(doc): @driftluo
    fn get_peer(&self, peer_index: PeerIndex) -> Option<Peer>;
    /// TODO(doc): @driftluo
    fn with_peer_mut(&self, peer_index: PeerIndex, f: Box<dyn FnOnce(&mut Peer)>);
    /// TODO(doc): @driftluo
    fn connected_peers(&self) -> Vec<PeerIndex>;
    /// TODO(doc): @driftluo
    fn report_peer(&self, peer_index: PeerIndex, behaviour: Behaviour);
    /// TODO(doc): @driftluo
    fn ban_peer(&self, peer_index: PeerIndex, duration: Duration, reason: String);
    /// TODO(doc): @driftluo
    fn send_paused(&self) -> bool;
    /// TODO(doc): @driftluo
    // Other methods
    fn protocol_id(&self) -> ProtocolId;
    /// TODO(doc): @driftluo
    fn p2p_control(&self) -> Option<&ServiceControl> {
        None
    }
}

/// TODO(doc): @driftluo
pub trait CKBProtocolHandler: Sync + Send {
    /// TODO(doc): @driftluo
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
        _data: Bytes,
    ) {
    }
    /// Called when the Service receives the notify task
    fn notify(&mut self, _nc: Arc<dyn CKBProtocolContext + Sync>, _token: u64) {}
    /// Behave like `Stream::poll`
    fn poll(&mut self, _nc: Arc<dyn CKBProtocolContext + Sync>) -> Poll<Option<()>> {
        Poll::Ready(None)
    }
}

/// TODO(doc): @driftluo
pub struct CKBProtocol {
    id: ProtocolId,
    // for example: b"/ckb/"
    protocol_name: String,
    // supported version, used to check protocol version
    supported_versions: Vec<ProtocolVersion>,
    max_frame_length: usize,
    handler: Box<dyn CKBProtocolHandler>,
    network_state: Arc<NetworkState>,
    flag: BlockingFlag,
}

impl CKBProtocol {
    /// TODO(doc): @driftluo
    // a helper constructor to build `CKBProtocol` with `SupportProtocols` enum
    pub fn new_with_support_protocol(
        support_protocol: support_protocols::SupportProtocols,
        handler: Box<dyn CKBProtocolHandler>,
        network_state: Arc<NetworkState>,
    ) -> Self {
        CKBProtocol {
            id: support_protocol.protocol_id(),
            max_frame_length: support_protocol.max_frame_length(),
            protocol_name: support_protocol.name(),
            supported_versions: support_protocol.support_versions(),
            flag: support_protocol.flag(),
            network_state,
            handler,
        }
    }

    /// TODO(doc): @driftluo
    pub fn new(
        protocol_name: String,
        id: ProtocolId,
        versions: &[ProtocolVersion],
        max_frame_length: usize,
        handler: Box<dyn CKBProtocolHandler>,
        network_state: Arc<NetworkState>,
        flag: BlockingFlag,
    ) -> Self {
        CKBProtocol {
            id,
            max_frame_length,
            network_state,
            handler,
            protocol_name: format!("/ckb/{}", protocol_name),
            supported_versions: {
                let mut versions: Vec<_> = versions.to_vec();
                // TODO: https://github.com/rust-lang/rust-clippy/issues/6001
                #[allow(clippy::unnecessary_sort_by)]
                versions.sort_by(|a, b| b.cmp(a));
                versions.to_vec()
            },
            flag,
        }
    }

    /// TODO(doc): @driftluo
    pub fn id(&self) -> ProtocolId {
        self.id
    }

    /// TODO(doc): @driftluo
    pub fn protocol_name(&self) -> String {
        self.protocol_name.clone()
    }

    /// TODO(doc): @driftluo
    pub fn match_version(&self, version: ProtocolVersion) -> bool {
        self.supported_versions.contains(&version)
    }

    /// TODO(doc): @driftluo
    pub fn build(self) -> ProtocolMeta {
        let protocol_name = self.protocol_name();
        let max_frame_length = self.max_frame_length;
        let flag = self.flag;
        let supported_versions = self
            .supported_versions
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        MetaBuilder::default()
            .id(self.id)
            .name(move |_| protocol_name.clone())
            .codec(move || {
                Box::new(
                    length_delimited::Builder::new()
                        .max_frame_length(max_frame_length)
                        .new_codec(),
                )
            })
            .support_versions(supported_versions)
            .service_handle(move || {
                ProtocolHandle::Callback(Box::new(CKBHandler {
                    proto_id: self.id,
                    network_state: Arc::clone(&self.network_state),
                    handler: self.handler,
                }))
            })
            .before_send(compress)
            .before_receive(|| Some(Box::new(decompress)))
            .flag(flag)
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
            send_paused: false,
        };
        nc.set_notify(Duration::from_secs(6), std::u64::MAX)
            .expect("set_notify at init should be ok");
        self.handler.init(Arc::new(nc));
    }

    fn connected(&mut self, context: ProtocolContextMutRef, version: &str) {
        self.network_state.with_peer_registry_mut(|reg| {
            if let Some(peer) = reg.get_peer_mut(context.session.id) {
                peer.protocols.insert(self.proto_id, version.to_owned());
            }
        });

        if !self.network_state.is_active() {
            return;
        }

        let pending_data_size = context.session.pending_data_size();
        let send_paused = pending_data_size >= self.network_state.config.max_send_buffer();
        let nc = DefaultCKBProtocolContext {
            proto_id: self.proto_id,
            network_state: Arc::clone(&self.network_state),
            p2p_control: context.control().to_owned(),
            send_paused,
        };
        let peer_index = context.session.id;
        self.handler.connected(Arc::new(nc), peer_index, version);
    }

    fn disconnected(&mut self, context: ProtocolContextMutRef) {
        self.network_state.with_peer_registry_mut(|reg| {
            if let Some(peer) = reg.get_peer_mut(context.session.id) {
                peer.protocols.remove(&self.proto_id);
            }
        });

        if !self.network_state.is_active() {
            return;
        }

        let pending_data_size = context.session.pending_data_size();
        let send_paused = pending_data_size >= self.network_state.config.max_send_buffer();
        let nc = DefaultCKBProtocolContext {
            proto_id: self.proto_id,
            network_state: Arc::clone(&self.network_state),
            p2p_control: context.control().to_owned(),
            send_paused,
        };
        let peer_index = context.session.id;
        self.handler.disconnected(Arc::new(nc), peer_index);
    }

    fn received(&mut self, context: ProtocolContextMutRef, data: Bytes) {
        self.network_state.with_peer_registry_mut(|reg| {
            if let Some(peer) = reg.get_peer_mut(context.session.id) {
                peer.last_message_time = Some(Instant::now());
            }
        });

        if !self.network_state.is_active() {
            return;
        }

        trace!(
            "[received message]: {}, {}, length={}",
            self.proto_id,
            context.session.id,
            data.len()
        );
        let pending_data_size = context.session.pending_data_size();
        let send_paused = pending_data_size >= self.network_state.config.max_send_buffer();
        let nc = DefaultCKBProtocolContext {
            proto_id: self.proto_id,
            network_state: Arc::clone(&self.network_state),
            p2p_control: context.control().to_owned(),
            send_paused,
        };
        let peer_index = context.session.id;
        self.handler.received(Arc::new(nc), peer_index, data);
    }

    fn notify(&mut self, context: &mut ProtocolContext, token: u64) {
        if !self.network_state.is_active() {
            return;
        }

        if token == std::u64::MAX {
            trace!("protocol handler heart beat {}", self.proto_id);
        } else {
            let nc = DefaultCKBProtocolContext {
                proto_id: self.proto_id,
                network_state: Arc::clone(&self.network_state),
                p2p_control: context.control().to_owned(),
                send_paused: false,
            };
            self.handler.notify(Arc::new(nc), token);
        }
    }

    fn poll(
        mut self: Pin<&mut Self>,
        _nc: &mut Context,
        context: &mut ProtocolContext,
    ) -> Poll<Option<()>> {
        let nc = DefaultCKBProtocolContext {
            proto_id: self.proto_id,
            network_state: Arc::clone(&self.network_state),
            p2p_control: context.control().to_owned(),
            send_paused: false,
        };
        self.handler.poll(Arc::new(nc))
    }
}

struct DefaultCKBProtocolContext {
    proto_id: ProtocolId,
    network_state: Arc<NetworkState>,
    p2p_control: ServiceControl,
    send_paused: bool,
}

impl CKBProtocolContext for DefaultCKBProtocolContext {
    fn set_notify(&self, interval: Duration, token: u64) -> Result<(), Error> {
        self.p2p_control
            .set_service_notify(self.proto_id, interval, token)?;
        Ok(())
    }
    fn remove_notify(&self, token: u64) -> Result<(), Error> {
        self.p2p_control
            .remove_service_notify(self.proto_id, token)?;
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
    fn future_task(&self, task: BoxedFutureTask, blocking: bool) -> Result<(), Error> {
        let task = if blocking {
            Box::pin(BlockingFutureTask::new(task))
        } else {
            task
        };
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
    fn disconnect(&self, peer_index: PeerIndex, message: &str) -> Result<(), Error> {
        debug!("disconnect peer: {}, message: {}", peer_index, message);
        disconnect_with_message(&self.p2p_control, peer_index, message)?;
        Ok(())
    }

    fn get_peer(&self, peer_index: PeerIndex) -> Option<Peer> {
        self.network_state
            .with_peer_registry(|reg| reg.get_peer(peer_index).cloned())
    }
    fn with_peer_mut(&self, peer_index: PeerIndex, f: Box<dyn FnOnce(&mut Peer)>) {
        self.network_state.with_peer_registry_mut(|reg| {
            reg.get_peer_mut(peer_index).map(f);
        })
    }

    fn connected_peers(&self) -> Vec<PeerIndex> {
        self.network_state.with_peer_registry(|reg| {
            reg.peers()
                .iter()
                .filter_map(|(peer_index, peer)| {
                    if peer.protocols.contains_key(&self.proto_id) {
                        Some(peer_index)
                    } else {
                        None
                    }
                })
                .cloned()
                .collect()
        })
    }
    fn report_peer(&self, peer_index: PeerIndex, behaviour: Behaviour) {
        self.network_state
            .report_session(&self.p2p_control, peer_index, behaviour);
    }
    fn ban_peer(&self, peer_index: PeerIndex, duration: Duration, reason: String) {
        self.network_state
            .ban_session(&self.p2p_control, peer_index, duration, reason);
    }

    fn protocol_id(&self) -> ProtocolId {
        self.proto_id
    }

    fn send_paused(&self) -> bool {
        self.send_paused
    }

    fn p2p_control(&self) -> Option<&ServiceControl> {
        Some(&self.p2p_control)
    }
}

pub(crate) struct BlockingFutureTask {
    task: BoxedFutureTask,
}

impl BlockingFutureTask {
    pub(crate) fn new(task: BoxedFutureTask) -> BlockingFutureTask {
        BlockingFutureTask { task }
    }
}

impl Future for BlockingFutureTask {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        tokio::task::block_in_place(|| self.task.poll_unpin(cx))
    }
}
