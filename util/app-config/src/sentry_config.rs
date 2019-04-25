use build_info::{get_version, Version};
use serde_derive::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
        }

        guard
    }

    pub fn is_enabled(&self) -> bool {
        self.dsn.parse::<sentry::internals::Dsn>().is_ok()
    }

    fn build_sentry_client_options(&self, version: &Version) -> sentry::ClientOptions {
        sentry::ClientOptions {
            dsn: self.dsn.parse().ok(),
            release: Some(version.long().into()),
            ..Default::default()
        }
    }
}
