use ckb_core::alert::Alert;
use ckb_util::Mutex;
use log::warn;
use std::sync::Arc;

pub struct Notifier {
    alerts: Mutex<Vec<Arc<Alert>>>,
    client_version: String,
}

impl Notifier {
    pub fn new(client_version: String) -> Self {
        Notifier {
            alerts: Mutex::new(Vec::new()),
            client_version,
        }
    }

    pub fn add(&self, alert: Arc<Alert>) {
        if alert
            .min_version
            .as_ref()
            .map(|min_v| self.client_version < *min_v)
            == Some(true)
        {
            return;
        }

        if alert
            .max_version
            .as_ref()
            .map(|max_v| self.client_version > *max_v)
            == Some(true)
        {
            return;
        }
        let mut alerts = self.alerts.lock();
        if alerts.contains(&alert) {
            return;
        }
        warn!(target: "alert", "receive a new alert: {}", alert.message);
        alerts.push(alert);
    }

    pub fn cancel(&self, id: u32) {
        let mut alerts = self.alerts.lock();
        alerts.retain(|a| a.id == id);
    }

    pub fn clear_expired_alerts(&self, now: u64) {
        let mut alerts = self.alerts.lock();
        alerts.retain(|a| a.notice_until > now);
    }

    pub fn alerts(&self) -> Vec<Arc<Alert>> {
        self.alerts.lock().clone()
    }
}
