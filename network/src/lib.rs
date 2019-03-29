#[macro_use]
pub extern crate futures;
mod behaviour;
mod config;
pub mod errors;
pub mod network;
mod network_group;
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
    network::{NetworkController, NetworkService, NetworkState, SessionInfo},
    peer::{Peer, PeerIdentifyInfo},
    peer_store::Score,
    peers_registry::RegisterResult,
    protocols::{CKBProtocol, CKBProtocolContext, CKBProtocolHandler, ProtocolVersion},
};
pub use p2p::{
    context::{ProtocolContext, ProtocolContextMutRef, ServiceContext, SessionContext},
    multiaddr,
    secio::PeerId,
    service::ServiceControl,
    yamux::session::SessionType,
    ProtocolId, SessionId,
};

// used in CKBProtocolContext
pub type PeerIndex = usize;
