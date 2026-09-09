//! Block template HTTP/script notification.

use crate::block_assembler::BlockAssembler;
use futures_util::{StreamExt, stream::FuturesUnordered};
use http_body_util::Full;
use hyper::{Method, Request, header::HeaderValue};
use std::{sync::Arc, time::Duration};
use tokio::process::Command;
use tokio::time::timeout;

impl BlockAssembler {
    pub(crate) fn notifications_enabled(&self) -> bool {
        !self.config.notify.is_empty() || !self.config.notify_scripts.is_empty()
    }

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
            // The existing Notification lane owns this complete batch. All
            // configured endpoints run concurrently inside that one future;
            // no nested task can outlive generation cancellation or its join.
            // The resident concurrency bound is therefore exactly the number
            // of configured HTTP endpoints plus commands, independent of the
            // template publication rate.
            let http_notifications = FuturesUnordered::new();
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
                    http_notifications.push(async move {
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

            let script_notifications = FuturesUnordered::new();
            for command in &self.config.notify_scripts {
                let command = command.clone();
                let template_json = template_json.clone();
                script_notifications.push(async move {
                    let mut child = Command::new(&command);
                    child.arg(template_json).kill_on_drop(true);
                    match timeout(notify_timeout, child.status()).await {
                        Ok(Ok(status)) => {
                            ckb_logger::debug!("the command exited with: {status}")
                        }
                        Ok(Err(error)) => {
                            ckb_logger::error!("the script {command} failed to spawn: {error}")
                        }
                        Err(_) => ckb_logger::warn!(
                            "block assembler notifying {command} timed out and was terminated"
                        ),
                    }
                });
            }

            let mut notifications =
                futures_util::stream::select(http_notifications, script_notifications);
            while notifications.next().await.is_some() {}
        }
    }

    fn need_to_notify(&self) -> bool {
        self.notifications_enabled()
    }
}
