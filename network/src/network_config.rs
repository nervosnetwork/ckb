use crate::errors::{ConfigError, Error};
use crate::PeerId;
use bytes::Bytes;
use log::info;
use p2p::multiaddr::{self, Multiaddr, Protocol, ToMultiaddr};
use rand;
use rand::Rng;
use secio;
use std::fs;
use std::io::Read;
use std::io::Write;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::iter;
use std::net::Ipv4Addr;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct NetworkConfig {
    pub listen_addresses: Vec<Multiaddr>,
    pub public_addresses: Vec<Multiaddr>,
    pub client_version: String,
    pub protocol_version: String,
    pub reserved_only: bool,
    pub max_inbound_peers: u32,
    pub max_outbound_peers: u32,
    pub reserved_peers: Vec<String>,
    pub secret_key: Option<Bytes>,
    pub secret_key_path: Option<String>,
    // peer_store path
    pub config_dir_path: Option<String>,
    pub bootnodes: Vec<String>,
    pub ping_interval: Duration,
    pub discovery_timeout: Duration,
    pub discovery_response_count: usize,
    pub discovery_interval: Duration,
    pub try_outbound_connect_interval: Duration,
}

impl NetworkConfig {
    pub(crate) fn read_secret_key(&self) -> Option<Bytes> {
        if self.secret_key.is_some() {
            self.secret_key.clone()
        } else if let Some(ref path) = self.secret_key_path {
            match fs::File::open(path).and_then(|mut file| {
                let mut buf = Vec::new();
                file.read_to_end(&mut buf).map(|_| buf)
            }) {
                Ok(secret) => Some(secret.into()),
                Err(_err) => None,
            }
        } else {
            None
        }
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn generate_random_key(&mut self) -> Result<secio::SecioKeyPair, Error> {
        info!(target: "network", "Generate random key");
        let mut key: [u8; 32] = [0; 32];
        rand::thread_rng().fill(&mut key);
        self.secret_key = Some(Bytes::from(key.to_vec()));
        secio::SecioKeyPair::secp256k1_raw_key(&key).map_err(|_err| ConfigError::InvalidKey.into())
    }

    pub fn write_secret_key_to_file(&mut self) -> Result<(), IoError> {
        if let Some(ref secret_key_path) = self.secret_key_path {
            if let Some(secret_key) = self.secret_key.clone() {
                info!(target: "network", "write random secret key to {}", secret_key_path);
                return fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .open(secret_key_path)
                    .and_then(|mut file| file.write_all(&secret_key));
            }
        }
        Ok(())
    }

    pub fn fetch_private_key(&self) -> Option<Result<secio::SecioKeyPair, Error>> {
        if let Some(secret) = self.read_secret_key() {
            Some(
                secio::SecioKeyPair::secp256k1_raw_key(&secret)
                    .map_err(|_err| ConfigError::InvalidKey.into()),
            )
        } else {
            None
        }
    }

    pub fn reserved_peers(&self) -> Result<Vec<(PeerId, Multiaddr)>, Error> {
        let mut peers = Vec::with_capacity(self.reserved_peers.len());
        for addr_str in &self.reserved_peers {
            let mut addr = addr_str
                .to_multiaddr()
                .map_err(|_| ConfigError::BadAddress)?;
            let peer_id = match addr.pop() {
                Some(Protocol::P2p(key)) => {
                    PeerId::from_bytes(key.into_bytes()).map_err(|_| ConfigError::BadAddress)?
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
            let mut addr = addr_str
                .to_multiaddr()
                .map_err(|_| ConfigError::BadAddress)?;
            let peer_id = match addr.pop() {
                Some(Protocol::P2p(key)) => {
                    PeerId::from_bytes(key.into_bytes()).map_err(|_| ConfigError::BadAddress)?
                }
                _ => return Err(ConfigError::BadAddress.into()),
            };
            peers.push((peer_id, addr));
        }
        Ok(peers)
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        NetworkConfig {
            listen_addresses: vec![iter::once(Protocol::Ip4(Ipv4Addr::new(0, 0, 0, 0)))
                .chain(iter::once(Protocol::Tcp(30333)))
                .collect()],
            public_addresses: Vec::new(),
            client_version: "ckb<unknown>".to_owned(),
            protocol_version: "ckb".to_owned(),
            reserved_only: false,
            max_outbound_peers: 15,
            max_inbound_peers: 10,
            reserved_peers: vec![],
            secret_key: None,
            secret_key_path: None,
            bootnodes: vec![],
            config_dir_path: None,
            // protocol services config
            ping_interval: Duration::from_secs(30),
            discovery_timeout: Duration::from_secs(20),
            discovery_response_count: 20,
            discovery_interval: Duration::from_secs(15),
            try_outbound_connect_interval: Duration::from_secs(15),
        }
    }
}
