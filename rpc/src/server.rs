use crate::config::Config;
use crate::module::{SubscriptionRpc, SubscriptionRpcImpl, SubscriptionSession};
use crate::IoHandler;
use ckb_notify::NotifyController;
use jsonrpc_http_server;
use jsonrpc_pubsub::Session;
use jsonrpc_server_utils::cors::AccessControlAllowOrigin;
use jsonrpc_server_utils::hosts::DomainsValidation;
use jsonrpc_tcp_server;
use jsonrpc_ws_server;
use std::{collections::HashSet, net::ToSocketAddrs};

pub struct RpcServer {
    pub(crate) http: jsonrpc_http_server::Server,
    pub(crate) tcp: Option<jsonrpc_tcp_server::Server>,
    pub(crate) ws: Option<jsonrpc_ws_server::Server>,
}

impl RpcServer {
    pub fn new(
        config: Config,
        io_handler: IoHandler,
        notify_controller: &NotifyController,
    ) -> RpcServer {
        let http = jsonrpc_http_server::ServerBuilder::new(io_handler.clone())
            .cors(DomainsValidation::AllowOnly(vec![
                AccessControlAllowOrigin::Null,
                AccessControlAllowOrigin::Any,
            ]))
            .threads(config.threads.unwrap_or_else(num_cpus::get))
            .max_request_body_size(config.max_request_body_size)
            .health_api(("/ping", "ping"))
            .start_http(
                &config
                    .listen_address
                    .to_socket_addrs()
                    .expect("config listen_address parsed")
                    .next()
                    .expect("config listen_address parsed"),
            )
            .expect("Start Jsonrpc HTTP service");

        let tcp = config
            .tcp_listen_address
            .as_ref()
            .map(|tcp_listen_address| {
                let subscription_rpc_impl =
                    SubscriptionRpcImpl::new(notify_controller.clone(), Some("TcpSubscription"));
                let mut handler = io_handler.clone();
                if config.subscription_enable() {
                    handler.extend_with(subscription_rpc_impl.to_delegate());
                }
                jsonrpc_tcp_server::ServerBuilder::with_meta_extractor(
                    handler,
                    |context: &jsonrpc_tcp_server::RequestContext| {
                        Some(SubscriptionSession::new(Session::new(
                            context.sender.clone(),
                        )))
                    },
                )
                .start(
                    &tcp_listen_address
                        .to_socket_addrs()
                        .expect("config tcp_listen_address parsed")
                        .next()
                        .expect("config tcp_listen_address parsed"),
                )
                .expect("Start Jsonrpc TCP service")
            });

        let ws = config.ws_listen_address.as_ref().map(|ws_listen_address| {
            let subscription_rpc_impl =
                SubscriptionRpcImpl::new(notify_controller.clone(), Some("WsSubscription"));
            let mut handler = io_handler.clone();
            if config.subscription_enable() {
                handler.extend_with(subscription_rpc_impl.to_delegate());
            }
            jsonrpc_ws_server::ServerBuilder::with_meta_extractor(
                handler,
                |context: &jsonrpc_ws_server::RequestContext| {
                    Some(SubscriptionSession::new(Session::new(context.sender())))
                },
            )
            .start(
                &ws_listen_address
                    .to_socket_addrs()
                    .expect("config ws_listen_address parsed")
                    .next()
                    .expect("config ws_listen_address parsed"),
            )
            .expect("Start Jsonrpc WebSocket service")
        });

        RpcServer { http, tcp, ws }
    }

    pub fn close(self) {
        self.http.close();
        if let Some(tcp) = self.tcp {
            tcp.close();
        }
        if let Some(ws) = self.ws {
            ws.close();
        }
    }
}

use crate::error::RPCError;
use futures::{future::ok, future::Either, Async, Future, Poll};
use jsonrpc_core::{middleware::Middleware, Call, Id, Metadata, Output, Request, Response};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct ModuleEnableCheck(Arc<HashMap<String, Arc<String>>>);

impl ModuleEnableCheck {
    pub fn new(set: HashMap<String, Arc<String>>) -> Self {
        ModuleEnableCheck(Arc::new(set))
    }
}

/// Dummy future used as a noop result of middleware.
pub type NoopFuture = Box<dyn Future<Item = Option<Response>, Error = ()> + Send>;
/// Dummy future used as a noop call result of middleware.
pub type NoopCallFuture = Box<dyn Future<Item = Option<Output>, Error = ()> + Send>;

impl<M: Metadata> Middleware<M> for ModuleEnableCheck {
    type Future = NoopFuture;
    type CallFuture = NoopCallFuture;

    fn on_request<F, X>(&self, request: Request, meta: M, next: F) -> Either<Self::Future, X>
    where
        F: Fn(Request, M) -> X + Send + Sync,
        X: Future<Item = Option<Response>, Error = ()> + Send + 'static,
    {
        let output = |method: &jsonrpc_core::types::MethodCall, module: &str| {
            Output::from(
                Err(RPCError::custom(
                    RPCError::Invalid,
                    format!(
                        "You need to enable `{module}` module to invoke `{method}` rpc, \
                        please modify `rpc.modules` {miner_info} of configuration file ckb.toml and restart the ckb node",
                        method = method.method, module = module, miner_info = if module == "Miner" {"and `block_assembler`"} else {""}
                    ),
                )),
                method.id.clone(),
                method.jsonrpc,
            )
        };
        match request {
            Request::Single(Call::MethodCall(method)) => {
                if let Some(module) = self.0.get(&method.method) {
                    Either::A(Box::new(ok::<_, ()>(Some(Response::Single(output(
                        &method, module,
                    ))))))
                } else {
                    Either::B(next(Request::Single(Call::MethodCall(method)), meta))
                }
            }
            Request::Batch(batch_call) => {
                let mut replace_output = Vec::new();
                let mut ids = HashSet::new();
                for call in batch_call.iter() {
                    match call {
                        Call::MethodCall(method) => {
                            if let Some(module) = self.0.get(&method.method) {
                                replace_output.push(output(&method, module));
                                ids.insert(method.id.clone());
                            }
                        }
                        _ => continue,
                    }
                }
                if replace_output.is_empty() {
                    Either::B(next(Request::Batch(batch_call), meta))
                } else {
                    Either::A(Box::new(FutureResponse::new(
                        next(Request::Batch(batch_call), meta),
                        replace_output,
                        ids,
                    )))
                }
            }
            _ => Either::B(next(request, meta)),
        }
    }
}

struct FutureResponse<T> {
    next: T,
    replace_output: Vec<Output>,
    replace_ids: HashSet<Id>,
}

impl<T> FutureResponse<T>
where
    T: Future<Item = Option<Response>, Error = ()> + Send + 'static,
{
    fn new(next: T, replace_output: Vec<Output>, replace_ids: HashSet<Id>) -> Self {
        Self {
            next,
            replace_output,
            replace_ids,
        }
    }
}

impl<T> Future for FutureResponse<T>
where
    T: Future<Item = Option<Response>, Error = ()> + Send + 'static,
{
    type Item = Option<Response>;
    type Error = ();
    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.next.poll() {
            Ok(Async::Ready(res)) => Ok(Async::Ready(res.and_then(|response| match response {
                Response::Batch(list) => {
                    let mut res = list
                        .into_iter()
                        .filter(|out| !self.replace_ids.contains(out.id()))
                        .collect::<Vec<Output>>();
                    res.extend(self.replace_output.drain(..));
                    Some(Response::Batch(res))
                }
                raw => Some(raw),
            }))),
            res => res,
        }
    }
}
