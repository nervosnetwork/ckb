//! TODO(doc): @driftluo
use ckb_logger::debug;
use ckb_notify::NotifyController;
use ckb_types::{packed::Alert, prelude::*};
use lru::LruCache;
use std::collections::HashMap;

const CANCEL_FILTER_SIZE: usize = 128;

/// TODO(doc): @driftluo
pub struct Notifier {
    /// cancelled alerts
    cancel_filter: LruCache<u32, ()>,
    /// alerts we received
    received_alerts: HashMap<u32, Alert>,
    /// alerts that self node should notice
    noticed_alerts: Vec<Alert>,
    client_version: String,
    notify_controller: NotifyController,
}

impl Notifier {
    /// TODO(doc): @driftluo
    pub fn new(client_version: String, notify_controller: NotifyController) -> Self {
        Notifier {
            cancel_filter: LruCache::new(CANCEL_FILTER_SIZE),
            received_alerts: Default::default(),
            noticed_alerts: Vec::new(),
            client_version,
            notify_controller,
        }
    }

    fn is_version_effective(&self, alert: &Alert) -> bool {
        use semver::Version;
        if let Ok(client_version) = Version::parse(&self.client_version) {
            let test_min_ver_failed = alert
                .as_reader()
                .raw()
                .min_version()
                .to_opt()
                .and_then(|v| {
                    v.as_utf8()
                        .ok()
                        .and_then(|v| Version::parse(v).map(|min_v| client_version < min_v).ok())
                        .or(Some(true))
                })
                .unwrap_or(false);
            if test_min_ver_failed {
                return false;
            }
            let test_max_ver_failed = alert
                .as_reader()
                .raw()
                .max_version()
                .to_opt()
                .and_then(|v| {
                    v.as_utf8()
                        .ok()
                        .and_then(|v| Version::parse(v).map(|max_v| client_version > max_v).ok())
                        .or(Some(true))
                })
                .unwrap_or(false);
            if test_max_ver_failed {
                return false;
            }
        }
        true
    }

    /// TODO(doc): @driftluo
    pub fn add(&mut self, alert: &Alert) {
        let alert_id = alert.raw().id().unpack();
        let alert_cancel = alert.raw().cancel().unpack();
        if self.has_received(alert_id) {
            return;
        }
        // checkout cancel_id
        if alert_cancel > 0 {
            self.cancel(alert_cancel);
        }
        // add to received alerts
        self.received_alerts.insert(alert_id, alert.clone());

        // check conditions, figure out do we need to notice this alert
        if !self.is_version_effective(&alert) {
            debug!("received a version ineffective alert {:?}", alert);
            return;
        }

        if self.noticed_alerts.contains(alert) {
            return;
        }
        self.notify_controller.notify_network_alert(alert.clone());
        self.noticed_alerts.push(alert.clone());
        // sort by priority
        self.noticed_alerts.sort_by_key(|a| {
            let priority: u32 = a.raw().priority().unpack();
            std::u32::MAX - priority
        });
    }

    /// TODO(doc): @driftluo
    pub fn cancel(&mut self, cancel_id: u32) {
        self.cancel_filter.put(cancel_id, ());
        self.received_alerts.remove(&cancel_id);
        self.noticed_alerts.retain(|a| {
            let id: u32 = a.raw().id().unpack();
            id != cancel_id
        });
    }

    /// TODO(doc): @driftluo
    pub fn clear_expired_alerts(&mut self, now: u64) {
        self.received_alerts.retain(|_id, alert| {
            let notice_until: u64 = alert.raw().notice_until().unpack();
            notice_until > now
        });
        self.noticed_alerts.retain(|a| {
            let notice_until: u64 = a.raw().notice_until().unpack();
            notice_until > now
        });
    }

    /// TODO(doc): @driftluo
    pub fn has_received(&self, id: u32) -> bool {
        self.received_alerts.contains_key(&id) || self.cancel_filter.contains(&id)
    }

    /// TODO(doc): @driftluo
    // all unexpired alerts
    pub fn received_alerts(&self) -> Vec<Alert> {
        self.received_alerts.values().cloned().collect()
    }

    /// TODO(doc): @driftluo
    // alerts that self node should noticed
    pub fn noticed_alerts(&self) -> Vec<Alert> {
        self.noticed_alerts.clone()
    }
}
