use super::Network;
use super::PeerId;
use futures::future::{self, Future};
use futures::stream::FuturesUnordered;
use futures::Stream;
use libp2p::core::Multiaddr;
use libp2p::core::SwarmController;
use libp2p::core::{upgrade, MuxedTransport};
use libp2p::{self, ping};
use peer_store::Behaviour;
use protocol::Protocol;
use protocol_service::ProtocolService;
use std::boxed::Box;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::timer::{Interval, Timeout};
use transport::TransportOutput;

pub struct PingService {
    ping_interval: Duration,
    ping_timeout: Duration,
}
impl PingService {
    pub fn new(ping_interval: Duration, ping_timeout: Duration) -> Self {
        PingService {
            ping_interval,
            ping_timeout,
        }
    }

    fn ping_to_protocol<T>(peer_id: PeerId, output: ping::PingOutput) -> Protocol<T> {
        match output {
            ping::PingOutput::Ponger(processing) => Protocol::Pong(processing, peer_id),
            ping::PingOutput::Pinger { pinger, processing } => {
                Protocol::Ping(pinger, processing, peer_id)
            }
        }
    }
}

impl<T: Send> ProtocolService<T> for PingService {
    type Output = ping::PingOutput;
    fn convert_to_protocol(
        peer_id: Arc<PeerId>,
        _addr: &Multiaddr,
        output: Self::Output,
    ) -> Protocol<T> {
        Self::ping_to_protocol(PeerId::clone(&peer_id), output)
    }
    fn handle(
        &self,
        network: Arc<Network>,
        protocol: Protocol<T>,
    ) -> Box<Future<Item = (), Error = IoError> + Send> {
        match protocol {
            Protocol::Pong(processing, _peer_id) => {
                Box::new(processing) as Box<Future<Item = _, Error = _> + Send>
            }
            Protocol::Ping(pinger, processing, peer_id) => {
                match network.peers_registry().read().get(&peer_id) {
                    Some(peer) => {
                        // ping and store pinger
                        Box::new(peer.pinger_loader.tie_or_passthrough(pinger, processing))
                            as Box<Future<Item = _, Error = _> + Send>
                    }
                    None => Box::new(future::err(IoError::new(
                        IoErrorKind::Other,
                        "ping protocol can't find peer",
                    ))) as Box<Future<Item = _, Error = _> + Send>,
                }
            }
            _ => Box::new(future::ok(())) as Box<Future<Item = _, Error = _> + Send>,
        }
    }

    // Periodicly ping peers
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
        let transport = transport.and_then(move |out, endpoint, client_addr| {
            let peer_id = out.peer_id;
            upgrade::apply(out.socket, libp2p::ping::Ping, endpoint, client_addr)
                .map(move |(out, addr)| (Self::ping_to_protocol(peer_id, out), addr))
        });

        let periodic_ping_future =
            Interval::new(Instant::now() + Duration::from_secs(5), self.ping_interval)
                .map_err(|err| IoError::new(IoErrorKind::Other, err))
                .for_each({
                    let network = Arc::clone(&network);
                    let transport = transport.clone();
                    let ping_timeout = self.ping_timeout;
                    move |_| {
                        let mut ping_futures = FuturesUnordered::new();
                        let peers_registry = network.peers_registry().read();
                        // build ping future for each peer
                        for (peer_id, peer) in peers_registry.peers_iter() {
                            let peer_id = peer_id.clone();
                            // only ping first address?
                            if let Some(addr) = peer.remote_addresses.get(0) {
                                let ping_future = peer
                                    .pinger_loader
                                    .dial(&swarm_controller, &addr, transport.clone())
                                    .and_then({
                                        let peer_id = peer_id.clone();
                                        move |mut pinger| {
                                            pinger.ping().map(|_| peer_id).map_err(|err| {
                                                IoError::new(IoErrorKind::Other, err)
                                            })
                                        }
                                    });
                                let ping_start_time = Instant::now();
                                let ping_future = Timeout::new(ping_future, ping_timeout).then({
                                    let network = Arc::clone(&network);
                                    move |result| -> Result<(), IoError> {
                                        let mut peer_store = network.peer_store().write();
                                        match result {
                                            Ok(peer_id) => {
                                                let received_during = ping_start_time.elapsed();
                                                peer_store.report(&peer_id, Behaviour::Ping);
                                                trace!(
                                                    "received pong from {:?} in {:?}",
                                                    peer_id,
                                                    received_during
                                                );
                                                Ok(())
                                            }
                                            Err(err) => {
                                                peer_store
                                                    .report(&peer_id, Behaviour::FailedToPing);
                                                network
                                                    .peers_registry()
                                                    .write()
                                                    .drop_peer(&peer_id);
                                                trace!(
                                                    "error when send ping to {:?}, error: {:?}",
                                                    peer_id,
                                                    err
                                                );
                                                Ok(())
                                            }
                                        }
                                    }
                                });
                                ping_futures.push(Box::new(ping_future)
                                    as Box<Future<Item = _, Error = _> + Send>);
                            }
                        }
                        Box::new(
                            ping_futures
                                .into_future()
                                .map(|_| ())
                                .map_err(|(err, _)| err),
                        ) as Box<Future<Item = _, Error = _> + Send>
                    }
                }).then(|err| {
                    warn!("Ping service stopped, reason: {:?}", err);
                    err
                });
        Box::new(periodic_ping_future) as Box<Future<Item = _, Error = _> + Send>
    }
}
