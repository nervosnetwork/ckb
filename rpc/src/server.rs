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
use std::net::ToSocketAddrs;

// Wrapper for HTTP and WS servers that makes sure they are properly shut down.
pub(crate) mod waiting {
    pub struct HttpServer(pub jsonrpc_http_server::Server);
    impl HttpServer {
        pub fn close(self) {
            self.0.close_handle().close();
            self.0.wait();
        }
    }

    pub struct WsServer(pub Option<jsonrpc_ws_server::Server>);
    impl WsServer {
        pub fn close(mut self) {
            if let Some(server) = self.0.take() {
                server.close_handle().close();
                let _ = server.wait();
            }
        }
    }
}

pub struct RpcServer {
    pub(crate) http: waiting::HttpServer,
    pub(crate) tcp: Option<jsonrpc_tcp_server::Server>,
    pub(crate) ws: waiting::WsServer,
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

        RpcServer {
            tcp,
            http: waiting::HttpServer(http),
            ws: waiting::WsServer(ws),
        }
    }

    pub fn close(self) {
        self.http.close();
        self.ws.close();
        if let Some(tcp) = self.tcp {
            tcp.close();
        }
    }
}
