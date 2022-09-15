use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ckb_logger::{debug, error, trace, warn};
use p2p::{
    async_trait,
    bytes::Bytes,
    context::{ProtocolContext, ProtocolContextMutRef, SessionContext},
    multiaddr::{Multiaddr, Protocol},
    service::{SessionType, TargetProtocol},
    traits::ServiceProtocol,
    utils::{extract_peer_id, is_reachable, multiaddr_to_socketaddr},
    SessionId,
};

mod protocol;

use crate::{NetworkState, PeerIdentifyInfo, SupportProtocols};
use ckb_types::{packed, prelude::*};

use protocol::IdentifyMessage;

const MAX_RETURN_LISTEN_ADDRS: usize = 10;
const BAN_ON_NOT_SAME_NET: Duration = Duration::from_secs(5 * 60);
const CHECK_TIMEOUT_TOKEN: u64 = 100;
// Check timeout interval (seconds)
const CHECK_TIMEOUT_INTERVAL: u64 = 1;
const DEFAULT_TIMEOUT: u64 = 8;
const MAX_ADDRS: usize = 10;

/// The misbehavior to report to underlying peer storage
#[derive(Clone, Debug)]
pub enum Misbehavior {
    /// Repeat received message
    DuplicateReceived,
    /// Timeout reached
    Timeout,
    /// Remote peer send invalid data
    InvalidData,
    /// Send too many addresses in listen addresses
    TooManyAddresses(usize),
}

/// Misbehavior report result
pub enum MisbehaveResult {
    /// Continue to run
    Continue,
    /// Disconnect this peer
    Disconnect,
}

impl MisbehaveResult {
    pub fn is_disconnect(&self) -> bool {
        matches!(self, MisbehaveResult::Disconnect)
    }
}

/// The trait to communicate with underlying peer storage
#[async_trait]
pub trait Callback: Clone + Send {
    // Register open protocol
    fn register(&self, context: &ProtocolContextMutRef, version: &str);
    // remove registered identify protocol
    fn unregister(&self, context: &ProtocolContextMutRef);
    /// Received custom message
    async fn received_identify(
        &mut self,
        context: &mut ProtocolContextMutRef<'_>,
        identify: &[u8],
    ) -> MisbehaveResult;
    /// Get custom identify message
    fn identify(&mut self) -> &[u8];
    /// Get local listen addresses
    fn local_listen_addrs(&mut self) -> Vec<Multiaddr>;
    /// Add remote peer's listen addresses
    fn add_remote_listen_addrs(&mut self, session: &SessionContext, addrs: Vec<Multiaddr>);
    /// Add our address observed by remote peer
    fn add_observed_addr(&mut self, addr: Multiaddr, ty: SessionType) -> MisbehaveResult;
    /// Report misbehavior
    fn misbehave(&mut self, session: &SessionContext, kind: Misbehavior) -> MisbehaveResult;
}

/// Identify protocol
pub struct IdentifyProtocol<T> {
    callback: T,
    remote_infos: HashMap<SessionId, RemoteInfo>,
    global_ip_only: bool,
}

impl<T: Callback> IdentifyProtocol<T> {
    pub fn new(callback: T) -> IdentifyProtocol<T> {
        IdentifyProtocol {
            callback,
            remote_infos: HashMap::default(),
            global_ip_only: true,
        }
    }

    #[cfg(test)]
    pub fn global_ip_only(mut self, only: bool) -> Self {
        self.global_ip_only = only;
        self
    }

    fn check_duplicate(&mut self, context: &mut ProtocolContextMutRef) -> MisbehaveResult {
        let session = context.session;
        let info = self
            .remote_infos
            .get_mut(&session.id)
            .expect("RemoteInfo must exists");

        if info.has_received {
            self.callback
                .misbehave(&info.session, Misbehavior::DuplicateReceived)
        } else {
            info.has_received = true;
            MisbehaveResult::Continue
        }
    }

    fn process_listens(
        &mut self,
        context: &mut ProtocolContextMutRef,
        listens: Vec<Multiaddr>,
    ) -> MisbehaveResult {
        let session = context.session;
        let info = self
            .remote_infos
            .get_mut(&session.id)
            .expect("RemoteInfo must exists");

        if listens.len() > MAX_ADDRS {
            self.callback
                .misbehave(&info.session, Misbehavior::TooManyAddresses(listens.len()))
        } else {
            let global_ip_only = self.global_ip_only;
            let reachable_addrs = listens
                .into_iter()
                .filter(|addr| {
                    multiaddr_to_socketaddr(addr)
                        .map(|socket_addr| !global_ip_only || is_reachable(socket_addr.ip()))
                        .unwrap_or(false)
                })
                .collect::<Vec<_>>();
            self.callback
                .add_remote_listen_addrs(session, reachable_addrs);
            MisbehaveResult::Continue
        }
    }

    fn process_observed(
        &mut self,
        context: &mut ProtocolContextMutRef,
        observed: Multiaddr,
    ) -> MisbehaveResult {
        debug!(
            "IdentifyProtocol process observed address, session: {:?}, observed: {}",
            context.session, observed,
        );

        let session = context.session;
        let info = self
            .remote_infos
            .get_mut(&session.id)
            .expect("RemoteInfo must exists");
        let global_ip_only = self.global_ip_only;
        if multiaddr_to_socketaddr(&observed)
            .map(|socket_addr| socket_addr.ip())
            .filter(|ip_addr| !global_ip_only || is_reachable(*ip_addr))
            .is_none()
        {
            return MisbehaveResult::Continue;
        }

        self.callback.add_observed_addr(observed, info.session.ty)
    }
}

pub(crate) struct RemoteInfo {
    session: SessionContext,
    connected_at: Instant,
    timeout: Duration,
    has_received: bool,
}

impl RemoteInfo {
    fn new(session: SessionContext, timeout: Duration) -> RemoteInfo {
        RemoteInfo {
            session,
            connected_at: Instant::now(),
            timeout,
            has_received: false,
        }
    }
}

#[async_trait]
impl<T: Callback> ServiceProtocol for IdentifyProtocol<T> {
    async fn init(&mut self, context: &mut ProtocolContext) {
        let proto_id = context.proto_id;
        if let Err(err) = context
            .set_service_notify(
                proto_id,
                Duration::from_secs(CHECK_TIMEOUT_INTERVAL),
                CHECK_TIMEOUT_TOKEN,
            )
            .await
        {
            error!("IdentifyProtocol init error: {:?}", err)
        }
    }

    async fn connected(&mut self, context: ProtocolContextMutRef<'_>, version: &str) {
        let session = context.session;
        debug!("IdentifyProtocol connected, session: {:?}", session);

        self.callback.register(&context, version);

        let remote_info = RemoteInfo::new(session.clone(), Duration::from_secs(DEFAULT_TIMEOUT));
        self.remote_infos.insert(session.id, remote_info);

        let listen_addrs: Vec<Multiaddr> = self
            .callback
            .local_listen_addrs()
            .iter()
            .filter(|addr| {
                multiaddr_to_socketaddr(addr)
                    .map(|socket_addr| !self.global_ip_only || is_reachable(socket_addr.ip()))
                    .unwrap_or(false)
            })
            .take(MAX_ADDRS)
            .cloned()
            .collect();

        let identify = self.callback.identify();
        let data = IdentifyMessage::new(listen_addrs, session.address.clone(), identify).encode();
        let _ = context
            .quick_send_message(data)
            .await
            .map_err(|err| error!("IdentifyProtocol quick_send_message, error: {:?}", err));
    }

    async fn disconnected(&mut self, context: ProtocolContextMutRef<'_>) {
        self.remote_infos
            .remove(&context.session.id)
            .expect("RemoteInfo must exists");
        debug!(
            "IdentifyProtocol disconnected, session: {:?}",
            context.session
        );
        self.callback.unregister(&context);
    }

    async fn received(&mut self, mut context: ProtocolContextMutRef<'_>, data: Bytes) {
        let session = context.session;
        match IdentifyMessage::decode(&data) {
            Some(message) => {
                trace!(
                    "IdentifyProtocol received, session: {:?}, listen_addrs: {:?}, observed_addr: {}",
                    context.session, message.listen_addrs, message.observed_addr
                );

                // Interrupt processing if error, avoid pollution
                if let MisbehaveResult::Disconnect = self.check_duplicate(&mut context) {
                    error!(
                        "IdentifyProtocol disconnect session {:?}, reason: duplicate",
                        session
                    );
                    let _ = context.disconnect(session.id).await;
                    return;
                }
                if let MisbehaveResult::Disconnect = self
                    .callback
                    .received_identify(&mut context, message.identify)
                    .await
                {
                    error!(
                        "IdentifyProtocol disconnect session {:?}, reason: invalid identify message",
                        session,
                    );
                    let _ = context.disconnect(session.id).await;
                    return;
                }
                if let MisbehaveResult::Disconnect =
                    self.process_listens(&mut context, message.listen_addrs.clone())
                {
                    error!(
                        "IdentifyProtocol disconnect session {:?}, reason: invalid listen addrs: {:?}",
                        session, message.listen_addrs,
                    );
                    let _ = context.disconnect(session.id).await;
                    return;
                }
                if let MisbehaveResult::Disconnect =
                    self.process_observed(&mut context, message.observed_addr.clone())
                {
                    error!(
                        "IdentifyProtocol disconnect session {:?}, reason: invalid observed addr: {}",
                        session, message.observed_addr,
                    );
                    let _ = context.disconnect(session.id).await;
                }
            }
            None => {
                let info = self
                    .remote_infos
                    .get(&session.id)
                    .expect("RemoteInfo must exists");
                if self
                    .callback
                    .misbehave(&info.session, Misbehavior::InvalidData)
                    .is_disconnect()
                {
                    let _ = context.disconnect(session.id).await;
                }
            }
        }
    }

    async fn notify(&mut self, context: &mut ProtocolContext, _token: u64) {
        for (session_id, info) in &self.remote_infos {
            if !info.has_received && (info.connected_at + info.timeout) <= Instant::now() {
                let misbehave_result = self.callback.misbehave(&info.session, Misbehavior::Timeout);
                if misbehave_result.is_disconnect() {
                    let _ = context.disconnect(*session_id).await;
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct IdentifyCallback {
    network_state: Arc<NetworkState>,
    identify: Identify,
}

impl IdentifyCallback {
    pub(crate) fn new(
        network_state: Arc<NetworkState>,
        name: String,
        client_version: String,
        flags: Flags,
    ) -> IdentifyCallback {
        IdentifyCallback {
            network_state,
            identify: Identify::new(name, flags, client_version),
        }
    }

    fn listen_addrs(&self) -> Vec<Multiaddr> {
        let addrs = self.network_state.public_addrs(MAX_RETURN_LISTEN_ADDRS * 2);
        addrs
            .into_iter()
            .take(MAX_RETURN_LISTEN_ADDRS)
            .collect::<Vec<_>>()
    }
}

#[async_trait]
impl Callback for IdentifyCallback {
    fn register(&self, context: &ProtocolContextMutRef, version: &str) {
        self.network_state.with_peer_registry_mut(|reg| {
            reg.get_peer_mut(context.session.id).map(|peer| {
                peer.protocols.insert(context.proto_id, version.to_owned());
            })
        });
        if context.session.ty.is_outbound() {
            // why don't set inbound here?
            // because inbound address can't feeler during staying connected
            // and if set it to peer store, it will be broadcast to the entire network,
            // but this is an unverified address
            self.network_state.with_peer_store_mut(|peer_store| {
                peer_store.add_outbound_addr(context.session.address.clone());
            });
        }
    }

    fn unregister(&self, context: &ProtocolContextMutRef) {
        if context.session.ty.is_outbound() {
            // Due to the filtering strategy of the peer store, if the node is
            // disconnected after a long connection is maintained for more than seven days,
            // it is possible that the node will be accidentally evicted, so it is necessary
            // to reset the information of the node when disconnected.
            self.network_state.with_peer_store_mut(|peer_store| {
                if !peer_store.is_addr_banned(&context.session.address) {
                    peer_store.add_outbound_addr(context.session.address.clone());
                }
            });
        }
    }

    fn identify(&mut self) -> &[u8] {
        self.identify.encode()
    }

    async fn received_identify(
        &mut self,
        context: &mut ProtocolContextMutRef<'_>,
        identify: &[u8],
    ) -> MisbehaveResult {
        match self.identify.verify(identify) {
            None => {
                self.network_state.ban_session(
                    &context.control().clone().into(),
                    context.session.id,
                    BAN_ON_NOT_SAME_NET,
                    "The nodes are not on the same network".to_string(),
                );
                MisbehaveResult::Disconnect
            }
            Some((flags, client_version)) => {
                let registry_client_version = |version: String| {
                    self.network_state.with_peer_registry_mut(|registry| {
                        if let Some(peer) = registry.get_peer_mut(context.session.id) {
                            peer.identify_info = Some(PeerIdentifyInfo {
                                client_version: version,
                                flags,
                            })
                        }
                    });
                };

                registry_client_version(client_version);
                // set peer flags
                self.network_state.with_peer_store_mut(|peer| {
                    peer.change_flags(context.session.address.clone(), flags.bits())
                });

                if context.session.ty.is_outbound() {
                    if self
                        .network_state
                        .with_peer_registry(|reg| reg.is_feeler(&context.session.address))
                    {
                        let _ = context
                            .open_protocols(
                                context.session.id,
                                TargetProtocol::Single(SupportProtocols::Feeler.protocol_id()),
                            )
                            .await;
                    } else if (self.network_state.target_flags_filter)(flags) {
                        // The remote end can support all local protocols.
                        let _ = context
                            .open_protocols(
                                context.session.id,
                                TargetProtocol::Filter(Box::new(move |id| {
                                    id != &SupportProtocols::Feeler.protocol_id()
                                })),
                            )
                            .await;
                    } else {
                        // The remote end cannot support all local protocols.
                        warn!("IdentifyProtocol close session, reason: the peer's flag does not meet the requirement");
                        return MisbehaveResult::Disconnect;
                    }
                }
                MisbehaveResult::Continue
            }
        }
    }

    /// Get local listen addresses
    fn local_listen_addrs(&mut self) -> Vec<Multiaddr> {
        self.listen_addrs()
    }

    fn add_remote_listen_addrs(&mut self, session: &SessionContext, addrs: Vec<Multiaddr>) {
        trace!(
            "IdentifyProtocol add remote listening addresses, session: {:?}, addresses : {:?}",
            session,
            addrs,
        );
        let flags = self.network_state.with_peer_registry_mut(|reg| {
            if let Some(peer) = reg.get_peer_mut(session.id) {
                peer.listened_addrs = addrs.clone();
                peer.identify_info
                    .as_ref()
                    .map(|a| a.flags)
                    .unwrap_or(Flags::COMPATIBILITY)
            } else {
                Flags::COMPATIBILITY
            }
        });
        self.network_state.with_peer_store_mut(|peer_store| {
            for addr in addrs {
                if let Err(err) = peer_store.add_addr(addr.clone(), flags.bits()) {
                    error!("IdentifyProtocol failed to add address to peer store, address: {}, error: {:?}", addr, err);
                }
            }
        })
    }

    fn add_observed_addr(&mut self, mut addr: Multiaddr, ty: SessionType) -> MisbehaveResult {
        if ty.is_inbound() {
            // The address already been discovered by other peer
            return MisbehaveResult::Continue;
        }

        // observed addr is not a reachable ip
        if !multiaddr_to_socketaddr(&addr)
            .map(|socket_addr| is_reachable(socket_addr.ip()))
            .unwrap_or(false)
        {
            return MisbehaveResult::Continue;
        }

        if extract_peer_id(&addr).is_none() {
            addr.push(Protocol::P2P(Cow::Borrowed(
                self.network_state.local_peer_id().as_bytes(),
            )))
        }

        let source_addr = addr.clone();
        let observed_addrs_iter = self
            .listen_addrs()
            .into_iter()
            .filter_map(|listen_addr| multiaddr_to_socketaddr(&listen_addr))
            .map(|socket_addr| {
                addr.iter()
                    .map(|proto| match proto {
                        Protocol::Tcp(_) => Protocol::Tcp(socket_addr.port()),
                        value => value,
                    })
                    .collect::<Multiaddr>()
            })
            .chain(::std::iter::once(source_addr));

        self.network_state.add_observed_addrs(observed_addrs_iter);
        // NOTE: for future usage
        MisbehaveResult::Continue
    }

    fn misbehave(&mut self, session: &SessionContext, reason: Misbehavior) -> MisbehaveResult {
        error!(
            "IdentifyProtocol detects abnormal behavior, session: {:?}, reason: {:?}",
            session, reason
        );
        MisbehaveResult::Disconnect
    }
}

#[derive(Clone)]
struct Identify {
    name: String,
    encode_data: ckb_types::bytes::Bytes,
}

impl Identify {
    fn new(name: String, flags: Flags, client_version: String) -> Self {
        Identify {
            encode_data: packed::Identify::new_builder()
                .name(name.as_str().pack())
                .flag(flags.bits().pack())
                .client_version(client_version.as_str().pack())
                .build()
                .as_bytes(),
            name,
        }
    }

    fn encode(&mut self) -> &[u8] {
        &self.encode_data
    }

    fn verify(&self, data: &[u8]) -> Option<(Flags, String)> {
        let reader = packed::IdentifyReader::from_slice(data).ok()?;

        let name = reader.name().as_utf8().ok()?.to_owned();
        if self.name != name {
            warn!(
                "IdentifyProtocol detects peer has different network identifiers, local network id: {}, remote network id: {}",
                self.name, name,
            );
            return None;
        }

        let flag: u64 = reader.flag().unpack();
        if flag == 0 {
            return None;
        }

        let raw_client_version = reader.client_version().as_utf8().ok()?.to_owned();

        Some((
            unsafe { Flags::from_bits_unchecked(flag) },
            raw_client_version,
        ))
    }
}

bitflags::bitflags! {
    pub struct Flags: u64 {
        /// Compatibility reserved
        const COMPATIBILITY = 0b1;
        /// Discovery protocol, which can provide peers data service
        const DISCOVERY = 0b10;
        /// Sync protocol, can provide Block and Header download service
        const SYNC = 0b100;
        /// Relay protocol, which can provide CompactBlock and Transaction broadcast/forwarding services
        const RELAY = 0b1000;
        /// Light client protocol, which can provide Block / Transaction data and existence proof services
        const LIGHT_CLIENT = 0b10000;
        /// Client side block filter protocol, can provide BlockFilter download service
        const BLOCK_FILTER = 0b100000;
    }
}

impl Flags {
    pub fn support_light_client(&self) -> bool {
        self.contains(Flags::LIGHT_CLIENT) || self.contains(Flags::BLOCK_FILTER)
    }
}
