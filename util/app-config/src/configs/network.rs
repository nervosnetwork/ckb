use ckb_types::{H256, U256};
use multiaddr::Multiaddr;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Error, ErrorKind, Read, Write};
use std::path::PathBuf;

// Max data size in send buffer: 24MB (a little larger than max frame length)
const DEFAULT_SEND_BUFFER: usize = 24 * 1024 * 1024;

// Tentacle inner bound channel size, default 128
const DEFAULT_CHANNEL_SIZE: usize = 128;

/// Network config options.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
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
    /// The interval between discovery announce message checking.
    #[serde(default)]
    pub discovery_announce_check_interval_secs: Option<u64>,
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
    /// Supported protocols list
    #[serde(default = "default_support_all_protocols")]
    pub support_protocols: Vec<SupportProtocol>,
    /// Max send buffer size in bytes.
    pub max_send_buffer: Option<usize>,
    /// Network use reuse port or not
    #[serde(default = "default_reuse")]
    pub reuse_port_on_linux: bool,
    /// Chain synchronization config options.
    #[serde(default)]
    pub sync: SyncConfig,
    /// Tentacle inner channel_size.
    pub channel_size: Option<usize>,
}

/// Chain synchronization config options.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SyncConfig {
    /// Header map config options.
    #[serde(default)]
    pub header_map: HeaderMapConfig,
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
#[serde(deny_unknown_fields)]
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

#[derive(Clone, Debug, Copy, Eq, PartialEq, Serialize, Deserialize, Hash)]
#[allow(missing_docs)]
pub enum SupportProtocol {
    Ping,
    Discovery,
    Identify,
    Feeler,
    DisconnectMessage,
    Sync,
    Relay,
    Time,
    Alert,
}

#[allow(missing_docs)]
pub fn default_support_all_protocols() -> Vec<SupportProtocol> {
    vec![
        SupportProtocol::Ping,
        SupportProtocol::Discovery,
        SupportProtocol::Identify,
        SupportProtocol::Feeler,
        SupportProtocol::DisconnectMessage,
        SupportProtocol::Sync,
        SupportProtocol::Relay,
        SupportProtocol::Time,
        SupportProtocol::Alert,
    ]
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
            file.write_all(secret)?;
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

    /// Gets maximum send buffer size.
    pub fn channel_size(&self) -> usize {
        self.channel_size.unwrap_or(DEFAULT_CHANNEL_SIZE)
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
    pub fn whitelist_peers(&self) -> Vec<Multiaddr> {
        self.whitelist_peers.clone()
    }

    /// Gets a list of bootnodes.
    pub fn bootnodes(&self) -> Vec<Multiaddr> {
        self.bootnodes.clone()
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

/// By default, using reuse port can make any outbound connection of the node become a potential
/// listen address, which will help the robustness of our network
const fn default_reuse() -> bool {
    true
}
