use super::super::ProtocolEvent;
use crossbeam_channel as channel;
use network::{NetworkContext, NetworkProtocolHandler, PeerId, TimerToken};

pub struct TestProtocol {
    pub events: channel::Sender<ProtocolEvent>,
}

impl NetworkProtocolHandler for TestProtocol {
    /// Initialize the handler
    fn initialize(&self, _io: Box<NetworkContext>) {
        self.events.send(ProtocolEvent::Initialize);
    }

    /// Called when new network packet received.
    fn read(&self, io: Box<NetworkContext>, peer: &PeerId, _packet_id: u8, data: &[u8]) {
        if let Some(session_info) = io.session_info(*peer) {
            self.events.send(ProtocolEvent::Read(
                *peer,
                Box::new(session_info),
                data.len(),
            ));
        }
    }

    /// Called when new peer is connected. Only called when peer supports the same protocol.
    fn connected(&self, io: Box<NetworkContext>, peer: &PeerId) {
        if let Some(session_info) = io.session_info(*peer) {
            self.events
                .send(ProtocolEvent::Connected(*peer, Box::new(session_info)));
        }
    }

    /// Called when a previously connected peer disconnects.
    fn disconnected(&self, io: Box<NetworkContext>, peer: &PeerId) {
        info!("TestProtocol.disconnected(peer={:?})", peer);
        if let Some(session_info) = io.session_info(*peer) {
            self.events
                .send(ProtocolEvent::Disconnected(*peer, Box::new(session_info)));
        }
    }

    /// Timer function called after a timeout created with `NetworkContext::timeout`.
    fn timeout(&self, _io: Box<NetworkContext>, timer: TimerToken) {
        self.events.send(ProtocolEvent::Timeout(timer));
    }
}
