#![cfg_attr(feature = "cargo-clippy", allow(needless_pass_by_value))]

use super::Network;
use ckb_util::Mutex;
use fnv::FnvHashMap;
use futures::future::{self, Future};
use futures::Stream;
use libp2p::core::{upgrade, MuxedTransport, PeerId};
use libp2p::core::{Endpoint, Multiaddr, UniqueConnec};
use libp2p::core::{PublicKey, SwarmController};
use libp2p::{kad, Transport};
use peer_store::Status;
use protocol::Protocol;
use protocol_service::ProtocolService;
use rand;
use std::boxed::Box;
use std::error::Error;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::iter::Iterator;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use std::usize;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::timer::Interval;
use tokio::timer::Timeout;
use transport::TransportOutput;

pub struct DiscoveryService {
    timeout: Duration,
    discovery_interval: Duration,
    pub(crate) kad_system: Arc<kad::KadSystem>,
    default_response_neighbour_count: usize,
    pub(crate) kad_upgrade: kad::KadConnecConfig,
    kad_manage: Arc<Mutex<KadManage>>,
}

impl<T: Send> ProtocolService<T> for DiscoveryService {
    type Output = (
        kad::KadConnecController,
        Box<Stream<Item = kad::KadIncomingRequest, Error = IoError> + Send>,
    );

    fn convert_to_protocol(
        peer_id: Arc<PeerId>,
        addr: &Multiaddr,
        (kad_connection_controller, kad_stream): Self::Output,
    ) -> Protocol<T> {
        Protocol::Kad(
            kad_connection_controller,
            kad_stream,
            PeerId::clone(&peer_id),
            addr.clone(),
        )
    }

    fn handle(
        &self,
        network: Arc<Network>,
        protocol: Protocol<T>,
    ) -> Box<Future<Item = (), Error = IoError> + Send> {
        if let Protocol::Kad(kad_controller, stream, peer_id, addr) = protocol {
            match self.handle_kademlia_connection(
                network,
                peer_id,
                addr,
                kad_controller,
                Endpoint::Listener,
                stream,
            ) {
                Ok(future) => Box::new(future) as Box<Future<Item = _, Error = _> + Send>,
                Err(err) => Box::new(future::err(err)) as Box<Future<Item = _, Error = _> + Send>,
            }
        } else {
            Box::new(future::ok(())) as Box<Future<Item = _, Error = _> + Send>
        }
    }

    // start discovery peers
    fn start_protocol<SwarmTran, Tran, TranOut>(
        &self,
        network: Arc<Network>,
        swarm_controller: SwarmController<
            SwarmTran,
            Box<Future<Item = (), Error = IoError> + Send>,
        >,
        transport: Tran,
    ) -> Box<Future<Item = (), Error = IoError> + Send>
    where
        SwarmTran: MuxedTransport<Output = Protocol<T>> + Clone + Send + 'static,
        SwarmTran::MultiaddrFuture: Send + 'static,
        SwarmTran::Dial: Send,
        SwarmTran::Listener: Send,
        SwarmTran::ListenerUpgrade: Send,
        SwarmTran::Incoming: Send,
        SwarmTran::IncomingUpgrade: Send,
        Tran: MuxedTransport<Output = TransportOutput<TranOut>> + Clone + Send + 'static,
        Tran::MultiaddrFuture: Send + 'static,
        Tran::Dial: Send,
        Tran::Listener: Send,
        Tran::ListenerUpgrade: Send,
        Tran::Incoming: Send,
        Tran::IncomingUpgrade: Send,
        TranOut: AsyncRead + AsyncWrite + Send + 'static,
    {
        let kad_initialize_future = self.kad_system.perform_initialization({
            let network = Arc::clone(&network);
            let transport = transport.clone();
            let swarm_controller = swarm_controller.clone();
            let kad_manage = Arc::clone(&self.kad_manage);
            let kad_upgrade = self.kad_upgrade.clone();
            // dial kad peer
            move |peer_id| {
                debug!(target: "network", "Initialize kad search peers from peer {:?}", peer_id);
                Self::dial_kad_peer(
                    Arc::clone(&kad_manage),
                    kad_upgrade.clone(),
                    Arc::clone(&network),
                    peer_id.clone(),
                    transport.clone(),
                    swarm_controller.clone(),
                )
            }
        });

        let search_peers_future = Interval::new(
            Instant::now() + Duration::from_secs(5),
            self.discovery_interval,
        ).map_err(|err| IoError::new(IoErrorKind::Other, err))
        .for_each({
            let network = Arc::clone(&network);
            let transport = transport.clone();
            let swarm_controller = swarm_controller.clone();
            let kad_system = Arc::clone(&self.kad_system);
            let kad_manage = Arc::clone(&self.kad_manage);
            let kad_upgrade = self.kad_upgrade.clone();
            move |_| {
                // Use random key to search,
                // NOTICE, this can't prevent "Eclipse Attack" because attacker can still compute
                // our neighbours before they respond our query, but use the random key can make
                // attacker harder to apply attacking.
                let random_key =
                    PublicKey::Ed25519((0..32).map(|_| rand::random::<u8>()).collect());
                let random_peer_id = random_key.into_peer_id();
                let search_future = kad_system
                    .find_node(random_peer_id, {
                        let network = Arc::clone(&network);
                        let transport = transport.clone();
                        let swarm_controller = swarm_controller.clone();
                        let kad_manage = Arc::clone(&kad_manage);
                        let kad_upgrade = kad_upgrade.clone();
                        move |peer_id| {
                            debug!("kad search peers from peer {:?}", peer_id);
                            Self::dial_kad_peer(
                                Arc::clone(&kad_manage),
                                kad_upgrade.clone(),
                                Arc::clone(&network),
                                peer_id.clone(),
                                transport.clone(),
                                swarm_controller.clone(),
                            )
                        }
                    }).filter_map({
                        let network = Arc::clone(&network);
                        move |event| {
                            match event {
                                kad::KadQueryEvent::PeersReported(kad_peers) => {
                                    let mut peer_store = network.peer_store().write();
                                    for peer in kad_peers {
                                        // store peer info in peerstore
                                        let _ = peer_store.add_discovered_addresses(
                                            &peer.node_id,
                                            peer.multiaddrs,
                                        );
                                    }
                                    None
                                }
                                kad::KadQueryEvent::Finished(_) => Some(()),
                            }
                        }
                    }).into_future()
                    .map_err(|(err, _)| err)
                    .map(|_| ());
                Box::new(search_future) as Box<Future<Item = _, Error = _> + Send>
            }
        });
        let discovery_service_future = kad_initialize_future
            .select(search_peers_future)
            .map_err(|(err, _)| err)
            .and_then(|(_, stream)| stream);
        Box::new(discovery_service_future) as Box<Future<Item = _, Error = _> + Send>
    }
}

impl DiscoveryService {
    pub fn new<T>(
        timeout: Duration,
        default_response_neighbour_count: usize,
        discovery_interval: Duration,
        kad_config: kad::KadSystemConfig<T>,
    ) -> Self
    where
        T: Iterator<Item = PeerId>,
    {
        let kad_system = Arc::new(kad::KadSystem::without_init(kad_config));
        let kad_manage = Arc::new(Mutex::new(KadManage::new()));
        DiscoveryService {
            timeout,
            kad_system,
            kad_upgrade: kad::KadConnecConfig::new(),
            default_response_neighbour_count,
            discovery_interval,
            kad_manage,
        }
    }

    fn handle_kademlia_connection(
        &self,
        network: Arc<Network>,
        peer_id: PeerId,
        client_addr: Multiaddr,
        kad_connection_controller: kad::KadConnecController,
        _endpoint: Endpoint,
        kademlia_stream: Box<Stream<Item = kad::KadIncomingRequest, Error = IoError> + Send>,
    ) -> Result<Box<Future<Item = (), Error = IoError> + Send>, IoError> {
        debug!(target: "network", "client_addr is {:?}", client_addr);
        //let peer_id = match convert_addr_into_peer_id(client_addr) {
        //    Some(peer_id) => peer_id,
        //    None => {
        //        return Err(IoError::new(
        //            IoErrorKind::Other,
        //            "failed to extract peer_id from client addr",
        //        ))
        //    }
        //};

        let handling_future = Box::new(
            future::loop_fn(kademlia_stream, {
                let peer_id = peer_id.clone();
                let kad_system = Arc::clone(&self.kad_system);
                let timeout = self.timeout;
                let respond_peers_count = self.default_response_neighbour_count;
                move |kademlia_stream| {
                    let network = Arc::clone(&network);
                    let peer_id = peer_id.clone();
                    let next_future = kademlia_stream.into_future().map_err(|(err, _)| err);
                    Timeout::new(next_future, timeout)
                        .map_err({
                            move |err| {
                                info!(target: "network", "kad timeout error {:?}", err.description());
                                IoError::new(
                                    IoErrorKind::Other,
                                    format!("discovery request timeout {:?}", err.description()),
                                )
                            }
                        }).and_then({
                            let kad_system = Arc::clone(&kad_system);
                            move |(req, next_stream)| {
                                kad_system.update_kbuckets(peer_id);
                                match req {
                                    Some(kad::KadIncomingRequest::FindNode {
                                        searched,
                                        responder,
                                    }) => responder.respond(Self::build_kademlia_response(
                                        kad_system,
                                        Arc::clone(&network),
                                        &searched,
                                        respond_peers_count,
                                    )),
                                    Some(kad::KadIncomingRequest::PingPong) => (),
                                    None => return Ok(future::Loop::Break(())),
                                }
                                Ok(future::Loop::Continue(next_stream))
                            }
                        })
                }
            }).then({
                let peer_id = peer_id.clone();
                move |val| {
                    trace!(
                        target: "network",
                        "Kad connection closed when handling peer {:?} reason: {:?}",
                        peer_id,
                        val
                    );
                    val
                }
            }),
        ) as Box<Future<Item = _, Error = _> + Send>;
        let kad_connection = self.kad_manage.lock().obtain_connection(peer_id.clone());
        Ok(Box::new(
            kad_connection
                .tie_or_passthrough(kad_connection_controller, handling_future)
                .then({
                    let kad_manage = Arc::clone(&self.kad_manage);
                    // drop kad connection
                    move |val| {
                        kad_manage.lock().drop_connection(&peer_id);
                        val
                    }
                }),
        ))
    }

    fn build_kademlia_response(
        kad_system: Arc<kad::KadSystem>,
        network: Arc<Network>,
        searched_peer_id: &PeerId,
        respond_peers_count: usize,
    ) -> Vec<kad::KadPeer> {
        kad_system
            .known_closest_peers(searched_peer_id)
            .map({
                let kad_system = Arc::clone(&kad_system);
                let network = Arc::clone(&network);
                move |peer_id| {
                    if peer_id == *kad_system.local_peer_id() {
                        debug!(
                            target: "network",
                            "response self address to kad {:?}",
                            network.listened_addresses.read().clone()
                        );
                        kad::KadPeer {
                            node_id: peer_id.clone(),
                            multiaddrs: network.listened_addresses.read().clone(),
                            connection_ty: kad::KadConnectionType::Connected,
                        }
                    } else {
                        let peer_store = network.peer_store().read();
                        let multiaddrs = match peer_store
                            .peer_addrs(&peer_id)
                            .map(|i| i.take(10).map(|addr| addr.to_owned()).collect::<Vec<_>>())
                        {
                            Some(addrs) => addrs,
                            None => Vec::new(),
                        };
                        let connection_ty = match peer_store.peer_status(&peer_id) {
                            Status::Connected => kad::KadConnectionType::Connected,
                            _ => kad::KadConnectionType::NotConnected,
                        };
                        debug!(
                            target: "network",
                            "response other address to kad {:?} {:?}",
                            peer_id,
                            multiaddrs.clone()
                        );
                        kad::KadPeer {
                            node_id: peer_id.clone(),
                            multiaddrs,
                            connection_ty,
                        }
                    }
                }
            }).filter(|kad_peer| {
                kad_peer.node_id == *kad_system.local_peer_id() || !kad_peer.multiaddrs.is_empty()
            }).take(respond_peers_count)
            .collect::<Vec<_>>()
    }

    #[cfg_attr(feature = "cargo-clippy", allow(let_and_return))]
    fn dial_kad_peer<Tran, To, St, T: Send>(
        kad_manage: Arc<Mutex<KadManage>>,
        kad_upgrade: kad::KadConnecConfig,
        network: Arc<Network>,
        peer_id: PeerId,
        transport: Tran,
        swarm_controller: SwarmController<St, Box<Future<Item = (), Error = IoError> + Send>>,
    ) -> Box<Future<Item = kad::KadConnecController, Error = IoError> + Send>
    where
        Tran: MuxedTransport<Output = TransportOutput<To>> + Clone + Send + 'static,
        Tran::MultiaddrFuture: Send + 'static,
        Tran::Dial: Send,
        Tran::Listener: Send,
        Tran::ListenerUpgrade: Send,
        Tran::Incoming: Send,
        Tran::IncomingUpgrade: Send,
        To: AsyncRead + AsyncWrite + Send + 'static,
        St: MuxedTransport<Output = Protocol<T>> + Clone + Send + 'static,
        St::Dial: Send,
        St::MultiaddrFuture: Send,
        St::Listener: Send,
        St::ListenerUpgrade: Send,
        St::Incoming: Send,
        St::IncomingUpgrade: Send,
    {
        let mut kad_manage = kad_manage.lock();
        let transport = transport
                    .and_then(move |out, endpoint, client_addr|
                              upgrade::apply(out.socket, kad_upgrade.clone(), endpoint, client_addr)
                             ).and_then({
                        let peer_id = peer_id.clone();
                        move |output, _, client_addr|
                            // wrap kad output into our own protocol
                            client_addr.map(|client_addr| {
                                let out = Self::convert_to_protocol(Arc::new(peer_id), &client_addr.clone(), output);
                                (out, future::ok(client_addr))
                            })
                    });

        // TODO optimiz here
        let addr: Multiaddr = {
            let peer_store = network.peer_store().read();
            let addr = match peer_store
                .peer_addrs(&peer_id)
                .map(move |mut addrs| addrs.next())
            {
                Some(Some(addr)) => addr.to_owned(),
                _ => {
                    debug!(
                        target: "network",
                        "dial kad error, can't find dial address for peer_id {:?}",
                        peer_id
                    );
                    return Box::new(future::err(IoError::new(
                        IoErrorKind::Other,
                        format!("can't find dial address for peer_id {:?}", peer_id),
                    )));
                }
            };
            addr
        };
        let kad_connection = kad_manage.obtain_connection(peer_id.clone());
        let dial_future = kad_connection.dial(&swarm_controller, &addr, transport);
        Box::new(dial_future) as Box<Future<Item = _, Error = _> + Send>
    }
}

struct KadManage(FnvHashMap<PeerId, UniqueConnec<kad::KadConnecController>>);

impl KadManage {
    fn new() -> Self {
        KadManage(FnvHashMap::with_capacity_and_hasher(10, Default::default()))
    }

    fn obtain_connection(&mut self, peer_id: PeerId) -> UniqueConnec<kad::KadConnecController> {
        self.0
            .entry(peer_id)
            .or_insert_with(UniqueConnec::empty)
            .to_owned()
    }

    fn drop_connection(&mut self, peer_id: &PeerId) {
        self.0.remove(peer_id);
    }
}
