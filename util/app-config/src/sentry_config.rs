use build_info::Version;
use sentry::{
    configure_scope, init,
    integrations::panic::register_panic_handler,
    internals::{ClientInitGuard, Dsn},
    protocol::Event,
    ClientOptions,
};
use serde_derive::{Deserialize, Serialize};
use std::borrow::Cow;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SentryConfig {
    pub dsn: String,
}

impl SentryConfig {
    pub fn init(&self, version: &Version) -> ClientInitGuard {
        let guard = init(self.build_sentry_client_options(&version));
        if guard.is_enabled() {
            configure_scope(|scope| {
                scope.set_tag("release.pre", version.is_pre());
                scope.set_tag("release.dirty", version.is_dirty());
            });

            register_panic_handler();
        }

        guard
    }

    pub fn is_enabled(&self) -> bool {
        self.dsn.parse::<Dsn>().is_ok()
    }

    fn build_sentry_client_options(&self, version: &Version) -> ClientOptions {
        ClientOptions {
            dsn: self.dsn.parse().ok(),
            release: Some(version.long().into()),
            before_send: Some(Arc::new(Box::new(before_send))),
            ..Default::default()
        }
    }
}

static DB_OPEN_FINGERPRINT: &[Cow<'static, str>] =
    &[Cow::Borrowed("ckb-db"), Cow::Borrowed("open")];
static SQLITE_FINGERPRINT: &[Cow<'static, str>] = &[
    Cow::Borrowed("ckb-network"),
    Cow::Borrowed("peerstore"),
    Cow::Borrowed("sqlite"),
];

fn before_send(mut event: Event<'static>) -> Option<Event<'static>> {
    let ex = match event
        .exception
        .values
        .iter()
        .next()
        .and_then(|ex| ex.value.as_ref())
    {
        Some(ex) => ex,
        None => return Some(event),
    };

    // Group events via fingerprint, or ignore

    if ex.starts_with("DBError failed to open the database") {
        event.fingerprint = Cow::Borrowed(DB_OPEN_FINGERPRINT);
    } else if ex.contains("SqliteFailure") {
        event.fingerprint = Cow::Borrowed(SQLITE_FINGERPRINT);
    } else if ex.starts_with("DBError the database version")
        || ex.contains("kind: AddrInUse")
        || ex.contains("kind: AddrNotAvailable")
    {
        // ignore
        return None;
    }

    Some(event)
}
