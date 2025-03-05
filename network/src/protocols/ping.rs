use crate::NetworkState;
use crate::network::async_disconnect_with_message;
use ckb_logger::{debug, error, trace, warn};
use ckb_types::{packed, prelude::*};
use futures::{
    channel::mpsc::{Receiver, Sender, channel},
    prelude::*,
};
use p2p::{
    SessionId, async_trait,
    bytes::Bytes,
    context::{ProtocolContext, ProtocolContextMutRef},
    service::TargetSession,
    traits::ServiceProtocol,
};
use std::{
    collections::{HashMap, HashSet},
    str,
    sync::Arc,
};

use ckb_systemtime::{Duration, Instant};

const SEND_PING_TOKEN: u64 = 0;
const CHECK_TIMEOUT_TOKEN: u64 = 1;
const CONTROL_CHANNEL_BUFFER_SIZE: usize = 2;

/// Ping protocol handler.
///
/// The interval means that we send ping to peers.
/// The timeout means that consider peer is timeout if during a timeout we still have not received pong from a peer
pub struct PingHandler {
    interval: Duration,
    timeout: Duration,
    connected_session_ids: HashMap<SessionId, PingStatus>,
    network_state: Arc<NetworkState>,
    control_receiver: Receiver<()>,
    start_time: Instant,
}

impl PingHandler {
    pub fn new(
        interval: Duration,
        timeout: Duration,
        network_state: Arc<NetworkState>,
    ) -> (PingHandler, Sender<()>) {
        let (control_sender, control_receiver) = channel(CONTROL_CHANNEL_BUFFER_SIZE);
        let now = Instant::now();
        (
            PingHandler {
                interval,
                timeout,
                connected_session_ids: Default::default(),
                network_state,
                control_receiver,
                start_time: now,
            },
            control_sender,
        )
    }

    fn ping_received(&mut self, id: SessionId) {
        trace!("received ping from: {:?}", id);
        self.network_state.with_peer_registry_mut(|reg| {
            if let Some(peer) = reg.get_peer_mut(id) {
                peer.last_ping_protocol_message_received_at = Some(Instant::now());
            }
        });
    }

    fn pong_received(&mut self, id: SessionId, last_ping: Instant) {
        let now = Instant::now();
        self.network_state.with_peer_registry_mut(|reg| {
            if let Some(peer) = reg.get_peer_mut(id) {
                peer.ping_rtt = Some(now.saturating_duration_since(last_ping));
                peer.last_ping_protocol_message_received_at = Some(now);
            }
        });
    }

    async fn ping_peers(&mut self, context: &ProtocolContext) {
        let now = Instant::now();
        let send_nonce = nonce(&now, self.start_time);
        let peers: HashSet<SessionId> = self
            .connected_session_ids
            .iter_mut()
            .filter_map(|(session_id, ps)| {
                if ps.processing {
                    None
                } else {
                    ps.processing = true;
                    ps.last_ping_sent_at = now;
                    ps.nonce = send_nonce;
                    Some(*session_id)
                }
            })
            .collect();
        if !peers.is_empty() {
            debug!("start ping peers: {:?}", peers);
            let ping_msg = PingMessage::build_ping(send_nonce);
            let proto_id = context.proto_id;
            if context
                .filter_broadcast(
                    TargetSession::Multi(Box::new(peers.into_iter())),
                    proto_id,
                    ping_msg,
                )
                .await
                .is_err()
            {
                debug!("Failed to send message");
            }
        }
    }
}

fn nonce(t: &Instant, start_time: Instant) -> u32 {
    t.saturating_duration_since(start_time).as_secs() as u32
}

/// PingStatus of a peer
#[derive(Clone, Debug)]
struct PingStatus {
    /// Are we currently pinging this peer?
    processing: bool,
    /// The time we last send ping to this peer.
    last_ping_sent_at: Instant,
    nonce: u32,
}

impl PingStatus {
    /// A meaningless value, peer must send a pong has same nonce to respond a ping.
    fn nonce(&self) -> u32 {
        self.nonce
    }

    /// Time duration since we last send ping.
    fn elapsed(&self) -> Duration {
        Instant::now().saturating_duration_since(self.last_ping_sent_at)
    }
}

#[async_trait]
impl ServiceProtocol for PingHandler {
    async fn init(&mut self, context: &mut ProtocolContext) {
        // periodicly send ping to peers
        let proto_id = context.proto_id;
        if context
            .set_service_notify(proto_id, self.interval, SEND_PING_TOKEN)
            .await
            .is_err()
        {
            warn!("Failed to start ping");
        }
        if context
            .set_service_notify(proto_id, self.timeout, CHECK_TIMEOUT_TOKEN)
            .await
            .is_err()
        {
            warn!("Failed to start ping");
        }
    }

    async fn connected(&mut self, context: ProtocolContextMutRef<'_>, version: &str) {
        let session = context.session;
        self.connected_session_ids
            .entry(session.id)
            .or_insert_with(|| PingStatus {
                last_ping_sent_at: Instant::now(),
                processing: false,
                nonce: 0,
            });
        debug!(
            "proto id [{}] open on session [{}], address: [{}], type: [{:?}], version: {}",
            context.proto_id, session.id, session.address, session.ty, version
        );
        debug!("Connected sessions are: {:?}", self.connected_session_ids);
        // Register open ping protocol
        self.network_state.with_peer_registry_mut(|reg| {
            reg.get_peer_mut(session.id).map(|peer| {
                peer.protocols.insert(context.proto_id, version.to_owned());
            })
        });
    }

    async fn disconnected(&mut self, context: ProtocolContextMutRef<'_>) {
        let session = context.session;
        self.connected_session_ids.remove(&session.id);
        // remove registered ping protocol
        self.network_state.with_peer_registry_mut(|reg| {
            let _ = reg.get_peer_mut(session.id).map(|peer| {
                peer.protocols.remove(&context.proto_id);
            });
        });
        debug!(
            "Proto id [{}] closed on session [{}]",
            context.proto_id, session.id
        );
    }

    async fn received(&mut self, context: ProtocolContextMutRef<'_>, data: Bytes) {
        let session = context.session;
        match PingMessage::decode(data.as_ref()) {
            None => {
                error!("Message decode error");
                if let Err(err) =
                    async_disconnect_with_message(context.control(), session.id, "ping failed")
                        .await
                {
                    debug!("Disconnect failed {:?}, error: {:?}", session.id, err);
                }
            }
            Some(msg) => {
                match msg {
                    PingPayload::Ping(nonce) => {
                        self.ping_received(session.id);
                        if context
                            .send_message(PingMessage::build_pong(nonce))
                            .await
                            .is_err()
                        {
                            debug!("Failed to send message");
                        }
                    }
                    PingPayload::Pong(nonce) => {
                        // check pong
                        if let Some(status) = self.connected_session_ids.get_mut(&session.id) {
                            if (true, nonce) == (status.processing, status.nonce()) {
                                status.processing = false;
                                let last_ping_sent_at = status.last_ping_sent_at;
                                self.pong_received(session.id, last_ping_sent_at);
                                return;
                            }
                        }
                        // if nonce is incorrect or can't find ping info
                        if let Err(err) = async_disconnect_with_message(
                            context.control(),
                            session.id,
                            "ping failed",
                        )
                        .await
                        {
                            debug!("Disconnect failed {:?}, error: {:?}", session.id, err);
                        }
                    }
                }
            }
        }
    }

    async fn notify(&mut self, context: &mut ProtocolContext, token: u64) {
        match token {
            SEND_PING_TOKEN => self.ping_peers(context).await,
            CHECK_TIMEOUT_TOKEN => {
                let timeout = self.timeout;
                for (id, _ps) in self
                    .connected_session_ids
                    .iter()
                    .filter(|(_id, ps)| ps.processing && ps.elapsed() >= timeout)
                {
                    debug!("Ping timeout, {:?}", id);
                    if let Err(err) =
                        async_disconnect_with_message(context.control(), *id, "ping timeout").await
                    {
                        debug!("Disconnect failed {:?}, error: {:?}", id, err);
                    }
                }
            }
            _ => panic!("unknown token {token}"),
        }
    }

    async fn poll(&mut self, context: &mut ProtocolContext) -> Option<()> {
        if self.control_receiver.next().await.is_some() {
            self.ping_peers(context).await;
            Some(())
        } else {
            None
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
