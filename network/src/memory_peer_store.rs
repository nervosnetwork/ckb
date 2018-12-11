use crate::peer_store::{Behaviour, PeerStore, Score, Status};
use crate::PeerId;
use fnv::FnvHashMap;
use libp2p::core::Multiaddr;
use log::trace;
use std::time::Instant;

// peer_id -> addresses,
// sort by score
// addr -> peer_id
// last report or updated_time
const INITIALIZED_SCORE: u32 = 0;

#[derive(Debug)]
struct PeerInfo {
    addresses: Vec<Multiaddr>,
    last_updated_at: Instant,
    score: u32,
    status: Status,
}

// NOTICE MemoryPeerStore is used for test environment only!!!
pub struct MemoryPeerStore {
    bootnodes: Vec<(PeerId, Multiaddr)>,
    peers: FnvHashMap<PeerId, PeerInfo>,
}

impl MemoryPeerStore {
    pub fn new(bootnodes: Vec<(PeerId, Multiaddr)>) -> Self {
        let mut peer_store = MemoryPeerStore {
            bootnodes: bootnodes.clone(),
            peers: Default::default(),
        };
        for (peer_id, addr) in bootnodes {
            peer_store.add_peer(peer_id, vec![addr]);
        }
        peer_store
    }

    fn add_peer(&mut self, peer_id: PeerId, addresses: Vec<Multiaddr>) -> bool {
        if self.peers.get(&peer_id).is_some() {
            return false;
        }
        let now = Instant::now();
        let peer = PeerInfo {
            addresses,
            last_updated_at: now,
            score: INITIALIZED_SCORE,
            status: Status::Unknown,
        };
        self.peers.insert(peer_id, peer);
        true
    }
}

impl PeerStore for MemoryPeerStore {
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
    // TODO
    fn report(&mut self, _peer_id: &PeerId, _behaviour: Behaviour) {}
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
    //TODO
    fn peer_score(&self, _peer_id: &PeerId) -> Score {
        0
    }

    fn bootnodes<'a>(&'a self) -> Box<Iterator<Item = (&'a PeerId, &'a Multiaddr)> + 'a> {
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
    ) -> Option<Box<Iterator<Item = &'a Multiaddr> + 'a>> {
        let iter = match self.peers.get(peer_id) {
            Some(peer) => peer.addresses.iter(),
            None => return None,
        };
        Some(Box::new(iter) as Box<_>)
    }
    fn peers_to_attempt<'a>(&'a self) -> Box<Iterator<Item = (&'a PeerId, &'a Multiaddr)> + 'a> {
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
}
