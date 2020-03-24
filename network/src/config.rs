use crate::{
    errors::{ConfigError, Error},
    PeerId, DEFAULT_SEND_BUFFER,
};
use ckb_logger::info;
use p2p::{
    multiaddr::{Multiaddr, Protocol},
    secio,
};
use rand;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::io::Write;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct NetworkConfig {
    #[serde(default)]
    pub whitelist_only: bool,
    pub max_peers: u32,
    pub max_outbound_peers: u32,
    #[serde(default)]
    pub path: PathBuf,
    #[serde(default)]
    pub dns_seeds: Vec<String>,
    // Set if discovery add local address to peer store
    #[serde(default)]
    pub discovery_local_address: bool,
    pub ping_interval_secs: u64,
    pub ping_timeout_secs: u64,
    pub connect_outbound_interval_secs: u64,
    pub listen_addresses: Vec<Multiaddr>,
    #[serde(default)]
    pub public_addresses: Vec<Multiaddr>,
    pub bootnodes: Vec<Multiaddr>,
    #[serde(default)]
    pub whitelist_peers: Vec<Multiaddr>,
    #[serde(default)]
    pub upnp: bool,
    #[serde(default)]
    pub bootnode_mode: bool,
    // Max send buffer size
    pub max_send_buffer: Option<usize>,
}

fn generate_random_key() -> [u8; 32] {
    loop {
        let mut key: [u8; 32] = [0; 32];
        rand::thread_rng().fill(&mut key);
        if secio::SecioKeyPair::secp256k1_raw_key(&key).is_ok() {
            return key;
        }
    }
}

impl NetworkConfig {
    pub fn secret_key_path(&self) -> PathBuf {
        let mut path = self.path.clone();
        path.push("secret_key");
        path
    }

    pub fn peer_store_path(&self) -> PathBuf {
        let mut path = self.path.clone();
        path.push("peer_store");
        path
    }

    pub fn create_dir_if_not_exists(&self) -> Result<(), Error> {
        if !self.path.exists() {
            fs::create_dir(&self.path)?;
        }
        Ok(())
    }

    pub fn max_inbound_peers(&self) -> u32 {
        self.max_peers.saturating_sub(self.max_outbound_peers)
    }

    pub fn max_outbound_peers(&self) -> u32 {
        self.max_outbound_peers
    }

    pub fn max_send_buffer(&self) -> usize {
        self.max_send_buffer.unwrap_or(DEFAULT_SEND_BUFFER)
    }

    fn read_secret_key(&self) -> Result<Option<secio::SecioKeyPair>, Error> {
        let path = self.secret_key_path();
        let mut file = match fs::File::open(path) {
            Ok(file) => file,
            Err(_) => return Ok(None),
        };
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        Ok(Some(secio::SecioKeyPair::secp256k1_raw_key(&buf).map_err(
            |_err: secio::error::SecioError| ConfigError::InvalidKey,
        )?))
    }

    fn write_secret_key_to_file(&self) -> Result<(), Error> {
        let path = self.secret_key_path();
        info!("Generate random key");
        let random_key_pair = generate_random_key();
        info!("write random secret key to {:?}", path);
        fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(path)
            .and_then(|mut file| file.write_all(&random_key_pair))
            .map_err(Into::into)
    }

    pub fn fetch_private_key(&self) -> Result<secio::SecioKeyPair, Error> {
        match self.read_secret_key()? {
            Some(key) => Ok(key),
            None => {
                self.write_secret_key_to_file()?;
                Ok(self.read_secret_key()?.expect("key must exists"))
            }
        }
    }

    pub fn whitelist_peers(&self) -> Result<Vec<(PeerId, Multiaddr)>, Error> {
        let mut peers = Vec::with_capacity(self.whitelist_peers.len());
        for addr_str in &self.whitelist_peers {
            let mut addr = addr_str.to_owned();
            let peer_id = match addr.pop() {
                Some(Protocol::P2P(key)) => {
                    PeerId::from_bytes(key.to_vec()).map_err(|_| ConfigError::BadAddress)?
                }
                _ => return Err(ConfigError::BadAddress.into()),
            };
            peers.push((peer_id, addr))
        }
        Ok(peers)
    }

    pub fn bootnodes(&self) -> Result<Vec<(PeerId, Multiaddr)>, Error> {
        let mut peers = Vec::with_capacity(self.bootnodes.len());
        for addr_str in &self.bootnodes {
            let mut addr = addr_str.to_owned();
            let peer_id = match addr.pop() {
                Some(Protocol::P2P(key)) => {
                    PeerId::from_bytes(key.to_vec()).map_err(|_| ConfigError::BadAddress)?
                }
                _ => return Err(ConfigError::BadAddress.into()),
            };
            peers.push((peer_id, addr));
        }
        Ok(peers)
    }

    pub fn outbound_peer_service_enabled(&self) -> bool {
        self.connect_outbound_interval_secs > 0
    }

    pub fn dns_seeding_service_enabled(&self) -> bool {
        !self.dns_seeds.is_empty()
    }
}
