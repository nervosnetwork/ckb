//! Anchors
use crate::peer_registry::MAX_OUTBOUND_BLOCK_RELAY;
use p2p::multiaddr::Multiaddr;
use std::collections::HashSet;

/// Anchor IP address database,
/// created on shutdown and deleted at startup.
/// Anchors are last known outgoing block-relay-only peers that
/// are tried to re-connect to on startup
#[derive(Default)]
pub struct Anchors {
    addrs: HashSet<Multiaddr>,
}

impl Anchors {
    /// Add an address information to anchors
    pub fn add(&mut self, addr: Multiaddr) {
        self.addrs.insert(addr);
    }

    /// The count of address in anchors
    pub fn count(&self) -> usize {
        self.addrs.len()
    }

    /// Anchors dump iterator, take MAX_OUTBOUND_BLOCK_RELAY
    pub fn dump_iter(&self) -> impl Iterator<Item = &Multiaddr> {
        self.addrs.iter().take(MAX_OUTBOUND_BLOCK_RELAY as usize)
    }

    /// Anchors drain
    pub fn drain(&mut self) -> impl Iterator<Item = Multiaddr> {
        self.addrs.drain()
    }

    /// Whether Anchors contains specified addr
    pub fn contains(&self, addr: &Multiaddr) -> bool {
        self.addrs.contains(addr)
    }
}
