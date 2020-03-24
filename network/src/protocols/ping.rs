use crate::network::disconnect_with_message;
use crate::NetworkState;
use ckb_logger::{debug, trace};
use futures::{channel::mpsc::Receiver, Stream, StreamExt};
use p2p::service::ServiceControl;
use p2p_ping::Event;
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Instant,
};

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
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        use Event::*;

        loop {
            match self.event_receiver.poll_next_unpin(cx) {
                Poll::Ready(Some(Ping(peer_id))) => {
                    trace!("send ping to {:?}", peer_id);
                }
                Poll::Ready(Some(Pong(peer_id, duration))) => {
                    trace!("receive pong from {:?} duration {:?}", peer_id, duration);
                    if let Some(session_id) = self.network_state.query_session_id(&peer_id) {
                        self.network_state.with_peer_registry_mut(|reg| {
                            if let Some(mut peer) = reg.get_peer_mut(session_id) {
                                peer.ping = Some(duration);
                                peer.last_ping_time = Some(Instant::now());
                            }
                        })
                    }
                }
                Poll::Ready(Some(Timeout(peer_id))) => {
                    debug!("timeout to ping {:?}", peer_id);
                    if let Some(session_id) = self.network_state.with_peer_registry_mut(|reg| {
                        reg.remove_peer_by_peer_id(&peer_id)
                            .map(|peer| peer.session_id)
                    }) {
                        if let Err(err) =
                            disconnect_with_message(&self.p2p_control, session_id, "ping timeout")
                        {
                            debug!("Disconnect failed {:?}, error: {:?}", session_id, err);
                        }
                    }
                }
                Poll::Ready(Some(UnexpectedError(peer_id))) => {
                    debug!("failed to ping {:?}", peer_id);
                    if let Some(session_id) = self.network_state.with_peer_registry_mut(|reg| {
                        reg.remove_peer_by_peer_id(&peer_id)
                            .map(|peer| peer.session_id)
                    }) {
                        if let Err(err) =
                            disconnect_with_message(&self.p2p_control, session_id, "ping failed")
                        {
                            debug!("Disconnect failed {:?}, error: {:?}", session_id, err);
                        }
                    }
                }
                Poll::Ready(None) => {
                    debug!("ping service shutdown");
                    return Poll::Ready(None);
                }
                Poll::Pending => break,
            }
        }
        Poll::Pending
    }
}
