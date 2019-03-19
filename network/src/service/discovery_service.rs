// use crate::peer_store::Behaviour;
use crate::Network;
use fnv::FnvHashMap;
use futures::{sync::mpsc, sync::oneshot, Async, Future, Stream};
use log::{debug, warn};
use std::sync::Arc;

use p2p::{
    context::{ServiceContext, SessionContext},
    multiaddr::Multiaddr,
    secio::PeerId,
    traits::ServiceProtocol,
    yamux::session::SessionType,
    ProtocolId, SessionId,
};
use p2p_discovery::{
    AddressManager, Direction, Discovery, DiscoveryHandle, MisbehaveResult, Misbehavior, Substream,
};

pub struct DiscoveryProtocol {
    id: ProtocolId,
    discovery: Option<Discovery<DiscoveryAddressManager>>,
    discovery_handle: DiscoveryHandle,
    discovery_senders: FnvHashMap<SessionId, mpsc::Sender<Vec<u8>>>,
    event_sender: mpsc::UnboundedSender<DiscoveryEvent>,
}

impl DiscoveryProtocol {
    pub fn new(
        id: ProtocolId,
        event_sender: mpsc::UnboundedSender<DiscoveryEvent>,
    ) -> DiscoveryProtocol {
        let addr_mgr = DiscoveryAddressManager {
            event_sender: event_sender.clone(),
        };
        let discovery = Discovery::new(addr_mgr);
        let discovery_handle = discovery.handle();
        DiscoveryProtocol {
            id,
            discovery: Some(discovery),
            discovery_handle,
            discovery_senders: FnvHashMap::default(),
            event_sender,
        }
    }
}

impl ServiceProtocol for DiscoveryProtocol {
    fn init(&mut self, control: &mut ServiceContext) {
        debug!(target: "network", "protocol [discovery({})]: init", self.id);

        let discovery_task = self
            .discovery
            .take()
            .map(|discovery| {
                debug!(target: "network", "Start discovery future_task");
                discovery
                    .for_each(|()| Ok(()))
                    .map_err(|err| {
                        warn!(target: "network", "discovery stream error: {:?}", err);
                    })
                    .then(|_| {
                        debug!(target: "network", "End of discovery");
                        Ok(())
                    })
            })
            .unwrap();
        control.future_task(discovery_task);
    }

    fn connected(&mut self, control: &mut ServiceContext, session: &SessionContext, _: &str) {
        debug!(
            target: "network",
            "protocol [discovery] open on session [{}], address: [{}], type: [{:?}]",
            session.id, session.address, session.ty
        );
        let event = DiscoveryEvent::Connected {
            session_id: session.id,
            peer_id: session.remote_pubkey.clone().map(|pubkey| pubkey.peer_id()),
        };
        if self.event_sender.unbounded_send(event).is_err() {
            warn!(target: "network", "receiver maybe dropped!");
            return;
        }

        let direction = if session.ty == SessionType::Server {
            Direction::Inbound
        } else {
            Direction::Outbound
        };
        let (sender, receiver) = mpsc::channel(8);
        self.discovery_senders.insert(session.id, sender);
        let substream = Substream::new(
            session.address.clone(),
            direction,
            self.id,
            session.id,
            receiver,
            control.control().clone(),
            control.listens(),
        );
        match self.discovery_handle.substream_sender.try_send(substream) {
            Ok(_) => {
                debug!(target: "network", "Send substream success");
            }
            Err(err) => {
                // TODO: handle channel is full (wait for poll API?)
                warn!(target: "network", "Send substream failed : {:?}", err);
            }
        }
    }

    fn disconnected(&mut self, _control: &mut ServiceContext, session: &SessionContext) {
        let event = DiscoveryEvent::Disconnected(session.id);
        if self.event_sender.unbounded_send(event).is_err() {
            warn!(target: "network", "receiver maybe dropped!");
            return;
        }
        self.discovery_senders.remove(&session.id);
        debug!(target: "network", "protocol [discovery] close on session [{}]", session.id);
    }

    fn received(
        &mut self,
        _control: &mut ServiceContext,
        session: &SessionContext,
        data: bytes::Bytes,
    ) {
        debug!(target: "network", "[received message]: length={}", data.len());

        if let Some(ref mut sender) = self.discovery_senders.get_mut(&session.id) {
            // TODO: handle channel is full (wait for poll API?)
            if let Err(err) = sender.try_send(data.to_vec()) {
                if err.is_full() {
                    warn!(target: "network", "channel is full");
                } else if err.is_disconnected() {
                    warn!(target: "network", "channel is disconnected");
                } else {
                    warn!(target: "network", "other channel error: {:?}", err);
                }
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
    AddNewAddr {
        session_id: SessionId,
        addr: Multiaddr,
    },
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
    network: Arc<Network>,
    sessions: FnvHashMap<SessionId, PeerId>,
}

impl DiscoveryService {
    pub fn new(
        network: Arc<Network>,
        event_receiver: mpsc::UnboundedReceiver<DiscoveryEvent>,
    ) -> DiscoveryService {
        DiscoveryService {
            event_receiver,
            network,
            sessions: FnvHashMap::default(),
        }
    }
}

impl Stream for DiscoveryService {
    type Item = ();
    type Error = ();
    fn poll(&mut self) -> Result<Async<Option<Self::Item>>, Self::Error> {
        match try_ready!(self.event_receiver.poll()) {
            Some(DiscoveryEvent::Connected {
                session_id,
                peer_id,
            }) => {
                if let Some(peer_id) = peer_id {
                    self.sessions.insert(session_id, peer_id);
                }
            }
            Some(DiscoveryEvent::Disconnected(session_id)) => {
                self.sessions.remove(&session_id);
            }
            Some(DiscoveryEvent::AddNewAddr { .. }) => {
                // NOTE: ignore add new addr message, handle this in identify protocol
            }
            Some(DiscoveryEvent::AddNewAddrs { session_id, addrs }) => {
                if let Some(peer_id) = self.sessions.get(&session_id) {
                    let _ = self
                        .network
                        .peer_store()
                        .write()
                        .add_discovered_addresses(peer_id, addrs);
                }
            }
            Some(DiscoveryEvent::Misbehave {
                session_id: _session_id,
                kind: _kind,
                result: _result,
            }) => {
                // FIXME:
            }
            Some(DiscoveryEvent::GetRandom { n, result }) => {
                let addrs = self
                    .network
                    .peer_store()
                    .read()
                    .peers_to_attempt(n as u32)
                    .into_iter()
                    .map(|(_peer_id, addr)| addr)
                    .collect();
                result
                    .send(addrs)
                    .expect("Send failed (should not happened)");
            }
            None => {
                debug!(target: "network", "discovery service shutdown");
                return Ok(Async::Ready(None));
            }
        }
        Ok(Async::Ready(Some(())))
    }
}

pub struct DiscoveryAddressManager {
    pub event_sender: mpsc::UnboundedSender<DiscoveryEvent>,
}

impl AddressManager for DiscoveryAddressManager {
    fn add_new_addr(&mut self, session_id: SessionId, addr: Multiaddr) {
        let event = DiscoveryEvent::AddNewAddr { session_id, addr };
        if self.event_sender.unbounded_send(event).is_err() {
            warn!(target: "network", "receiver maybe dropped!");
        }
    }

    fn add_new_addrs(&mut self, session_id: SessionId, addrs: Vec<Multiaddr>) {
        let event = DiscoveryEvent::AddNewAddrs { session_id, addrs };
        if self.event_sender.unbounded_send(event).is_err() {
            warn!(target: "network", "receiver maybe dropped!");
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
            warn!(target: "network", "receiver maybe dropped!");
            MisbehaveResult::Disconnect
        } else {
            receiver.wait().unwrap_or(MisbehaveResult::Disconnect)
        }
    }

    fn get_random(&mut self, n: usize) -> Vec<Multiaddr> {
        let (sender, receiver) = oneshot::channel();
        let event = DiscoveryEvent::GetRandom { n, result: sender };
        if self.event_sender.unbounded_send(event).is_err() {
            warn!(target: "network", "receiver maybe dropped!");
            Vec::new()
        } else {
            receiver.wait().ok().unwrap_or_else(Vec::new)
        }
    }
}
