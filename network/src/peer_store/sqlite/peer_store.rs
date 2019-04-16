use crate::network_group::MultiaddrExt;
use crate::peer_store::sqlite::{self, db, ConnectionPool, ConnectionPoolExt, DBError};
/// SqlitePeerStore
/// Principles:
/// 1. PeerId is easy to be generated, should never use a PeerId as an identity.
/// 2. Peer's connected addr should be use as an identify to ban a peer, it is based on our
///    assumption that IP is a limited resource.
/// Solution:
/// 1. Through PeerId to ban or score a peer.
/// 2. When a peer get banned we also ban peer's connected addr.
/// 3. A bad peer can always avoid punishment by change it's PeerId, but it can't get high
///    score.
/// 4. Good peers can get higher score than bad peers.
use crate::peer_store::{
    Behaviour, Multiaddr, PeerId, PeerScoreConfig, PeerStore, ReportResult, Score, Status,
};
use crate::SessionType;
use ckb_util::RwLock;
use faketime::unix_time;
use fnv::FnvHashMap;
use std::time::Duration;

/// After this limitation, peer store will try to eviction peers
pub(crate) const PEER_STORE_LIMIT: u32 = 8192;
/// Consider we never seen a peer if peer's last_connected_at beyond this timeout
pub(crate) const LAST_CONNECTED_TIMEOUT_SECS: u64 = 14 * 24 * 3600;
/// Clear banned list if the list reach this size
const BAN_LIST_CLEAR_EXPIRES_SIZE: usize = 1024;
/// SQLITE connection pool size
const DEFAULT_POOL_SIZE: u32 = 1;
const DEFAULT_ADDRS: u32 = 3;

pub struct SqlitePeerStore {
    bootnodes: RwLock<Vec<(PeerId, Multiaddr)>>,
    peer_score_config: PeerScoreConfig,
    ban_list: RwLock<FnvHashMap<Vec<u8>, Duration>>,
    pub(crate) pool: ConnectionPool,
}

impl SqlitePeerStore {
    pub fn new(connection_pool: ConnectionPool, peer_score_config: PeerScoreConfig) -> Self {
        let peer_store = SqlitePeerStore {
            bootnodes: RwLock::new(Vec::new()),
            ban_list: RwLock::new(Default::default()),
            pool: connection_pool,
            peer_score_config,
        };
        peer_store.prepare().expect("prepare tables");
        peer_store
    }

    pub fn file(path: String) -> Result<Self, DBError> {
        let pool = sqlite::open_pool(sqlite::StorePath::File(path), DEFAULT_POOL_SIZE)?;
        Ok(SqlitePeerStore::new(pool, PeerScoreConfig::default()))
    }

    pub fn memory(db: String) -> Result<Self, DBError> {
        let pool = sqlite::open_pool(sqlite::StorePath::Memory(db), DEFAULT_POOL_SIZE)?;
        Ok(SqlitePeerStore::new(pool, PeerScoreConfig::default()))
    }

    #[allow(dead_code)]
    pub fn temp() -> Result<Self, DBError> {
        Self::file("".into())
    }

    fn prepare(&self) -> Result<(), DBError> {
        self.create_tables()?;
        self.reset_status()?;
        self.load_banlist()
    }

    fn create_tables(&self) -> Result<(), DBError> {
        self.pool.fetch(|conn| db::create_tables(conn))
    }

    fn reset_status(&self) -> Result<usize, DBError> {
        self.pool.fetch(|conn| db::PeerInfo::reset_status(conn))
    }

    fn load_banlist(&self) -> Result<(), DBError> {
        self.clear_expires_banned_ip()?;
        let now = unix_time();
        let ban_records = self.pool.fetch(|conn| db::get_ban_records(conn, now))?;
        let mut guard = self.ban_list.write();
        for (ip, ban_time) in ban_records {
            guard.insert(ip, ban_time);
        }
        Ok(())
    }

    fn ban_ip(&self, addr: &Multiaddr, timeout: Duration) {
        let ip = {
            match addr.extract_ip_addr_binary() {
                Some(binary) => binary,
                None => return,
            }
        };
        let ban_time = unix_time() + timeout;
        {
            self.pool
                .fetch(|conn| db::insert_ban_record(&conn, &ip, ban_time))
                .expect("ban ip");
        }
        let mut guard = self.ban_list.write();
        guard.insert(ip, ban_time);
        if guard.len() > BAN_LIST_CLEAR_EXPIRES_SIZE {
            self.clear_expires_banned_ip().expect("clear ban list");
        }
    }

    fn is_addr_banned(&self, addr: &Multiaddr) -> bool {
        let ip = match addr.extract_ip_addr_binary() {
            Some(ip) => ip,
            None => return false,
        };
        let now = unix_time();
        match self.ban_list.read().get(&ip) {
            Some(ban_time) => *ban_time > now,
            None => false,
        }
    }

    fn clear_expires_banned_ip(&self) -> Result<(), DBError> {
        let now = unix_time();
        let ips = self
            .pool
            .fetch(|conn| db::clear_expires_banned_ip(conn, now))?;
        let mut guard = self.ban_list.write();
        for ip in ips {
            guard.remove(&ip);
        }
        Ok(())
    }

    /// check and try delete peer_info if peer_infos reach limit
    fn check_store_limit(&self) -> Result<(), ()> {
        let peer_info_count = self
            .pool
            .fetch(|conn| db::PeerInfo::count(conn))
            .expect("peer info count");
        if peer_info_count < PEER_STORE_LIMIT {
            return Ok(());
        }
        let candidate_peers = {
            let peers = self
                .pool
                .fetch(|conn| db::PeerInfo::largest_network_group(conn))
                .expect("query largest network group");
            let not_seen_timeout = unix_time() - Duration::from_secs(LAST_CONNECTED_TIMEOUT_SECS);
            peers
                .into_iter()
                .filter(move |peer| peer.last_connected_at < not_seen_timeout)
        };
        let candidate_peer = match candidate_peers.min_by_key(|peer| peer.score) {
            Some(peer) => peer,
            None => return Err(()),
        };

        if candidate_peer.score >= self.peer_score_config.default_score {
            return Err(());
        }
        self.pool
            .fetch(|conn| {
                let tx = conn.transaction().expect("db tx");
                db::PeerInfo::delete(&tx, candidate_peer.id)?;
                db::PeerAddr::delete_by_peer_id(&tx, candidate_peer.id)?;
                tx.commit().map_err(Into::into)
            })
            .expect("delete peer");
        Ok(())
    }

    fn fetch_peer_info(&self, peer_id: &PeerId) -> db::PeerInfo {
        let blank_addr = &Multiaddr::from_bytes(Vec::new()).expect("null multiaddr");
        self.pool
            .fetch(|conn| {
                db::PeerInfo::get_or_insert(
                    conn,
                    peer_id,
                    &blank_addr,
                    SessionType::Inbound,
                    self.peer_score_config.default_score,
                    Duration::from_secs(0),
                )
            })
            .expect("get or insert")
            .expect("must have peer info")
    }

    fn get_peer_info(&self, peer_id: &PeerId) -> Option<db::PeerInfo> {
        self.pool
            .fetch(|conn| db::PeerInfo::get_by_peer_id(conn, peer_id))
            .expect("get peer info")
    }

    fn find_addrs_for_peers(
        &self,
        conn: &rusqlite::Connection,
        peers: Vec<(u32, PeerId)>,
    ) -> Result<Vec<(PeerId, Multiaddr)>, DBError> {
        let mut peer_addrs = Vec::with_capacity(peers.len());
        for (id, peer_id) in peers {
            let addrs = db::PeerAddr::get_addrs(conn, id, DEFAULT_ADDRS)?;
            if let Some(addr) = addrs.into_iter().find(|addr| !self.is_addr_banned(&addr)) {
                peer_addrs.push((peer_id, addr));
            }
        }
        Ok(peer_addrs)
    }
}

impl PeerStore for SqlitePeerStore {
    fn add_connected_peer(&self, peer_id: &PeerId, addr: Multiaddr, endpoint: SessionType) {
        if self.check_store_limit().is_err() {
            return;
        }
        let now = unix_time();
        let default_peer_score = self.peer_score_config().default_score;
        // upsert peer_info
        self.pool
            .fetch(move |conn| {
                db::PeerInfo::update(conn, peer_id, &addr, endpoint, now).and_then(
                    |affected_lines| {
                        if affected_lines > 0 {
                            Ok(())
                        } else {
                            db::PeerInfo::insert(
                                conn,
                                peer_id,
                                &addr,
                                endpoint,
                                default_peer_score,
                                now,
                            )
                            .map(|_| ())
                        }
                    },
                )?;

                if endpoint.is_outbound() {
                    let peer = db::PeerInfo::get_by_peer_id(conn, peer_id)?.expect("must have");
                    db::PeerAddr::update_connected_at(conn, peer.id, addr, now)?;
                }
                Ok(())
            })
            .expect("upsert peer info");
    }

    fn add_discovered_addr(&self, peer_id: &PeerId, addr: Multiaddr) -> bool {
        // peer store is full
        if self.check_store_limit().is_err() {
            return false;
        }
        let peer_info = self.fetch_peer_info(peer_id);
        let inserted = self
            .pool
            .fetch(|conn| db::PeerAddr::insert(&conn, peer_info.id, &addr, Duration::from_secs(0)))
            .expect("insert addr");
        inserted > 0
    }

    fn report(&self, peer_id: &PeerId, behaviour: Behaviour) -> ReportResult {
        if self.is_banned(peer_id) {
            return ReportResult::Banned;
        }
        let peer = self.fetch_peer_info(peer_id);
        let score = peer.score.saturating_add(behaviour.score());
        if score < self.peer_score_config.ban_score {
            self.ban_peer(peer_id, self.peer_score_config.ban_timeout);
            return ReportResult::Banned;
        }
        self.pool
            .fetch(|conn| db::PeerInfo::update_score(&conn, peer.id, score))
            .expect("update peer score");
        ReportResult::Ok
    }

    fn update_status(&self, peer_id: &PeerId, status: Status) {
        if let Some(peer) = self.get_peer_info(peer_id) {
            self.pool
                .fetch(|conn| db::PeerInfo::update_status(&conn, peer.id, status))
                .expect("update status");
        }
    }

    fn peer_status(&self, peer_id: &PeerId) -> Status {
        self.get_peer_info(peer_id)
            .map(|peer| peer.status)
            .unwrap_or_else(|| Status::Unknown)
    }

    fn peer_score(&self, peer_id: &PeerId) -> Option<Score> {
        self.get_peer_info(peer_id).map(|peer| peer.score)
    }

    fn add_bootnode(&self, peer_id: PeerId, addr: Multiaddr) {
        self.bootnodes.write().push((peer_id, addr));
    }
    // should return high scored nodes if possible, otherwise, return boostrap nodes
    fn bootnodes(&self, count: u32) -> Vec<(PeerId, Multiaddr)> {
        let mut peers = self.peers_to_attempt(count);
        if peers.len() < count as usize {
            for (peer_id, addr) in self.bootnodes.read().iter() {
                let peer = (peer_id.to_owned(), addr.to_owned());
                if !peers.contains(&peer) {
                    peers.push(peer);
                }
            }
        }
        peers
    }
    fn peer_addrs<'a>(&'a self, peer_id: &'a PeerId, count: u32) -> Option<Vec<Multiaddr>> {
        self.get_peer_info(peer_id).map(|peer| {
            self.pool
                .fetch(|conn| db::PeerAddr::get_addrs(&conn, peer.id, count))
                .expect("get peer addrs")
        })
    }

    fn peers_to_attempt(&self, count: u32) -> Vec<(PeerId, Multiaddr)> {
        self.pool
            .fetch(|conn| {
                let peers = db::get_peers_to_attempt(&conn, count)?;
                self.find_addrs_for_peers(&conn, peers)
            })
            .expect("get peers to attempt")
    }

    fn peers_to_feeler(&self, count: u32) -> Vec<(PeerId, Multiaddr)> {
        self.pool
            .fetch(|conn| {
                let peers = db::get_peers_to_feeler(
                    &conn,
                    count,
                    unix_time() - Duration::from_secs(LAST_CONNECTED_TIMEOUT_SECS),
                )?;
                self.find_addrs_for_peers(&conn, peers)
            })
            .expect("get peers to attempt")
    }

    fn random_peers(&self, count: u32) -> Vec<(PeerId, Multiaddr)> {
        self.pool
            .fetch(|conn| {
                db::get_random_peers(
                    &conn,
                    count,
                    unix_time() - Duration::from_secs(LAST_CONNECTED_TIMEOUT_SECS),
                )
            })
            .expect("get random peers")
    }

    fn ban_peer(&self, peer_id: &PeerId, timeout: Duration) {
        if let Some(peer) = self.get_peer_info(peer_id) {
            self.ban_ip(&peer.connected_addr, timeout);
        }
    }

    fn is_banned(&self, peer_id: &PeerId) -> bool {
        if let Some(peer) = self.get_peer_info(peer_id) {
            return self.is_addr_banned(&peer.connected_addr);
        }
        false
    }
    fn peer_score_config(&self) -> PeerScoreConfig {
        self.peer_score_config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiaddr::ToMultiaddr;
    use rayon::prelude::*;
    use tempfile::NamedTempFile;

    #[test]
    fn concurrent_write() {
        let file = NamedTempFile::new().unwrap();
        let peer_store = SqlitePeerStore::file(file.path().to_str().unwrap().to_string()).unwrap();
        peer_store.prepare().unwrap();
        let addr = "/ip4/127.0.0.1".to_multiaddr().unwrap();
        (0..100u64)
            .into_par_iter()
            .map(|_| {
                let peer_id = PeerId::random();
                peer_store.add_connected_peer(&peer_id, addr.clone(), SessionType::Outbound);
                let _ = peer_store.add_discovered_addr(&peer_id, addr.clone());
            })
            .collect::<Vec<_>>();
    }
}
