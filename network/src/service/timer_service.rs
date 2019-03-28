use crate::errors::Error;
use crate::protocol::ckb_handler::DefaultCKBProtocolContext;
use crate::Network;
use crate::{CKBProtocolHandler, ProtocolId};
use ckb_util::Mutex;
use futures::future::{self, Future};
use futures::stream::FuturesUnordered;
use futures::IntoFuture;
use futures::Stream;
use log::trace;
use std::boxed::Box;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::sync::Arc;
use std::time::Duration;
use tokio;
use tokio::timer::Interval;

pub type TimerToken = usize;
pub type Timer = (Arc<CKBProtocolHandler>, ProtocolId, TimerToken, Duration);
pub type TimerRegistry = Arc<Mutex<Option<Vec<Timer>>>>;
type TimerFutures = FuturesUnordered<Box<dyn Future<Item = (), Error = Error> + Send>>;

pub struct TimerService {
    pub timer_futures: TimerFutures,
}

impl TimerService {
    pub fn new(timer_registry: TimerRegistry, network: Arc<Network>) -> Self {
        let mut timer_futures: TimerFutures = FuturesUnordered::new();
        if let Some(timers) = timer_registry.lock().take() {
            trace!(target: "network", "start timer service, timer count: {}", timers.len());
            // register timers
            for (handler, protocol_id, timer_symbol, duration) in timers {
                trace!(
                target: "network",
                "register timer: timer_symbol {} protocol {:?} duration: {:?}",
                timer_symbol,
                protocol_id,
                duration
                );
                let timer_interval = Interval::new_interval(duration);
                let timer_future = Box::new(timer_interval.for_each({
                    let network = Arc::clone(&network);
                    move |_| {
                        let network = Arc::clone(&network);
                        let handler = Arc::clone(&handler);
                        let handle_timer = future::lazy(move || {
                            handler.timer_triggered(
                                Box::new(DefaultCKBProtocolContext::new(
                                    Arc::clone(&network),
                                    protocol_id,
                                )),
                                timer_symbol,
                            );
                            Ok(())
                        });
                        tokio::spawn(handle_timer);
                        Ok(())
                    }
                }));
                timer_futures.push(Box::new(timer_future.into_future().map_err(|err| {
                    Error::Io(IoError::new(
                        IoErrorKind::Other,
                        format!("timer error : {}", err),
                    ))
                })));
            }
        }
        TimerService { timer_futures }
    }
}
