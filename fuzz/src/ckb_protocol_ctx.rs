use ckb_network::{async_trait, bytes::Bytes, PeerIndex, ProtocolId};
use futures::Future;
use std::{pin::Pin, time::Duration};

pub struct EmptyProtocolCtx {
    pub protocol: ProtocolId,
}

#[async_trait]
impl ckb_network::CKBProtocolContext for EmptyProtocolCtx {
    fn ckb2023(&self) -> bool {
        false
    }
    async fn set_notify(&self, _interval: Duration, _token: u64) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    /// Remove notify
    async fn remove_notify(&self, _token: u64) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    /// Send message through quick queue
    async fn async_quick_send_message(
        &self,
        _proto_id: ProtocolId,
        _peer_index: PeerIndex,
        _data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    /// Send message through quick queue
    async fn async_quick_send_message_to(
        &self,
        _peer_index: PeerIndex,
        _data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    /// Filter broadcast message through quick queue
    async fn async_quick_filter_broadcast(
        &self,
        _target: ckb_network::TargetSession,
        _data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    /// spawn a future task, if `blocking` is true we use tokio_threadpool::blocking to handle the task.
    async fn async_future_task(
        &self,
        _task: Pin<Box<dyn Future<Output = ()> + 'static + Send>>,
        _blocking: bool,
    ) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    /// Send message
    async fn async_send_message(
        &self,
        _proto_id: ProtocolId,
        _peer_index: PeerIndex,
        _data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    /// Send message
    async fn async_send_message_to(
        &self,
        _peer_index: PeerIndex,
        _data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    /// Filter broadcast message
    async fn async_filter_broadcast(
        &self,
        _target: ckb_network::TargetSession,
        _data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    /// Disconnect session
    async fn async_disconnect(
        &self,
        _peer_index: PeerIndex,
        _message: &str,
    ) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    /// Send message through quick queue
    fn quick_send_message(
        &self,
        _proto_id: ProtocolId,
        _peer_index: PeerIndex,
        _data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    /// Send message through quick queue
    fn quick_send_message_to(
        &self,
        _peer_index: PeerIndex,
        _data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    /// Filter broadcast message through quick queue
    fn quick_filter_broadcast(
        &self,
        _target: ckb_network::TargetSession,
        _data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    /// spawn a future task, if `blocking` is true we use tokio_threadpool::blocking to handle the task.
    fn future_task(
        &self,
        _task: Pin<Box<dyn Future<Output = ()> + 'static + Send>>,
        _blocking: bool,
    ) -> Result<(), ckb_network::Error> {
        //        task.await.expect("resolve future task ckb_network::Error");
        Ok(())
    }
    /// Send message
    fn send_message(
        &self,
        _proto_id: ProtocolId,
        _peer_index: PeerIndex,
        _data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    /// Send message
    fn send_message_to(
        &self,
        _peer_index: PeerIndex,
        _data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    /// Filter broadcast message
    fn filter_broadcast(
        &self,
        _target: ckb_network::TargetSession,
        _data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    /// Disconnect session
    fn disconnect(&self, _peer_index: PeerIndex, _message: &str) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    // Interact with NetworkState
    /// Get peer info
    fn get_peer(&self, _peer_index: PeerIndex) -> Option<ckb_network::Peer> {
        None
    }
    /// Modify peer info
    fn with_peer_mut(&self, _peer_index: PeerIndex, _f: Box<dyn FnOnce(&mut ckb_network::Peer)>) {}
    /// Get all session id
    fn connected_peers(&self) -> Vec<PeerIndex> {
        Vec::new()
    }
    /// Report peer behavior
    fn report_peer(&self, _peer_index: PeerIndex, _behaviour: ckb_network::Behaviour) {}
    /// Ban peer
    fn ban_peer(&self, _peer_index: PeerIndex, _duration: Duration, _reason: String) {}
    /// current protocol id
    fn protocol_id(&self) -> ProtocolId {
        self.protocol
    }
}
