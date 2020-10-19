use crate::network::disconnect_with_message;
use crate::NetworkState;
use ckb_logger::{debug, error, trace, warn};
use ckb_types::{packed, prelude::*};
use futures::{
    channel::mpsc::{channel, Receiver, Sender},
    prelude::*,
};
use p2p::{
    bytes::Bytes,
    context::{ProtocolContext, ProtocolContextMutRef},
    service::TargetSession,
    traits::ServiceProtocol,
    SessionId,
};
use std::{
    collections::HashMap,
    pin::Pin,
    str,
    sync::Arc,
    task::{Context, Poll},
    time::Instant,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

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
}

impl PingHandler {
    pub fn new(
        interval: Duration,
        timeout: Duration,
        network_state: Arc<NetworkState>,
    ) -> (PingHandler, Sender<()>) {
        let (control_sender, control_receiver) = channel(CONTROL_CHANNEL_BUFFER_SIZE);

        (
            PingHandler {
                interval,
                timeout,
                connected_session_ids: Default::default(),
                network_state,
                control_receiver,
            },
            control_sender,
        )
    }

    fn ping_received(&self, id: SessionId) {
        trace!("received ping from: {:?}", id);
        self.mark_time(id, None);
    }

    fn ping_peers(&mut self, context: &ProtocolContext) {
        let now = SystemTime::now();
        let peers: Vec<SessionId> = self
            .connected_session_ids
            .iter_mut()
            .filter_map(|(session_id, ps)| {
                if ps.processing {
                    None
                } else {
                    ps.processing = true;
                    ps.last_ping = now;
                    Some(*session_id)
                }
            })
            .collect();
        if !peers.is_empty() {
            debug!("start ping peers: {:?}", peers);
            let ping_msg = PingMessage::build_ping(nonce(&now));
            let proto_id = context.proto_id;
            if context
                .filter_broadcast(TargetSession::Multi(peers), proto_id, ping_msg)
                .is_err()
            {
                debug!("send message fail");
            }
        }
    }

    fn mark_time(&self, id: SessionId, ping_time: Option<Duration>) {
        self.network_state.with_peer_registry_mut(|reg| {
            if let Some(mut peer) = reg.get_peer_mut(id) {
                if ping_time.is_some() {
                    peer.ping = ping_time;
                }
                peer.last_message_time = Some(Instant::now());
            }
        });
    }
}

fn nonce(t: &SystemTime) -> u32 {
    t.duration_since(UNIX_EPOCH)
        .map(|dur| dur.as_secs())
        .unwrap_or_default() as u32
}

/// PingStatus of a peer
#[derive(Clone, Debug)]
struct PingStatus {
    /// Are we currently pinging this peer?
    processing: bool,
    /// The time we last send ping to this peer.
    last_ping: SystemTime,
}

impl PingStatus {
    /// A meaningless value, peer must send a pong has same nonce to respond a ping.
    fn nonce(&self) -> u32 {
        nonce(&self.last_ping)
    }

    /// Time duration since we last send ping.
    fn elapsed(&self) -> Duration {
        self.last_ping.elapsed().unwrap_or_default()
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
            Some(_) => {
                self.connected_session_ids
                    .entry(session.id)
                    .or_insert_with(|| PingStatus {
                        last_ping: SystemTime::now(),
                        processing: false,
                    });
                debug!(
                    "proto id [{}] open on session [{}], address: [{}], type: [{:?}], version: {}",
                    context.proto_id, session.id, session.address, session.ty, version
                );
                debug!("connected sessions are: {:?}", self.connected_session_ids);
                // Register open ping protocol
                self.network_state.with_peer_registry_mut(|reg| {
                    reg.get_peer_mut(session.id).map(|peer| {
                        peer.protocols.insert(context.proto_id, version.to_owned());
                    })
                });
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
        // remove registered ping protocol
        self.network_state.with_peer_registry_mut(|reg| {
            let _ = reg.get_peer_mut(session.id).map(|peer| {
                peer.protocols.remove(&context.proto_id);
            });
        });
        debug!(
            "proto id [{}] close on session [{}]",
            context.proto_id, session.id
        );
    }

    fn received(&mut self, context: ProtocolContextMutRef, data: Bytes) {
        let session = context.session;
        match PingMessage::decode(data.as_ref()) {
            None => {
                error!("decode message error");
                if let Err(err) =
                    disconnect_with_message(context.control(), session.id, "ping failed")
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
                            .is_err()
                        {
                            debug!("send message fail");
                        }
                    }
                    PingPayload::Pong(nonce) => {
                        // check pong
                        if let Some(status) = self.connected_session_ids.get_mut(&session.id) {
                            if (true, nonce) == (status.processing, status.nonce()) {
                                status.processing = false;
                                let ping_time = status.elapsed();
                                self.mark_time(session.id, Some(ping_time));
                                return;
                            }
                        }
                        // if nonce is incorrect or can't find ping info
                        if let Err(err) =
                            disconnect_with_message(context.control(), session.id, "ping failed")
                        {
                            debug!("Disconnect failed {:?}, error: {:?}", session.id, err);
                        }
                    }
                }
            }
        }
    }

    fn notify(&mut self, context: &mut ProtocolContext, token: u64) {
        match token {
            SEND_PING_TOKEN => self.ping_peers(context),
            CHECK_TIMEOUT_TOKEN => {
                let timeout = self.timeout;
                for (id, _ps) in self
                    .connected_session_ids
                    .iter()
                    .filter(|(_id, ps)| ps.processing && ps.elapsed() >= timeout)
                {
                    debug!("ping timeout, {:?}", id);
                    if let Err(err) =
                        disconnect_with_message(context.control(), *id, "ping timeout")
                    {
                        debug!("Disconnect failed {:?}, error: {:?}", id, err);
                    }
                }
            }
            _ => panic!("unknown token {}", token),
        }
    }

    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
        context: &mut ProtocolContext,
    ) -> Poll<Option<()>> {
        self.control_receiver
            .poll_next_unpin(cx)
            .map(|control_message| {
                control_message.map(|_| {
                    self.ping_peers(context);
                })
            })
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
