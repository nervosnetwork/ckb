use crate::{NetworkState, Peer};
use futures::{sync::mpsc::Receiver, try_ready, Async, Stream};
use log::{debug, trace};
use p2p::service::ServiceControl;
use p2p_ping::Event;
use std::sync::Arc;
use std::time::Instant;

pub struct PingService {
    network_state: Arc<NetworkState>,
    p2p_control: ServiceControl,
    event_receiver: Receiver<Event>,
}

impl PingService {
    pub fn new(
        network_state: Arc<NetworkState>,
        p2p_control: ServiceControl,
        event_receiver: Receiver<Event>,
    ) -> PingService {
        PingService {
            network_state,
            p2p_control,
            event_receiver,
        }
    }
}

impl Stream for PingService {
    type Item = ();
    type Error = ();
    fn poll(&mut self) -> Result<Async<Option<Self::Item>>, Self::Error> {
        use Event::*;

        match try_ready!(self.event_receiver.poll()) {
            Some(Ping(peer_id)) => {
                trace!(target: "network", "send ping to {:?}", peer_id);
            }
            Some(Pong(peer_id, duration)) => {
                trace!(target: "network", "receive pong from {:?} duration {:?}", peer_id, duration);
                self.network_state.modify_peer(&peer_id, |peer: &mut Peer| {
                    peer.ping = Some(duration);
                    peer.last_ping_time = Some(Instant::now());
                });
            }
            Some(Timeout(peer_id)) => {
                debug!(target: "network", "timeout to ping {:?}", peer_id);
                self.network_state
                    .drop_peer(&mut self.p2p_control, &peer_id);
            }
            Some(UnexpectedError(peer_id)) => {
                debug!(target: "network", "failed to ping {:?}", peer_id);
                self.network_state
                    .drop_peer(&mut self.p2p_control, &peer_id);
            }
            None => {
                debug!(target: "network", "ping service shutdown");
                return Ok(Async::Ready(None));
            }
        }
        Ok(Async::Ready(Some(())))
    }
}
