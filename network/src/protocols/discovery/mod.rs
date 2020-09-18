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

pub use self::{
    addr::{AddrKnown, AddressManager, MisbehaveResult, Misbehavior},
    protocol::{DiscoveryMessage, Node, Nodes},
    state::SessionState,
};
use self::{
    protocol::{decode, encode},
    state::RemoteAddress,
};
use crate::{NetworkState, ProtocolId};

mod addr;
mod protocol;
mod state;

const CHECK_INTERVAL: Duration = Duration::from_secs(3);
const ANNOUNCE_THRESHOLD: usize = 10;
// The maximum number of new addresses to accumulate before announcing.
const MAX_ADDR_TO_SEND: usize = 1000;
// The maximum number addresses in one Nodes item
const MAX_ADDRS: usize = 3;
// Every 24 hours send announce nodes message
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(3600 * 24);

pub struct DiscoveryProtocol<M> {
    sessions: HashMap<SessionId, SessionState>,
    dynamic_query_cycle: Option<Duration>,
    addr_mgr: M,

    check_interval: Option<Duration>,
}

impl<M: AddressManager> DiscoveryProtocol<M> {
    pub fn new(addr_mgr: M) -> DiscoveryProtocol<M> {
        DiscoveryProtocol {
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
        context
            .set_service_notify(
                context.proto_id,
                self.check_interval.unwrap_or(CHECK_INTERVAL),
                0,
            )
            .expect("set discovery notify fail")
    }

    fn connected(&mut self, context: ProtocolContextMutRef, version: &str) {
        let session = context.session;
        debug!(
            "protocol [discovery] open on session [{}], address: [{}], type: [{:?}]",
            session.id, session.address, session.ty
        );

        self.addr_mgr
            .register(session.id, context.proto_id, version);

        self.sessions.insert(session.id, SessionState::new(context));
    }

    fn disconnected(&mut self, context: ProtocolContextMutRef) {
        let session = context.session;
        self.sessions.remove(&session.id);
        self.addr_mgr.unregister(session.id, context.proto_id);
        debug!("protocol [discovery] close on session [{}]", session.id);
    }

    fn received(&mut self, context: ProtocolContextMutRef, data: bytes::Bytes) {
        let session = context.session;
        trace!("[received message]: length={}", data.len());

        let mgr = &mut self.addr_mgr;
        let mut check = |behavior| -> bool {
            if mgr.misbehave(session.id, behavior).is_disconnect() {
                if context.disconnect(session.id).is_err() {
                    debug!("disconnect {:?} send fail", session.id)
                }
                true
            } else {
                false
            }
        };

        match decode(&mut BytesMut::from(data.as_ref())) {
            Some(item) => {
                match item {
                    DiscoveryMessage::GetNodes {
                        listen_port, count, ..
                    } => {
                        if let Some(state) = self.sessions.get_mut(&session.id) {
                            if state.received_get_nodes && check(Misbehavior::DuplicateGetNodes) {
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
                                state.addr_known.insert(state.remote_addr.to_inner());
                                // add client listen address to manager
                                if let RemoteAddress::Listen(ref addr) = state.remote_addr {
                                    self.addr_mgr.add_new_addr(session.id, addr.clone());
                                }
                            }

                            let max = ::std::cmp::max(MAX_ADDR_TO_SEND, count as usize);
                            if items.len() > max {
                                items = items
                                    .choose_multiple(&mut rand::thread_rng(), max)
                                    .cloned()
                                    .collect();
                            }

                            state.addr_known.extend(items.iter());

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

                            let msg = encode(DiscoveryMessage::Nodes(nodes));
                            if context.send_message(msg).is_err() {
                                debug!("{:?} send discovery msg Nodes fail", session.id)
                            }
                        }
                    }
                    DiscoveryMessage::Nodes(nodes) => {
                        if let Some(misbehavior) = verify_nodes_message(&nodes) {
                            if check(misbehavior) {
                                return;
                            }
                        }

                        if let Some(state) = self.sessions.get_mut(&session.id) {
                            if !nodes.announce && state.received_nodes {
                                warn!("already received Nodes(announce=false) message");
                                if check(Misbehavior::DuplicateFirstNodes) {
                                    return;
                                }
                            } else {
                                let addrs = nodes
                                    .items
                                    .into_iter()
                                    .flat_map(|node| node.addresses.into_iter())
                                    .collect::<Vec<_>>();

                                state.addr_known.extend(addrs.iter());
                                // Non-announce nodes can only receive once
                                // Due to the uncertainty of the other party’s state,
                                // the announce node may be sent out first, and it must be
                                // determined to be Non-announce before the state can be changed
                                if !nodes.announce {
                                    state.received_nodes = true;
                                }
                                self.addr_mgr.add_new_addrs(session.id, addrs);
                            }
                        }
                    }
                }
            }
            None => {
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

        let dynamic_query_cycle = self.dynamic_query_cycle.unwrap_or(ANNOUNCE_INTERVAL);
        let addr_mgr = &self.addr_mgr;

        // get announce list
        let announce_list: Vec<_> = self
            .sessions
            .iter_mut()
            .filter_map(|(id, state)| {
                // send all announce addr to remote
                state.send_messages(context, *id);
                // check timer
                state
                    .check_timer(now, dynamic_query_cycle)
                    .filter(|addr| addr_mgr.is_valid_addr(addr))
                    .cloned()
            })
            .collect();

        if !announce_list.is_empty() {
            let mut rng = rand::thread_rng();
            let mut keys = self.sessions.keys().cloned().collect::<Vec<_>>();
            for announce_multiaddr in announce_list {
                keys.shuffle(&mut rng);
                for key in keys.iter().take(3) {
                    if let Some(value) = self.sessions.get_mut(key) {
                        trace!(
                            ">> send {} to: {:?}, contains: {}",
                            announce_multiaddr,
                            value.remote_addr,
                            value.addr_known.contains(&announce_multiaddr)
                        );
                        if value.announce_multiaddrs.len() < ANNOUNCE_THRESHOLD
                            && !value.addr_known.contains(&announce_multiaddr)
                        {
                            value.announce_multiaddrs.push(announce_multiaddr.clone());
                            value.addr_known.insert(&announce_multiaddr);
                        }
                    }
                }
            }
        }
    }
}

fn verify_nodes_message(nodes: &Nodes) -> Option<Misbehavior> {
    let mut misbehavior = None;
    if nodes.announce {
        if nodes.items.len() > ANNOUNCE_THRESHOLD {
            warn!("Nodes items more than {}", ANNOUNCE_THRESHOLD);
            misbehavior = Some(Misbehavior::TooManyItems {
                announce: nodes.announce,
                length: nodes.items.len(),
            });
        }
    } else if nodes.items.len() > MAX_ADDR_TO_SEND {
        warn!(
            "Too many items (announce=false) length={}",
            nodes.items.len()
        );
        misbehavior = Some(Misbehavior::TooManyItems {
            announce: nodes.announce,
            length: nodes.items.len(),
        });
    }

    if misbehavior.is_none() {
        for item in &nodes.items {
            if item.addresses.len() > MAX_ADDRS {
                misbehavior = Some(Misbehavior::TooManyAddresses(item.addresses.len()));
                break;
            }
        }
    }

    misbehavior
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
