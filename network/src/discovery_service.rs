#![cfg_attr(feature = "cargo-clippy", allow(needless_pass_by_value))]

use super::Network;
use ckb_util::Mutex;
use fnv::FnvHashMap;
use futures::future::{self, Future};
use futures::sync::{mpsc, oneshot};
use futures::Stream;
use libp2p::core::{upgrade, MuxedTransport, PeerId};
use libp2p::core::{Endpoint, Multiaddr, UniqueConnec};
use libp2p::core::{PublicKey, SwarmController};
use libp2p::{kad, Transport};
use peer_store::Status;
use protocol::Protocol;
use protocol_service::ProtocolService;
use rand::{self, Rng};
use std::boxed::Box;
use std::error::Error;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::iter::Iterator;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use std::usize;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::prelude::{task, Async, Poll};
use tokio::spawn;
use tokio::timer::Interval;
use tokio::timer::Timeout;
use transport::TransportOutput;

pub(crate) struct DiscoveryService {
    timeout: Duration,
    pub(crate) kad_system: Arc<kad::KadSystem>,
    default_response_neighbour_count: usize,
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
}

impl DiscoveryService {
    pub fn new(
        timeout: Duration,
        default_response_neighbour_count: usize,
        kad_manage: Arc<Mutex<KadManage>>,
        kad_system: Arc<kad::KadSystem>,
    ) -> Self {
        DiscoveryService {
            timeout,
            kad_system,
            default_response_neighbour_count,
            kad_manage,
        }
    }

    fn handle_kademlia_connection(
        &self,
        network: Arc<Network>,
        peer_id: PeerId,
        _client_addr: Multiaddr,
        kad_connection_controller: kad::KadConnecController,
        _endpoint: Endpoint,
        kademlia_stream: Box<Stream<Item = kad::KadIncomingRequest, Error = IoError> + Send>,
    ) -> Result<Box<Future<Item = (), Error = IoError> + Send>, IoError> {
        let handling_future = Box::new(
            future::loop_fn(kademlia_stream, {
                let peer_id = peer_id.clone();
                let kad_system = Arc::clone(&self.kad_system);
                let timeout = self.timeout;
                let respond_peers_count = self.default_response_neighbour_count;
                let kad_manage = Arc::clone(&self.kad_manage);
                move |kademlia_stream| {
                    let network = Arc::clone(&network);
                    let peer_id = peer_id.clone();
                    let next_future = kademlia_stream.into_future().map_err(|(err, _)| {
                        debug!(target: "discovery","kad stream error: {}", err);
                        err
                    });
                    let kad_manage = Arc::clone(&kad_manage);
                    Timeout::new(next_future, timeout)
                        .map_err({
                            move |err| {
                                info!(target: "discovery", "kad timeout error {:?}", err.description());
                                IoError::new(
                                    IoErrorKind::Other,
                                    format!("discovery request timeout {:?}", err.description()),
                                )
                            }
                        }).and_then({
                            let kad_system = Arc::clone(&kad_system);
                            let kad_manage = Arc::clone(&kad_manage);
                            move |(req, next_stream)| {
                                kad_system.update_kbuckets(peer_id.clone());
                                match req {
                                    Some(kad::KadIncomingRequest::FindNode {
                                        searched,
                                        responder,
                                    }) => {
                                        let kad_peers = Self::build_kademlia_response(
                                            kad_system,
                                            Arc::clone(&network),
                                            &searched,
                                            respond_peers_count,
                                            );
                                        debug!(target:"network", "kad respond nodes count: {}", kad_peers.len());
                                        responder.respond(kad_peers);
                                    },
                                    Some(kad::KadIncomingRequest::PingPong) => (),
                                    None => {
                                        debug!(target: "discovery","finish kad stream");
                                        return Ok(future::Loop::Break(()))
                                    }
                                }
                                let mut kad_manage = kad_manage.lock();
                                if let Some(to_notify) = kad_manage.to_notify.take() {
                                    to_notify.notify();
                                }
                                Ok(future::Loop::Continue(next_stream))
                            }
                        })
                }
            }).then({
                let peer_id = peer_id.clone();
                move |val| {
                    debug!(
                        target: "discovery",
                        "Kad connection closed when handling peer {:?} reason: {:?}",
                        peer_id,
                        val
                    );
                    val
                }
            }),
        ) as Box<Future<Item = _, Error = _> + Send>;

        let kad_unique_connec = {
            let mut kad_manage = self.kad_manage.lock();
            if let Some(to_notify) = kad_manage.to_notify.take() {
                to_notify.notify();
            }
            kad_manage.complete_kad_connection(peer_id.clone(), kad_connection_controller.clone());
            kad_manage.fetch_unique_connec(peer_id.clone())
        };
        Ok(Box::new(
            kad_unique_connec
                .tie_or_passthrough(kad_connection_controller, handling_future)
                .then({
                    let kad_manage = Arc::clone(&self.kad_manage);
                    // drop kad connection
                    move |val| {
                        info!("kad exit because {:?}", val);
                        let mut kad_manage = kad_manage.lock();
                        if let Some(to_notify) = kad_manage.to_notify.take() {
                            to_notify.notify();
                        }
                        kad_manage.drop_connection(&peer_id);
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
        let mut kad_peers = kad_system
            .known_closest_peers(searched_peer_id)
            .map({
                let kad_system = Arc::clone(&kad_system);
                let network = Arc::clone(&network);
                move |peer_id| {
                    if peer_id == *kad_system.local_peer_id() {
                        debug!(
                            target: "discovery",
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
                            target: "discovery",
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
            .collect::<Vec<_>>();
        // Here we must return at least 1 KadPeer, otherwise kad stream will close
        if kad_peers.is_empty() {
            kad_peers.push(kad::KadPeer {
                node_id: kad_system.local_peer_id().to_owned(),
                multiaddrs: network.listened_addresses.read().clone(),
                connection_ty: kad::KadConnectionType::Connected,
            });
        }
        kad_peers
    }
}

pub(crate) struct DiscoveryQueryService<SwarmTran, Tran, TranOut, T>
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
    network: Arc<Network>,
    swarm_controller: SwarmController<SwarmTran, Box<Future<Item = (), Error = IoError> + Send>>,
    transport: Tran,
    query_interval: Interval,
    kad_controller_request_sender: mpsc::UnboundedSender<PeerId>,
    kad_controller_request_receiver: mpsc::UnboundedReceiver<PeerId>,
    kad_query_events:
        Vec<Box<Stream<Item = kad::KadQueryEvent<Vec<PeerId>>, Error = IoError> + Send>>,
    kad_system: Arc<kad::KadSystem>,
    kad_manage: Arc<Mutex<KadManage>>,
}

impl<SwarmTran, Tran, TranOut, T> DiscoveryQueryService<SwarmTran, Tran, TranOut, T>
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
    T: Send,
{
    pub fn new(
        network: Arc<Network>,
        swarm_controller: SwarmController<
            SwarmTran,
            Box<Future<Item = (), Error = IoError> + Send>,
        >,
        transport: Tran,
        discovery_interval: Duration,
        kad_system: Arc<kad::KadSystem>,
        kad_manage: Arc<Mutex<KadManage>>,
    ) -> Self {
        let (kad_controller_request_sender, kad_controller_request_receiver) = mpsc::unbounded();
        DiscoveryQueryService {
            network,
            swarm_controller,
            transport,
            query_interval: Interval::new(
                Instant::now() + Duration::from_secs(5),
                discovery_interval,
            ),
            kad_query_events: Vec::with_capacity(10),
            kad_system,
            kad_manage,
            kad_controller_request_sender,
            kad_controller_request_receiver,
        }
    }

    // start discovery peers
    fn perform_random_query(&mut self) {
        // Use random key to search,
        // NOTICE, this can't prevent "Eclipse Attack" because attacker can still compute
        // our neighbours before they respond our query, but use the random key can make
        // attacker harder to apply attacking.
        let mut key = vec![0u8; 32];
        rand::thread_rng().fill(&mut key[..]);
        let random_key = PublicKey::Ed25519(key);
        let random_peer_id = random_key.into_peer_id();
        let query = self.kad_system.find_node(random_peer_id, {
            let kad_manage = Arc::clone(&self.kad_manage);
            let kad_controller_request_sender = self.kad_controller_request_sender.clone();
            move |peer_id| {
                let (tx, rx) = oneshot::channel();
                let mut kad_manage = kad_manage.lock();
                kad_manage
                    .kad_pending_dials
                    .entry(peer_id.clone())
                    .or_insert_with(Vec::new)
                    .push(tx);
                debug!(target: "discovery", "find node from {:?} pending: {}", peer_id, kad_manage.kad_pending_dials[&peer_id].len());
                kad_controller_request_sender.unbounded_send(peer_id.clone()).expect("send kad controller request");
                rx.map_err(|err| {
                    IoError::new(
                        IoErrorKind::Other,
                        format!("Fetch kad controller failed {}", err),
                    )
                })
            }
        });
        self.kad_query_events.push(Box::new(query));
    }

    fn handle_kad_controller_request(&self, peer_id: PeerId) {
        let mut kad_manage = self.kad_manage.lock();
        if &peer_id == self.network.local_peer_id() {
            debug!(
                target: "discovery",
                "ignore kad dial to self"
                );
            kad_manage.kad_pending_dials.remove(&peer_id);
            return;
        }
        let peer_store = self.network.peer_store().read();
        if let Some(addrs) = peer_store.peer_addrs(&peer_id) {
            for addr in addrs {
                // dial by kad_manage
                if kad_manage
                    .ensure_connection(
                        peer_id.clone(),
                        addr,
                        self.transport.clone(),
                        &self.swarm_controller,
                    ).is_ok()
                {
                    return;
                }
            }
        }
        debug!(
            target: "discovery",
            "can't open kad stream for {:?}, because no address usable",
            peer_id
        );
        kad_manage.kad_pending_dials.remove(&peer_id);
    }
}

impl<SwarmTran, Tran, TranOut, T> Stream for DiscoveryQueryService<SwarmTran, Tran, TranOut, T>
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
    T: Send,
{
    type Item = ();
    type Error = IoError;

    fn poll(&mut self) -> Poll<Option<()>, IoError> {
        // 1. handle kad queries response: response, finish...
        for n in (0..self.kad_query_events.len()).rev() {
            let mut query = self.kad_query_events.swap_remove(n);
            loop {
                match query.poll() {
                    Ok(Async::Ready(Some(kad::KadQueryEvent::PeersReported(kad_peers)))) => {
                        let mut peer_store = self.network.peer_store().write();
                        debug!(target:"network", "discovery new nodes count: {}", kad_peers.len());
                        for peer in kad_peers {
                            debug!(target:"network", "discovery new node {:?}", peer);
                            // store peer info in peerstore
                            let _ =
                                peer_store.add_discovered_addresses(&peer.node_id, peer.multiaddrs);
                        }
                    }
                    Ok(Async::Ready(Some(kad::KadQueryEvent::Finished(out)))) => {
                        debug!(target: "discovery", "Kad query finished and respond {} result", out.len());
                        break;
                    }
                    Ok(Async::Ready(None)) => {
                        error!(target: "discovery", "Kad query None result");
                        break;
                    }
                    Ok(Async::NotReady) => {
                        // put back
                        self.kad_query_events.push(query);
                        break;
                    }
                    Err(err) => {
                        error!(target: "discovery", "Kad query error: {}", err);
                        break;
                    }
                }
            }
        }

        // 2. handle dial requests
        loop {
            match self.kad_controller_request_receiver.poll() {
                Ok(Async::NotReady) => break,
                Ok(Async::Ready(Some(peer_id))) => self.handle_kad_controller_request(peer_id),
                Ok(Async::Ready(None)) => {
                    error!("kad_controller_request_receiver closed unexpected!");
                    return Ok(Async::Ready(None));
                }
                Err(err) => {
                    error!("kad_controller_request_receiver error unexpected!");
                    return Err(IoError::new(
                        IoErrorKind::Other,
                        format!(
                            "discovery service kad_controller_request_receiver error: {:?}",
                            err
                        ),
                    ));
                }
            }
        }
        // 3. handle periodic queries
        loop {
            match self.query_interval.poll() {
                Ok(Async::NotReady) => break,
                Ok(Async::Ready(Some(_))) => self.perform_random_query(),
                Ok(Async::Ready(None)) => {
                    error!("query task timer closed unexpected!");
                    return Ok(Async::Ready(None));
                }
                Err(err) => {
                    error!("query task timer error: {:?}", err);
                    return Err(IoError::new(
                        IoErrorKind::Other,
                        format!("discovery service timer error: {:?}", err),
                    ));
                }
            }
        }

        let mut kad_manage = self.kad_manage.lock();
        kad_manage.to_notify = Some(task::current());
        Ok(Async::NotReady)
    }
}

const MAX_DIALING_COUNT: usize = 30;
const MAX_CONNECTING_COUNT: usize = 30;
const KAD_DIAL_TIMEOUT_SECS: u64 = 15;

pub(crate) struct KadManage {
    kad_connections: FnvHashMap<PeerId, UniqueConnec<kad::KadConnecController>>,
    kad_pending_dials: FnvHashMap<PeerId, Vec<oneshot::Sender<kad::KadConnecController>>>,
    kad_dialing_peers: FnvHashMap<PeerId, Instant>,
    kad_upgrade: kad::KadConnecConfig,
    pub(crate) to_notify: Option<task::Task>,
}

impl KadManage {
    pub fn new(_network: Arc<Network>, kad_upgrade: kad::KadConnecConfig) -> Self {
        KadManage {
            kad_connections: FnvHashMap::with_capacity_and_hasher(10, Default::default()),
            kad_pending_dials: FnvHashMap::with_capacity_and_hasher(10, Default::default()),
            kad_dialing_peers: FnvHashMap::with_capacity_and_hasher(10, Default::default()),
            kad_upgrade,
            to_notify: None,
        }
    }

    // check and remove timeout dials
    fn check_unused_conn(&mut self) {
        let now = Instant::now();
        let timeout = Duration::from_secs(KAD_DIAL_TIMEOUT_SECS);
        self.kad_dialing_peers
            .retain(move |_peer_id, added_at| now.duration_since(*added_at) > timeout);
    }

    fn complete_kad_connection(
        &mut self,
        peer_id: PeerId,
        kad_connection_controller: kad::KadConnecController,
    ) {
        debug!(target: "discovery", "incoming new kad connection for {:?}", peer_id);
        if let Some(txs) = self.kad_pending_dials.remove(&peer_id) {
            debug!(target: "discovery", "incoming new kad connection send to waiting queries {:?}", txs.len());
            for tx in txs {
                let _ = tx.send(kad_connection_controller.clone());
            }
        }
        // remove dialing status
        self.kad_dialing_peers.remove(&peer_id);
    }

    fn connected_peers(&self) -> impl Iterator<Item = &PeerId> {
        self.kad_connections
            .iter()
            .filter_map(|(key, unique_connec)| {
                if unique_connec.is_alive() {
                    Some(key)
                } else {
                    None
                }
            })
    }

    fn fetch_unique_connec(&mut self, peer_id: PeerId) -> UniqueConnec<kad::KadConnecController> {
        self.kad_connections
            .entry(peer_id)
            .or_insert_with(UniqueConnec::empty)
            .to_owned()
    }

    #[cfg_attr(feature = "cargo-clippy", allow(let_and_return))]
    fn ensure_connection<Tran, To, St, T: Send>(
        &mut self,
        peer_id: PeerId,
        addr: &Multiaddr,
        transport: Tran,
        swarm_controller: &SwarmController<St, Box<Future<Item = (), Error = IoError> + Send>>,
    ) -> Result<(), IoError>
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
        debug!(target: "discovery", "dial kad connection to {:?} {:?}", peer_id, addr);
        let kad_connection = self.fetch_unique_connec(peer_id.clone());
        if kad_connection.is_alive() {
            return Ok(());
        }

        // remove unused connections
        self.check_unused_conn();
        // check peer
        if self.kad_dialing_peers.contains_key(&peer_id)
            || self.kad_dialing_peers.len() >= MAX_DIALING_COUNT
        {
            debug!(target: "discovery", "we are already dialing to {:?} {:?}", peer_id, addr);
            return Ok(());
        }
        let is_connected = self
            .kad_connections
            .get(&peer_id)
            .map(|unique_connec| unique_connec.is_alive())
            .unwrap_or(false);
        let count_of_connected_peers = self.connected_peers().collect::<Vec<_>>().len();
        if is_connected || count_of_connected_peers >= MAX_CONNECTING_COUNT {
            debug!(target: "discovery", "we are already connected to {:?} {:?}", peer_id, addr);
            // should return a error?
            return Ok(());
        }
        //set peer to dialing list
        self.kad_dialing_peers
            .entry(peer_id.clone())
            .or_insert_with(Instant::now);

        let kad_upgrade = self.kad_upgrade.clone();
        let transport = transport
            .and_then(move |out, endpoint, client_addr| {
                upgrade::apply(out.socket, kad_upgrade.clone(), endpoint, client_addr)
            }).and_then({
                let peer_id = peer_id.clone();
                move |(kad_connection_controller, kad_stream), _, client_addr| {
                    debug!(target: "discovery", "upgraded kad connection!!!!!! {:?}",  peer_id);
                    // wrap kad output into our own protocol
                    client_addr.map(move |client_addr| {
                        let out = Protocol::Kad(
                            kad_connection_controller,
                            kad_stream,
                            peer_id.clone(),
                            client_addr.clone(),
                        );
                        (out, future::ok(client_addr))
                    })
                }
            });

        let dial_future = kad_connection.dial(swarm_controller, addr, transport);
        spawn(dial_future.then(|err| {
            debug!(target: "discovery", "dialing result {:?}", err);
            future::ok(())
        }));
        Ok(())
    }

    fn drop_connection(&mut self, peer_id: &PeerId) {
        debug!(target: "discovery","drop kad connection from {:?}", peer_id);
        self.kad_connections.remove(peer_id);
    }
}
