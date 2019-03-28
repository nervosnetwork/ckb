#[macro_use]
pub extern crate futures;
mod behaviour;
pub mod errors;
pub mod network;
mod network_config;
mod network_group;
mod network_service;
mod peer;
pub mod peer_store;
pub mod peers_registry;
pub mod protocol;
mod service;
#[cfg(test)]
mod tests;

pub use crate::behaviour::Behaviour;
pub use crate::network::{Network, SessionInfo};
pub use crate::network_config::NetworkConfig;
pub use crate::network_service::NetworkService;
pub use crate::peer::{Peer, PeerIdentifyInfo};
pub use crate::peer_store::Score;
pub use crate::peers_registry::RegisterResult;
pub use crate::protocol::{
    ckb::{CKBProtocol, Event as CKBEvent, Version as ProtocolVersion},
    ckb_handler::{CKBProtocolContext, CKBProtocolHandler, Severity},
};
pub use crate::service::timer_service::{Timer, TimerRegistry, TimerToken};
pub use errors::Error;
pub use p2p::{multiaddr, secio::PeerId, yamux::session::SessionType, ProtocolId};
// p2p internal expose
pub(crate) use p2p::{
    context::{ServiceContext, SessionContext},
    service::ServiceControl,
    SessionId,
};

// used in CKBProtocolContext
pub type PeerIndex = usize;
