use build_info::{get_version, Version};
use log::info;
use serde_derive::Deserialize;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SentryConfig {
    pub dsn: String,
}

impl SentryConfig {
    pub fn init(&self) -> sentry::internals::ClientInitGuard {
        let version = get_version!();
        let guard = sentry::init(self.build_sentry_client_options(&version));
        if guard.is_enabled() {
            sentry::configure_scope(|scope| {
                scope.set_tag("release.pre", version.is_pre());
                scope.set_tag("release.dirty", version.is_dirty());
            });

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

    fn build_sentry_client_options(&self, version: &Version) -> sentry::ClientOptions {
        sentry::ClientOptions {
            dsn: self.dsn.parse().ok(),
            release: Some(version.long().into()),
            ..Default::default()
        }
    }
}
