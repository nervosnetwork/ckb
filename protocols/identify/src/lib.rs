use std::collections::HashMap;
use std::time::{Duration, Instant};

use ckb_logger::{debug, error, trace, warn};
use p2p::{
    bytes::Bytes,
    context::{ProtocolContext, ProtocolContextMutRef, SessionContext},
    multiaddr::{Multiaddr, Protocol},
    secio::PeerId,
    service::SessionType,
    traits::ServiceProtocol,
    utils::{is_reachable, multiaddr_to_socketaddr},
    SessionId,
};

mod protocol;

use protocol::IdentifyMessage;

const CHECK_TIMEOUT_TOKEN: u64 = 100;
// Check timeout interval (seconds)
const CHECK_TIMEOUT_INTERVAL: u64 = 1;
const DEFAULT_TIMEOUT: u64 = 8;
const MAX_ADDRS: usize = 10;

/// The misbehavior to report to underlying peer storage
pub enum Misbehavior {
    /// Repeat send listen addresses
    DuplicateListenAddrs,
    /// Repeat send observed address
    DuplicateObservedAddr,
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
    pub fn is_continue(&self) -> bool {
        match self {
            MisbehaveResult::Continue => true,
            _ => false,
        }
    }
    pub fn is_disconnect(&self) -> bool {
        match self {
            MisbehaveResult::Disconnect => true,
            _ => false,
        }
    }
}

/// The trait to communicate with underlying peer storage
pub trait Callback: Clone + Send {
    /// Received custom message
    fn received_identify(
        &mut self,
        context: &mut ProtocolContextMutRef,
        identify: &[u8],
    ) -> MisbehaveResult;
    /// Get custom identify message
    fn identify(&mut self) -> &[u8];
    /// Get local listen addresses
    fn local_listen_addrs(&mut self) -> Vec<Multiaddr>;
    /// Add remote peer's listen addresses
    fn add_remote_listen_addrs(&mut self, peer: &PeerId, addrs: Vec<Multiaddr>);
    /// Add our address observed by remote peer
    fn add_observed_addr(
        &mut self,
        peer: &PeerId,
        addr: Multiaddr,
        ty: SessionType,
    ) -> MisbehaveResult;
    /// Report misbehavior
    fn misbehave(&mut self, peer: &PeerId, kind: Misbehavior) -> MisbehaveResult;
}

/// Identify protocol
pub struct IdentifyProtocol<T> {
    callback: T,
    remote_infos: HashMap<SessionId, RemoteInfo>,
    secio_enabled: bool,
    global_ip_only: bool,
}

impl<T: Callback> IdentifyProtocol<T> {
    pub fn new(callback: T) -> IdentifyProtocol<T> {
        IdentifyProtocol {
            callback,
            remote_infos: HashMap::default(),
            secio_enabled: true,
            global_ip_only: true,
        }
    }

    /// Turning off global ip only mode will allow any ip to be broadcast, default is true
    pub fn global_ip_only(mut self, global_ip_only: bool) -> Self {
        self.global_ip_only = global_ip_only;
        self
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

        if info.listen_addrs.is_some() {
            debug!("remote({:?}) repeat send observed address", info.peer_id);
            self.callback
                .misbehave(&info.peer_id, Misbehavior::DuplicateListenAddrs)
        } else if listens.len() > MAX_ADDRS {
            self.callback
                .misbehave(&info.peer_id, Misbehavior::TooManyAddresses(listens.len()))
        } else {
            trace!("received listen addresses: {:?}", listens);
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
                .add_remote_listen_addrs(&info.peer_id, reachable_addrs.clone());
            info.listen_addrs = Some(reachable_addrs);
            MisbehaveResult::Continue
        }
    }

    fn process_observed(
        &mut self,
        context: &mut ProtocolContextMutRef,
        observed: Multiaddr,
    ) -> MisbehaveResult {
        let session = context.session;
        let mut info = self
            .remote_infos
            .get_mut(&session.id)
            .expect("RemoteInfo must exists");

        if info.observed_addr.is_some() {
            debug!("remote({:?}) repeat send listen addresses", info.peer_id);
            self.callback
                .misbehave(&info.peer_id, Misbehavior::DuplicateObservedAddr)
        } else {
            trace!("received observed address: {}", observed);

            let global_ip_only = self.global_ip_only;
            if multiaddr_to_socketaddr(&observed)
                .map(|socket_addr| socket_addr.ip())
                .filter(|ip_addr| !global_ip_only || is_reachable(*ip_addr))
                .is_some()
                && self
                    .callback
                    .add_observed_addr(&info.peer_id, observed.clone(), info.session.ty)
                    .is_disconnect()
            {
                return MisbehaveResult::Disconnect;
            }
            info.observed_addr = Some(observed);
            MisbehaveResult::Continue
        }
    }
}

pub(crate) struct RemoteInfo {
    peer_id: PeerId,
    session: SessionContext,
    connected_at: Instant,
    timeout: Duration,
    listen_addrs: Option<Vec<Multiaddr>>,
    observed_addr: Option<Multiaddr>,
}

impl RemoteInfo {
    fn new(session: SessionContext, timeout: Duration) -> RemoteInfo {
        let peer_id = session
            .remote_pubkey
            .as_ref()
            .map(|key| PeerId::from_public_key(&key))
            .expect("secio must enabled!");
        RemoteInfo {
            peer_id,
            session,
            connected_at: Instant::now(),
            timeout,
            listen_addrs: None,
            observed_addr: None,
        }
    }
}

impl<T: Callback> ServiceProtocol for IdentifyProtocol<T> {
    fn init(&mut self, context: &mut ProtocolContext) {
        let proto_id = context.proto_id;
        if context
            .set_service_notify(
                proto_id,
                Duration::from_secs(CHECK_TIMEOUT_INTERVAL),
                CHECK_TIMEOUT_TOKEN,
            )
            .is_err()
        {
            warn!("identify start fail")
        }
    }

    fn connected(&mut self, context: ProtocolContextMutRef, _version: &str) {
        let session = context.session;
        if session.remote_pubkey.is_none() {
            error!("IdentifyProtocol require secio enabled!");
            let _ = context.disconnect(session.id);
            self.secio_enabled = false;
            return;
        }

        let remote_info = RemoteInfo::new(session.clone(), Duration::from_secs(DEFAULT_TIMEOUT));
        trace!("IdentifyProtocol sconnected from {:?}", remote_info.peer_id);
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

        let observed_addr = session
            .address
            .iter()
            .filter(|proto| match proto {
                Protocol::P2P(_) => false,
                _ => true,
            })
            .collect::<Multiaddr>();

        let identify = self.callback.identify();
        let data = IdentifyMessage::new(listen_addrs, observed_addr, identify).encode();
        let _ = context.quick_send_message(data);
    }

    fn disconnected(&mut self, context: ProtocolContextMutRef) {
        if self.secio_enabled {
            let info = self
                .remote_infos
                .remove(&context.session.id)
                .expect("RemoteInfo must exists");
            trace!("IdentifyProtocol disconnected from {:?}", info.peer_id);
        }
    }

    fn received(&mut self, mut context: ProtocolContextMutRef, data: Bytes) {
        if !self.secio_enabled {
            return;
        }

        let session = context.session;

        match IdentifyMessage::decode(&data) {
            Some(message) => {
                // Need to interrupt processing, avoid pollution
                if self
                    .callback
                    .received_identify(&mut context, message.identify)
                    .is_disconnect()
                    || self
                        .process_listens(&mut context, message.listen_addrs)
                        .is_disconnect()
                    || self
                        .process_observed(&mut context, message.observed_addr)
                        .is_disconnect()
                {
                    let _ = context.disconnect(session.id);
                }
            }
            None => {
                let info = self
                    .remote_infos
                    .get(&session.id)
                    .expect("RemoteInfo must exists");
                debug!(
                    "IdentifyProtocol received invalid data from {:?}",
                    info.peer_id
                );
                if self
                    .callback
                    .misbehave(&info.peer_id, Misbehavior::InvalidData)
                    .is_disconnect()
                {
                    let _ = context.disconnect(session.id);
                }
            }
        }
    }

    fn notify(&mut self, context: &mut ProtocolContext, _token: u64) {
        if !self.secio_enabled {
            return;
        }

        let now = Instant::now();
        for (session_id, info) in &self.remote_infos {
            if (info.listen_addrs.is_none() || info.observed_addr.is_none())
                && (info.connected_at + info.timeout) <= now
            {
                debug!("{:?} receive identify message timeout", info.peer_id);
                if self
                    .callback
                    .misbehave(&info.peer_id, Misbehavior::Timeout)
                    .is_disconnect()
                {
                    let _ = context.disconnect(*session_id);
                }
            }
        }
    }
}
