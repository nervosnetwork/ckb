use crate::Work;
use ckb_app_config::MinerClientConfig;
use ckb_channel::Sender;
use ckb_jsonrpc_types::{
    error::Error as RpcFail, error::ErrorCode as RpcFailCode, id::Id, params::Params,
    request::MethodCall, response::Output, version::Version, Block as JsonBlock, BlockTemplate,
};
use ckb_logger::{debug, error, warn};
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_types::{packed::Block, H256};
use failure::Error;
use futures::sync::{mpsc, oneshot};
use hyper::error::Error as HyperError;
use hyper::header::{HeaderValue, CONTENT_TYPE};
use hyper::rt::{self, Future, Stream};
use hyper::Uri;
use hyper::{Body, Chunk, Client as HttpClient, Method, Request};
use serde_json::error::Error as JsonError;
use serde_json::{self, json, Value};
use std::convert::Into;
use std::thread;
use std::time;

type RpcRequest = (oneshot::Sender<Result<Chunk, RpcError>>, MethodCall);

#[derive(Debug)]
pub enum RpcError {
    Http(HyperError),
    Canceled, //oneshot canceled
    Json(JsonError),
    Fail(RpcFail),
}

#[derive(Debug, Clone)]
pub struct Rpc {
    sender: mpsc::Sender<RpcRequest>,
    stop: StopHandler<()>,
}

impl Rpc {
    pub fn new(url: Uri) -> Rpc {
        let (sender, receiver) = mpsc::channel(65_535);
        let (stop, stop_rx) = oneshot::channel::<()>();

        let thread = thread::spawn(move || {
            // 1 is number of blocking DNS threads, this connector will use plain HTTP if the URL provded uses the HTTP scheme.
            let https =
                hyper_tls::HttpsConnector::new(1).expect("init https connector should be OK");
            let client = HttpClient::builder().keep_alive(true).build(https);

            let stream = receiver.for_each(move |(sender, call): RpcRequest| {
                let req_url = url.clone();
                let request_json = serde_json::to_vec(&call).expect("valid rpc call");

                let mut req = Request::new(Body::from(request_json));
                *req.method_mut() = Method::POST;
                *req.uri_mut() = req_url;
                req.headers_mut()
                    .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

                let request = client
                    .request(req)
                    .and_then(|res| res.into_body().concat2())
                    .then(|res| sender.send(res.map_err(RpcError::Http)))
                    .map_err(|_| ());

                rt::spawn(request);
                Ok(())
            });

            rt::run(stream.select2(stop_rx).map(|_| ()).map_err(|_| ()));
        });

        Rpc {
            sender,
            stop: StopHandler::new(SignalSender::Future(stop), thread),
        }
    }

    pub fn request(
        &self,
        method: String,
        params: Vec<Value>,
    ) -> impl Future<Item = Output, Error = RpcError> {
        let (tx, rev) = oneshot::channel();

        let call = MethodCall {
            method,
            params: Params::Array(params),
            jsonrpc: Some(Version::V2),
            id: Id::Num(0),
        };

        let req = (tx, call);
        let mut sender = self.sender.clone();
        let _ = sender.try_send(req);
        rev.map_err(|_| RpcError::Canceled)
            .flatten()
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
}

impl Client {
    /// TODO(doc): @quake
    pub fn new(new_work_tx: Sender<Work>, config: MinerClientConfig) -> Client {
        let uri: Uri = config.rpc_url.parse().expect("valid rpc url");

        Client {
            current_work_id: None,
            rpc: Rpc::new(uri),
            new_work_tx,
            config,
        }
    }

    fn send_submit_block_request(
        &self,
        work_id: &str,
        block: Block,
    ) -> impl Future<Item = Output, Error = RpcError> {
        let block: JsonBlock = block.into();
        let method = "submit_block".to_owned();
        let params = vec![json!(work_id), json!(block)];

        self.rpc.request(method, params)
    }

    /// TODO(doc): @quake
    pub fn submit_block(&self, work_id: &str, block: Block) {
        let future = self.send_submit_block_request(work_id, block);
        if self.config.block_on_submit {
            let ret: Result<Option<H256>, RpcError> = future.and_then(parse_response).wait();
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
        match self.get_block_template().wait() {
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

    fn get_block_template(&self) -> impl Future<Item = BlockTemplate, Error = RpcError> {
        let method = "get_block_template".to_owned();
        let params = vec![];

        self.rpc.request(method, params).and_then(parse_response)
    }

    fn notify_new_work(&self, block_template: BlockTemplate) -> Result<(), Error> {
        let work: Work = block_template.into();
        self.new_work_tx.send(work)?;
        Ok(())
    }
}

fn parse_response<T: serde::de::DeserializeOwned>(output: Output) -> Result<T, RpcError> {
    match output {
        Output::Success(success) => {
            serde_json::from_value::<T>(success.result).map_err(RpcError::Json)
        }
        Output::Failure(failure) => Err(RpcError::Fail(failure.error)),
    }
}
