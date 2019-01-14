use crate::{Config, Work};
use ckb_util::{Mutex, RwLockUpgradableReadGuard};
use crossbeam_channel::Sender;
use futures::sync::{mpsc, oneshot};
use hyper::error::Error as HyperError;
use hyper::header::{HeaderValue, CONTENT_TYPE};
use hyper::rt::{self, Future, Stream};
use hyper::Uri;
use hyper::{Body, Chunk, Client as HttpClinet, Method, Request};
use jsonrpc_types::BlockTemplate;
use jsonrpc_types::{id::Id, params::Params, request::MethodCall, version::Version, Block as JsonBlock};
use log::debug;
use log::error;
use serde_json::error::Error as JsonError;
use serde_json::{self, json, Value};
use std::sync::Arc;
use std::thread;
use std::time;
use ckb_core::block::Block;

type RpcRequest = (oneshot::Sender<Result<Chunk, RpcError>>, MethodCall);

#[derive(Debug)]
pub enum RpcError {
    Http(HyperError),
    Canceled, //oneshot canceled
    Json(JsonError),
}

#[derive(Debug)]
pub(crate) struct Stop {
    tx: oneshot::Sender<()>,
    thread: thread::JoinHandle<()>,
}

impl Stop {
    pub fn new(tx: oneshot::Sender<()>, thread: thread::JoinHandle<()>) -> Stop {
        Stop { tx, thread }
    }

    pub fn send(self) {
        self.tx.send(()).expect("rpc stop channel");;
        self.thread.join().expect("rpc thread join");
    }
}

#[derive(Debug)]
pub(crate) struct RpcInner {
    sender: mpsc::Sender<RpcRequest>,
    stop: Mutex<Option<Stop>>,
}

#[derive(Debug, Clone)]
pub struct Rpc {
    inner: Arc<RpcInner>,
}

impl Rpc {
    pub fn new(url: Uri) -> Rpc {
        let (sender, receiver) = mpsc::channel(65_535);
        let (stop, stop_rx) = oneshot::channel::<()>();

        let thread = thread::spawn(move || {
            let client = HttpClinet::builder().keep_alive(true).build_http();

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
                    .map_err(|err| {
                        error!(target: "miner", "rpc request error {:?}", err);
                    });

                rt::spawn(request);
                Ok(())
            });

            rt::run(stream.select2(stop_rx).map(|_| ()).map_err(|_| ()));
        });

        Rpc {
            inner: Arc::new(RpcInner {
                sender,
                stop: Mutex::new(Some(Stop::new(stop, thread))),
            }),
        }
    }

    pub fn request(
        &self,
        method: String,
        params: Vec<Value>,
    ) -> impl Future<Item = Chunk, Error = RpcError> {
        let (tx, rev) = oneshot::channel();

        let call = MethodCall {
            method,
            params: Params::Array(params),
            jsonrpc: Some(Version::V2),
            id: Id::Num(0),
        };

        let req = (tx, call);
        let mut sender = self.inner.sender.clone();
        let _ = sender.try_send(req);
        rev.map_err(|_| RpcError::Canceled).flatten()
    }
}

impl Drop for Rpc {
    fn drop(&mut self) {
        let stop = self.inner.stop.lock().take().expect("rpc close only once");
        stop.send();
    }
}

#[derive(Debug, Clone)]
pub struct Client {
    pub current_work: Work,
    pub new_work: Sender<()>,
    pub config: Config,
    pub rpc: Rpc,
}

impl Client {
    pub fn new(current_work: Work, new_work: Sender<()>, config: Config) -> Client {
        let uri: Uri = config.rpc_url.parse().expect("valid rpc url");

        Client {
            current_work,
            rpc: Rpc::new(uri),
            new_work,
            config,
        }
    }

    pub fn run(&self) {
        self.poll_block_template();
    }

    pub fn submit_block(
        &self,
        work_id: &str,
        block: &Block,
    ) -> impl Future<Item = Chunk, Error = RpcError> {
        let block: JsonBlock = block.into();
        let method = "submit_block".to_owned();
        let params = vec![json!(block), json!(work_id)];

        self.rpc.request(method, params)
    }

    fn poll_block_template(&self) {
        loop {
            debug!(target: "miner", "poll block template...");
            match self.get_block_template().wait() {
                Ok(new) => {
                    let work = self.current_work.upgradable_read();
                    if work.as_ref().map_or(true, |old| old.work_id != new.work_id) {
                        let mut write_guard = RwLockUpgradableReadGuard::upgrade(work);
                        *write_guard = Some(new);
                        let _ = self.new_work.send(());
                    }
                }
                Err(e) => {
                    error!(target: "miner", "rpc call get_block_template error: {:?}", e);
                }
            }
            thread::sleep(time::Duration::from_secs(self.config.poll_interval));
        }
    }

    fn get_block_template(&self) -> impl Future<Item = BlockTemplate, Error = RpcError> {
        let method = "get_block_template".to_owned();
        let params = vec![
            json!(self.config.cycles_limit),
            json!(self.config.bytes_limit),
            json!(self.config.max_version),
        ];

        self.rpc
            .request(method, params)
            .and_then(|body| serde_json::from_slice(&body).map_err(RpcError::Json))
    }
}
