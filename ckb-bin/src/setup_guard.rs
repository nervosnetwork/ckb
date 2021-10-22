use ckb_app_config::{ExitCode, Setup};
use ckb_async_runtime::Handle;
use ckb_build_info::Version;
use ckb_logger_service::{self, LoggerInitGuard};
use ckb_metrics_service::{self, Guard as MetricsInitGuard};

const CKB_LOG_ENV: &str = "CKB_LOG";

pub struct SetupGuard {
    _logger_guard: LoggerInitGuard,
    #[cfg(feature = "with_sentry")]
    _sentry_guard: Option<sentry::ClientInitGuard>,
    _metrics_guard: MetricsInitGuard,
}

impl SetupGuard {
    #[cfg(feature = "with_sentry")]
    pub(crate) fn from_setup(
        setup: &Setup,
        version: &Version,
        async_handle: Handle,
        silent_logging: bool,
    ) -> Result<Self, ExitCode> {
        // Initialization of logger must do before sentry, since `logger::init()` and
        // `sentry_config::init()` both registers custom panic hooks, but `logger::init()`
        // replaces all hooks previously registered.
        let logger_guard = if silent_logging {
            ckb_logger_service::init_silent()?
        } else {
            let mut logger_config = setup.config.logger().to_owned();
            if logger_config.emit_sentry_breadcrumbs.is_none() {
                logger_config.emit_sentry_breadcrumbs = Some(setup.is_sentry_enabled);
            }
            ckb_logger_service::init(Some(CKB_LOG_ENV), logger_config)?
        };

        let sentry_guard = if setup.is_sentry_enabled {
            let sentry_config = setup.config.sentry();

            ckb_logger::info_target!(
                crate::LOG_TARGET_SENTRY,
                "**Notice**: \
                 The ckb process will send stack trace to sentry on Rust panics. \
                 This is enabled by default before mainnet, which can be opted out by setting \
                 the option `dsn` to empty in the config file. The DSN is now {}",
                sentry_config.dsn
            );

            let guard = sentry_config.init(version);

            sentry::configure_scope(|scope| {
                scope.set_tag("subcommand", &setup.subcommand_name);
            });

            Some(guard)
        } else {
            ckb_logger::info_target!(crate::LOG_TARGET_SENTRY, "sentry is disabled");
            None
        };

        let metrics_config = setup.config.metrics().to_owned();
        let metrics_guard =
            ckb_metrics_service::init(metrics_config, async_handle).map_err(|err| {
                eprintln!("Config Error: {:?}", err);
                ExitCode::Config
            })?;

        Ok(Self {
            _logger_guard: logger_guard,
            _sentry_guard: sentry_guard,
            _metrics_guard: metrics_guard,
        })
    }

    #[cfg(not(feature = "with_sentry"))]
    pub(crate) fn from_setup(
        setup: &Setup,
        _version: &Version,
        async_handle: Handle,
        silent_logging: bool,
    ) -> Result<Self, ExitCode> {
        let logger_guard = if silent_logging {
            ckb_logger_service::init_silent()?
        } else {
            let logger_config = setup.config.logger().to_owned();
            ckb_logger_service::init(Some(CKB_LOG_ENV), logger_config)?
        };

        let metrics_config = setup.config.metrics().to_owned();
        let metrics_guard =
            ckb_metrics_service::init(metrics_config, async_handle).map_err(|err| {
                eprintln!("Config Error: {:?}", err);
                ExitCode::Config
            })?;

        Ok(Self {
            _logger_guard: logger_guard,
            _metrics_guard: metrics_guard,
        })
    }
}
