use build_info::{get_version, Version};
use serde_derive::Deserialize;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SentryConfig {
    pub dsn: String,
}

impl SentryConfig {
    pub fn init(self) -> sentry::internals::ClientInitGuard {
        let guard = sentry::init(self);
        if guard.is_enabled() {
            sentry::integrations::panic::register_panic_handler();
        }

        guard
    }
}

impl Into<sentry::ClientOptions> for SentryConfig {
    fn into(self) -> sentry::ClientOptions {
        let version = get_version!();

        sentry::ClientOptions {
            dsn: self.dsn.parse().ok(),
            release: Some(version.long().into()),
            ..Default::default()
        }
    }
}
