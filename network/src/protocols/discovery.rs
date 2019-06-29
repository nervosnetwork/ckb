// use crate::peer_store::Behaviour;
use crate::peer_store::types::PeerAddr;
use crate::NetworkState;
use ckb_logger::{debug, error, trace, warn};
use fnv::FnvHashMap;
use futures::{sync::mpsc, sync::oneshot, Async, Future, Stream};
use std::{sync::Arc, time::Duration};

use p2p::{
    context::{ProtocolContext, ProtocolContextMutRef},
    multiaddr::{multihash::Multihash, Multiaddr, Protocol},
    secio::PeerId,
    traits::ServiceProtocol,
    utils::{extract_peer_id, is_reachable, multiaddr_to_socketaddr},
    SessionId,
};
use p2p_discovery::{
    AddressManager, Discovery, DiscoveryHandle, MisbehaveResult, Misbehavior, Substream,
};

pub struct DiscoveryProtocol {
    discovery: Option<Discovery<DiscoveryAddressManager>>,
    discovery_handle: DiscoveryHandle,
    discovery_senders: FnvHashMap<SessionId, mpsc::Sender<Vec<u8>>>,
    event_sender: mpsc::UnboundedSender<DiscoveryEvent>,
}

impl DiscoveryProtocol {
    pub fn new(event_sender: mpsc::UnboundedSender<DiscoveryEvent>) -> DiscoveryProtocol {
        let addr_mgr = DiscoveryAddressManager {
            event_sender: event_sender.clone(),
        };
        let discovery = Discovery::new(addr_mgr, Some(Duration::from_secs(7)));
        let discovery_handle = discovery.handle();
        DiscoveryProtocol {
            discovery: Some(discovery),
            discovery_handle,
            discovery_senders: FnvHashMap::default(),
            event_sender,
        }
    }
}

impl ServiceProtocol for DiscoveryProtocol {
    fn init(&mut self, context: &mut ProtocolContext) {
        debug!("protocol [discovery({})]: init", context.proto_id);

        let discovery_task = self
            .discovery
            .take()
            .map(|discovery| {
                debug!("Start discovery future_task");
                discovery
                    .for_each(|()| Ok(()))
                    .map_err(|err| {
                        warn!("discovery stream error: {:?}", err);
                    })
                    .then(|_| {
                        debug!("End of discovery");
                        Ok(())
                    })
            })
            .expect("Discovery init only once");
        if let Err(err) = context.future_task(discovery_task) {
            error!("Start discovery_task failed: {:?}", err);
        }
    }

    fn connected(&mut self, context: ProtocolContextMutRef, _: &str) {
        let session = context.session;
        debug!(
            "protocol [discovery] open on session [{}], address: [{}], type: [{:?}]",
            session.id, session.address, session.ty
        );
        let event = DiscoveryEvent::Connected {
            session_id: session.id,
            peer_id: session.remote_pubkey.clone().map(|pubkey| pubkey.peer_id()),
        };
        if self.event_sender.unbounded_send(event).is_err() {
            debug!("receiver maybe dropped! (ServiceProtocol::connected)");
            return;
        }

        let (sender, receiver) = mpsc::channel(8);
        self.discovery_senders.insert(session.id, sender);
        let substream = Substream::new(context, receiver);
        match self.discovery_handle.substream_sender.try_send(substream) {
            Ok(_) => {
                debug!("Send substream success");
            }
            Err(err) => {
                // TODO: handle channel is full (wait for poll API?)
                warn!("Send substream failed : {:?}", err);
            }
        }
    }

    fn disconnected(&mut self, context: ProtocolContextMutRef) {
        let session = context.session;
        let event = DiscoveryEvent::Disconnected(session.id);
        if self.event_sender.unbounded_send(event).is_err() {
            debug!("receiver maybe dropped! (ServiceProtocol::disconnected)");
            return;
        }
        self.discovery_senders.remove(&session.id);
        debug!("protocol [discovery] close on session [{}]", session.id);
    }

    fn received(&mut self, context: ProtocolContextMutRef, data: bytes::Bytes) {
        let session = context.session;
        trace!("[received message]: length={}", data.len());

        if let Some(ref mut sender) = self.discovery_senders.get_mut(&session.id) {
            // TODO: handle channel is full (wait for poll API?)
            if let Err(err) = sender.try_send(data.to_vec()) {
                if err.is_full() {
                    warn!("channel is full");
                } else if err.is_disconnected() {
                    warn!("channel is disconnected");
                } else {
                    warn!("other channel error: {:?}", err);
                }
                self.discovery_senders.remove(&session.id);
            }
        }
    }
}

pub enum DiscoveryEvent {
    Connected {
        session_id: SessionId,
        peer_id: Option<PeerId>,
    },
    Disconnected(SessionId),
    AddNewAddrs {
        session_id: SessionId,
        addrs: Vec<Multiaddr>,
    },
    Misbehave {
        session_id: SessionId,
        kind: Misbehavior,
        result: oneshot::Sender<MisbehaveResult>,
    },
    GetRandom {
        n: usize,
        result: oneshot::Sender<Vec<Multiaddr>>,
    },
}

pub struct DiscoveryService {
    event_receiver: mpsc::UnboundedReceiver<DiscoveryEvent>,
    network_state: Arc<NetworkState>,
    sessions: FnvHashMap<SessionId, PeerId>,
    discovery_local_address: bool,
}

impl DiscoveryService {
    pub fn new(
        network_state: Arc<NetworkState>,
        event_receiver: mpsc::UnboundedReceiver<DiscoveryEvent>,
        discovery_local_address: bool,
    ) -> DiscoveryService {
        DiscoveryService {
            event_receiver,
            network_state,
            sessions: FnvHashMap::default(),
            discovery_local_address,
        }
    }

    fn is_valid_addr(&self, addr: &Multiaddr) -> bool {
        if !self.discovery_local_address {
            let local_or_invalid = multiaddr_to_socketaddr(&addr)
                .map(|socket_addr| !is_reachable(socket_addr.ip()))
                .unwrap_or(true);
            !local_or_invalid
        } else {
            true
        }
    }

    fn handle_event(&mut self, event: DiscoveryEvent) {
        match event {
            DiscoveryEvent::Connected {
                session_id,
                peer_id,
            } => {
                if let Some(peer_id) = peer_id {
                    self.sessions.insert(session_id, peer_id);
                }
            }
            DiscoveryEvent::Disconnected(session_id) => {
                self.sessions.remove(&session_id);
            }
            DiscoveryEvent::AddNewAddrs { session_id, addrs } => {
                if let Some(_peer_id) = self.sessions.get(&session_id) {
                    // TODO: wait for peer store update
                    for addr in addrs.into_iter().filter(|addr| self.is_valid_addr(addr)) {
                        trace!("Add discovered address:{:?}", addr);
                        if let Some(peer_id) = extract_peer_id(&addr) {
                            self.network_state.with_peer_store_mut(|peer_store| {
                                peer_store.add_discovered_addr(&peer_id, addr);
                            });
                        }
                    }
                }
            }
            DiscoveryEvent::Misbehave {
                session_id: _session_id,
                kind: _kind,
                result: _result,
            } => {
                // FIXME:
            }
            DiscoveryEvent::GetRandom { n, result } => {
                let random_peers = self
                    .network_state
                    .with_peer_store(|peer_store| peer_store.random_peers(n as u32));
                let addrs = random_peers
                    .into_iter()
                    .filter_map(|paddr| {
                        let PeerAddr {
                            mut addr, peer_id, ..
                        } = paddr;
                        if !self.is_valid_addr(&addr) {
                            return None;
                        }
                        Multihash::from_bytes(peer_id.into_bytes())
                            .ok()
                            .map(move |peer_id_hash| {
                                addr.push(Protocol::P2p(peer_id_hash));
                                addr
                            })
                    })
                    .collect();
                trace!("discovery send random addrs: {:?}", addrs);
                result
                    .send(addrs)
                    .expect("Send failed (should not happened)");
            }
        }
    }
}

impl Future for DiscoveryService {
    type Item = ();
    type Error = ();
    fn poll(&mut self) -> Result<Async<Self::Item>, Self::Error> {
        loop {
            match self.event_receiver.poll()? {
                Async::Ready(Some(event)) => {
                    self.handle_event(event);
                }
                Async::Ready(None) => {
                    debug!("discovery service shutdown");
                    return Ok(Async::Ready(()));
                }
                Async::NotReady => break,
            }
        }
        Ok(Async::NotReady)
    }
}

pub struct DiscoveryAddressManager {
    pub event_sender: mpsc::UnboundedSender<DiscoveryEvent>,
}

impl AddressManager for DiscoveryAddressManager {
    fn add_new_addr(&mut self, session_id: SessionId, addr: Multiaddr) {
        self.add_new_addrs(session_id, vec![addr])
    }

    fn add_new_addrs(&mut self, session_id: SessionId, addrs: Vec<Multiaddr>) {
        if addrs.is_empty() {
            return;
        }
        let event = DiscoveryEvent::AddNewAddrs { session_id, addrs };
        if self.event_sender.unbounded_send(event).is_err() {
            debug!("receiver maybe dropped! (DiscoveryAddressManager::add_new_addrs)");
        }
    }

    fn misbehave(&mut self, session_id: SessionId, kind: Misbehavior) -> MisbehaveResult {
        let (sender, receiver) = oneshot::channel();
        let event = DiscoveryEvent::Misbehave {
            session_id,
            kind,
            result: sender,
        };
        if self.event_sender.unbounded_send(event).is_err() {
            debug!("receiver maybe dropped! (DiscoveryAddressManager::misbehave)");
            MisbehaveResult::Disconnect
        } else {
            receiver.wait().unwrap_or(MisbehaveResult::Disconnect)
        }
    }

    fn get_random(&mut self, n: usize) -> Vec<Multiaddr> {
        let (sender, receiver) = oneshot::channel();
        let event = DiscoveryEvent::GetRandom { n, result: sender };
        if self.event_sender.unbounded_send(event).is_err() {
            debug!("receiver maybe dropped! (DiscoveryAddressManager::get_random)");
            Vec::new()
        } else {
            receiver.wait().ok().unwrap_or_else(Vec::new)
        }
    }
}
