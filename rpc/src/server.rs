use crate::IoHandler;
use axum::routing::post;
use axum::{Extension, Router};
use ckb_notify::NotifyController;
use futures_util::{SinkExt, TryStreamExt};
use jsonrpc_utils::axum_utils::{handle_jsonrpc, handle_jsonrpc_ws};
use jsonrpc_utils::pub_sub::Session;
use jsonrpc_utils::stream::{serve_stream_sink, StreamMsg, StreamServerConfig};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec, LinesCodecError};
use tower_http::timeout::TimeoutLayer;

#[doc(hidden)]
pub struct RpcServer {}

impl RpcServer {
    /// Creates an RPC server.
    ///
    /// ## Parameters
    ///
    /// * `config` - RPC config options.
    /// * `io_handler` - RPC methods handler. See [ServiceBuilder](../service_builder/struct.ServiceBuilder.html).
    /// * `notify_controller` - Controller emitting notifications.
    pub async fn start_jsonrpc_server(
        io_handler: IoHandler,
        _notify_controller: &NotifyController,
        _handle: Handle,
    ) -> Result<(), String> {
        let rpc = Arc::new(io_handler);
        let stream_config = StreamServerConfig::default()
            .with_channel_size(4)
            .with_pipeline_size(4);

        // HTTP and WS server.
        let method_router =
            post(handle_jsonrpc::<Option<Session>>).get(handle_jsonrpc_ws::<Option<Session>>);
        let ws_config = stream_config.clone().with_keep_alive(true);
        let app = Router::new()
            .route("/", method_router.clone())
            .route("/*path", method_router)
            .layer(Extension(rpc.clone()))
            .layer(Extension(ws_config))
            .layer(TimeoutLayer::new(Duration::from_secs(30)));

        // You can use additional tower-http middlewares to add e.g. CORS.
        let _http = tokio::spawn(async move {
            axum::Server::bind(&"0.0.0.0:8114".parse().unwrap())
                .serve(app.into_make_service())
                .await
                .unwrap();
        });

        eprintln!("started http ...........");

        // TCP server.

        // TCP server with line delimited json codec.
        //
        // You can also use other transports (e.g. TLS, unix socket) and codecs
        // (e.g. netstring, JSON splitter).
        let listener = TcpListener::bind("0.0.0.0:8116").await.unwrap();
        let codec = LinesCodec::new_with_max_length(2 * 1024 * 1024);
        while let Ok((s, _)) = listener.accept().await {
            let rpc = rpc.clone();
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

        Ok(())
    }
}
