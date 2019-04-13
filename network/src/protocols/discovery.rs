// use crate::peer_store::Behaviour;
use crate::protocols::BackgroundService;
use crate::NetworkState;
use fnv::FnvHashMap;
use futures::{sync::mpsc, sync::oneshot, try_ready, Async, Future, Stream};
use log::{debug, error, trace, warn};
use std::{sync::Arc, time::Duration};

use p2p::{
    context::{ProtocolContext, ProtocolContextMutRef},
    multiaddr::{multihash::Multihash, Multiaddr, Protocol},
    secio::PeerId,
    traits::ServiceProtocol,
    utils::extract_peer_id,
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
        debug!(target: "network", "protocol [discovery({})]: init", context.proto_id);

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
            .expect("Discovery init only once");
        context.future_task(discovery_task);
    }

    fn connected(&mut self, context: ProtocolContextMutRef, _: &str) {
        let session = context.session;
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

        let (sender, receiver) = mpsc::channel(8);
        self.discovery_senders.insert(session.id, sender);
        let substream = Substream::new(context, receiver);
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

    fn disconnected(&mut self, context: ProtocolContextMutRef) {
        let session = context.session;
        let event = DiscoveryEvent::Disconnected(session.id);
        if self.event_sender.unbounded_send(event).is_err() {
            warn!(target: "network", "receiver maybe dropped!");
            return;
        }
        self.discovery_senders.remove(&session.id);
        debug!(target: "network", "protocol [discovery] close on session [{}]", session.id);
    }

    fn received(&mut self, context: ProtocolContextMutRef, data: bytes::Bytes) {
        let session = context.session;
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

pub struct DiscoveryAddressManager {
    pub event_sender: mpsc::UnboundedSender<DiscoveryEvent>,
}

impl AddressManager for DiscoveryAddressManager {
    fn add_new_addr(&mut self, _session_id: SessionId, _addr: Multiaddr) {}

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
