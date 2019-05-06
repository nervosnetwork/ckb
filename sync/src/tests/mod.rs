use bytes::Bytes;
use ckb_network::{
    Behaviour, CKBProtocolContext, CKBProtocolHandler, Peer, PeerIndex, ProtocolId, TargetSession,
};
use ckb_util::RwLock;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[cfg(not(disable_faketime))]
mod relayer;
#[cfg(not(disable_faketime))]
mod synchronizer;

#[derive(Default)]
struct TestNode {
    pub peers: Vec<PeerIndex>,
    pub protocols: HashMap<ProtocolId, Arc<RwLock<CKBProtocolHandler + Send + Sync>>>,
    pub msg_senders: HashMap<(ProtocolId, PeerIndex), Sender<Vec<u8>>>,
    pub msg_receivers: HashMap<(ProtocolId, PeerIndex), Receiver<Vec<u8>>>,
    pub timer_senders: HashMap<(ProtocolId, u64), Sender<()>>,
    pub timer_receivers: HashMap<(ProtocolId, u64), Receiver<()>>,
}

impl TestNode {
    pub fn add_protocol(
        &mut self,
        protocol: ProtocolId,
        handler: &Arc<RwLock<CKBProtocolHandler + Send + Sync>>,
        timers: &[u64],
    ) {
        self.protocols.insert(protocol, Arc::clone(handler));
        timers.iter().for_each(|timer| {
            let (timer_sender, timer_receiver) = channel();
            self.timer_senders.insert((protocol, *timer), timer_sender);
            self.timer_receivers
                .insert((protocol, *timer), timer_receiver);
        });

        handler.write().init(Box::new(TestNetworkContext {
            protocol,
            msg_senders: self.msg_senders.clone(),
            timer_senders: self.timer_senders.clone(),
        }))
    }

    pub fn connect(&mut self, remote: &mut TestNode, protocol: ProtocolId) {
        let (local_sender, local_receiver) = channel();
        let local_index = self.peers.len();
        self.peers.insert(local_index, local_index.into());
        self.msg_senders
            .insert((protocol, local_index.into()), local_sender);

        let (remote_sender, remote_receiver) = channel();
        let remote_index = remote.peers.len();
        remote.peers.insert(remote_index, remote_index.into());
        remote
            .msg_senders
            .insert((protocol, remote_index.into()), remote_sender);

        self.msg_receivers
            .insert((protocol, remote_index.into()), remote_receiver);
        remote
            .msg_receivers
            .insert((protocol, local_index.into()), local_receiver);

        if let Some(handler) = self.protocols.get(&protocol) {
            handler.write().connected(
                Box::new(TestNetworkContext {
                    protocol,
                    msg_senders: self.msg_senders.clone(),
                    timer_senders: self.timer_senders.clone(),
                }),
                local_index.into(),
                "v1",
            )
        }

        if let Some(handler) = remote.protocols.get(&protocol) {
            handler.write().connected(
                Box::new(TestNetworkContext {
                    protocol,
                    msg_senders: remote.msg_senders.clone(),
                    timer_senders: remote.timer_senders.clone(),
                }),
                local_index.into(),
                "v1",
            )
        }
    }

    pub fn start<F: Fn(&[u8]) -> bool>(&self, signal: &Sender<()>, pred: F) {
        loop {
            for ((protocol, peer), receiver) in &self.msg_receivers {
                let _ = receiver.try_recv().map(|payload| {
                    if let Some(handler) = self.protocols.get(protocol) {
                        handler.write().received(
                            Box::new(TestNetworkContext {
                                protocol: *protocol,
                                msg_senders: self.msg_senders.clone(),
                                timer_senders: self.timer_senders.clone(),
                            }),
                            *peer,
                            Bytes::from(payload.clone()),
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
                        handler.write().notify(
                            Box::new(TestNetworkContext {
                                protocol: *protocol,
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

    pub fn broadcast(&self, protocol: ProtocolId, msg: &[u8]) {
        self.msg_senders
            .iter()
            .for_each(|((protocol_id, _), sender)| {
                if *protocol_id == protocol {
                    let _ = sender.send(msg.to_vec());
                }
            })
    }
}

struct TestNetworkContext {
    protocol: ProtocolId,
    msg_senders: HashMap<(ProtocolId, PeerIndex), Sender<Vec<u8>>>,
    timer_senders: HashMap<(ProtocolId, u64), Sender<()>>,
}

impl CKBProtocolContext for TestNetworkContext {
    // Interact with underlying p2p service
    fn set_notify(&self, interval: Duration, token: u64) {
        if let Some(sender) = self.timer_senders.get(&(self.protocol, token)) {
            let sender = sender.clone();
            thread::spawn(move || loop {
                thread::sleep(interval);
                let _ = sender.send(());
            });
        }
    }
    fn send_message_to(&self, peer_index: PeerIndex, data: Vec<u8>) {
        if let Some(sender) = self.msg_senders.get(&(self.protocol, peer_index)) {
            let _ = sender.send(data);
        }
    }
    fn filter_broadcast(&self, _target: TargetSession, _data: Vec<u8>) {
        unimplemented!();
    }
    fn disconnect(&self, _peer_index: PeerIndex) {}
    // Interact with NetworkState
    fn get_peer(&self, _peer_index: PeerIndex) -> Option<Peer> {
        None
    }
    fn connected_peers(&self) -> Vec<PeerIndex> {
        self.msg_senders.keys().map(|k| k.1).collect::<Vec<_>>()
    }
    fn report_peer(&self, _peer_index: PeerIndex, _behaviour: Behaviour) {}
    fn ban_peer(&self, _peer_index: PeerIndex, _timeout: Duration) {}
    // Other methods
    fn protocol_id(&self) -> ProtocolId {
        self.protocol
    }
}
