use crate::peer_store::{Behaviour, PeerStore, ReportResult, Score, ScoringSchema, Status};
use crate::PeerId;
use fnv::FnvHashMap;
use libp2p::core::Multiaddr;
use log::{debug, trace};
use std::time::{Duration, Instant};

#[derive(Debug)]
struct PeerInfo {
    addresses: Vec<Multiaddr>,
    last_updated_at: Instant,
    score: Score,
    status: Status,
}

// NOTICE MemoryPeerStore is used for test environment only!!!
pub struct MemoryPeerStore {
    bootnodes: Vec<(PeerId, Multiaddr)>,
    peers: FnvHashMap<PeerId, PeerInfo>,
    ban_list: FnvHashMap<PeerId, Instant>,
    schema: ScoringSchema,
}

impl MemoryPeerStore {
    pub fn new(scoring_schema: ScoringSchema) -> Self {
        MemoryPeerStore {
            bootnodes: Default::default(),
            peers: Default::default(),
            ban_list: Default::default(),
            schema: scoring_schema,
        }
    }

    fn add_peer(&mut self, peer_id: PeerId, addresses: Vec<Multiaddr>) -> bool {
        if self.peers.get(&peer_id).is_some() {
            return false;
        }
        let now = Instant::now();
        let peer = PeerInfo {
            addresses,
            last_updated_at: now,
            score: self.schema.peer_init_score(),
            status: Status::Unknown,
        };
        self.peers.insert(peer_id, peer);
        true
    }
}

impl PeerStore for MemoryPeerStore {
    fn new_connected_peer(&mut self, peer_id: &PeerId, address: Multiaddr) {
        self.add_discovered_address(peer_id, address).unwrap();
    }

    fn scoring_schema(&self) -> &ScoringSchema {
        &self.schema
    }

    fn add_discovered_address(&mut self, peer_id: &PeerId, address: Multiaddr) -> Result<(), ()> {
        self.add_discovered_addresses(peer_id, vec![address])
            .map(|_| ())
    }

    fn add_discovered_addresses(
        &mut self,
        peer_id: &PeerId,
        addresses: Vec<Multiaddr>,
    ) -> Result<usize, ()> {
        if let Some(peer) = self.peers.get_mut(&peer_id) {
            let now = Instant::now();
            let origin_addrs_len = peer.addresses.len();
            for addr in addresses {
                if !peer.addresses.contains(&addr) {
                    peer.addresses.push(addr);
                }
            }
            peer.last_updated_at = now;
            return Ok(peer.addresses.len() - origin_addrs_len);
        }
        let len = addresses.len();
        self.add_peer(peer_id.to_owned(), addresses);
        Ok(len)
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
        // apply reported score
        let score = match self.peers.get_mut(peer_id) {
            Some(peer) => {
                peer.score = peer.score.saturating_add(behaviour_score);
                peer.score
            }
            None => return ReportResult::Ok,
        };
        // ban peer is score is lower than ban_score
        if score < self.schema.ban_score() {
            let default_ban_timeout = self.schema.default_ban_timeout();
            self.ban_peer(peer_id.to_owned(), default_ban_timeout);
            return ReportResult::Banned;
        }
        ReportResult::Ok
    }

    fn update_status(&mut self, peer_id: &PeerId, status: Status) {
        if let Some(peer) = self.peers.get_mut(&peer_id) {
            let now = Instant::now();
            peer.last_updated_at = now;
            peer.status = status;
        }
    }

    fn peer_status(&self, peer_id: &PeerId) -> Status {
        match self.peers.get(&peer_id) {
            Some(peer) => peer.status,
            None => Status::Unknown,
        }
    }

    fn peer_score(&self, peer_id: &PeerId) -> Option<Score> {
        self.peers.get(peer_id).map(|peer| peer.score)
    }

    fn add_bootnode(&mut self, peer_id: PeerId, addr: Multiaddr) {
        self.bootnodes.push((peer_id.clone(), addr.clone()));
        self.add_peer(peer_id, vec![addr]);
    }

    fn bootnodes<'a>(&'a self) -> Box<dyn Iterator<Item = (&'a PeerId, &'a Multiaddr)> + 'a> {
        let mut bootnodes = self
            .peers_to_attempt()
            .chain(self.bootnodes.iter().map(|(peer_id, addr)| (peer_id, addr)))
            .collect::<Vec<_>>();
        bootnodes.dedup();
        let iter = bootnodes.into_iter();
        Box::new(iter) as Box<_>
    }

    fn peer_addrs<'a>(
        &'a self,
        peer_id: &'a PeerId,
    ) -> Option<Box<dyn Iterator<Item = &'a Multiaddr> + 'a>> {
        let iter = match self.peers.get(peer_id) {
            Some(peer) => peer.addresses.iter(),
            None => return None,
        };
        Some(Box::new(iter) as Box<_>)
    }

    fn peers_to_attempt<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = (&'a PeerId, &'a Multiaddr)> + 'a> {
        trace!(
        target: "network",
        "try fetch attempt peers from {:?}",
        self.peers.iter().collect::<Vec<_>>()
        );
        let peers = self.peers.iter().filter_map(move |(peer_id, peer_info)| {
            if peer_info.status == Status::Connected || peer_info.addresses.is_empty() {
                None
            } else {
                Some((peer_id, &peer_info.addresses[0]))
            }
        });
        Box::new(peers) as Box<_>
    }

    fn ban_peer(&mut self, peer_id: PeerId, timeout: Duration) {
        let now = Instant::now();
        let timeout_at = now + timeout;
        self.ban_list.insert(peer_id, timeout_at);
    }

    fn is_banned(&self, peer_id: &PeerId) -> bool {
        if let Some(timeout_at) = self.ban_list.get(peer_id) {
            return *timeout_at > Instant::now();
        }
        false
    }
}
