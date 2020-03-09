use ckb_network::{
    bytes::Bytes, Behaviour, CKBProtocolContext, CKBProtocolHandler, Peer, PeerIndex, ProtocolId,
    TargetSession,
};
use ckb_util::RwLock;
use futures::future::Future;
use std::collections::HashMap;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

mod inflight_blocks;
mod sync_shared;
#[cfg(not(disable_faketime))]
mod synchronizer;
mod util;

const DEFAULT_CHANNEL: usize = 128;

#[derive(Default)]
struct TestNode {
    pub peers: Vec<PeerIndex>,
    pub protocols: HashMap<ProtocolId, Arc<RwLock<dyn CKBProtocolHandler + Send + Sync>>>,
    pub msg_senders: HashMap<(ProtocolId, PeerIndex), SyncSender<Bytes>>,
    pub msg_receivers: HashMap<(ProtocolId, PeerIndex), Receiver<Bytes>>,
    pub timer_senders: HashMap<(ProtocolId, u64), SyncSender<()>>,
    pub timer_receivers: HashMap<(ProtocolId, u64), Receiver<()>>,
}

impl TestNode {
    pub fn add_protocol(
        &mut self,
        protocol: ProtocolId,
        handler: &Arc<RwLock<dyn CKBProtocolHandler + Send + Sync>>,
        timers: &[u64],
    ) {
        self.protocols.insert(protocol, Arc::clone(handler));
        timers.iter().for_each(|timer| {
            let (timer_sender, timer_receiver) = sync_channel(DEFAULT_CHANNEL);
            self.timer_senders.insert((protocol, *timer), timer_sender);
            self.timer_receivers
                .insert((protocol, *timer), timer_receiver);
        });

        handler.write().init(Arc::new(TestNetworkContext {
            protocol,
            msg_senders: self.msg_senders.clone(),
            timer_senders: self.timer_senders.clone(),
        }))
    }

    pub fn connect(&mut self, remote: &mut TestNode, protocol: ProtocolId) {
        let (local_sender, local_receiver) = sync_channel(DEFAULT_CHANNEL);
        let local_index = self.peers.len();
        self.peers.insert(local_index, local_index.into());
        self.msg_senders
            .insert((protocol, local_index.into()), local_sender);

        let (remote_sender, remote_receiver) = sync_channel(DEFAULT_CHANNEL);
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
                Arc::new(TestNetworkContext {
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
                Arc::new(TestNetworkContext {
                    protocol,
                    msg_senders: remote.msg_senders.clone(),
                    timer_senders: remote.timer_senders.clone(),
                }),
                local_index.into(),
                "v1",
            )
        }
    }

    pub fn start<F: Fn(&[u8]) -> bool>(&self, signal: &SyncSender<()>, pred: F) {
        loop {
            for ((protocol, peer), receiver) in &self.msg_receivers {
                let _ = receiver.try_recv().map(|payload| {
                    if let Some(handler) = self.protocols.get(protocol) {
                        handler.write().received(
                            Arc::new(TestNetworkContext {
                                protocol: *protocol,
                                msg_senders: self.msg_senders.clone(),
                                timer_senders: self.timer_senders.clone(),
                            }),
                            *peer,
                            payload.clone(),
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
                            Arc::new(TestNetworkContext {
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
}

struct TestNetworkContext {
    protocol: ProtocolId,
    msg_senders: HashMap<(ProtocolId, PeerIndex), SyncSender<Bytes>>,
    timer_senders: HashMap<(ProtocolId, u64), SyncSender<()>>,
}

impl CKBProtocolContext for TestNetworkContext {
    // Interact with underlying p2p service
    fn set_notify(&self, interval: Duration, token: u64) -> Result<(), ckb_network::Error> {
        if let Some(sender) = self.timer_senders.get(&(self.protocol, token)) {
            let sender = sender.clone();
            thread::spawn(move || loop {
                thread::sleep(interval);
                let _ = sender.send(());
            });
        }
        Ok(())
    }

    fn remove_notify(&self, _token: u64) -> Result<(), ckb_network::Error> {
        Ok(())
    }

    fn future_task(
        &self,
        task: Box<
            (dyn futures::future::Future<Item = (), Error = ()> + std::marker::Send + 'static),
        >,
        _blocking: bool,
    ) -> Result<(), ckb_network::Error> {
        task.wait().expect("resolve future task error");
        Ok(())
    }

    fn quick_send_message(
        &self,
        proto_id: ProtocolId,
        peer_index: PeerIndex,
        data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        self.send_message(proto_id, peer_index, data)
    }
    fn quick_send_message_to(
        &self,
        peer_index: PeerIndex,
        data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        self.send_message_to(peer_index, data)
    }
    fn quick_filter_broadcast(
        &self,
        target: TargetSession,
        data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        self.filter_broadcast(target, data)
    }
    fn send_message(
        &self,
        proto_id: ProtocolId,
        peer_index: PeerIndex,
        data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        if let Some(sender) = self.msg_senders.get(&(proto_id, peer_index)) {
            let _ = sender.send(data);
        }
        Ok(())
    }
    fn send_message_to(
        &self,
        peer_index: PeerIndex,
        data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        if let Some(sender) = self.msg_senders.get(&(self.protocol, peer_index)) {
            let _ = sender.send(data);
        }
        Ok(())
    }
    fn filter_broadcast(
        &self,
        target: TargetSession,
        data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        match target {
            TargetSession::Single(peer) => self.send_message_to(peer, data).unwrap(),
            TargetSession::Multi(peers) => {
                for peer in peers {
                    self.send_message_to(peer, data.clone()).unwrap();
                }
            }
            TargetSession::All => {
                unimplemented!();
            }
        }
        Ok(())
    }
    fn disconnect(&self, _peer_index: PeerIndex, _msg: &str) -> Result<(), ckb_network::Error> {
        Ok(())
    }
    // Interact with NetworkState
    fn get_peer(&self, _peer_index: PeerIndex) -> Option<Peer> {
        None
    }
    fn with_peer_mut(&self, _peer_index: PeerIndex, _f: Box<dyn FnOnce(&mut Peer)>) {}
    fn connected_peers(&self) -> Vec<PeerIndex> {
        self.msg_senders.keys().map(|k| k.1).collect::<Vec<_>>()
    }
    fn report_peer(&self, _peer_index: PeerIndex, _behaviour: Behaviour) {}
    fn ban_peer(&self, _peer_index: PeerIndex, _duration: Duration, _reason: String) {}
    // Other methods
    fn protocol_id(&self) -> ProtocolId {
        self.protocol
    }
    fn send_paused(&self) -> bool {
        false
    }
}
