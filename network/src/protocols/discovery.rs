// use crate::peer_store::Behaviour;
use crate::NetworkState;
use fnv::FnvHashMap;
use futures::{sync::mpsc, Future, Stream};
use log::{debug, trace, warn};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use p2p::{
    context::{ProtocolContext, ProtocolContextMutRef, SessionContext},
    multiaddr::{multihash::Multihash, Multiaddr, Protocol},
    traits::ServiceProtocol,
    utils::extract_peer_id,
    SessionId,
};
use p2p_discovery::{
    AddressManager, Discovery, DiscoveryHandle, MisbehaveResult, Misbehavior, Substream,
};

// Every 4 seconds check bootnode finish NewAddrs messages
const BOOTNODE_CHECK: Duration = Duration::from_secs(5);

pub struct DiscoveryProtocol {
    discovery: Option<Discovery<DiscoveryAddressManager>>,
    discovery_handle: DiscoveryHandle,
    discovery_senders: FnvHashMap<SessionId, mpsc::Sender<Vec<u8>>>,
    event_receiver: crossbeam_channel::Receiver<SessionId>,
    bootnodes: HashSet<Multiaddr>,
    sessions: FnvHashMap<SessionId, SessionContext>,
}

impl DiscoveryProtocol {
    pub fn new(network_state: Arc<NetworkState>, bootnodes: Vec<Multiaddr>) -> DiscoveryProtocol {
        let (event_sender, event_receiver) = crossbeam_channel::unbounded();
        let addr_mgr = DiscoveryAddressManager {
            network_state,
            event_sender,
        };
        let discovery = Discovery::new(addr_mgr, Some(Duration::from_secs(7)));
        let discovery_handle = discovery.handle();
        let bootnodes = bootnodes.into_iter().collect();
        DiscoveryProtocol {
            discovery: Some(discovery),
            discovery_handle,
            discovery_senders: FnvHashMap::default(),
            event_receiver,
            bootnodes,
            sessions: FnvHashMap::default(),
        }
    }
}

impl ServiceProtocol for DiscoveryProtocol {
    fn init(&mut self, context: &mut ProtocolContext) {
        let proto_id = context.proto_id;
        debug!(target: "network", "protocol [discovery({})]: init", proto_id);
        context.set_service_notify(proto_id, BOOTNODE_CHECK, 0);

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
        self.sessions.insert(session.id, session.clone());
        debug!(
            target: "network",
            "protocol [discovery] open on session [{}], address: [{}], type: [{:?}]",
            session.id, session.address, session.ty
        );
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
        self.sessions.remove(&session.id);
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

    fn notify(&mut self, context: &mut ProtocolContext, _token: u64) {
        // When received new addrs message from bootnode, disconnect it
        while let Ok(session_id) = self.event_receiver.try_recv() {
            if let Some(session_context) = self.sessions.get(&session_id) {
                if self.bootnodes.contains(&session_context.address) {
                    debug!(target: "network", "disconnect discovered bootnode: {}", session_context.address);
                    context.disconnect(session_id);
                }
            }
        }
    }
}

struct DiscoveryAddressManager {
    network_state: Arc<NetworkState>,
    event_sender: crossbeam_channel::Sender<SessionId>,
}

impl AddressManager for DiscoveryAddressManager {
    fn add_new_addr(&mut self, _session_id: SessionId, _addr: Multiaddr) {}

    fn add_new_addrs(&mut self, session_id: SessionId, addrs: Vec<Multiaddr>) {
        if let Err(err) = self.event_sender.try_send(session_id) {
            warn!(target: "network", "send message to DiscoveryProtocol failed: {:?}", err);
        }

        // TODO: wait for peer store update
        for addr in addrs.into_iter() {
            trace!(target: "network", "Add discovered address:{:?}", addr);
            if let Some(peer_id) = extract_peer_id(&addr) {
                let addr = addr
                    .into_iter()
                    .filter(|proto| match proto {
                        Protocol::P2p(_) => false,
                        _ => true,
                    })
                    .collect::<Multiaddr>();

                if !self
                    .network_state
                    .peer_store()
                    .add_discovered_addr(&peer_id, addr)
                {
                    warn!(target: "network", "add_discovered_addr failed {:?}", peer_id);
                }
            }
        }
    }

    fn misbehave(&mut self, _session_id: SessionId, _kind: Misbehavior) -> MisbehaveResult {
        // FIXME
        MisbehaveResult::Disconnect
    }

    fn get_random(&mut self, n: usize) -> Vec<Multiaddr> {
        let addrs = self
            .network_state
            .peer_store()
            .random_peers(n as u32)
            .into_iter()
            .filter_map(|(peer_id, mut addr)| {
                Multihash::from_bytes(peer_id.into_bytes())
                    .ok()
                    .map(move |peer_id_hash| {
                        addr.append(Protocol::P2p(peer_id_hash));
                        addr
                    })
            })
            .collect();
        trace!(target: "network", "discovery send random addrs: {:?}", addrs);
        addrs
    }
}
