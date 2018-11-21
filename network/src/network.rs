#![cfg_attr(feature = "cargo-clippy", allow(needless_pass_by_value))]

use super::NetworkConfig;
use super::{Error, ErrorKind, ProtocolId};
use bytes::Bytes;
use ckb_protocol::{CKBProtocol, CKBProtocols};
use ckb_protocol_handler::CKBProtocolHandler;
use ckb_protocol_handler::DefaultCKBProtocolContext;
use ckb_service::CKBService;
use ckb_util::{Mutex, RwLock};
use discovery_service::{DiscoveryQueryService, DiscoveryService, KadManage};
use futures::future::{self, select_all, Future};
use futures::sync::mpsc::UnboundedSender;
use futures::sync::oneshot;
use futures::Stream;
use identify_service::IdentifyService;
use libp2p::core::{upgrade, MuxedTransport, PeerId};
use libp2p::core::{Endpoint, Multiaddr, UniqueConnec};
use libp2p::core::{PublicKey, SwarmController};
use libp2p::{self, identify, kad, secio, Transport, TransportTimeout};
use memory_peer_store::MemoryPeerStore;
use outgoing_service::OutgoingService;
use peer_store::{Behaviour, PeerStore};
use peers_registry::PeerIdentifyInfo;
use peers_registry::PeersRegistry;
use ping_service::PingService;
use protocol::Protocol;
use protocol_service::ProtocolService;
use std::boxed::Box;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use std::usize;
use timer_service::TimerService;
use tokio::io::{AsyncRead, AsyncWrite};
use transport::{new_transport, TransportOutput};

// const WAIT_LOCK_TIMEOUT: u64 = 3;
const KBUCKETS_TIMEOUT: u64 = 600;
const DIAL_BOOTNODE_TIMEOUT: u64 = 20;

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub peer: PeerInfo,
    pub protocol_version: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub peer_id: PeerId,
    pub endpoint_role: Endpoint,
    pub last_ping_time: Option<Instant>,
    pub remote_addresses: Vec<Multiaddr>,
    pub identify_info: Option<PeerIdentifyInfo>,
}

impl PeerInfo {
    #[inline]
    pub fn is_outgoing(&self) -> bool {
        self.endpoint_role == Endpoint::Dialer
    }

    #[inline]
    pub fn is_incoming(&self) -> bool {
        !self.is_outgoing()
    }
}

pub struct Network {
    peers_registry: RwLock<PeersRegistry>,
    peer_store: Arc<RwLock<Box<PeerStore>>>,
    pub(crate) listened_addresses: RwLock<Vec<Multiaddr>>,
    pub(crate) original_listened_addresses: RwLock<Vec<Multiaddr>>,
    pub(crate) ckb_protocols: CKBProtocols<Arc<CKBProtocolHandler>>,
    local_private_key: secio::SecioKeyPair,
}

impl Network {
    // keep peers_registry function crate available, to avoiding lock race condition from outside.
    #[inline]
    pub(crate) fn peers_registry<'a>(&'a self) -> &'a RwLock<PeersRegistry> {
        &self.peers_registry
    }

    #[inline]
    pub(crate) fn peer_store<'a>(&'a self) -> &'a RwLock<Box<PeerStore>> {
        &self.peer_store
    }

    pub(crate) fn local_private_key(&self) -> &secio::SecioKeyPair {
        &self.local_private_key
    }

    pub(crate) fn local_public_key(&self) -> PublicKey {
        self.local_private_key.to_public_key()
    }

    pub fn external_url(&self) -> Option<String> {
        self.original_listened_addresses
            .read()
            .get(0)
            .map(|addr| self.to_external_url(addr))
    }

    fn to_external_url(&self, addr: &Multiaddr) -> String {
        format!(
            "{}/p2p/{}",
            addr,
            self.local_private_key.to_peer_id().to_base58()
        )
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
                )).into())
            }
        } else {
            Err(ErrorKind::PeerNotFound.into())
        }
    }
    pub(crate) fn ckb_protocol_connec(
        &self,
        peer_id: &PeerId,
        protocol_id: ProtocolId,
        endpoint: Endpoint,
        addresses: Option<Vec<Multiaddr>>,
    ) -> Result<UniqueConnec<(UnboundedSender<Bytes>, u8)>, Error> {
        let mut peers_registry = self.peers_registry().write();
        // get peer protocol_connection
        match peers_registry.new_peer(peer_id.clone(), endpoint) {
            Ok(_) => {
                let mut peer = peers_registry.get_mut(&peer_id).unwrap();
                if let Some(addresses) = addresses {
                    peer.append_addresses(addresses.clone());
                }
                if let Some(protocol_connec) = peer
                    .ckb_protocols
                    .iter()
                    .find(|&(id, _)| id == &protocol_id)
                    .map(|(_, ref protocol_connec)| protocol_connec.clone())
                {
                    Ok(protocol_connec)
                } else {
                    let protocol_connec = UniqueConnec::empty();
                    peer.ckb_protocols
                        .push((protocol_id, protocol_connec.clone()));
                    Ok(protocol_connec)
                }
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
                        remote_addresses: peer.remote_addresses.clone(),
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
        let transport = transport
            .clone()
            .and_then(move |out, endpoint, client_addr| {
                let peer_id = out.peer_id;
                upgrade::apply(out.socket, protocol, endpoint, client_addr).map(
                    move |(output, client_addr)| {
                        (
                            (
                                peer_id.clone(),
                                Protocol::CKBProtocol(output, peer_id, None),
                            ),
                            client_addr,
                        )
                    },
                )
            });

        let transport = TransportTimeout::new(transport, timeout);
        let unique_connec = match self.ckb_protocol_connec(
            expected_peer_id,
            protocol_id,
            Endpoint::Dialer,
            Some(vec![addr.to_owned()]),
        ) {
            Ok(unique_connec) => unique_connec,
            Err(_) => return,
        };

        let transport = transport.and_then({
            let expected_peer_id = expected_peer_id.clone();
            move |(peer_id, protocol), _, client_addr| {
                if peer_id == expected_peer_id {
                    trace!("dial success to {:?}", peer_id);
                    future::ok((protocol, client_addr))
                } else {
                    trace!("dial peer id mismatch {:?}", peer_id);
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

    pub(crate) fn build(
        config: &NetworkConfig,
        ckb_protocols: Vec<CKBProtocol<Arc<CKBProtocolHandler>>>,
    ) -> Result<Arc<Self>, Error> {
        let local_private_key = match config.fetch_private_key() {
            Some(private_key) => private_key?,
            None => return Err(ErrorKind::Other("secret_key not set".to_owned()).into()),
        };
        let listened_addresses = config.public_addresses.clone();
        let peer_store: Arc<RwLock<Box<PeerStore>>> = Arc::new(RwLock::new(Box::new(
            MemoryPeerStore::new(config.bootnodes()?),
        ) as Box<_>));
        let reserved_peers = config.reserved_peers()?;
        {
            let mut peer_store = peer_store.write();
            // put reserved_peers into peer_store
            for (peer_id, addr) in reserved_peers.clone() {
                peer_store.add_reserved_node(peer_id, vec![addr]);
            }
        }
        let peers_registry = PeersRegistry::new(
            Arc::clone(&peer_store),
            config.max_incoming_peers,
            config.max_outgoing_peers,
            config.reserved_only,
        );
        let network: Arc<Network> = Arc::new(Network {
            peers_registry: RwLock::new(peers_registry),
            peer_store: Arc::clone(&peer_store),
            listened_addresses: RwLock::new(listened_addresses),
            original_listened_addresses: RwLock::new(Vec::new()),
            ckb_protocols: CKBProtocols(ckb_protocols),
            local_private_key: local_private_key.clone(),
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
        let max_outgoing = config.max_outgoing_peers as usize;
        let basic_transport = {
            let basic_transport = new_transport(local_private_key, basic_transport_timeout)
                .map_err_dial({
                    let network = Arc::clone(&network);
                    move |err, addr| {
                        let mut peer_store = network.peer_store().write();
                        peer_store.report_address(&addr, Behaviour::FailedToConnect);
                        trace!(target: "network", "Failed to connect to peer {}, error: {:?}", addr, err);
                        err
                    }
                });
            Transport::and_then(basic_transport, {
                // Register new peers information
                let local_peer_id = local_peer_id.clone();
                move |(peer_id, stream), _endpoint, remote_addr_fut| {
                    remote_addr_fut.and_then(move |remote_addr| {
                        trace!(target: "network", "connection from {:?}", remote_addr);
                        if peer_id == local_peer_id {
                            trace!(target: "network", "connect to self, disconnect");
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
        let kad_upgrade = kad::KadConnecConfig::new();
        let kad_manage = Arc::new(Mutex::new(KadManage::new(
            Arc::clone(&network),
            kad_upgrade.clone(),
        )));
        let kad_system = {
            let peer_store = network.peer_store().read();
            let known_initial_peers: Box<Iterator<Item = PeerId>> = Box::new(
                peer_store
                    .bootnodes()
                    .map(|(peer_id, _)| peer_id.to_owned())
                    .take(100)
                    .collect::<Vec<_>>()
                    .into_iter(),
            ) as Box<_>;
            Arc::new(kad::KadSystem::without_init(kad::KadSystemConfig {
                parallelism: 1,
                local_peer_id: local_peer_id.clone(),
                kbuckets_timeout: Duration::from_secs(KBUCKETS_TIMEOUT),
                request_timeout: config.discovery_timeout,
                known_initial_peers,
            }))
        };

        let ping_service = Arc::new(PingService::new(config.ping_interval, config.ping_timeout));
        let discovery_service = Arc::new(DiscoveryService::new(
            config.discovery_timeout,
            config.discovery_response_count,
            config.discovery_interval,
            Arc::clone(&kad_manage),
            Arc::clone(&kad_system),
        ));
        let identify_service = Arc::new(IdentifyService {
            client_version,
            protocol_version,
            identify_timeout: config.identify_timeout,
            identify_interval: config.identify_interval,
        });

        let ckb_protocol_service = Arc::new(CKBService {
            kad_system: Arc::clone(&kad_system),
        });
        let timer_service = Arc::new(TimerService {
            timer_registry: Arc::clone(&timer_registry),
        });
        let outgoing_service = Arc::new(OutgoingService {
            outgoing_interval: config.outgoing_interval,
            timeout: config.outgoing_timeout,
        });
        // Transport used to handling received connections
        let handling_transport = {
            let transport = basic_transport.clone();
            transport.and_then({
                let network = Arc::clone(&network);
                let kad_upgrade = kad_upgrade.clone();
                move |out, endpoint, fut| {
                    let peer_id = Arc::new(out.peer_id);
                    let original_addr = out.original_addr;
                    // upgrades and apply protocols
                    let ping_upgrade = upgrade::map_with_addr(libp2p::ping::Ping, {
                        let peer_id = Arc::clone(&peer_id);
                        move |out, addr| PingService::convert_to_protocol(peer_id, addr, out)
                    });
                    let discovery_upgrade = upgrade::map_with_addr(kad_upgrade, {
                        let peer_id = Arc::clone(&peer_id);
                        move |out, addr| DiscoveryService::convert_to_protocol(peer_id, addr, out)
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
                        upgrade::or(
                            identify_upgrade,
                            upgrade::or(ping_upgrade, discovery_upgrade),
                        ),
                    );
                    upgrade::apply(out.socket, all_upgrade, endpoint, fut)
                }
            })
        };
        let (swarm_controller, swarm_events) = libp2p::core::swarm(handling_transport, {
            let ping_service = Arc::clone(&ping_service);
            let discovery_service = Arc::clone(&discovery_service);
            let identify_service = Arc::clone(&identify_service);
            let ckb_protocol_service = Arc::clone(&ckb_protocol_service);
            let network = Arc::clone(&network);
            move |protocol, _addr| match protocol {
                Protocol::Ping(..) | Protocol::Pong(..) => ping_service
                    .handle(Arc::clone(&network), protocol)
                    as Box<Future<Item = (), Error = IoError> + Send>,
                Protocol::Kad(..) => discovery_service.handle(Arc::clone(&network), protocol),
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
            let peer_store = network.peer_store().read();
            // dial reserved_nodes
            for (peer_id, addr) in peer_store.reserved_nodes() {
                network.dial_to_peer(
                    basic_transport.clone(),
                    addr,
                    peer_id,
                    &swarm_controller,
                    dial_timeout,
                );
            }
            // dial bootnodes
            for (peer_id, addr) in peer_store.bootnodes().take(max_outgoing) {
                debug!(target: "network", "dial bootnode {:?} {:?}", peer_id, addr);
                network.dial_to_peer(
                    basic_transport.clone(),
                    addr,
                    peer_id,
                    &swarm_controller,
                    dial_timeout,
                );
            }
        }

        let discovery_query_service = DiscoveryQueryService::new(
            Arc::clone(&network),
            swarm_controller.clone(),
            basic_transport.clone(),
            config.discovery_interval,
            Arc::clone(&kad_system),
            Arc::clone(&kad_manage),
            kad_upgrade.clone(),
        );

        // prepare services futures
        let futures: Vec<Box<Future<Item = (), Error = IoError> + Send>> = vec![
            Box::new(swarm_events.for_each(|_| Ok(()))),
            Box::new(
                discovery_query_service
                    .into_future()
                    .map(|_| ())
                    .map_err(|(err, _)| err),
            ) as Box<Future<Item = (), Error = IoError> + Send>,
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
            outgoing_service.start_protocol(
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
                    let mut peers_registry = network.peers_registry().write();
                    debug!(target: "network", "drop all connections...");
                    peers_registry.drop_all();
                    Ok(())
                }
            }).map_err(|(err, _, _)| {
                debug!(target: "network", "network exit, error {:?}", err);
                err
            });
        let service_futures =
            Box::new(service_futures) as Box<Future<Item = (), Error = IoError> + Send>;
        Ok(service_futures)
    }

    #[cfg_attr(feature = "cargo-clippy", allow(type_complexity))]
    pub fn new(
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
        let network = Self::build(config, ckb_protocols)?;
        let (close_tx, close_rx) = oneshot::channel();
        let network_future = Self::build_network_future(Arc::clone(&network), &config, close_rx)?;
        Ok((network, close_tx, network_future))
    }
}
