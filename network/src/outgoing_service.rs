use crate::protocol::Protocol;
use crate::protocol_service::ProtocolService;
use crate::transport::TransportOutput;
use crate::Network;
use crate::PeerId;
use futures::future::{self, lazy, Future};
use futures::Stream;
use libp2p::core::Multiaddr;
use libp2p::core::MuxedTransport;
use libp2p::core::SwarmController;
use log::warn;
use std::boxed::Box;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use std::usize;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::timer::Interval;

pub struct OutgoingService {
    pub outgoing_interval: Duration,
    pub timeout: Duration,
}

impl<T: Send + 'static> ProtocolService<T> for OutgoingService {
    type Output = ();
    fn convert_to_protocol(
        _peer_id: Arc<PeerId>,
        _addr: &Multiaddr,
        _output: Self::Output,
    ) -> Protocol<T> {
        unreachable!()
    }
    fn handle(
        &self,
        _network: Arc<Network>,
        _protocol: Protocol<T>,
    ) -> Box<Future<Item = (), Error = IoError> + Send> {
        unreachable!()
    }

    // Periodicly connect to new peers
    #[allow(clippy::let_and_return)]
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
        let outgoing_future = Interval::new(
            Instant::now() + Duration::from_secs(5),
            self.outgoing_interval,
        )
        .map_err(|err| {
            IoError::new(
                IoErrorKind::Other,
                format!("outgoing service error {:?}", err),
            )
        })
        .for_each({
            let transport = transport.clone();
            let timeout = self.timeout;
            let network = Arc::clone(&network);
            move |_| {
                let connection_status = network.connection_status();
                let new_outgoing = (connection_status.max_outgoing
                    - connection_status.unreserved_outgoing)
                    as usize;
                if new_outgoing > 0 {
                    let peer_store = network.peer_store().read();
                    for (peer_id, addr) in peer_store
                        .peers_to_attempt()
                        .take(new_outgoing)
                        .filter_map(|(peer_id, addr)| {
                            if network.local_peer_id() != peer_id {
                                Some((peer_id.clone(), addr.clone()))
                            } else {
                                None
                            }
                        })
                    {
                        network.dial_to_peer(
                            transport.clone(),
                            &addr,
                            &peer_id,
                            &swarm_controller,
                            timeout,
                        );
                    }
                }

                Box::new(lazy(|| future::ok(()))) as Box<Future<Item = _, Error = _> + Send>
            }
        })
        .then(|err| {
            warn!(target: "network", "Outgoing service stopped, reason: {:?}", err);
            err
        });
        Box::new(outgoing_future) as Box<Future<Item = _, Error = _> + Send>
    }
}
