#[macro_use]
pub extern crate futures;
mod behaviour;
pub mod errors;
mod network;
mod network_config;
mod network_group;
mod network_service;
pub mod peer_store;
pub mod peers_registry;
mod protocol;
mod protocol_handler;
mod service;
#[cfg(test)]
mod tests;

pub use crate::behaviour::Behaviour;
pub use crate::network::{Network, PeerInfo, SessionInfo};
pub use crate::network_config::NetworkConfig;
pub use crate::network_service::NetworkService;
pub use crate::peer_store::Score;
pub use crate::peers_registry::RegisterResult;
pub use crate::protocol::{CKBProtocol, Event as CKBEvent, Version as ProtocolVersion};
pub use crate::protocol_handler::{CKBProtocolContext, CKBProtocolHandler, Severity};
pub use crate::service::timer_service::{Timer, TimerRegistry, TimerToken};
pub use errors::Error;
pub use p2p::{multiaddr, secio::PeerId, yamux::session::SessionType, ProtocolId};
// p2p internal expose
pub(crate) use p2p::{
    context::{ServiceContext, SessionContext},
    service::ServiceControl,
};

// used in CKBProtocolContext
pub type PeerIndex = usize;
