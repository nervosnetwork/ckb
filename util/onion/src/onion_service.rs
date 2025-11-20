use base64::Engine;
use ckb_async_runtime::Handle;
use ckb_error::{Error, InternalErrorKind};
use ckb_logger::{error, info, warn};
use ckb_network::multiaddr::Multiaddr;
use ckb_network::NetworkController;
use ckb_stop_handler::CancellationToken;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use torut::onion::TorSecretKeyV3;

use crate::tor_controller::TorController;
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
    ) -> Result<(OnionService, Multiaddr), Error> {
        let key: TorSecretKeyV3 =
            load_or_create_tor_secret_key(config.onion_private_key_path.clone())?;

        let tor_address_without_dot_onion = key
            .public()
            .get_onion_address()
            .get_address_without_dot_onion();

        let onion_external_port = config.onion_external_port;
        let onion_multi_addr_str = format!(
            "/onion3/{}:{}/p2p/{}",
            tor_address_without_dot_onion, onion_external_port, node_id
        );
        let onion_multi_addr = Multiaddr::from_str(&onion_multi_addr_str).map_err(|err| {
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

    /// Start the onion service.
    pub async fn start(
        &self,
        network_controller: NetworkController,
        onion_service_addr: Multiaddr,
    ) -> Result<(), Error> {
        let stop_rx = ckb_stop_handler::new_tokio_exit_rx();
        loop {
            let (tor_server_alive_tx, mut tor_server_alive_rx) =
                tokio::sync::mpsc::unbounded_channel::<()>();
            match self
                .launch_onion_service(stop_rx.clone(), tor_server_alive_tx)
                .await
            {
                Ok(_) => {
                    info!("CKB has started listening on the onion hidden network, the onion service address is: {}", onion_service_addr.clone());
                    network_controller.add_public_addr(onion_service_addr.clone());
                }
                Err(err) => {
                    error!("start onion service failed: {}", err);
                }
            }

            let _ = tor_server_alive_rx.recv().await;
            if stop_rx.is_cancelled() {
                return Ok(());
            }
            warn!("It seems that the connection to tor server's controller has been closed, retry connect to tor controller({})", self.config.tor_controller.to_string());
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// Launch the onion service.
    pub async fn launch_onion_service(
        &self,
        stop_rx: CancellationToken,
        tor_server_alive_tx: UnboundedSender<()>,
    ) -> Result<(), Error> {
        let tor_controller = self.config.tor_controller.to_string();
        let tor_password = self.config.tor_password.clone();

        let mut tor_controller = TorController::new(tor_controller, tor_password, None).await?;

        tor_controller.wait_tor_server_bootstrap_done().await?;

        let p2p_listener_addresses = [(
            self.config.onion_external_port,
            self.config.p2p_listen_address,
        )];
        info!(
            "Adding onion service v3: {}",
            self.config.p2p_listen_address.to_string()
        );
        tor_controller
            .add_onion_v3(self.key.clone(), &mut p2p_listener_addresses.iter())
            .await
            .map_err(|err| {
                InternalErrorKind::Other.other(format!("Failed to add onion service: {:?}", err))
            })?;
        info!(
            "Added onion service v3: {}",
            self.config.p2p_listen_address.to_string()
        );

        self.handle.spawn(async move {
            let mut ticker = tokio::time::interval(tokio::time::Duration::from_secs(3));
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        let uptime = tor_controller.get_uptime().await;
                        if let Err(err) = uptime {
                            error!("Failed to get tor server uptime: {:?}", err);
                            drop(tor_server_alive_tx);
                            return;
                        }
                    }
                    _ = stop_rx.cancelled() => {
                        info!("OnionService received stop signal, exiting...");
                        drop(tor_server_alive_tx);
                        return;
                    }
                }
            }
        });
        Ok(())
    }
}

fn load_or_create_tor_secret_key(onion_private_key_path: String) -> Result<TorSecretKeyV3, Error> {
    let is_onion_private_key_exists = Path::new(&onion_private_key_path).exists();
    let key = match is_onion_private_key_exists {
        true => load_tor_secret_key(onion_private_key_path)?,
        false => create_tor_secret_key(onion_private_key_path)?,
    };
    Ok(key)
}

fn create_tor_secret_key(onion_private_key_path: String) -> Result<TorSecretKeyV3, Error> {
    let key = torut::onion::TorSecretKeyV3::generate();
    info!(
        "Generated new onion service v3 key for address: {}",
        key.public().get_onion_address()
    );

    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut options = OpenOptions::new().create(true).truncate(true).write(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options = options.mode(0o600);
    }

    let mut file = options.open(&onion_private_key_path).map_err(|err| {
        InternalErrorKind::Other.other(format!(
            "Failed to open onion private key for writing: {:?}",
            err
        ))
    })?;
    file.write_all(
        &base64::engine::general_purpose::STANDARD
            .encode(key.as_bytes())
            .into_bytes(),
    )
    .map_err(|err| {
        InternalErrorKind::Other.other(format!("Failed to write onion private key: {:?}", err))
    })?;

    Ok(key)
}

const TOR_SECRET_KEY_LENGTH: usize = 64;

fn load_tor_secret_key(onion_private_key_path: String) -> Result<TorSecretKeyV3, Error> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(
            std::fs::read_to_string(&onion_private_key_path).map_err(|err| {
                InternalErrorKind::Other.other(format!(
                    "Read onion private key({}) failed: {}",
                    onion_private_key_path, err
                ))
            })?,
        )
        .map_err(|err| {
            InternalErrorKind::Other.other(format!("Failed to decode onion private key: {:?}", err))
        })?;
    let raw = raw.as_slice();
    if raw.len() != TOR_SECRET_KEY_LENGTH {
        return Err(InternalErrorKind::Other
            .other("Invalid secret key length")
            .into());
    }
    let mut buf = [0u8; TOR_SECRET_KEY_LENGTH];
    buf.copy_from_slice(raw);
    Ok(TorSecretKeyV3::from(buf))
}
