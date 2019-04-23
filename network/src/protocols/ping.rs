use crate::NetworkState;
use futures::{sync::mpsc::Receiver, try_ready, Async, Stream};
use log::{debug, trace, warn};
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
                if let Some(session_id) = self.network_state.query_session_id(&peer_id) {
                    self.network_state.with_peer_registry_mut(|reg| {
                        if let Some(mut peer) = reg.get_peer_mut(session_id) {
                            peer.ping = Some(duration);
                            peer.last_ping_time = Some(Instant::now());
                        }
                    })
                }
            }
            Some(Timeout(peer_id)) => {
                debug!(target: "network", "timeout to ping {:?}", peer_id);
                if let Some(session_id) = self.network_state.with_peer_registry_mut(|reg| {
                    reg.remove_peer_by_peer_id(&peer_id)
                        .map(|peer| peer.session_id)
                }) {
                    if let Err(err) = self.p2p_control.disconnect(session_id) {
                        warn!(
                            target: "network",
                            "send disconnect failed {} => {:?}, error={:?}",
                            session_id,
                            peer_id,
                            err,
                        );
                    }
                }
            }
            Some(UnexpectedError(peer_id)) => {
                debug!(target: "network", "failed to ping {:?}", peer_id);
                if let Some(session_id) = self.network_state.with_peer_registry_mut(|reg| {
                    reg.remove_peer_by_peer_id(&peer_id)
                        .map(|peer| peer.session_id)
                }) {
                    if let Err(err) = self.p2p_control.disconnect(session_id) {
                        warn!(
                            target: "network",
                            "send disconnect failed {} => {:?}, error={:?}",
                            session_id,
                            peer_id,
                            err,
                        );
                    }
                }
            }
            None => {
                debug!(target: "network", "ping service shutdown");
                return Ok(Async::Ready(None));
            }
        }
        Ok(Async::Ready(Some(())))
    }
}
