use ckb_error::{Error, InternalErrorKind};
use ckb_logger::{debug, error, info, warn};
use futures::future::BoxFuture;
use std::borrow::Cow;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use torut::control::{TorAuthData, TorAuthMethod, UnauthenticatedConn, COOKIE_LENGTH};
use torut::{
    control::{AsyncEvent, AuthenticatedConn, ConnError},
    onion::TorSecretKeyV3,
};

use crate::TorEventHandlerFn;

type TorAuthenticatedConn =
    AuthenticatedConn<TcpStream, fn(AsyncEvent<'_>) -> BoxFuture<'static, Result<(), ConnError>>>;

/// A controller for a Tor server.
pub struct TorController {
    inner: TorAuthenticatedConn,
}

impl TorController {
    /// Create a new TorController instance.
    /// event_handler is an optional function that will be called when a Tor event occurs.
    pub async fn new(
        tor_controller_url: String,
        tor_password: Option<String>,
        event_handler: Option<TorEventHandlerFn>,
    ) -> Result<Self, Error> {
        let s = TcpStream::connect(tor_controller_url.clone())
            .await
            .map_err(|err| {
                InternalErrorKind::Other.other(format!(
                    "Failed to connect to tor controller {}: {:?}",
                    tor_controller_url, err
                ))
            })?;

        let mut utc: UnauthenticatedConn<TcpStream> = UnauthenticatedConn::new(s);

        authenticate(tor_password, &mut utc).await?;

        let mut ac = utc.into_authenticated().await;

        ac.set_async_event_handler(event_handler);

        Ok(TorController { inner: ac })
    }

    /// get tor server's status
    pub async fn get_bootstrap_phase(&mut self) -> Result<String, ConnError> {
        self.inner.get_info_unquote("status/bootstrap-phase").await
    }

    /// get tor server's version
    pub async fn get_version(&mut self) -> Result<String, ConnError> {
        self.inner.get_info("version").await
    }

    /// get tor server's uptime
    pub async fn get_uptime(&mut self) -> Result<Duration, ConnError> {
        let uptime = self.inner.get_info("uptime").await.map_err(|err| {
            // the tor server's version is less than 0.3.5.1-alpha
            warn!(
                "failed to get uptime; the Tor controller may not expose 'uptime' (older Tor versions) or returned an error: {}",
                err
            );
            err
        })?;
        debug!("tor server's uptime is {} seconds", uptime);
        let secs: u64 = uptime.parse().map_err(|err| {
            ConnError::IOError(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("failed to parse uptime {} to u64 {}", uptime, err),
            ))
        })?;
        Ok(Duration::from_secs(secs))
    }

    /// Waits asynchronously until the Tor server has completed its bootstrap process.
    /// Periodically checks the bootstrap status and returns an error if a stop signal is received or if the status cannot be retrieved.
    pub async fn wait_tor_server_bootstrap_done(&mut self) -> Result<(), Error> {
        info!("waiting tor server bootstrap");
        loop {
            if ckb_stop_handler::has_received_stop_signal() {
                return Err(InternalErrorKind::Other
                    .other("Received stop signal")
                    .into());
            }
            let bootstrap_done = match self.get_bootstrap_phase().await {
                Ok(info) => {
                    info!("Waiting Tor bootstrapping: current status: {:?}", info);
                    info.contains("Done")
                }
                Err(err) => {
                    error!("Failed to get tor bootstrap status: {:?}", err);
                    return Err(InternalErrorKind::Other
                        .other(format!("Failed to get tor bootstrap status: {:?}", err))
                        .into());
                }
            };
            if bootstrap_done {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        info!("Tor server bootstrap done!");
        Ok(())
    }

    /// Add a new v3 onion service to the Tor server.
    pub async fn add_onion_v3(
        &mut self,
        key: TorSecretKeyV3,
        listeners: &mut impl Iterator<Item = &(u16, SocketAddr)>,
    ) -> Result<(), torut::control::ConnError> {
        self.inner
            .add_onion_v3(&key, false, false, false, None, listeners)
            .await
    }
}

/// Authenticates with the Tor controller using the given password or cookie.
pub async fn authenticate(
    tor_password: Option<String>,
    utc: &mut UnauthenticatedConn<TcpStream>,
) -> Result<(), Error> {
    let proto_info = utc.load_protocol_info().await.map_err(|err| {
        InternalErrorKind::Other.other(format!("Failed to load protocol info: {:?}", err))
    })?;
    proto_info.auth_methods.iter().for_each(|m| {
        info!("Tor Server Controller supports auth method: {:?}", m);
    });
    if proto_info.auth_methods.contains(&TorAuthMethod::Null) {
        utc.authenticate(&TorAuthData::Null).await.map_err(|err| {
            InternalErrorKind::Other.other(format!("Failed to authenticate with null: {:?}", err))
        })?;
        if tor_password.is_some() {
            warn!("Password not required for the Tor controller, but `tor_password` is configured in [network.onion].");
        }
        return Ok(());
    }

    if proto_info
        .auth_methods
        .contains(&TorAuthMethod::HashedPassword)
    {
        match tor_password {
            Some(tor_password) => {
                utc.authenticate(&TorAuthData::HashedPassword(Cow::Owned(tor_password)))
                    .await
                    .map_err(|err| {
                        InternalErrorKind::Other
                            .other(format!("Failed to authenticate with password: {:?}", err))
                    })?;
                return Ok(());
            }
            None => {
                warn!("Tor server requires a password, but none is configured");
            }
        }
    }

    if proto_info.auth_methods.contains(&TorAuthMethod::Cookie)
        || proto_info.auth_methods.contains(&TorAuthMethod::SafeCookie)
    {
        let cookie = load_auth_cookie(proto_info).await?;
        let tor_auth_data = {
            if proto_info.auth_methods.contains(&TorAuthMethod::Cookie) {
                debug!("Using Cookie auth method...");
                TorAuthData::Cookie(Cow::Owned(cookie))
            } else {
                debug!("Using SafeCookie auth method...");
                TorAuthData::SafeCookie(Cow::Owned(cookie))
            }
        };
        utc.authenticate(&tor_auth_data).await.map_err(|err| {
            InternalErrorKind::Other.other(format!("Failed to authenticate with cookie: {:?}", err))
        })?;
        return Ok(());
    }
    Err(InternalErrorKind::Other
        .other(format!(
            "Tor server does not support any authentication method; proto_info: {:?}",
            proto_info
        ))
        .into())
}

async fn load_auth_cookie(
    proto_info: &torut::control::TorPreAuthInfo<'_>,
) -> Result<Vec<u8>, Error> {
    let mut cookie_file = File::open(
        proto_info
            .cookie_file
            .as_ref()
            .ok_or_else(|| {
                InternalErrorKind::Other.other("Tor server did not provide cookie file path")
            })?
            .as_ref(),
    )
    .await
    .map_err(|err| {
        InternalErrorKind::Other.other(format!("Failed to open cookie file: {:?}", err))
    })?;
    let mut cookie = Vec::new();
    cookie_file.read_to_end(&mut cookie).await.map_err(|err| {
        InternalErrorKind::Other.other(format!("Failed to read cookie file: {:?}", err))
    })?;
    if cookie.len() != COOKIE_LENGTH {
        return Err(InternalErrorKind::Other
            .other(format!(
                "Invalid cookie length: expected {}, got {}",
                COOKIE_LENGTH,
                cookie.len()
            ))
            .into());
    }
    Ok(cookie)
}
