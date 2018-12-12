use crate::PeerId;
use fnv::FnvHashMap;
use libp2p::core::Multiaddr;
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
    Normal,
    Banned,
}

impl ReportResult {
    #[allow(dead_code)]
    pub fn is_banned(&self) -> bool {
        self == &ReportResult::Banned
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
        self.schema.get(&behaviour).map(|s| *s)
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
    // update peer addresses, return numbers of new inserted line
    // return Err if peer not exists
    fn add_discovered_address(&mut self, peer_id: &PeerId, address: Multiaddr) -> Result<(), ()>;
    fn add_discovered_addresses(
        &mut self,
        peer_id: &PeerId,
        addresses: Vec<Multiaddr>,
    ) -> Result<usize, ()>;
    fn report(&mut self, peer_id: &PeerId, behaviour: Behaviour) -> ReportResult;
    fn update_status(&mut self, peer_id: &PeerId, status: Status);
    fn peer_status(&self, peer_id: &PeerId) -> Status;
    fn peer_score(&self, peer_id: &PeerId) -> Option<Score>;
    fn add_bootnode(&mut self, peer_id: PeerId, addr: Multiaddr);
    // should return high scored nodes if possible, otherwise, return boostrap nodes
    fn bootnodes<'a>(&'a self) -> Box<Iterator<Item = (&'a PeerId, &'a Multiaddr)> + 'a>;
    fn peer_addrs<'a>(
        &'a self,
        peer_id: &'a PeerId,
    ) -> Option<Box<Iterator<Item = &'a Multiaddr> + 'a>>;
    fn peers_to_attempt<'a>(&'a self) -> Box<Iterator<Item = (&'a PeerId, &'a Multiaddr)> + 'a>;
    fn ban_peer(&mut self, peer_id: PeerId, timeout: Duration);
    fn is_banned(&self, peer_id: &PeerId) -> bool;
}
