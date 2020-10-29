use crate::{
    errors::{PeerStoreError, Result},
    network_group::{Group, NetworkGroup},
    peer_store::{
        addr_manager::AddrManager,
        ban_list::BanList,
        types::{ip_to_network, AddrInfo, BannedAddr, MultiaddrExt, PeerInfo},
        Behaviour, Multiaddr, PeerScoreConfig, ReportResult, Status, ADDR_COUNT_LIMIT,
        ADDR_TIMEOUT_MS,
    },
    PeerId, SessionType,
};
use ipnetwork::IpNetwork;
use std::cell::{Ref, RefCell};
use std::collections::{hash_map::Entry, HashMap};

/// TODO(doc): @driftluo
#[derive(Default)]
pub struct PeerStore {
    addr_manager: AddrManager,
    ban_list: RefCell<BanList>,
    peers: RefCell<HashMap<PeerId, PeerInfo>>,
    score_config: PeerScoreConfig,
}

impl PeerStore {
    /// TODO(doc): @driftluo
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
    pub fn add_connected_peer(
        &mut self,
        peer_id: PeerId,
        addr: Multiaddr,
        session_type: SessionType,
    ) -> Result<()> {
        let now_ms = faketime::unix_time_as_millis();
        match self.peers.get_mut().entry(peer_id.to_owned()) {
            Entry::Occupied(mut entry) => {
                let mut peer = entry.get_mut();
                peer.connected_addr = addr.clone();
                peer.last_connected_at_ms = now_ms;
                peer.session_type = session_type;
            }
            Entry::Vacant(entry) => {
                let peer = PeerInfo::new(peer_id.to_owned(), addr.clone(), session_type, now_ms);
                entry.insert(peer);
            }
        }
        let score = self.score_config.default_score;
        if session_type.is_outbound() {
            self.addr_manager.add(AddrInfo::new(
                peer_id,
                addr.extract_ip_addr()?,
                addr.exclude_p2p(),
                now_ms,
                score,
            ));
        }
        Ok(())
    }

    /// Add discovered peer addresses
    /// this method will assume peer and addr is untrust since we have not connected to it.
    pub fn add_addr(&mut self, peer_id: PeerId, addr: Multiaddr) -> Result<()> {
        self.check_purge()?;
        let score = self.score_config.default_score;
        self.addr_manager.add(AddrInfo::new(
            peer_id,
            addr.extract_ip_addr()?,
            addr.exclude_p2p(),
            0,
            score,
        ));
        Ok(())
    }

    /// TODO(doc): @driftluo
    pub fn addr_manager(&self) -> &AddrManager {
        &self.addr_manager
    }

    /// TODO(doc): @driftluo
    pub fn mut_addr_manager(&mut self) -> &mut AddrManager {
        &mut self.addr_manager
    }

    /// Report peer behaviours
    pub fn report(&mut self, peer_id: &PeerId, behaviour: Behaviour) -> Result<ReportResult> {
        if let Some(peer) = {
            let peers = self.peers.borrow();
            peers.get(peer_id).map(ToOwned::to_owned)
        } {
            let key = peer.connected_addr.extract_ip_addr()?;
            let mut peer_addr = self.addr_manager.get_mut(&key).expect("peer addr exists");
            let score = peer_addr.score.saturating_add(behaviour.score());
            peer_addr.score = score;
            if score < self.score_config.ban_score {
                self.ban_addr(
                    &peer.connected_addr,
                    self.score_config.ban_timeout_ms,
                    format!("report behaviour {:?}", behaviour),
                )?;
                return Ok(ReportResult::Banned);
            }
        }
        Ok(ReportResult::Ok)
    }

    /// TODO(doc): @driftluo
    pub fn remove_disconnected_peer(&mut self, peer_id: &PeerId) -> Option<PeerInfo> {
        self.peers.borrow_mut().remove(peer_id)
    }

    /// TODO(doc): @driftluo
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
                    && !peers.contains_key(&peer_addr.peer_id)
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
                    && !peers.contains_key(&peer_addr.peer_id)
                    && !peer_addr.tried_in_last_minute(now_ms)
                    && !peer_addr.had_connected(addr_expired_ms)
            })
    }

    /// return valid addrs that success connected, used for discovery.
    pub fn fetch_random_addrs(&mut self, count: usize) -> Vec<AddrInfo> {
        let now_ms = faketime::unix_time_as_millis();
        let addr_expired_ms = now_ms - ADDR_TIMEOUT_MS;
        let ban_list = self.ban_list.borrow();
        let peers = self.peers.borrow();
        // get success connected addrs.
        self.addr_manager
            .fetch_random(count, |peer_addr: &AddrInfo| {
                !ban_list.is_addr_banned(&peer_addr.addr)
                    && (peers.contains_key(&peer_addr.peer_id)
                        || peer_addr.had_connected(addr_expired_ms))
            })
    }

    /// Ban an addr
    pub(crate) fn ban_addr(
        &mut self,
        addr: &Multiaddr,
        timeout_ms: u64,
        ban_reason: String,
    ) -> Result<()> {
        let network = ip_to_network(addr.extract_ip_addr()?.ip);
        self.ban_network(network, timeout_ms, ban_reason)
    }

    pub(crate) fn ban_network(
        &mut self,
        network: IpNetwork,
        timeout_ms: u64,
        ban_reason: String,
    ) -> Result<()> {
        let now_ms = faketime::unix_time_as_millis();
        let ban_addr = BannedAddr {
            address: network,
            ban_until: now_ms + timeout_ms,
            created_at: now_ms,
            ban_reason,
        };
        self.mut_ban_list().ban(ban_addr);
        Ok(())
    }

    /// TODO(doc): @driftluo
    pub fn is_addr_banned(&self, addr: &Multiaddr) -> bool {
        self.ban_list().is_addr_banned(addr)
    }

    /// TODO(doc): @driftluo
    pub fn ban_list(&self) -> Ref<BanList> {
        self.ban_list.borrow()
    }

    /// TODO(doc): @driftluo
    pub fn mut_ban_list(&mut self) -> &mut BanList {
        self.ban_list.get_mut()
    }

    /// TODO(doc): @driftluo
    pub fn clear_ban_list(&self) {
        self.ban_list.replace(Default::default());
    }

    /// check and try delete addrs if reach limit
    /// return Err if peer_store is full and can't be purge
    fn check_purge(&mut self) -> Result<()> {
        if self.addr_manager.count() < ADDR_COUNT_LIMIT {
            return Ok(());
        }
        let now_ms = faketime::unix_time_as_millis();
        let candidate_peers: Vec<_> = {
            // find candidate peers by network group
            let mut peers_by_network_group: HashMap<Group, Vec<_>> = HashMap::default();
            for addr in self.addr_manager.addrs_iter() {
                let network_group = addr.addr.network_group();
                peers_by_network_group
                    .entry(network_group)
                    .or_default()
                    .push(addr);
            }
            let ban_score = self.score_config.ban_score;
            // find the largest network group
            peers_by_network_group
                .values()
                .max_by_key(|peers| peers.len())
                .expect("largest network group")
                .iter()
                .filter(move |addr| addr.is_terrible(now_ms) || addr.score <= ban_score)
                .map(|addr| addr.ip_port())
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
