use build_info::{get_version, Version};
use log::info;
use serde_derive::Deserialize;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SentryConfig {
    pub dsn: String,
}

impl SentryConfig {
    pub fn init(self) -> sentry::internals::ClientInitGuard {
        let guard = sentry::init(&self);
        if guard.is_enabled() {
            sentry::integrations::panic::register_panic_handler();
            info!(target: "sentry", "**Notice**: \
                The ckb process will send stack trace to sentry on Rust panics. \
                This is enabled by default before mainnet, which can be opted out by setting \
                the option `dsn` to empty in the config file. The DSN is now {}", self.dsn);
        } else {
            info!(target: "sentry", "sentry is disabled");
        }

        guard
    }
}

impl<'a> Into<sentry::ClientOptions> for &'a SentryConfig {
    fn into(self) -> sentry::ClientOptions {
        let version = get_version!();

        sentry::ClientOptions {
            dsn: self.dsn.parse().ok(),
            release: Some(version.long().into()),
            ..Default::default()
        }
    }
}
