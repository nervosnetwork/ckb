//! ckb network module
//!
//! This module is based on the Tentacle library, once again abstract the context that protocols can use,
//! and providing a unified implementation of the peer storage and registration mechanism.
//!
//! And implemented several basic protocols: identify, discovery, ping, feeler, disconnect_message
//!

mod behaviour;
/// compress module
pub mod compress;
pub mod errors;
pub mod network;
mod network_group;
mod peer;
pub mod peer_registry;
pub mod peer_store;
mod protocols;

#[cfg(not(target_family = "wasm"))]
mod proxy;
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
    peer::{Peer, PeerIdentifyInfo, SessionType},
    peer_registry::PeerRegistry,
    peer_store::Score,
    protocols::{
        BoxedCKBProtocolContext, CKBProtocol, CKBProtocolContext, CKBProtocolHandler, PeerIndex,
        identify::Flags, support_protocols::SupportProtocols,
    },
};
pub use p2p::{
    ProtocolId, SessionId, async_trait,
    builder::ServiceBuilder,
    bytes, multiaddr, runtime,
    secio::{self, PeerId, PublicKey},
    service::{
        ServiceAsyncControl, ServiceControl, SessionType as RawSessionType, TargetProtocol,
        TargetSession,
    },
    traits::ServiceProtocol,
    utils::{extract_peer_id, multiaddr_to_socketaddr},
};
pub use tokio;

/// Protocol version used by network protocol open
pub type ProtocolVersion = String;

/// Observe listen port occupancy
pub async fn observe_listen_port_occupancy(
    _addrs: &[multiaddr::Multiaddr],
) -> Result<(), std::io::Error> {
    #[cfg(target_os = "linux")]
    {
        use p2p::utils::dns::DnsResolver;
        use std::net::{SocketAddr, TcpListener};

        for raw_addr in _addrs {
            let ip_addr: Option<SocketAddr> = match DnsResolver::new(raw_addr.clone()) {
                Some(dns) => dns.await.ok().as_ref().and_then(multiaddr_to_socketaddr),
                None => multiaddr_to_socketaddr(raw_addr),
            };

            if let Some(addr) = ip_addr {
                if let Err(e) = TcpListener::bind(addr) {
                    ckb_logger::error!(
                        "addr {} can't use on your machines by error: {}, please check",
                        raw_addr,
                        e
                    );
                    return Err(e);
                }
            }
        }
    }

    Ok(())
}
