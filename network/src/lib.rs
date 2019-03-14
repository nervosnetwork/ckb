#[macro_use]
pub extern crate futures;
pub mod errors;
mod network;
mod network_config;
mod network_group;
mod network_service;
pub mod peer_store;
pub mod peers_registry;
mod protocol;
mod protocol_handler;
mod service;
#[cfg(test)]
mod tests;

pub use crate::network::{Network, PeerInfo, SessionInfo};
pub use crate::network_config::NetworkConfig;
pub use crate::network_service::NetworkService;
pub use crate::peers_registry::RegisterResult;
pub use crate::protocol::{CKBProtocol, Event as CKBEvent, Version as ProtocolVersion};
pub use crate::protocol_handler::{CKBProtocolContext, CKBProtocolHandler, Severity};
pub use crate::service::timer_service::{Timer, TimerRegistry, TimerToken};
pub use p2p::{multiaddr, secio::PeerId, yamux::session::SessionType, ProtocolId};
// p2p internal expose
pub(crate) use p2p::{
    context::{ServiceContext, SessionContext},
    service::ServiceControl,
};
use serde_derive::Deserialize;
use std::time::Duration;

const DEFAULT_OUTGOING_PEERS_RATIO: u32 = 3;

// used in CKBProtocolContext
pub type PeerIndex = usize;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Config {
    pub listen_addresses: Vec<multiaddr::Multiaddr>,
    pub secret_file: Option<String>,
    pub try_outbound_connect_secs: Option<u64>,
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
        if let Some(try_outbound_connect_secs) = config.try_outbound_connect_secs {
            cfg.try_outbound_connect_interval = Duration::from_secs(try_outbound_connect_secs);
        }
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

pub fn random_peer_id() -> PeerId {
    use p2p::secio::SecioKeyPair;
    let pubkey = SecioKeyPair::secp256k1_generated().to_public_key();
    PeerId::from_public_key(&pubkey)
}
