use crate::config::NotifierConfig;
use ckb_core::alert::Alert;
use ckb_logger::{error, info, warn};
use fnv::FnvHashMap;
use lru_cache::LruCache;
use std::process::Command;
use std::sync::Arc;

const CANCEL_FILTER_SIZE: usize = 128;

pub struct Notifier {
    /// cancelled alerts
    cancel_filter: LruCache<u32, ()>,
    /// alerts we received
    received_alerts: FnvHashMap<u32, Arc<Alert>>,
    /// alerts that self node should notice
    noticed_alerts: Vec<Arc<Alert>>,
    client_version: String,
    config: NotifierConfig,
}

impl Notifier {
    pub fn new(client_version: String, config: NotifierConfig) -> Self {
        Notifier {
            cancel_filter: LruCache::new(CANCEL_FILTER_SIZE),
            received_alerts: Default::default(),
            noticed_alerts: Vec::new(),
            client_version,
            config,
        }
    }

    pub fn add(&mut self, alert: Arc<Alert>) {
        use semver::Version;

        if self.has_received(alert.id) {
            return;
        }
        // checkout cancel_id
        if alert.cancel > 0 {
            self.cancel(alert.cancel);
        }
        // add to received alerts
        self.received_alerts.insert(alert.id, Arc::clone(&alert));

        // check conditions, figure out do we need to notice this alert
        if alert
            .min_version
            .as_ref()
            .map(|min_v| Version::parse(&self.client_version) < Version::parse(min_v))
            == Some(true)
        {
            return;
        }

        if alert
            .max_version
            .as_ref()
            .map(|max_v| Version::parse(&self.client_version) > Version::parse(max_v))
            == Some(true)
        {
            return;
        }
        if self.noticed_alerts.contains(&alert) {
            return;
        }
        self.notify(&alert);
        self.noticed_alerts.push(alert);
        // sort by priority
        self.noticed_alerts
            .sort_by_key(|a| std::u32::MAX - a.priority);
    }

    fn notify(&self, alert: &Alert) {
        warn!("receive a new alert: {}", alert.message);
        if let Some(notify_script) = self.config.notify_script.as_ref() {
            match Command::new(notify_script)
                .args(&[alert.message.to_owned()])
                .status()
            {
                Ok(exit_status) => {
                    info!("send alert to notify script. {}", exit_status);
                }
                Err(err) => {
                    error!("failed to run notify script: {}", err);
                }
            }
        }
    }

    pub fn cancel(&mut self, cancel_id: u32) {
        self.cancel_filter.insert(cancel_id, ());
        self.received_alerts.remove(&cancel_id);
        self.noticed_alerts.retain(|a| a.id != cancel_id);
    }

    pub fn clear_expired_alerts(&mut self, now: u64) {
        self.received_alerts
            .retain(|_id, alert| alert.notice_until > now);
        self.noticed_alerts.retain(|a| a.notice_until > now);
    }

    pub fn has_received(&self, id: u32) -> bool {
        self.received_alerts.contains_key(&id) || self.cancel_filter.contains_key(&id)
    }

    // all unexpired alerts
    pub fn received_alerts(&self) -> Vec<Arc<Alert>> {
        self.received_alerts.values().cloned().collect()
    }

    // alerts that self node should noticed
    pub fn noticed_alerts(&self) -> Vec<Arc<Alert>> {
        self.noticed_alerts.clone()
    }
}
