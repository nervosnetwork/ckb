//! Block template HTTP/script notification.

use crate::block_assembler::BlockAssembler;
use ckb_types::bytes::Bytes;
use futures_util::{
    StreamExt,
    stream::{self, FuturesUnordered},
};
use http_body_util::Full;
use hyper::{
    Request,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue},
};
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

impl BlockAssembler {
    pub(crate) fn notifications_enabled(&self) -> bool {
        self.poster.is_some() || !self.config.notify_scripts.is_empty()
    }

    pub(crate) async fn notify(&self) {
        let Some(template_json) = self
            .get_current()
            .and_then(|template| serde_json::to_string(&template).ok())
        else {
            return;
        };
        let limit = Duration::from_millis(self.config.notify_timeout_millis);
        // Commands own their arguments. The HTTP requests can then share the
        // original encoded bytes. All deliveries stay within this one task.
        let mut scripts = FuturesUnordered::new();
        for command in &self.config.notify_scripts {
            let mut child = Command::new(command);
            child.arg(&template_json).kill_on_drop(true);
            scripts.push(async move {
                match timeout(limit, child.status()).await {
                    Ok(Ok(status)) => ckb_logger::debug!("the command exited with: {status}"),
                    Ok(Err(error)) => {
                        ckb_logger::error!("the script {command} failed to spawn: {error}")
                    }
                    Err(_) => ckb_logger::warn!(
                        "block assembler notifying {command} timed out and was terminated"
                    ),
                }
            });
        }
        tokio::join!(self.post_template(template_json, limit), async {
            while scripts.next().await.is_some() {}
        });
    }

    async fn post_template(&self, template_json: String, limit: Duration) {
        let Some(client) = &self.poster else {
            return;
        };
        let authorization = match self
            .config
            .notify_auth_token
            .as_ref()
            .map(|token| HeaderValue::from_str(&format!("Bearer {token}")))
            .transpose()
        {
            Ok(mut header) => {
                if let Some(value) = &mut header {
                    value.set_sensitive(true);
                }
                header
            }
            Err(error) => {
                ckb_logger::error!("invalid block_assembler.notify_auth_token: {error}");
                return;
            }
        };
        let body = Bytes::from(template_json);
        stream::iter(&self.config.notify)
            .for_each_concurrent(None, |url| {
                let mut request =
                    Request::post(url.as_ref()).header(CONTENT_TYPE, "application/json");
                if let Some(header) = &authorization {
                    request = request.header(AUTHORIZATION, header);
                }
                let request = request.body(Full::new(body.clone()));
                async move {
                    if let Ok(request) = request
                        && timeout(limit, client.request(request)).await.is_err()
                    {
                        ckb_logger::warn!("block assembler notifying {url} timed out");
                    }
                }
            })
            .await;
    }
}
