use super::Network;
use futures::future::{empty as empty_future, Future};
use libp2p::core::Multiaddr;
use libp2p::core::SwarmController;
use libp2p::core::{MuxedTransport, PeerId};
use protocol::Protocol;
use std::boxed::Box;
use std::io::Error as IoError;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use transport::TransportOutput;

pub trait ProtocolService<T> {
    type Output;
    fn convert_to_protocol(Arc<PeerId>, &Multiaddr, Self::Output) -> Protocol<T>;
    fn handle(&self, Arc<Network>, Protocol<T>) -> Box<Future<Item = (), Error = IoError> + Send>;
    fn start_protocol<SwarmTran, Tran, TranOut>(
        &self,
        Arc<Network>,
        SwarmController<SwarmTran, Box<Future<Item = (), Error = IoError> + Send>>,
        Tran,
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
