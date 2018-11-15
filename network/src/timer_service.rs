use super::Network;
use super::PeerId;
use super::{CKBProtocolHandler, ProtocolId, TimerToken};
use ckb_protocol_handler::DefaultCKBProtocolContext;
use futures::future::{self, Future};
use futures::stream::FuturesUnordered;
use futures::Stream;
use libp2p::core::Multiaddr;
use libp2p::core::{MuxedTransport, SwarmController};
use protocol::Protocol;
use protocol_service::ProtocolService;
use std::boxed::Box;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::sync::Arc;
use std::time::Duration;
use tokio;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::timer::Interval;
use transport::TransportOutput;
use util::Mutex;

pub(crate) type Timer = (Arc<CKBProtocolHandler>, ProtocolId, TimerToken, Duration);

pub struct TimerService {
    pub timer_registry: Arc<Mutex<Option<Vec<Timer>>>>,
}

impl<T: Send> ProtocolService<T> for TimerService {
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

    fn start_protocol<SwarmTran, Tran, TranOut>(
        &self,
        network: Arc<Network>,
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
        Box::new(future::lazy({
            let network = Arc::clone(&network);
            let timer_registry = Arc::clone(&self.timer_registry);
            move || -> Box<Future<Item = (), Error = IoError> + Send> {
                let timer_registry = timer_registry.lock().take().unwrap();
                let mut timer_futures = FuturesUnordered::new();
                trace!(target: "network", "start timer service, timer count: {}", timer_registry.len());
                // register timers
                for (handler, protocol_id, timer_symbol, duration) in timer_registry {
                    trace!(
                        target: "network",
                        "register timer: timer_symbol {} protocol {:?} duration: {:?}",
                        timer_symbol,
                        protocol_id,
                        duration
                    );
                    let timer_interval = Interval::new_interval(duration);
                    let network_clone = Arc::clone(&network);
                    let handler_clone = Arc::clone(&handler);
                    let timer_future = Box::new(
                        timer_interval
                            .for_each(move |_| {
                                let network_clone = Arc::clone(&network_clone);
                                let handler_clone = Arc::clone(&handler_clone);
                                let handle_timer = future::lazy(move || {
                                    handler_clone.timer_triggered(
                                        Box::new(DefaultCKBProtocolContext::new(
                                            Arc::clone(&network_clone),
                                            protocol_id,
                                        )),
                                        timer_symbol,
                                    );
                                    Ok(())
                                });
                                tokio::spawn(handle_timer);
                                Ok(())
                            }).map_err(|err| IoError::new(IoErrorKind::Other, err)),
                    );
                    timer_futures.push(timer_future);
                }
                if timer_futures.is_empty() {
                    Box::new(future::empty()) as Box<Future<Item = (), Error = IoError> + Send>
                } else {
                    Box::new(
                        timer_futures
                            .into_future()
                            .map(|_| ())
                            .map_err(|(err, _)| err),
                    ) as Box<Future<Item = (), Error = IoError> + Send>
                }
            }
        })) as Box<Future<Item = (), Error = IoError> + Send>
    }
}
