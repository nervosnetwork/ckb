use super::error::{Error, JsonRpcError};
use ckb_core::header::Header;
use futures::sync::{mpsc, oneshot};
use hyper::error::Error as HttpError;
use hyper::header::{HeaderValue, CONTENT_TYPE};
use hyper::rt::{self, Future, Stream};
use hyper::{Body, Chunk, Client, Method, Request};
use serde;
use serde_json;
use serde_json::Value;
use std::thread;

pub use hyper::Uri;
pub type RpcRequest = (oneshot::Sender<Result<Chunk, HttpError>>, String, String);

/// RPC client to the ckb process.
///
/// Makes use of "async IO" (non-blocking sockets) via the Hyper crates,
/// all interface return Future that make easy to do async tasks combine
pub struct Rpc {
    pub sender: mpsc::Sender<RpcRequest>,
    //
    pub stop: Option<oneshot::Sender<()>>,
    pub thread: Option<thread::JoinHandle<()>>,
}

// success response
#[derive(Debug, PartialEq, Clone, Deserialize)]
pub struct Success {
    pub jsonrpc: Option<String>,
    pub result: Value,
    pub id: u32,
}

/// failure response
#[derive(Debug, PartialEq, Clone, Deserialize)]
pub struct Failure {
    pub jsonrpc: Option<String>,
    pub error: JsonRpcError,
    pub id: u32,
}

#[derive(Debug, PartialEq, Clone, Deserialize)]
#[serde(untagged)]
pub enum RpcResponse {
    /// Success
    Success(Success),
    /// Failure
    Failure(Failure),
}

impl RpcResponse {
    fn parse<R: serde::de::DeserializeOwned>(self) -> Result<R, Error> {
        match self {
            RpcResponse::Success(s) => serde_json::from_value(s.result).map_err(Error::new_parse),
            RpcResponse::Failure(f) => Err(Error::new_jsonrpc(f.error)),
        }
    }
}

impl Rpc {
    pub fn new(url: Uri) -> Rpc {
        let (sender, receiver) = mpsc::channel(65_535);
        let (stop, stop_rx) = oneshot::channel::<()>();

        let thread = thread::spawn(move || {
            let client = Client::builder().keep_alive(true).build_http();

            let stream = receiver.for_each(move |(sender, method, params): RpcRequest| {
                let req_json = format!(
                    "{{\"id\": 2, \"jsonrpc\": \"2.0\", \"method\": \"{}\",\"params\": {}}}",
                    method, params
                );

                let req_url = url.clone();
                let mut req = Request::new(Body::from(req_json));
                *req.method_mut() = Method::POST;
                *req.uri_mut() = req_url;
                req.headers_mut()
                    .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

                let request = client
                    .request(req)
                    .and_then(|res| res.into_body().concat2())
                    .then(|res| sender.send(res))
                    .map_err(|_| ());

                rt::spawn(request);
                Ok(())
            });

            rt::run(stream.select2(stop_rx).map(|_| ()).map_err(|_| ()));
        });

        Rpc {
            sender,
            thread: Some(thread),
            stop: Some(stop),
        }
    }

    pub fn request(
        &self,
        method: String,
        params: String,
    ) -> impl Future<Item = Chunk, Error = Error> {
        let (req, res) = oneshot::channel();
        let req = (req, method, params);

        let mut sender = self.sender.clone();
        let _ = sender.try_send(req);
        res.map_err(Error::new_future)
            .and_then(|c| c.map_err(Error::new_hyper))
    }

    pub fn get_tip_header(&self) -> impl Future<Item = Header, Error = Error> {
        self.request("get_tip_header".to_owned(), "null".to_owned())
            .and_then(|chunk| {
                serde_json::from_slice(&chunk)
                    .map_err(Error::new_parse)
                    .and_then(|res: RpcResponse| res.parse())
            })
    }

    pub fn submit_pow_solution(&self, nonce: u64) -> impl Future<Item = (), Error = Error> {
        self.request("submit_pow_solution".to_owned(), format!("[{}]", nonce))
            .and_then(|chunk| {
                serde_json::from_slice(&chunk)
                    .map_err(Error::new_parse)
                    .and_then(|res: RpcResponse| res.parse())
            })
    }
}

impl Drop for Rpc {
    fn drop(&mut self) {
        let _ = self.stop.take().expect("rpc wasn't running").send(());
        let _ = self.thread.take().expect("rpc wasn't running").join();
    }
}
