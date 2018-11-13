use network::{
    Error as NetworkError, NetworkContext, NetworkProtocolHandler, PacketId, PeerId, ProtocolId,
    SessionInfo, Severity, TimerToken,
};
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

mod relayer;
mod synchronizer;

#[derive(Default)]
struct TestNode {
    pub peers: Vec<PeerId>,
    pub protocols: HashMap<ProtocolId, Arc<NetworkProtocolHandler + Send + Sync>>,
    pub msg_senders: HashMap<(ProtocolId, PeerId), Sender<Vec<u8>>>,
    pub msg_receivers: HashMap<(ProtocolId, PeerId), Receiver<Vec<u8>>>,
    pub timer_senders: HashMap<(ProtocolId, TimerToken), Sender<()>>,
    pub timer_receivers: HashMap<(ProtocolId, TimerToken), Receiver<()>>,
}

impl TestNode {
    pub fn add_protocol(
        &mut self,
        protocol: ProtocolId,
        handler: Arc<NetworkProtocolHandler + Send + Sync>,
        timers: Vec<TimerToken>,
    ) {
        self.protocols.insert(protocol, Arc::clone(&handler));
        timers.iter().for_each(|timer| {
            let (timer_sender, timer_receiver) = channel();
            self.timer_senders.insert((protocol, *timer), timer_sender);
            self.timer_receivers
                .insert((protocol, *timer), timer_receiver);
        });

        handler.initialize(Box::new(TestNetworkContext {
            protocol,
            current_peer: None,
            msg_senders: self.msg_senders.clone(),
            timer_senders: self.timer_senders.clone(),
        }))
    }

    pub fn connect(&mut self, remote: &mut TestNode, protocol: ProtocolId) {
        let (local_sender, local_receiver) = channel();
        let local_index = self.peers.len();
        self.peers.insert(local_index, local_index);
        self.msg_senders
            .insert((protocol, local_index), local_sender);

        let (remote_sender, remote_receiver) = channel();
        let remote_index = remote.peers.len();
        remote.peers.insert(remote_index, remote_index);
        remote
            .msg_senders
            .insert((protocol, remote_index), remote_sender);

        self.msg_receivers
            .insert((protocol, remote_index), remote_receiver);
        remote
            .msg_receivers
            .insert((protocol, local_index), local_receiver);

        if let Some(handler) = self.protocols.get(&protocol) {
            handler.connected(
                Box::new(TestNetworkContext {
                    protocol,
                    current_peer: Some(local_index),
                    msg_senders: self.msg_senders.clone(),
                    timer_senders: self.timer_senders.clone(),
                }),
                &local_index,
            )
        }
    }

    pub fn start<F: Fn(&[u8]) -> bool>(&self, signal: Sender<()>, pred: F) {
        loop {
            for ((protocol, peer), receiver) in &self.msg_receivers {
                let _ = receiver.try_recv().map(|payload| {
                    if let Some(handler) = self.protocols.get(protocol) {
                        handler.read(
                            Box::new(TestNetworkContext {
                                protocol: *protocol,
                                current_peer: Some(*peer),
                                msg_senders: self.msg_senders.clone(),
                                timer_senders: self.timer_senders.clone(),
                            }),
                            &peer,
                            0,
                            &payload,
                        )
                    };

                    if pred(&payload) {
                        let _ = signal.send(());
                    }
                });
            }

            for ((protocol, timer), receiver) in &self.timer_receivers {
                let _ = receiver.try_recv().map(|_| {
                    if let Some(handler) = self.protocols.get(protocol) {
                        handler.timeout(
                            Box::new(TestNetworkContext {
                                protocol: *protocol,
                                current_peer: None,
                                msg_senders: self.msg_senders.clone(),
                                timer_senders: self.timer_senders.clone(),
                            }),
                            *timer,
                        )
                    }
                });
            }
        }
    }

    pub fn broadcast(&self, protocol: ProtocolId, msg: Vec<u8>) {
        self.msg_senders
            .iter()
            .for_each(|((protocol_id, _), sender)| {
                if *protocol_id == protocol {
                    let _ = sender.send(msg.clone());
                }
            })
    }
}

struct TestNetworkContext {
    protocol: ProtocolId,
    current_peer: Option<PeerId>,
    msg_senders: HashMap<(ProtocolId, PeerId), Sender<Vec<u8>>>,
    timer_senders: HashMap<(ProtocolId, TimerToken), Sender<()>>,
}

impl NetworkContext for TestNetworkContext {
    fn send(&self, peer: PeerId, _packet_id: PacketId, data: Vec<u8>) {
        if let Some(sender) = self.msg_senders.get(&(self.protocol, peer)) {
            let _ = sender.send(data);
        }
    }

    /// Send a packet over the network to another peer using specified protocol.
    fn send_protocol(
        &self,
        _protocol: ProtocolId,
        _peer: PeerId,
        _packet_id: PacketId,
        _data: Vec<u8>,
    ) {
    }

    fn respond(&self, packet_id: PacketId, data: Vec<u8>) {
        self.send(self.current_peer.unwrap(), packet_id, data)
    }

    fn report_peer(&self, _peer: PeerId, _reason: Severity) {}

    fn is_expired(&self) -> bool {
        false
    }

    fn register_timer(&self, token: TimerToken, delay: Duration) -> Result<(), NetworkError> {
        if let Some(sender) = self.timer_senders.get(&(self.protocol, token)) {
            let sender = sender.clone();
            thread::spawn(move || loop {
                thread::sleep(delay);
                let _ = sender.send(());
            });
        }
        Ok(())
    }

    fn peer_client_version(&self, _peer: PeerId) -> String {
        "unknown".to_string()
    }

    fn session_info(&self, _peer: PeerId) -> Option<SessionInfo> {
        None
    }

    fn protocol_version(&self, _protocol: ProtocolId, _peer: PeerId) -> Option<u8> {
        None
    }

    fn connected_peers(&self) -> Vec<PeerIndex> {
        self.msg_senders.keys().map(|k| k.1).collect::<Vec<_>>()
    }
}

    fn subprotocol_name(&self) -> ProtocolId {
        [1, 1, 1]
    }
}
