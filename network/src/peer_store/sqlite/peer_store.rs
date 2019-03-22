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
    Behaviour, Multiaddr, PeerId, PeerStore, ReportResult, Score, ScoringSchema, Status,
};
use crate::SessionType;
use faketime::unix_time;
use fnv::FnvHashMap;
use log::debug;
use std::net::IpAddr;
use std::time::Duration;

pub(crate) const PEER_STORE_LIMIT: u32 = 8192;
pub(crate) const PEER_NOT_SEEN_TIMEOUT_SECS: u32 = 14 * 24 * 3600;
const BAN_LIST_CLEAR_EXPIRES_SIZE: usize = 255;
const DEFAULT_POOL_SIZE: u32 = 16;

pub struct SqlitePeerStore {
    bootnodes: Vec<(PeerId, Multiaddr)>,
    schema: ScoringSchema,
    ban_list: FnvHashMap<Vec<u8>, Duration>,
    pub(crate) pool: ConnectionPool,
}

impl SqlitePeerStore {
    pub fn new(connection_pool: ConnectionPool) -> Self {
        let mut peer_store = SqlitePeerStore {
            bootnodes: Vec::new(),
            schema: Default::default(),
            ban_list: Default::default(),
            pool: connection_pool,
        };
        peer_store.prepare().expect("prepare tables");
        peer_store
    }

    pub fn file(path: String) -> Result<Self, DBError> {
        let pool = sqlite::open_pool(sqlite::StorePath::File(path), DEFAULT_POOL_SIZE)?;
        Ok(SqlitePeerStore::new(pool))
    }

    pub fn memory(db: String) -> Result<Self, DBError> {
        let pool = sqlite::open_pool(sqlite::StorePath::Memory(db), DEFAULT_POOL_SIZE)?;
        Ok(SqlitePeerStore::new(pool))
    }

    #[allow(dead_code)]
    pub fn temp() -> Result<Self, DBError> {
        Self::file("".into())
    }

    fn prepare(&mut self) -> Result<(), DBError> {
        self.create_tables()?;
        self.load_banlist()
    }

    fn create_tables(&mut self) -> Result<(), DBError> {
        self.pool.fetch(|conn| db::create_tables(conn))
    }

    fn load_banlist(&mut self) -> Result<(), DBError> {
        self.clear_expires_banned_ip()?;
        let now = unix_time();
        let ban_records = self.pool.fetch(|conn| db::get_ban_records(conn, now))?;
        for (ip, ban_time) in ban_records {
            self.ban_list.insert(ip, ban_time);
        }
        Ok(())
    }

    fn ban_ip(&mut self, addr: &Multiaddr, timeout: Duration) {
        let ip = match addr.extract_ip_addr() {
            Some(IpAddr::V4(ipv4)) => ipv4.octets().to_vec(),
            Some(IpAddr::V6(ipv6)) => ipv6.octets().to_vec(),
            None => return,
        };
        let ban_time = unix_time() + timeout;
        {
            self.pool
                .fetch(|conn| db::insert_ban_record(&conn, &ip, ban_time))
                .expect("ban ip");
        }
        self.ban_list.insert(ip, ban_time);
        if self.ban_list.len() > BAN_LIST_CLEAR_EXPIRES_SIZE {
            self.clear_expires_banned_ip().expect("clear ban list");
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

    fn clear_expires_banned_ip(&mut self) -> Result<(), DBError> {
        let now = unix_time();
        let ips = self
            .pool
            .fetch(|conn| db::clear_expires_banned_ip(conn, now))?;
        for ip in ips {
            self.ban_list.remove(&ip);
        }
        Ok(())
    }

    /// check and try delete peer_info if peer_infos reach limit
    fn check_store_limit(&mut self) -> Result<(), ()> {
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
            let not_seen_timeout =
                unix_time() - Duration::from_secs(PEER_NOT_SEEN_TIMEOUT_SECS.into());
            peers
                .into_iter()
                .filter(move |peer| peer.last_connected_at < not_seen_timeout)
        };
        let candidate_peer = match candidate_peers.min_by_key(|peer| peer.score) {
            Some(peer) => peer,
            None => return Err(()),
        };

        if candidate_peer.score >= self.schema.peer_init_score() {
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

    fn get_and_upsert_peer_info_with(
        &mut self,
        peer_id: &PeerId,
        addr: &Multiaddr,
        endpoint: SessionType,
        last_connected_at: Duration,
    ) -> db::PeerInfo {
        self.pool
            .fetch(|conn| {
                db::PeerInfo::get_by_peer_id(conn, peer_id).and_then(|peer| match peer {
                    Some(mut peer) => {
                        db::PeerInfo::update(conn, peer.id, &addr, endpoint, last_connected_at)
                            .expect("update peer info");
                        peer.connected_addr = addr.to_owned();
                        peer.endpoint = endpoint;
                        peer.last_connected_at = last_connected_at;
                        Ok(Some(peer))
                    }
                    None => {
                        db::PeerInfo::insert(
                            conn,
                            peer_id,
                            &addr,
                            endpoint,
                            self.scoring_schema().peer_init_score(),
                            last_connected_at,
                        )?;
                        db::PeerInfo::get_by_peer_id(conn, &peer_id)
                    }
                })
            })
            .expect("upsert peer info")
            .expect("get peer info")
    }

    fn get_or_insert_peer_info(&mut self, peer_id: &PeerId) -> db::PeerInfo {
        let now = unix_time();
        let addr = &Multiaddr::from_bytes(Vec::new()).expect("null multiaddr");
        self.pool
            .fetch(|conn| {
                db::PeerInfo::get_by_peer_id(conn, peer_id).and_then(|peer| match peer {
                    Some(peer) => Ok(Some(peer)),
                    None => {
                        db::PeerInfo::insert(
                            conn,
                            peer_id,
                            &addr,
                            SessionType::Server,
                            self.scoring_schema().peer_init_score(),
                            now,
                        )?;
                        db::PeerInfo::get_by_peer_id(conn, &peer_id)
                    }
                })
            })
            .expect("get or insert peer info")
            .expect("get peer info")
    }

    fn get_peer_info(&self, peer_id: &PeerId) -> Option<db::PeerInfo> {
        self.pool
            .fetch(|conn| db::PeerInfo::get_by_peer_id(conn, peer_id))
            .expect("get peer info")
    }
}

impl PeerStore for SqlitePeerStore {
    fn add_connected_peer(&mut self, peer_id: &PeerId, addr: Multiaddr, endpoint: SessionType) {
        if self.check_store_limit().is_err() {
            return;
        }
        let now = unix_time();
        // upsert peer_info
        self.get_and_upsert_peer_info_with(peer_id, &addr, endpoint, now);
    }

    fn add_discovered_addr(&mut self, peer_id: &PeerId, addr: Multiaddr) -> bool {
        // peer store is full
        if self.check_store_limit().is_err() {
            return false;
        }
        let id = self.get_or_insert_peer_info(peer_id).id;
        let inserted = self
            .pool
            .fetch(|conn| db::PeerAddr::insert(&conn, id, &addr))
            .expect("insert addr");
        inserted > 0
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
        let peer = self.get_or_insert_peer_info(peer_id);
        let score = peer.score.saturating_add(behaviour_score);
        if score < self.schema.ban_score() {
            let ban_timeout = self.schema.default_ban_timeout();
            self.ban_peer(peer_id, ban_timeout);
            return ReportResult::Banned;
        }
        self.pool
            .fetch(|conn| db::PeerInfo::update_score(&conn, peer.id, score))
            .expect("update peer score");
        ReportResult::Ok
    }

    fn update_status(&mut self, peer_id: &PeerId, status: Status) {
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

    fn add_bootnode(&mut self, peer_id: PeerId, addr: Multiaddr) {
        self.bootnodes.push((peer_id, addr));
    }
    // should return high scored nodes if possible, otherwise, return boostrap nodes
    fn bootnodes(&self, count: u32) -> Vec<(PeerId, Multiaddr)> {
        let mut peers = self
            .pool
            .fetch(|conn| db::get_peers_to_attempt(&conn, count))
            .expect("get peers to attempt");
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
        self.get_peer_info(peer_id).map(|peer| {
            self.pool
                .fetch(|conn| db::PeerAddr::get_addrs(&conn, peer.id, count))
                .expect("get peer addrs")
        })
    }

    fn peers_to_attempt(&self, count: u32) -> Vec<(PeerId, Multiaddr)> {
        self.pool
            .fetch(|conn| db::get_peers_to_attempt(&conn, count))
            .expect("get peers to attempt")
    }

    //TODO Only return connected addresses after network support feeler connection
    fn random_peers(&self, count: u32) -> Vec<(PeerId, Multiaddr)> {
        self.pool
            .fetch(|conn| db::get_random_peers(&conn, count))
            .expect("get random peers")
    }

    fn ban_peer(&mut self, peer_id: &PeerId, timeout: Duration) {
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

    fn scoring_schema(&self) -> &ScoringSchema {
        &self.schema
    }
}
