use crate::network::disconnect_with_message;
use crate::NetworkState;
use ckb_logger::{debug, error, trace, warn};
use futures::{
    channel::mpsc::{Receiver, Sender},
    Stream, StreamExt,
};
use std::{
    collections::HashMap,
    str,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Instant,
};

use ckb_types::{packed, prelude::*};
use p2p::{
    bytes::Bytes,
    context::{ProtocolContext, ProtocolContextMutRef},
    secio::PeerId,
    service::{ServiceControl, TargetSession},
    traits::ServiceProtocol,
    SessionId,
};

const SEND_PING_TOKEN: u64 = 0;
const CHECK_TIMEOUT_TOKEN: u64 = 1;

/// Ping protocol events
#[derive(Debug)]
pub enum Event {
    /// Peer send ping to us.
    Ping(PeerId),
    /// Peer send pong to us.
    Pong(PeerId, Duration),
    /// Peer is timeout.
    Timeout(PeerId),
    /// Peer cause a unexpected error.
    UnexpectedError(PeerId),
}

/// Ping protocol handler.
///
/// The interval means that we send ping to peers.
/// The timeout means that consider peer is timeout if during a timeout we still have not received pong from a peer
pub struct PingHandler {
    interval: Duration,
    timeout: Duration,
    connected_session_ids: HashMap<SessionId, PingStatus>,
    event_sender: Sender<Event>,
}

impl PingHandler {
    pub fn new(interval: Duration, timeout: Duration, event_sender: Sender<Event>) -> PingHandler {
        PingHandler {
            interval,
            timeout,
            connected_session_ids: Default::default(),
            event_sender,
        }
    }

    pub fn send_event(&mut self, event: Event) {
        if let Err(err) = self.event_sender.try_send(event) {
            error!("send ping event error: {}", err);
        }
    }
}

/// PingStatus of a peer
#[derive(Clone, Debug)]
struct PingStatus {
    /// Are we currently pinging this peer?
    processing: bool,
    /// The time we last send ping to this peer.
    last_ping: SystemTime,
    peer_id: PeerId,
    version: String,
}

impl PingStatus {
    /// A meaningless value, peer must send a pong has same nonce to respond a ping.
    fn nonce(&self) -> u32 {
        self.last_ping
            .duration_since(UNIX_EPOCH)
            .map(|dur| dur.as_secs())
            .unwrap_or(0) as u32
    }

    /// Time duration since we last send ping.
    fn elapsed(&self) -> Duration {
        self.last_ping.elapsed().unwrap_or(Duration::from_secs(0))
    }
}

impl ServiceProtocol for PingHandler {
    fn init(&mut self, context: &mut ProtocolContext) {
        // periodicly send ping to peers
        let proto_id = context.proto_id;
        if context
            .set_service_notify(proto_id, self.interval, SEND_PING_TOKEN)
            .is_err()
        {
            warn!("start ping fail");
        }
        if context
            .set_service_notify(proto_id, self.timeout, CHECK_TIMEOUT_TOKEN)
            .is_err()
        {
            warn!("start ping fail");
        }
    }

    fn connected(&mut self, context: ProtocolContextMutRef, version: &str) {
        let session = context.session;
        match session.remote_pubkey {
            Some(ref pubkey) => {
                let peer_id = pubkey.peer_id();
                self.connected_session_ids
                    .entry(session.id)
                    .or_insert_with(|| PingStatus {
                        last_ping: SystemTime::now(),
                        processing: false,
                        peer_id,
                        version: version.to_owned(),
                    });
                debug!(
                    "proto id [{}] open on session [{}], address: [{}], type: [{:?}], version: {}",
                    context.proto_id, session.id, session.address, session.ty, version
                );
                debug!("connected sessions are: {:?}", self.connected_session_ids);
            }
            None => {
                if context.disconnect(session.id).is_err() {
                    debug!("disconnect fail");
                }
            }
        }
    }

    fn disconnected(&mut self, context: ProtocolContextMutRef) {
        let session = context.session;
        self.connected_session_ids.remove(&session.id);
        debug!(
            "proto id [{}] close on session [{}]",
            context.proto_id, session.id
        );
    }

    fn received(&mut self, context: ProtocolContextMutRef, data: Bytes) {
        let session = context.session;
        if let Some(peer_id) = self
            .connected_session_ids
            .get(&session.id)
            .map(|ps| ps.peer_id.clone())
        {
            match PingMessage::decode(data.as_ref()) {
                None => {
                    error!("decode message error");
                    self.send_event(Event::UnexpectedError(peer_id));
                }
                Some(msg) => {
                    match msg {
                        PingPayload::Ping(nonce) => {
                            if context
                                .send_message(PingMessage::build_pong(nonce))
                                .is_err()
                            {
                                debug!("send message fail");
                            }
                            self.send_event(Event::Ping(peer_id));
                        }
                        PingPayload::Pong(nonce) => {
                            // check pong
                            if self
                                .connected_session_ids
                                .get(&session.id)
                                .map(|ps| (ps.processing, ps.nonce()))
                                == Some((true, nonce))
                            {
                                let ping_time =
                                    match self.connected_session_ids.get_mut(&session.id) {
                                        Some(ps) => {
                                            ps.processing = false;
                                            ps.elapsed()
                                        }
                                        None => return,
                                    };
                                self.send_event(Event::Pong(peer_id, ping_time));
                            } else {
                                // ignore if nonce is incorrect
                                self.send_event(Event::UnexpectedError(peer_id));
                            }
                        }
                    }
                }
            }
        }
    }

    fn notify(&mut self, context: &mut ProtocolContext, token: u64) {
        match token {
            SEND_PING_TOKEN => {
                debug!("proto [{}] start ping peers", context.proto_id);
                let now = SystemTime::now();
                let peers: Vec<(SessionId, u32)> = self
                    .connected_session_ids
                    .iter_mut()
                    .filter_map(|(session_id, ps)| {
                        if ps.processing {
                            None
                        } else {
                            ps.processing = true;
                            ps.last_ping = now;
                            Some((*session_id, ps.nonce()))
                        }
                    })
                    .collect();
                if !peers.is_empty() {
                    let ping_msg = PingMessage::build_ping(peers[0].1);
                    let peer_ids: Vec<SessionId> = peers
                        .into_iter()
                        .map(|(session_id, _)| session_id)
                        .collect();
                    let proto_id = context.proto_id;
                    if context
                        .filter_broadcast(TargetSession::Multi(peer_ids), proto_id, ping_msg)
                        .is_err()
                    {
                        debug!("send message fail");
                    }
                }
            }
            CHECK_TIMEOUT_TOKEN => {
                debug!("proto [{}] check ping timeout", context.proto_id);
                let timeout = self.timeout;
                for peer_id in self
                    .connected_session_ids
                    .values()
                    .filter(|ps| ps.processing && ps.elapsed() >= timeout)
                    .map(|ps| ps.peer_id.clone())
                    .collect::<Vec<PeerId>>()
                {
                    self.send_event(Event::Timeout(peer_id));
                }
            }
            _ => panic!("unknown token {}", token),
        }
    }
}

enum PingPayload {
    Ping(u32),
    Pong(u32),
}

struct PingMessage;

impl PingMessage {
    fn build_ping(nonce: u32) -> Bytes {
        let nonce_le = nonce.to_le_bytes();
        let nonce = packed::Uint32::new_builder()
            .nth0(nonce_le[0].into())
            .nth1(nonce_le[1].into())
            .nth2(nonce_le[2].into())
            .nth3(nonce_le[3].into())
            .build();
        let ping = packed::Ping::new_builder().nonce(nonce).build();
        let payload = packed::PingPayload::new_builder().set(ping).build();
        packed::PingMessage::new_builder()
            .payload(payload)
            .build()
            .as_bytes()
    }

    fn build_pong(nonce: u32) -> Bytes {
        let nonce_le = nonce.to_le_bytes();
        let nonce = packed::Uint32::new_builder()
            .nth0(nonce_le[0].into())
            .nth1(nonce_le[1].into())
            .nth2(nonce_le[2].into())
            .nth3(nonce_le[3].into())
            .build();
        let pong = packed::Pong::new_builder().nonce(nonce).build();
        let payload = packed::PingPayload::new_builder().set(pong).build();
        packed::PingMessage::new_builder()
            .payload(payload)
            .build()
            .as_bytes()
    }

    fn decode(data: &[u8]) -> Option<PingPayload> {
        let reader = packed::PingMessageReader::from_compatible_slice(data).ok()?;
        match reader.payload().to_enum() {
            packed::PingPayloadUnionReader::Ping(reader) => {
                let nonce = {
                    let mut b = [0u8; 4];
                    b.copy_from_slice(reader.nonce().raw_data());
                    u32::from_le_bytes(b)
                };
                Some(PingPayload::Ping(nonce))
            }
            packed::PingPayloadUnionReader::Pong(reader) => {
                let nonce = {
                    let mut b = [0u8; 4];
                    b.copy_from_slice(reader.nonce().raw_data());
                    u32::from_le_bytes(b)
                };
                Some(PingPayload::Pong(nonce))
            }
        }
    }
}

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
