#![type_length_limit = "2097152"]

mod ckb_protocol;
mod ckb_protocol_handler;
mod ckb_service;
mod errors;
mod identify_service;
mod network;
mod network_config;
mod network_group;
mod network_service;
mod outbound_peer_service;
mod peer_store;
mod peers_registry;
mod ping_service;
mod protocol;
mod protocol_service;
#[cfg(test)]
mod tests;
mod timer_service;
mod transport;

pub use crate::ckb_protocol::{CKBProtocol, CKBProtocols};
pub use crate::ckb_protocol_handler::{CKBProtocolContext, CKBProtocolHandler, Severity};
pub use crate::errors::{Error, ErrorKind};
pub use crate::network::{Network, PeerInfo, SessionInfo};
pub use crate::network_config::NetworkConfig;
pub use crate::network_service::NetworkService;
pub use libp2p::{
    core::Endpoint, multiaddr::AddrComponent, multiaddr::ToMultiaddr, Multiaddr, PeerId,
};

pub type TimerToken = usize;
pub type ProtocolId = [u8; 3];

use multihash::{encode, Hash};
use rand::Rng;
use serde_derive::Deserialize;
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
    pub bootnodes: Vec<String>,
    /// List of reserved node addresses.
    pub reserved_nodes: Vec<String>,
    /// The non-reserved peer mode.
    pub non_reserved_mode: Option<String>,
    /// Minimum number of connected peers to maintain
    pub max_peers: u32,
    pub outbound_peers_ratio: Option<u32>,
    pub config_dir_path: Option<String>,
}

impl Config {
    fn max_outbound_peers(&self) -> u32 {
        self.max_peers
            / self
                .outbound_peers_ratio
                .unwrap_or_else(|| DEFAULT_OUTGOING_PEERS_RATIO)
    }
    fn max_inbound_peers(&self) -> u32 {
        self.max_peers - self.max_outbound_peers()
    }
}

impl From<Config> for NetworkConfig {
    fn from(config: Config) -> Self {
        let mut cfg = NetworkConfig::default();
        cfg.max_outbound_peers = config.max_outbound_peers();
        cfg.max_inbound_peers = config.max_inbound_peers();
        cfg.listen_addresses = config.listen_addresses;
        cfg.bootnodes = config.bootnodes;
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
    let mut seed: [u8; 32] = [0; 32];
    rand::thread_rng().fill(&mut seed);
    let random_key = encode(Hash::SHA2256, &seed)
        .expect("sha2256 encode")
        .into_bytes();
    let peer_id = PeerId::from_bytes(random_key).expect("convert key to peer_id");
    Ok(peer_id)
}
