use crate::protocols::BackgroundService;
use crate::{Behaviour, NetworkState, Peer};
use futures::{sync::mpsc::Receiver, try_ready, Async, Stream};
use log::{debug, error, trace};
use p2p::service::ServiceControl;
use p2p_ping::Event;
use std::sync::Arc;
use std::time::Instant;

pub struct PingService {
    p2p_control: ServiceControl,
    event_receiver: Receiver<Event>,
}

impl PingService {
    pub fn new(p2p_control: ServiceControl, event_receiver: Receiver<Event>) -> PingService {
        PingService {
            p2p_control,
            event_receiver,
        }
    }
}

impl BackgroundService for PingService {
    fn poll(&mut self, network_state: &mut NetworkState) -> Result<bool, ()> {
        use Event::*;

        match self.event_receiver.poll() {
            Ok(Async::Ready(event)) => {
                match event {
                    Some(Ping(peer_id)) => {
                        trace!(target: "network", "send ping to {:?}", peer_id);
                    }
                    Some(Pong(peer_id, duration)) => {
                        trace!(target: "network", "receive pong from {:?} duration {:?}", peer_id, duration);
                        network_state.modify_peer(&peer_id, |peer: &mut Peer| {
                            peer.ping = Some(duration);
                            peer.last_ping_time = Some(Instant::now());
                        });
                        network_state.report(&peer_id, Behaviour::Ping);
                    }
                    Some(Timeout(peer_id)) => {
                        debug!(target: "network", "timeout to ping {:?}", peer_id);
                        network_state.report(&peer_id, Behaviour::FailedToPing);
                        network_state.drop_peer(&mut self.p2p_control, &peer_id);
                    }
                    Some(UnexpectedError(peer_id)) => {
                        debug!(target: "network", "failed to ping {:?}", peer_id);
                        network_state.report(&peer_id, Behaviour::FailedToPing);
                        network_state.drop_peer(&mut self.p2p_control, &peer_id);
                    }
                    None => {
                        debug!(target: "network", "ping service shutdown");
                    }
                }
                Ok(true)
            }
            Ok(Async::NotReady) => Ok(false),
            Err(err) => {
                error!(target: "network", "ping service error: {:?}", err);
                Err(())
            }
        }
    }
}
