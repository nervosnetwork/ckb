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
use std::net::{SocketAddr, ToSocketAddrs};

pub struct RpcServer {
    pub(crate) http: jsonrpc_http_server::Server,
    pub(crate) _tcp: Option<jsonrpc_tcp_server::Server>,
    pub(crate) _ws: Option<jsonrpc_ws_server::Server>,
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

        let _tcp = config
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

        let _ws = config.ws_listen_address.as_ref().map(|ws_listen_address| {
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

        RpcServer { http, _tcp, _ws }
    }

    pub fn http_address(&self) -> &SocketAddr {
        self.http.address()
    }
}
