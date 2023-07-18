//! The service which handles the metrics data in CKB.

use std::{convert::Infallible, net::SocketAddr};

use hyper::{
    header::CONTENT_TYPE,
    service::{make_service_fn, service_fn},
    Body, Error as HyperError, Method, Request, Response, Server,
};
use prometheus::Encoder as _;

use ckb_async_runtime::Handle;
use ckb_logger::debug;
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
            let make_svc = make_service_fn(move |_conn| async move {
                Ok::<_, Infallible>(service_fn(start_prometheus_service))
            });
            ckb_logger::info!("start prometheus exporter at {}", addr);
            handle.spawn(async move {
                let server = Server::bind(&addr)
                    .serve(make_svc)
                    .with_graceful_shutdown(async {
                        let exit_rx: CancellationToken = new_tokio_exit_rx();
                        exit_rx.cancelled().await;
                        debug!("prometheus server received exit signal, exit now");
                    });
                if let Err(err) = server.await {
                    ckb_logger::error!("prometheus server error: {}", err);
                }
            });
        }
    }
    Ok(())
}

async fn start_prometheus_service(req: Request<Body>) -> Result<Response<Body>, HyperError> {
    Ok(match (req.method(), req.uri().path()) {
        (&Method::GET, "/") => {
            let mut buffer = vec![];
            let encoder = prometheus::TextEncoder::new();
            let metric_families = ckb_metrics::gather();
            encoder.encode(&metric_families, &mut buffer).unwrap();
            Response::builder()
                .status(200)
                .header(CONTENT_TYPE, encoder.format_type())
                .body(Body::from(buffer))
        }
        _ => Response::builder()
            .status(404)
            .body(Body::from("Page Not Found")),
    }
    .unwrap())
}
