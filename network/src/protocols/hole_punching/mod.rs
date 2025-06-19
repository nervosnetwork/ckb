use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};

use ckb_logger::{debug, error, trace, warn};
use ckb_systemtime::unix_time_as_millis;
use ckb_types::{packed, prelude::*};
use p2p::{
    async_trait, bytes,
    context::{ProtocolContext, ProtocolContextMutRef},
    multiaddr::Multiaddr,
    service::TargetSession,
    traits::ServiceProtocol,
    utils::extract_peer_id,
};

use crate::{
    PeerId, PeerIndex, SupportProtocols, network::NetworkState,
    protocols::hole_punching::status::BAD_MESSAGE_BAN_TIME,
};

mod component;
pub(crate) mod status;

pub(crate) const MAX_HOPS: u8 = 6;
pub(crate) const HOLE_PUNCHING_INTERVAL: u64 = 2 * 60 * 1000; // 2 minutes
const CHECK_INTERVAL: Duration = Duration::from_secs(5 * 60);
const CHECK_TOKEN: u64 = 0;
const ADDRS_COUNT_LIMIT: usize = 24;
const TIMEOUT: u64 = 5 * 60 * 1000; // 5 minutes

type PendingDeliveredInfo = (Vec<Multiaddr>, u64);
type RateLimiter<T> = governor::RateLimiter<
    T,
    governor::state::keyed::HashMapStateStore<T>,
    governor::clock::DefaultClock,
>;

/// Hole Punching Protocol
pub(crate) struct HolePunching {
    network_state: Arc<NetworkState>,
    bind_addr: Option<SocketAddr>,
    // Request timestamp recorded
    inflight_requests: HashMap<PeerId, u64>,
    // Delivered timestamp recorded
    pending_delivered: HashMap<PeerId, PendingDeliveredInfo>,
    rate_limiter: RateLimiter<(PeerIndex, u32)>,
    forward_rate_limiter: RateLimiter<(PeerId, PeerId, u32)>,
}

#[async_trait]
impl ServiceProtocol for HolePunching {
    async fn init(&mut self, context: &mut ProtocolContext) {
        context
            .set_service_notify(context.proto_id, CHECK_INTERVAL, CHECK_TOKEN)
            .await
            .expect("set discovery notify fail")
    }

    async fn connected(&mut self, context: ProtocolContextMutRef<'_>, version: &str) {
        self.network_state.with_peer_registry_mut(|reg| {
            reg.get_peer_mut(context.session.id).map(|peer| {
                peer.protocols.insert(context.proto_id, version.to_owned());
            })
        });
    }

    async fn disconnected(&mut self, context: ProtocolContextMutRef<'_>) {
        self.rate_limiter.retain_recent();
        self.forward_rate_limiter.retain_recent();
        debug!("HolePunching.disconnected session={}", context.session.id);
    }

    async fn received(&mut self, context: ProtocolContextMutRef<'_>, data: bytes::Bytes) {
        let session_id = context.session.id;
        trace!("HolePunching.received session={}", session_id);

        let msg = match packed::HolePunchingMessageReader::from_slice(&data) {
            Ok(msg) => msg.to_enum(),
            _ => {
                warn!(
                    "HolePunching.received a malformed message from {}",
                    session_id
                );
                self.network_state.ban_session(
                    &context.control().clone().into(),
                    session_id,
                    BAD_MESSAGE_BAN_TIME,
                    String::from("send us a malformed message"),
                );
                return;
            }
        };

        let item_name = msg.item_name();

        if self
            .rate_limiter
            .check_key(&(session_id, msg.item_id()))
            .is_err()
        {
            debug!(
                "process {} from {}; result is {}",
                item_name,
                session_id,
                status::StatusCode::TooManyRequests.with_context(msg.item_name())
            );
            return;
        }

        let status = match msg {
            packed::HolePunchingMessageUnionReader::ConnectionRequest(reader) => {
                component::ConnectionRequestProcess::new(
                    reader,
                    self,
                    context.session.id,
                    context.control(),
                    msg.item_id(),
                )
                .execute()
                .await
            }
            packed::HolePunchingMessageUnionReader::ConnectionRequestDelivered(reader) => {
                component::ConnectionRequestDeliveredProcess::new(
                    reader,
                    self,
                    context.control(),
                    context.session.id,
                    self.bind_addr,
                    msg.item_id(),
                )
                .execute()
                .await
            }
            packed::HolePunchingMessageUnionReader::ConnectionSync(reader) => {
                component::ConnectionSyncProcess::new(
                    reader,
                    self,
                    context.control(),
                    self.bind_addr,
                    msg.item_id(),
                )
                .execute()
                .await
            }
        };
        if let Some(ban_time) = status.should_ban() {
            error!(
                "process {} from {}; ban {:?} since result is {}",
                item_name, session_id, ban_time, status
            );
            self.network_state.ban_session(
                &context.control().clone().into(),
                session_id,
                ban_time,
                status.to_string(),
            );
        } else if status.should_warn() {
            warn!(
                "process {} from {}; result is {}",
                item_name, session_id, status
            );
        } else if !status.is_ok() {
            debug!(
                "process {} from {}; result is {}",
                item_name, session_id, status
            );
        }
    }

    async fn notify(&mut self, context: &mut ProtocolContext, _token: u64) {
        let status = self.network_state.connection_status();

        let now = unix_time_as_millis();
        self.pending_delivered
            .retain(|_, (_, t)| (now - *t) < TIMEOUT);
        self.inflight_requests.retain(|_, t| (now - *t) < TIMEOUT);

        if status.non_whitelist_outbound < status.max_outbound && status.total > 0 {
            let target = &self.network_state.required_flags;
            let addrs = self.network_state.with_peer_store_mut(|p| {
                p.fetch_nat_addrs(
                    (status.max_outbound - status.non_whitelist_outbound) as usize,
                    *target,
                )
            });

            let from_peer_id = self.network_state.local_peer_id();
            let listen_addrs = {
                let public_addr = self.network_state.public_addrs(ADDRS_COUNT_LIMIT);
                if public_addr.len() < ADDRS_COUNT_LIMIT {
                    let observed_addrs = self
                        .network_state
                        .observed_addrs(ADDRS_COUNT_LIMIT - public_addr.len());
                    let iter = public_addr
                        .iter()
                        .chain(observed_addrs.iter())
                        .map(Multiaddr::to_vec)
                        .map(|v| packed::Address::new_builder().bytes(v).build());
                    packed::AddressVec::new_builder().extend(iter).build()
                } else {
                    let iter = public_addr
                        .iter()
                        .map(Multiaddr::to_vec)
                        .map(|v| packed::Address::new_builder().bytes(v).build());
                    packed::AddressVec::new_builder().extend(iter).build()
                }
            };

            let mut inflight = Vec::new();
            for i in addrs {
                if let Some(to_peer_id) = extract_peer_id(&i.addr) {
                    let conn_req = {
                        let content = component::init_request(
                            from_peer_id,
                            &to_peer_id,
                            listen_addrs.clone(),
                        );
                        packed::HolePunchingMessage::new_builder()
                            .set(content)
                            .build()
                    };
                    let proto_id = SupportProtocols::HolePunching.protocol_id();

                    // Broadcast to a number of nodes equal to the square root of the total connection count using gossip.
                    let mut total = status.total.isqrt();
                    let _ignore = context
                        .filter_broadcast(
                            TargetSession::Filter(Box::new(move |_| {
                                total = total.saturating_sub(1);
                                total != 0
                            })),
                            proto_id,
                            conn_req.as_bytes(),
                        )
                        .await;
                    inflight.push(to_peer_id);
                }
            }

            let now = unix_time_as_millis();
            for peer_id in inflight {
                self.inflight_requests.insert(peer_id, now);
            }
        }
    }
}

impl HolePunching {
    pub(crate) fn new(network_state: Arc<NetworkState>) -> Self {
        // setup a rate limiter keyed by peer and message type that lets through 30 requests per second
        // current max rps is 10 (CHECK_TOKEN), 30 is a flexible hard cap with buffer
        let quota = governor::Quota::per_second(std::num::NonZeroU32::new(30).unwrap());
        let rate_limiter = RateLimiter::hashmap(quota);

        // In the request forwarding process, the same group of from/to should not be received by the same
        // node more than 1 times within one second.
        let quota = governor::Quota::per_second(std::num::NonZeroU32::new(1).unwrap());
        let forward_rate_limiter = RateLimiter::hashmap(quota);

        Self {
            #[cfg(not(target_os = "linux"))]
            bind_addr: None,
            #[cfg(target_os = "linux")]
            bind_addr: {
                let mut bind_addr = None;
                if network_state.config.reuse_port_on_linux {
                    for multi_addr in &network_state.config.listen_addresses {
                        if let crate::network::TransportType::Tcp =
                            crate::network::find_type(multi_addr)
                        {
                            if let Some(addr) = p2p::utils::multiaddr_to_socketaddr(multi_addr) {
                                bind_addr = Some(addr);
                                break;
                            }
                        }
                    }
                }
                bind_addr
            },
            network_state,
            pending_delivered: HashMap::new(),
            inflight_requests: HashMap::new(),
            rate_limiter,
            forward_rate_limiter,
        }
    }
}
