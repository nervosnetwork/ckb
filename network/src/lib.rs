//! TODO(doc): @driftluo
mod behaviour;
mod compress;
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
    errors::Error,
    network::{DefaultExitHandler, ExitHandler, NetworkController, NetworkService, NetworkState},
    peer::{Peer, PeerIdentifyInfo},
    peer_registry::PeerRegistry,
    peer_store::{types::MultiaddrExt, Score},
    protocols::{
        support_protocols::SupportProtocols, CKBProtocol, CKBProtocolContext, CKBProtocolHandler,
        PeerIndex,
    },
};
pub use p2p::{
    bytes, multiaddr,
    secio::{PeerId, PublicKey},
    service::{BlockingFlag, ServiceControl, SessionType, TargetSession},
    traits::ServiceProtocol,
    ProtocolId,
};
pub use tokio;

/// TODO(doc): @driftluo
pub type ProtocolVersion = String;
