use super::Network;
use crate::protocol::Protocol;
use crate::transport::TransportOutput;
use futures::future::{empty as empty_future, Future};
use libp2p::core::Multiaddr;
use libp2p::core::SwarmController;
use libp2p::core::{MuxedTransport, PeerId};
use std::boxed::Box;
use std::io::Error as IoError;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};

pub trait ProtocolService<T> {
    type Output;
    fn convert_to_protocol(
        peer_id: Arc<PeerId>,
        addr: &Multiaddr,
        output: Self::Output,
    ) -> Protocol<T>;

    fn handle(
        &self,
        _network: Arc<Network>,
        protocol: Protocol<T>,
    ) -> Box<Future<Item = (), Error = IoError> + Send>;

    fn start_protocol<SwarmTran, Tran, TranOut>(
        &self,
        _network: Arc<Network>,
        _swarm_controller: SwarmController<
            SwarmTran,
            Box<Future<Item = (), Error = IoError> + Send>,
        >,
        _transport: Tran,
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
        Box::new(empty_future())
    }
}
