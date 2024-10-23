//! The service which handles the metrics data in CKB.

use std::net::SocketAddr;

use http_body_util::Full;
use hyper::{
    body::Bytes, header::CONTENT_TYPE, service::service_fn, Error as HyperError, Method, Request,
    Response,
};
use hyper_util::{
    rt::TokioExecutor,
    server::{conn::auto, graceful::GracefulShutdown},
};
use prometheus::Encoder as _;
use tokio::net::TcpListener;

use ckb_async_runtime::Handle;
use ckb_logger::info;
use ckb_metrics_config::{Config, Exporter, Target};
use ckb_stop_handler::{new_tokio_exit_rx, CancellationToken};
use ckb_util::strings;

/// Ensures the metrics service can shutdown gracefully.
#[must_use]
pub enum Guard {
    /// The metrics service is disabled.
    Off,
    /// The metrics service is enabled.
    On,
}

/// Initializes the metrics service and lets it run in the background.
///
/// Returns [Guard](enum.Guard.html) if succeeded, or an `String` to describes the reason for the failure.
pub fn init(config: Config, handle: Handle) -> Result<Guard, String> {
    if config.exporter.is_empty() {
        let _ignored = ckb_metrics::METRICS_SERVICE_ENABLED.set(false);
        return Ok(Guard::Off);
    }

    for (name, exporter) in config.exporter {
        check_exporter_name(&name)?;
        run_exporter(exporter, &handle)?;
    }
    // The .set() method's return value can indicate whether the value has set or not.
    // Just ignore its return value
    // I don't care this because CKB only initializes the ckb-metrics-service once.
    let _ignored = ckb_metrics::METRICS_SERVICE_ENABLED.set(true);

    Ok(Guard::On)
}

fn check_exporter_name(name: &str) -> Result<(), String> {
    strings::check_if_identifier_is_valid(name)
}

fn run_exporter(exporter: Exporter, handle: &Handle) -> Result<(), String> {
    let Exporter { target } = exporter;
    match target {
        Target::Prometheus { listen_address } => {
            let addr = listen_address
                .parse::<SocketAddr>()
                .map_err(|err| format!("failed to parse listen_address because {err}"))?;
            let make_svc = service_fn(start_prometheus_service);
            ckb_logger::info!("Start prometheus exporter at {}", addr);
            handle.spawn(async move {
                let listener = TcpListener::bind(&addr).await.unwrap();
                let server = auto::Builder::new(TokioExecutor::new());
                let graceful = GracefulShutdown::new();
                let stop_rx: CancellationToken = new_tokio_exit_rx();
                loop {
                    tokio::select! {
                        conn = listener.accept() => {
                            let (stream, _) = match conn {
                                Ok(conn) => conn,
                                Err(e) => {
                                    eprintln!("accept error: {}", e);
                                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                                    continue;
                                }
                            };
                            let stream = hyper_util::rt::TokioIo::new(Box::pin(stream));
                            let conn = server.serve_connection_with_upgrades(stream, make_svc);

                            let conn = graceful.watch(conn.into_owned());
                            tokio::spawn(async move {
                                if let Err(err) = conn.await {
                                    info!("connection error: {}", err);
                                }
                            });
                        },
                        _ = stop_rx.cancelled() => {
                            info!("Prometheus server received exit signal; exit now");
                            break;
                        }
                    }
                }
                drop(listener);
                graceful.shutdown().await;
            });
        }
    }
    Ok(())
}

async fn start_prometheus_service(
    req: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, HyperError> {
    Ok(match (req.method(), req.uri().path()) {
        (&Method::GET, "/") => {
            let mut buffer = vec![];
            let encoder = prometheus::TextEncoder::new();
            let metric_families = ckb_metrics::gather();
            encoder.encode(&metric_families, &mut buffer).unwrap();
            Response::builder()
                .status(200)
                .header(CONTENT_TYPE, encoder.format_type())
                .body(Full::new(Bytes::from(buffer)))
        }
        _ => Response::builder()
            .status(404)
            .body(Full::from("Page Not Found")),
    }
    .unwrap())
}
