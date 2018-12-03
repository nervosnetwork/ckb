use super::{Error, ErrorKind, PeerId, PeerIndex, ProtocolId};
use bytes::Bytes;
use ckb_util::{Mutex, RwLock};
use fnv::FnvHashMap;
use futures::sync::mpsc::UnboundedSender;
use libp2p::core::{Endpoint, Multiaddr, UniqueConnec};
use libp2p::ping;
use peer_store::PeerStore;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

struct PeerConnections {
    id_allocator: AtomicUsize,
    peers: FnvHashMap<PeerId, PeerConnection>,
    pub(crate) peer_id_by_index: FnvHashMap<PeerIndex, PeerId>,
}

impl PeerConnections {
    #[inline]
    fn get<'a>(&'a self, peer_id: &PeerId) -> Option<&'a PeerConnection> {
        self.peers.get(peer_id)
    }

    #[allow(clippy::needless_lifetimes)]
    #[inline]
    fn get_peer_id<'a>(&'a self, peer_index: PeerIndex) -> Option<&'a PeerId> {
        self.peer_id_by_index.get(&peer_index)
    }

    #[inline]
    fn get_mut<'a>(&'a mut self, peer_id: &PeerId) -> Option<&'a mut PeerConnection> {
        self.peers.get_mut(peer_id)
    }

    #[inline]
    fn remove(&mut self, peer_id: &PeerId) -> Option<PeerConnection> {
        if let Some(peer) = self.peers.remove(peer_id) {
            self.peer_id_by_index.remove(&peer.peer_index.unwrap());
            return Some(peer);
        }
        None
    }

    #[allow(clippy::needless_lifetimes)]
    #[inline]
    fn iter<'a>(&'a self) -> impl Iterator<Item = (&'a PeerId, &'a PeerConnection)> {
        self.peers.iter()
    }
    #[inline]
    fn or_insert(&mut self, peer_id: PeerId, peer: PeerConnection) -> PeerIndex {
        let mut peer = peer;
        let peer_index = match peer.peer_index {
            Some(peer_index) => peer_index,
            None => {
                let id = self.id_allocator.fetch_add(1, Ordering::Relaxed);
                peer.peer_index = Some(id);
                id
            }
        };
        self.peers.entry(peer_id.clone()).or_insert(peer);
        self.peer_id_by_index.entry(peer_index).or_insert(peer_id);
        peer_index
    }
}

impl Default for PeerConnections {
    fn default() -> Self {
        PeerConnections {
            id_allocator: AtomicUsize::new(0),
            peers: FnvHashMap::with_capacity_and_hasher(20, Default::default()),
            peer_id_by_index: FnvHashMap::with_capacity_and_hasher(20, Default::default()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PeerIdentifyInfo {
    pub client_version: String,
    pub protocol_version: String,
    pub supported_protocols: Vec<String>,
    pub count_of_known_listen_addrs: usize,
}

type ProtocolConnec = (ProtocolId, UniqueConnec<(UnboundedSender<Bytes>, u8)>);

pub struct PeerConnection {
    pub(crate) peer_index: Option<PeerIndex>,
    pub remote_addresses: Vec<Multiaddr>,
    // Dialer or Listener
    pub endpoint_role: Endpoint,
    // Used for send ping to peer
    pub(crate) pinger_loader: UniqueConnec<ping::Pinger>,
    pub identify_info: Option<PeerIdentifyInfo>,
    pub(crate) ckb_protocols: Vec<ProtocolConnec>,
    pub last_ping_time: Option<Instant>,
}

impl PeerConnection {
    pub fn new(endpoint_role: Endpoint) -> Self {
        PeerConnection {
            endpoint_role,
            // at least should have 1 remote address
            remote_addresses: Vec::with_capacity(1),
            pinger_loader: UniqueConnec::empty(),
            identify_info: None,
            ckb_protocols: Vec::with_capacity(1),
            last_ping_time: None,
            peer_index: None,
        }
    }

    pub fn append_addresses(&mut self, addresses: Vec<Multiaddr>) {
        for addr in addresses {
            if !self.remote_addresses.contains(&addr) {
                self.remote_addresses.push(addr);
            }
        }
    }

    #[inline]
    pub fn is_outgoing(&self) -> bool {
        self.endpoint_role == Endpoint::Dialer
    }

    #[allow(dead_code)]
    #[inline]
    pub fn is_incoming(&self) -> bool {
        !self.is_outgoing()
    }

    #[allow(dead_code)]
    pub fn add_remote_address(&mut self, remote_address: Multiaddr) {
        if self
            .remote_addresses
            .iter()
            .all(|addr| addr != &remote_address)
        {
            self.remote_addresses.push(remote_address);
        }
    }
}

pub struct ConnectionStatus {
    pub total: u32,
    pub unreserved_incoming: u32,
    pub unreserved_outgoing: u32,
    pub max_incoming: u32,
    pub max_outgoing: u32,
}

pub(crate) struct PeersRegistry {
    // store all known peers
    peer_store: Arc<RwLock<Box<PeerStore>>>,
    peer_connections: PeerConnections,
    // max incoming limitation
    max_incoming: u32,
    // max outgoing limitation
    max_outgoing: u32,
    // Only reserved peers or allow all peers.
    reserved_only: bool,
    deny_list: PeersDenyList,
}

impl PeersRegistry {
    pub fn new(
        peer_store: Arc<RwLock<Box<PeerStore>>>,
        max_incoming: u32,
        max_outgoing: u32,
        reserved_only: bool,
    ) -> Self {
        let deny_list = PeersDenyList::new();
        PeersRegistry {
            peer_store,
            peer_connections: Default::default(),
            max_incoming,
            max_outgoing,
            reserved_only,
            deny_list,
        }
    }

    #[allow(clippy::needless_lifetimes)]
    #[inline]
    pub fn get_peer_id<'a>(&'a self, peer_index: PeerIndex) -> Option<&'a PeerId> {
        self.peer_connections.get_peer_id(peer_index)
    }

    // registry a new peer
    #[allow(clippy::needless_pass_by_value)]
    pub fn new_peer(&mut self, peer_id: PeerId, endpoint: Endpoint) -> Result<(), Error> {
        if self.peer_connections.get(&peer_id).is_some() {
            return Ok(());
        }
        let is_reserved = self.peer_store.read().is_reserved(&peer_id);

        if !is_reserved {
            if self.reserved_only {
                return Err(ErrorKind::InvalidNewPeer(format!(
                    "We are in reserved_only mode, rejected non-reserved peer {:?}",
                    peer_id
                ))
                .into());
            }
            if self.deny_list.is_denied(&peer_id) {
                return Err(
                    ErrorKind::InvalidNewPeer(format!("peer {:?} is denied", peer_id)).into(),
                );
            }
            let connection_status = self.connection_status();
            // check peers connection limitation
            match endpoint {
                Endpoint::Listener
                    if connection_status.unreserved_incoming >= self.max_incoming =>
                {
                    return Err(ErrorKind::InvalidNewPeer(format!(
                        "reach max incoming peers limitation, reject peer {:?}",
                        peer_id
                    ))
                    .into())
                }
                Endpoint::Dialer if connection_status.unreserved_outgoing >= self.max_outgoing => {
                    return Err(ErrorKind::InvalidNewPeer(format!(
                        "reach max outgoing peers limitation, reject peer {:?}",
                        peer_id
                    ))
                    .into())
                }
                _ => (),
            }
        }
        let peer = PeerConnection::new(endpoint);
        let peer_index = self.add_peer(peer_id.clone(), peer);
        debug!(target: "network", "allocate peer_index {} to peer {:?}", peer_index, peer_id);
        Ok(())
    }

    // add peer without validation
    #[inline]
    pub fn add_peer(&mut self, peer_id: PeerId, peer_connection: PeerConnection) -> PeerIndex {
        self.peer_connections.or_insert(peer_id, peer_connection)
    }

    #[allow(clippy::needless_lifetimes)]
    #[inline]
    pub fn peers_iter<'a>(&'a self) -> impl Iterator<Item = (&'a PeerId, &'a PeerConnection)> {
        self.peer_connections.iter()
    }

    #[inline]
    pub fn get<'a>(&'a self, peer_id: &PeerId) -> Option<&'a PeerConnection> {
        self.peer_connections.get(peer_id)
    }

    #[inline]
    pub fn get_mut<'a>(&'a mut self, peer_id: &PeerId) -> Option<&'a mut PeerConnection> {
        self.peer_connections.get_mut(peer_id)
    }

    pub fn connection_status(&self) -> ConnectionStatus {
        let mut total: u32 = 0;
        let mut unreserved_incoming: u32 = 0;
        let mut unreserved_outgoing: u32 = 0;
        for (_, peer_connection) in self.peer_connections.iter() {
            total += 1;
            if peer_connection.is_outgoing() {
                unreserved_outgoing += 1;
            } else {
                unreserved_incoming += 1;
            }
        }
        ConnectionStatus {
            total,
            unreserved_incoming,
            unreserved_outgoing,
            max_incoming: self.max_incoming,
            max_outgoing: self.max_outgoing,
        }
    }

    #[inline]
    pub fn connected_peers_indexes<'a>(&'a self) -> impl Iterator<Item = PeerIndex> + 'a {
        Box::new(
            self.peer_connections
                .peer_id_by_index
                .iter()
                .map(|(k, _v)| *k),
        )
    }

    #[inline]
    pub fn drop_peer(&mut self, peer_id: &PeerId) {
        self.peer_connections.remove(peer_id);
    }

    #[inline]
    pub fn drop_all(&mut self) {
        debug!(target: "network", "drop_all");
        self.peer_connections = Default::default();
    }

    pub(crate) fn ban_peer(&mut self, peer_id: PeerId, timeout: Duration) {
        debug!(target: "network", "ban_peer: {:?}", peer_id);
        self.drop_peer(&peer_id);
        self.deny_list.ban_peer(peer_id, timeout);
    }
}

struct PeersDenyList {
    deny_list: Mutex<FnvHashMap<PeerId, Instant>>,
    size: usize,
}

impl PeersDenyList {
    pub fn new() -> Self {
        PeersDenyList {
            deny_list: Mutex::new(Default::default()),
            size: 4096,
        }
    }

    pub fn ban_peer(&self, peer_id: PeerId, timeout: Duration) {
        let now = Instant::now();
        let timeout_stamp = now + timeout;
        let mut deny_list = self.deny_list.lock();
        deny_list.insert(peer_id, timeout_stamp);
        // release memories
        if deny_list.len() > self.size {
            deny_list.retain(move |_peer_id, &mut timeout| timeout < now);
        }
    }

    pub fn is_denied(&self, peer_id: &PeerId) -> bool {
        let mut deny_list = self.deny_list.lock();
        if let Some(timeout) = deny_list.get(peer_id).cloned() {
            if timeout > Instant::now() {
                return true;
            } else {
                deny_list.remove(peer_id);
            }
        }
        false
    }
}
