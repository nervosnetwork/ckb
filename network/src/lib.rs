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
    peer_store::Score,
    protocols::{CKBProtocol, CKBProtocolContext, CKBProtocolHandler, PeerIndex},
};
pub use p2p::{
    multiaddr,
    secio::{PeerId, PublicKey},
    service::{ServiceControl, SessionType, TargetSession},
    traits::ServiceProtocol,
    ProtocolId,
};

// Max message frame length: 20MB
pub const MAX_FRAME_LENGTH: usize = 20 * 1024 * 1024;
pub type ProtocolVersion = String;
