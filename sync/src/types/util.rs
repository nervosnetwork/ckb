use crate::Status;
use ckb_constant::sync::RETRY_ASK_TX_TIMEOUT_INCREASE;
use ckb_logger::{debug, error, warn};
use ckb_network::{CKBProtocolContext, PeerIndex};
use lru::LruCache;
use std::cmp;
use std::hash::Hash;
use std::time::Instant;

pub(crate) const FILTER_SIZE: usize = 50000;
// 2 ** 13 < 6 * 1800 < 2 ** 14
pub(crate) const FILTER_TTL: u64 = 4 * 60 * 60;

pub struct TtlFilter<T> {
    inner: LruCache<T, u64>,
    ttl: u64,
}

impl<T: Eq + Hash + Clone> Default for TtlFilter<T> {
    fn default() -> Self {
        TtlFilter::new(FILTER_SIZE, FILTER_TTL)
    }
}

impl<T: Eq + Hash + Clone> TtlFilter<T> {
    pub fn new(size: usize, ttl: u64) -> Self {
        Self {
            inner: LruCache::new(size),
            ttl,
        }
    }

    pub fn contains(&self, item: &T) -> bool {
        self.inner.contains(item)
    }

    pub fn insert(&mut self, item: T) -> bool {
        let now = ckb_systemtime::unix_time().as_secs();
        self.inner.put(item, now).is_none()
    }

    pub fn remove(&mut self, item: &T) -> bool {
        self.inner.pop(item).is_some()
    }

    /// Removes expired items.
    pub fn remove_expired(&mut self) {
        let now = ckb_systemtime::unix_time().as_secs();
        let expired_keys: Vec<T> = self
            .inner
            .iter()
            .filter_map(|(key, time)| {
                if *time + self.ttl < now {
                    Some(key)
                } else {
                    None
                }
            })
            .cloned()
            .collect();

        for k in expired_keys {
            self.remove(&k);
        }
    }
}

#[derive(Eq, PartialEq, Clone)]
pub struct UnknownTxHashPriority {
    pub(crate) request_time: Instant,
    pub(crate) peers: Vec<PeerIndex>,
    pub(crate) requested: bool,
}

impl UnknownTxHashPriority {
    pub fn should_request(&self, now: Instant) -> bool {
        self.next_request_at() < now
    }

    pub fn next_request_at(&self) -> Instant {
        if self.requested {
            self.request_time + RETRY_ASK_TX_TIMEOUT_INCREASE
        } else {
            self.request_time
        }
    }

    pub fn next_request_peer(&mut self) -> Option<PeerIndex> {
        if self.requested {
            if self.peers.len() > 1 {
                self.request_time = Instant::now();
                self.peers.swap_remove(0);
                self.peers.first().cloned()
            } else {
                None
            }
        } else {
            self.requested = true;
            self.peers.first().cloned()
        }
    }

    pub fn push_peer(&mut self, peer_index: PeerIndex) {
        self.peers.push(peer_index);
    }

    pub fn requesting_peer(&self) -> Option<PeerIndex> {
        if self.requested {
            self.peers.first().cloned()
        } else {
            None
        }
    }
}

impl Ord for UnknownTxHashPriority {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.next_request_at()
            .cmp(&other.next_request_at())
            .reverse()
    }
}

impl PartialOrd for UnknownTxHashPriority {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// The `IBDState` enum represents whether the node is currently in the IBD process (`In`) or has
/// completed it (`Out`).
#[derive(Clone, Copy, Debug)]
pub enum IBDState {
    In,
    Out,
}

impl From<bool> for IBDState {
    fn from(src: bool) -> Self {
        if src { IBDState::In } else { IBDState::Out }
    }
}

impl From<IBDState> for bool {
    fn from(s: IBDState) -> bool {
        match s {
            IBDState::In => true,
            IBDState::Out => false,
        }
    }
}

pub(crate) fn post_sync_process(
    nc: &dyn CKBProtocolContext,
    peer: PeerIndex,
    item_name: &str,
    status: Status,
) {
    if let Some(ban_time) = status.should_ban() {
        error!(
            "Receive {} from {}. Ban {:?} for {}",
            item_name, peer, ban_time, status
        );
        nc.ban_peer(peer, ban_time, status.to_string());
    } else if status.should_warn() {
        warn!("Receive {} from {}, {}", item_name, peer, status);
    } else if !status.is_ok() {
        debug!("Receive {} from {}, {}", item_name, peer, status);
    }
}
