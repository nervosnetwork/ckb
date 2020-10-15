//! The service which handles the metrics data in CKB.

use std::{net::SocketAddr, time::Duration};

use metrics_core::Observe;
use metrics_runtime::{
    exporters::{HttpExporter, LogExporter},
    observers::{JsonBuilder, PrometheusBuilder, YamlBuilder},
    Receiver,
};
use tokio::sync::oneshot;

use ckb_async_runtime::{new_runtime, Builder, Handle};
use ckb_metrics_config::{Config, Exporter, Format, Target};
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_util::strings;

/// Ensures the metrics service can shutdown gracefully.
#[must_use]
pub enum Guard {
    /// The metrics service is disabled.
    Off,
    /// The metrics service is enabled.
    On {
        #[doc(hidden)]
        handle: Handle,
        #[doc(hidden)]
        stop: StopHandler<()>,
    },
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Self::On { ref mut stop, .. } = self {
            stop.try_send();
        }
    }
}

/// Initializes the metrics service and lets it run in the background.
///
/// Returns [Guard](enum.Guard.html) if succeeded, or an `String` to describes the reason for the failure.
pub fn init(config: Config) -> Result<Guard, String> {
    if config.exporter.is_empty() {
        return Ok(Guard::Off);
    }

    let mut runtime_builder = Builder::new();
    runtime_builder
        .threaded_scheduler()
        .enable_io()
        .enable_time();
    if config.threads != 0 {
        runtime_builder.core_threads(config.threads);
    } else {
        runtime_builder.core_threads(2);
    };
    let (signal_sender, mut signal_receiver) = oneshot::channel();
    let service = move |_: Handle| async move {
        loop {
            tokio::select! { _ = &mut signal_receiver => break }
        }
    };
    let (handle, thread) = new_runtime("Metrics", Some(runtime_builder), service);

    let receiver = {
        let histogram_window_secs = if config.histogram_window > 0 {
            config.histogram_window
        } else {
            10
        };
        let histogram_granularity_secs = if config.histogram_granularity > 0 {
            config.histogram_granularity
        } else {
            1
        };
        let upkeep_interval_millis = if config.upkeep_interval > 0 {
            config.upkeep_interval
        } else {
            50
        };
        let histogram_window = Duration::from_secs(histogram_window_secs);
        let histogram_granularity = Duration::from_secs(histogram_granularity_secs);
        let upkeep_interval = Duration::from_millis(upkeep_interval_millis);
        Receiver::builder()
            .histogram(histogram_window, histogram_granularity)
            .upkeep_interval(upkeep_interval)
    }
    .build()
    .unwrap();
    let controller = receiver.controller();

    for (name, exporter) in config.exporter {
        check_exporter_name(&name)?;
        run_exporter(exporter, &handle, controller.clone())?;
    }

    receiver.install();

    let stop = StopHandler::new(SignalSender::Tokio(signal_sender), thread);

    Ok(Guard::On { handle, stop })
}

fn check_exporter_name(name: &str) -> Result<(), String> {
    strings::check_if_identifier_is_valid(name)
}

fn run_exporter<C>(exporter: Exporter, handle: &Handle, c: C) -> Result<(), String>
where
    C: Observe + Sync + Send + 'static,
{
    let Exporter { target, format } = exporter;
    match target {
        Target::Log {
            level: lv,
            interval,
        } => {
            let dur = Duration::from_secs(interval);
            match format {
                Format::Json { pretty } => {
                    let b = JsonBuilder::new().set_pretty_json(pretty);
                    let exporter = LogExporter::new(c, b, lv, dur);
                    handle.spawn(async {
                        tokio::spawn(exporter.async_run());
                    });
                }
                Format::Yaml => {
                    let b = YamlBuilder::new();
                    let exporter = LogExporter::new(c, b, lv, dur);
                    handle.spawn(async {
                        tokio::spawn(exporter.async_run());
                    });
                }
                Format::Prometheus => {
                    let b = PrometheusBuilder::new();
                    let exporter = LogExporter::new(c, b, lv, dur);
                    handle.spawn(async {
                        tokio::spawn(exporter.async_run());
                    });
                }
            };
        }
        Target::Http { listen_address } => {
            let addr = listen_address
                .parse::<SocketAddr>()
                .map_err(|err| format!("failed to parse listen_address because {}", err))?;
            match format {
                Format::Json { pretty } => {
                    let b = JsonBuilder::new().set_pretty_json(pretty);
                    let exporter = HttpExporter::new(c, b, addr);
                    handle.spawn(async {
                        tokio::spawn(exporter.async_run());
                    });
                }
                Format::Yaml => {
                    let b = YamlBuilder::new();
                    let exporter = HttpExporter::new(c, b, addr);
                    handle.spawn(async {
                        tokio::spawn(exporter.async_run());
                    });
                }
                Format::Prometheus => {
                    let b = PrometheusBuilder::new();
                    let exporter = HttpExporter::new(c, b, addr);
                    handle.spawn(async {
                        tokio::spawn(exporter.async_run());
                    });
                }
            };
        }
    }
    Ok(())
}
