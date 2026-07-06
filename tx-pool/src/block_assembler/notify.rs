//! Block template HTTP/script notification.

use crate::block_assembler::BlockAssembler;
use http_body_util::Full;
use hyper::{Method, Request};
use std::{sync::Arc, time::Duration};
use tokio::process::Command;
use tokio::time::timeout;

impl BlockAssembler {
    pub(crate) async fn notify(&self) {
        if !self.need_to_notify() {
            return;
        }
        let template = self.get_current().await;
        if let Ok(template_json) = serde_json::to_string(&template) {
            let notify_timeout = Duration::from_millis(self.config.notify_timeout_millis);
            for url in &self.config.notify {
                if let Ok(req) = Request::builder()
                    .method(Method::POST)
                    .uri(url.as_ref())
                    .header("content-type", "application/json")
                    .body(Full::new(template_json.to_owned().into()))
                {
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

            for script in &self.config.notify_scripts {
                let script = script.to_owned();
                let template_json = template_json.to_owned();
                tokio::spawn(async move {
                    // Errors
                    // This future will return an error if the child process cannot be spawned
                    // or if there is an error while awaiting its status.

                    // On Unix platforms this method will fail with std::io::ErrorKind::WouldBlock
                    // if the system process limit is reached
                    // (which includes other applications running on the system).
                    match timeout(
                        notify_timeout,
                        Command::new(&script).arg(template_json).status(),
                    )
                    .await
                    {
                        Ok(ret) => match ret {
                            Ok(status) => ckb_logger::debug!("the command exited with: {}", status),
                            Err(e) => {
                                ckb_logger::error!("the script {} failed to spawn {}", script, e)
                            }
                        },
                        Err(_) => {
                            ckb_logger::warn!("block assembler notifying {} timed out", script)
                        }
                    }
                });
            }
        }
    }

    fn need_to_notify(&self) -> bool {
        !self.config.notify.is_empty() || !self.config.notify_scripts.is_empty()
    }
}
