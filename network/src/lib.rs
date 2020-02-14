mod behaviour;
mod compress;
mod config;
pub mod errors;
pub mod network;
mod network_group;
mod peer;
pub mod peer_registry;
pub mod peer_store;
mod protocols;
mod services;

#[cfg(test)]
mod tests;

pub use crate::{
    behaviour::Behaviour,
    config::NetworkConfig,
    errors::Error,
    network::{NetworkController, NetworkService, NetworkState},
    peer::{Peer, PeerIdentifyInfo},
    peer_registry::PeerRegistry,
    peer_store::{types::MultiaddrExt, Score},
    protocols::{CKBProtocol, CKBProtocolContext, CKBProtocolHandler, PeerIndex},
};
pub use p2p::{
    bytes, multiaddr,
    secio::{PeerId, PublicKey},
    service::{ServiceControl, SessionType, TargetSession},
    traits::ServiceProtocol,
    ProtocolId,
};

// Max message frame length for sync protocol: 2MB
//   NOTE: update this value when block size limit changed
pub const MAX_FRAME_LENGTH_SYNC: usize = 2 * 1024 * 1024;
// Max message frame length for relay protocol: 4MB
//   NOTE: update this value when block size limit changed
pub const MAX_FRAME_LENGTH_RELAY: usize = 4 * 1024 * 1024;
// Max message frame length for time protocol: 1KB
pub const MAX_FRAME_LENGTH_TIME: usize = 1024;
// Max message frame length for alert protocol: 128KB
pub const MAX_FRAME_LENGTH_ALERT: usize = 128 * 1024;
// Max message frame length for discovery protocol: 512KB
pub const MAX_FRAME_LENGTH_DISCOVERY: usize = 512 * 1024;
// Max message frame length for ping protocol: 1KB
pub const MAX_FRAME_LENGTH_PING: usize = 1024;
// Max message frame length for identify protocol: 2KB
pub const MAX_FRAME_LENGTH_IDENTIFY: usize = 2 * 1024;
// Max message frame length for disconnectmsg protocol: 1KB
pub const MAX_FRAME_LENGTH_DISCONNECTMSG: usize = 1024;
// Max message frame length for feeler protocol: 1KB
pub const MAX_FRAME_LENGTH_FEELER: usize = 1024;

// Max data size in send buffer: 24MB (a little larger than max frame length)
pub const DEFAULT_SEND_BUFFER: usize = 24 * 1024 * 1024;

pub type ProtocolVersion = String;
