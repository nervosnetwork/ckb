use ckb_channel::{bounded, Receiver, Select, Sender};
use ckb_network::{
    bytes::Bytes, Behaviour, CKBProtocolContext, CKBProtocolHandler, Peer, PeerIndex, ProtocolId,
    TargetSession,
};
use ckb_util::RwLock;
use futures::future::Future;
use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

mod block_status;
mod inflight_blocks;
mod net_time_checker;
mod orphan_block_pool;
mod sync_shared;
#[cfg(not(disable_faketime))]
mod synchronizer;
mod types;
mod util;

const DEFAULT_CHANNEL: usize = 128;

enum Msg {
    Bytes(Bytes),
    Empty,
}

#[derive(Hash, Clone, PartialEq, Eq)]
enum Index {
    Msg(ProtocolId, PeerIndex),
    Timer(ProtocolId, u64),
    Stop,
}

struct TestNode {
    pub peers: Vec<PeerIndex>,
    pub protocols: HashMap<ProtocolId, Arc<RwLock<dyn CKBProtocolHandler + Send + Sync>>>,
    pub senders: HashMap<Index, Sender<Msg>>,
    pub receivers: HashMap<Index, Receiver<Msg>>,
    pub stop: Sender<Msg>,
    pub th: Option<JoinHandle<()>>,
}

impl TestNode {
    pub fn new() -> TestNode {
        let (stop_tx, stop_rx) = bounded(1);
        let mut receivers = HashMap::new();
        receivers.insert(Index::Stop, stop_rx);

        TestNode {
            receivers,
            senders: HashMap::new(),
            protocols: HashMap::new(),
            peers: Vec::new(),
            stop: stop_tx,
            th: None,
        }
    }

    pub fn add_protocol(
        &mut self,
        protocol: ProtocolId,
        handler: &Arc<RwLock<dyn CKBProtocolHandler + Send + Sync>>,
        timers: &[u64],
    ) {
        self.protocols.insert(protocol, Arc::clone(handler));
        timers.iter().for_each(|timer| {
            let (timer_sender, timer_receiver) = bounded(DEFAULT_CHANNEL);
            let index = Index::Timer(protocol, *timer);
            self.senders.insert(index.clone(), timer_sender);
            self.receivers.insert(index, timer_receiver);
        });

        handler.write().init(Arc::new(TestNetworkContext {
            protocol,
            senders: self.senders.clone(),
        }))
    }

    pub fn connect(&mut self, remote: &mut TestNode, protocol: ProtocolId) {
        let (local_sender, local_receiver) = bounded(DEFAULT_CHANNEL);
        let local_index = self.peers.len();
        self.peers.insert(local_index, local_index.into());
        let local_ch_index = Index::Msg(protocol, local_index.into());
        self.senders.insert(local_ch_index.clone(), local_sender);

        let (remote_sender, remote_receiver) = bounded(DEFAULT_CHANNEL);
        let remote_index = remote.peers.len();
        remote.peers.insert(remote_index, remote_index.into());

        let remote_ch_index = Index::Msg(protocol, local_index.into());
        remote
            .senders
            .insert(remote_ch_index.clone(), remote_sender);
        self.receivers.insert(remote_ch_index, remote_receiver);

        remote.receivers.insert(local_ch_index, local_receiver);

        if let Some(handler) = self.protocols.get(&protocol) {
            handler.write().connected(
                Arc::new(TestNetworkContext {
                    protocol,
                    senders: self.senders.clone(),
                }),
                local_index.into(),
                "v1",
            )
        }

        if let Some(handler) = remote.protocols.get(&protocol) {
            handler.write().connected(
                Arc::new(TestNetworkContext {
                    protocol,
                    senders: remote.senders.clone(),
                }),
                local_index.into(),
                "v1",
            )
        }
    }

    pub fn start<F: Fn(Bytes) -> bool + Send + 'static>(
        &mut self,
        thread_name: String,
        signal: Sender<()>,
        pred: F,
    ) {
        let receivers: Vec<_> = self
            .receivers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let protocols = self.protocols.clone();
        let senders = self.senders.clone();

        let th = thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                let mut sel = Select::new();
                for r in &receivers {
                    sel.recv(&r.1);
                }
                loop {
                    let index = sel.ready();
                    let (index, rv) = &receivers[index];
                    let res = rv.try_recv();

                    match index {
                        Index::Msg(protocol, peer) => {
                            if let Ok(Msg::Bytes(payload)) = res {
                                if let Some(handler) = protocols.get(protocol) {
                                    handler.write().received(
                                        Arc::new(TestNetworkContext {
                                            protocol: *protocol,
                                            senders: senders.clone(),
                                        }),
                                        *peer,
                                        payload.clone(),
                                    )
                                };

                                if pred(payload) {
                                    let _ = signal.send(());
                                }
                            }
                        }
                        Index::Timer(protocol, timer) => {
                            if let Some(handler) = protocols.get(protocol) {
                                handler.write().notify(
                                    Arc::new(TestNetworkContext {
                                        protocol: *protocol,
                                        senders: senders.clone(),
                                    }),
                                    *timer,
                                )
                            }
                        }
                        Index::Stop => {
                            break;
                        }
                    };
                }
            })
            .expect("thread spawn");

        self.th = Some(th);
    }

    pub fn stop(mut self) {
        self.stop.send(Msg::Empty).expect("stop recv");
        if let Some(th) = self.th.take() {
            th.join().expect("th join");
        }
    }
}

struct TestNetworkContext {
    protocol: ProtocolId,
    senders: HashMap<Index, Sender<Msg>>,
}

impl CKBProtocolContext for TestNetworkContext {
    fn ckb2021(&self) -> bool {
        false
    }
    // Interact with underlying p2p service
    fn set_notify(&self, interval: Duration, token: u64) -> Result<(), ckb_network::Error> {
        let index = Index::Timer(self.protocol, token);
        if let Some(sender) = self.senders.get(&index) {
            let sender = sender.clone();
            thread::spawn(move || loop {
                thread::sleep(interval);
                let _ = sender.send(Msg::Empty);
            });
        }
        Ok(())
    }

    fn remove_notify(&self, _token: u64) -> Result<(), ckb_network::Error> {
        Ok(())
    }

    fn future_task(
        &self,
        _task: Pin<Box<dyn Future<Output = ()> + 'static + Send>>,
        _blocking: bool,
    ) -> Result<(), ckb_network::Error> {
        //        task.await.expect("resolve future task error");
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
        let index = Index::Msg(proto_id, peer_index);
        if let Some(sender) = self.senders.get(&index) {
            let _ = sender.send(Msg::Bytes(data));
        }
        Ok(())
    }
    fn send_message_to(
        &self,
        peer_index: PeerIndex,
        data: Bytes,
    ) -> Result<(), ckb_network::Error> {
        let index = Index::Msg(self.protocol, peer_index);
        if let Some(sender) = self.senders.get(&index) {
            let _ = sender.send(Msg::Bytes(data));
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
            TargetSession::Filter(peers) => {
                for peer in self
                    .senders
                    .keys()
                    .filter_map(|index| match index {
                        Index::Msg(_, id) => Some(id),
                        _ => None,
                    })
                    .copied()
                    .collect::<HashSet<PeerIndex>>()
                {
                    if peers(&peer) {
                        self.send_message_to(peer, data.clone()).unwrap();
                    }
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
        self.senders
            .keys()
            .filter_map(|index| match index {
                Index::Msg(_, peer_id) => Some(*peer_id),
                _ => None,
            })
            .collect::<Vec<_>>()
    }
    fn report_peer(&self, _peer_index: PeerIndex, _behaviour: Behaviour) {}
    fn ban_peer(&self, _peer_index: PeerIndex, _duration: Duration, _reason: String) {}
    // Other methods
    fn protocol_id(&self) -> ProtocolId {
        self.protocol
    }
}
