mod behaviour;
mod config;
pub mod errors;
mod network_event;
mod network_group;
mod network_service;
mod network_state;
mod peer;
pub mod peer_store;
mod peers_registry;
mod protocols;

#[cfg(test)]
mod tests;

pub use crate::{
    behaviour::Behaviour,
    config::NetworkConfig,
    errors::Error,
    network_service::{NetworkController, NetworkService},
    network_state::{NetworkState, SessionInfo},
    peer::{Peer, PeerIdentifyInfo},
    peer_store::Score,
    protocols::{CKBProtocol, CKBProtocolContext, CKBProtocolHandler, ProtocolVersion},
};
pub use p2p::{
    context::{ProtocolContext, ProtocolContextMutRef, ServiceContext, SessionContext},
    multiaddr,
    secio::{PeerId, PublicKey},
    service::{ServiceControl, SessionType},
    ProtocolId, SessionId,
};

pub use tokio;

pub type MultiaddrList = Vec<(multiaddr::Multiaddr, u8)>;

// basic protcol ids
pub const PING_PROTOCOL_ID: ProtocolId = 0;
pub const DISCOVERY_PROTOCOL_ID: ProtocolId = 1;
pub const IDENTIFY_PROTOCOL_ID: ProtocolId = 2;
pub const FEELER_PROTOCOL_ID: ProtocolId = 3;
