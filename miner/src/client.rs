use crate::Work;
use base64::Engine;
use ckb_app_config::MinerClientConfig;
use ckb_async_runtime::Handle;
use ckb_channel::Sender;
use ckb_jsonrpc_types::{Block as JsonBlock, BlockTemplate};
use ckb_logger::{debug, error, info};
use ckb_stop_handler::{new_tokio_exit_rx, CancellationToken};
use ckb_types::{
    packed::{Block, Byte32},
    H256,
};
use futures::prelude::*;
use http_body_util::{BodyExt, Empty, Full};
use hyper::{
    body::{Buf, Bytes},
    header::{HeaderValue, CONTENT_TYPE},
    service::service_fn,
    Error as HyperError, Request, Response, Uri,
};
use hyper_util::{
    client::legacy::{Client as HttpClient, Error as ClientError},
    rt::TokioExecutor,
    server::{conn::auto, graceful::GracefulShutdown},
};
use jsonrpc_core::{
    error::Error as RpcFail, error::ErrorCode as RpcFailCode, id::Id, params::Params,
    request::MethodCall, response::Output, version::Version,
};
use serde_json::error::Error as JsonError;
use serde_json::{self, json, Value};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::{convert::Into, time};
use tokio::{
    net::TcpListener,
    sync::{mpsc, oneshot},
};

type RpcRequest = (oneshot::Sender<Result<Bytes, RpcError>>, MethodCall);

#[derive(Debug)]
pub enum RpcError {
    Http(HyperError),
    Client(ClientError),
    Canceled, //oneshot canceled
    Json(JsonError),
    Fail(RpcFail),
    SendError,
    NoRespData,
}

#[derive(Debug, Clone)]
pub struct Rpc {
    sender: mpsc::Sender<RpcRequest>,
}

impl Rpc {
    pub fn new(url: Uri, handle: Handle) -> Rpc {
        let (sender, mut receiver) = mpsc::channel(65_535);
        let stop_rx: CancellationToken = new_tokio_exit_rx();

        let https = hyper_tls::HttpsConnector::new();
        let client = HttpClient::builder(TokioExecutor::new()).build::<_, Full<Bytes>>(https);
        let loop_handle = handle.clone();
        handle.spawn(async move {
            loop {
                tokio::select! {
                    Some(item) = receiver.recv() => {
                        let (sender, call): RpcRequest = item;
                        let req_url = url.clone();
                        let request_json = serde_json::to_vec(&call).expect("valid rpc call");

                        let mut req = Request::builder().uri(req_url).method("POST").header(CONTENT_TYPE, "application/json");

                        if let Some(value) = parse_authorization(&url) {
                            req = req
                                .header(hyper::header::AUTHORIZATION, value);
                        }
                        let req = req.body(Full::new(Bytes::from(request_json))).unwrap();
                        let client = client.clone();
                        loop_handle.spawn(async move {
                            let request = match client
                                .request(req)
                                .await
                                .map(|res|res.into_body())
                            {
                                Ok(body) => BodyExt::collect(body).await.map_err(RpcError::Http).map(|t| t.to_bytes()),
                                Err(err) => Err(RpcError::Client(err)),
                            };
                            if sender.send(request).is_err() {
                                error!("rpc response send back error")
                            }
                        });
                    },
                    _ = stop_rx.cancelled() => {
                        info!("Rpc server received exit signal, exit now");
                        break
                    },
                    else => break
                }
            }
        });

        Rpc { sender }
    }

    pub fn request(
        &self,
        method: String,
        params: Vec<Value>,
    ) -> impl Future<Output = Result<Output, RpcError>> {
        let (tx, rev) = oneshot::channel();

        let call = MethodCall {
            method,
            params: Params::Array(params),
            jsonrpc: Some(Version::V2),
            id: Id::Num(0),
        };

        let req = (tx, call);
        let sender = self.sender.clone();
        async move {
            sender
                .clone()
                .send(req)
                .map_err(|_| RpcError::SendError)
                .await?;
            rev.map_err(|_| RpcError::Canceled)
                .await?
                .and_then(|chunk| serde_json::from_slice(&chunk).map_err(RpcError::Json))
        }
    }
}

pub enum Works {
    New(Work),
    FailSubmit(Byte32),
}

/// TODO(doc): @quake
#[derive(Debug, Clone)]
pub struct Client {
    /// TODO(doc): @quake
    pub current_work_id: Arc<AtomicU64>,
    /// TODO(doc): @quake
    pub new_work_tx: Sender<Works>,
    /// TODO(doc): @quake
    pub config: MinerClientConfig,
    /// TODO(doc): @quake
    pub rpc: Rpc,
    handle: Handle,
}

impl Client {
    /// Construct new Client
    pub fn new(new_work_tx: Sender<Works>, config: MinerClientConfig, handle: Handle) -> Client {
        let uri: Uri = config.rpc_url.parse().expect("valid rpc url");

        Client {
            current_work_id: Arc::new(AtomicU64::new(0)),
            rpc: Rpc::new(uri, handle.clone()),
            new_work_tx,
            config,
            handle,
        }
    }

    fn send_submit_block_request(
        &self,
        work_id: &str,
        block: Block,
    ) -> impl Future<Output = Result<Output, RpcError>> + 'static + Send {
        let block: JsonBlock = block.into();
        let method = "submit_block".to_owned();
        let params = vec![json!(work_id), json!(block)];

        self.rpc.clone().request(method, params)
    }

    pub(crate) fn submit_block(&self, work_id: &str, block: Block) -> Result<(), RpcError> {
        let parent = block.header().raw().parent_hash();
        let future = self
            .send_submit_block_request(work_id, block)
            .and_then(parse_response::<H256>);

        if self.config.block_on_submit {
            self.handle.block_on(future).map(|_| ())
        } else {
            let sender = self.new_work_tx.clone();
            self.handle.spawn(async move {
                if let Err(e) = future.await {
                    error!("rpc call submit_block error: {:?}", e);
                    sender.send(Works::FailSubmit(parent)).unwrap()
                }
            });
            Ok(())
        }
    }

    /// spawn background update process
    pub fn spawn_background(self) {
        let client = self.clone();
        if let Some(addr) = self.config.listen {
            ckb_logger::info!("listen notify mode : {}", addr);
            ckb_logger::info!(
                r#"
Please note that ckb-miner runs in notify mode. \
You should configure the corresponding information in CKB block assembler, \
for example:

[block_assembler]
...
notify = ["http://{}"]

Otherwise ckb-miner will malfunction and stop submitting valid blocks after a certain period.
"#,
                addr
            );
            self.handle.spawn(async move {
                client.listen_block_template_notify(addr).await;
            });
            self.blocking_fetch_block_template();
        } else {
            ckb_logger::info!("loop poll mode: interval {}ms", self.config.poll_interval);
            self.handle.spawn(async move {
                client.poll_block_template().await;
            });
        }
    }

    async fn listen_block_template_notify(&self, addr: SocketAddr) {
        let listener = TcpListener::bind(addr).await.unwrap();
        let server = auto::Builder::new(TokioExecutor::new());
        let graceful = GracefulShutdown::new();
        let stop_rx: CancellationToken = new_tokio_exit_rx();

        loop {
            let client = self.clone();
            let handle = service_fn(move |req| handle(client.clone(), req));
            tokio::select! {
                conn = listener.accept() => {
                    let (stream, _) = match conn {
                        Ok(conn) => conn,
                        Err(e) => {
                            info!("accept error: {}", e);
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                            continue;
                        }
                    };
                    let stream = hyper_util::rt::TokioIo::new(Box::pin(stream));
                    let conn = server.serve_connection_with_upgrades(stream, handle);

                    let conn = graceful.watch(conn.into_owned());
                    tokio::spawn(async move {
                        if let Err(err) = conn.await {
                            info!("connection error: {}", err);
                        }
                    });
                },
                _ = stop_rx.cancelled() => {
                    info!("Miner client received exit signal. Exit now");
                    break;
                }
            }
        }
        drop(listener);
        graceful.shutdown().await;
    }

    async fn poll_block_template(&self) {
        let poll_interval = time::Duration::from_millis(self.config.poll_interval);
        let mut interval = tokio::time::interval(poll_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let stop_rx: CancellationToken = new_tokio_exit_rx();
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    debug!("poll block template...");
                    self.fetch_block_template().await;
                }
                _ = stop_rx.cancelled() => {
                    info!("Miner client pool_block_template received exit signal, exit now");
                    break
                },
                else => break,
            }
        }
    }

    fn update_block_template(&self, block_template: BlockTemplate) {
        let work_id = block_template.work_id.into();
        let updated = |id| {
            if id != work_id || id == 0 {
                Some(work_id)
            } else {
                None
            }
        };
        if self
            .current_work_id
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, updated)
            .is_ok()
        {
            let work: Work = block_template.into();
            if let Err(e) = self.new_work_tx.send(Works::New(work)) {
                error!("notify_new_block error: {:?}", e);
            }
        }
    }

    pub(crate) fn blocking_fetch_block_template(&self) {
        self.handle.block_on(self.fetch_block_template())
    }

    async fn fetch_block_template(&self) {
        match self.get_block_template().await {
            Ok(block_template) => {
                self.update_block_template(block_template);
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
                         Please perform the following checks: \
                         1. Ensure that the CKB server has enabled the Miner API module; \
                         2. Verify that the CKB server has set the `block_assembler` correctly; \
                         3. Confirm that the RPC URL for CKB miner is correct.",
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
}

type Error = Box<dyn std::error::Error + Send + Sync>;

async fn handle(
    client: Client,
    req: Request<hyper::body::Incoming>,
) -> Result<Response<Empty<Bytes>>, Error> {
    let body = BodyExt::collect(req).await?.aggregate();

    if let Ok(template) = serde_json::from_reader(body.reader()) {
        client.update_block_template(template);
    }

    Ok(Response::new(Empty::new()))
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
        base64::prelude::BASE64_STANDARD.encode_string(a[0], &mut encoded);
        let mut header = HeaderValue::from_str(&encoded).unwrap();
        header.set_sensitive(true);
        Some(header)
    } else {
        None
    }
}
