use crate::IoHandler;
use ckb_app_config::RpcConfig;
use ckb_notify::NotifyController;
use futures_util::{SinkExt, TryStreamExt};
use jsonrpc_core::MetaIoHandler;
use jsonrpc_utils::axum_utils::jsonrpc_router;
use jsonrpc_utils::stream::{serve_stream_sink, StreamMsg, StreamServerConfig};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec, LinesCodecError};

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
        config: RpcConfig,
        io_handler: IoHandler,
        notify_controller: &NotifyController,
        handle: Handle,
    ) -> Result<(), String> {
        let rpc = MetaIoHandler::with_compatibility(jsonrpc_core::Compatibility::V2);

        let rpc = Arc::new(rpc);
        let stream_config = StreamServerConfig::default()
            .with_channel_size(4)
            .with_pipeline_size(4);

        // HTTP and WS server.
        let ws_config = stream_config.clone().with_keep_alive(true);
        let app = jsonrpc_router("/", rpc.clone(), ws_config);
        // You can use additional tower-http middlewares to add e.g. CORS.
        let http = tokio::spawn(async move {
            axum::Server::bind(&"0.0.0.0:3000".parse().unwrap())
                .serve(app.into_make_service())
                .await
                .unwrap();
        });

        // TCP server.

        // TCP server with line delimited json codec.
        //
        // You can also use other transports (e.g. TLS, unix socket) and codecs
        // (e.g. netstring, JSON splitter).
        let listener = TcpListener::bind("0.0.0.0:3001").await.unwrap();
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
