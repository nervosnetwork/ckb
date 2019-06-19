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
use ckb_logger::{debug, info, trace};
use ckb_network::{CKBProtocolContext, CKBProtocolHandler, PeerIndex, TargetSession};
use ckb_protocol::{get_root, AlertMessage};
use ckb_util::Mutex;
use flatbuffers::FlatBufferBuilder;
use fnv::FnvHashSet;
use lru_cache::LruCache;
use std::convert::TryInto;
use std::sync::Arc;

const KNOWN_LIST_SIZE: usize = 64;

/// AlertRelayer
/// relay alert messages
#[derive(Clone)]
pub struct AlertRelayer {
    notifier: Arc<Mutex<Notifier>>,
    verifier: Arc<Verifier>,
    known_lists: LruCache<PeerIndex, FnvHashSet<u32>>,
}

impl AlertRelayer {
    pub fn new(client_version: String, config: Config) -> Self {
        AlertRelayer {
            notifier: Arc::new(Mutex::new(Notifier::new(client_version))),
            verifier: Arc::new(Verifier::new(config)),
            known_lists: LruCache::new(KNOWN_LIST_SIZE),
        }
    }

    pub fn notifier(&self) -> &Arc<Mutex<Notifier>> {
        &self.notifier
    }

    pub fn verifier(&self) -> &Arc<Verifier> {
        &self.verifier
    }

    fn clear_expired_alerts(&mut self) {
        let now = faketime::unix_time_as_millis();
        self.notifier.lock().clear_expired_alerts(now);
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
        for alert in self.notifier.lock().received_alerts() {
            trace!("send alert {} to peer {}", alert.id, peer_index);
            let fbb = &mut FlatBufferBuilder::new();
            let msg = AlertMessage::build_alert(fbb, &alert);
            fbb.finish(msg, None);
            if let Err(err) = nc.quick_send_message_to(peer_index, fbb.finished_data().into()) {
                debug!("alert_relayer send alert when connected error: {:?}", err);
            }
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
                info!("Peer {} sends us malformed message", peer_index);
                nc.ban_peer(peer_index, BAD_MESSAGE_BAN_TIME);
                return;
            }
        };
        trace!("receive alert {} from peer {}", alert.id, peer_index);
        // ignore alert
        if self.notifier.lock().has_received(alert.id) {
            return;
        }
        // verify
        if let Err(err) = self.verifier.verify_signatures(&alert) {
            debug!(
                "Peer {} sends us a alert with invalid signatures, error {:?}",
                peer_index, err
            );
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
        if let Err(err) = nc.quick_filter_broadcast(TargetSession::Multi(selected_peers), data) {
            debug!("alert broadcast error: {:?}", err);
        }
        // add to received alerts
        self.notifier.lock().add(alert);
    }
}
