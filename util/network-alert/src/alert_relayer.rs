//! AlertRelayer
//! We implement a Bitcoin like alert system, n of m alert key holders can decide to send alert
//messages to all client
//! to leave a space to reach consensus offline under critical bugs
//!
//! A cli to generate alert message,
//! A config option to set alert messages to broadcast.
//
use crate::BAD_MESSAGE_BAN_TIME;
use crate::notifier::Notifier;
use crate::verifier::Verifier;
use ckb_app_config::NetworkAlertConfig;
use ckb_logger::{debug, info, trace};
use ckb_network::{
    CKBProtocolContext, CKBProtocolHandler, PeerIndex, TargetSession, async_trait, bytes::Bytes,
};
use ckb_notify::NotifyController;
use ckb_types::{packed, prelude::*};
use ckb_util::Mutex;
use lru::LruCache;
use std::collections::HashSet;
use std::sync::Arc;

const KNOWN_LIST_SIZE: usize = 64;

/// AlertRelayer
/// relay alert messages
pub struct AlertRelayer {
    notifier: Arc<Mutex<Notifier>>,
    verifier: Arc<Verifier>,
    known_lists: LruCache<PeerIndex, HashSet<u32>>,
}

impl AlertRelayer {
    /// Init
    pub fn new(
        client_version: String,
        notify_controller: NotifyController,
        signature_config: NetworkAlertConfig,
    ) -> Self {
        AlertRelayer {
            notifier: Arc::new(Mutex::new(Notifier::new(client_version, notify_controller))),
            verifier: Arc::new(Verifier::new(signature_config)),
            known_lists: LruCache::new(KNOWN_LIST_SIZE),
        }
    }

    /// Get notifier
    pub fn notifier(&self) -> &Arc<Mutex<Notifier>> {
        &self.notifier
    }

    /// Get verifier
    pub fn verifier(&self) -> &Arc<Verifier> {
        &self.verifier
    }

    fn clear_expired_alerts(&mut self) {
        let now = ckb_systemtime::unix_time_as_millis();
        self.notifier.lock().clear_expired_alerts(now);
    }

    // return true if it this first time the peer know this alert
    fn mark_as_known(&mut self, peer: PeerIndex, alert_id: u32) -> bool {
        match self.known_lists.get_mut(&peer) {
            Some(alert_ids) => alert_ids.insert(alert_id),
            None => {
                let mut alert_ids = HashSet::new();
                alert_ids.insert(alert_id);
                self.known_lists.put(peer, alert_ids);
                true
            }
        }
    }
}

#[async_trait]
impl CKBProtocolHandler for AlertRelayer {
    async fn init(&mut self, _nc: Arc<dyn CKBProtocolContext + Sync>) {}

    async fn connected(
        &mut self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer_index: PeerIndex,
        _version: &str,
    ) {
        self.clear_expired_alerts();
        for alert in self.notifier.lock().received_alerts() {
            let alert_id: u32 = alert.as_reader().raw().id().unpack();
            trace!("Send alert {} to peer {}", alert_id, peer_index);
            if let Err(err) = nc.quick_send_message_to(peer_index, alert.as_bytes()) {
                debug!("alert_relayer send alert when connected error: {:?}", err);
            }
        }
    }

    #[allow(clippy::needless_collect)]
    async fn received(
        &mut self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer_index: PeerIndex,
        data: Bytes,
    ) {
        let alert: packed::Alert = match packed::AlertReader::from_slice(&data) {
            Ok(alert) => {
                if alert.raw().message().is_utf8()
                    && alert
                        .raw()
                        .min_version()
                        .to_opt()
                        .map(|x| x.is_utf8())
                        .unwrap_or(true)
                    && alert
                        .raw()
                        .max_version()
                        .to_opt()
                        .map(|x| x.is_utf8())
                        .unwrap_or(true)
                {
                    alert.to_entity()
                } else {
                    info!(
                        "A malformed message fromP peer {} : not utf-8 string",
                        peer_index
                    );
                    nc.ban_peer(
                        peer_index,
                        BAD_MESSAGE_BAN_TIME,
                        String::from("send us a malformed message: not utf-8 string"),
                    );
                    return;
                }
            }
            Err(err) => {
                info!("A malformed message from peer {}: {:?}", peer_index, err);
                nc.ban_peer(
                    peer_index,
                    BAD_MESSAGE_BAN_TIME,
                    String::from("send us a malformed message"),
                );
                return;
            }
        };
        let alert_id = alert.as_reader().raw().id().unpack();
        trace!("ReceiveD alert {} from peer {}", alert_id, peer_index);
        // ignore alert
        if self.notifier.lock().has_received(alert_id) {
            return;
        }
        // verify
        if let Err(err) = self.verifier.verify_signatures(&alert) {
            debug!(
                "An alert from peer {} with invalid signatures, error {:?}",
                peer_index, err
            );
            nc.ban_peer(
                peer_index,
                BAD_MESSAGE_BAN_TIME,
                String::from("send us an alert with invalid signatures"),
            );
            return;
        }
        // mark sender as known
        self.mark_as_known(peer_index, alert_id);
        // broadcast message
        let selected_peers: Vec<PeerIndex> = nc
            .connected_peers()
            .into_iter()
            .filter(|peer| self.mark_as_known(*peer, alert_id))
            .collect();
        if let Err(err) = nc.quick_filter_broadcast(
            TargetSession::Multi(Box::new(selected_peers.into_iter())),
            data,
        ) {
            debug!("alert broadcast error: {:?}", err);
        }
        // add to received alerts
        self.notifier.lock().add(&alert);
    }
}
