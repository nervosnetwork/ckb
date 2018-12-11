use crate::PeerId;
use libp2p::core::Multiaddr;
// TODO
// 1. maintain peer and addresses
// 2. provide interface to score peer by difference behaviours
// 3. cleanup expired peers?
// 4. limit stored peers by ip
// 5. limit peers from same ip group
// 6. maintain reserved_node behaviours?

#[allow(dead_code)]
#[derive(Copy, Clone, Eq, PartialEq)]
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

pub type Score = i32;

pub trait PeerStore: Send + Sync {
    // update peer addresses, return numbers of new inserted line
    // return Err if peer not exists
    fn add_discovered_address(&mut self, peer_id: &PeerId, address: Multiaddr) -> Result<(), ()>;
    fn add_discovered_addresses(
        &mut self,
        peer_id: &PeerId,
        addresses: Vec<Multiaddr>,
    ) -> Result<usize, ()>;
    fn report(&mut self, peer_id: &PeerId, behaviour: Behaviour);
    fn update_status(&mut self, peer_id: &PeerId, status: Status);
    fn peer_status(&self, peer_id: &PeerId) -> Status;
    fn peer_score(&self, peer_id: &PeerId) -> Score;
    // should return high scored nodes if possible, otherwise, return boostrap nodes
    fn bootnodes<'a>(&'a self) -> Box<Iterator<Item = (&'a PeerId, &'a Multiaddr)> + 'a>;
    fn peer_addrs<'a>(
        &'a self,
        peer_id: &'a PeerId,
    ) -> Option<Box<Iterator<Item = &'a Multiaddr> + 'a>>;
    fn peers_to_attempt<'a>(&'a self) -> Box<Iterator<Item = (&'a PeerId, &'a Multiaddr)> + 'a>;
}
