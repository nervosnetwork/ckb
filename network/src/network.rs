//! Global state struct and start function
use crate::errors::{Error, P2PError};
use crate::peer_registry::{ConnectionStatus, PeerRegistry};
use crate::peer_store::{
    types::{AddrInfo, BannedAddr},
    PeerStore,
};
use crate::protocols::{
    disconnect_message::DisconnectMessageProtocol,
    discovery::{DiscoveryAddressManager, DiscoveryProtocol},
    feeler::Feeler,
    identify::{IdentifyCallback, IdentifyProtocol},
    ping::PingHandler,
    support_protocols::SupportProtocols,
};
use crate::services::{
    dump_peer_store::DumpPeerStoreService, outbound_peer::OutboundPeerService,
    protocol_type_checker::ProtocolTypeCheckerService,
};
use crate::{Behaviour, CKBProtocol, Peer, PeerIndex, ProtocolId, ServiceControl};
use ckb_app_config::{NetworkConfig, SupportProtocol};
use ckb_logger::{debug, error, info, trace, warn};
use ckb_spawn::Spawn;
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_util::{Condvar, Mutex, RwLock};
use futures::{channel::mpsc::Sender, Future, StreamExt};
use ipnetwork::IpNetwork;
use p2p::{
    builder::ServiceBuilder,
    bytes::Bytes,
    context::{ServiceContext, SessionContext},
    error::{DialerErrorKind, HandshakeErrorKind, ProtocolHandleErrorKind, SendErrorKind},
    multiaddr::{Multiaddr, Protocol},
    secio::{self, error::SecioError, PeerId},
    service::{ProtocolHandle, Service, ServiceError, ServiceEvent, TargetProtocol, TargetSession},
    traits::ServiceHandle,
    utils::{extract_peer_id, is_reachable, multiaddr_to_socketaddr},
    yamux::config::Config as YamuxConfig,
    SessionId,
};
use rand::prelude::IteratorRandom;
#[cfg(feature = "with_sentry")]
use sentry::{capture_message, with_scope, Level};
use std::sync::mpsc;
use std::{
    borrow::Cow,
    cmp::max,
    collections::{HashMap, HashSet},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
    usize,
};
use tokio::{self, sync::oneshot};

const P2P_SEND_TIMEOUT: Duration = Duration::from_secs(6);
const P2P_TRY_SEND_INTERVAL: Duration = Duration::from_millis(100);
// After 5 minutes we consider this dial hang
const DIAL_HANG_TIMEOUT: Duration = Duration::from_secs(300);

/// The global shared state of the network module
pub struct NetworkState {
    pub(crate) peer_registry: RwLock<PeerRegistry>,
    pub(crate) peer_store: Mutex<PeerStore>,
    /// Node listened addresses
    pub(crate) listened_addrs: RwLock<Vec<Multiaddr>>,
    dialing_addrs: RwLock<HashMap<PeerId, Instant>>,
    /// Node public addresses,
    /// includes manually public addrs and remote peer observed addrs
    public_addrs: RwLock<HashSet<Multiaddr>>,
    pending_observed_addrs: RwLock<HashSet<Multiaddr>>,
    local_private_key: secio::SecioKeyPair,
    local_peer_id: PeerId,
    bootnodes: Vec<Multiaddr>,
    pub(crate) config: NetworkConfig,
    pub(crate) active: AtomicBool,
    /// Node supported protocols
    /// fields: ProtocolId, Protocol Name, Supported Versions
    pub(crate) protocols: RwLock<Vec<(ProtocolId, String, Vec<String>)>>,

    pub(crate) ckb2021: AtomicBool,
}

impl NetworkState {
    /// Init from config
    pub fn from_config(config: NetworkConfig) -> Result<NetworkState, Error> {
        config.create_dir_if_not_exists()?;
        let local_private_key = config.fetch_private_key()?;
        let local_peer_id = local_private_key.peer_id();
        // set max score to public addresses
        let public_addrs: HashSet<Multiaddr> = config
            .listen_addresses
            .iter()
            .chain(config.public_addresses.iter())
            .cloned()
            .filter_map(|mut addr| {
                multiaddr_to_socketaddr(&addr)
                    .filter(|addr| is_reachable(addr.ip()))
                    .and({
                        if extract_peer_id(&addr).is_none() {
                            addr.push(Protocol::P2P(Cow::Borrowed(local_peer_id.as_bytes())));
                        }
                        Some(addr)
                    })
            })
            .collect();
        let peer_store = Mutex::new(PeerStore::load_from_dir_or_default(
            config.peer_store_path(),
        ));
        let bootnodes = config.bootnodes();

        let peer_registry = PeerRegistry::new(
            config.max_inbound_peers(),
            config.max_outbound_peers(),
            config.whitelist_only,
            config.whitelist_peers(),
        );

        Ok(NetworkState {
            peer_store,
            config,
            bootnodes,
            peer_registry: RwLock::new(peer_registry),
            dialing_addrs: RwLock::new(HashMap::default()),
            public_addrs: RwLock::new(public_addrs),
            listened_addrs: RwLock::new(Vec::new()),
            pending_observed_addrs: RwLock::new(HashSet::default()),
            local_private_key,
            local_peer_id,
            active: AtomicBool::new(true),
            protocols: RwLock::new(Vec::new()),
            ckb2021: AtomicBool::new(false),
        })
    }

    /// fork flag
    pub fn ckb2021(self, init: bool) -> Self {
        self.ckb2021.store(init, Ordering::SeqCst);
        self
    }

    pub(crate) fn report_session(
        &self,
        p2p_control: &ServiceControl,
        session_id: SessionId,
        behaviour: Behaviour,
    ) {
        if let Some(addr) = self.with_peer_registry(|reg| {
            reg.get_peer(session_id)
                .filter(|peer| !peer.is_whitelist)
                .map(|peer| peer.connected_addr.clone())
        }) {
            trace!("report {:?} because {:?}", addr, behaviour);
            let report_result = match self.peer_store.lock().report(&addr, behaviour) {
                Ok(result) => result,
                Err(err) => {
                    error!(
                        "Report failed addr: {:?} behaviour: {:?} error: {:?}",
                        addr, behaviour, err
                    );
                    return;
                }
            };
            if report_result.is_banned() {
                if let Err(err) = disconnect_with_message(p2p_control, session_id, "banned") {
                    debug!("Disconnect failed {:?}, error: {:?}", session_id, err);
                }
            }
        } else {
            debug!(
                "Report {} failed: not in peer registry or it is in the whitelist",
                session_id
            );
        }
    }

    pub(crate) fn ban_session(
        &self,
        p2p_control: &ServiceControl,
        session_id: SessionId,
        duration: Duration,
        reason: String,
    ) {
        if let Some(addr) = self.with_peer_registry(|reg| {
            reg.get_peer(session_id)
                .filter(|peer| !peer.is_whitelist)
                .map(|peer| peer.connected_addr.clone())
        }) {
            info!(
                "Ban peer {:?} for {} seconds, reason: {}",
                addr,
                duration.as_secs(),
                reason
            );
            if let Some(peer) = self.with_peer_registry_mut(|reg| reg.remove_peer(session_id)) {
                self.peer_store.lock().ban_addr(
                    &peer.connected_addr,
                    duration.as_millis() as u64,
                    reason,
                );
                let message = format!("Ban for {} seconds", duration.as_secs());
                if let Err(err) =
                    disconnect_with_message(p2p_control, peer.session_id, message.as_str())
                {
                    debug!("Disconnect failed {:?}, error: {:?}", peer.session_id, err);
                }
            }
        } else {
            debug!(
                "Ban session({}) failed: not in peer registry or it is in the whitelist",
                session_id
            );
        }
    }

    pub(crate) fn accept_peer(
        &self,
        session_context: &SessionContext,
    ) -> Result<Option<Peer>, Error> {
        // NOTE: be careful, here easy cause a deadlock,
        //    because peer_store's lock scope across peer_registry's lock scope
        let mut peer_store = self.peer_store.lock();
        let accept_peer_result = {
            self.peer_registry.write().accept_peer(
                session_context.address.clone(),
                session_context.id,
                session_context.ty,
                &mut peer_store,
            )
        };
        accept_peer_result.map_err(Into::into)
    }

    // For restrict lock in inner scope
    pub(crate) fn with_peer_registry<F, T>(&self, callback: F) -> T
    where
        F: FnOnce(&PeerRegistry) -> T,
    {
        callback(&self.peer_registry.read())
    }

    // For restrict lock in inner scope
    pub(crate) fn with_peer_registry_mut<F, T>(&self, callback: F) -> T
    where
        F: FnOnce(&mut PeerRegistry) -> T,
    {
        callback(&mut self.peer_registry.write())
    }

    // For restrict lock in inner scope
    pub(crate) fn with_peer_store_mut<F, T>(&self, callback: F) -> T
    where
        F: FnOnce(&mut PeerStore) -> T,
    {
        callback(&mut self.peer_store.lock())
    }

    /// Get peer id of local node
    pub fn local_peer_id(&self) -> &PeerId {
        &self.local_peer_id
    }

    /// Use on test
    #[allow(dead_code)]
    pub(crate) fn local_private_key(&self) -> &secio::SecioKeyPair {
        &self.local_private_key
    }

    /// Get local node's peer id in base58 format string
    pub fn node_id(&self) -> String {
        self.local_peer_id().to_base58()
    }

    pub(crate) fn public_addrs(&self, count: usize) -> Vec<Multiaddr> {
        self.public_addrs
            .read()
            .iter()
            .take(count)
            .cloned()
            .collect()
    }

    pub(crate) fn connection_status(&self) -> ConnectionStatus {
        self.peer_registry.read().connection_status()
    }

    /// Get local node's listen address list
    pub fn public_urls(&self, max_urls: usize) -> Vec<(String, u8)> {
        let listened_addrs = self.listened_addrs.read();
        self.public_addrs(max_urls.saturating_sub(listened_addrs.len()))
            .into_iter()
            .filter_map(|addr| {
                if !listened_addrs.contains(&addr) {
                    Some((addr, 1))
                } else {
                    None
                }
            })
            .chain(listened_addrs.iter().map(|addr| (addr.to_owned(), 1)))
            .map(|(addr, score)| (addr.to_string(), score))
            .collect()
    }

    pub(crate) fn add_node(&self, p2p_control: &ServiceControl, address: Multiaddr) {
        self.dial_identify(p2p_control, address);
    }

    /// use a filter to get protocol id list
    pub fn get_protocol_ids<F: Fn(ProtocolId) -> bool>(&self, filter: F) -> Vec<ProtocolId> {
        self.protocols
            .read()
            .iter()
            .filter_map(|&(id, _, _)| if filter(id) { Some(id) } else { None })
            .collect::<Vec<_>>()
    }

    pub(crate) fn can_dial(&self, addr: &Multiaddr) -> bool {
        let peer_id = extract_peer_id(addr);
        if peer_id.is_none() {
            error!("Do not dial addr without peer id, addr: {}", addr);
            return false;
        }
        let peer_id = peer_id.as_ref().unwrap();

        if self.local_peer_id() == peer_id {
            trace!("Do not dial self: {:?}, {}", peer_id, addr);
            return false;
        }
        if self.public_addrs.read().contains(&addr) {
            trace!(
                "Do not dial listened address(self): {:?}, {}",
                peer_id,
                addr
            );
            return false;
        }

        let peer_in_registry = self.with_peer_registry(|reg| {
            reg.get_key_by_peer_id(peer_id).is_some() || reg.is_feeler(addr)
        });
        if peer_in_registry {
            trace!("Do not dial peer in registry: {:?}, {}", peer_id, addr);
            return false;
        }

        if let Some(dial_started) = self.dialing_addrs.read().get(peer_id) {
            trace!(
                "Do not repeat send dial command to network service: {:?}, {}",
                peer_id,
                addr
            );
            if dial_started.elapsed() > DIAL_HANG_TIMEOUT {
                #[cfg(feature = "with_sentry")]
                with_scope(
                    |scope| scope.set_fingerprint(Some(&["ckb-network", "dialing-timeout"])),
                    || {
                        capture_message(
                            &format!(
                                "Dialing {:?}, {:?} for more than {} seconds, \
                                 something is wrong in network service",
                                peer_id,
                                addr,
                                DIAL_HANG_TIMEOUT.as_secs(),
                            ),
                            Level::Warning,
                        )
                    },
                );
            }
            return false;
        }

        true
    }

    pub(crate) fn dial_success(&self, addr: &Multiaddr) {
        if let Some(peer_id) = extract_peer_id(addr) {
            self.dialing_addrs.write().remove(&peer_id);
        }
    }

    pub(crate) fn dial_failed(&self, addr: &Multiaddr) {
        self.with_peer_registry_mut(|reg| {
            reg.remove_feeler(addr);
        });

        if let Some(peer_id) = extract_peer_id(addr) {
            self.dialing_addrs.write().remove(&peer_id);
        }
    }

    /// Dial
    /// return value indicates the dialing is actually sent or denied.
    fn dial_inner(
        &self,
        p2p_control: &ServiceControl,
        addr: Multiaddr,
        target: TargetProtocol,
    ) -> Result<(), Error> {
        if !self.can_dial(&addr) {
            return Err(Error::Dial(format!("ignore dialing addr {}", addr)));
        }

        debug!("dialing {}", addr);
        p2p_control.dial(addr.clone(), target)?;
        self.dialing_addrs.write().insert(
            extract_peer_id(&addr).expect("verified addr"),
            Instant::now(),
        );
        Ok(())
    }

    /// Dial just identify protocol
    pub fn dial_identify(&self, p2p_control: &ServiceControl, addr: Multiaddr) {
        if let Err(err) = self.dial_inner(
            p2p_control,
            addr,
            TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
        ) {
            debug!("dial_identify error: {}", err);
        }
    }

    /// Dial just feeler protocol
    pub fn dial_feeler(&self, p2p_control: &ServiceControl, addr: Multiaddr) {
        if let Err(err) = self.dial_inner(
            p2p_control,
            addr.clone(),
            TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
        ) {
            debug!("dial_feeler error {}", err);
        } else {
            self.with_peer_registry_mut(|reg| {
                reg.add_feeler(&addr);
            });
        }
    }

    /// this method is intent to check observed addr by dial to self
    pub(crate) fn try_dial_observed_addrs(&self, p2p_control: &ServiceControl) {
        let mut pending_observed_addrs = self.pending_observed_addrs.write();
        if pending_observed_addrs.is_empty() {
            let addrs = self.public_addrs.read();
            if addrs.is_empty() {
                return;
            }
            // random get addr
            if let Some(addr) = addrs.iter().choose(&mut rand::thread_rng()) {
                if let Err(err) = p2p_control.dial(
                    addr.clone(),
                    TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
                ) {
                    trace!("try_dial_observed_addrs fail {} on public address", err)
                }
            }
        } else {
            for addr in pending_observed_addrs.drain() {
                trace!("try dial observed addr: {:?}", addr);
                if let Err(err) = p2p_control.dial(
                    addr,
                    TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
                ) {
                    trace!("try_dial_observed_addrs fail {} on pending observed", err)
                }
            }
        }
    }

    /// add observed address for identify protocol
    pub(crate) fn add_observed_addrs(&self, iter: impl Iterator<Item = Multiaddr>) {
        let mut pending_observed_addrs = self.pending_observed_addrs.write();
        pending_observed_addrs.extend(iter)
    }

    /// Network message processing controller, default is true, if false, discard any received messages
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }
}

/// Used to handle global events of tentacle, such as session open/close
pub struct EventHandler<T> {
    pub(crate) network_state: Arc<NetworkState>,
    pub(crate) exit_handler: T,
}

/// Exit trait used to notify all other module to exit
pub trait ExitHandler: Send + Unpin + 'static {
    /// notify other module to exit
    fn notify_exit(&self);
}

/// Default exit handle
#[derive(Clone, Default)]
pub struct DefaultExitHandler {
    lock: Arc<Mutex<()>>,
    exit: Arc<Condvar>,
}

impl DefaultExitHandler {
    /// Block on current thread util exit notify
    pub fn wait_for_exit(&self) {
        self.exit.wait(&mut self.lock.lock());
    }
}

impl ExitHandler for DefaultExitHandler {
    fn notify_exit(&self) {
        self.exit.notify_all();
    }
}

impl<T> EventHandler<T> {
    fn inbound_eviction(&self, context: &mut ServiceContext) {
        if self.network_state.config.bootnode_mode {
            let status = self.network_state.connection_status();

            if status.max_inbound <= status.non_whitelist_inbound.saturating_add(10) {
                for (index, peer) in self
                    .network_state
                    .with_peer_registry(|registry| {
                        registry
                            .peers()
                            .values()
                            .filter(|peer| peer.is_inbound() && !peer.is_whitelist)
                            .map(|peer| peer.session_id)
                            .collect::<Vec<SessionId>>()
                    })
                    .into_iter()
                    .enumerate()
                {
                    if index & 0x1 != 0 {
                        if let Err(err) = disconnect_with_message(
                            context.control(),
                            peer,
                            "bootnode random eviction",
                        ) {
                            debug!("Inbound eviction failed {:?}, error: {:?}", peer, err);
                            return;
                        }
                    }
                }
            }
        }
    }
}

impl<T: ExitHandler> ServiceHandle for EventHandler<T> {
    fn handle_error(&mut self, context: &mut ServiceContext, error: ServiceError) {
        match error {
            ServiceError::DialerError { address, error } => {
                debug!("DialerError({}) {}", address, error);

                let mut public_addrs = self.network_state.public_addrs.write();

                if let DialerErrorKind::HandshakeError(HandshakeErrorKind::SecioError(
                    SecioError::ConnectSelf,
                )) = error
                {
                    debug!("dial observed address success: {:?}", address);
                    if let Some(ip) = multiaddr_to_socketaddr(&address) {
                        if is_reachable(ip.ip()) {
                            public_addrs.insert(address);
                        }
                    }
                    return;
                } else {
                    public_addrs.remove(&address);
                }
                self.network_state.dial_failed(&address);
            }
            ServiceError::ProtocolError {
                id,
                proto_id,
                error,
            } => {
                debug!("ProtocolError({}, {}) {}", id, proto_id, error);
                let message = format!("ProtocolError id={}", proto_id);
                // Ban because misbehave of remote peer
                self.network_state.ban_session(
                    &context.control(),
                    id,
                    Duration::from_secs(300),
                    message,
                );
            }
            ServiceError::SessionTimeout { session_context } => {
                warn!(
                    "SessionTimeout({}, {})",
                    session_context.id, session_context.address,
                );
            }
            ServiceError::MuxerError {
                session_context,
                error,
            } => {
                debug!(
                    "MuxerError({}, {}), substream error {}, disconnect it",
                    session_context.id, session_context.address, error,
                );
            }
            ServiceError::ListenError { address, error } => {
                debug!("ListenError: address={:?}, error={:?}", address, error);
            }
            ServiceError::ProtocolSelectError {
                proto_name,
                session_context,
            } => {
                debug!(
                    "ProtocolSelectError: proto_name={:?}, session_id={}",
                    proto_name, session_context.id,
                );
            }
            ServiceError::SessionBlocked { session_context } => {
                debug!("SessionBlocked: {}", session_context.id);
            }
            ServiceError::ProtocolHandleError { proto_id, error } => {
                debug!("ProtocolHandleError: {:?}, proto_id: {}", error, proto_id);
                #[cfg(feature = "with_sentry")]
                with_scope(
                    |scope| scope.set_fingerprint(Some(&["ckb-network", "p2p-service-error"])),
                    || {
                        capture_message(
                            &format!("ProtocolHandleError: {:?}, proto_id: {}", error, proto_id),
                            Level::Warning,
                        )
                    },
                );

                if let ProtocolHandleErrorKind::AbnormallyClosed(opt_session_id) = error {
                    if let Some(id) = opt_session_id {
                        self.network_state.ban_session(
                            &context.control(),
                            id,
                            Duration::from_secs(300),
                            format!("protocol {} panic when process peer message", proto_id),
                        );
                    }
                    self.exit_handler.notify_exit();
                }
            }
        }
    }

    fn handle_event(&mut self, context: &mut ServiceContext, event: ServiceEvent) {
        // When session disconnect update status anyway
        match event {
            ServiceEvent::SessionOpen { session_context } => {
                debug!(
                    "SessionOpen({}, {})",
                    session_context.id, session_context.address,
                );
                self.network_state.dial_success(&session_context.address);

                self.inbound_eviction(context);

                if self
                    .network_state
                    .with_peer_registry(|reg| reg.is_feeler(&session_context.address))
                {
                    debug!(
                        "feeler connected {} => {}",
                        session_context.id, session_context.address,
                    );
                } else {
                    match self.network_state.accept_peer(&session_context) {
                        Ok(Some(evicted_peer)) => {
                            debug!(
                                "evict peer (disonnect it), {} => {}",
                                evicted_peer.session_id, evicted_peer.connected_addr,
                            );
                            if let Err(err) = disconnect_with_message(
                                context.control(),
                                evicted_peer.session_id,
                                "evict because accepted better peer",
                            ) {
                                debug!(
                                    "Disconnect failed {:?}, error: {:?}",
                                    evicted_peer.session_id, err
                                );
                            }
                        }
                        Ok(None) => debug!(
                            "{} open, registry {} success",
                            session_context.id, session_context.address,
                        ),
                        Err(err) => {
                            debug!(
                                "registry peer failed {:?} disconnect it, {} => {}",
                                err, session_context.id, session_context.address,
                            );
                            if let Err(err) = disconnect_with_message(
                                context.control(),
                                session_context.id,
                                "reject peer connection",
                            ) {
                                debug!(
                                    "Disconnect failed {:?}, error: {:?}",
                                    session_context.id, err
                                );
                            }
                        }
                    }
                }
            }
            ServiceEvent::SessionClose { session_context } => {
                debug!(
                    "SessionClose({}, {})",
                    session_context.id, session_context.address,
                );

                let peer_exists = self
                    .network_state
                    .peer_registry
                    .write()
                    .remove_peer(session_context.id)
                    .is_some();
                if peer_exists {
                    debug!(
                        "{} closed, remove {} from peer_registry",
                        session_context.id, session_context.address,
                    );
                    self.network_state.with_peer_store_mut(|peer_store| {
                        peer_store.remove_disconnected_peer(&session_context.address);
                    })
                }
            }
            _ => {
                info!("p2p service event: {:?}", event);
            }
        }
    }
}

/// Ckb network service, use to start p2p network
pub struct NetworkService<T> {
    p2p_service: Service<EventHandler<T>>,
    network_state: Arc<NetworkState>,
    ping_controller: Option<Sender<()>>,
    // Background services
    bg_services: Vec<Pin<Box<dyn Future<Output = ()> + 'static + Send>>>,
    version: String,
}

impl<T: ExitHandler> NetworkService<T> {
    /// init with all config
    pub fn new(
        network_state: Arc<NetworkState>,
        protocols: Vec<CKBProtocol>,
        required_protocol_ids: Vec<ProtocolId>,
        name: String,
        version: String,
        exit_handler: T,
    ) -> Self {
        let config = &network_state.config;
        // == Build p2p service struct
        let mut protocol_metas = protocols
            .into_iter()
            .map(CKBProtocol::build)
            .collect::<Vec<_>>();

        // == Build special protocols

        // Identify is a core protocol, user cannot disable it via config
        let identify_callback =
            IdentifyCallback::new(Arc::clone(&network_state), name, version.clone());
        let identify_meta = SupportProtocols::Identify.build_meta_with_service_handle(move || {
            ProtocolHandle::Callback(Box::new(IdentifyProtocol::new(identify_callback)))
        });
        protocol_metas.push(identify_meta);

        // Ping protocol
        let ping_controller = if config.support_protocols.contains(&SupportProtocol::Ping) {
            let ping_interval = Duration::from_secs(config.ping_interval_secs);
            let ping_timeout = Duration::from_secs(config.ping_timeout_secs);

            let ping_network_state = Arc::clone(&network_state);
            let (ping_handler, ping_controller) =
                PingHandler::new(ping_interval, ping_timeout, ping_network_state);
            let ping_meta = SupportProtocols::Ping.build_meta_with_service_handle(move || {
                ProtocolHandle::Callback(Box::new(ping_handler))
            });
            protocol_metas.push(ping_meta);
            Some(ping_controller)
        } else {
            None
        };

        // Discovery protocol
        if config
            .support_protocols
            .contains(&SupportProtocol::Discovery)
        {
            let addr_mgr = DiscoveryAddressManager {
                network_state: Arc::clone(&network_state),
                discovery_local_address: config.discovery_local_address,
            };
            let disc_meta = SupportProtocols::Discovery.build_meta_with_service_handle(move || {
                ProtocolHandle::Callback(Box::new(DiscoveryProtocol::new(
                    addr_mgr,
                    config
                        .discovery_announce_check_interval_secs
                        .map(Duration::from_secs),
                )))
            });
            protocol_metas.push(disc_meta);
        }

        // Feeler protocol
        if config.support_protocols.contains(&SupportProtocol::Feeler) {
            let feeler_meta = SupportProtocols::Feeler.build_meta_with_service_handle({
                let network_state = Arc::clone(&network_state);
                move || ProtocolHandle::Callback(Box::new(Feeler::new(Arc::clone(&network_state))))
            });
            protocol_metas.push(feeler_meta);
        }

        // DisconnectMessage protocol
        if config
            .support_protocols
            .contains(&SupportProtocol::DisconnectMessage)
        {
            let disconnect_message_state = Arc::clone(&network_state);
            let disconnect_message_meta = SupportProtocols::DisconnectMessage
                .build_meta_with_service_handle(move || {
                    ProtocolHandle::Callback(Box::new(DisconnectMessageProtocol::new(
                        disconnect_message_state,
                    )))
                });
            protocol_metas.push(disconnect_message_meta);
        }

        let mut service_builder = ServiceBuilder::default();
        let yamux_config = YamuxConfig {
            max_stream_count: protocol_metas.len(),
            ..Default::default()
        };
        for meta in protocol_metas.into_iter() {
            network_state
                .protocols
                .write()
                .push((meta.id(), meta.name(), meta.support_versions()));
            service_builder = service_builder.insert_protocol(meta);
        }
        let event_handler = EventHandler {
            network_state: Arc::clone(&network_state),
            exit_handler,
        };
        service_builder = service_builder
            .key_pair(network_state.local_private_key.clone())
            .upnp(config.upnp)
            .yamux_config(yamux_config)
            .forever(true)
            .max_connection_number(1024)
            .set_send_buffer_size(config.max_send_buffer());

        #[cfg(target_os = "linux")]
        let p2p_service = {
            if config.reuse {
                let iter = config.listen_addresses.iter();

                #[derive(Clone, Copy, Debug, Eq, PartialEq)]
                enum TransportType {
                    Ws,
                    Tcp,
                }

                fn find_type(addr: &Multiaddr) -> TransportType {
                    let mut iter = addr.iter();

                    iter.find_map(|proto| {
                        if let p2p::multiaddr::Protocol::Ws = proto {
                            Some(TransportType::Ws)
                        } else {
                            None
                        }
                    })
                    .unwrap_or(TransportType::Tcp)
                }

                #[derive(Clone, Copy, Debug, Eq, PartialEq)]
                enum BindType {
                    None,
                    Ws,
                    Tcp,
                    Both,
                }
                impl BindType {
                    fn transform(&mut self, other: TransportType) {
                        match (&self, other) {
                            (BindType::None, TransportType::Ws) => *self = BindType::Ws,
                            (BindType::None, TransportType::Tcp) => *self = BindType::Tcp,
                            (BindType::Ws, TransportType::Tcp) => *self = BindType::Both,
                            (BindType::Tcp, TransportType::Ws) => *self = BindType::Both,
                            _ => (),
                        }
                    }

                    fn is_ready(&self) -> bool {
                        // should change to Both if ckb enable ws
                        matches!(self, BindType::Tcp)
                    }
                }

                let mut init = BindType::None;
                for addr in iter {
                    if init.is_ready() {
                        break;
                    }
                    match find_type(addr) {
                        // wait ckb enable ws support
                        TransportType::Ws => (),
                        TransportType::Tcp => {
                            // only bind once
                            if matches!(init, BindType::Tcp) {
                                continue;
                            }
                            service_builder = service_builder.tcp_bind(addr.clone());
                            init.transform(TransportType::Tcp)
                        }
                    }
                }
            }

            service_builder.build(event_handler)
        };

        #[cfg(not(target_os = "linux"))]
        // The default permissions of Windows are not enough to enable this function,
        // and the administrator permissions of group permissions must be turned on.
        // This operation is very burdensome for windows users, so it is turned off by default
        //
        // The integration test fails after MacOS is turned on, the behavior is different from linux.
        // Decision to turn off it
        let p2p_service = service_builder.build(event_handler);

        // == Build background service tasks
        let dump_peer_store_service = DumpPeerStoreService::new(Arc::clone(&network_state));
        let protocol_type_checker_service = ProtocolTypeCheckerService::new(
            Arc::clone(&network_state),
            p2p_service.control().to_owned(),
            required_protocol_ids,
        );
        let mut bg_services = vec![
            Box::pin(dump_peer_store_service) as Pin<Box<_>>,
            Box::pin(protocol_type_checker_service) as Pin<Box<_>>,
        ];
        if config.outbound_peer_service_enabled() {
            let outbound_peer_service = OutboundPeerService::new(
                Arc::clone(&network_state),
                p2p_service.control().to_owned(),
                Duration::from_secs(config.connect_outbound_interval_secs),
            );
            bg_services.push(Box::pin(outbound_peer_service) as Pin<Box<_>>);
        };

        #[cfg(feature = "with_dns_seeding")]
        if config.dns_seeding_service_enabled() {
            let dns_seeding_service = crate::services::dns_seeding::DnsSeedingService::new(
                Arc::clone(&network_state),
                config.dns_seeds.clone(),
            );
            bg_services.push(Box::pin(dns_seeding_service.start()) as Pin<Box<_>>);
        };

        NetworkService {
            p2p_service,
            network_state,
            ping_controller,
            bg_services,
            version,
        }
    }

    /// Start the network in the background and return a controller
    pub fn start<S: Spawn>(self, handle: &S) -> Result<NetworkController, Error> {
        let config = self.network_state.config.clone();

        // dial whitelist_nodes
        for addr in self.network_state.config.whitelist_peers() {
            debug!("dial whitelist_peers {:?}", addr);
            self.network_state
                .dial_identify(self.p2p_service.control(), addr);
        }

        // get bootnodes
        // try get addrs from peer_store, if peer_store have no enough addrs then use bootnodes
        let bootnodes = self.network_state.with_peer_store_mut(|peer_store| {
            let count = max((config.max_outbound_peers >> 1) as usize, 1);
            let mut addrs: Vec<_> = peer_store
                .fetch_addrs_to_attempt(count)
                .into_iter()
                .map(|paddr| paddr.addr)
                .collect();
            // Get bootnodes randomly
            let bootnodes = self
                .network_state
                .bootnodes
                .iter()
                .choose_multiple(&mut rand::thread_rng(), count.saturating_sub(addrs.len()))
                .into_iter()
                .cloned();
            addrs.extend(bootnodes);
            addrs
        });

        // dial half bootnodes
        for addr in bootnodes {
            debug!("dial bootnode {:?}", addr);
            self.network_state
                .dial_identify(self.p2p_service.control(), addr);
        }

        let Self {
            mut p2p_service,
            network_state,
            ping_controller,
            bg_services,
            version,
        } = self;
        let p2p_control = p2p_service.control().to_owned();

        // NOTE: for ensure background task finished
        let (bg_signals, bg_receivers): (Vec<_>, Vec<_>) = bg_services
            .into_iter()
            .map(|bg_service| {
                let (signal_sender, signal_receiver) = oneshot::channel::<()>();
                (signal_sender, (bg_service, signal_receiver))
            })
            .unzip();

        let (sender, mut receiver) = oneshot::channel();
        let (start_sender, start_receiver) = mpsc::channel();
        {
            let network_state = Arc::clone(&network_state);
            let p2p_control = p2p_control.clone();
            handle.spawn_task(async move {
                for addr in &config.listen_addresses {
                    match p2p_service.listen(addr.to_owned()).await {
                        Ok(listen_address) => {
                            info!(
                                "Listen on address: {}",
                                listen_address
                            );
                            network_state
                                .listened_addrs
                                .write()
                                .push(listen_address.clone());
                        }
                        Err(err) => {
                            warn!(
                                "listen on address {} failed, due to error: {}",
                                addr.clone(),
                                err
                            );
                            start_sender
                                .send(Err(Error::P2P(P2PError::Transport(err))))
                                .expect("channel abnormal shutdown");
                            return;
                        }
                    };
                }
                start_sender.send(Ok(())).unwrap();
                loop {
                    tokio::select! {
                        Some(_) = p2p_service.next() => {},
                        _ = &mut receiver => {
                            for peer in network_state.peer_registry.read().peers().values() {
                                info!("Disconnect peer {}", peer.connected_addr);
                                if let Err(err) =
                                    disconnect_with_message(&p2p_control, peer.session_id, "shutdown")
                                {
                                    debug!("Disconnect failed {:?}, error: {:?}", peer.session_id, err);
                                }
                            }
                            // Drop senders to stop all corresponding background task
                            drop(bg_signals);

                            break;
                        },
                        else => {
                            for peer in network_state.peer_registry.read().peers().values() {
                                info!("Disconnect peer {}", peer.connected_addr);
                                if let Err(err) =
                                    disconnect_with_message(&p2p_control, peer.session_id, "shutdown")
                                {
                                    debug!("Disconnect failed {:?}, error: {:?}", peer.session_id, err);
                                }
                            }
                            // Drop senders to stop all corresponding background task
                            drop(bg_signals);

                            break;
                        },
                    }
                }
            });
        }
        for (mut service, mut receiver) in bg_receivers {
            handle.spawn_task(async move {
                loop {
                    tokio::select! {
                        _ = &mut service => {},
                        _ = &mut receiver => break
                    }
                }
            });
        }

        if let Ok(Err(e)) = start_receiver.recv() {
            return Err(e);
        }

        let stop = StopHandler::new(SignalSender::Tokio(sender), None);
        Ok(NetworkController {
            version,
            network_state,
            p2p_control,
            ping_controller,
            stop,
        })
    }
}

/// Network controller
#[derive(Clone)]
pub struct NetworkController {
    version: String,
    network_state: Arc<NetworkState>,
    p2p_control: ServiceControl,
    ping_controller: Option<Sender<()>>,
    stop: StopHandler<()>,
}

impl NetworkController {
    /// set ckb2021 start
    pub fn init_ckb2021(&self) {
        self.network_state.ckb2021.store(true, Ordering::SeqCst);
    }

    /// get ckb2021 flag
    pub fn load_ckb2021(&self) -> bool {
        self.network_state.ckb2021.load(Ordering::SeqCst)
    }

    /// Node listen address list
    pub fn public_urls(&self, max_urls: usize) -> Vec<(String, u8)> {
        self.network_state.public_urls(max_urls)
    }

    /// ckb version
    pub fn version(&self) -> &String {
        &self.version
    }

    /// Node peer id's base58 format string
    pub fn node_id(&self) -> String {
        self.network_state.node_id()
    }

    /// Dial remote node
    pub fn add_node(&self, address: Multiaddr) {
        self.network_state.add_node(&self.p2p_control, address)
    }

    /// Disconnect session with peer id
    pub fn remove_node(&self, peer_id: &PeerId) {
        if let Some(session_id) = self
            .network_state
            .peer_registry
            .read()
            .get_key_by_peer_id(peer_id)
        {
            if let Err(err) =
                disconnect_with_message(&self.p2p_control, session_id, "disconnect manually")
            {
                debug!("Disconnect failed {:?}, error: {:?}", session_id, err);
            }
        } else {
            error!("Cannot find peer {:?}", peer_id);
        }
    }

    /// Get banned peer list
    pub fn get_banned_addrs(&self) -> Vec<BannedAddr> {
        self.network_state
            .peer_store
            .lock()
            .ban_list()
            .get_banned_addrs()
    }

    /// Clear banned list
    pub fn clear_banned_addrs(&self) {
        self.network_state.peer_store.lock().clear_ban_list();
    }

    /// Get address info from peer store
    pub fn addr_info(&self, addr: &Multiaddr) -> Option<AddrInfo> {
        self.network_state
            .peer_store
            .lock()
            .addr_manager()
            .get(addr)
            .cloned()
    }

    /// Ban an ip
    pub fn ban(&self, address: IpNetwork, ban_until: u64, ban_reason: String) {
        self.network_state
            .peer_store
            .lock()
            .ban_network(address, ban_until, ban_reason)
    }

    /// Unban an ip
    pub fn unban(&self, address: &IpNetwork) {
        self.network_state
            .peer_store
            .lock()
            .mut_ban_list()
            .unban_network(address);
    }

    /// Return all connected peers' information
    pub fn connected_peers(&self) -> Vec<(PeerIndex, Peer)> {
        self.network_state.with_peer_registry(|reg| {
            reg.peers()
                .iter()
                .map(|(peer_index, peer)| (*peer_index, peer.clone()))
                .collect::<Vec<_>>()
        })
    }

    /// Ban an peer through peer index
    pub fn ban_peer(&self, peer_index: PeerIndex, duration: Duration, reason: String) {
        self.network_state
            .ban_session(&self.p2p_control, peer_index, duration, reason);
    }

    fn try_broadcast(
        &self,
        quick: bool,
        target: Option<SessionId>,
        proto_id: ProtocolId,
        data: Bytes,
    ) -> Result<(), SendErrorKind> {
        let now = Instant::now();
        loop {
            let target = target
                .clone()
                .map(TargetSession::Single)
                .unwrap_or(TargetSession::All);
            let result = if quick {
                self.p2p_control
                    .quick_filter_broadcast(target, proto_id, data.clone())
            } else {
                self.p2p_control
                    .filter_broadcast(target, proto_id, data.clone())
            };
            match result {
                Ok(()) => {
                    return Ok(());
                }
                Err(SendErrorKind::WouldBlock) => {
                    if now.elapsed() > P2P_SEND_TIMEOUT {
                        warn!("broadcast message to {} timeout", proto_id);
                        return Err(SendErrorKind::WouldBlock);
                    }
                    thread::sleep(P2P_TRY_SEND_INTERVAL);
                }
                Err(err) => {
                    warn!("broadcast message to {} failed: {:?}", proto_id, err);
                    return Err(err);
                }
            }
        }
    }

    /// Broadcast a message to all connected peers
    pub fn broadcast(&self, proto_id: ProtocolId, data: Bytes) -> Result<(), SendErrorKind> {
        self.try_broadcast(false, None, proto_id, data)
    }

    /// Broadcast a message to all connected peers through quick queue
    pub fn quick_broadcast(&self, proto_id: ProtocolId, data: Bytes) -> Result<(), SendErrorKind> {
        self.try_broadcast(true, None, proto_id, data)
    }

    /// Send message to one connected peer
    pub fn send_message_to(
        &self,
        session_id: SessionId,
        proto_id: ProtocolId,
        data: Bytes,
    ) -> Result<(), SendErrorKind> {
        self.try_broadcast(false, Some(session_id), proto_id, data)
    }

    /// network message processing controller, always true, if false, discard any received messages
    pub fn is_active(&self) -> bool {
        self.network_state.is_active()
    }

    /// Change active status, if set false discard any received messages
    pub fn set_active(&self, active: bool) {
        self.network_state.active.store(active, Ordering::Relaxed);
    }

    /// Return all connected peers' protocols info
    pub fn protocols(&self) -> Vec<(ProtocolId, String, Vec<String>)> {
        self.network_state.protocols.read().clone()
    }

    /// Try ping all connected peers
    pub fn ping_peers(&self) {
        if let Some(mut ping_controller) = self.ping_controller.clone() {
            let _ignore = ping_controller.try_send(());
        }
    }
}

impl Drop for NetworkController {
    fn drop(&mut self) {
        self.stop.try_send(());
    }
}

// Send an optional message before disconnect a peer
pub(crate) fn disconnect_with_message(
    control: &ServiceControl,
    peer_index: SessionId,
    message: &str,
) -> Result<(), SendErrorKind> {
    if !message.is_empty() {
        let data = Bytes::from(message.as_bytes().to_vec());
        // Must quick send, otherwise this message will be dropped.
        control.quick_send_message_to(
            peer_index,
            SupportProtocols::DisconnectMessage.protocol_id(),
            data,
        )?;
    }
    control.disconnect(peer_index)
}
