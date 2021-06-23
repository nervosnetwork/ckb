use crate::Work;
use ckb_app_config::MinerClientConfig;
use ckb_async_runtime::Handle;
use ckb_channel::Sender;
use ckb_error::AnyError;
use ckb_jsonrpc_types::{Block as JsonBlock, BlockTemplate};
use ckb_logger::{debug, error, warn};
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_types::{packed::Block, H256};
use futures::prelude::*;
use hyper::{
    body::{Bytes, HttpBody},
    error::Error as HyperError,
    header::{HeaderValue, CONTENT_TYPE},
    Body, Client as HttpClient, Method, Request, Uri,
};
use jsonrpc_core::{
    error::Error as RpcFail, error::ErrorCode as RpcFailCode, id::Id, params::Params,
    request::MethodCall, response::Output, version::Version,
};
use serde_json::error::Error as JsonError;
use serde_json::{self, json, Value};
use std::convert::Into;
use std::thread;
use std::time;
use tokio::sync::{mpsc, oneshot};
use tokio_compat_02::FutureExt;

type RpcRequest = (oneshot::Sender<Result<Bytes, RpcError>>, MethodCall);

#[derive(Debug)]
pub enum RpcError {
    Http(HyperError),
    Canceled, //oneshot canceled
    Json(JsonError),
    Fail(RpcFail),
    SendError,
    NoRespData,
}

#[derive(Debug, Clone)]
pub struct Rpc {
    sender: mpsc::Sender<RpcRequest>,
    stop: StopHandler<()>,
}

impl Rpc {
    pub fn new(url: Uri, handle: Handle) -> Rpc {
        let (sender, mut receiver) = mpsc::channel(65_535);
        let (stop, mut stop_rx) = oneshot::channel::<()>();

        let https = hyper_tls::HttpsConnector::new();
        let client = HttpClient::builder().build(https);
        handle.spawn(async move {
            loop {
                tokio::select! {
                    Some(item) = receiver.recv() => {
                        let (sender, call): RpcRequest = item;
                        let req_url = url.clone();
                        let request_json = serde_json::to_vec(&call).expect("valid rpc call");
                        let mut req = Request::new(Body::from(request_json));
                        *req.method_mut() = Method::POST;
                        *req.uri_mut() = req_url;
                        req.headers_mut()
                            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
                        if let Some(value) = parse_authorization(&url) {
                            req.headers_mut()
                                .append(hyper::header::AUTHORIZATION, value);
                        }
                        let request = match client
                            .request(req)
                            .compat()
                            .await
                            .map(|res|res.into_body())
                        {
                            Ok(mut body) => body
                                .data()
                                .await
                                .ok_or(RpcError::NoRespData)
                                .and_then(|res| res.map_err(RpcError::Http)),
                            Err(err) => Err(RpcError::Http(err)),
                        };
                        if sender.send(request).is_err() {
                            break;
                        }
                    },
                    _ = &mut stop_rx => break,
                    else => break
                }
            }
        });

        Rpc {
            sender,
            stop: StopHandler::new(SignalSender::Tokio(stop), None),
        }
    }

    pub async fn request(&self, method: String, params: Vec<Value>) -> Result<Output, RpcError> {
        let (tx, rev) = oneshot::channel();

        let call = MethodCall {
            method,
            params: Params::Array(params),
            jsonrpc: Some(Version::V2),
            id: Id::Num(0),
        };

        let req = (tx, call);
        self.sender
            .send(req)
            .map_err(|_| RpcError::SendError)
            .await?;
        rev.map_err(|_| RpcError::Canceled)
            .await?
            .and_then(|chunk| serde_json::from_slice(&chunk).map_err(RpcError::Json))
    }
}

impl Drop for Rpc {
    fn drop(&mut self) {
        self.stop.try_send();
    }
}

/// TODO(doc): @quake
#[derive(Debug, Clone)]
pub struct Client {
    /// TODO(doc): @quake
    pub current_work_id: Option<u64>,
    /// TODO(doc): @quake
    pub new_work_tx: Sender<Work>,
    /// TODO(doc): @quake
    pub config: MinerClientConfig,
    /// TODO(doc): @quake
    pub rpc: Rpc,
    handle: Handle,
}

impl Client {
    /// TODO(doc): @quake
    pub fn new(new_work_tx: Sender<Work>, config: MinerClientConfig, handle: Handle) -> Client {
        let uri: Uri = config.rpc_url.parse().expect("valid rpc url");

        Client {
            current_work_id: None,
            rpc: Rpc::new(uri, handle.clone()),
            new_work_tx,
            config,
            handle,
        }
    }

    async fn send_submit_block_request(
        &self,
        work_id: &str,
        block: Block,
    ) -> Result<Output, RpcError> {
        let block: JsonBlock = block.into();
        let method = "submit_block".to_owned();
        let params = vec![json!(work_id), json!(block)];

        self.rpc.request(method, params).await
    }

    /// TODO(doc): @quake
    pub fn submit_block(&self, work_id: &str, block: Block) {
        let future = self
            .send_submit_block_request(work_id, block)
            .and_then(parse_response);
        if self.config.block_on_submit {
            let ret: Result<Option<H256>, RpcError> = self.handle.block_on(future);
            match ret {
                Ok(hash) => {
                    if hash.is_none() {
                        warn!("submit_block failed");
                    }
                }
                Err(e) => {
                    error!("rpc call submit_block error: {:?}", e);
                }
            }
        }
    }

    /// TODO(doc): @quake
    pub fn poll_block_template(&mut self) {
        loop {
            debug!("poll block template...");
            self.try_update_block_template();
            thread::sleep(time::Duration::from_millis(self.config.poll_interval));
        }
    }

    /// TODO(doc): @quake
    pub fn try_update_block_template(&mut self) {
        match self.handle.block_on(self.get_block_template()) {
            Ok(block_template) => {
                if self.current_work_id != Some(block_template.work_id.into()) {
                    self.current_work_id = Some(block_template.work_id.into());
                    if let Err(e) = self.notify_new_work(block_template) {
                        error!("notify_new_block error: {:?}", e);
                    }
                }
            }
            Err(ref err) => {
                let is_method_not_found = if let RpcError::Fail(RpcFail { code, .. }) = err {
                    *code == RpcFailCode::MethodNotFound
                } else {
                    false
                };
                if is_method_not_found {
                    error!(
                        "RPC Method Not Found: \
                         please do checks as follow: \
                         1. if the CKB server has enabled the Miner API module; \
                         2. if the CKB server has set `block_assembler`; \
                         3. If the RPC URL for CKB miner is right.",
                    );
                } else {
                    error!("rpc call get_block_template error: {:?}", err);
                }
            }
        }
    }

    async fn get_block_template(&self) -> Result<BlockTemplate, RpcError> {
        let method = "get_block_template".to_owned();
        let params = vec![];

        self.rpc
            .request(method, params)
            .and_then(parse_response)
            .await
    }

    fn notify_new_work(&self, block_template: BlockTemplate) -> Result<(), AnyError> {
        let work: Work = block_template.into();
        self.new_work_tx.send(work)?;
        Ok(())
    }
}

async fn parse_response<T: serde::de::DeserializeOwned>(output: Output) -> Result<T, RpcError> {
    match output {
        Output::Success(success) => {
            serde_json::from_value::<T>(success.result).map_err(RpcError::Json)
        }
        Output::Failure(failure) => Err(RpcError::Fail(failure.error)),
    }
}

fn parse_authorization(url: &Uri) -> Option<HeaderValue> {
    let a: Vec<&str> = url.authority()?.as_str().split('@').collect();
    if a.len() >= 2 {
        if a[0].is_empty() {
            return None;
        }
        let mut encoded = "Basic ".to_string();
        base64::encode_config_buf(a[0], base64::STANDARD, &mut encoded);
        let mut header = HeaderValue::from_str(&encoded).unwrap();
        header.set_sensitive(true);
        Some(header)
    } else {
        None
    }
}
