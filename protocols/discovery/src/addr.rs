use std::{
    collections::{BTreeMap, HashMap, HashSet},
    convert::TryFrom,
    io,
    net::{IpAddr, SocketAddr},
    time::Instant,
};

use p2p::{
    multiaddr::Multiaddr,
    utils::{is_reachable, multiaddr_to_socketaddr},
    SessionId,
};

// See: bitcoin/netaddress.cpp pchIPv4[12]
pub(crate) const PCH_IPV4: [u8; 18] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, // ipv4 part
    0, 0, 0, 0, // port part
    0, 0,
];
pub(crate) const DEFAULT_MAX_KNOWN: usize = 5000;

pub enum Misbehavior {
    // Already received GetNodes message
    DuplicateGetNodes,
    // Already received Nodes(announce=false) message
    DuplicateFirstNodes,
    // Nodes message include too many items
    TooManyItems { announce: bool, length: usize },
    // Too many address in one item
    TooManyAddresses(usize),
}

/// Misbehavior report result
pub enum MisbehaveResult {
    /// Continue to run
    Continue,
    /// Disconnect this peer
    Disconnect,
}

impl MisbehaveResult {
    pub fn is_continue(&self) -> bool {
        match self {
            MisbehaveResult::Continue => true,
            _ => false,
        }
    }
    pub fn is_disconnect(&self) -> bool {
        match self {
            MisbehaveResult::Disconnect => true,
            _ => false,
        }
    }
}

// FIXME: Should be peer store?
pub trait AddressManager {
    fn add_new_addr(&mut self, session_id: SessionId, addr: Multiaddr);
    fn add_new_addrs(&mut self, session_id: SessionId, addrs: Vec<Multiaddr>);
    fn misbehave(&mut self, session_id: SessionId, kind: Misbehavior) -> MisbehaveResult;
    fn get_random(&mut self, n: usize) -> Vec<Multiaddr>;
}

// bitcoin: bloom.h, bloom.cpp => CRollingBloomFilter
pub struct AddrKnown {
    max_known: usize,
    addrs: HashSet<RawAddr>,
    addr_times: HashMap<RawAddr, Instant>,
    time_addrs: BTreeMap<Instant, RawAddr>,
}

impl AddrKnown {
    pub(crate) fn new(max_known: usize) -> AddrKnown {
        AddrKnown {
            max_known,
            addrs: HashSet::default(),
            addr_times: HashMap::default(),
            time_addrs: BTreeMap::default(),
        }
    }

    pub(crate) fn insert(&mut self, key: RawAddr) {
        let now = Instant::now();
        self.addrs.insert(key);
        self.time_addrs.insert(now, key);
        self.addr_times.insert(key, now);

        if self.addrs.len() > self.max_known {
            let first_time = {
                let (first_time, first_key) = self.time_addrs.iter().next().unwrap();
                self.addrs.remove(&first_key);
                self.addr_times.remove(&first_key);
                *first_time
            };
            self.time_addrs.remove(&first_time);
        }
    }

    pub(crate) fn contains(&self, addr: &RawAddr) -> bool {
        self.addrs.contains(addr)
    }

    pub(crate) fn remove<'a>(&mut self, addrs: impl Iterator<Item = &'a RawAddr>) {
        addrs.for_each(|addr| {
            self.addrs.remove(addr);
            if let Some(time) = self.addr_times.remove(addr) {
                self.time_addrs.remove(&time);
            }
        })
    }
}

impl Default for AddrKnown {
    fn default() -> AddrKnown {
        AddrKnown::new(DEFAULT_MAX_KNOWN)
    }
}

#[derive(Copy, Clone, Debug, PartialOrd, Ord, Eq, PartialEq, Hash)]
pub struct RawAddr(pub(crate) [u8; 18]);

impl From<&[u8]> for RawAddr {
    fn from(source: &[u8]) -> RawAddr {
        let n = std::cmp::min(source.len(), 18);
        let mut data = PCH_IPV4;
        data.copy_from_slice(&source[0..n]);
        RawAddr(data)
    }
}

impl TryFrom<Multiaddr> for RawAddr {
    type Error = io::Error;
    fn try_from(addr: Multiaddr) -> Result<Self, Self::Error> {
        // FIXME: maybe not socket addr
        match multiaddr_to_socketaddr(&addr) {
            Some(addr) => Ok(RawAddr::from(addr)),
            None => Err(io::ErrorKind::InvalidData.into()),
        }
    }
}

impl From<SocketAddr> for RawAddr {
    // CService::GetKey()
    fn from(addr: SocketAddr) -> RawAddr {
        let mut data = PCH_IPV4;
        match addr.ip() {
            IpAddr::V4(ipv4) => {
                data[12..16].copy_from_slice(&ipv4.octets());
            }
            IpAddr::V6(ipv6) => {
                data[0..16].copy_from_slice(&ipv6.octets());
            }
        }
        let port = addr.port();
        data[16] = (port / 0x100) as u8;
        data[17] = (port & 0x0FF) as u8;
        RawAddr(data)
    }
}

impl RawAddr {
    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.ip(), self.port())
    }

    pub fn ip(&self) -> IpAddr {
        let mut is_ipv4 = true;
        for (i, value) in PCH_IPV4.iter().enumerate().take(12) {
            if self.0[i] != *value {
                is_ipv4 = false;
                break;
            }
        }
        if is_ipv4 {
            let mut buf = [0u8; 4];
            buf.copy_from_slice(&self.0[12..16]);
            From::from(buf)
        } else {
            let mut buf = [0u8; 16];
            buf.copy_from_slice(&self.0[0..16]);
            From::from(buf)
        }
    }

    pub fn port(&self) -> u16 {
        0x100 * u16::from(self.0[16]) + u16::from(self.0[17])
    }

    // Copy from std::net::IpAddr::is_global
    pub fn is_reachable(&self) -> bool {
        is_reachable(self.socket_addr().ip())
    }
}
