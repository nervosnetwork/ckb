mod behaviour;
mod config;
pub mod errors;
pub mod network;
mod network_group;
mod network_service;
mod peer;
pub mod peer_store;
pub mod peers_registry;
mod protocols;

#[cfg(test)]
mod tests;

pub use crate::{
    behaviour::Behaviour,
    config::NetworkConfig,
    errors::Error,
    network::{NetworkState, SessionInfo},
    network_service::{NetworkController, NetworkService},
    peer::{Peer, PeerIdentifyInfo},
    peer_store::Score,
    peers_registry::RegisterResult,
    protocols::{CKBProtocol, CKBProtocolContext, CKBProtocolHandler, ProtocolVersion},
};
pub use p2p::{
    context::{ProtocolContext, ProtocolContextMutRef, ServiceContext, SessionContext},
    multiaddr,
    secio::{PeerId, PublicKey},
    service::{ServiceControl, SessionType},
    ProtocolId, SessionId,
};

// used in CKBProtocolContext
pub type PeerIndex = usize;
pub type MultiaddrList = Vec<(multiaddr::Multiaddr, u8)>;

// basic protcol ids
pub const PING_PROTOCOL_ID: ProtocolId = 0;
pub const DISCOVERY_PROTOCOL_ID: ProtocolId = 1;
pub const IDENTIFY_PROTOCOL_ID: ProtocolId = 2;
pub const FEELER_PROTOCOL_ID: ProtocolId = 3;
