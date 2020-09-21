use ckb_app_config::{ExitCode, Setup};
use ckb_build_info::Version;
use ckb_debug_console::{self, Guard as DebugConsoleInitGuard};
use ckb_logger::info_target;
use ckb_logger_service::{self, LoggerInitGuard};
use ckb_metrics_service::{self, Guard as MetricsInitGuard};

pub struct SetupGuard {
    _logger_guard: LoggerInitGuard,
    _sentry_guard: Option<sentry::internals::ClientInitGuard>,
    _metrics_guard: MetricsInitGuard,
    _debug_console_guard: DebugConsoleInitGuard,
}

impl SetupGuard {
    pub(crate) fn from_setup(setup: &Setup, version: &Version) -> Result<Self, ExitCode> {
        // Initialization of logger must do before sentry, since `logger::init()` and
        // `sentry_config::init()` both registers custom panic hooks, but `logger::init()`
        // replaces all hooks previously registered.
        let mut logger_config = setup.config.logger().to_owned();
        if logger_config.emit_sentry_breadcrumbs.is_none() {
            logger_config.emit_sentry_breadcrumbs = Some(setup.is_sentry_enabled);
        }
        let logger_guard = ckb_logger_service::init(logger_config)?;

        let sentry_guard = if setup.is_sentry_enabled {
            let sentry_config = setup.config.sentry();

            info_target!(
                crate::LOG_TARGET_SENTRY,
                "**Notice**: \
                 The ckb process will send stack trace to sentry on Rust panics. \
                 This is enabled by default before mainnet, which can be opted out by setting \
                 the option `dsn` to empty in the config file. The DSN is now {}",
                sentry_config.dsn
            );

            let guard = sentry_config.init(&version);

            sentry::configure_scope(|scope| {
                scope.set_tag("subcommand", &setup.subcommand_name);
            });

            Some(guard)
        } else {
            info_target!(crate::LOG_TARGET_SENTRY, "sentry is disabled");
            None
        };

        let metrics_config = setup.config.metrics().to_owned();
        let metrics_guard = ckb_metrics_service::init(metrics_config).map_err(|err| {
            eprintln!("Config Error: {:?}", err);
            ExitCode::Config
        })?;

        let debug_console_config = setup.config.debug_console().cloned();
        let debug_console_guard = ckb_debug_console::init(debug_console_config).map_err(|err| {
            eprintln!("DebugConsole error: {:?}", err);
            ExitCode::Failure
        })?;

        Ok(Self {
            _logger_guard: logger_guard,
            _sentry_guard: sentry_guard,
            _metrics_guard: metrics_guard,
            _debug_console_guard: debug_console_guard,
        })
    }
}
