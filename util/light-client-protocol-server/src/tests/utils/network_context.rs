use std::cell::RefCell;
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use ckb_network::{
    async_trait, bytes::Bytes as P2pBytes, Behaviour, CKBProtocolContext, Error, Peer, PeerIndex,
    ProtocolId, SupportProtocols, TargetSession,
};

struct MockProtocolContext {
    protocol: SupportProtocols,
    sent_messages: RefCell<Vec<(ProtocolId, PeerIndex, P2pBytes)>>,
    banned_peers: RefCell<Vec<(PeerIndex, Duration, String)>>,
    connected_peers: RefCell<HashSet<PeerIndex>>,
}

pub(crate) struct MockNetworkContext {
    inner: Arc<MockProtocolContext>,
}

// test mock context with single thread
unsafe impl Send for MockProtocolContext {}
unsafe impl Sync for MockProtocolContext {}

impl MockProtocolContext {
    fn new(protocol: SupportProtocols) -> Self {
        Self {
            protocol,
            sent_messages: Default::default(),
            banned_peers: Default::default(),
            connected_peers: Default::default(),
        }
    }
}

impl MockNetworkContext {
    pub(crate) fn new(protocol: SupportProtocols) -> Self {
        let context = MockProtocolContext::new(protocol);
        let inner = Arc::new(context);
        Self { inner }
    }

    pub(crate) fn sent_messages(&self) -> &RefCell<Vec<(ProtocolId, PeerIndex, P2pBytes)>> {
        &self.inner.sent_messages
    }

    pub(crate) fn banned_peers(&self) -> &RefCell<Vec<(PeerIndex, Duration, String)>> {
        &self.inner.banned_peers
    }

    pub(crate) fn has_banned(&self, target: PeerIndex) -> Option<(Duration, String)> {
        self.banned_peers()
            .borrow()
            .iter()
            .find(|(peer, _, _)| *peer == target)
            .map(|(_, duration, reason)| (*duration, reason.clone()))
    }

    pub(crate) fn not_banned(&self, target: PeerIndex) -> bool {
        self.has_banned(target)
            .map(|(_, reason)| {
                eprintln!("banned reason is {reason}");
                false
            })
            .unwrap_or(true)
    }

    pub(crate) fn context(&self) -> Arc<dyn CKBProtocolContext + Sync> {
        Arc::clone(&self.inner) as Arc<dyn CKBProtocolContext + Sync>
    }
}

#[async_trait]
impl CKBProtocolContext for MockProtocolContext {
    fn ckb2023(&self) -> bool {
        false
    }
    async fn set_notify(&self, _interval: Duration, _token: u64) -> Result<(), Error> {
        // NOTE: no need to mock this function, just call protocol.notity(token) in
        // test code to test the functionality of the protocol.
        unimplemented!()
    }
    async fn remove_notify(&self, _token: u64) -> Result<(), Error> {
        unimplemented!()
    }
    async fn async_quick_send_message(
        &self,
        _proto_id: ProtocolId,
        _peer_index: PeerIndex,
        _data: P2pBytes,
    ) -> Result<(), Error> {
        unimplemented!();
    }
    async fn async_quick_send_message_to(
        &self,
        _peer_index: PeerIndex,
        _data: P2pBytes,
    ) -> Result<(), Error> {
        unimplemented!();
    }
    async fn async_quick_filter_broadcast(
        &self,
        _target: TargetSession,
        _data: P2pBytes,
    ) -> Result<(), Error> {
        unimplemented!();
    }
    async fn async_future_task(
        &self,
        _task: Pin<Box<dyn Future<Output = ()> + 'static + Send>>,
        _blocking: bool,
    ) -> Result<(), Error> {
        Ok(())
    }
    async fn async_send_message(
        &self,
        proto_id: ProtocolId,
        peer_index: PeerIndex,
        data: P2pBytes,
    ) -> Result<(), Error> {
        self.send_message(proto_id, peer_index, data)
    }
    async fn async_send_message_to(
        &self,
        peer_index: PeerIndex,
        data: P2pBytes,
    ) -> Result<(), Error> {
        let protocol_id = self.protocol_id();
        self.send_message(protocol_id, peer_index, data)
    }
    fn quick_send_message(
        &self,
        proto_id: ProtocolId,
        peer_index: PeerIndex,
        data: P2pBytes,
    ) -> Result<(), Error> {
        self.send_message(proto_id, peer_index, data)
    }
    fn quick_send_message_to(&self, peer_index: PeerIndex, data: P2pBytes) -> Result<(), Error> {
        let protocol_id = self.protocol_id();
        self.send_message(protocol_id, peer_index, data)
    }

    async fn async_filter_broadcast(
        &self,
        _target: TargetSession,
        _data: P2pBytes,
    ) -> Result<(), Error> {
        unimplemented!();
    }
    async fn async_disconnect(&self, _peer_index: PeerIndex, _message: &str) -> Result<(), Error> {
        unimplemented!();
    }
    fn quick_filter_broadcast(&self, _target: TargetSession, _data: P2pBytes) -> Result<(), Error> {
        unimplemented!();
    }
    fn future_task(
        &self,
        _task: Pin<Box<dyn Future<Output = ()> + 'static + Send>>,
        _blocking: bool,
    ) -> Result<(), Error> {
        Ok(())
    }
    fn send_message(
        &self,
        proto_id: ProtocolId,
        peer_index: PeerIndex,
        data: P2pBytes,
    ) -> Result<(), Error> {
        self.sent_messages
            .borrow_mut()
            .push((proto_id, peer_index, data));
        Ok(())
    }
    fn send_message_to(&self, peer_index: PeerIndex, data: P2pBytes) -> Result<(), Error> {
        let protocol_id = self.protocol_id();
        self.send_message(protocol_id, peer_index, data)
    }

    fn filter_broadcast(&self, _target: TargetSession, _data: P2pBytes) -> Result<(), Error> {
        unimplemented!();
    }
    fn disconnect(&self, peer_index: PeerIndex, _message: &str) -> Result<(), Error> {
        self.connected_peers.borrow_mut().remove(&peer_index);
        Ok(())
    }
    fn get_peer(&self, _peer_index: PeerIndex) -> Option<Peer> {
        unimplemented!();
    }
    fn with_peer_mut(&self, _peer_index: PeerIndex, _f: Box<dyn FnOnce(&mut Peer)>) {
        unimplemented!();
    }
    fn connected_peers(&self) -> Vec<PeerIndex> {
        self.connected_peers.borrow().iter().cloned().collect()
    }
    fn report_peer(&self, _peer_index: PeerIndex, _behaviour: Behaviour) {
        unimplemented!();
    }
    fn ban_peer(&self, peer_index: PeerIndex, duration: Duration, reason: String) {
        self.banned_peers
            .borrow_mut()
            .push((peer_index, duration, reason));
    }
    fn protocol_id(&self) -> ProtocolId {
        self.protocol.protocol_id()
    }
}
