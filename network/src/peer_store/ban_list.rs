//! Ban list
use crate::peer_store::Multiaddr;
use crate::peer_store::types::{BannedAddr, ip_to_network};
use crate::{PeerId, extract_peer_id};
use ckb_systemtime::unix_time_as_millis;
use ckb_util::LinkedHashMap;
use ipnetwork::IpNetwork;
use p2p::utils::multiaddr_to_socketaddr;
use std::collections::HashMap;
use std::net::IpAddr;

pub(crate) const CLEAR_INTERVAL_COUNTER: usize = 1024;
// Proxy clients can change identities, so bound these temporary bans.
pub(crate) const MAX_BANNED_PEERS: usize = 16384;

/// Ban list
pub struct BanList {
    inner: HashMap<IpNetwork, BannedAddr>,
    // Session identity bans are intentionally memory-only; the persisted/RPC ban list is IP based.
    peers: LinkedHashMap<PeerId, u64>,
    insert_count: usize,
}

impl Default for BanList {
    fn default() -> Self {
        Self::new()
    }
}

impl BanList {
    /// Init
    pub fn new() -> Self {
        BanList {
            inner: HashMap::default(),
            peers: LinkedHashMap::default(),
            insert_count: 0,
        }
    }

    /// Ban address
    pub fn ban(&mut self, banned_addr: BannedAddr) {
        self.inner.insert(banned_addr.address, banned_addr);
        let (insert_count, _) = self.insert_count.overflowing_add(1);
        self.insert_count = insert_count;
        if self.insert_count.is_multiple_of(CLEAR_INTERVAL_COUNTER) {
            self.clear_expires();
        }
    }

    /// Unban address
    pub fn unban_network(&mut self, ip_network: &IpNetwork) {
        self.inner.remove(ip_network);
    }

    pub(crate) fn ban_peer(&mut self, peer_id: PeerId, timeout_ms: u64) {
        let now = unix_time_as_millis();
        self.peers.insert(peer_id, now.saturating_add(timeout_ms));
        if self.peers.len() > MAX_BANNED_PEERS {
            self.peers.pop_front();
        }
    }

    fn is_ip_banned_until(&self, ip: IpAddr, now_ms: u64) -> bool {
        let ip_network = ip_to_network(ip);
        if let Some(banned_addr) = self.inner.get(&ip_network)
            && banned_addr.ban_until.gt(&now_ms)
        {
            return true;
        }

        self.inner.iter().any(|(ip_network, banned_addr)| {
            banned_addr.ban_until.gt(&now_ms) && ip_network.contains(ip)
        })
    }

    /// Whether the ip is banned
    pub fn is_ip_banned(&self, ip: &IpAddr) -> bool {
        let now_ms = unix_time_as_millis();
        self.is_ip_banned_until(ip.to_owned(), now_ms)
    }

    /// Whether the address is banned
    pub fn is_addr_banned(&self, addr: &Multiaddr) -> bool {
        let peer_banned = extract_peer_id(addr)
            .and_then(|peer_id| self.peers.get(&peer_id))
            .is_some_and(|until| *until > unix_time_as_millis());
        peer_banned
            || multiaddr_to_socketaddr(addr)
                .map(|socket_addr| self.is_ip_banned(&socket_addr.ip()))
                .unwrap_or_default()
    }

    /// Get banned address list
    pub fn get_banned_addrs(&self) -> Vec<BannedAddr> {
        self.inner.values().map(ToOwned::to_owned).collect()
    }

    fn clear_expires(&mut self) {
        let now = unix_time_as_millis();
        self.inner
            .retain(|_, banned_addr| banned_addr.ban_until.gt(&now));
    }

    /// Get the numbers of banned address
    pub fn count(&self) -> usize {
        self.inner.len()
    }
}
