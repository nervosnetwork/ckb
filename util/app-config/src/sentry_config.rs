use ckb_build_info::Version;
use sentry::{
    configure_scope, init,
    integrations::panic::register_panic_handler,
    internals::{ClientInitGuard, Dsn},
    protocol::Event,
    ClientOptions, Level,
};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SentryConfig {
    pub dsn: String,
    pub org_ident: Option<String>,
    pub org_contact: Option<String>,
}

impl SentryConfig {
    pub fn init(&self, version: &Version) -> ClientInitGuard {
        let guard = init(self.build_sentry_client_options(&version));
        if guard.is_enabled() {
            configure_scope(|scope| {
                scope.set_tag("release.pre", version.is_pre());
                scope.set_tag("release.dirty", version.is_dirty());
                if let Some(org_ident) = &self.org_ident {
                    scope.set_tag("org_ident", org_ident);
                }
                if let Some(org_contact) = &self.org_contact {
                    scope.set_extra("org_contact", org_contact.clone().into());
                }
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
    if let Some(name) = std::thread::current().name() {
        event.extra.insert("thread.name".to_string(), name.into());
    }

    let ex = match event
        .exception
        .values
        .get(0)
        .and_then(|ex| ex.value.as_ref())
    {
        Some(ex) => ex,
        None => return Some(event),
    };

    // Group events via fingerprint, or ignore

    if ex.starts_with("DBError failed to open the database") {
        event.level = Level::Warning;
        event.fingerprint = Cow::Borrowed(DB_OPEN_FINGERPRINT);
    } else if ex.contains("SqliteFailure") {
        event.level = Level::Warning;
        event.fingerprint = Cow::Borrowed(SQLITE_FINGERPRINT);
    } else if ex.starts_with("DBError the database version")
        || ex.contains("kind: AddrInUse")
        || ex.contains("kind: AddrNotAvailable")
        || ex.contains("IO error: No space left")
    {
        // ignore
        return None;
    }

    Some(event)
}
