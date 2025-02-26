use base64::Engine;
use ckb_async_runtime::Handle;
use ckb_error::{Error, InternalErrorKind};
use ckb_logger::{debug, error, info, warn};
use ckb_network::multiaddr::MultiAddr;
use ckb_network::NetworkController;
use ckb_stop_handler::CancellationToken;
use multiaddr::Multiaddr;
use std::borrow::Cow;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::mpsc::UnboundedSender;
use tokio::{fs::File, io::AsyncReadExt};
use torut::control::{AsyncEvent, AuthenticatedConn, ConnError};
use torut::{
    control::{TorAuthData, TorAuthMethod, UnauthenticatedConn, COOKIE_LENGTH},
    onion::TorSecretKeyV3,
};

use crate::OnionServiceConfig;

/// Onion service.
pub struct OnionService {
    key: TorSecretKeyV3,
    config: OnionServiceConfig,
    handle: Handle,
}
impl OnionService {
    /// Create a new onion service with the given configuration.
    pub fn new(
        handle: Handle,
        config: OnionServiceConfig,
        node_id: String,
    ) -> Result<(OnionService, MultiAddr), Error> {
        let key = if std::fs::exists(&config.onion_private_key_path).map_err(|err| {
            InternalErrorKind::Other
                .other(format!("Failed to check onion private key path: {:?}", err))
        })? {
            let raw = base64::engine::general_purpose::STANDARD
                .decode(std::fs::read_to_string(&config.onion_private_key_path).unwrap())
                .map_err(|err| {
                    InternalErrorKind::Other
                        .other(format!("Failed to decode onion private key: {:?}", err))
                })?;
            let raw = raw.as_slice();

            if raw.len() != 64 {
                return Err(InternalErrorKind::Other
                    .other("Invalid secret key length")
                    .into());
            }
            let mut buf = [0u8; 64];
            buf.clone_from_slice(raw);
            TorSecretKeyV3::from(buf)
        } else {
            let key = torut::onion::TorSecretKeyV3::generate();
            info!(
                "Generated new onion service v3 key for address: {}",
                key.public().get_onion_address()
            );

            std::fs::write(
                &config.onion_private_key_path,
                base64::engine::general_purpose::STANDARD.encode(key.as_bytes()),
            )
            .map_err(|err| {
                InternalErrorKind::Other
                    .other(format!("Failed to write onion private key: {:?}", err))
            })?;

            key
        };

        let tor_address_without_dot_onion = key
            .public()
            .get_onion_address()
            .get_address_without_dot_onion();

        let onion_multi_addr_str = format!(
            "/onion3/{}:8115/p2p/{}",
            tor_address_without_dot_onion, node_id
        );
        let onion_multi_addr = MultiAddr::from_str(&onion_multi_addr_str).map_err(|err| {
            InternalErrorKind::Other.other(format!(
                "Failed to parse onion address {} to multi_addr: {:?}",
                onion_multi_addr_str, err
            ))
        })?;

        let onion_service = OnionService {
            config,
            key,
            handle,
        };
        Ok((onion_service, onion_multi_addr))
    }

    pub async fn start(
        &self,
        network_controller: NetworkController,
        onion_service_addr: MultiAddr,
    ) -> Result<(), Error> {
        let stop_rx = ckb_stop_handler::new_tokio_exit_rx();
        loop {
            let (tor_server_alive_tx, mut tor_server_alive_rx) =
                tokio::sync::mpsc::unbounded_channel::<()>();
            match self.start_inner(stop_rx.clone(), tor_server_alive_tx).await {
                Ok(_) => {
                    info!("CKB has started listening on the onion hidden network, the onion service address is: {}", onion_service_addr.clone());
                    network_controller.add_public_addr(onion_service_addr.clone());
                }
                Err(err) => {
                    error!("start onion service failed: {}", err);
                }
            }

            let _ = tor_server_alive_rx.recv().await;
            warn!("It seem that the connection to tor server's controller has been closed, retry connect to tor controller({})", self.config.tor_controller.to_string());
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    pub async fn start_inner(
        &self,
        stop_rx: CancellationToken,
        mut tor_server_alive_tx: UnboundedSender<()>,
    ) -> Result<(), Error> {
        let s = TcpStream::connect(self.config.tor_controller.to_string())
            .await
            .map_err(|err| {
                InternalErrorKind::Other
                    .other(format!("Failed to connect to tor controller: {:?}", err))
            })?;

        let mut utc = UnauthenticatedConn::new(s);
        let proto_info = utc.load_protocol_info().await.map_err(|err| {
            InternalErrorKind::Other.other(format!("Failed to load protocol info: {:?}", err))
        })?;

        proto_info.auth_methods.iter().for_each(|m| {
            debug!("Tor Server supports auth method: {:?}", m);
        });

        if proto_info.auth_methods.contains(&TorAuthMethod::Null) {
            utc.authenticate(&TorAuthData::Null).await.map_err(|err| {
                InternalErrorKind::Other
                    .other(format!("Failed to authenticate with null: {:?}", err))
            })?;
        } else if proto_info
            .auth_methods
            .contains(&TorAuthMethod::HashedPassword)
            && self.config.tor_password.is_some()
        {
            utc.authenticate(&TorAuthData::HashedPassword(Cow::Owned(
                self.config
                    .tor_password
                    .as_ref()
                    .expect("tor password exists")
                    .to_owned(),
            )))
            .await
            .map_err(|err| {
                InternalErrorKind::Other
                    .other(format!("Failed to authenticate with password: {:?}", err))
            })?;
        } else if proto_info.auth_methods.contains(&TorAuthMethod::Cookie)
            || proto_info.auth_methods.contains(&TorAuthMethod::SafeCookie)
        {
            let cookie = {
                let mut cookie_file = File::open(
                    proto_info
                        .cookie_file
                        .as_ref()
                        .ok_or_else(|| {
                            InternalErrorKind::Other
                                .other("Tor server did not provide cookie file path")
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
                assert_eq!(cookie.len(), COOKIE_LENGTH);
                cookie
            };
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
                InternalErrorKind::Other
                    .other(format!("Failed to authenticate with cookie: {:?}", err))
            })?;
        } else {
            return Err(InternalErrorKind::Other
                .other("Tor server does not support any authentication method")
                .into());
        }

        let mut ac = utc.into_authenticated().await;
        ac.set_async_event_handler(Some(|_| async move { Ok(()) }));

        let mut tor_controller = TorController::new(ac);

        tor_controller.wait_tor_server_bootstrap_done().await;

        info!("Adding onion service v3...");
        tor_controller
            .inner
            .add_onion_v3(
                &self.key,
                false,
                false,
                false,
                None,
                &mut [
                    (8115, self.config.onion_service_target),
                    (
                        8114,
                        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 8114)),
                    ),
                ]
                .iter(),
            )
            .await
            .map_err(|err| {
                InternalErrorKind::Other.other(format!("Failed to add onion service: {:?}", err))
            })?;
        info!("Added onion service v3!");

        self.handle.spawn(async move {
            let _tx = tor_server_alive_tx;
            loop {
                if stop_rx.is_cancelled() {
                    info!("OnionService received stop signal, exiting...");
                    return;
                }
                if let Err(err) = tor_controller.get_version().await {
                    error!("tor_controller get_version failed: {}", err);
                    return;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
        Ok(())
    }
}

struct TorController<S, H> {
    inner: AuthenticatedConn<S, H>,
}

impl<S, F, H> TorController<S, H>
where
    S: AsyncRead + AsyncWrite + Unpin,
    H: Fn(AsyncEvent<'static>) -> F,
    F: Future<Output = Result<(), ConnError>>,
{
    pub fn new(inner: AuthenticatedConn<S, H>) -> Self {
        TorController { inner }
    }

    pub async fn get_bootstrap_phase(&mut self) -> Result<String, ConnError> {
        self.inner.get_info_unquote("status/bootstrap-phase").await
    }

    pub async fn get_version(&mut self) -> Result<String, ConnError> {
        self.inner.get_info("version").await
    }

    /// get tor server's uptime
    pub async fn get_uptime(&mut self) -> Result<Duration, ConnError> {
        let uptime = self.inner.get_info("uptime").await.map_err(|err| {
            // the tor server's version is less than 0.3.5.1-alpha
            warn!("failed to get uptime: {}, It seems that the tor server's version is less than 0.3.5.1-alpha", err);
            err
        })?;
        info!("tor server's uptime is {}", uptime);
        let secs: u64 = uptime.parse().map_err(|err| {
            ConnError::IOError(std::io::Error::other(format!(
                "failed to parse uptime {} to u64",
                uptime
            )))
        })?;
        Ok(Duration::from_secs(secs))
    }

    // Implement the new method:
    pub async fn wait_tor_server_bootstrap_done(&mut self) {
        info!("waiting tor server bootstrap");
        loop {
            let boostrap_done = match self.get_bootstrap_phase().await {
                Ok(info) => {
                    info!("Tor bootstrap status: {:?}", info);
                    info.contains("Done")
                }
                Err(err) => {
                    error!("Failed to get tor bootstrap status: {:?}", err);
                    return;
                }
            };
            if boostrap_done {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await; // Use tokio::time::sleep for async
        }
        info!("Tor server bootstrap done!")
    }
}
