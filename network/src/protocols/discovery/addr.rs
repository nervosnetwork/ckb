use std::collections::hash_map::RandomState;

use bloom_filters::{BloomFilter, DefaultBuildHashKernels, StableBloomFilter};
use p2p::{context::SessionContext, multiaddr::Multiaddr, ProtocolId, SessionId};

use crate::Flags;

pub(crate) const DEFAULT_BUCKETS_NUM: usize = 5000;

#[derive(Debug, Clone)]
pub enum Misbehavior {
    // Already received GetNodes message
    DuplicateGetNodes,
    // Already received Nodes(announce=false) message
    DuplicateFirstNodes,
    // Nodes message include too many items
    TooManyItems { announce: bool, length: usize },
    // Too many address in one item
    TooManyAddresses(usize),
    // Decode message error
    InvalidData,
}

/// Misbehavior report result
pub enum MisbehaveResult {
    /// Disconnect this peer
    Disconnect,
}

impl MisbehaveResult {
    pub fn is_disconnect(&self) -> bool {
        match self {
            MisbehaveResult::Disconnect => true,
            // _ => false,
        }
    }
}

// FIXME: Should be peer store?
pub trait AddressManager {
    fn register(&self, id: SessionId, pid: ProtocolId, version: &str);
    fn unregister(&self, id: SessionId, pid: ProtocolId);
    fn is_valid_addr(&self, addr: &Multiaddr) -> bool;
    fn add_new_addr(&mut self, session_id: SessionId, addr: (Multiaddr, Flags));
    fn add_new_addrs(&mut self, session_id: SessionId, addrs: Vec<(Multiaddr, Flags)>);
    fn misbehave(&mut self, session: &SessionContext, kind: &Misbehavior) -> MisbehaveResult;
    fn get_random(&mut self, n: usize, target: Flags) -> Vec<(Multiaddr, Flags)>;
    fn required_flags(&self) -> Flags;
    fn node_flags(&self, id: SessionId) -> Option<Flags>;
}

// bitcoin: bloom.h, bloom.cpp => CRollingBloomFilter
pub struct AddrKnown {
    filters: StableBloomFilter<DefaultBuildHashKernels<RandomState>>,
}

impl AddrKnown {
    pub(crate) fn new(buckets_num: usize) -> AddrKnown {
        AddrKnown {
            filters: StableBloomFilter::new(
                buckets_num,
                3,
                0.03,
                DefaultBuildHashKernels::new(rand::random(), RandomState::default()),
            ),
        }
    }

    pub(crate) fn insert<T: ::std::hash::Hash>(&mut self, key: T) {
        self.filters.insert(&key)
    }

    pub(crate) fn extend<'a, T: 'a + ::std::hash::Hash>(
        &mut self,
        keys: impl Iterator<Item = &'a T>,
    ) {
        for key in keys {
            self.filters.insert(key)
        }
    }

    pub(crate) fn contains<T: ::std::hash::Hash>(&self, addr: &T) -> bool {
        self.filters.contains(addr)
    }
}

impl Default for AddrKnown {
    fn default() -> AddrKnown {
        AddrKnown::new(DEFAULT_BUCKETS_NUM)
    }
}
