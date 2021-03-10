use ckb_types::{H256, U256};
use multiaddr::{Multiaddr, Protocol};
use rand::Rng;
use secio::{self, PeerId};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Error, ErrorKind, Read, Write};
use std::path::PathBuf;

// Max data size in send buffer: 24MB (a little larger than max frame length)
const DEFAULT_SEND_BUFFER: usize = 24 * 1024 * 1024;

/// Network config options.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Config {
    /// Only connect to whitelist peers.
    #[serde(default)]
    pub whitelist_only: bool,
    /// Maximum number of allowed connected peers.
    ///
    /// The node will evict connections when the number exceeds this limit.
    pub max_peers: u32,
    /// Maximum number of outbound peers.
    ///
    /// When node A connects to B, B is the outbound peer of A.
    pub max_outbound_peers: u32,
    /// Network data storage directory path.
    #[serde(default)]
    pub path: PathBuf,
    /// A list of DNS servers to discover peers.
    #[serde(default)]
    pub dns_seeds: Vec<String>,
    /// Whether to probe and store local addresses.
    #[serde(default)]
    pub discovery_local_address: bool,
    /// Interval between pings in seconds.
    ///
    /// A node pings peer regularly to see whether the connection is alive.
    pub ping_interval_secs: u64,
    /// The ping timeout in seconds.
    ///
    /// If a peer does not respond to ping before the timeout, it is evicted.
    pub ping_timeout_secs: u64,
    /// The interval between trials to connect more outbound peers.
    pub connect_outbound_interval_secs: u64,
    /// Listen addresses.
    pub listen_addresses: Vec<Multiaddr>,
    /// Public addresses.
    ///
    /// Set this if this is different from `listen_addresses`.
    #[serde(default)]
    pub public_addresses: Vec<Multiaddr>,
    /// A list of peers used to boot the node discovery.
    ///
    /// Bootnodes are used to bootstrap the discovery when local peer storage is empty.
    pub bootnodes: Vec<Multiaddr>,
    /// A list of peers added in the whitelist.
    ///
    /// When `whitelist_only` is enabled, the node will only connect to peers in this list.
    #[serde(default)]
    pub whitelist_peers: Vec<Multiaddr>,
    /// Enable UPNP when the router supports it.
    #[serde(default)]
    pub upnp: bool,
    /// Enable bootnode mode.
    ///
    /// It is recommended to enable this when this server is intended to be used as a node in the
    /// `bootnodes`.
    #[serde(default)]
    pub bootnode_mode: bool,
    /// Max send buffer size in bytes.
    pub max_send_buffer: Option<usize>,
    /// Chain synchronization config options.
    #[serde(default)]
    pub sync: SyncConfig,
}

/// Chain synchronization config options.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SyncConfig {
    /// Header map config options.
    #[serde(default)]
    pub header_map: HeaderMapConfig,
    /// Block status map config options.
    #[serde(default)]
    pub block_status_map: BlockStatusMapConfig,
    /// Block hash of assume valid target
    #[serde(skip, default)]
    pub assume_valid_target: Option<H256>,
    /// Proof of minimum work during synchronization
    #[serde(skip, default)]
    pub min_chain_work: U256,
}

/// Header map config options.
///
/// Header map stores the block headers before fully verifying the block.
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

/// Block status map config options.
///
/// Block status map stores the block statuses before fully verifying the block.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockStatusMapConfig {
    /// The maximum size of data in memory
    pub primary_limit: usize,
    /// Disable cache if the size of data in memory less than this threshold
    pub backend_close_threshold: usize,
}

impl Default for BlockStatusMapConfig {
    fn default() -> Self {
        Self {
            primary_limit: 1_000_000,
            backend_close_threshold: 50_000,
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
    /// Gets the network secret key path.
    pub fn secret_key_path(&self) -> PathBuf {
        let mut path = self.path.clone();
        path.push("secret_key");
        path
    }

    /// Gets the peer store path.
    pub fn peer_store_path(&self) -> PathBuf {
        let mut path = self.path.clone();
        path.push("peer_store");
        path
    }

    /// Creates missing directories.
    pub fn create_dir_if_not_exists(&self) -> Result<(), Error> {
        if !self.path.exists() {
            fs::create_dir(&self.path)
        } else {
            Ok(())
        }
    }

    /// Gets maximum inbound peers.
    pub fn max_inbound_peers(&self) -> u32 {
        self.max_peers.saturating_sub(self.max_outbound_peers)
    }

    /// Gets maximum outbound peers.
    pub fn max_outbound_peers(&self) -> u32 {
        self.max_outbound_peers
    }

    /// Gets maximum send buffer size.
    pub fn max_send_buffer(&self) -> usize {
        self.max_send_buffer.unwrap_or(DEFAULT_SEND_BUFFER)
    }

    /// Reads the secret key from secret key file.
    ///
    /// If the key file does not exists, it returns `Ok(None)`.
    fn read_secret_key(&self) -> Result<Option<secio::SecioKeyPair>, Error> {
        let path = self.secret_key_path();
        read_secret_key(path)
    }

    /// Generates a random secret key and saves it into the file.
    fn write_secret_key_to_file(&self) -> Result<(), Error> {
        let path = self.secret_key_path();
        let random_key_pair = generate_random_key();
        write_secret_to_file(&random_key_pair, path)
    }

    /// Reads the private key from file or generates one if the file does not exist.
    pub fn fetch_private_key(&self) -> Result<secio::SecioKeyPair, Error> {
        match self.read_secret_key()? {
            Some(key) => Ok(key),
            None => {
                self.write_secret_key_to_file()?;
                Ok(self.read_secret_key()?.expect("key must exists"))
            }
        }
    }

    /// Gets the list of whitelist peers.
    ///
    /// ## Error
    ///
    /// Returns `ErrorKind::InvalidData` when the peer addresses in the config file are invalid.
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

    /// Gets a list of bootnodes.
    ///
    /// ## Error
    ///
    /// Returns `ErrorKind::InvalidData` when the peer addresses in the config file are invalid.
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

    /// Checks whether the outbound peer service should be enabled.
    pub fn outbound_peer_service_enabled(&self) -> bool {
        self.connect_outbound_interval_secs > 0
    }

    /// Checks whether the DNS seeding service should be enabled.
    pub fn dns_seeding_service_enabled(&self) -> bool {
        !self.dns_seeds.is_empty()
    }
}
