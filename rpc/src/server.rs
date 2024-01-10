use crate::IoHandler;
use axum::routing::post;
use axum::{Extension, Router};
use ckb_app_config::RpcConfig;
use ckb_async_runtime::Handle;
use ckb_error::AnyError;
use ckb_logger::info;

use ckb_stop_handler::{new_tokio_exit_rx, CancellationToken};
use futures_util::{SinkExt, TryStreamExt};
use jsonrpc_core::MetaIoHandler;
use jsonrpc_utils::axum_utils::{handle_jsonrpc, handle_jsonrpc_ws};
use jsonrpc_utils::pub_sub::Session;
use jsonrpc_utils::stream::{serve_stream_sink, StreamMsg, StreamServerConfig};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
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
    /// * `handler` - Tokio runtime handle.
    pub fn new(config: RpcConfig, io_handler: IoHandler, handler: Handle) -> Self {
        let rpc = Arc::new(io_handler);

        let http_address = Self::start_server(
            &rpc,
            config.listen_address.to_owned(),
            handler.clone(),
            false,
        )
        .map(|local_addr| {
            info!("Listen HTTP RPCServer on address: {}", local_addr);
            local_addr
        })
        .unwrap();

        let ws_address = if let Some(addr) = config.ws_listen_address {
            let local_addr = Self::start_server(&rpc, addr, handler.clone(), true).map(|addr| {
                info!("Listen WebSocket RPCServer on address: {}", addr);
                addr
            });
            local_addr.ok()
        } else {
            None
        };

        let tcp_address = if let Some(addr) = config.tcp_listen_address {
            let local_addr = handler.block_on(Self::start_tcp_server(rpc, addr));
            if let Ok(addr) = &local_addr {
                info!("Listen TCP RPCServer on address: {}", addr);
            };
            local_addr.ok()
        } else {
            None
        };

        Self {
            http_address,
            tcp_address,
            ws_address,
        }
    }

    fn start_server(
        rpc: &Arc<MetaIoHandler<Option<Session>>>,
        address: String,
        handler: Handle,
        enable_websocket: bool,
    ) -> Result<SocketAddr, AnyError> {
        let stream_config = StreamServerConfig::default()
            .with_channel_size(4)
            .with_pipeline_size(4);

        // HTTP and WS server.
        let method_router =
            post(handle_jsonrpc::<Option<Session>>).get(handle_jsonrpc_ws::<Option<Session>>);
        let mut app = Router::new()
            .route("/", method_router.clone())
            .route("/*path", method_router)
            .layer(Extension(Arc::clone(rpc)))
            .layer(TimeoutLayer::new(Duration::from_secs(30)));

        if enable_websocket {
            let ws_config: StreamServerConfig =
                stream_config
                    .with_keep_alive(true)
                    .with_shutdown(async move {
                        new_tokio_exit_rx().cancelled().await;
                    });
            app = app.layer(Extension(ws_config));
        }

        let (tx_addr, rx_addr) = tokio::sync::oneshot::channel::<SocketAddr>();

        handler.spawn(async move {
            let server = axum::Server::bind(
                &address
                    .to_socket_addrs()
                    .expect("config listen_address parsed")
                    .next()
                    .expect("config listen_address parsed"),
            )
            .serve(app.clone().into_make_service());

            let _ = tx_addr.send(server.local_addr());
            let graceful = server.with_graceful_shutdown(async move {
                new_tokio_exit_rx().cancelled().await;
            });
            drop(graceful.await);
        });

        let rx_addr = handler.block_on(rx_addr)?;
        Ok(rx_addr)
    }

    async fn start_tcp_server(
        rpc: Arc<MetaIoHandler<Option<Session>>>,
        tcp_listen_address: String,
    ) -> Result<SocketAddr, AnyError> {
        // TCP server with line delimited json codec.
        let listener = TcpListener::bind(tcp_listen_address).await?;
        let tcp_address = listener.local_addr()?;
        tokio::spawn(async move {
            let codec = LinesCodec::new_with_max_length(2 * 1024 * 1024);
            let stream_config = StreamServerConfig::default()
                .with_channel_size(4)
                .with_pipeline_size(4)
                .with_shutdown(async move {
                    new_tokio_exit_rx().cancelled().await;
                });

            let exit_signal: CancellationToken = new_tokio_exit_rx();
            tokio::select! {
                _ = async {
                        while let Ok((stream, _)) = listener.accept().await {
                            let rpc = Arc::clone(&rpc);
                            let stream_config = stream_config.clone();
                            let codec = codec.clone();
                            tokio::spawn(async move {
                                let (r, w) = stream.into_split();
                                let r = FramedRead::new(r, codec.clone()).map_ok(StreamMsg::Str);
                                let w = FramedWrite::new(w, codec).with(|msg| async move {
                                    Ok::<_, LinesCodecError>(match msg {
                                        StreamMsg::Str(msg) => msg,
                                        _ => "".into(),
                                    })
                                });
                                tokio::pin!(w);
                                if let Err(err) = serve_stream_sink(&rpc, w, r, stream_config).await {
                                    info!("TCP RPCServer error: {:?}", err);
                                }
                            });
                        }
                    } => {},
                _ = exit_signal.cancelled() => {
                    info!("TCP RPCServer stopped");
                }
            }
        });
        Ok(tcp_address)
    }
}
