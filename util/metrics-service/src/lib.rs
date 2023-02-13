//! The service which handles the metrics data in CKB.

use std::{convert::Infallible, net::SocketAddr, sync::Arc};

use hyper::{
    header::CONTENT_TYPE,
    service::{make_service_fn, service_fn},
    Body, Error as HyperError, Method, Request, Response, Server,
};
use opentelemetry_prometheus::PrometheusExporter;
use prometheus::Encoder as _;

use ckb_async_runtime::Handle;
use ckb_metrics_config::{Config, Exporter, Target};
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
        return Ok(Guard::Off);
    }

    for (name, exporter) in config.exporter {
        check_exporter_name(&name)?;
        run_exporter(exporter, &handle)?;
    }

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
            // TODO Not allow to configure the prometheus exporter, since the API is not stable.
            // If anyone who want to customize the configurations, update the follow code.
            // Ref: https://docs.rs/opentelemetry-prometheus/*/opentelemetry_prometheus/struct.ExporterBuilder.html
            let exporter = {
                let exporter = opentelemetry_prometheus::exporter()
                    .try_init()
                    .map_err(|err| format!("failed to init prometheus exporter because {err}"))?;
                Arc::new(exporter)
            };
            let make_svc = make_service_fn(move |_conn| {
                let exporter = Arc::clone(&exporter);
                async move {
                    Ok::<_, Infallible>(service_fn(move |req| {
                        start_prometheus_service(req, Arc::clone(&exporter))
                    }))
                }
            });
            ckb_logger::info!("start prometheus exporter at {}", addr);
            handle.spawn(async move {
                let server = Server::bind(&addr).serve(make_svc);
                if let Err(err) = server.await {
                    ckb_logger::error!("prometheus server error: {}", err);
                }
            });
        }
    }
    Ok(())
}

async fn start_prometheus_service(
    req: Request<Body>,
    exporter: Arc<PrometheusExporter>,
) -> Result<Response<Body>, HyperError> {
    Ok(match (req.method(), req.uri().path()) {
        (&Method::GET, "/") => {
            let mut buffer = vec![];
            let encoder = prometheus::TextEncoder::new();
            let metric_families = exporter.registry().gather();
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
