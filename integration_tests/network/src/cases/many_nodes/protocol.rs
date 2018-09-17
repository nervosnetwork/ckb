use super::super::ProtocolEvent;
use network::{NetworkContext, NetworkProtocolHandler, PeerId, TimerToken};
use parking_lot::RwLock;
use std::time::Instant;

pub struct TestProtocol {
    pub events: RwLock<Vec<(Instant, ProtocolEvent)>>,
}

impl NetworkProtocolHandler for TestProtocol {
    /// Initialize the handler
    fn initialize(&self, _io: Box<NetworkContext>) {
        self.events
            .write()
            .push((Instant::now(), ProtocolEvent::Initialize));
    }

    /// Called when new network packet received.
    fn read(&self, io: Box<NetworkContext>, peer: &PeerId, _packet_id: u8, data: &[u8]) {
        if let Some(session_info) = io.session_info(*peer) {
            self.events.write().push((
                Instant::now(),
                ProtocolEvent::Read(*peer, Box::new(session_info), data.len()),
            ));
        }
    }

    /// Called when new peer is connected. Only called when peer supports the same protocol.
    fn connected(&self, io: Box<NetworkContext>, peer: &PeerId) {
        if let Some(session_info) = io.session_info(*peer) {
            self.events.write().push((
                Instant::now(),
                ProtocolEvent::Connected(*peer, Box::new(session_info)),
            ));
        }
    }

    /// Called when a previously connected peer disconnects.
    fn disconnected(&self, io: Box<NetworkContext>, peer: &PeerId) {
        info!("TestProtocol.disconnected(peer={:?})", peer);
        if let Some(session_info) = io.session_info(*peer) {
            self.events.write().push((
                Instant::now(),
                ProtocolEvent::Disconnected(*peer, Box::new(session_info)),
            ));
        }
    }

    /// Timer function called after a timeout created with `NetworkContext::timeout`.
    fn timeout(&self, _io: Box<NetworkContext>, timer: TimerToken) {
        self.events
            .write()
            .push((Instant::now(), ProtocolEvent::Timeout(timer)));
    }
}
