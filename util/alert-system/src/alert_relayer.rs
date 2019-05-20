//! AlertRelayer
//! We implment a Bitcoin like alert system, n of m alert key holders can decide to send alert
//messages to all client
//! to leave a space to reach consensus offline under critical bugs
//!
//! A cli to generate alert message,
//! A config option to set alert messages to broard cast.
//
use crate::config::Config;
use crate::notifier::Notifier;
use crate::verifier::Verifier;
use crate::BAD_MESSAGE_BAN_TIME;
use ckb_core::alert::Alert;
use ckb_network::{CKBProtocolContext, CKBProtocolHandler, PeerIndex, TargetSession};
use ckb_protocol::{get_root, AlertMessage};
use flatbuffers::FlatBufferBuilder;
use fnv::{FnvHashMap, FnvHashSet};
use log::{debug, info, trace};
use lru_cache::LruCache;
use std::convert::TryInto;
use std::sync::Arc;

const CANCEL_FILTER_SIZE: usize = 128;
const KNOWN_LIST_SIZE: usize = 64;

/// AlertRelayer
/// relay alert messages
#[derive(Clone)]
pub struct AlertRelayer {
    /// cancelled alerts
    cancel_filter: LruCache<u32, ()>,
    /// unexpired alerts we received
    received_alerts: FnvHashMap<u32, Arc<Alert>>,
    /// alerts that self node should notice
    notifier: Arc<Notifier>,
    verifier: Arc<Verifier>,
    known_lists: LruCache<PeerIndex, FnvHashSet<u32>>,
}

impl AlertRelayer {
    pub fn new(client_version: String, config: Config) -> Self {
        AlertRelayer {
            cancel_filter: LruCache::new(CANCEL_FILTER_SIZE),
            received_alerts: Default::default(),
            notifier: Arc::new(Notifier::new(client_version)),
            verifier: Arc::new(Verifier::new(config)),
            known_lists: LruCache::new(KNOWN_LIST_SIZE),
        }
    }

    pub fn notifier(&self) -> Arc<Notifier> {
        Arc::clone(&self.notifier)
    }

    pub fn verifier(&self) -> Arc<Verifier> {
        Arc::clone(&self.verifier)
    }

    fn receive_new_alert(&mut self, alert: Arc<Alert>) {
        // checkout cancel_id
        if alert.cancel > 0 {
            self.cancel_filter.insert(alert.cancel, ());
            self.received_alerts.remove(&alert.cancel);
            self.notifier.cancel(alert.cancel);
        }
        // add to received alerts
        self.received_alerts.insert(alert.id, Arc::clone(&alert));
        // set self node notice
        self.notifier.add(alert);
    }

    fn clear_expired_alerts(&mut self) {
        let now = faketime::unix_time_as_millis();
        self.received_alerts
            .retain(|_id, alert| alert.notice_until > now);
        self.notifier.clear_expired_alerts(now);
    }

    // return true if it this first time the peer know this alert
    fn mark_as_known(&mut self, peer: PeerIndex, alert_id: u32) -> bool {
        match self.known_lists.get_refresh(&peer) {
            Some(alert_ids) => alert_ids.insert(alert_id),
            None => {
                let mut alert_ids = FnvHashSet::default();
                alert_ids.insert(alert_id);
                self.known_lists.insert(peer, alert_ids);
                true
            }
        }
    }
}

impl CKBProtocolHandler for AlertRelayer {
    fn init(&mut self, _nc: Arc<dyn CKBProtocolContext + Sync>) {}

    fn connected(
        &mut self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer_index: PeerIndex,
        _version: &str,
    ) {
        self.clear_expired_alerts();
        for alert in self.received_alerts.values() {
            trace!(target: "alert", "send alert {} to peer {}", alert.id, peer_index);
            let fbb = &mut FlatBufferBuilder::new();
            let msg = AlertMessage::build_alert(fbb, &alert);
            fbb.finish(msg, None);
            nc.quick_send_message_to(peer_index, fbb.finished_data().into());
        }
    }

    fn received(
        &mut self,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer_index: PeerIndex,
        data: bytes::Bytes,
    ) {
        let alert: Arc<Alert> = match get_root::<AlertMessage>(&data)
            .ok()
            .and_then(|m| m.payload())
            .map(TryInto::try_into)
        {
            Some(Ok(alert)) => Arc::new(alert),
            Some(Err(_)) | None => {
                info!(target: "network", "Peer {} sends us malformed message", peer_index);
                nc.ban_peer(peer_index, BAD_MESSAGE_BAN_TIME);
                return;
            }
        };
        trace!(target: "alert", "receive alert {} from peer {}", alert.id, peer_index);
        // ignore alert
        if self.received_alerts.contains_key(&alert.id)
            || self.cancel_filter.contains_key(&alert.id)
        {
            return;
        }
        // verify
        if let Err(err) = self.verifier.verify_signatures(&alert) {
            debug!(target: "network", "Peer {} sends us a alert with invalid signatures, error {:?}", peer_index, err);
            nc.ban_peer(peer_index, BAD_MESSAGE_BAN_TIME);
            return;
        }
        // mark sender as known
        self.mark_as_known(peer_index, alert.id);
        // broadcast message
        let fbb = &mut FlatBufferBuilder::new();
        let msg = AlertMessage::build_alert(fbb, &alert);
        fbb.finish(msg, None);
        let data = fbb.finished_data().into();
        let selected_peers: Vec<PeerIndex> = nc
            .connected_peers()
            .into_iter()
            .filter(|peer| self.mark_as_known(*peer, alert.id))
            .collect();
        nc.quick_filter_broadcast(TargetSession::Multi(selected_peers), data);
        // add to received alerts
        self.receive_new_alert(alert);
    }
}
