use crate::IoHandler;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Extension, Router};
use ckb_app_config::RpcConfig;
use ckb_async_runtime::Handle;
use ckb_error::AnyError;
use ckb_logger::info;

use axum::{body::Bytes, http::StatusCode, response::Response, Json};

use jsonrpc_core::{MetaIoHandler, Metadata, Request};

use ckb_stop_handler::{new_tokio_exit_rx, CancellationToken};
use futures_util::future;
use futures_util::future::Either::{Left, Right};
use jsonrpc_core::types::error::ErrorCode;
use jsonrpc_core::types::Response as RpcResponse;
use jsonrpc_core::Error;

use futures_util::{SinkExt, TryStreamExt};
use jsonrpc_utils::axum_utils::handle_jsonrpc_ws;
use jsonrpc_utils::pub_sub::Session;
use jsonrpc_utils::stream::{serve_stream_sink, StreamMsg, StreamServerConfig};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec, LinesCodecError};
use tower_http::cors::CorsLayer;
use tower_http::timeout::TimeoutLayer;

static JSONRPC_BATCH_LIMIT: OnceLock<usize> = OnceLock::new();

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
        if let Some(jsonrpc_batch_limit) = config.rpc_batch_limit {
            let _ = JSONRPC_BATCH_LIMIT.get_or_init(|| jsonrpc_batch_limit);
        }

        let rpc = Arc::new(io_handler);

        let http_address = Self::start_server(
            &rpc,
            config.listen_address.to_owned(),
            handler.clone(),
            false,
        )
        .inspect(|&local_addr| {
            info!("Listen HTTP RPCServer on address: {}", local_addr);
        })
        .unwrap();

        let ws_address = if let Some(addr) = config.ws_listen_address {
            let local_addr =
                Self::start_server(&rpc, addr, handler.clone(), true).inspect(|&addr| {
                    info!("Listen WebSocket RPCServer on address: {}", addr);
                });
            local_addr.ok()
        } else {
            None
        };

        let tcp_address = if let Some(addr) = config.tcp_listen_address {
            let local_addr = handler.block_on(Self::start_tcp_server(rpc, addr, handler.clone()));
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
            .with_keep_alive(true)
            .with_pipeline_size(4)
            .with_shutdown(async move {
                new_tokio_exit_rx().cancelled().await;
            });

        // HTTP and WS server.
        let post_router = post(handle_jsonrpc::<Option<Session>>);
        let get_router = if enable_websocket {
            get(handle_jsonrpc_ws::<Option<Session>>)
        } else {
            get(get_error_handler)
        };
        let method_router = post_router.merge(get_router);

        let app = Router::new()
            .route("/", method_router.clone())
            .route("/*path", method_router)
            .route("/ping", get(ping_handler))
            .layer(Extension(Arc::clone(rpc)))
            .layer(CorsLayer::permissive())
            .layer(TimeoutLayer::new(Duration::from_secs(30)))
            .layer(Extension(stream_config));

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
        handler: Handle,
    ) -> Result<SocketAddr, AnyError> {
        // TCP server with line delimited json codec.
        let listener = TcpListener::bind(tcp_listen_address).await?;
        let tcp_address = listener.local_addr()?;
        handler.spawn(async move {
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

/// used for compatible with old health endpoint
async fn ping_handler() -> impl IntoResponse {
    "pong"
}

/// used for compatible with old PRC error response for GET
async fn get_error_handler() -> impl IntoResponse {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        "Used HTTP Method is not allowed. POST or OPTIONS is required",
    )
}

async fn handle_jsonrpc<T: Default + Metadata>(
    Extension(io): Extension<Arc<MetaIoHandler<T>>>,
    req_body: Bytes,
) -> Response {
    let make_error_response = |error| {
        Json(jsonrpc_core::Failure {
            jsonrpc: Some(jsonrpc_core::Version::V2),
            id: jsonrpc_core::Id::Null,
            error,
        })
        .into_response()
    };

    let req = match std::str::from_utf8(req_body.as_ref()) {
        Ok(req) => req,
        Err(_) => {
            return make_error_response(jsonrpc_core::Error::parse_error());
        }
    };

    let req = serde_json::from_str::<Request>(req);
    let result = match req {
        Err(_error) => Left(future::ready(Some(RpcResponse::from(
            Error::new(ErrorCode::ParseError),
            Some(jsonrpc_core::Version::V2),
        )))),
        Ok(request) => {
            if let Request::Batch(ref arr) = request {
                if let Some(batch_size) = JSONRPC_BATCH_LIMIT.get() {
                    if arr.len() > *batch_size {
                        return make_error_response(jsonrpc_core::Error::invalid_params(format!(
                            "batch size is too large, expect it less than: {}",
                            batch_size
                        )));
                    }
                }
            }
            Right(io.handle_rpc_request(request, T::default()))
        }
    };

    if let Some(response) = result.await {
        serde_json::to_string(&response)
            .map(|json| {
                (
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    json,
                )
                    .into_response()
            })
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
    } else {
        StatusCode::NO_CONTENT.into_response()
    }
}
