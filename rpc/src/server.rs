use crate::config::Config;
use jsonrpc_core::IoHandler;
use jsonrpc_http_server;
use jsonrpc_server_utils::cors::AccessControlAllowOrigin;
use jsonrpc_server_utils::hosts::DomainsValidation;
use jsonrpc_tcp_server;
use jsonrpc_ws_server;
use std::net::ToSocketAddrs;

pub struct RpcServer {
    pub(crate) http: jsonrpc_http_server::Server,
    pub(crate) tcp: Option<jsonrpc_tcp_server::Server>,
    pub(crate) ws: Option<jsonrpc_ws_server::Server>,
}

impl RpcServer {
    pub fn new(config: Config, io_handler: IoHandler) -> RpcServer {
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

        let tcp = config.tcp_listen_address.map(|tcp_listen_address| {
            jsonrpc_tcp_server::ServerBuilder::new(io_handler.clone())
                .start(
                    &tcp_listen_address
                        .to_socket_addrs()
                        .expect("config tcp_listen_address parsed")
                        .next()
                        .expect("config tcp_listen_address parsed"),
                )
                .expect("Start Jsonrpc TCP service")
        });

        let ws = config.ws_listen_address.map(|ws_listen_address| {
            jsonrpc_ws_server::ServerBuilder::new(io_handler.clone())
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
