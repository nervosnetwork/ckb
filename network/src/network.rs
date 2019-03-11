#![allow(clippy::needless_pass_by_value)]

use crate::ckb_protocol::{CKBProtocol, CKBProtocols};
use crate::ckb_protocol_handler::CKBProtocolHandler;
use crate::ckb_protocol_handler::DefaultCKBProtocolContext;
use crate::ckb_service::CKBService;
use crate::identify_service::IdentifyService;
use crate::outbound_peer_service::OutboundPeerService;
use crate::peer_store::{Behaviour, PeerStore, SqlitePeerStore};
use crate::peers_registry::{ConnectionStatus, PeerConnection, PeerIdentifyInfo, PeersRegistry};
use crate::ping_service::PingService;
use crate::protocol::Protocol;
use crate::protocol_service::ProtocolService;
use crate::timer_service::TimerService;
use crate::transport::{new_transport, TransportOutput};
use crate::NetworkConfig;
use crate::{Error, ErrorKind, PeerIndex, ProtocolId};
use bytes::Bytes;
use ckb_util::{Mutex, RwLock};
use fnv::FnvHashMap;
use futures::future::{self, select_all, Future};
use futures::sync::mpsc::UnboundedSender;
use futures::sync::oneshot;
use futures::Stream;
use libp2p::core::{upgrade, MuxedTransport, PeerId};
use libp2p::core::{Endpoint, Multiaddr, UniqueConnec};
use libp2p::core::{PublicKey, SwarmController};
use libp2p::{self, identify, ping, secio, Transport, TransportTimeout};
use log::{debug, info, trace, warn};
use std::boxed::Box;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::sync::Arc;
use std::time::Duration;
use std::usize;
use tokio::io::{AsyncRead, AsyncWrite};

const DIAL_BOOTNODE_TIMEOUT: u64 = 20;
const PEER_ADDRS_COUNT: u32 = 5;

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub peer: PeerInfo,
    pub protocol_version: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub peer_id: PeerId,
    pub endpoint_role: Endpoint,
    pub last_ping_time: Option<u64>,
    pub connected_addr: Multiaddr,
    pub identify_info: Option<PeerIdentifyInfo>,
}

impl PeerInfo {
    #[inline]
    pub fn is_outbound(&self) -> bool {
        self.endpoint_role == Endpoint::Dialer
    }

    #[inline]
    pub fn is_inbound(&self) -> bool {
        !self.is_outbound()
    }
}

pub struct Network {
    peers_registry: RwLock<PeersRegistry>,
    peer_store: Arc<RwLock<dyn PeerStore>>,
    listened_addresses: RwLock<FnvHashMap<Multiaddr, u8>>,
    pub(crate) original_listened_addresses: RwLock<Vec<Multiaddr>>,
    pub(crate) ckb_protocols: CKBProtocols<Arc<CKBProtocolHandler>>,
    local_private_key: secio::SecioKeyPair,
    local_peer_id: PeerId,
}

impl Network {
    pub fn report(&self, peer_id: &PeerId, behaviour: Behaviour) {
        self.peer_store.write().report(peer_id, behaviour);
    }

    pub fn drop_peer(&self, peer_id: &PeerId) {
        self.peers_registry.write().drop_peer(&peer_id);
    }

    pub fn local_peer_id(&self) -> &PeerId {
        &self.local_peer_id
    }

    pub(crate) fn discovery_listened_address(&self, addr: Multiaddr) {
        let mut listened_addresses = self.listened_addresses.write();
        let score = listened_addresses.entry(addr).or_insert(0);
        *score = score.saturating_add(1);
    }

    pub(crate) fn listened_addresses(&self, count: usize) -> Vec<(Multiaddr, u8)> {
        let listened_addresses = self.listened_addresses.read();
        listened_addresses
            .iter()
            .take(count)
            .map(|(addr, score)| (addr.to_owned(), *score))
            .collect()
    }

    pub(crate) fn get_peer_index(&self, peer_id: &PeerId) -> Option<PeerIndex> {
        let peers_registry = self.peers_registry.read();
        peers_registry
            .get(&peer_id)
            .and_then(|peer| peer.peer_index)
    }

    pub(crate) fn get_peer_id(&self, peer_index: PeerIndex) -> Option<PeerId> {
        let peers_registry = self.peers_registry.read();
        peers_registry
            .get_peer_id(peer_index)
            .map(|peer_id| peer_id.to_owned())
    }

    pub(crate) fn connection_status(&self) -> ConnectionStatus {
        let peers_registry = self.peers_registry.read();
        peers_registry.connection_status()
    }

    pub(crate) fn modify_peer<F>(&self, peer_id: &PeerId, mut f: F) -> bool
    where
        F: FnMut(&mut PeerConnection) -> (),
    {
        let mut peers_registry = self.peers_registry.write();
        match peers_registry.get_mut(peer_id) {
            Some(peer) => {
                f(peer);
                true
            }
            None => false,
        }
    }

    pub(crate) fn get_peer_identify_info(&self, peer_id: &PeerId) -> Option<PeerIdentifyInfo> {
        let peers_registry = self.peers_registry.read();
        peers_registry
            .get(peer_id)
            .and_then(|peer| peer.identify_info.clone())
    }

    pub(crate) fn set_peer_identify_info(
        &self,
        peer_id: &PeerId,
        identify_info: PeerIdentifyInfo,
    ) -> Result<(), ()> {
        let mut peers_registry = self.peers_registry.write();
        match peers_registry.get_mut(peer_id) {
            Some(peer) => {
                peer.identify_info = Some(identify_info);
                Ok(())
            }
            None => Err(()),
        }
    }

    pub(crate) fn get_peer_pinger(&self, peer_id: &PeerId) -> Option<UniqueConnec<ping::Pinger>> {
        let peers_registry = self.peers_registry.read();
        peers_registry
            .get(peer_id)
            .map(|peer| peer.pinger_loader.clone())
    }

    pub(crate) fn get_peer_addresses(&self, peer_id: &PeerId) -> Vec<Multiaddr> {
        let peer_store = self.peer_store.read();
        let addrs = peer_store.peer_addrs(&peer_id, PEER_ADDRS_COUNT);
        addrs.unwrap_or_default()
    }

    pub(crate) fn peers(&self) -> impl Iterator<Item = PeerId> {
        let peers_registry = self.peers_registry.read();
        let peers = peers_registry
            .peers_iter()
            .map(|(peer_id, _peer)| peer_id.to_owned())
            .collect::<Vec<_>>();
        peers.into_iter()
    }

    pub(crate) fn peers_indexes(&self) -> Vec<PeerIndex> {
        let peers_registry = self.peers_registry.read();
        let iter = peers_registry.connected_peers_indexes();
        iter.collect::<Vec<_>>()
    }

    #[inline]
    pub(crate) fn ban_peer(&self, peer_id: &PeerId, timeout: Duration) {
        let mut peers_registry = self.peers_registry.write();
        peers_registry.drop_peer(peer_id);
        self.peer_store.write().ban_peer(peer_id, timeout);
    }

    #[inline]
    pub(crate) fn peer_store(&self) -> &RwLock<dyn PeerStore> {
        &self.peer_store
    }

    pub(crate) fn local_private_key(&self) -> &secio::SecioKeyPair {
        &self.local_private_key
    }

    pub(crate) fn local_public_key(&self) -> PublicKey {
        self.local_private_key.to_public_key()
    }

    pub fn external_urls(&self, max_urls: usize) -> Vec<(String, u8)> {
        let original_listened_addresses = self.original_listened_addresses.read();
        self.listened_addresses(max_urls.saturating_sub(original_listened_addresses.len()))
            .into_iter()
            .chain(
                original_listened_addresses
                    .iter()
                    .map(|addr| (addr.to_owned(), 1)),
            )
            .map(|(addr, score)| (self.to_external_url(&addr), score))
            .collect()
    }

    pub fn node_id(&self) -> String {
        self.local_private_key.to_peer_id().to_base58()
    }

    // A workaround method for `add_node` rpc call, need to re-write it after new p2p lib integration.
    pub fn add_node(&self, peer_id: &PeerId, address: Multiaddr) {
        let _ = self
            .peer_store()
            .write()
            .add_discovered_address(peer_id, address);
    }

    fn to_external_url(&self, addr: &Multiaddr) -> String {
        format!("{}/p2p/{}", addr, self.node_id())
    }

    pub(crate) fn send(
        &self,
        peer_id: &PeerId,
        protocol_id: ProtocolId,
        data: Bytes,
    ) -> Result<(), Error> {
        if let Some(peer) = self.peers_registry.read().get(peer_id) {
            if let Some(sender) = peer
                .ckb_protocols
                .iter()
                .find(|(id, _)| id == &protocol_id)
                .and_then(|(_, protocol_connec)| protocol_connec.poll())
                .map(|(sender, _)| sender)
            {
                sender.unbounded_send(data).map_err(|err| {
                    Error::from(ErrorKind::Other(format!("send to error: {:?}", err)))
                })?;
                Ok(())
            } else {
                Err(ErrorKind::Other(format!(
                    "can't find protocol: {:?} for peer {:?}",
                    protocol_id, peer_id
                ))
                .into())
            }
        } else {
            Err(ErrorKind::PeerNotFound.into())
        }
    }

    fn ckb_protocol_connec(
        &self,
        peer: &mut PeerConnection,
        protocol_id: ProtocolId,
    ) -> UniqueConnec<(UnboundedSender<Bytes>, u8)> {
        peer.ckb_protocols
            .iter()
            .find(|&(id, _)| id == &protocol_id)
            .map(|(_, ref protocol_connec)| protocol_connec.clone())
            .unwrap_or_else(|| {
                let protocol_connec = UniqueConnec::empty();
                peer.ckb_protocols
                    .push((protocol_id, protocol_connec.clone()));
                protocol_connec
            })
    }
    pub(crate) fn try_outbound_ckb_protocol_connec(
        &self,
        peer_id: &PeerId,
        protocol_id: ProtocolId,
        connected_addr: Multiaddr,
    ) -> Result<UniqueConnec<(UnboundedSender<Bytes>, u8)>, Error> {
        let mut peers_registry = self.peers_registry.write();
        // get peer protocol_connection
        match peers_registry.try_outbound_peer(peer_id.clone(), connected_addr.clone()) {
            Ok(_) => {
                let _ = self
                    .peer_store()
                    .write()
                    .add_discovered_address(peer_id, connected_addr);
                let peer = peers_registry.get_mut(&peer_id).unwrap();
                Ok(self.ckb_protocol_connec(peer, protocol_id))
            }
            Err(err) => Err(err),
        }
    }

    pub(crate) fn try_inbound_ckb_protocol_connec(
        &self,
        peer_id: &PeerId,
        protocol_id: ProtocolId,
        connected_addr: Multiaddr,
    ) -> Result<UniqueConnec<(UnboundedSender<Bytes>, u8)>, Error> {
        let mut peers_registry = self.peers_registry.write();
        // get peer protocol_connection
        match peers_registry.accept_inbound_peer(peer_id.clone(), connected_addr.clone()) {
            Ok(_) => {
                let _ = self
                    .peer_store()
                    .write()
                    .add_discovered_address(peer_id, connected_addr);
                let peer = peers_registry.get_mut(&peer_id).unwrap();
                Ok(self.ckb_protocol_connec(peer, protocol_id))
            }
            Err(err) => Err(err),
        }
    }

    pub fn peer_protocol_version(&self, peer_id: &PeerId, protocol_id: ProtocolId) -> Option<u8> {
        let peers_registry = self.peers_registry.read();
        match peers_registry.get(peer_id) {
            Some(peer) => match peer.ckb_protocols.iter().find(|(id, _)| id == &protocol_id) {
                Some((_, protocol_connec)) => protocol_connec.poll().map(|(_, version)| version),
                None => None,
            },
            None => None,
        }
    }
    pub fn session_info(&self, peer_id: &PeerId, protocol_id: ProtocolId) -> Option<SessionInfo> {
        let peers_registry = self.peers_registry.read();
        match peers_registry.get(peer_id) {
            Some(peer) => {
                let protocol_version =
                    match peer.ckb_protocols.iter().find(|(id, _)| id == &protocol_id) {
                        Some((_, protocol_connec)) => {
                            protocol_connec.poll().map(|(_, version)| version)
                        }
                        None => None,
                    };
                let session = SessionInfo {
                    peer: PeerInfo {
                        peer_id: peer_id.to_owned(),
                        endpoint_role: peer.endpoint_role,
                        last_ping_time: peer.last_ping_time,
                        connected_addr: peer.connected_addr.clone(),
                        identify_info: peer.identify_info.clone(),
                    },
                    protocol_version,
                };
                Some(session)
            }
            None => None,
        }
    }

    pub fn dial_to_peer<Tran, To, St, C>(
        &self,
        transport: Tran,
        addr: &Multiaddr,
        expected_peer_id: &PeerId,
        swarm_controller: &SwarmController<St, Box<Future<Item = (), Error = IoError> + Send>>,
        timeout: Duration,
    ) where
        Tran: MuxedTransport<Output = TransportOutput<To>> + Send + Clone + 'static,
        Tran::MultiaddrFuture: Send + 'static,
        Tran::Dial: Send,
        Tran::Listener: Send,
        Tran::ListenerUpgrade: Send,
        Tran::Incoming: Send,
        Tran::IncomingUpgrade: Send,
        To: AsyncRead + AsyncWrite + Send + 'static,
        St: MuxedTransport<Output = Protocol<C>> + Send + Clone + 'static,
        St::Dial: Send,
        St::MultiaddrFuture: Send,
        St::Listener: Send,
        St::ListenerUpgrade: Send,
        St::Incoming: Send,
        St::IncomingUpgrade: Send,
        C: Send + 'static,
    {
        if expected_peer_id == self.local_peer_id() {
            debug!(target: "network", "ignore dial to self");
            return;
        }
        debug!(target: "network", "dial to peer {:?} address {:?}", expected_peer_id, addr);
        for protocol in &self.ckb_protocols.0 {
            self.dial_to_peer_protocol(
                transport.clone(),
                addr,
                protocol.to_owned(),
                expected_peer_id,
                swarm_controller,
                timeout,
            )
        }
    }

    fn dial_to_peer_protocol<Tran, To, St, C>(
        &self,
        transport: Tran,
        addr: &Multiaddr,
        protocol: CKBProtocol<Arc<CKBProtocolHandler>>,
        expected_peer_id: &PeerId,
        swarm_controller: &SwarmController<St, Box<Future<Item = (), Error = IoError> + Send>>,
        timeout: Duration,
    ) where
        Tran: MuxedTransport<Output = TransportOutput<To>> + Send + Clone + 'static,
        Tran::MultiaddrFuture: Send + 'static,
        Tran::Dial: Send,
        Tran::Listener: Send,
        Tran::ListenerUpgrade: Send,
        Tran::Incoming: Send,
        Tran::IncomingUpgrade: Send,
        To: AsyncRead + AsyncWrite + Send + 'static,
        St: MuxedTransport<Output = Protocol<C>> + Send + Clone + 'static,
        St::Dial: Send,
        St::MultiaddrFuture: Send,
        St::Listener: Send,
        St::ListenerUpgrade: Send,
        St::Incoming: Send,
        St::IncomingUpgrade: Send,
        C: Send + 'static,
    {
        trace!(
        target: "network",
        "prepare open protocol {:?} to {:?}",
        protocol.base_name(),
        addr
        );

        let protocol_id = protocol.id();
        let transport = transport.clone().and_then({
            let addr = addr.clone();
            move |out, endpoint, client_addr| {
                let peer_id = out.peer_id;
                upgrade::apply(out.socket, protocol, endpoint, client_addr).map(
                    move |(output, client_addr)| {
                        (
                            (
                                peer_id.clone(),
                                Protocol::CKBProtocol(output, peer_id, addr),
                            ),
                            client_addr,
                        )
                    },
                )
            }
        });

        let transport = TransportTimeout::new(transport, timeout);
        let unique_connec = match self.try_outbound_ckb_protocol_connec(
            expected_peer_id,
            protocol_id,
            addr.to_owned(),
        ) {
            Ok(unique_connec) => unique_connec,
            Err(_) => return,
        };

        let transport = transport.and_then({
                let expected_peer_id = expected_peer_id.clone();
                move |(peer_id, protocol), _, client_addr| {
                    if peer_id == expected_peer_id {
                        debug!(target: "network", "success connect to {:?}", peer_id);
                        future::ok((protocol, client_addr))
                    } else {
                        debug!(target: "network", "connected peer id mismatch {:?}, disconnect!", peer_id);
                        //Because multiaddrs is responsed by a third-part node, the mismatched
                        //peer itself should not seems as a misbehaviour peer.
                        //So we do not report this behaviour
                        future::err(IoError::new(
                                IoErrorKind::ConnectionRefused,
                                "Peer id mismatch",
                                ))
                    }
                }
            });

        trace!(
        target: "network",
        "Opening connection to {:?} addr {} with protocol {:?}",
        expected_peer_id,
        addr,
        protocol_id
        );
        let _ = unique_connec.dial(swarm_controller, addr, transport);
    }

    pub(crate) fn inner_build(
        config: &NetworkConfig,
        ckb_protocols: Vec<CKBProtocol<Arc<CKBProtocolHandler>>>,
    ) -> Result<Arc<Self>, Error> {
        let local_private_key = match config.fetch_private_key() {
            Some(private_key) => private_key?,
            None => return Err(ErrorKind::Other("secret_key not set".to_owned()).into()),
        };
        // set max score to public addresses
        let listened_addresses: FnvHashMap<Multiaddr, u8> = config
            .public_addresses
            .iter()
            .map(|addr| (addr.to_owned(), std::u8::MAX))
            .collect();
        let peer_store: Arc<RwLock<dyn PeerStore>> = {
            let mut peer_store = SqlitePeerStore::default();
            let bootnodes = config.bootnodes()?;
            for (peer_id, addr) in bootnodes {
                peer_store.add_bootnode(peer_id, addr);
            }
            Arc::new(RwLock::new(peer_store))
        };
        let reserved_peers = config
            .reserved_peers()?
            .iter()
            .map(|(peer_id, _)| peer_id.to_owned())
            .collect::<Vec<_>>();
        let peers_registry = PeersRegistry::new(
            Arc::clone(&peer_store),
            config.max_inbound_peers,
            config.max_outbound_peers,
            config.reserved_only,
            reserved_peers,
        );
        let network: Arc<Network> = Arc::new(Network {
            peers_registry: RwLock::new(peers_registry),
            peer_store: Arc::clone(&peer_store),
            listened_addresses: RwLock::new(listened_addresses),
            original_listened_addresses: RwLock::new(Vec::new()),
            ckb_protocols: CKBProtocols(ckb_protocols),
            local_private_key: local_private_key.clone(),
            local_peer_id: local_private_key.to_peer_id(),
        });
        Ok(network)
    }

    pub(crate) fn build_network_future(
        network: Arc<Network>,
        config: &NetworkConfig,
        close_rx: oneshot::Receiver<()>,
    ) -> Result<Box<Future<Item = (), Error = IoError> + Send>, Error> {
        let local_private_key = network.local_private_key().to_owned();
        let local_peer_id: PeerId = local_private_key.to_peer_id();
        let basic_transport_timeout = config.transport_timeout;
        let client_version = config.client_version.clone();
        let protocol_version = config.protocol_version.clone();
        let max_outbound = config.max_outbound_peers as usize;
        let basic_transport = {
            let basic_transport = new_transport(local_private_key, basic_transport_timeout)
                .map_err_dial({
                    move |err, addr| {
                        trace!(target: "network", "Failed to connect to peer {}, error: {:?}", addr, err);
                        err
                    }
                });
            Transport::and_then(basic_transport, {
                // Register new peers information
                let local_peer_id = local_peer_id.clone();
                move |(peer_id, stream), _endpoint, remote_addr_fut| {
                    remote_addr_fut.and_then(move |remote_addr| {
                        debug!(target: "network", "connection from {:?} peer_id: {:?}", remote_addr, peer_id);
                        if peer_id == local_peer_id {
                            debug!(target: "network", "connect to self, disconnect");
                            return Err(IoErrorKind::ConnectionRefused.into());
                        }
                        let out = TransportOutput {
                            socket: stream,
                            peer_id,
                            original_addr: remote_addr.clone(),
                        };
                        Ok((out, future::ok(remote_addr)))
                    })
                }
            })
        };

        // initialize ckb_protocols
        let timer_registry = Arc::new(Mutex::new(Some(Vec::new())));
        for protocol in &network.ckb_protocols.0 {
            protocol.protocol_handler().initialize(Box::new(
                DefaultCKBProtocolContext::with_timer_registry(
                    Arc::clone(&network),
                    protocol.id(),
                    Arc::clone(&timer_registry),
                ),
            ));
        }
        let ping_service = Arc::new(PingService::new(config.ping_interval, config.ping_timeout));
        let identify_service = Arc::new(IdentifyService {
            client_version,
            protocol_version,
            identify_timeout: config.identify_timeout,
            identify_interval: config.identify_interval,
        });

        let ckb_protocol_service = Arc::new(CKBService {});
        let timer_service = Arc::new(TimerService {
            timer_registry: Arc::clone(&timer_registry),
        });
        let outbound_peer_service = Arc::new(OutboundPeerService {
            try_connect_interval: config.try_outbound_connect_interval,
            timeout: config.try_outbound_connect_timeout,
        });
        // Transport used to handling received connections
        let handling_transport = {
            let transport = basic_transport.clone();
            transport.and_then({
                let network = Arc::clone(&network);
                move |out, endpoint, fut| {
                    let peer_id = Arc::new(out.peer_id);
                    let original_addr = out.original_addr;
                    // upgrades and apply protocols
                    let ping_upgrade = upgrade::map_with_addr(libp2p::ping::Ping, {
                        let peer_id = Arc::clone(&peer_id);
                        move |out, addr| PingService::convert_to_protocol(peer_id, addr, out)
                    });
                    let identify_upgrade =
                        upgrade::map_with_addr(identify::IdentifyProtocolConfig, {
                            let peer_id = Arc::clone(&peer_id);
                            let original_addr = original_addr.clone();
                            move |out, _addr| {
                                IdentifyService::convert_to_protocol(peer_id, &original_addr, out)
                            }
                        });
                    let ckb_protocols_upgrade =
                        upgrade::map_with_addr(network.ckb_protocols.clone(), {
                            let peer_id = Arc::clone(&peer_id);
                            move |out, addr| CKBService::convert_to_protocol(peer_id, addr, out)
                        });
                    let all_upgrade = upgrade::or(
                        ckb_protocols_upgrade,
                        upgrade::or(identify_upgrade, ping_upgrade),
                    );
                    upgrade::apply(out.socket, all_upgrade, endpoint, fut)
                }
            })
        };
        let (swarm_controller, swarm_events) = libp2p::core::swarm(handling_transport, {
            let ping_service = Arc::clone(&ping_service);
            let identify_service = Arc::clone(&identify_service);
            let ckb_protocol_service = Arc::clone(&ckb_protocol_service);
            let network = Arc::clone(&network);
            move |protocol, _addr| match protocol {
                Protocol::Ping(..) | Protocol::Pong(..) => ping_service
                    .handle(Arc::clone(&network), protocol)
                    as Box<Future<Item = (), Error = IoError> + Send>,
                Protocol::IdentifyRequest(..) | Protocol::IdentifyResponse(..) => {
                    identify_service.handle(Arc::clone(&network), protocol)
                }
                Protocol::CKBProtocol(..) => {
                    ckb_protocol_service.handle(Arc::clone(&network), protocol)
                }
            }
        });

        // listen_on local addresses
        for addr in &config.listen_addresses {
            match swarm_controller.listen_on(addr.clone()) {
                Ok(listen_address) => {
                    info!(
                    target: "network",
                    "Listen on address: {}",
                    network.to_external_url(&listen_address)
                    );
                    network
                        .original_listened_addresses
                        .write()
                        .push(listen_address.clone())
                }
                Err(err) => {
                    warn!(
                    target: "network",
                    "listen on address {} failed, due to error: {}",
                    addr.clone(),
                    err
                    );
                    return Err(ErrorKind::Other(format!("listen address error: {:?}", err)).into());
                }
            };
        }

        // dial reserved nodes and bootnodes
        {
            let network = Arc::clone(&network);
            let dial_timeout = Duration::from_secs(DIAL_BOOTNODE_TIMEOUT);
            // dial reserved_nodes
            for (peer_id, addr) in config.reserved_peers()? {
                network.dial_to_peer(
                    basic_transport.clone(),
                    &addr,
                    &peer_id,
                    &swarm_controller,
                    dial_timeout,
                );
            }

            let bootnodes = network
                .peer_store()
                .read()
                .bootnodes((max_outbound / 2) as u32)
                .clone();
            // dial half bootnodes
            for (peer_id, addr) in bootnodes {
                debug!(target: "network", "dial bootnode {:?} {:?}", peer_id, addr);
                network.dial_to_peer(
                    basic_transport.clone(),
                    &addr,
                    &peer_id,
                    &swarm_controller,
                    dial_timeout,
                );
            }
        }

        // prepare services futures
        let futures: Vec<Box<Future<Item = (), Error = IoError> + Send>> = vec![
            Box::new(swarm_events.for_each(|_| Ok(()))),
            // Box::new(
            //     discovery_query_service
            //         .into_future()
            //         .map(|_| ())
            //         .map_err(|(err, _)| err),
            // ) as Box<Future<Item = (), Error = IoError> + Send>,
            ping_service.start_protocol(
                Arc::clone(&network),
                swarm_controller.clone(),
                basic_transport.clone(),
            ),
            identify_service.start_protocol(
                Arc::clone(&network),
                swarm_controller.clone(),
                basic_transport.clone(),
            ),
            timer_service.start_protocol(
                Arc::clone(&network),
                swarm_controller.clone(),
                basic_transport.clone(),
            ),
            outbound_peer_service.start_protocol(
                Arc::clone(&network),
                swarm_controller.clone(),
                basic_transport.clone(),
            ),
            Box::new(close_rx.map_err(|err| IoError::new(IoErrorKind::Other, err))),
        ];
        let service_futures = select_all(futures)
            .and_then({
                let network = Arc::clone(&network);
                move |_| {
                    let mut peers_registry = network.peers_registry.write();
                    debug!(target: "network", "drop all connections...");
                    peers_registry.drop_all();
                    Ok(())
                }
            })
            .map_err(|(err, _, _)| {
                debug!(target: "network", "network exit, error {:?}", err);
                err
            });
        let service_futures =
            Box::new(service_futures) as Box<Future<Item = (), Error = IoError> + Send>;
        Ok(service_futures)
    }

    #[allow(clippy::type_complexity)]
    pub fn build(
        config: &NetworkConfig,
        ckb_protocols: Vec<CKBProtocol<Arc<CKBProtocolHandler>>>,
    ) -> Result<
        (
            Arc<Self>,
            oneshot::Sender<()>,
            Box<Future<Item = (), Error = IoError> + Send>,
        ),
        Error,
    > {
        let network = Self::inner_build(config, ckb_protocols)?;
        let (close_tx, close_rx) = oneshot::channel();
        let network_future = Self::build_network_future(Arc::clone(&network), &config, close_rx)?;
        Ok((network, close_tx, network_future))
    }
}
