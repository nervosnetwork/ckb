use std::{
    collections::HashSet,
    convert::TryFrom,
    io,
    net::{IpAddr, SocketAddr},
};

use p2p::{multiaddr::Multiaddr, utils::multiaddr_to_socketaddr, ProtocolId, SessionId};

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
    // Decode message error
    InvalidData,
}

/// Misbehavior report result
pub enum MisbehaveResult {
    /// Disconnect this peer
    Disconnect,
}

impl MisbehaveResult {
    pub fn is_disconnect(&self) -> bool {
        match self {
            MisbehaveResult::Disconnect => true,
            // _ => false,
        }
    }
}

// FIXME: Should be peer store?
pub trait AddressManager {
    fn register(&self, id: SessionId, pid: ProtocolId, version: &str);
    fn unregister(&self, id: SessionId, pid: ProtocolId);
    fn is_valid_addr(&self, addr: &Multiaddr) -> bool;
    fn add_new_addr(&mut self, session_id: SessionId, addr: Multiaddr);
    fn add_new_addrs(&mut self, session_id: SessionId, addrs: Vec<Multiaddr>);
    fn misbehave(&mut self, session_id: SessionId, kind: Misbehavior) -> MisbehaveResult;
    fn get_random(&mut self, n: usize) -> Vec<Multiaddr>;
}

// bitcoin: bloom.h, bloom.cpp => CRollingBloomFilter
pub struct AddrKnown<T> {
    max_known: usize,
    addrs: HashSet<T>,
    order_addrs: Vec<T>,
}

impl<T> AddrKnown<T>
where
    T: Eq + ::std::hash::Hash + Copy,
{
    pub(crate) fn new(max_known: usize) -> AddrKnown<T> {
        AddrKnown {
            max_known,
            addrs: HashSet::default(),
            order_addrs: Vec::default(),
        }
    }

    pub(crate) fn insert(&mut self, key: T) {
        if self.addrs.insert(key) {
            self.order_addrs.push(key);
        } else {
            return;
        }

        if self.addrs.len() > self.max_known {
            let addr = self.order_addrs.remove(0);
            self.addrs.remove(&addr);
        }
    }

    pub(crate) fn extend(&mut self, mut keys: HashSet<T>) {
        if keys.len() + self.addrs.len() > self.max_known {
            self.addrs.clear();
            self.order_addrs.clear();
            let index = if keys.len() > self.max_known {
                keys.len() - self.max_known
            } else {
                0
            };
            self.order_addrs.extend(keys.iter().skip(index));
            self.addrs = keys.into_iter().skip(index).collect();
        } else {
            let common: HashSet<_> = self.addrs.intersection(&keys).copied().collect();

            if !common.is_empty() {
                for i in common {
                    keys.remove(&i);
                }
            }

            self.order_addrs.extend(keys.iter());
            self.addrs.extend(keys);
        }
    }

    pub(crate) fn contains(&self, addr: &T) -> bool {
        self.addrs.contains(addr)
    }

    pub(crate) fn remove(&mut self, addr: &T) {
        self.order_addrs.retain(|key| key != addr);
        self.addrs.remove(addr);
    }
}

impl<T> Default for AddrKnown<T>
where
    T: Eq + ::std::hash::Hash + Copy,
{
    fn default() -> AddrKnown<T> {
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

#[cfg(test)]
mod test {
    use super::AddrKnown;
    use std::{collections::HashSet, iter::FromIterator};

    #[test]
    fn test_addr_known_behavior() {
        let mut k = AddrKnown::new(10);

        for i in 1..=10 {
            k.insert(i);
        }

        assert_eq!(k.order_addrs, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(
            k.addrs,
            HashSet::from_iter(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10])
        );
        assert_eq!(k.addrs, HashSet::from_iter(k.order_addrs.iter().copied()));

        k.insert(0);

        assert_eq!(k.order_addrs, vec![2, 3, 4, 5, 6, 7, 8, 9, 10, 0]);
        assert_eq!(
            k.addrs,
            HashSet::from_iter(vec![2, 3, 4, 5, 6, 7, 8, 9, 10, 0])
        );
        assert_eq!(k.addrs, HashSet::from_iter(k.order_addrs.iter().copied()));

        k.order_addrs.clear();
        k.addrs.clear();

        k.insert(1);
        k.insert(2);

        k.extend(HashSet::from_iter(vec![3, 4, 5, 6, 7, 8]));

        assert_eq!(k.order_addrs.len(), 8);
        assert_eq!(k.addrs, HashSet::from_iter(vec![1, 2, 3, 4, 5, 6, 7, 8]));
        assert_eq!(k.addrs, HashSet::from_iter(k.order_addrs.iter().copied()));

        k.remove(&1);

        assert_eq!(k.order_addrs.len(), 7);
        assert_eq!(k.addrs, HashSet::from_iter(vec![2, 3, 4, 5, 6, 7, 8]));
        assert_eq!(k.addrs, HashSet::from_iter(k.order_addrs.iter().copied()));

        k.extend(HashSet::from_iter(vec![9, 10, 11, 12]));

        assert_eq!(k.order_addrs.len(), 4);
        assert_eq!(k.addrs, HashSet::from_iter(vec![9, 10, 11, 12]));
        assert_eq!(k.addrs, HashSet::from_iter(k.order_addrs.iter().copied()));

        k.extend(HashSet::from_iter(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 0]));

        assert_eq!(k.order_addrs.len(), 10);
        assert_eq!(
            k.addrs,
            HashSet::from_iter(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 0])
        );
        assert_eq!(k.addrs, HashSet::from_iter(k.order_addrs.iter().copied()));

        k.extend(HashSet::from_iter(vec![1, 2, 3, 4]));

        assert_eq!(k.order_addrs.len(), 4);
        assert_eq!(k.addrs, HashSet::from_iter(vec![1, 2, 3, 4]));
        assert_eq!(k.addrs, HashSet::from_iter(k.order_addrs.iter().copied()));

        k.extend(HashSet::from_iter(vec![4, 5, 6, 7]));

        assert_eq!(k.order_addrs.len(), 7);
        assert_eq!(k.addrs, HashSet::from_iter(vec![1, 2, 3, 4, 5, 6, 7]));
        assert_eq!(k.addrs, HashSet::from_iter(k.order_addrs.iter().copied()));
    }
}
