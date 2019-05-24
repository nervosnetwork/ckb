//! SqlitePeerStore
//! Principles:
//! 1. PeerId is easy to be generated, should never use a PeerId as an identity.
//! 2. Peer's connected addr should be use as an identify to ban a peer, it is based on our
//!    assumption that IP is a limited resource.

use crate::network_group::MultiaddrExt;
use crate::peer_store::sqlite::{db, DBError};
use crate::peer_store::types::{PeerAddr, PeerInfo};
use crate::peer_store::{
    Behaviour, Multiaddr, PeerId, PeerScoreConfig, PeerStore, ReportResult, Status,
};
use crate::peer_store::{
    ADDR_TIMEOUT_MS, BAN_LIST_CLEAR_EXPIRES_SIZE, DEFAULT_ADDRS, MAX_ADDRS, PEER_STORE_LIMIT,
};
use crate::SessionType;
use faketime::unix_time_as_millis;
use fnv::FnvHashMap;
use rand::{seq::SliceRandom, thread_rng};
use rusqlite::Connection;
use std::time::Duration;

pub struct SqlitePeerStore {
    bootnodes: Vec<(PeerId, Multiaddr)>,
    peer_score_config: PeerScoreConfig,
    ban_list: FnvHashMap<Vec<u8>, u64>,
    pub(crate) conn: Connection,
}

impl SqlitePeerStore {
    pub fn new(conn: Connection, peer_score_config: PeerScoreConfig) -> Self {
        let mut peer_store = SqlitePeerStore {
            bootnodes: Vec::new(),
            ban_list: Default::default(),
            conn,
            peer_score_config,
        };
        peer_store.prepare().expect("prepare tables");
        peer_store
    }

    pub fn file(path: String) -> Result<Self, DBError> {
        let conn = Connection::open(path)?;
        Ok(SqlitePeerStore::new(conn, PeerScoreConfig::default()))
    }

    pub fn memory() -> Result<Self, DBError> {
        let conn = Connection::open_in_memory()?;
        Ok(SqlitePeerStore::new(conn, PeerScoreConfig::default()))
    }

    #[allow(dead_code)]
    pub fn temp() -> Result<Self, DBError> {
        Self::file("".into())
    }

    fn prepare(&mut self) -> Result<(), DBError> {
        self.create_tables()?;
        self.reset_status()?;
        self.load_banlist()
    }

    fn create_tables(&self) -> Result<(), DBError> {
        db::create_tables(&self.conn)
    }

    fn reset_status(&self) -> Result<usize, DBError> {
        db::PeerInfoDB::reset_status(&self.conn)
    }

    fn load_banlist(&mut self) -> Result<(), DBError> {
        self.clear_expires_banned_ip()?;
        let now = unix_time_as_millis();
        let ban_records = db::get_ban_records(&self.conn, now)?;
        for (ip, ban_time) in ban_records {
            self.ban_list.insert(ip, ban_time);
        }
        Ok(())
    }

    fn ban_ip(&mut self, addr: &Multiaddr, timeout: Duration) {
        let ip = {
            match addr.extract_ip_addr_binary() {
                Some(binary) => binary,
                None => return,
            }
        };
        let ban_time = unix_time_as_millis() + (timeout.as_millis() as u64);
        db::insert_ban_record(&self.conn, &ip, ban_time).expect("ban ip");
        self.ban_list.insert(ip, ban_time);
        if self.ban_list.len() > BAN_LIST_CLEAR_EXPIRES_SIZE {
            self.clear_expires_banned_ip().expect("clear ban list");
        }
    }

    fn is_addr_banned(&self, addr: &Multiaddr) -> bool {
        let ip = match addr.extract_ip_addr_binary() {
            Some(ip) => ip,
            None => return false,
        };
        let now = unix_time_as_millis();
        match self.ban_list.get(&ip) {
            Some(ban_time) => *ban_time > now,
            None => false,
        }
    }

    fn clear_expires_banned_ip(&mut self) -> Result<(), DBError> {
        let now = unix_time_as_millis();
        let ips = db::clear_expires_banned_ip(&self.conn, now)?;
        for ip in ips {
            self.ban_list.remove(&ip);
        }
        Ok(())
    }

    /// check and try delete peer_info if peer_infos reach limit
    fn check_store_limit(&mut self) -> Result<(), ()> {
        let peer_info_count = db::PeerInfoDB::count(&self.conn).expect("peer info count");
        if peer_info_count < PEER_STORE_LIMIT {
            return Ok(());
        }
        let candidate_peers = {
            let peers = db::PeerInfoDB::largest_network_group(&self.conn)
                .expect("query largest network group");
            let not_seen_timeout = unix_time_as_millis() - ADDR_TIMEOUT_MS;
            peers
                .into_iter()
                .filter(move |peer| peer.last_connected_at_ms < not_seen_timeout)
        };
        let candidate_peer = match candidate_peers.min_by_key(|peer| peer.score) {
            Some(peer) => peer,
            None => return Err(()),
        };

        if candidate_peer.score >= self.peer_score_config.default_score {
            return Err(());
        }

        let tx = self.conn.transaction().expect("db tx");
        db::PeerInfoDB::delete(&tx, &candidate_peer.peer_id).expect("delete peer error");
        db::PeerAddrDB::delete_by_peer_id(&tx, &candidate_peer.peer_id)
            .expect("delete peer by peer_id error");
        tx.commit().expect("delete peer error");
        Ok(())
    }

    /// check and try delete peer_addr if peer_addr reach limit
    fn check_limit_and_evict_peer_addrs(&mut self, peer_id: &PeerId) {
        let now = unix_time_as_millis();
        let peer_addrs_count = db::PeerAddrDB::count(&self.conn, peer_id).expect("peer info count");
        if peer_addrs_count < MAX_ADDRS {
            return;
        }
        let mut peer_addrs =
            db::PeerAddrDB::get_addrs(&self.conn, peer_id, MAX_ADDRS).expect("peer addrs");
        let mut terrible_addrs_count = 0;
        for paddr in &peer_addrs {
            if paddr.is_terrible(now) {
                db::PeerAddrDB::delete(&self.conn, &paddr.peer_id, &paddr.addr)
                    .expect("delete peer addr");
                terrible_addrs_count += 1;
            }
        }
        // have evict addrs
        if terrible_addrs_count > 0 {
            return;
        }
        // find oldest last_connected_at_ms addr
        peer_addrs.sort_by_key(|paddr| paddr.last_connected_at_ms);
        if let Some(old_peer_addr) = peer_addrs.get(0) {
            db::PeerAddrDB::delete(&self.conn, &old_peer_addr.peer_id, &old_peer_addr.addr)
                .expect("evict peer addr");
        }
    }

    fn fetch_peer_info(&self, peer_id: &PeerId) -> PeerInfo {
        let blank_addr = Multiaddr::empty();
        // Build a default empty peer info record
        let peer = PeerInfo::new(
            peer_id.to_owned(),
            blank_addr,
            self.peer_score_config.default_score,
            SessionType::Inbound,
            0,
        );
        db::PeerInfoDB::get_or_insert(&self.conn, peer)
            .expect("get or insert")
            .expect("must have peer info")
    }

    fn get_peer_info(&self, peer_id: &PeerId) -> Option<PeerInfo> {
        db::PeerInfoDB::get_by_peer_id(&self.conn, peer_id).expect("get peer info")
    }
}

impl PeerStore for SqlitePeerStore {
    fn add_connected_peer(&mut self, peer_id: &PeerId, addr: Multiaddr, session_type: SessionType) {
        if self.check_store_limit().is_err() {
            return;
        }

        let now = unix_time_as_millis();
        let default_peer_score = self.peer_score_config().default_score;
        // upsert peer_info
        db::PeerInfoDB::update(&self.conn, peer_id, &addr, session_type, now)
            .and_then(|affected_lines| {
                if affected_lines > 0 {
                    Ok(())
                } else {
                    db::PeerInfoDB::insert_or_update(
                        &self.conn,
                        &PeerInfo::new(
                            peer_id.to_owned(),
                            addr.clone(),
                            default_peer_score,
                            session_type,
                            now,
                        ),
                    )
                    .map(|_| ())
                }
            })
            .expect("update peer failed");

        // update peer connected_info if outbound
        if session_type.is_outbound() {
            // update peer_addr if addr already exists
            if let Some(mut paddr) =
                db::PeerAddrDB::get(&self.conn, &peer_id, &addr).expect("get peer addr")
            {
                paddr.mark_connected(now);
                db::PeerAddrDB::insert_or_update(&self.conn, &paddr).expect("insert peer addr");
            } else {
                db::PeerAddrDB::insert_or_update(
                    &self.conn,
                    &PeerAddr::new(peer_id.to_owned(), addr, now),
                )
                .expect("insert addr");
            }
        }
    }

    fn add_discovered_addr(&mut self, peer_id: &PeerId, addr: Multiaddr) {
        // check and evict addrs if reach limit
        self.check_limit_and_evict_peer_addrs(peer_id);
        // insert a peer if peer not exists in db
        let peer = self.fetch_peer_info(peer_id);
        db::PeerAddrDB::insert_or_update(
            &self.conn,
            &PeerAddr::new(peer.peer_id.to_owned(), addr, 0),
        )
        .expect("insert addr");
    }

    fn update_peer_addr(&mut self, peer_addr: &PeerAddr) {
        // check and evict addrs if reach limit
        self.check_limit_and_evict_peer_addrs(&peer_addr.peer_id);
        db::PeerAddrDB::insert_or_update(&self.conn, peer_addr).expect("insert addr");
    }

    fn report(&mut self, peer_id: &PeerId, behaviour: Behaviour) -> ReportResult {
        let peer = self.fetch_peer_info(peer_id);
        if self.is_banned(&peer.connected_addr) {
            return ReportResult::Banned;
        }
        let score = peer.score.saturating_add(behaviour.score());
        if score < self.peer_score_config.ban_score {
            self.ban_addr(&peer.connected_addr, self.peer_score_config.ban_timeout);
            return ReportResult::Banned;
        }
        db::PeerInfoDB::update_score(&self.conn, &peer.peer_id, score).expect("update peer score");
        ReportResult::Ok
    }

    fn update_status(&self, peer_id: &PeerId, status: Status) {
        db::PeerInfoDB::update_status(&self.conn, &peer_id, status).expect("update status");
    }

    fn peer_status(&self, peer_id: &PeerId) -> Status {
        self.get_peer_info(peer_id)
            .map(|peer| peer.status)
            .unwrap_or_else(|| Status::Unknown)
    }

    fn add_bootnode(&mut self, peer_id: PeerId, addr: Multiaddr) {
        self.bootnodes.push((peer_id, addr));
    }

    // should return high scored nodes if possible, otherwise, return boostrap nodes
    fn bootnodes(&self, count: u32) -> Vec<(PeerId, Multiaddr)> {
        let mut peers = self
            .peers_to_attempt(count)
            .into_iter()
            .map(|paddr| {
                let PeerAddr { peer_id, addr, .. } = paddr;
                (peer_id, addr)
            })
            .collect::<Vec<_>>();
        if peers.len() < count as usize {
            for (peer_id, addr) in self.bootnodes.iter() {
                let peer = (peer_id.to_owned(), addr.to_owned());
                if !peers.contains(&peer) {
                    peers.push(peer);
                }
            }
        }
        peers
    }

    fn peer_addrs<'a>(&'a self, peer_id: &'a PeerId, count: u32) -> Vec<PeerAddr> {
        db::PeerAddrDB::get_addrs(&self.conn, peer_id, count).expect("get peer addrs")
    }

    fn peers_to_attempt(&self, count: u32) -> Vec<PeerAddr> {
        let now_ms = unix_time_as_millis();
        let peers = db::get_peers_to_attempt(&self.conn, count).expect("get peers to attempt");
        peers
            .into_iter()
            .filter_map(|peer_id| {
                let mut paddrs = db::PeerAddrDB::get_addrs(&self.conn, &peer_id, DEFAULT_ADDRS)
                    .expect("get peer addr");
                let mut rng = thread_rng();
                // randomly find a address to attempt
                paddrs.shuffle(&mut rng);
                paddrs
                    .into_iter()
                    .find(|paddr| !self.is_addr_banned(&paddr.addr) && !paddr.is_terrible(now_ms))
            })
            .collect()
    }

    fn peers_to_feeler(&self, count: u32) -> Vec<PeerAddr> {
        let now_ms = unix_time_as_millis();
        let peers = db::get_peers_to_feeler(&self.conn, count, now_ms - ADDR_TIMEOUT_MS)
            .expect("get peers to feeler");
        peers
            .into_iter()
            .filter_map(|peer_id| {
                let mut paddrs = db::PeerAddrDB::get_addrs(&self.conn, &peer_id, DEFAULT_ADDRS)
                    .expect("get peer addr");
                // find worst addr to feeler unless it is terrible or banned or already tried in one minute
                paddrs.sort_by_key(|paddr| paddr.last_connected_at_ms);
                paddrs.into_iter().find(|paddr| {
                    if paddr.is_terrible(now_ms) {
                        // delete terrible addr
                        db::PeerAddrDB::delete(&self.conn, &paddr.peer_id, &paddr.addr)
                            .expect("delete peer addr");
                        false
                    } else {
                        !self.is_addr_banned(&paddr.addr) && !paddr.tried_in_last_minute(now_ms)
                    }
                })
            })
            .collect()
    }

    fn random_peers(&self, count: u32) -> Vec<PeerAddr> {
        let now_ms = unix_time_as_millis();
        let peers = db::get_random_peers(&self.conn, count, now_ms - ADDR_TIMEOUT_MS)
            .expect("get random peers");
        peers
            .into_iter()
            .filter_map(|peer_id| {
                let mut paddrs = db::PeerAddrDB::get_addrs(&self.conn, &peer_id, DEFAULT_ADDRS)
                    .expect("get peer addr");
                let mut rng = thread_rng();
                // randomly find a address to attempt
                paddrs.shuffle(&mut rng);
                paddrs
                    .into_iter()
                    .find(|paddr| !self.is_addr_banned(&paddr.addr) && !paddr.is_terrible(now_ms))
            })
            .collect()
    }

    fn ban_addr(&mut self, addr: &Multiaddr, timeout: Duration) {
        self.ban_ip(addr, timeout);
    }

    fn is_banned(&self, addr: &Multiaddr) -> bool {
        self.is_addr_banned(addr)
    }

    fn peer_score_config(&self) -> PeerScoreConfig {
        self.peer_score_config
    }
}
