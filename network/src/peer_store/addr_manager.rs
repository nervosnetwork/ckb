//! TODO(doc): @driftluo
use crate::peer_store::types::{AddrInfo, IpPort};
use rand::Rng;
use std::collections::{HashMap, HashSet};

/// TODO(doc): @driftluo
#[derive(Default)]
pub struct AddrManager {
    next_id: u64,
    addr_to_id: HashMap<IpPort, u64>,
    id_to_info: HashMap<u64, AddrInfo>,
    random_ids: Vec<u64>,
}

impl AddrManager {
    /// TODO(doc): @driftluo
    pub fn add(&mut self, mut addr_info: AddrInfo) {
        let id = self.next_id;
        let key = addr_info.ip_port();
        if let Some(exists_last_connected_at_ms) =
            self.get(&key).map(|addr| addr.last_connected_at_ms)
        {
            // replace exists addr if has later last_connected_at_ms
            if addr_info.last_connected_at_ms > exists_last_connected_at_ms {
                self.remove(&key);
            } else {
                return;
            }
        }
        self.addr_to_id.insert(key, id);
        addr_info.random_id_pos = self.random_ids.len();
        self.id_to_info.insert(id, addr_info);
        self.random_ids.push(id);
        self.next_id += 1;
    }

    /// randomly return addrs that worth to try or connect.
    pub fn fetch_random<F>(&mut self, count: usize, filter: F) -> Vec<AddrInfo>
    where
        F: Fn(&AddrInfo) -> bool,
    {
        let mut duplicate_ips = HashSet::new();
        let mut addr_infos = Vec::with_capacity(count);
        let mut rng = rand::thread_rng();
        let now_ms = faketime::unix_time_as_millis();
        for i in 0..self.random_ids.len() {
            // reuse the for loop to shuffle random ids
            // https://en.wikipedia.org/wiki/Fisher%E2%80%93Yates_shuffle
            let j = rng.gen_range(i, self.random_ids.len());
            self.swap_random_id(j, i);
            let addr_info: AddrInfo = self.id_to_info[&self.random_ids[i]].to_owned();
            let is_unique_ip = duplicate_ips.insert(addr_info.ip_port.ip);
            // A trick to make our tests work
            // TODO remove this after fix the network tests.
            let is_test_ip =
                addr_info.ip_port.ip.is_unspecified() || addr_info.ip_port.ip.is_loopback();
            if (is_test_ip || is_unique_ip) && !addr_info.is_terrible(now_ms) && filter(&addr_info)
            {
                addr_infos.push(addr_info);
            }
            if addr_infos.len() == count {
                break;
            }
        }
        addr_infos
    }

    /// TODO(doc): @driftluo
    pub fn count(&self) -> usize {
        self.addr_to_id.len()
    }

    /// TODO(doc): @driftluo
    pub fn addrs_iter(&self) -> impl Iterator<Item = &AddrInfo> {
        self.id_to_info.values()
    }

    /// TODO(doc): @driftluo
    pub fn remove(&mut self, addr: &IpPort) -> Option<AddrInfo> {
        if let Some(id) = self.addr_to_id.remove(&addr) {
            let random_id_pos = self.id_to_info.get(&id).expect("exists").random_id_pos;
            // swap with last index, then remove the last index
            self.swap_random_id(random_id_pos, self.random_ids.len() - 1);
            self.random_ids.pop();
            self.id_to_info.remove(&id)
        } else {
            None
        }
    }

    /// TODO(doc): @driftluo
    pub fn get(&self, addr: &IpPort) -> Option<&AddrInfo> {
        self.addr_to_id
            .get(addr)
            .and_then(|id| self.id_to_info.get(&id))
    }

    /// TODO(doc): @driftluo
    pub fn get_mut(&mut self, addr: &IpPort) -> Option<&mut AddrInfo> {
        if let Some(id) = self.addr_to_id.get(addr) {
            self.id_to_info.get_mut(&id)
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
