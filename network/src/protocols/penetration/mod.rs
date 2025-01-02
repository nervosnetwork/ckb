use std::{collections::HashMap, sync::Arc};

use ckb_logger::{debug, error, trace, warn};
use ckb_systemtime::{Duration, Instant};
use ckb_types::{packed, prelude::*};
use ckb_util::RwLock;
use p2p::{async_trait, bytes::Bytes, service::ServiceControl};

use crate::{
    protocols::{ProtocolContext, ProtocolContextMutRef},
    NetworkState, PeerId, PeerIndex, ServiceProtocol,
};

mod components;
mod status;

use status::{Status, StatusCode};

const BAD_MESSAGE_BAN_TIME: Duration = Duration::from_secs(5 * 60);
pub(crate) const PENETRATED_INTERVAL: Duration = Duration::from_secs(2 * 60);
pub(crate) const MAX_TTL: u8 = 6;
pub(crate) const ADDRS_COUNT_LIMIT: usize = 20;

/// Penetration Protocol
pub(crate) struct Penetration {
    network_state: Arc<NetworkState>,
    from_addrs: RwLock<HashMap<PeerId, Instant>>,
}

#[async_trait]
impl ServiceProtocol for Penetration {
    async fn init(&mut self, _context: &mut ProtocolContext) {}

    async fn connected(&mut self, context: ProtocolContextMutRef<'_>, version: &str) {
        debug!(
            "Penetration({}).connected session={}",
            version, context.session.id
        );
    }

    async fn disconnected(&mut self, context: ProtocolContextMutRef<'_>) {
        debug!("Penetration.disconnected session={}", context.session.id);
    }

    async fn received(&mut self, context: ProtocolContextMutRef<'_>, data: Bytes) {
        let session_id = context.session.id;
        trace!("Penetration.received session={}", session_id);

        let msg = match packed::PenetrationMessageReader::from_slice(&data) {
            Ok(msg) => msg.to_enum(),
            _ => {
                warn!(
                    "Penetration.received a malformed message from {}",
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
        let status = self.try_process(&context.control().clone().into(), session_id, msg);
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
}

impl Penetration {
    pub(crate) fn new(network_state: Arc<NetworkState>) -> Self {
        Self {
            network_state,
            from_addrs: RwLock::new(HashMap::default()),
        }
    }

    fn try_process(
        &mut self,
        p2p_control: &ServiceControl,
        peer: PeerIndex,
        message: packed::PenetrationMessageUnionReader<'_>,
    ) -> Status {
        match message {
            packed::PenetrationMessageUnionReader::ConnectionRequest(reader) => {
                components::ConnectionRequestProcess::new(reader, self, peer, p2p_control).execute()
            }
            packed::PenetrationMessageUnionReader::ConnectionRequestDelivered(reader) => {
                components::ConnectionRequestDeliveredProcess::new(reader, self, p2p_control)
                    .execute()
            }
        }
    }
}
