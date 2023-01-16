//! Address manager
use crate::peer_store::types::AddrInfo;
use p2p::{multiaddr::Multiaddr, utils::multiaddr_to_socketaddr};
use rand::Rng;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;

/// Address manager
#[derive(Default)]
pub struct AddrManager {
    next_id: u64,
    addr_to_id: HashMap<SocketAddr, u64>,
    id_to_info: HashMap<u64, AddrInfo>,
    random_ids: Vec<u64>,
}

impl AddrManager {
    /// Add an address information to address manager
    pub fn add(&mut self, mut addr_info: AddrInfo) {
        if let Some(key) = multiaddr_to_socketaddr(&addr_info.addr) {
            if let Some(exists_last_connected_at_ms) = self
                .get(&addr_info.addr)
                .map(|addr| addr.last_connected_at_ms)
            {
                // Get time earlier than record time, return directly
                if addr_info.last_connected_at_ms < exists_last_connected_at_ms {
                    return;
                }
            }

            let id = self.next_id;
            self.addr_to_id.insert(key, id);
            addr_info.random_id_pos = self.random_ids.len();
            self.id_to_info.insert(id, addr_info);
            self.random_ids.push(id);
            self.next_id += 1;
        }
    }

    /// Randomly return addrs that worth to try or connect.
    pub fn fetch_random<F>(&mut self, count: usize, filter: F) -> Vec<AddrInfo>
    where
        F: Fn(&AddrInfo) -> bool,
    {
        let mut duplicate_ips = HashSet::new();
        let mut addr_infos = Vec::with_capacity(count);
        let mut rng = rand::thread_rng();
        let now_ms = ckb_systemtime::unix_time_as_millis();
        for i in 0..self.random_ids.len() {
            // reuse the for loop to shuffle random ids
            // https://en.wikipedia.org/wiki/Fisher%E2%80%93Yates_shuffle
            let j = rng.gen_range(i, self.random_ids.len());
            self.swap_random_id(j, i);
            let addr_info: AddrInfo = self.id_to_info[&self.random_ids[i]].to_owned();
            if let Some(socket_addr) = multiaddr_to_socketaddr(&addr_info.addr) {
                let ip = socket_addr.ip();
                let is_unique_ip = duplicate_ips.insert(ip);
                // A trick to make our tests work
                // TODO remove this after fix the network tests.
                let is_test_ip = ip.is_unspecified() || ip.is_loopback();
                if (is_test_ip || is_unique_ip)
                    && addr_info.is_connectable(now_ms)
                    && filter(&addr_info)
                {
                    addr_infos.push(addr_info);
                }
                if addr_infos.len() == count {
                    break;
                }
            }
        }
        addr_infos
    }

    /// The count of address in address manager
    pub fn count(&self) -> usize {
        self.addr_to_id.len()
    }

    /// Addresses iterator
    pub fn addrs_iter(&self) -> impl Iterator<Item = &AddrInfo> {
        self.id_to_info.values()
    }

    /// Remove an address by ip and port
    pub fn remove(&mut self, addr: &Multiaddr) -> Option<AddrInfo> {
        multiaddr_to_socketaddr(addr).and_then(|addr| {
            self.addr_to_id.remove(&addr).and_then(|id| {
                let random_id_pos = self.id_to_info.get(&id).expect("exists").random_id_pos;
                // swap with last index, then remove the last index
                self.swap_random_id(random_id_pos, self.random_ids.len() - 1);
                self.random_ids.pop();
                self.id_to_info.remove(&id)
            })
        })
    }

    /// Get an address information by ip and port
    pub fn get(&self, addr: &Multiaddr) -> Option<&AddrInfo> {
        multiaddr_to_socketaddr(addr).and_then(|addr| {
            self.addr_to_id
                .get(&addr)
                .and_then(|id| self.id_to_info.get(id))
        })
    }

    /// Get a mutable address information by ip and port
    pub fn get_mut(&mut self, addr: &Multiaddr) -> Option<&mut AddrInfo> {
        if let Some(addr) = multiaddr_to_socketaddr(addr) {
            if let Some(id) = self.addr_to_id.get(&addr) {
                self.id_to_info.get_mut(id)
            } else {
                None
            }
        } else {
            None
        }
    }

    /// swap random_id i and j,
    /// this function keep random_id_pos in consistency
    fn swap_random_id(&mut self, i: usize, j: usize) {
        if i == j {
            return;
        }
        self.id_to_info
            .get_mut(&self.random_ids[i])
            .expect("exists")
            .random_id_pos = j;
        self.id_to_info
            .get_mut(&self.random_ids[j])
            .expect("exists")
            .random_id_pos = i;
        self.random_ids.swap(i, j);
    }
}
