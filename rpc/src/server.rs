use crate::module::{SubscriptionRpc, SubscriptionRpcImpl, SubscriptionSession};
use crate::IoHandler;
use ckb_app_config::RpcConfig;
use ckb_logger::info;
use ckb_notify::NotifyController;
use jsonrpc_pubsub::Session;
use jsonrpc_server_utils::cors::AccessControlAllowOrigin;
use jsonrpc_server_utils::hosts::DomainsValidation;
use std::net::{SocketAddr, ToSocketAddrs};

#[doc(hidden)]
pub struct RpcServer {
    pub(crate) http: jsonrpc_http_server::Server,
    pub(crate) _tcp: Option<jsonrpc_tcp_server::Server>,
    pub(crate) _ws: Option<jsonrpc_ws_server::Server>,
}

impl RpcServer {
    /// TODO(doc): @doitian
    pub fn new(
        config: RpcConfig,
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
        info!("Listen HTTP RPCServer on address {}", config.listen_address);

        let _tcp = config
            .tcp_listen_address
            .as_ref()
            .map(|tcp_listen_address| {
                let subscription_rpc_impl =
                    SubscriptionRpcImpl::new(notify_controller.clone(), "TcpSubscription");
                let mut handler = io_handler.clone();
                if config.subscription_enable() {
                    handler.extend_with(subscription_rpc_impl.to_delegate());
                }
                let tcp_server = jsonrpc_tcp_server::ServerBuilder::with_meta_extractor(
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
                .expect("Start Jsonrpc TCP service");
                info!("Listen TCP RPCServer on address {}", tcp_listen_address);

                tcp_server
            });

        let _ws = config.ws_listen_address.as_ref().map(|ws_listen_address| {
            let subscription_rpc_impl =
                SubscriptionRpcImpl::new(notify_controller.clone(), "WsSubscription");
            let mut handler = io_handler.clone();
            if config.subscription_enable() {
                handler.extend_with(subscription_rpc_impl.to_delegate());
            }
            let ws_server = jsonrpc_ws_server::ServerBuilder::with_meta_extractor(
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
            .expect("Start Jsonrpc WebSocket service");
            info!("Listen WS RPCServer on address {}", ws_listen_address);

            ws_server
        });

        RpcServer { http, _tcp, _ws }
    }

    /// TODO(doc): @doitian
    pub fn http_address(&self) -> &SocketAddr {
        self.http.address()
    }
}
