use crate::Behaviour;
use crate::Network;
use crate::Peer;
use futures::{sync::mpsc::Receiver, Async, Stream};
use log::{debug, trace};
use p2p_ping::Event;
use std::sync::Arc;
use std::time::Instant;

pub struct PingService {
    pub event_receiver: Receiver<Event>,
    pub network: Arc<Network>,
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
                self.network.modify_peer(&peer_id, |peer: &mut Peer| {
                    peer.ping = Some(duration);
                    peer.last_ping_time = Some(Instant::now());
                });
                self.network.report(&peer_id, Behaviour::Ping);
            }
            Some(Timeout(peer_id)) => {
                debug!(target: "network", "timeout to ping {:?}", peer_id);
                self.network.report(&peer_id, Behaviour::FailedToPing);
                self.network.drop_peer(&peer_id);
            }
            Some(UnexpectedError(peer_id)) => {
                debug!(target: "network", "failed to ping {:?}", peer_id);
                self.network.report(&peer_id, Behaviour::FailedToPing);
                self.network.drop_peer(&peer_id);
            }
            None => {
                debug!(target: "network", "ping service shutdown");
                return Ok(Async::Ready(None));
            }
        }
        Ok(Async::Ready(Some(())))
    }
}
