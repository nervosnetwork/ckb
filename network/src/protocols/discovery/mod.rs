use std::{collections::HashMap, sync::Arc};

use ckb_logger::{debug, error, trace, warn};
use ckb_systemtime::{Duration, Instant};
use p2p::{
    async_trait, bytes,
    context::{ProtocolContext, ProtocolContextMutRef, SessionContext},
    multiaddr::Multiaddr,
    traits::ServiceProtocol,
    utils::{is_reachable, multiaddr_to_socketaddr},
    SessionId,
};
use rand::seq::SliceRandom;

pub use self::{
    addr::{AddressManager, MisbehaveResult, Misbehavior},
    protocol::{DiscoveryMessage, Node, Nodes},
    state::SessionState,
};
use self::{
    protocol::{decode, encode},
    state::RemoteAddress,
};
use crate::{Flags, NetworkState, ProtocolId};

mod addr;
pub(crate) mod protocol;
mod state;

const ANNOUNCE_CHECK_INTERVAL: Duration = Duration::from_secs(60);
const ANNOUNCE_THRESHOLD: usize = 10;
// The maximum number of new addresses to accumulate before announcing.
const MAX_ADDR_TO_SEND: usize = 1000;
// The maximum number addresses in one Nodes item
const MAX_ADDRS: usize = 3;
// Every 24 hours send announce nodes message
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(3600 * 24);

pub struct DiscoveryProtocol<M> {
    sessions: HashMap<SessionId, SessionState>,
    announce_check_interval: Option<Duration>,
    addr_mgr: M,
}

impl<M: AddressManager + Send> DiscoveryProtocol<M> {
    pub fn new(addr_mgr: M, announce_check_interval: Option<Duration>) -> DiscoveryProtocol<M> {
        DiscoveryProtocol {
            sessions: HashMap::default(),
            announce_check_interval,
            addr_mgr,
        }
    }
}

#[async_trait]
impl<M: AddressManager + Send + Sync> ServiceProtocol for DiscoveryProtocol<M> {
    async fn init(&mut self, context: &mut ProtocolContext) {
        debug!("protocol [discovery({})]: init", context.proto_id);
        context
            .set_service_notify(
                context.proto_id,
                self.announce_check_interval
                    .unwrap_or(ANNOUNCE_CHECK_INTERVAL),
                0,
            )
            .await
            .expect("set discovery notify fail")
    }

    async fn connected(&mut self, context: ProtocolContextMutRef<'_>, version: &str) {
        let session = context.session;
        debug!(
            "DiscoveryProtocol connected, session: {:?}, version: {}",
            session, version
        );

        self.addr_mgr
            .register(session.id, context.proto_id, version);

        self.sessions
            .insert(session.id, SessionState::new(context, &self.addr_mgr).await);
    }

    async fn disconnected(&mut self, context: ProtocolContextMutRef<'_>) {
        let session = context.session;
        self.sessions.remove(&session.id);
        self.addr_mgr.unregister(session.id, context.proto_id);
        debug!("DiscoveryProtocol disconnected, session {:?}", session);
    }

    async fn received(&mut self, context: ProtocolContextMutRef<'_>, data: bytes::Bytes) {
        let session = context.session;
        trace!("[received message]: length={}", data.len());

        let mgr = &mut self.addr_mgr;
        let mut check =
            |behavior: Misbehavior| -> bool { mgr.misbehave(session, &behavior).is_disconnect() };

        match decode(&data) {
            Some(item) => {
                match item {
                    DiscoveryMessage::GetNodes {
                        listen_port,
                        count,
                        version,
                        required_flags,
                    } => {
                        if let Some(state) = self.sessions.get_mut(&session.id) {
                            if state.received_get_nodes && check(Misbehavior::DuplicateGetNodes) {
                                if context.disconnect(session.id).await.is_err() {
                                    debug!("Disconnect {:?} msg failed to send", session.id)
                                }
                                return;
                            }

                            state.received_get_nodes = true;
                            // must get the item first, otherwise it is possible to load
                            // the address of peer listen.
                            let mut items = self.addr_mgr.get_random(2500, required_flags);

                            // change client random outbound port to client listen port
                            debug!("listen port: {:?}", listen_port);
                            if let Some(port) = listen_port {
                                state.remote_addr.update_port(port);
                                state.addr_known.insert(state.remote_addr.to_inner());
                                // add client listen address to manager
                                if let RemoteAddress::Listen(ref addr) = state.remote_addr {
                                    let flags = self.addr_mgr.node_flags(session.id);
                                    self.addr_mgr.add_new_addr(
                                        session.id,
                                        (addr.clone(), flags.unwrap_or(Flags::COMPATIBILITY)),
                                    );
                                }
                            }
                            if version >= state::REUSE_PORT_VERSION {
                                // after enable reuse port, it can be broadcast
                                state.remote_addr.change_to_listen();
                            }

                            let max = ::std::cmp::min(MAX_ADDR_TO_SEND, count as usize);
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
                                    addresses: vec![addr.0],
                                    flags: addr.1,
                                })
                                .collect::<Vec<_>>();

                            let nodes = Nodes {
                                announce: false,
                                items,
                            };

                            let msg = encode(DiscoveryMessage::Nodes(nodes));
                            if context.send_message(msg).await.is_err() {
                                debug!("{:?} send discovery msg Nodes fail", session.id)
                            }
                        }
                    }
                    DiscoveryMessage::Nodes(nodes) => {
                        if let Some(misbehavior) = verify_nodes_message(&nodes) {
                            if check(misbehavior) {
                                if context.disconnect(session.id).await.is_err() {
                                    debug!("Disconnect {:?} msg failed to send", session.id)
                                }
                                return;
                            }
                        }

                        if let Some(state) = self.sessions.get_mut(&session.id) {
                            if !nodes.announce && state.received_nodes {
                                warn!("Nodes (announce=false) message received");
                                if check(Misbehavior::DuplicateFirstNodes)
                                    && context.disconnect(session.id).await.is_err()
                                {
                                    debug!("Disconnect {:?} msg failed to send", session.id)
                                }
                            } else {
                                let addrs = nodes
                                    .items
                                    .into_iter()
                                    .flat_map(|node| {
                                        node.addresses.into_iter().map(move |a| (a, node.flags))
                                    })
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
                    .misbehave(session, &Misbehavior::InvalidData)
                    .is_disconnect()
                    && context.disconnect(session.id).await.is_err()
                {
                    debug!("Disconnect {:?} msg failed to send", session.id)
                }
            }
        }
    }

    async fn notify(&mut self, context: &mut ProtocolContext, _token: u64) {
        let now = Instant::now();
        // get announce list
        let mut announce_list = Vec::new();
        for (id, state) in self.sessions.iter_mut() {
            state.send_messages(context, *id).await;

            if let Some(addr) = state
                .check_timer(now, ANNOUNCE_INTERVAL)
                .filter(|addr| self.addr_mgr.is_valid_addr(addr))
            {
                if let Some(flags) = self.addr_mgr.node_flags(*id) {
                    announce_list.push((addr.clone(), flags));
                }
            }
        }

        if !announce_list.is_empty() {
            let mut rng = rand::thread_rng();
            let mut keys = self.sessions.keys().cloned().collect::<Vec<_>>();
            for announce_multiaddr in announce_list {
                keys.shuffle(&mut rng);
                for key in keys.iter().take(3) {
                    if let Some(value) = self.sessions.get_mut(key) {
                        trace!(
                            ">> send {:?} to: {:?}, containing: {}",
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
            warn!(
                "Number of nodes exceeds announce threshold {}",
                ANNOUNCE_THRESHOLD
            );
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
    // Register open discovery protocol
    fn register(&self, id: SessionId, pid: ProtocolId, version: &str) {
        self.network_state.with_peer_registry_mut(|reg| {
            reg.get_peer_mut(id).map(|peer| {
                peer.protocols.insert(pid, version.to_owned());
            })
        });
    }

    // remove registered discovery protocol
    fn unregister(&self, id: SessionId, pid: ProtocolId) {
        self.network_state.with_peer_registry_mut(|reg| {
            let _ = reg.get_peer_mut(id).map(|peer| {
                peer.protocols.remove(&pid);
            });
        });
    }

    fn is_valid_addr(&self, addr: &Multiaddr) -> bool {
        if !self.discovery_local_address {
            match multiaddr_to_socketaddr(addr) {
                Some(socket_addr) => is_reachable(socket_addr.ip()),
                None => true,
            }
        } else {
            true
        }
    }

    fn add_new_addr(&mut self, session_id: SessionId, addr: (Multiaddr, Flags)) {
        self.add_new_addrs(session_id, vec![addr])
    }

    fn add_new_addrs(&mut self, _session_id: SessionId, addrs: Vec<(Multiaddr, Flags)>) {
        if addrs.is_empty() {
            return;
        }

        for (addr, flags) in addrs.into_iter().filter(|addr| self.is_valid_addr(&addr.0)) {
            trace!("Add discovered address:{:?}", addr);
            self.network_state.with_peer_store_mut(|peer_store| {
                if let Err(err) = peer_store.add_addr(addr.clone(), flags) {
                    debug!(
                        "Failed to add discovered address to peer_store {:?} {:?}",
                        err, addr
                    );
                }
            });
        }
    }

    fn misbehave(&mut self, session: &SessionContext, behavior: &Misbehavior) -> MisbehaveResult {
        error!(
            "DiscoveryProtocol detects abnormal behavior, session: {:?}, behavior: {:?}",
            session, behavior
        );

        // FIXME:
        MisbehaveResult::Disconnect
    }

    fn get_random(&mut self, n: usize, flags: Flags) -> Vec<(Multiaddr, Flags)> {
        let fetch_random_addrs = self
            .network_state
            .with_peer_store_mut(|peer_store| peer_store.fetch_random_addrs(n, flags));
        let addrs = fetch_random_addrs
            .into_iter()
            .filter_map(|paddr| {
                if !self.is_valid_addr(&paddr.addr) {
                    return None;
                }
                let f = Flags::from_bits_truncate(paddr.flags);
                Some((paddr.addr, f))
            })
            .collect();
        trace!("Discovered random addrs: {:?}", addrs);
        addrs
    }

    fn required_flags(&self) -> super::identify::Flags {
        self.network_state.required_flags
    }

    fn node_flags(&self, id: SessionId) -> Option<Flags> {
        self.network_state.with_peer_registry(|reg| {
            reg.get_peer(id)
                .and_then(|peer| peer.identify_info.as_ref().map(|a| a.flags))
        })
    }
}
