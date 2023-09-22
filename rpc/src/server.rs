
use crate::IoHandler;
use axum::routing::{get, post};
use axum::{Extension, Router};
use ckb_app_config::RpcConfig;
use ckb_logger::info;
use ckb_notify::NotifyController;
use futures_util::{SinkExt, TryStreamExt};
use jsonrpc_utils::axum_utils::{handle_jsonrpc, handle_jsonrpc_ws};
use jsonrpc_utils::pub_sub::Session;
use jsonrpc_utils::stream::{serve_stream_sink, StreamMsg, StreamServerConfig};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec, LinesCodecError};
use tower_http::timeout::TimeoutLayer;

#[doc(hidden)]
#[derive(Debug)]
pub struct RpcServer {
    pub http_address: SocketAddr,
    pub tcp_address: Option<SocketAddr>,
    pub ws_address: Option<SocketAddr>,
}

impl RpcServer {
    /// Creates an RPC server.
    ///
    /// ## Parameters
    ///
    /// * `config` - RPC config options.
    /// * `io_handler` - RPC methods handler. See [ServiceBuilder](../service_builder/struct.ServiceBuilder.html).
    /// * `notify_controller` - Controller emitting notifications.
    pub async fn new(
        config: RpcConfig,
        io_handler: IoHandler,
        _handle: Handle,
        _notify_controller: &NotifyController,
    ) -> Self {
        let rpc = Arc::new(io_handler);
        let stream_config = StreamServerConfig::default()
            .with_channel_size(4)
            .with_pipeline_size(4);

        // HTTP and WS server.
        let method_router =
            post(handle_jsonrpc::<Option<Session>>).get(handle_jsonrpc_ws::<Option<Session>>);
        let app = Router::new()
            .route("/", method_router.clone())
            .route("/*path", method_router)
            .layer(Extension(Arc::clone(&rpc)))
            .layer(TimeoutLayer::new(Duration::from_secs(30)));

        // You can use additional tower-http middlewares to add e.g. CORS.
        let incomming = axum::Server::bind(
            &config
                .listen_address
                .clone()
                .to_socket_addrs()
                .expect("config listen_address parsed")
                .next()
                .expect("config listen_address parsed"),
        )
        .serve(app.into_make_service());

        let local_addr = incomming.local_addr();
        let http_address = local_addr;
        let _http = tokio::spawn(async move { (local_addr, incomming.await.unwrap()) });
        info!("Listen HTTP RPCServer on address {}", local_addr);

        let mut ws_address = None;
        if let Some(addr) = config.ws_listen_address {
            let ws_config = stream_config.clone().with_keep_alive(true);
            let method_router = get(handle_jsonrpc_ws::<Option<Session>>);
            let app = Router::new()
                .route("/", method_router.clone())
                .route("/*path", method_router)
                .layer(Extension(Arc::clone(&rpc)))
                .layer(Extension(ws_config))
                .layer(TimeoutLayer::new(Duration::from_secs(30)));
            let incomming = axum::Server::bind(
                &addr
                    .to_socket_addrs()
                    .expect("config ws_listen_address parsed")
                    .next()
                    .expect("config ws_listen_address parsed"),
            )
            .serve(app.into_make_service());

            let local_addr = incomming.local_addr();
            ws_address = Some(local_addr);
            let _ws = tokio::spawn(async move { (local_addr, incomming.await.unwrap()) });
            info!("Listen WebSocket RPCServer on address {}", local_addr);
        }

        // TCP server.

        // TCP server with line delimited json codec.
        //
        // You can also use other transports (e.g. TLS, unix socket) and codecs
        // (e.g. netstring, JSON splitter).
        let mut tcp_address = None;
        if let Some(tcp_listen_address) = config.tcp_listen_address {
            let listener = TcpListener::bind(tcp_listen_address).await.unwrap();
            tcp_address = listener.local_addr().ok();
            info!("listen TCP RPCServer on address {:?}", tcp_address.unwrap());
            tokio::spawn(async move {
                let codec = LinesCodec::new_with_max_length(2 * 1024 * 1024);
                while let Ok((s, _)) = listener.accept().await {
                    let rpc = Arc::clone(&rpc);
                    let stream_config = stream_config.clone();
                    let codec = codec.clone();
                    tokio::spawn(async move {
                        let (r, w) = s.into_split();
                        let r = FramedRead::new(r, codec.clone()).map_ok(StreamMsg::Str);
                        let w = FramedWrite::new(w, codec).with(|msg| async move {
                            Ok::<_, LinesCodecError>(match msg {
                                StreamMsg::Str(msg) => msg,
                                _ => "".into(),
                            })
                        });
                        tokio::pin!(w);
                        drop(serve_stream_sink(&rpc, w, r, stream_config).await);
                    });
                }
            });
        }

        Self {
            http_address,
            tcp_address,
            ws_address,
        }
    }
}
