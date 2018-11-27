use super::PeerId;
use super::{Error, ErrorKind};
use bytes::Bytes;
use libp2p::core::{AddrComponent, Multiaddr};
use libp2p::multiaddr::ToMultiaddr;
use libp2p::secio;
use rand;
use rand::Rng;
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
    pub transport_timeout: Duration,
    pub reserved_only: bool,
    pub max_incoming_peers: u32,
    pub max_outgoing_peers: u32,
    pub reserved_peers: Vec<String>,
    pub secret_key: Option<Bytes>,
    pub secret_key_path: Option<String>,
    // peer_store path
    pub config_dir_path: Option<String>,
    pub bootnodes: Vec<String>,
    pub ping_interval: Duration,
    pub ping_timeout: Duration,
    pub discovery_timeout: Duration,
    pub discovery_response_count: usize,
    pub discovery_interval: Duration,
    pub identify_timeout: Duration,
    pub identify_interval: Duration,
    pub outgoing_timeout: Duration,
    pub outgoing_interval: Duration,
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

    pub fn generate_random_key(&mut self) -> Result<secio::SecioKeyPair, IoError> {
        info!(target: "network", "Generate random key");
        let mut key: [u8; 32] = [0; 32];
        rand::thread_rng().fill(&mut key);
        self.secret_key = Some(Bytes::from(key.to_vec()));
        secio::SecioKeyPair::secp256k1_raw_key(&key).map_err(|err| {
            IoError::new(
                IoErrorKind::InvalidData,
                format!("generate random key error: {:?}", err),
            )
        })
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

    pub fn fetch_private_key(&self) -> Option<Result<secio::SecioKeyPair, IoError>> {
        if let Some(secret) = self.read_secret_key() {
            Some(
                secio::SecioKeyPair::secp256k1_raw_key(&secret).map_err(|err| {
                    IoError::new(
                        IoErrorKind::InvalidData,
                        format!("fetch private_key error: {:?}", err),
                    )
                }),
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
                .map_err(|_| ErrorKind::ParseAddress)?;
            let peer_id = match addr.pop() {
                Some(AddrComponent::P2P(key)) => {
                    PeerId::from_bytes(key.into_bytes()).map_err(|_| ErrorKind::ParseAddress)?
                }
                _ => return Err(ErrorKind::ParseAddress.into()),
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
                .map_err(|_| ErrorKind::ParseAddress)?;
            let peer_id = match addr.pop() {
                Some(AddrComponent::P2P(key)) => {
                    PeerId::from_bytes(key.into_bytes()).map_err(|_| ErrorKind::ParseAddress)?
                }
                _ => return Err(ErrorKind::ParseAddress.into()),
            };
            peers.push((peer_id, addr));
        }
        Ok(peers)
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        NetworkConfig {
            listen_addresses: vec![iter::once(AddrComponent::IP4(Ipv4Addr::new(0, 0, 0, 0)))
                .chain(iter::once(AddrComponent::TCP(30333)))
                .collect()],
            public_addresses: Vec::new(),
            client_version: "ckb<unknown>".to_owned(),
            protocol_version: "ckb".to_owned(),
            transport_timeout: Duration::from_secs(20),
            reserved_only: false,
            max_outgoing_peers: 15,
            max_incoming_peers: 10,
            reserved_peers: vec![],
            secret_key: None,
            secret_key_path: None,
            bootnodes: vec![],
            config_dir_path: None,
            // protocol services config
            ping_interval: Duration::from_secs(30),
            ping_timeout: Duration::from_secs(30),
            discovery_timeout: Duration::from_secs(20),
            discovery_response_count: 20,
            discovery_interval: Duration::from_secs(15),
            identify_timeout: Duration::from_secs(30),
            identify_interval: Duration::from_secs(15),
            outgoing_timeout: Duration::from_secs(30),
            outgoing_interval: Duration::from_secs(15),
        }
    }
}
