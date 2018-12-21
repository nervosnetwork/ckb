mod db;
mod sqlite_peer_store;
pub use crate::peer_store::sqlite_peer_store::{SqlitePeerStore, StorePath};

use crate::PeerId;
use fnv::FnvHashMap;
use libp2p::core::{Endpoint, Multiaddr};
use std::time::Duration;

#[allow(dead_code)]
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum Behaviour {
    FailedToConnect,
    FailedToPing,
    Ping,
    Connect,
    UnexpectedDisconnect,
}
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Status {
    Connected,
    Disconnected,
    Unknown,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ReportResult {
    Ok,
    Banned,
}

impl ReportResult {
    #[allow(dead_code)]
    pub fn is_banned(self) -> bool {
        self == ReportResult::Banned
    }
}

pub type Score = i32;

pub struct ScoringSchema {
    schema: FnvHashMap<Behaviour, Score>,
    peer_init_score: Score,
    ban_score: Score,
    default_ban_timeout: Duration,
}

impl ScoringSchema {
    pub fn new_default() -> Self {
        let schema = [
            (Behaviour::FailedToConnect, -20),
            (Behaviour::FailedToPing, -10),
            (Behaviour::Ping, 5),
            (Behaviour::Connect, 10),
            (Behaviour::UnexpectedDisconnect, -20),
        ]
        .iter()
        .cloned()
        .collect();
        ScoringSchema {
            schema,
            peer_init_score: 100,
            ban_score: 40,
            default_ban_timeout: Duration::from_secs(24 * 3600),
        }
    }

    pub fn peer_init_score(&self) -> Score {
        self.peer_init_score
    }

    pub fn ban_score(&self) -> Score {
        self.ban_score
    }

    pub fn get_score(&self, behaviour: Behaviour) -> Option<Score> {
        self.schema.get(&behaviour).cloned()
    }

    pub fn default_ban_timeout(&self) -> Duration {
        self.default_ban_timeout
    }
}

impl Default for ScoringSchema {
    fn default() -> Self {
        ScoringSchema::new_default()
    }
}

pub trait PeerStore: Send + Sync {
    // initial or update peer_info in peer_store
    fn new_connected_peer(&mut self, peer_id: &PeerId, address: Multiaddr, endpoint: Endpoint);
    // add peer discovered addresses, return numbers of new inserted line, return Err if peer not exists
    fn add_discovered_address(&mut self, peer_id: &PeerId, address: Multiaddr) -> Result<(), ()>;
    fn add_discovered_addresses(
        &mut self,
        peer_id: &PeerId,
        address: Vec<Multiaddr>,
    ) -> Result<usize, ()>;
    fn report(&mut self, peer_id: &PeerId, behaviour: Behaviour) -> ReportResult;
    fn update_status(&mut self, peer_id: &PeerId, status: Status);
    fn peer_status(&self, peer_id: &PeerId) -> Status;
    fn peer_score(&self, peer_id: &PeerId) -> Option<Score>;
    fn add_bootnode(&mut self, peer_id: PeerId, addr: Multiaddr);
    // should return high scored nodes if possible, otherwise, return boostrap nodes
    fn bootnodes(&self, count: usize) -> Vec<(PeerId, Multiaddr)>;
    fn peer_addrs(&self, peer_id: &PeerId, count: usize) -> Option<Vec<Multiaddr>>;
    fn peers_to_attempt(&self, count: usize) -> Vec<(PeerId, Multiaddr)>;
    fn ban_peer(&mut self, peer_id: &PeerId, timeout: Duration);
    fn is_banned(&self, peer_id: &PeerId) -> bool;
    fn scoring_schema(&self) -> &ScoringSchema;
    fn peer_score_or_default(&self, peer_id: &PeerId) -> Score {
        self.peer_score(peer_id)
            .unwrap_or_else(|| self.scoring_schema().peer_init_score())
    }
}
