use base64::Engine;
use ckb_async_runtime::Handle;
use ckb_error::{Error, InternalErrorKind};
use ckb_logger::{debug, error, info};
use multiaddr::MultiAddr;
use std::{borrow::Cow, str::FromStr};
use tokio::net::TcpStream;
use tokio::{fs::File, io::AsyncReadExt};
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
    pub fn new(handle: Handle, config: OnionServiceConfig) -> Result<OnionService, Error> {
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

        let onion_service = OnionService {
            config,
            key,
            handle,
        };
        Ok(onion_service)
    }

    /// Start the onion service with the given node id.
    pub async fn start(&self, node_id: String) -> Result<MultiAddr, Error> {
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

        info!("Adding onion service v3...");
        ac.add_onion_v3(
            &self.key,
            false,
            false,
            false,
            None,
            &mut [(8115, self.config.onion_service_target)].iter(),
        )
        .await
        .map_err(|err| {
            InternalErrorKind::Other.other(format!("Failed to add onion service: {:?}", err))
        })?;
        info!("Added onion service v3!");

        let tor_address_without_dot_onion = self
            .key
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

        self.handle.spawn(async move {
            let stop_rx = ckb_stop_handler::new_tokio_exit_rx();
            stop_rx.cancelled().await;
            // wait stop_rx
            info!("OnionService received stop signal, exiting...");

            info!("Deleting created onion service...");
            // delete onion service so it works no more
            if let Err(err) = ac
                .del_onion(&tor_address_without_dot_onion)
                .await
                .map_err(|err| {
                    InternalErrorKind::Other
                        .other(format!("Failed to delete onion service: {:?}", err))
                })
            {
                error!("Failed to delete onion service: {:?}", err);
            } else {
                info!("Deleted created onion service! It runs no more!");
            }
        });

        Ok(onion_multi_addr)
    }
}
