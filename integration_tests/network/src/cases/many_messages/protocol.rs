use super::super::ProtocolEvent;
use network::{NetworkContext, NetworkProtocolHandler, PeerId, TimerToken};
use parking_lot::RwLock;
use std::collections::HashSet;
use std::time::{Duration, Instant};

pub struct TestProtocol {
    pub events: RwLock<Vec<(Instant, ProtocolEvent)>>,
    pub peers: RwLock<HashSet<PeerId>>,
    pub count: RwLock<u32>,
    pub stop: RwLock<bool>,
    pub timer: u64,
    pub send_msgs: u32,
}

impl NetworkProtocolHandler for TestProtocol {
    /// Initialize the handler
    fn initialize(&self, io: Box<NetworkContext>) {
        let _ = io.register_timer(666, Duration::from_millis(self.timer));
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
        self.peers.write().insert(*peer);
        if let Some(session_info) = io.session_info(*peer) {
            self.events.write().push((
                Instant::now(),
                ProtocolEvent::Connected(*peer, Box::new(session_info)),
            ));
        }
    }

    /// Called when a previously connected peer disconnects.
    fn disconnected(&self, io: Box<NetworkContext>, peer: &PeerId) {
        self.peers.write().remove(peer);
        if let Some(session_info) = io.session_info(*peer) {
            self.events.write().push((
                Instant::now(),
                ProtocolEvent::Disconnected(*peer, Box::new(session_info)),
            ));
        }
    }

    /// Timer function called after a timeout created with `NetworkContext::timeout`.
    fn timeout(&self, io: Box<NetworkContext>, timer: TimerToken) {
        self.events
            .write()
            .push((Instant::now(), ProtocolEvent::Timeout(timer)));
        if !*self.stop.read() {
            let mut count = self.count.write();
            for peer in self.peers.read().iter() {
                for i in 0..self.send_msgs {
                    io.send(*peer, 0, format!("message {:010}", *count + i).into_bytes());
                }
                *count += self.send_msgs;
            }
        }
    }
}
