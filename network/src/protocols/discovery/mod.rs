use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use ckb_logger::{debug, error, trace, warn};
use ckb_types::bytes::BytesMut;
use p2p::{
    bytes,
    context::{ProtocolContext, ProtocolContextMutRef},
    multiaddr::Multiaddr,
    traits::ServiceProtocol,
    utils::{extract_peer_id, is_reachable, multiaddr_to_socketaddr},
    SessionId,
};
use rand::seq::SliceRandom;
use tokio_util::codec::{Decoder, Encoder};

pub use self::{
    addr::{AddrKnown, AddressManager, MisbehaveResult, Misbehavior, RawAddr},
    protocol::{DiscoveryMessage, Node, Nodes},
    state::SessionState,
};
use self::{protocol::DiscoveryCodec, state::RemoteAddress};
use crate::{NetworkState, ProtocolId};

mod addr;
mod protocol;
mod state;

const CHECK_INTERVAL: Duration = Duration::from_secs(3);
const ANNOUNCE_THRESHOLD: usize = 10;
// The maximum number of new addresses to accumulate before announcing.
const MAX_ADDR_TO_SEND: usize = 1000;
// The maximum number addresses in on Nodes item
const MAX_ADDRS: usize = 3;
// Every 24 hours send announce nodes message
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(3600 * 24);

pub struct DiscoveryProtocol<M> {
    codec: DiscoveryCodec,
    sessions: HashMap<SessionId, SessionState>,
    dynamic_query_cycle: Option<Duration>,
    addr_mgr: M,

    check_interval: Option<Duration>,
}

impl<M: AddressManager> DiscoveryProtocol<M> {
    pub fn new(addr_mgr: M) -> DiscoveryProtocol<M> {
        DiscoveryProtocol {
            codec: DiscoveryCodec::default(),
            sessions: HashMap::default(),
            dynamic_query_cycle: Some(Duration::from_secs(7)),
            check_interval: None,
            addr_mgr,
        }
    }
}

impl<M: AddressManager> ServiceProtocol for DiscoveryProtocol<M> {
    fn init(&mut self, context: &mut ProtocolContext) {
        debug!("protocol [discovery({})]: init", context.proto_id);
        if context
            .set_service_notify(
                context.proto_id,
                self.check_interval.unwrap_or(CHECK_INTERVAL),
                0,
            )
            .is_err()
        {
            debug!("set discovery notify fail")
        }
    }

    fn connected(&mut self, context: ProtocolContextMutRef, version: &str) {
        let session = context.session;
        debug!(
            "protocol [discovery] open on session [{}], address: [{}], type: [{:?}]",
            session.id, session.address, session.ty
        );

        self.addr_mgr
            .register(session.id, context.proto_id, version);

        self.sessions
            .insert(session.id, SessionState::new(context, &mut self.codec));
    }

    fn disconnected(&mut self, context: ProtocolContextMutRef) {
        let session = context.session;
        if let Some(remove_state) = self.sessions.remove(&session.id) {
            if let RemoteAddress::Listen(maddr) = remove_state.remote_addr {
                if let Some(addr) = multiaddr_to_socketaddr(&maddr) {
                    self.sessions
                        .values_mut()
                        .for_each(|state| state.addr_known.remove(&addr.into()))
                }
            }
        }
        self.addr_mgr.unregister(session.id, context.proto_id);
        debug!("protocol [discovery] close on session [{}]", session.id);
    }

    fn received(&mut self, context: ProtocolContextMutRef, data: bytes::Bytes) {
        let session = context.session;
        trace!("[received message]: length={}", data.len());

        match self.codec.decode(&mut BytesMut::from(data.as_ref())) {
            Ok(Some(item)) => {
                match item {
                    DiscoveryMessage::GetNodes { listen_port, .. } => {
                        if let Some(state) = self.sessions.get_mut(&session.id) {
                            if state.received_get_nodes
                                && self
                                    .addr_mgr
                                    .misbehave(session.id, Misbehavior::DuplicateGetNodes)
                                    .is_disconnect()
                            {
                                if context.disconnect(session.id).is_err() {
                                    debug!("disconnect {:?} send fail", session.id)
                                }
                                return;
                            }

                            state.received_get_nodes = true;
                            // must get the item first, otherwise it is possible to load
                            // the address of peer listen.
                            let mut items = self.addr_mgr.get_random(2500);

                            // change client random outbound port to client listen port
                            debug!("listen port: {:?}", listen_port);
                            if let Some(port) = listen_port {
                                state.remote_addr.update_port(port);
                                if let Some(raw_addr) = state.remote_raw_addr() {
                                    state.addr_known.insert(raw_addr);
                                }
                                // add client listen address to manager
                                if let RemoteAddress::Listen(ref addr) = state.remote_addr {
                                    self.addr_mgr.add_new_addr(session.id, addr.clone());
                                }
                            }

                            while items.len() > 1000 {
                                if let Some(last_item) = items.pop() {
                                    let idx = rand::random::<usize>() % 1000;
                                    items[idx] = last_item;
                                }
                            }

                            let items_clone = items
                                .iter()
                                .filter_map(|addr| {
                                    if let Some(addr) = multiaddr_to_socketaddr(addr) {
                                        Some(RawAddr::from(addr))
                                    } else {
                                        None
                                    }
                                })
                                .collect();
                            state.addr_known.extend(items_clone);

                            let items = items
                                .into_iter()
                                .map(|addr| Node {
                                    addresses: vec![addr],
                                })
                                .collect::<Vec<_>>();

                            let nodes = Nodes {
                                announce: false,
                                items,
                            };

                            let mut msg = BytesMut::new();
                            self.codec
                                .encode(DiscoveryMessage::Nodes(nodes), &mut msg)
                                .expect("encode must be success");
                            if context.send_message(msg.freeze()).is_err() {
                                debug!("{:?} send discovery msg Nodes fail", session.id)
                            }
                        }
                    }
                    DiscoveryMessage::Nodes(nodes) => {
                        for item in &nodes.items {
                            if item.addresses.len() > MAX_ADDRS {
                                let misbehavior =
                                    Misbehavior::TooManyAddresses(item.addresses.len());
                                if self
                                    .addr_mgr
                                    .misbehave(session.id, misbehavior)
                                    .is_disconnect()
                                {
                                    if context.disconnect(session.id).is_err() {
                                        debug!("disconnect {:?} send fail", session.id)
                                    }
                                    return;
                                }
                            }
                        }

                        if let Some(state) = self.sessions.get_mut(&session.id) {
                            if nodes.announce {
                                if nodes.items.len() > ANNOUNCE_THRESHOLD {
                                    warn!("Nodes items more than {}", ANNOUNCE_THRESHOLD);
                                    let misbehavior = Misbehavior::TooManyItems {
                                        announce: nodes.announce,
                                        length: nodes.items.len(),
                                    };
                                    if self
                                        .addr_mgr
                                        .misbehave(session.id, misbehavior)
                                        .is_disconnect()
                                    {
                                        if context.disconnect(session.id).is_err() {
                                            debug!("disconnect {:?} send fail", session.id)
                                        }
                                        return;
                                    }
                                }

                                let addrs = nodes
                                    .items
                                    .into_iter()
                                    .flat_map(|node| node.addresses.into_iter())
                                    .collect::<Vec<_>>();

                                let items_clone = addrs
                                    .iter()
                                    .filter_map(|addr| {
                                        if let Some(addr) = multiaddr_to_socketaddr(addr) {
                                            Some(RawAddr::from(addr))
                                        } else {
                                            None
                                        }
                                    })
                                    .collect();
                                state.addr_known.extend(items_clone);

                                self.addr_mgr.add_new_addrs(session.id, addrs);
                                return;
                            }

                            if state.received_nodes {
                                warn!("already received Nodes(announce=false) message");
                                if self
                                    .addr_mgr
                                    .misbehave(session.id, Misbehavior::DuplicateFirstNodes)
                                    .is_disconnect()
                                {
                                    if context.disconnect(session.id).is_err() {
                                        debug!("disconnect {:?} send fail", session.id)
                                    }
                                    return;
                                }
                            }

                            if nodes.items.len() > MAX_ADDR_TO_SEND {
                                warn!(
                                    "Too many items (announce=false) length={}",
                                    nodes.items.len()
                                );
                                let misbehavior = Misbehavior::TooManyItems {
                                    announce: nodes.announce,
                                    length: nodes.items.len(),
                                };

                                if self
                                    .addr_mgr
                                    .misbehave(session.id, misbehavior)
                                    .is_disconnect()
                                {
                                    if context.disconnect(session.id).is_err() {
                                        debug!("disconnect {:?} send fail", session.id)
                                    }
                                    return;
                                }
                            }

                            let addrs = nodes
                                .items
                                .into_iter()
                                .flat_map(|node| node.addresses.into_iter())
                                .collect::<Vec<_>>();

                            let items_clone = addrs
                                .iter()
                                .filter_map(|addr| {
                                    if let Some(addr) = multiaddr_to_socketaddr(addr) {
                                        Some(RawAddr::from(addr))
                                    } else {
                                        None
                                    }
                                })
                                .collect();
                            state.addr_known.extend(items_clone);
                            state.received_nodes = true;

                            self.addr_mgr.add_new_addrs(session.id, addrs);
                        }
                    }
                }
            }
            Ok(None) => (),
            Err(_) => {
                if self
                    .addr_mgr
                    .misbehave(session.id, Misbehavior::InvalidData)
                    .is_disconnect()
                    && context.disconnect(session.id).is_err()
                {
                    debug!("disconnect {:?} send fail", session.id)
                }
            }
        }
    }

    fn notify(&mut self, context: &mut ProtocolContext, _token: u64) {
        let now = Instant::now();

        let codec = &mut self.codec;
        let dynamic_query_cycle = self.dynamic_query_cycle.unwrap_or(ANNOUNCE_INTERVAL);
        let addr_mgr = &self.addr_mgr;

        // get announce list
        let announce_list: Vec<_> = self
            .sessions
            .iter_mut()
            .filter_map(|(id, state)| {
                // send all announce addr to remote
                state.send_messages(context, *id, codec);
                // check timer
                state.check_timer(now, dynamic_query_cycle);

                if state.announce {
                    state.announce = false;
                    state.last_announce = Some(now);
                    if let RemoteAddress::Listen(addr) = &state.remote_addr {
                        if addr_mgr.is_valid_addr(addr) {
                            Some(addr.clone())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        if !announce_list.is_empty() {
            let mut rng = rand::thread_rng();
            let mut remain_keys = self.sessions.keys().cloned().collect::<Vec<_>>();
            for announce_multiaddr in announce_list {
                let raw_addr = if let Some(addr) = multiaddr_to_socketaddr(&announce_multiaddr) {
                    RawAddr::from(addr)
                } else {
                    continue;
                };
                remain_keys.shuffle(&mut rng);
                for i in 0..2 {
                    if let Some(key) = remain_keys.get(i) {
                        if let Some(value) = self.sessions.get_mut(key) {
                            trace!(
                                ">> send {} to: {:?}, contains: {}",
                                announce_multiaddr,
                                value.remote_addr,
                                value.addr_known.contains(&raw_addr)
                            );
                            if value.announce_multiaddrs.len() < 10
                                && !value.addr_known.contains(&raw_addr)
                            {
                                value.announce_multiaddrs.push(announce_multiaddr.clone());
                                value.addr_known.insert(raw_addr);
                            }
                        }
                    } else {
                        break;
                    }
                }
            }
        }
    }
}

pub struct DiscoveryAddressManager {
    pub network_state: Arc<NetworkState>,
    pub discovery_local_address: bool,
}

impl AddressManager for DiscoveryAddressManager {
    // Register open ping protocol
    fn register(&self, id: SessionId, pid: ProtocolId, version: &str) {
        self.network_state.with_peer_registry_mut(|reg| {
            reg.get_peer_mut(id).map(|peer| {
                peer.protocols.insert(pid, version.to_owned());
            })
        });
    }

    // remove registered ping protocol
    fn unregister(&self, id: SessionId, pid: ProtocolId) {
        self.network_state.with_peer_registry_mut(|reg| {
            let _ = reg.get_peer_mut(id).map(|peer| {
                peer.protocols.remove(&pid);
            });
        });
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

    fn add_new_addr(&mut self, session_id: SessionId, addr: Multiaddr) {
        self.add_new_addrs(session_id, vec![addr])
    }

    fn add_new_addrs(&mut self, _session_id: SessionId, addrs: Vec<Multiaddr>) {
        if addrs.is_empty() {
            return;
        }

        for addr in addrs.into_iter().filter(|addr| self.is_valid_addr(addr)) {
            trace!("Add discovered address:{:?}", addr);
            if let Some(peer_id) = extract_peer_id(&addr) {
                self.network_state.with_peer_store_mut(|peer_store| {
                    if let Err(err) = peer_store.add_addr(peer_id.clone(), addr) {
                        debug!(
                            "Failed to add discoved address to peer_store {:?} {:?}",
                            err, peer_id
                        );
                    }
                });
            }
        }
    }

    fn misbehave(&mut self, _session_id: SessionId, _kind: Misbehavior) -> MisbehaveResult {
        // FIXME:
        MisbehaveResult::Disconnect
    }

    fn get_random(&mut self, n: usize) -> Vec<Multiaddr> {
        let fetch_random_addrs = self
            .network_state
            .with_peer_store_mut(|peer_store| peer_store.fetch_random_addrs(n));
        let addrs = fetch_random_addrs
            .into_iter()
            .filter_map(|paddr| {
                if !self.is_valid_addr(&paddr.addr) {
                    return None;
                }
                match paddr.multiaddr() {
                    Ok(addr) => Some(addr),
                    Err(err) => {
                        error!("return discovery addresses error: {:?}", err);
                        None
                    }
                }
            })
            .collect();
        trace!("discovery send random addrs: {:?}", addrs);
        addrs
    }
}
