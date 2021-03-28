//! Global state struct and start function
use crate::errors::{Error, P2PError};
use crate::peer_registry::{ConnectionStatus, PeerRegistry};
use crate::peer_store::{
    types::{AddrInfo, BannedAddr, IpPort, MultiaddrExt},
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
use crate::{Behaviour, CKBProtocol, Peer, PeerIndex, ProtocolId, PublicKey, ServiceControl};
use ckb_app_config::NetworkConfig;
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
    multiaddr::{self, Multiaddr},
    secio::{self, error::SecioError, PeerId},
    service::{ProtocolHandle, Service, ServiceError, ServiceEvent, TargetProtocol, TargetSession},
    traits::ServiceHandle,
    utils::extract_peer_id,
    yamux::config::Config as YamuxConfig,
    SessionId,
};
#[cfg(feature = "with_sentry")]
use sentry::{capture_message, with_scope, Level};
use std::sync::mpsc;
use std::{
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
    bootnodes: Vec<(PeerId, Multiaddr)>,
    pub(crate) config: NetworkConfig,
    pub(crate) active: AtomicBool,
    /// Node supported protocols
    /// fields: ProtocolId, Protocol Name, Supported Versions
    pub(crate) protocols: RwLock<Vec<(ProtocolId, String, Vec<String>)>>,
}

impl NetworkState {
    /// Init from config
    pub fn from_config(config: NetworkConfig) -> Result<NetworkState, Error> {
        config.create_dir_if_not_exists()?;
        let local_private_key = config.fetch_private_key()?;
        // set max score to public addresses
        let public_addrs: HashSet<Multiaddr> = config
            .listen_addresses
            .iter()
            .chain(config.public_addresses.iter())
            .cloned()
            .collect();
        let peer_store = Mutex::new(PeerStore::load_from_dir_or_default(
            config.peer_store_path(),
        ));
        let bootnodes = config.bootnodes()?;

        let whitelist_peers = config
            .whitelist_peers()?
            .iter()
            .map(|(peer_id, _)| peer_id.to_owned())
            .collect::<Vec<_>>();
        let peer_registry = PeerRegistry::new(
            config.max_inbound_peers(),
            config.max_outbound_peers(),
            config.whitelist_only,
            whitelist_peers,
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
            local_private_key: local_private_key.clone(),
            local_peer_id: local_private_key.public_key().peer_id(),
            active: AtomicBool::new(true),
            protocols: RwLock::new(Vec::new()),
        })
    }

    pub(crate) fn report_session(
        &self,
        p2p_control: &ServiceControl,
        session_id: SessionId,
        behaviour: Behaviour,
    ) {
        if let Some(peer_id) = self.with_peer_registry(|reg| {
            reg.get_peer(session_id)
                .filter(|peer| !peer.is_whitelist)
                .map(|peer| peer.peer_id.clone())
        }) {
            self.report_peer(p2p_control, &peer_id, behaviour);
        } else {
            debug!(
                "Report {} failed: not in peer registry or it is in the whitelist",
                session_id
            );
        }
    }

    fn report_peer(&self, p2p_control: &ServiceControl, peer_id: &PeerId, behaviour: Behaviour) {
        trace!("report {:?} because {:?}", peer_id, behaviour);
        let report_result = match self.peer_store.lock().report(peer_id, behaviour) {
            Ok(result) => result,
            Err(err) => {
                error!(
                    "Report failed peer_id: {:?} behaviour: {:?} error: {:?}",
                    peer_id, behaviour, err
                );
                return;
            }
        };
        if report_result.is_banned() {
            info!("peer {:?} banned", peer_id);
            if let Some(session_id) = self.peer_registry.read().get_key_by_peer_id(peer_id) {
                if let Err(err) = disconnect_with_message(p2p_control, session_id, "banned") {
                    debug!("Disconnect failed {:?}, error: {:?}", session_id, err);
                }
            }
        }
    }

    pub(crate) fn ban_session(
        &self,
        p2p_control: &ServiceControl,
        session_id: SessionId,
        duration: Duration,
        reason: String,
    ) {
        if let Some(peer_id) = self.with_peer_registry(|reg| {
            reg.get_peer(session_id)
                .filter(|peer| !peer.is_whitelist)
                .map(|peer| peer.peer_id.clone())
        }) {
            self.ban_peer(p2p_control, &peer_id, duration, reason);
        } else {
            debug!(
                "Ban session({}) failed: not in peer registry or it is in the whitelist",
                session_id
            );
        }
    }

    fn ban_peer(
        &self,
        p2p_control: &ServiceControl,
        peer_id: &PeerId,
        duration: Duration,
        reason: String,
    ) {
        info!(
            "Ban peer {:?} for {} seconds, reason: {}",
            peer_id,
            duration.as_secs(),
            reason
        );
        let peer_opt = self.with_peer_registry_mut(|reg| reg.remove_peer_by_peer_id(peer_id));
        if let Some(peer) = peer_opt {
            if let Err(err) = self.peer_store.lock().ban_addr(
                &peer.connected_addr,
                duration.as_millis() as u64,
                reason,
            ) {
                debug!("Failed to ban peer {:?} {:?}", err, peer);
            }
            let message = format!("Ban for {} seconds", duration.as_secs());
            if let Err(err) =
                disconnect_with_message(p2p_control, peer.session_id, message.as_str())
            {
                debug!("Disconnect failed {:?}, error: {:?}", peer.session_id, err);
            }
        }
    }

    pub(crate) fn query_session_id(&self, peer_id: &PeerId) -> Option<SessionId> {
        self.with_peer_registry(|registry| registry.get_key_by_peer_id(peer_id))
    }

    pub(crate) fn accept_peer(
        &self,
        session_context: &SessionContext,
    ) -> Result<Option<Peer>, Error> {
        let peer_id = session_context
            .remote_pubkey
            .as_ref()
            .map(PublicKey::peer_id)
            .expect("Secio must enabled");

        // NOTE: be careful, here easy cause a deadlock,
        //    because peer_store's lock scope across peer_registry's lock scope
        let mut peer_store = self.peer_store.lock();
        let accept_peer_result = {
            self.peer_registry.write().accept_peer(
                peer_id,
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
            .map(|(addr, score)| (self.to_external_url(&addr), score))
            .collect()
    }

    pub(crate) fn add_node(
        &self,
        p2p_control: &ServiceControl,
        peer_id: &PeerId,
        address: Multiaddr,
    ) {
        self.dial_identify(p2p_control, peer_id, address);
    }

    fn to_external_url(&self, addr: &Multiaddr) -> String {
        format!("{}/p2p/{}", addr, self.node_id())
    }

    /// use a filter to get protocol id list
    pub fn get_protocol_ids<F: Fn(ProtocolId) -> bool>(&self, filter: F) -> Vec<ProtocolId> {
        self.protocols
            .read()
            .iter()
            .filter_map(|&(id, _, _)| if filter(id) { Some(id) } else { None })
            .collect::<Vec<_>>()
    }

    pub(crate) fn can_dial(
        &self,
        peer_id: &PeerId,
        addr: &Multiaddr,
        allow_dial_to_self: bool,
    ) -> bool {
        if !allow_dial_to_self && self.local_peer_id() == peer_id {
            trace!("Do not dial self: {:?}, {}", peer_id, addr);
            return false;
        }
        if !allow_dial_to_self && self.public_addrs.read().contains(&addr) {
            trace!(
                "Do not dial listened address(self): {:?}, {}",
                peer_id,
                addr
            );
            return false;
        }

        let peer_in_registry = self.with_peer_registry(|reg| {
            reg.get_key_by_peer_id(peer_id).is_some() || reg.is_feeler(peer_id)
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

    pub(crate) fn dial_success(&self, peer_id: &PeerId) {
        self.dialing_addrs.write().remove(peer_id);
    }

    pub(crate) fn dial_failed(&self, peer_id: PeerId) {
        self.with_peer_registry_mut(|reg| {
            reg.remove_feeler(&peer_id);
        });
        self.dialing_addrs.write().remove(&peer_id);
    }

    /// Dial
    /// return value indicates the dialing is actually sent or denied.
    fn dial_inner(
        &self,
        p2p_control: &ServiceControl,
        peer_id: &PeerId,
        addr: Multiaddr,
        target: TargetProtocol,
        allow_dial_to_self: bool,
    ) -> Result<(), Error> {
        if !self.can_dial(peer_id, &addr, allow_dial_to_self) {
            return Err(Error::Dial(format!(
                "ignore dialing peer_id {:?}, addr {}",
                peer_id, addr
            )));
        }

        let addr = addr.attach_p2p(peer_id)?;
        debug!("dialing {} with {:?}", addr, target);
        p2p_control.dial(addr, target)?;
        self.dialing_addrs
            .write()
            .insert(peer_id.to_owned(), Instant::now());
        Ok(())
    }

    /// Dial just identify protocol
    pub fn dial_identify(&self, p2p_control: &ServiceControl, peer_id: &PeerId, addr: Multiaddr) {
        if let Err(err) = self.dial_inner(
            p2p_control,
            peer_id,
            addr,
            TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
            false,
        ) {
            debug!("dial_identify error: {}", err);
        }
    }

    /// Dial just feeler protocol
    pub fn dial_feeler(&self, p2p_control: &ServiceControl, peer_id: &PeerId, addr: Multiaddr) {
        if let Err(err) = self.dial_inner(
            p2p_control,
            peer_id,
            addr,
            TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
            false,
        ) {
            debug!("dial_feeler error {}", err);
        } else {
            self.with_peer_registry_mut(|reg| {
                reg.add_feeler(peer_id.clone());
            });
        }
    }

    /// this method is intent to check observed addr by dial to self
    pub(crate) fn try_dial_observed_addrs(&self, p2p_control: &ServiceControl) {
        let mut pending_observed_addrs = self.pending_observed_addrs.write();
        let public_addrs = { self.public_addrs.read().clone() };
        for addr in pending_observed_addrs
            .drain()
            .chain(public_addrs.into_iter())
        {
            trace!("try dial observed addr: {:?}", addr);
            if let Err(err) = self.dial_inner(
                p2p_control,
                self.local_peer_id(),
                addr,
                TargetProtocol::Single(SupportProtocols::Identify.protocol_id()),
                true,
            ) {
                debug!("try_dial_observed_addrs error {}", err);
            }
        }
    }

    /// add observed address for identify protocol
    pub(crate) fn add_observed_addrs(&self, iter: impl Iterator<Item = Multiaddr>) {
        let mut pending_observed_addrs = self.pending_observed_addrs.write();
        for addr in iter {
            trace!("pending observed addr: {:?}", addr,);
            pending_observed_addrs.insert(addr);
        }
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
            ServiceError::DialerError {
                ref address,
                ref error,
            } => {
                debug!("DialerError({}) {}", address, error);

                let mut public_addrs = self.network_state.public_addrs.write();
                let addr = address
                    .iter()
                    .filter(|proto| match proto {
                        multiaddr::Protocol::P2P(_) => false,
                        _ => true,
                    })
                    .collect();

                if let DialerErrorKind::HandshakeError(HandshakeErrorKind::SecioError(
                    SecioError::ConnectSelf,
                )) = error
                {
                    debug!("dial observed address success: {:?}", address);
                    public_addrs.insert(addr);
                } else {
                    public_addrs.remove(&addr);
                }
                let peer_id = extract_peer_id(address).expect("Secio must enabled");
                self.network_state.dial_failed(peer_id);
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
                let peer_id = session_context
                    .remote_pubkey
                    .as_ref()
                    .map(PublicKey::peer_id)
                    .expect("Secio must enabled");

                self.network_state.dial_success(&peer_id);

                self.inbound_eviction(context);

                if self
                    .network_state
                    .with_peer_registry(|reg| reg.is_feeler(&peer_id))
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
                let peer_id = session_context
                    .remote_pubkey
                    .as_ref()
                    .map(PublicKey::peer_id)
                    .expect("Secio must enabled");

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
                        peer_store.remove_disconnected_peer(&peer_id);
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
    ping_controller: Sender<()>,
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
        // == Build special protocols

        // TODO: how to deny banned node to open those protocols?
        // Ping protocol
        let ping_interval = Duration::from_secs(config.ping_interval_secs);
        let ping_timeout = Duration::from_secs(config.ping_timeout_secs);

        let ping_network_state = Arc::clone(&network_state);
        let (ping_handler, ping_controller) =
            PingHandler::new(ping_interval, ping_timeout, ping_network_state);
        let ping_meta = SupportProtocols::Ping.build_meta_with_service_handle(move || {
            ProtocolHandle::Callback(Box::new(ping_handler))
        });

        // Discovery protocol
        let addr_mgr = DiscoveryAddressManager {
            network_state: Arc::clone(&network_state),
            discovery_local_address: config.discovery_local_address,
        };
        let disc_meta = SupportProtocols::Discovery.build_meta_with_service_handle(move || {
            ProtocolHandle::Callback(Box::new(DiscoveryProtocol::new(addr_mgr)))
        });

        // Identify protocol
        let identify_callback =
            IdentifyCallback::new(Arc::clone(&network_state), name, version.clone());
        let identify_meta = SupportProtocols::Identify.build_meta_with_service_handle(move || {
            ProtocolHandle::Callback(Box::new(IdentifyProtocol::new(identify_callback)))
        });

        // Feeler protocol
        let feeler_meta = SupportProtocols::Feeler.build_meta_with_service_handle({
            let network_state = Arc::clone(&network_state);
            move || ProtocolHandle::Both(Box::new(Feeler::new(Arc::clone(&network_state))))
        });

        let disconnect_message_state = Arc::clone(&network_state);
        let disconnect_message_meta = SupportProtocols::DisconnectMessage
            .build_meta_with_service_handle(move || {
                ProtocolHandle::Callback(Box::new(DisconnectMessageProtocol::new(
                    disconnect_message_state,
                )))
            });

        // == Build p2p service struct
        let mut protocol_metas = protocols
            .into_iter()
            .map(CKBProtocol::build)
            .collect::<Vec<_>>();
        protocol_metas.push(feeler_meta);
        protocol_metas.push(disconnect_message_meta);
        protocol_metas.push(ping_meta);
        protocol_metas.push(disc_meta);
        protocol_metas.push(identify_meta);

        let mut service_builder = ServiceBuilder::default();
        let mut yamux_config = YamuxConfig::default();
        yamux_config.max_stream_count = protocol_metas.len();
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
                        if let multiaddr::Protocol::Ws = proto {
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
        for (peer_id, addr) in self.network_state.config.whitelist_peers()? {
            debug!("dial whitelist_peers {:?} {:?}", peer_id, addr);
            self.network_state
                .dial_identify(self.p2p_service.control(), &peer_id, addr);
        }

        // get bootnodes
        // try get addrs from peer_store, if peer_store have no enough addrs then use bootnodes
        let bootnodes = self.network_state.with_peer_store_mut(|peer_store| {
            let count = max((config.max_outbound_peers >> 1) as usize, 1);
            let mut addrs: Vec<_> = peer_store
                .fetch_addrs_to_attempt(count)
                .into_iter()
                .map(|paddr| (paddr.peer_id, paddr.addr))
                .collect();
            addrs.extend(
                self.network_state
                    .bootnodes
                    .iter()
                    .take(count.saturating_sub(addrs.len()))
                    .cloned(),
            );
            addrs
        });

        // dial half bootnodes
        for (peer_id, addr) in bootnodes {
            debug!("dial bootnode {:?} {:?}", peer_id, addr);
            self.network_state
                .dial_identify(self.p2p_service.control(), &peer_id, addr);
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
                                network_state.to_external_url(&listen_address)
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
    ping_controller: Sender<()>,
    stop: StopHandler<()>,
}

impl NetworkController {
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
    pub fn add_node(&self, peer_id: &PeerId, address: Multiaddr) {
        self.network_state
            .add_node(&self.p2p_control, peer_id, address)
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
    pub fn addr_info(&self, ip_port: &IpPort) -> Option<AddrInfo> {
        self.network_state
            .peer_store
            .lock()
            .addr_manager()
            .get(ip_port)
            .cloned()
    }

    /// Ban an ip
    pub fn ban(&self, address: IpNetwork, ban_until: u64, ban_reason: String) -> Result<(), Error> {
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
        target: TargetSession,
        proto_id: ProtocolId,
        data: Bytes,
    ) -> Result<(), SendErrorKind> {
        let now = Instant::now();
        loop {
            let result = if quick {
                self.p2p_control
                    .quick_filter_broadcast(target.clone(), proto_id, data.clone())
            } else {
                self.p2p_control
                    .filter_broadcast(target.clone(), proto_id, data.clone())
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
        let session_ids = self.network_state.peer_registry.read().connected_peers();
        let target = TargetSession::Multi(session_ids);
        self.try_broadcast(false, target, proto_id, data)
    }

    /// Broadcast a message to all connected peers through quick queue
    pub fn quick_broadcast(&self, proto_id: ProtocolId, data: Bytes) -> Result<(), SendErrorKind> {
        let session_ids = self.network_state.peer_registry.read().connected_peers();
        let target = TargetSession::Multi(session_ids);
        self.try_broadcast(true, target, proto_id, data)
    }

    /// Send message to one connected peer
    pub fn send_message_to(
        &self,
        session_id: SessionId,
        proto_id: ProtocolId,
        data: Bytes,
    ) -> Result<(), SendErrorKind> {
        let target = TargetSession::Single(session_id);
        self.try_broadcast(false, target, proto_id, data)
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
        let mut ping_controller = self.ping_controller.clone();
        let _ignore = ping_controller.try_send(());
    }
}

impl Drop for NetworkController {
    fn drop(&mut self) {
        self.stop.try_send();
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

#[cfg(test)]
mod test {
    use super::NetworkState;
    use crate::peer_registry::PeerRegistry;
    use crate::peer_store::{types::MultiaddrExt, PeerStore};
    use ckb_app_config::NetworkConfig;
    use ckb_util::{Mutex, RwLock};
    use p2p::{multiaddr::MultiAddr, secio::SecioKeyPair};
    use std::{
        collections::{HashMap, HashSet},
        sync::atomic::AtomicBool,
    };

    #[test]
    fn test_can_dail_self() {
        let local_private_key = SecioKeyPair::secp256k1_generated();
        let mut public_addrs = HashSet::new();
        let addr = "/ip4/127.0.0.1/tcp/8114"
            .parse::<MultiAddr>()
            .unwrap()
            .attach_p2p(&local_private_key.peer_id())
            .unwrap();
        public_addrs.insert(addr.clone());
        let state = NetworkState {
            peer_store: Mutex::new(PeerStore::default()),
            config: NetworkConfig::default(),
            bootnodes: Vec::new(),
            peer_registry: RwLock::new(PeerRegistry::new(1, 1, false, Vec::default())),
            dialing_addrs: RwLock::new(HashMap::default()),
            public_addrs: RwLock::new(public_addrs),
            listened_addrs: RwLock::new(Vec::new()),
            pending_observed_addrs: RwLock::new(HashSet::default()),
            local_private_key: local_private_key.clone(),
            local_peer_id: local_private_key.public_key().peer_id(),
            active: AtomicBool::new(true),
            protocols: RwLock::new(Vec::new()),
        };

        assert!(state.can_dial(&local_private_key.peer_id(), &addr, true));
    }
}
