use super::{Behaviour, Multiaddr, PeerId, PeerStore, ReportResult, Score, ScoringSchema, Status};
use crate::network_group::MultiaddrExt;
use crate::peer_store::db;
use crate::peer_store::sqlite::{self, ConnectionPool, PooledConnection as Connection, StorePath};
use faketime::unix_time;
use fnv::FnvHashMap;
use libp2p::core::Endpoint;
use log::debug;
use std::net::IpAddr;
use std::time::Duration;

pub(crate) const PEER_STORE_LIMIT: u32 = 8192;
pub(crate) const PEER_NOT_SEEN_TIMEOUT_SECS: u32 = 14 * 24 * 3600;
const BAN_LIST_CLEAR_EXPIRES_SIZE: usize = 255;
const DEFAULT_POOL_SIZE: u32 = 16;

// Scoring and ban:
// Because peer_id is easy to forge, we should consider to identify a peer by it's connected_addr
// instead of peer_id
// Howerver connected_addr maybe same for multiple inbound peers, these peers may in the same sub network or our node may behind a reverse proxy, so we can't just reject them.
// A solution is to identify and score a peer by it's peer_id, but ban a peer through it connected_addr, it's
// mean when a peer got banned, we're no longer accept new peers from the same connected_addr.

pub struct SqlitePeerStore {
    bootnodes: Vec<(PeerId, Multiaddr)>,
    schema: ScoringSchema,
    ban_list: FnvHashMap<Vec<u8>, Duration>,
    pool: ConnectionPool,
}

impl Default for SqlitePeerStore {
    fn default() -> Self {
        SqlitePeerStore::new(StorePath::Memory, DEFAULT_POOL_SIZE)
    }
}

impl SqlitePeerStore {
    pub fn new(store_path: StorePath, max_connection: u32) -> Self {
        let pool = sqlite::open_pool(store_path, max_connection);
        let mut peer_store = SqlitePeerStore {
            bootnodes: Vec::new(),
            schema: Default::default(),
            ban_list: Default::default(),
            pool,
        };
        peer_store.prepare();
        peer_store
    }

    pub(crate) fn connection(&self) -> Connection {
        self.pool.get().expect("fetch connection")
    }

    fn prepare(&mut self) {
        self.create_tables();
        self.load_banlist();
    }

    fn create_tables(&mut self) {
        let conn = self.connection();
        db::create_tables(&conn);
    }

    fn load_banlist(&mut self) {
        self.clear_expires_banned_ip();
        let now = unix_time();
        let conn = self.connection();
        for (ip, ban_time) in db::get_ban_records(&conn, now) {
            self.ban_list.insert(ip, ban_time);
        }
    }

    fn ban_ip(&mut self, addr: &Multiaddr, timeout: Duration) {
        let ip = match addr.extract_ip_addr() {
            Some(IpAddr::V4(ipv4)) => ipv4.octets().to_vec(),
            Some(IpAddr::V6(ipv6)) => ipv6.octets().to_vec(),
            None => return,
        };
        let ban_time = unix_time() + timeout;
        {
            let conn = self.connection();
            db::insert_ban_record(&conn, &ip, ban_time);
        }
        self.ban_list.insert(ip, ban_time);
        if self.ban_list.len() > BAN_LIST_CLEAR_EXPIRES_SIZE {
            self.clear_expires_banned_ip();
        }
    }

    fn is_addr_banned(&self, addr: &Multiaddr) -> bool {
        let ip = match addr.extract_ip_addr() {
            Some(IpAddr::V4(ipv4)) => ipv4.octets().to_vec(),
            Some(IpAddr::V6(ipv6)) => ipv6.octets().to_vec(),
            None => return false,
        };
        let now = unix_time();
        match self.ban_list.get(&ip) {
            Some(ban_time) => *ban_time > now,
            None => false,
        }
    }

    fn clear_expires_banned_ip(&mut self) {
        let now = unix_time();
        let conn = self.connection();
        let ips = db::clear_expires_banned_ip(&conn, now);
        for ip in ips {
            self.ban_list.remove(&ip);
        }
    }

    // check and try to delete peer_info if peer_infos reach limit
    fn check_and_allow_new_record(&mut self) -> bool {
        let mut conn = self.connection();
        if db::PeerInfo::count(&conn) < PEER_STORE_LIMIT {
            return true;
        }
        let peers = db::PeerInfo::largest_network_group(&conn);
        let not_seen_timeout = unix_time() - Duration::from_secs(PEER_NOT_SEEN_TIMEOUT_SECS.into());
        let recently_not_touched_peers = peers
            .iter()
            .filter(|peer| peer.connected_time < not_seen_timeout);
        let candidate_peer = match recently_not_touched_peers.min_by_key(|peer| peer.score) {
            Some(peer) => peer,
            None => return false,
        };

        if candidate_peer.score < self.schema.peer_init_score() {
            let tx = conn.transaction().expect("db tx");
            db::PeerInfo::delete(&tx, candidate_peer.id);
            db::PeerAddr::delete_by_peer_id(&tx, candidate_peer.id);
            tx.commit().expect("commit");
            true
        } else {
            false
        }
    }

    fn ensure_peer(
        &mut self,
        refresh_exist: bool,
        peer_id: &PeerId,
        addr: &Multiaddr,
        endpoint: Endpoint,
        connected_time: Duration,
    ) {
        let conn = self.connection();
        match db::PeerInfo::get_by_peer_id(&conn, peer_id) {
            Some(peer) => {
                if refresh_exist {
                    db::PeerInfo::update(&conn, peer.id, &addr, endpoint, connected_time);
                }
            }
            None => db::PeerInfo::insert(
                &conn,
                peer_id,
                &addr,
                endpoint,
                self.scoring_schema().peer_init_score(),
                connected_time,
            ),
        }
    }

    fn ensure_peer_with_default(&mut self, peer_id: &PeerId) {
        let now = unix_time();
        let addr = &Multiaddr::from_bytes(Vec::new()).expect("null multiaddr");
        self.ensure_peer(false, peer_id, addr, Endpoint::Listener, now);
    }
}

impl PeerStore for SqlitePeerStore {
    fn new_connected_peer(&mut self, peer_id: &PeerId, addr: Multiaddr, endpoint: Endpoint) {
        if !self.check_and_allow_new_record() {
            return;
        }
        let now = unix_time();
        // upsert peer_info
        self.ensure_peer(true, peer_id, &addr, endpoint, now);
    }

    fn add_discovered_address(&mut self, peer_id: &PeerId, addr: Multiaddr) -> Result<(), ()> {
        if !self.check_and_allow_new_record() {
            return Err(());
        }
        self.ensure_peer_with_default(peer_id);
        let conn = self.connection();
        let peer_info_id = db::PeerInfo::get_by_peer_id(&conn, peer_id)
            .expect("query after insert")
            .id;
        if db::PeerAddr::insert(&conn, peer_info_id, &addr) > 0 {
            return Ok(());
        }
        Err(())
    }

    fn add_discovered_addresses(
        &mut self,
        peer_id: &PeerId,
        addrs: Vec<Multiaddr>,
    ) -> Result<usize, ()> {
        if !self.check_and_allow_new_record() {
            return Err(());
        }
        self.ensure_peer_with_default(peer_id);
        let conn = self.connection();
        let peer_info_id = db::PeerInfo::get_by_peer_id(&conn, peer_id)
            .expect("query after insert")
            .id;
        let mut count = 0;
        for addr in addrs {
            count += db::PeerAddr::insert(&conn, peer_info_id, &addr);
        }
        Ok(count)
    }

    fn report(&mut self, peer_id: &PeerId, behaviour: Behaviour) -> ReportResult {
        if self.is_banned(peer_id) {
            return ReportResult::Banned;
        }
        let behaviour_score = match self.schema.get_score(behaviour) {
            Some(score) => score,
            None => {
                debug!(target: "network", "behaviour {:?} is undefined", behaviour);
                return ReportResult::Ok;
            }
        };
        self.ensure_peer_with_default(peer_id);
        let conn = self.connection();
        let peer = match db::PeerInfo::get_by_peer_id(&conn, peer_id) {
            Some(peer) => peer,
            None => return ReportResult::Banned,
        };
        let now = unix_time();
        let score = peer.score.saturating_add(behaviour_score);
        if score < self.schema.ban_score() {
            let ban_time = self.schema.default_ban_timeout() + now;
            drop(conn);
            self.ban_peer(peer_id, ban_time);
            return ReportResult::Banned;
        }
        db::PeerInfo::update_score(&conn, peer.id, score);
        ReportResult::Ok
    }

    fn update_status(&mut self, peer_id: &PeerId, status: Status) {
        let conn = self.connection();
        if let Some(peer) = db::PeerInfo::get_by_peer_id(&conn, peer_id) {
            db::PeerInfo::update_status(&conn, peer.id, status);
        }
    }

    fn peer_status(&self, peer_id: &PeerId) -> Status {
        let conn = self.connection();
        db::PeerInfo::get_by_peer_id(&conn, peer_id)
            .map(|peer| peer.status)
            .unwrap_or_else(|| Status::Unknown)
    }

    fn peer_score(&self, peer_id: &PeerId) -> Option<Score> {
        let conn = self.connection();
        let peer_info = db::PeerInfo::get_by_peer_id(&conn, peer_id);
        peer_info.map(|peer| peer.score)
    }

    fn add_bootnode(&mut self, peer_id: PeerId, addr: Multiaddr) {
        self.new_connected_peer(&peer_id, addr.clone(), Endpoint::Dialer);
        self.bootnodes.push((peer_id, addr));
    }
    // should return high scored nodes if possible, otherwise, return boostrap nodes
    fn bootnodes(&self, count: u32) -> Vec<(PeerId, Multiaddr)> {
        let conn = self.connection();
        let mut peers = db::get_peers_to_attempt(&conn, count);
        if peers.len() < count as usize {
            for (peer_id, addr) in &self.bootnodes {
                let peer = (peer_id.to_owned(), addr.to_owned());
                if !peers.contains(&peer) {
                    peers.push(peer);
                }
            }
        }
        peers
    }
    fn peer_addrs<'a>(&'a self, peer_id: &'a PeerId, count: u32) -> Option<Vec<Multiaddr>> {
        let conn = self.connection();
        db::PeerInfo::get_by_peer_id(&conn, peer_id)
            .map(|peer| db::PeerAddr::get_addrs(&conn, peer.id, count))
    }

    fn peers_to_attempt(&self, count: u32) -> Vec<(PeerId, Multiaddr)> {
        let conn = self.connection();
        db::get_peers_to_attempt(&conn, count)
    }

    fn ban_peer(&mut self, peer_id: &PeerId, timeout: Duration) {
        let conn = self.connection();
        if let Some(peer) = db::PeerInfo::get_by_peer_id(&conn, &peer_id) {
            drop(conn);
            self.ban_ip(&peer.connected_addr, timeout);
        }
    }

    fn is_banned(&self, peer_id: &PeerId) -> bool {
        let conn = self.connection();
        if let Some(peer) = db::PeerInfo::get_by_peer_id(&conn, &peer_id) {
            drop(conn);
            return self.is_addr_banned(&peer.connected_addr);
        }
        false
    }

    fn scoring_schema(&self) -> &ScoringSchema {
        &self.schema
    }
}
