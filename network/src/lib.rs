#[macro_use]
pub extern crate futures;
mod behaviour;
pub mod errors;
pub mod network;
mod network_config;
mod network_group;
mod peer;
pub mod peer_store;
pub mod peers_registry;
pub mod protocol;
mod protocols;
mod service;
#[cfg(test)]
mod tests;

pub use crate::{
    behaviour::Behaviour,
    errors::Error,
    network::{NetworkController, NetworkService, NetworkState, SessionInfo},
    network_config::NetworkConfig,
    peer::{Peer, PeerIdentifyInfo},
    peer_store::Score,
    peers_registry::RegisterResult,
    protocol::ckb::{CKBProtocol, ProtocolVersion},
    protocol::ckb_handler::{CKBProtocolContext, CKBProtocolHandler, Severity},
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
