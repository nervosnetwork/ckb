use std::net::SocketAddr;

use ckb_logger::debug;
use ckb_types::{packed, prelude::*};
use futures::future::select_ok;
use p2p::{
    runtime,
    service::{RawSessionInfo, ServiceAsyncControl},
};

use crate::{
    PeerId,
    protocols::{
        SupportProtocols,
        hole_punching::{
            HolePunching, MAX_HOPS,
            component::{forward_sync, try_nat_traversal},
            status::{Status, StatusCode},
        },
    },
};

struct SyncContent {
    route: Vec<PeerId>,
    from: PeerId,
    to: PeerId,
}

impl TryFrom<&packed::ConnectionSyncReader<'_>> for SyncContent {
    type Error = Status;

    fn try_from(value: &packed::ConnectionSyncReader<'_>) -> Result<Self, Self::Error> {
        let route = value
            .route()
            .iter()
            .map(|id| {
                PeerId::from_bytes(id.raw_data().to_vec()).map_err(|_| {
                    StatusCode::InvalidRoute.with_context("the route peer id is invalid")
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let from = PeerId::from_bytes(value.from().raw_data().to_vec()).map_err(|_| {
            StatusCode::InvalidFromPeerId.with_context("the from peer id is invalid")
        })?;
        let to = PeerId::from_bytes(value.to().raw_data().to_vec())
            .map_err(|_| StatusCode::InvalidToPeerId.with_context("the to peer id is invalid"))?;
        Ok(SyncContent { route, from, to })
    }
}

pub(crate) struct ConnectionSyncProcess<'a> {
    message: packed::ConnectionSyncReader<'a>,
    protocol: &'a HolePunching,
    p2p_control: &'a ServiceAsyncControl,
    bind_addr: Option<SocketAddr>,
    msg_item_id: u32,
}

impl<'a> ConnectionSyncProcess<'a> {
    pub(crate) fn new(
        message: packed::ConnectionSyncReader<'a>,
        protocol: &'a HolePunching,
        p2p_control: &'a ServiceAsyncControl,
        bind_addr: Option<SocketAddr>,
        msg_item_id: u32,
    ) -> Self {
        Self {
            message,
            protocol,
            p2p_control,
            bind_addr,
            msg_item_id,
        }
    }

    pub(crate) async fn execute(self) -> Status {
        let content = match SyncContent::try_from(&self.message) {
            Ok(content) => content,
            Err(status) => return status,
        };

        if content.route.len() > MAX_HOPS as usize {
            return StatusCode::InvalidRoute.with_context("the route length is too long");
        }
        if self
            .protocol
            .forward_rate_limiter
            .check_key(&(content.from.clone(), content.to.clone(), self.msg_item_id))
            .is_err()
        {
            debug!(
                "from: {}, to {}, item_name: {}, rate limit is reached",
                content.from, content.to, "ConnectionSync",
            );
            return StatusCode::TooManyRequests.with_context("ConnectionSync");
        }

        match content.route.last() {
            Some(next_peer_id) => self.forward_sync(next_peer_id).await,
            None => {
                let self_peer_id = self.protocol.network_state.local_peer_id();
                if self_peer_id != &content.to {
                    // forward the message to the `to` peer
                    self.forward_sync(&content.to).await
                } else {
                    // Current node should be the `to` target.
                    if let Some(metrics) = ckb_metrics::handle() {
                        metrics.ckb_hole_punching_passive_count.inc();
                    }

                    let listens_info = self
                        .protocol
                        .pending_delivered
                        .get(&content.from)
                        .map(|info| info.0.clone());

                    match listens_info {
                        Some(listens) => {
                            let tasks = listens
                                .into_iter()
                                .map(|listen_addr| {
                                    Box::pin(try_nat_traversal(self.bind_addr, listen_addr))
                                })
                                .collect::<Vec<_>>();

                            if tasks.is_empty() {
                                return StatusCode::Ignore.with_context("no valid listen address");
                            }

                            debug!(
                                "current peer is the target peer {}, start NAT traversal",
                                content.to
                            );

                            match self
                                .protocol
                                .network_state
                                .config
                                .listen_addresses
                                .first()
                                .cloned()
                            {
                                Some(listen_addr) => {
                                    let control: ServiceAsyncControl = self.p2p_control.clone();
                                    runtime::spawn(async move {
                                        if let Ok(((stream, addr), _)) = select_ok(tasks).await {
                                            debug!("NAT traversal success, addr: {:?}", addr);
                                            if let Some(metrics) = ckb_metrics::handle() {
                                                metrics
                                                    .ckb_hole_punching_passive_success_count
                                                    .inc();
                                            }

                                            let _ignore = control
                                                .raw_session(
                                                    stream,
                                                    addr,
                                                    RawSessionInfo::inbound(listen_addr),
                                                )
                                                .await;
                                        }
                                    });
                                    Status::ok()
                                }
                                None => {
                                    StatusCode::Ignore.with_context("no listen address configured")
                                }
                            }
                        }
                        None => StatusCode::Ignore
                            .with_context("the from peer id is not in the pending list"),
                    }
                }
            }
        }
    }

    async fn forward_sync(&self, peer_id: &PeerId) -> Status {
        let target_sid = self
            .protocol
            .network_state
            .peer_registry
            .read()
            .get_key_by_peer_id(peer_id);

        match target_sid {
            Some(next_peer) => {
                let content = forward_sync(self.message);
                let new_message = packed::HolePunchingMessage::new_builder()
                    .set(content)
                    .build()
                    .as_bytes();
                let proto_id = SupportProtocols::HolePunching.protocol_id();
                debug!(
                    "forward the sync to next peer {} (id: {})",
                    next_peer, peer_id
                );
                if let Err(error) = self
                    .p2p_control
                    .send_message_to(next_peer, proto_id, new_message)
                    .await
                {
                    StatusCode::ForwardError.with_context(error)
                } else {
                    Status::ok()
                }
            }
            None => StatusCode::Ignore.with_context("the next peer in the route is disconnected"),
        }
    }
}
