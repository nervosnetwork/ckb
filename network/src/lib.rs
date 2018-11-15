#![type_length_limit = "2097152"]
extern crate bytes;
extern crate futures;
extern crate libp2p;
extern crate rand;
extern crate tokio;
extern crate unsigned_varint;
#[macro_use]
extern crate log;
extern crate fnv;
#[macro_use]
extern crate serde_derive;
extern crate ckb_util as util;

mod ckb_protocol;
mod ckb_protocol_handler;
mod ckb_service;
mod discovery_service;
mod errors;
mod identify_service;
mod memory_peer_store;
mod network;
mod network_config;
mod network_service;
mod outgoing_service;
mod peer_store;
mod peers_registry;
mod ping_service;
mod protocol;
mod protocol_service;
mod timer_service;
mod transport;

pub use self::errors::{Error, ErrorKind};
pub use self::network::{Network, PeerInfo, SessionInfo};
pub use self::network_config::NetworkConfig;
pub use self::network_service::NetworkService;
pub use ckb_protocol::{CKBProtocol, CKBProtocols};
pub use ckb_protocol_handler::{CKBProtocolContext, CKBProtocolHandler, Severity};
pub use libp2p::{core::Endpoint, multiaddr::AddrComponent, Multiaddr, PeerId};

pub type TimerToken = usize;
pub type ProtocolId = [u8; 3];

use libp2p::secio;
use rand::Rng;
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_OUTGOING_PEERS_RATIO: u32 = 3;
pub(crate) type Timer = (Arc<CKBProtocolHandler>, ProtocolId, TimerToken, Duration);

// used in CKBProtocolContext
pub type PeerIndex = usize;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Config {
    pub listen_addresses: Vec<Multiaddr>,
    pub secret_file: Option<String>,
    pub nodes_file: Option<String>,
    /// List of initial node addresses
    pub boot_nodes: Vec<String>,
    /// List of reserved node addresses.
    pub reserved_nodes: Vec<String>,
    /// The non-reserved peer mode.
    pub non_reserved_mode: Option<String>,
    /// Minimum number of connected peers to maintain
    pub max_peers: u32,
    pub outgoing_peers_ratio: Option<u32>,
    pub config_dir_path: Option<String>,
}

impl Config {
    fn max_outgoing_peers(&self) -> u32 {
        self.max_peers / self
            .outgoing_peers_ratio
            .unwrap_or_else(|| DEFAULT_OUTGOING_PEERS_RATIO)
    }
    fn max_incoming_peers(&self) -> u32 {
        self.max_peers - self.max_outgoing_peers()
    }
}

impl From<Config> for NetworkConfig {
    fn from(config: Config) -> Self {
        let mut cfg = NetworkConfig::default();
        cfg.max_outgoing_peers = config.max_outgoing_peers();
        cfg.max_incoming_peers = config.max_incoming_peers();
        cfg.listen_addresses = config.listen_addresses;
        cfg.bootnodes = config.boot_nodes;
        cfg.reserved_peers = config.reserved_nodes;
        if let Some(value) = config.non_reserved_mode {
            cfg.reserved_only = match value.as_str() {
                "Accept" => false,
                "Deny" => true,
                _ => false,
            };
        }
        if let Some(dir_path) = config.config_dir_path {
            cfg.config_dir_path = Some(dir_path.clone());
            cfg.secret_key_path = Some(format!("{}/secret_key", dir_path))
        }
        cfg.client_version = "ckb network".to_string();
        match cfg.read_secret_key() {
            Some(raw_key) => cfg.secret_key = Some(raw_key),
            None => {
                cfg.generate_random_key().expect("generate random key");
                cfg.write_secret_key_to_file().expect("write random key");
            }
        }
        cfg
    }
}

pub fn random_peer_id() -> Result<PeerId, Error> {
    let mut key: [u8; 32] = [0; 32];
    rand::rngs::EntropyRng::new().fill(&mut key);
    let local_private_key = secio::SecioKeyPair::secp256k1_raw_key(&key)
        .map_err(|err| ErrorKind::Other(err.description().to_string()))?;
    Ok(local_private_key.to_peer_id())
}
