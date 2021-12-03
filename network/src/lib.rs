//! ckb network module
//!
//! This module is based on the Tentacle library, once again abstract the context that protocols can use,
//! and providing a unified implementation of the peer storage and registration mechanism.
//!
//! And implemented several basic protocols: identify, discovery, ping, feeler, disconnect_message
//!

mod behaviour;
pub mod compress;
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
    network::{
        DefaultExitHandler, EventHandler, ExitHandler, NetworkController, NetworkService,
        NetworkState,
    },
    peer::{Peer, PeerIdentifyInfo},
    peer_registry::PeerRegistry,
    peer_store::Score,
    protocols::{
        support_protocols::SupportProtocols, CKBProtocol, CKBProtocolContext, CKBProtocolHandler,
        PeerIndex,
    },
};
pub use p2p::{
    builder::ServiceBuilder,
    bytes, multiaddr,
    secio::{PeerId, PublicKey},
    service::{BlockingFlag, ServiceControl, SessionType, TargetProtocol, TargetSession},
    traits::ServiceProtocol,
    utils::{extract_peer_id, multiaddr_to_socketaddr},
    ProtocolId, SessionId,
};
pub use tokio;

/// Protocol version used by network protocol open
pub type ProtocolVersion = String;
