//! Block template HTTP/script notification.

use crate::block_assembler::BlockAssembler;
use http_body_util::Full;
use hyper::{Method, Request, header::HeaderValue};
use std::{sync::Arc, time::Duration};
use tokio::process::Command;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::timeout;

struct NotifyScript {
    command: Arc<str>,
    slot: Arc<Semaphore>,
}

impl NotifyScript {
    fn try_claim(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.slot).try_acquire_owned().ok()
    }
}

/// Process-lifecycle boundary for configured block-template scripts.
///
/// Notifications are observational and may be coalesced while the previous
/// invocation of the same command is still running. This makes the maximum
/// live child count a startup-configured constant instead of a function of
/// transaction/template arrival rate.
pub(super) struct NotifyScriptRunner {
    scripts: Box<[NotifyScript]>,
}

impl NotifyScriptRunner {
    pub(super) fn new(commands: &[String]) -> Self {
        let scripts = commands
            .iter()
            .map(|command| NotifyScript {
                command: Arc::from(command.as_str()),
                slot: Arc::new(Semaphore::new(1)),
            })
            .collect();
        Self { scripts }
    }

    fn notify(&self, template_json: &str, notify_timeout: Duration) {
        for script in &self.scripts {
            let Some(permit) = script.try_claim() else {
                ckb_logger::debug!(
                    "block assembler notification script {} is still running; coalescing update",
                    script.command
                );
                continue;
            };
            let command = Arc::clone(&script.command);
            let template_json = template_json.to_owned();
            tokio::spawn(async move {
                let _permit = permit;
                let mut child = Command::new(command.as_ref());
                child.arg(template_json).kill_on_drop(true);
                match timeout(notify_timeout, child.status()).await {
                    Ok(Ok(status)) => {
                        ckb_logger::debug!("the command exited with: {status}")
                    }
                    Ok(Err(error)) => {
                        ckb_logger::error!("the script {} failed to spawn: {error}", command)
                    }
                    Err(_) => ckb_logger::warn!(
                        "block assembler notifying {} timed out and was terminated",
                        command
                    ),
                }
            });
        }
    }
}

impl BlockAssembler {
    pub(crate) async fn notify(&self) {
        #[cfg(test)]
        self.notify_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if !self.need_to_notify() {
            return;
        }
        let template = self.get_current().await;
        if let Ok(template_json) = serde_json::to_string(&template) {
            let notify_timeout = Duration::from_millis(self.config.notify_timeout_millis);
            for url in &self.config.notify {
                let mut req_builder = Request::builder()
                    .method(Method::POST)
                    .uri(url.as_ref())
                    .header("content-type", "application/json");

                if let Some(token) = &self.config.notify_auth_token {
                    let mut auth_value = match HeaderValue::from_str(&format!("Bearer {token}")) {
                        Ok(value) => value,
                        Err(err) => {
                            ckb_logger::error!("invalid block_assembler.notify_auth_token: {err}");
                            continue;
                        }
                    };
                    auth_value.set_sensitive(true);
                    req_builder = req_builder.header(hyper::header::AUTHORIZATION, auth_value);
                }

                if let Ok(req) = req_builder.body(Full::new(template_json.to_owned().into())) {
                    let client = Arc::clone(&self.poster);
                    let url = url.to_owned();
                    tokio::spawn(async move {
                        let _resp =
                            timeout(notify_timeout, client.request(req))
                                .await
                                .map_err(|_| {
                                    ckb_logger::warn!(
                                        "block assembler notifying {} timed out",
                                        url
                                    );
                                });
                    });
                }
            }

            self.script_notifier.notify(&template_json, notify_timeout);
        }
    }

    fn need_to_notify(&self) -> bool {
        !self.config.notify.is_empty() || !self.config.notify_scripts.is_empty()
    }
}

#[cfg(test)]
#[path = "tests/notify.rs"]
mod tests;
