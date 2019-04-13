use crate::errors::{Error, ProtocolError};
use crate::network_event::{EventHandler, NetworkEvent};
use crate::peer_store::{sqlite::SqlitePeerStore, PeerStore, Status};
use crate::peers_registry::{ConnectionStatus, PeersRegistry};
use crate::protocols::{
    discovery::{DiscoveryEvent, DiscoveryProtocol},
    identify::IdentifyCallback,
};
use crate::protocols::{feeler::Feeler, BackgroundService, DefaultCKBProtocolContext};
use crate::MultiaddrList;
use crate::Peer;
use crate::{
    Behaviour, CKBProtocol, CKBProtocolContext, NetworkConfig, NetworkState, PeerIndex, ProtocolId,
    ProtocolVersion, ServiceContext, ServiceControl, SessionId, SessionType,
};
use crate::{DISCOVERY_PROTOCOL_ID, FEELER_PROTOCOL_ID, IDENTIFY_PROTOCOL_ID, PING_PROTOCOL_ID};
use ckb_core::service::{Request, DEFAULT_CHANNEL_SIZE, SIGNAL_CHANNEL_SIZE};
use ckb_util::RwLock;
use crossbeam_channel::{self, select, Receiver, Sender, TryRecvError};
use fnv::{FnvHashMap, FnvHashSet};
use futures::sync::{
    mpsc::{self, channel},
    oneshot,
};
use futures::Future;
use futures::Stream;
use futures::{try_ready, Async, Poll};
use log::{debug, error, info, trace, warn};
use lru_cache::LruCache;
use p2p::{
    builder::{MetaBuilder, ServiceBuilder},
    error::Error as P2pError,
    multiaddr::{self, multihash::Multihash, Multiaddr},
    secio::PeerId,
    service::{DialProtocol, ProtocolEvent, ProtocolHandle, Service, ServiceError, ServiceEvent},
    traits::ServiceHandle,
    utils::extract_peer_id,
};
use p2p_identify::IdentifyProtocol;
use p2p_ping::{Event as PingEvent, PingHandler};
use secio;
use std::boxed::Box;
use std::cell::RefCell;
use std::cmp::max;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use std::usize;
use stop_handler::{SignalSender, StopHandler};
use tokio::runtime::Runtime;
use tokio::timer::Interval;

const FEELER_CONNECTION_COUNT: u32 = 5;

enum HandleResult {
    Continue,
    Stop(Option<Sender<()>>),
}

/// forward events from tokio channel to crossbeam channel
struct EventForward {
    network_event_sender: Sender<NetworkEvent>,
    network_event_source: futures::sync::mpsc::UnboundedReceiver<NetworkEvent>,
    outbound_sender: Sender<Instant>,
    outbound_interval: Duration,
    disc_sender: Sender<DiscoveryEvent>,
    disc_source: futures::sync::mpsc::UnboundedReceiver<DiscoveryEvent>,
    ping_sender: Sender<PingEvent>,
    ping_source: futures::sync::mpsc::Receiver<PingEvent>,
}

struct NetworkReceivers {
    /// network event
    network_event_receiver: Receiver<NetworkEvent>,
    /// ping event
    ping_receiver: Receiver<PingEvent>,
    /// disc event
    disc_receiver: Receiver<DiscoveryEvent>,
    /// outbound event
    outbound_receiver: Receiver<Instant>,
    /// stop signal
    stop_signal: Receiver<Sender<()>>,
    //== RPC calls ==
    external_urls_receiver: Receiver<Request<usize, Vec<(String, u8)>>>,
    listened_addresses_receiver: Receiver<Request<usize, Vec<(Multiaddr, u8)>>>,
    dial_node_receiver: Receiver<Request<(PeerId, Multiaddr), ()>>,
    connected_peers_receiver: Receiver<Request<(), Vec<(PeerId, Peer, MultiaddrList)>>>,
    add_discovered_addr_receiver: Receiver<Request<(PeerId, Multiaddr), ()>>,
    broadcast_receiver: Receiver<Request<(ProtocolId, Vec<u8>), ()>>,
}

pub struct NetworkService {
    p2p_control: ServiceControl,
    network_state: RefCell<NetworkState>,
    /// Event forward
    event_forward: Option<EventForward>,
    protocols: Vec<CKBProtocol>,
    receivers: NetworkReceivers,
}

impl NetworkService {
    pub fn build(
        mut network_state: NetworkState,
        protocols: Vec<CKBProtocol>,
    ) -> (NetworkService, Service<EventHandler>, NetworkController) {
        let config = network_state.config.clone();

        // == Build NetworkReceiver
        let (external_urls_sender, external_urls_receiver) =
            crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);
        let (dial_node_sender, dial_node_receiver) =
            crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);
        let (connected_peers_sender, connected_peers_receiver) =
            crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);
        let (listened_addresses_sender, listened_addresses_receiver) =
            crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);
        let (add_discovered_addr_sender, add_discovered_addr_receiver) =
            crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);
        let (broadcast_sender, broadcast_receiver) =
            crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);
        let (stop_sender, stop_signal) = crossbeam_channel::bounded(1);

        // == Build NetworkController
        let controller = NetworkController {
            peer_id: network_state.local_peer_id().to_owned(),
            external_urls_sender,
            listened_addresses_sender,
            dial_node_sender,
            connected_peers_sender,
            add_discovered_addr_sender,
            broadcast_sender,
            stop_sender,
        };

        // == Build special protocols

        // TODO: how to deny banned node to open those protocols?
        // Ping protocol
        let (ping_fut_sender, ping_fut_receiver) = mpsc::channel(std::u8::MAX as usize);
        let ping_meta = MetaBuilder::default()
            .id(PING_PROTOCOL_ID)
            .service_handle({
                let ping_fut_sender = ping_fut_sender.clone();
                let ping_interval_secs = config.ping_interval_secs;
                let ping_timeout_secs = config.ping_timeout_secs;
                move || {
                    ProtocolHandle::Both(Box::new(PingHandler::new(
                        Duration::from_secs(ping_interval_secs),
                        Duration::from_secs(ping_timeout_secs),
                        ping_fut_sender.clone(),
                    )))
                }
            })
            .build();

        // Discovery protocol
        let (disc_fut_sender, disc_fut_receiver) = mpsc::unbounded();
        let disc_meta = MetaBuilder::default()
            .id(DISCOVERY_PROTOCOL_ID)
            .service_handle({
                let disc_fut_sender = disc_fut_sender.clone();
                move || {
                    ProtocolHandle::Both(Box::new(DiscoveryProtocol::new(disc_fut_sender.clone())))
                }
            })
            .build();

        // Identify protocol
        let identify_meta = MetaBuilder::default()
            .id(IDENTIFY_PROTOCOL_ID)
            .service_handle({
                let controller = controller.clone();
                move || {
                    let identify_callback = IdentifyCallback::new(controller.clone());
                    ProtocolHandle::Both(Box::new(IdentifyProtocol::new(identify_callback)))
                }
            })
            .build();

        // Feeler protocol
        let feeler_protocol = CKBProtocol::new(
            "flr".to_string(),
            FEELER_PROTOCOL_ID,
            &[1][..],
            Box::new(Feeler {}),
        );

        // == Build p2p service struct
        let mut protocol_metas = protocols
            .iter()
            .map(|protocol| protocol.build())
            .collect::<Vec<_>>();
        protocol_metas.push(feeler_protocol.build());
        protocol_metas.push(ping_meta);
        protocol_metas.push(disc_meta);
        protocol_metas.push(identify_meta);

        let mut service_builder = ServiceBuilder::default();
        for meta in protocol_metas.into_iter() {
            network_state.protocol_ids.insert(meta.id());
            service_builder = service_builder.insert_protocol(meta);
        }

        let (event_fut_sender, event_fut_receiver) = mpsc::unbounded();

        let event_handler = EventHandler::new(event_fut_sender);
        let mut p2p_service = service_builder
            .key_pair(network_state.local_private_key().clone())
            .forever(true)
            .build(event_handler);

        // == build EventForward
        let (network_event_sender, network_event_receiver) =
            crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);
        let (ping_sender, ping_receiver) = crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);
        let (disc_sender, disc_receiver) = crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);
        let (outbound_sender, outbound_receiver) = crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);

        let event_forward = EventForward {
            network_event_source: event_fut_receiver,
            network_event_sender,
            ping_source: ping_fut_receiver,
            ping_sender,
            disc_source: disc_fut_receiver,
            disc_sender,
            outbound_interval: Duration::from_secs(config.connect_outbound_interval_secs),
            outbound_sender,
        };

        let receivers = NetworkReceivers {
            external_urls_receiver,
            listened_addresses_receiver,
            dial_node_receiver,
            connected_peers_receiver,
            add_discovered_addr_receiver,
            broadcast_receiver,
            network_event_receiver,
            ping_receiver,
            disc_receiver,
            outbound_receiver,
            stop_signal,
        };

        let network_service = NetworkService {
            p2p_control: p2p_service.control().clone(),
            network_state: RefCell::new(network_state),
            protocols,
            receivers,
            event_forward: Some(event_forward),
        };
        (network_service, p2p_service, controller)
    }
    fn handle_receivers(&self, network_state: &mut NetworkState) -> HandleResult {
        select! {
            // handle network events
            recv(self.receivers.network_event_receiver) -> msg => match msg {
                Ok((NetworkEvent::Error(error))) => {
                    self.handle_service_error(network_state, error);
                }
                Ok(NetworkEvent::Event(event)) => {
                    self.handle_service_event(network_state, event);
                }

                Ok(NetworkEvent::Protocol(event)) => {
                    self.handle_protocol(network_state, event);
                }
                Err(err) => {
                    error!(target: "network", "event_receiver error: {:?}", err);
                }
            },
            // handle network events
            recv(self.receivers.ping_receiver) -> msg => match msg {
                Ok(event) => self.handle_ping_event(event, network_state),
                Err(err) => error!(target: "network", "ping_receiver error: {:?}", err),
            },
            recv(self.receivers.disc_receiver) -> msg => match msg {
                Ok(event) => self.handle_disc_event(event, network_state),
                Err(err) => error!(target: "network", "disc_receiver error: {:?}", err),
            },
            recv(self.receivers.outbound_receiver) -> msg => match msg {
                Ok(_) => self.dial_outbound_peers(network_state),
                Err(err) => error!(target: "network", "disc_receiver error: {:?}", err),
            },
            // handle stop events
            recv(self.receivers.stop_signal) -> msg => match msg {
                Ok(stop_waiter) => {
                    debug!(target: "network", "network received stop signal");
                    return HandleResult::Stop(Some(stop_waiter));
                }
                Err(err) => {
                    debug!(target: "network", "network stop signal dropped, error {:?}", err);
                    network_state.drop_all(&mut self.p2p_control.clone());
                    return HandleResult::Stop(None);
                }
            },
            //=== handle controller requests ===
            recv(self.receivers.external_urls_receiver) -> msg => match msg {
                Ok(Request {responder, arguments: count}) => {
                    let _ = responder.send(network_state.external_urls(count));
                },
                _ => {
                    error!(target: "network", "external_urls_receiver closed");
                },
            },
            recv(self.receivers.listened_addresses_receiver) -> msg => match msg {
                Ok(Request {responder, arguments: count}) => {
                    let _ = responder.send(network_state.listened_addresses(count));
                },
                _ => {
                    error!(target: "network", "listened_addresses_receiver closed");
                },
            },
            recv(self.receivers.dial_node_receiver) -> msg => match msg {
                Ok(Request {responder, arguments: (peer_id, addr)}) => {
                    let _ = responder.send(network_state.dial_node(&peer_id, addr));
                },
                _ => {
                    error!(target: "network", "dial_node_receiver closed");
                },
            },
            recv(self.receivers.connected_peers_receiver) -> msg => match msg {
                Ok(Request {responder, arguments}) => {
                    let _ = responder.send(network_state.connected_peers());
                },
                _ => {
                    error!(target: "network", "connected_peers_receiver closed");
                },
            },
            recv(self.receivers.add_discovered_addr_receiver) -> msg => match msg {
                Ok(Request {responder, arguments: (peer_id, addr)}) => {
                    let _ = responder.send(network_state.add_discovered_addr(&peer_id, addr));
                },
                _ => {
                    error!(target: "network", "add_discovered_addr_receiver closed");
                },
            },
            recv(self.receivers.broadcast_receiver) -> msg => match msg {
                Ok(Request {responder, arguments: (protocol_id, data)}) => {
                    for (_, peer) in network_state.peers_registry.iter() {
                        self.p2p_control.clone()
                            .send_message(peer.session_id, protocol_id, data.to_vec());
                    }
                    let _ = responder.send(());
                },
                _ => {
                    error!(target: "network", "add_discovered_addr_receiver closed");
                },
            },
        }
        HandleResult::Continue
    }

    fn event_loop(&mut self) {
        let mut network_state = self.network_state.borrow_mut();
        loop {
            let handle_result = self.handle_receivers(&mut network_state);
            network_state.drop_disconnect_peers(&mut self.p2p_control);

            match handle_result {
                HandleResult::Continue => {}
                HandleResult::Stop(stop_waiter) => {
                    network_state.drop_all(&mut self.p2p_control.clone());
                    if let Some(stop_waiter) = stop_waiter {
                        stop_waiter.send(());
                    }
                    // exit event loop
                    break;
                }
            }
        }
    }

    pub fn start(
        mut network_service: NetworkService,
        mut p2p_service: Service<EventHandler>,
    ) -> Result<(Runtime, std::thread::JoinHandle<()>), Error> {
        network_service.setup_network(&mut p2p_service)?;
        // spawn p2p service
        let mut runtime = Runtime::new().expect("Network tokio runtime init failed");
        debug!(target: "network", "spawn p2p service");
        runtime.spawn(p2p_service.for_each(|_| Ok(())));

        // forward p2p events to network events
        if let Some(EventForward {
            network_event_sender,
            network_event_source,
            ping_source,
            ping_sender,
            disc_source,
            disc_sender,
            outbound_interval,
            outbound_sender,
        }) = network_service.event_forward.take()
        {
            // forward p2p event to network service
            runtime.spawn(network_event_source.for_each(move |event| {
                if let Err(err) = network_event_sender.send(event) {
                    error!(target: "network", "forward network event error: {:?}", err);
                }
                Ok(())
            }));

            // ping events
            runtime.spawn(ping_source.for_each(move |event| {
                if let Err(err) = ping_sender.send(event) {
                    error!(target: "network", "forward ping event error: {:?}", err);
                }
                Ok(())
            }));

            // disc events
            runtime.spawn(disc_source.for_each(move |event| {
                if let Err(err) = disc_sender.send(event) {
                    error!(target: "network", "forward disc event error: {:?}", err);
                }
                Ok(())
            }));

            // outbound events
            runtime.spawn(
                Interval::new_interval(outbound_interval)
                    .for_each(move |event| {
                        if let Err(err) = outbound_sender.send(event) {
                            error!(target: "network", "forward outbound event error: {:?}", err);
                        }
                        Ok(())
                    })
                    .map_err(|err| ()),
            );
        }
        debug!(target: "network", "spawn network service");
        let handler = thread::spawn(move || {
            // start event loop
            network_service.event_loop();
        });
        Ok((runtime, handler))
    }

    fn setup_network(&mut self, p2p_service: &mut Service<EventHandler>) -> Result<(), Error> {
        let mut network_state = self.network_state.borrow_mut();
        let config = network_state.config.clone();
        // listen local addresses
        for addr in &config.listen_addresses {
            match p2p_service.listen(addr.to_owned()) {
                Ok(listen_address) => {
                    info!(
                    target: "network",
                    "Listen on address: {}",
                    network_state.to_external_url(&listen_address)
                    );
                    network_state
                        .original_listened_addresses
                        .push(listen_address.clone())
                }
                Err(err) => {
                    warn!(
                    target: "network",
                    "listen on address {} failed, due to error: {}",
                    addr.clone(),
                    err
                    );
                    return Err(Error::Io(err));
                }
            };
        }

        // dial reserved_nodes
        for (peer_id, addr) in config.reserved_peers()? {
            debug!(target: "network", "dial reserved_peers {:?} {:?}", peer_id, addr);
            network_state.dial_all(p2p_service.control(), &peer_id, addr);
        }

        let bootnodes = network_state
            .peer_store()
            .bootnodes(max((config.max_outbound_peers / 2) as u32, 1))
            .clone();
        // dial half bootnodes
        for (peer_id, addr) in bootnodes {
            debug!(target: "network", "dial bootnode {:?} {:?}", peer_id, addr);
            network_state.dial_all(p2p_service.control(), &peer_id, addr);
        }

        // init protocols
        self.init_protocols(&mut network_state);
        Ok(())
    }

    fn handle_service_event(&self, network_state: &mut NetworkState, event: ServiceEvent) {
        match event {
            // Register Peer
            ServiceEvent::SessionOpen { session_context } => {
                let peer_id = session_context
                    .remote_pubkey
                    .as_ref()
                    .map(|pubkey| pubkey.peer_id())
                    .expect("Secio must enabled");
                // try accept connection
                if let Err(err) = network_state.accept_connection(
                    peer_id.clone(),
                    session_context.address.clone(),
                    session_context.id,
                    session_context.ty,
                ) {
                    // disconnect immediatly
                    self.p2p_control.clone().disconnect(session_context.id);
                    info!(
                    target: "network",
                    "reject connection from {} {}, because {:?}",
                    peer_id.to_base58(),
                    session_context.address,
                    err,
                    );
                }
                debug!(target: "network", "connect new peer {:?}", peer_id);
            }
            // When session disconnect update status anyway
            ServiceEvent::SessionClose { session_context } => {
                let peer_id = session_context
                    .remote_pubkey
                    .as_ref()
                    .map(|pubkey| pubkey.peer_id())
                    .expect("Secio must enabled");
                debug!(target: "network", "disconnect from {:?}", peer_id);
                network_state.disconnect_peer(&peer_id);
            }
            _ => {
                // do nothing
            }
        }
    }

    fn handle_ping_event(&self, event: PingEvent, network_state: &mut NetworkState) {
        use PingEvent::*;
        match event {
            Ping(peer_id) => {
                trace!(target: "network", "send ping to {:?}", peer_id);
            }
            Pong(peer_id, duration) => {
                trace!(target: "network", "receive pong from {:?} duration {:?}", peer_id, duration);
                if let Some(peer) = network_state.peers_registry.get_mut(&peer_id) {
                    peer.ping = Some(duration);
                    peer.last_ping_time = Some(Instant::now());
                }
                network_state.report(&peer_id, Behaviour::Ping);
            }
            Timeout(peer_id) => {
                debug!(target: "network", "timeout to ping {:?}", peer_id);
                network_state.report(&peer_id, Behaviour::FailedToPing);
                network_state.disconnect_peer(&peer_id);
            }
            UnexpectedError(peer_id) => {
                debug!(target: "network", "failed to ping {:?}", peer_id);
                network_state.report(&peer_id, Behaviour::FailedToPing);
                network_state.disconnect_peer(&peer_id);
            }
        }
    }

    fn dial_outbound_peers(&self, network_state: &mut NetworkState) {
        let connection_status = network_state.connection_status();
        let remain_slots =
            (connection_status.max_outbound - connection_status.unreserved_outbound) as usize;
        let mut p2p_control = self.p2p_control.clone();
        if remain_slots > 0 {
            // dial peers
            let attempt_peers = network_state
                .peer_store()
                .peers_to_attempt((remain_slots + 5) as u32);
            trace!(target: "network", "count={}, attempt_peers: {:?}", remain_slots, attempt_peers);
            // TODO implement failed dials in peer store
            for (peer_id, addr) in attempt_peers
                .into_iter()
                .filter(|(peer_id, _addr)| {
                    network_state.local_peer_id() != peer_id
                        && network_state
                            .failed_dials
                            .get(peer_id)
                            .map(|last_dial| {
                                // Dial after 5 minutes when last failed
                                Instant::now() - *last_dial > Duration::from_secs(300)
                            })
                            .unwrap_or(true)
                })
                .take(remain_slots)
            {
                debug!(target: "network", "dial attempt peer: {:?}", addr);
                network_state.dial_all(&mut p2p_control, &peer_id, addr);
            }
        } else {
            // feeler peers
            let peers = network_state
                .peer_store()
                .peers_to_feeler(remain_slots as u32);
            for (peer_id, addr) in peers
                .into_iter()
                .filter(|(peer_id, _addr)| network_state.local_peer_id() != peer_id)
            {
                debug!(target: "network", "dial feeler peer: {:?}", addr);
                network_state.dial_feeler(&mut p2p_control, &peer_id, addr);
            }
        }
    }

    fn handle_disc_event(&self, event: DiscoveryEvent, network_state: &mut NetworkState) {
        use p2p::multiaddr::Protocol;
        match event {
            DiscoveryEvent::AddNewAddrs { session_id, addrs } => {
                // TODO: wait for peer store update
                for addr in addrs.into_iter() {
                    trace!(target: "network", "Add discovered address: {:?}", addr);
                    if let Some(peer_id) = extract_peer_id(&addr) {
                        let addr = addr
                            .into_iter()
                            .filter(|proto| match proto {
                                Protocol::P2p(_) => false,
                                _ => true,
                            })
                            .collect::<Multiaddr>();
                        network_state
                            .mut_peer_store()
                            .add_discovered_addr(&peer_id, addr);
                    }
                }
            }
            DiscoveryEvent::GetRandom { n, result: reply } => {
                let addrs = network_state
                    .peer_store()
                    .random_peers(n as u32)
                    .into_iter()
                    .filter_map(|(peer_id, mut addr)| {
                        Multihash::from_bytes(peer_id.into_bytes())
                            .ok()
                            .map(move |peer_id_hash| {
                                addr.append(Protocol::P2p(peer_id_hash));
                                addr
                            })
                    })
                    .collect();
                trace!(target: "network", "discovery send random addrs: {:?}", addrs);
                reply
                    .send(addrs)
                    .expect("Send failed (should not happened)");
            }
            event => {
                trace!(target: "network", "ignore discovery event");
            }
        }
    }

    fn handle_service_error(&self, network_state: &mut NetworkState, error: ServiceError) {
        if let ServiceError::DialerError {
            ref address,
            ref error,
        } = error
        {
            debug!(target: "network", "add self address: {:?}", address);
            if error == &P2pError::ConnectSelf {
                let addr = address
                    .iter()
                    .filter(|proto| match proto {
                        multiaddr::Protocol::P2p(_) => false,
                        _ => true,
                    })
                    .collect();
                network_state.listened_addresses.insert(addr, std::u8::MAX);
            }
            // TODO implement in peer store
            if let Some(peer_id) = extract_peer_id(address) {
                network_state.failed_dials.insert(peer_id, Instant::now());
            }
        }
    }

    fn handle_protocol(&self, network_state: &mut NetworkState, event: ProtocolEvent) {
        let p2p_control = self.p2p_control.clone();
        match event {
            ProtocolEvent::Connected {
                session_context,
                proto_id,
                version,
            } => {
                let protocol = match self.find_protocol(proto_id) {
                    Some(p) => p,
                    None => return,
                };
                let peer_id = session_context
                    .remote_pubkey
                    .as_ref()
                    .map(|pubkey| pubkey.peer_id())
                    .expect("Secio must enabled");
                let proto_version = version
                    .parse::<ProtocolVersion>()
                    .expect("parse protocol version");
                // register new protocol
                if let Err(err) =
                    network_state.peer_new_protocol(peer_id.clone(), proto_id, proto_version)
                {
                    error!(target: "network", "disconnect peer {:?}, because {:?}",peer_id, err);
                    network_state.disconnect_peer(&peer_id);
                    return;
                } // call handler
                let peer_index = network_state.get_peer_index(&peer_id).expect("peer index");
                protocol.handler().connected(
                    &mut DefaultCKBProtocolContext::new(proto_id, network_state, p2p_control),
                    peer_index,
                );
            }

            ProtocolEvent::Received {
                session_context,
                proto_id,
                data,
            } => {
                let peer_id = session_context
                    .remote_pubkey
                    .as_ref()
                    .map(|pubkey| pubkey.peer_id())
                    .expect("Secio must enabled");
                if let Some(protocol) = self.find_protocol(proto_id) {
                    let peer_index = network_state.get_peer_index(&peer_id).expect("peer index");
                    protocol.handler().received(
                        &mut DefaultCKBProtocolContext::new(proto_id, network_state, p2p_control),
                        peer_index,
                        data,
                    );
                }
            }
            ProtocolEvent::Disconnected {
                proto_id,
                session_context,
            } => {
                let peer_id = session_context
                    .remote_pubkey
                    .as_ref()
                    .map(|pubkey| pubkey.peer_id())
                    .expect("Secio must enabled");
                if let Some(protocol) = self.find_protocol(proto_id) {
                    let peer_index = network_state.get_peer_index(&peer_id).expect("peer index");
                    protocol.handler().disconnected(
                        &mut DefaultCKBProtocolContext::new(proto_id, network_state, p2p_control),
                        peer_index,
                    );
                }
            }
            ProtocolEvent::ProtocolNotify { proto_id, token } => {
                if let Some(protocol) = self.find_protocol(proto_id) {
                    protocol.handler().timer_triggered(
                        &mut DefaultCKBProtocolContext::new(proto_id, network_state, p2p_control),
                        token,
                    );
                }
            }
            ProtocolEvent::ProtocolSessionNotify {
                session_context,
                proto_id,
                token,
            } => {
                // ignore
            }
        }
    }
    fn init_protocols(&self, network_state: &mut NetworkState) {
        let p2p_control = self.p2p_control.clone();
        for p in &self.protocols {
            p.handler().initialize(&mut DefaultCKBProtocolContext::new(
                p.id(),
                network_state,
                p2p_control.clone(),
            ));
        }
    }

    fn find_protocol(&self, proto_id: ProtocolId) -> Option<&CKBProtocol> {
        self.protocols.iter().find(|p| p.id() == proto_id)
    }
}

#[derive(Clone)]
pub struct NetworkController {
    peer_id: PeerId,
    external_urls_sender: Sender<Request<usize, Vec<(String, u8)>>>,
    listened_addresses_sender: Sender<Request<usize, Vec<(Multiaddr, u8)>>>,
    dial_node_sender: Sender<Request<(PeerId, Multiaddr), ()>>,
    connected_peers_sender: Sender<Request<(), Vec<(PeerId, Peer, MultiaddrList)>>>,
    add_discovered_addr_sender: Sender<Request<(PeerId, Multiaddr), ()>>,
    broadcast_sender: Sender<Request<(ProtocolId, Vec<u8>), ()>>,
    stop_sender: Sender<Sender<()>>,
}

impl NetworkController {
    pub fn external_urls(&self, max_urls: usize) -> Vec<(String, u8)> {
        Request::call(&self.external_urls_sender, max_urls).expect("external_urls() failed")
    }

    pub fn listened_addresses(&self, count: usize) -> Vec<(Multiaddr, u8)> {
        Request::call(&self.listened_addresses_sender, count).expect("listened_addresses() failed")
    }

    pub fn dial_node(&self, peer_id: PeerId, addr: Multiaddr) {
        Request::call(&self.dial_node_sender, (peer_id, addr)).expect("dial_node() failed")
    }

    pub fn add_discovered_addr(&self, peer_id: PeerId, addr: Multiaddr) {
        Request::call(&self.add_discovered_addr_sender, (peer_id, addr))
            .expect("add_discovered_addr() failed")
    }

    pub fn local_peer_id(&self) -> &PeerId {
        &self.peer_id
    }

    pub fn node_id(&self) -> String {
        self.peer_id.to_base58()
    }

    /// Send stop signal to network, then wait until network shutdown
    pub fn shutdown(&mut self) {
        let (stopped_sender, stopped_receiver) = crossbeam_channel::bounded(1);
        self.stop_sender.send(stopped_sender);
        // NOTICE return a disconnect error is in expect, which mean network stream is dropped.
        if let Err(err) = stopped_receiver.recv() {
            debug!(target: "network", "network stopped {:?}", err);
        }
        info!(target: "network", "network shutdown");
    }

    pub fn connected_peers(&self) -> Vec<(PeerId, Peer, MultiaddrList)> {
        Request::call(&self.connected_peers_sender, ()).expect("connected_peers() failed")
    }

    pub fn broadcast(&self, protocol_id: ProtocolId, data: Vec<u8>) {
        Request::call(&self.broadcast_sender, (protocol_id, data)).expect("broadcast() failed")
    }
}
