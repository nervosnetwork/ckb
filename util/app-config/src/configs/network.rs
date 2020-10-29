use multiaddr::{Multiaddr, Protocol};
use rand::Rng;
use secio::{self, PeerId};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Error, ErrorKind, Read, Write};
use std::path::PathBuf;

// Max data size in send buffer: 24MB (a little larger than max frame length)
const DEFAULT_SEND_BUFFER: usize = 24 * 1024 * 1024;

/// TODO(doc): @doitian
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Config {
    /// TODO(doc): @doitian
    #[serde(default)]
    pub whitelist_only: bool,
    /// TODO(doc): @doitian
    pub max_peers: u32,
    /// TODO(doc): @doitian
    pub max_outbound_peers: u32,
    /// TODO(doc): @doitian
    #[serde(default)]
    pub path: PathBuf,
    /// TODO(doc): @doitian
    #[serde(default)]
    pub dns_seeds: Vec<String>,
    /// TODO(doc): @doitian
    // Set if discovery add local address to peer store
    #[serde(default)]
    pub discovery_local_address: bool,
    /// TODO(doc): @doitian
    pub ping_interval_secs: u64,
    /// TODO(doc): @doitian
    pub ping_timeout_secs: u64,
    /// TODO(doc): @doitian
    pub connect_outbound_interval_secs: u64,
    /// TODO(doc): @doitian
    pub listen_addresses: Vec<Multiaddr>,
    /// TODO(doc): @doitian
    #[serde(default)]
    pub public_addresses: Vec<Multiaddr>,
    /// TODO(doc): @doitian
    pub bootnodes: Vec<Multiaddr>,
    /// TODO(doc): @doitian
    #[serde(default)]
    pub whitelist_peers: Vec<Multiaddr>,
    /// TODO(doc): @doitian
    #[serde(default)]
    pub upnp: bool,
    /// TODO(doc): @doitian
    #[serde(default)]
    pub bootnode_mode: bool,
    /// TODO(doc): @doitian
    // Max send buffer size
    pub max_send_buffer: Option<usize>,
    /// TODO(doc): @doitian
    pub sync: Option<SyncConfig>,
}

/// TODO(doc): @doitian
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SyncConfig {
    /// TODO(doc): @doitian
    #[serde(default)]
    pub header_map: HeaderMapConfig,
}

/// TODO(doc): @doitian
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HeaderMapConfig {
    /// The maximum size of data in memory
    pub primary_limit: usize,
    /// Disable cache if the size of data in memory less than this threshold
    pub backend_close_threshold: usize,
}

impl Default for HeaderMapConfig {
    fn default() -> Self {
        Self {
            primary_limit: 300_000,
            backend_close_threshold: 20_000,
        }
    }
}

pub(crate) fn generate_random_key() -> [u8; 32] {
    loop {
        let mut key: [u8; 32] = [0; 32];
        rand::thread_rng().fill(&mut key);
        if secio::SecioKeyPair::secp256k1_raw_key(&key).is_ok() {
            return key;
        }
    }
}

pub(crate) fn write_secret_to_file(secret: &[u8], path: PathBuf) -> Result<(), Error> {
    fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(path)
        .and_then(|mut file| {
            file.write_all(&secret)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(fs::Permissions::from_mode(0o400))
            }
            #[cfg(not(unix))]
            {
                let mut permissions = file.metadata()?.permissions();
                permissions.set_readonly(true);
                file.set_permissions(permissions)
            }
        })
}

pub(crate) fn read_secret_key(path: PathBuf) -> Result<Option<secio::SecioKeyPair>, Error> {
    let mut file = match fs::File::open(path.clone()) {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };
    let warn = |m: bool, d: &str| {
        if m {
            ckb_logger::warn!(
                "Your network secret file's permission is not {}, path: {:?}, \
                please fix it as soon as possible",
                d,
                path
            )
        }
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        warn(
            file.metadata()?.permissions().mode() & 0o177 != 0,
            "less than 0o600",
        );
    }
    #[cfg(not(unix))]
    {
        warn(!file.metadata()?.permissions().readonly(), "readonly");
    }
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).and_then(|_read_size| {
        secio::SecioKeyPair::secp256k1_raw_key(&buf)
            .map(Some)
            .map_err(|_| Error::new(ErrorKind::InvalidData, "invalid secret key data"))
    })
}

impl Config {
    /// TODO(doc): @doitian
    pub fn secret_key_path(&self) -> PathBuf {
        let mut path = self.path.clone();
        path.push("secret_key");
        path
    }

    /// TODO(doc): @doitian
    pub fn peer_store_path(&self) -> PathBuf {
        let mut path = self.path.clone();
        path.push("peer_store");
        path
    }

    /// TODO(doc): @doitian
    pub fn create_dir_if_not_exists(&self) -> Result<(), Error> {
        if !self.path.exists() {
            fs::create_dir(&self.path)
        } else {
            Ok(())
        }
    }

    /// TODO(doc): @doitian
    pub fn max_inbound_peers(&self) -> u32 {
        self.max_peers.saturating_sub(self.max_outbound_peers)
    }

    /// TODO(doc): @doitian
    pub fn max_outbound_peers(&self) -> u32 {
        self.max_outbound_peers
    }

    /// TODO(doc): @doitian
    pub fn max_send_buffer(&self) -> usize {
        self.max_send_buffer.unwrap_or(DEFAULT_SEND_BUFFER)
    }

    fn read_secret_key(&self) -> Result<Option<secio::SecioKeyPair>, Error> {
        let path = self.secret_key_path();
        read_secret_key(path)
    }

    fn write_secret_key_to_file(&self) -> Result<(), Error> {
        let path = self.secret_key_path();
        let random_key_pair = generate_random_key();
        write_secret_to_file(&random_key_pair, path)
    }

    /// TODO(doc): @doitian
    pub fn fetch_private_key(&self) -> Result<secio::SecioKeyPair, Error> {
        match self.read_secret_key()? {
            Some(key) => Ok(key),
            None => {
                self.write_secret_key_to_file()?;
                Ok(self.read_secret_key()?.expect("key must exists"))
            }
        }
    }

    /// TODO(doc): @doitian
    pub fn whitelist_peers(&self) -> Result<Vec<(PeerId, Multiaddr)>, Error> {
        let mut peers = Vec::with_capacity(self.whitelist_peers.len());
        for addr_str in &self.whitelist_peers {
            let mut addr = addr_str.to_owned();
            let peer_id = match addr.pop() {
                Some(Protocol::P2P(key)) => PeerId::from_bytes(key.to_vec()).map_err(|_| {
                    Error::new(ErrorKind::InvalidData, "invalid whitelist peers config")
                })?,
                _ => {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "invalid whitelist peers config",
                    ))
                }
            };
            peers.push((peer_id, addr))
        }
        Ok(peers)
    }

    /// TODO(doc): @doitian
    pub fn bootnodes(&self) -> Result<Vec<(PeerId, Multiaddr)>, Error> {
        let mut peers = Vec::with_capacity(self.bootnodes.len());
        for addr_str in &self.bootnodes {
            let mut addr = addr_str.to_owned();
            let peer_id = match addr.pop() {
                Some(Protocol::P2P(key)) => PeerId::from_bytes(key.to_vec())
                    .map_err(|_| Error::new(ErrorKind::InvalidData, "invalid bootnodes config"))?,
                _ => {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "invalid bootnodes config",
                    ))
                }
            };
            peers.push((peer_id, addr));
        }
        Ok(peers)
    }

    /// TODO(doc): @doitian
    pub fn outbound_peer_service_enabled(&self) -> bool {
        self.connect_outbound_interval_secs > 0
    }

    /// TODO(doc): @doitian
    pub fn dns_seeding_service_enabled(&self) -> bool {
        !self.dns_seeds.is_empty()
    }
}
