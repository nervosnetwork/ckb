use crate::{
    errors::{PeerStoreError, Result},
    extract_peer_id, multiaddr_to_socketaddr,
    network_group::Group,
    peer_store::{
        addr_manager::AddrManager,
        ban_list::BanList,
        types::{ip_to_network, AddrInfo, BannedAddr, PeerInfo},
        Behaviour, Multiaddr, PeerScoreConfig, ReportResult, Status, ADDR_COUNT_LIMIT,
        ADDR_TIMEOUT_MS,
    },
    PeerId, SessionType,
};
use ipnetwork::IpNetwork;
use std::cell::{Ref, RefCell};
use std::collections::{hash_map::Entry, HashMap};

/// Peer store
#[derive(Default)]
pub struct PeerStore {
    addr_manager: AddrManager,
    ban_list: RefCell<BanList>,
    peers: RefCell<HashMap<PeerId, PeerInfo>>,
    score_config: PeerScoreConfig,
}

impl PeerStore {
    /// New with address list and ban list
    pub fn new(addr_manager: AddrManager, ban_list: BanList) -> Self {
        PeerStore {
            addr_manager,
            ban_list: RefCell::new(ban_list),
            peers: Default::default(),
            score_config: Default::default(),
        }
    }

    /// Add a peer and address into peer_store
    /// this method will assume peer is connected, which implies address is "verified".
    pub fn add_connected_peer(&mut self, addr: Multiaddr, session_type: SessionType) -> Result<()> {
        let now_ms = faketime::unix_time_as_millis();
        match self
            .peers
            .get_mut()
            .entry(extract_peer_id(&addr).expect("connected addr should have peer id"))
        {
            Entry::Occupied(mut entry) => {
                let mut peer = entry.get_mut();
                peer.connected_addr = addr.clone();
                peer.last_connected_at_ms = now_ms;
                peer.session_type = session_type;
            }
            Entry::Vacant(entry) => {
                let peer = PeerInfo::new(addr.clone(), session_type, now_ms);
                entry.insert(peer);
            }
        }
        let score = self.score_config.default_score;
        if session_type.is_outbound() {
            self.addr_manager.add(AddrInfo::new(addr, now_ms, score));
        }
        Ok(())
    }

    /// Add discovered peer addresses
    /// this method will assume peer and addr is untrust since we have not connected to it.
    pub fn add_addr(&mut self, addr: Multiaddr) -> Result<()> {
        self.check_purge()?;
        let score = self.score_config.default_score;
        self.addr_manager.add(AddrInfo::new(addr, 0, score));
        Ok(())
    }

    /// Get address manager
    pub fn addr_manager(&self) -> &AddrManager {
        &self.addr_manager
    }

    /// Get mut address manager
    pub fn mut_addr_manager(&mut self) -> &mut AddrManager {
        &mut self.addr_manager
    }

    /// Report peer behaviours
    pub fn report(&mut self, addr: &Multiaddr, behaviour: Behaviour) -> Result<ReportResult> {
        if let Some(peer_addr) = self.addr_manager.get_mut(addr) {
            let score = peer_addr.score.saturating_add(behaviour.score());
            peer_addr.score = score;
            if score < self.score_config.ban_score {
                self.ban_addr(
                    addr,
                    self.score_config.ban_timeout_ms,
                    format!("report behaviour {:?}", behaviour),
                );
                return Ok(ReportResult::Banned);
            }
        }
        Ok(ReportResult::Ok)
    }

    /// Remove peer id
    pub fn remove_disconnected_peer(&mut self, addr: &Multiaddr) -> Option<PeerInfo> {
        extract_peer_id(addr).and_then(|peer_id| self.peers.borrow_mut().remove(&peer_id))
    }

    /// Get peer status
    pub fn peer_status(&self, peer_id: &PeerId) -> Status {
        if self.peers.borrow().contains_key(peer_id) {
            Status::Connected
        } else {
            Status::Disconnected
        }
    }

    /// Get peers for outbound connection, this method randomly return non-connected peer addrs
    pub fn fetch_addrs_to_attempt(&mut self, count: usize) -> Vec<AddrInfo> {
        let now_ms = faketime::unix_time_as_millis();
        let ban_list = self.ban_list.borrow();
        let peers = self.peers.borrow();
        // get addrs that can attempt.
        self.addr_manager
            .fetch_random(count, |peer_addr: &AddrInfo| {
                !ban_list.is_addr_banned(&peer_addr.addr)
                    && extract_peer_id(&peer_addr.addr)
                        .map(|peer_id| !peers.contains_key(&peer_id))
                        .unwrap_or_default()
                    && !peer_addr.tried_in_last_minute(now_ms)
            })
    }

    /// Get peers for feeler connection, this method randomly return peer addrs that we never
    /// connected to.
    pub fn fetch_addrs_to_feeler(&mut self, count: usize) -> Vec<AddrInfo> {
        let now_ms = faketime::unix_time_as_millis();
        let addr_expired_ms = now_ms - ADDR_TIMEOUT_MS;
        // get expired or never successed addrs.
        let ban_list = self.ban_list.borrow();
        let peers = self.peers.borrow();
        self.addr_manager
            .fetch_random(count, |peer_addr: &AddrInfo| {
                !ban_list.is_addr_banned(&peer_addr.addr)
                    && extract_peer_id(&peer_addr.addr)
                        .map(|peer_id| !peers.contains_key(&peer_id))
                        .unwrap_or_default()
                    && !peer_addr.tried_in_last_minute(now_ms)
                    && !peer_addr.had_connected(addr_expired_ms)
            })
    }

    /// Return valid addrs that success connected, used for discovery.
    pub fn fetch_random_addrs(&mut self, count: usize) -> Vec<AddrInfo> {
        let now_ms = faketime::unix_time_as_millis();
        let addr_expired_ms = now_ms - ADDR_TIMEOUT_MS;
        let ban_list = self.ban_list.borrow();
        let peers = self.peers.borrow();
        // get success connected addrs.
        self.addr_manager
            .fetch_random(count, |peer_addr: &AddrInfo| {
                !ban_list.is_addr_banned(&peer_addr.addr)
                    && (extract_peer_id(&peer_addr.addr)
                        .map(|peer_id| peers.contains_key(&peer_id))
                        .unwrap_or_default()
                        || peer_addr.had_connected(addr_expired_ms))
            })
    }

    /// Ban an addr
    pub(crate) fn ban_addr(&mut self, addr: &Multiaddr, timeout_ms: u64, ban_reason: String) {
        if let Some(addr) = multiaddr_to_socketaddr(addr) {
            let network = ip_to_network(addr.ip());
            self.ban_network(network, timeout_ms, ban_reason)
        }
    }

    pub(crate) fn ban_network(&mut self, network: IpNetwork, timeout_ms: u64, ban_reason: String) {
        let now_ms = faketime::unix_time_as_millis();
        let ban_addr = BannedAddr {
            address: network,
            ban_until: now_ms + timeout_ms,
            created_at: now_ms,
            ban_reason,
        };
        self.mut_ban_list().ban(ban_addr);
    }

    /// Whether the address is banned
    pub fn is_addr_banned(&self, addr: &Multiaddr) -> bool {
        self.ban_list().is_addr_banned(addr)
    }

    /// Get ban list
    pub fn ban_list(&self) -> Ref<BanList> {
        self.ban_list.borrow()
    }

    /// Get mut ban list
    pub fn mut_ban_list(&mut self) -> &mut BanList {
        self.ban_list.get_mut()
    }

    /// Clear ban list
    pub fn clear_ban_list(&self) {
        self.ban_list.replace(Default::default());
    }

    /// Check and try delete addrs if reach limit
    /// return Err if peer_store is full and can't be purge
    fn check_purge(&mut self) -> Result<()> {
        if self.addr_manager.count() < ADDR_COUNT_LIMIT {
            return Ok(());
        }
        // Evicting invalid data in the peer store is a relatively rare operation
        // There are certain cleanup strategies here:
        // 1. Group current data according to network segment
        // 2. Sort according to the amount of data in the same network segment
        // 3. Prioritize cleaning on the same network segment

        let now_ms = faketime::unix_time_as_millis();
        let candidate_peers: Vec<_> = {
            // find candidate peers by network group
            let mut peers_by_network_group: HashMap<Group, Vec<_>> = HashMap::default();
            for addr in self.addr_manager.addrs_iter() {
                peers_by_network_group
                    .entry((&addr.addr).into())
                    .or_default()
                    .push(addr);
            }
            let len = peers_by_network_group.len();
            let mut peers = peers_by_network_group
                .drain()
                .map(|(_, v)| v)
                .collect::<Vec<Vec<_>>>();

            peers.sort_unstable_by_key(|k| std::cmp::Reverse(k.len()));
            let ban_score = self.score_config.ban_score;

            peers
                .into_iter()
                .take(len / 2)
                .flatten()
                .filter_map(move |addr| {
                    if addr.is_terrible(now_ms) || addr.score <= ban_score {
                        Some(addr.addr.clone())
                    } else {
                        None
                    }
                })
                .collect()
        };

        if candidate_peers.is_empty() {
            return Err(PeerStoreError::EvictionFailed.into());
        }

        for key in candidate_peers {
            self.addr_manager.remove(&key);
        }
        Ok(())
    }
}
