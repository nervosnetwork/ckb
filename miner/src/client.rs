use crate::{MinerConfig, Work};
use ckb_core::block::Block;
use crossbeam_channel::Sender;
use futures::sync::{mpsc, oneshot};
use hyper::error::Error as HyperError;
use hyper::header::{HeaderValue, CONTENT_TYPE};
use hyper::rt::{self, Future, Stream};
use hyper::Uri;
use hyper::{Body, Chunk, Client as HttpClient, Method, Request};
use jsonrpc_types::BlockTemplate;
use jsonrpc_types::{
    error::Error as RpcFail, id::Id, params::Params, request::MethodCall, response::Output,
    version::Version, Block as JsonBlock,
};
use log::{debug, error, warn};
use numext_fixed_hash::H256;
use serde_json::error::Error as JsonError;
use serde_json::{self, json, Value};
use std::thread;
use std::time;
use stop_handler::{SignalSender, StopHandler};

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
            let client = HttpClient::builder().keep_alive(true).build_http();

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

#[derive(Debug, Clone)]
pub struct Client {
    pub current_work: Work,
    pub new_work: Sender<()>,
    pub config: MinerConfig,
    pub rpc: Rpc,
}

impl Client {
    pub fn new(current_work: Work, new_work: Sender<()>, config: MinerConfig) -> Client {
        let uri: Uri = config.rpc_url.parse().expect("valid rpc url");

        Client {
            current_work,
            rpc: Rpc::new(uri),
            new_work,
            config,
        }
    }

    fn send_submit_block_request(
        &self,
        work_id: &str,
        block: &Block,
    ) -> impl Future<Item = Output, Error = RpcError> {
        let block: JsonBlock = block.into();
        let method = "submit_block".to_owned();
        let params = vec![json!(work_id), json!(block)];

        self.rpc.request(method, params)
    }

    pub fn submit_block(&self, work_id: &str, block: &Block) {
        let future = self.send_submit_block_request(work_id, block);
        if self.config.block_on_submit {
            let ret: Result<Option<H256>, RpcError> = future.and_then(parse_response).wait();
            match ret {
                Ok(hash) => {
                    if hash.is_none() {
                        warn!(target: "miner", "submit_block failed {}", serde_json::to_string(block).unwrap());
                    }
                }
                Err(e) => {
                    error!(target: "miner", "rpc call submit_block error: {:?}", e);
                }
            }
        }
    }

    pub fn poll_block_template(&self) {
        loop {
            debug!(target: "miner", "poll block template...");
            self.try_update_block_template();
            thread::sleep(time::Duration::from_millis(self.config.poll_interval));
        }
    }

    pub fn try_update_block_template(&self) -> bool {
        let mut updated = false;
        match self.get_block_template().wait() {
            Ok(new) => {
                let mut work = self.current_work.lock();
                if work.as_ref().map_or(true, |old| old.work_id != new.work_id) {
                    *work = Some(new);
                    updated = true;
                    let _ = self.new_work.send(());
                }
            }
            Err(e) => {
                error!(target: "miner", "rpc call get_block_template error: {:?}", e);
            }
        }
        updated
    }

    fn get_block_template(&self) -> impl Future<Item = BlockTemplate, Error = RpcError> {
        let method = "get_block_template".to_owned();
        let params = vec![
            json!(self.config.cycles_limit.to_string()),
            json!(self.config.bytes_limit.to_string()),
            json!(self.config.max_version),
        ];

        self.rpc.request(method, params).and_then(parse_response)
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
