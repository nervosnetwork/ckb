use super::PowEngine;
use byteorder::{ByteOrder, LittleEndian};
use core::header::BlockNumber;
use hash::blake2b;
use std::collections::HashMap;

pub const PROOF_LEN: usize = 42;

pub struct CuckooEngine {
    cuckoo: Cuckoo,
}

impl CuckooEngine {
    pub fn new() -> Self {
        CuckooEngine {
            cuckoo: Cuckoo::new(0x4000_0000, 0x2000_0000, PROOF_LEN),
        }
    }
}

impl Default for CuckooEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl PowEngine for CuckooEngine {
    fn init(&self, _number: BlockNumber) {}

    #[inline]
    fn verify(&self, _number: BlockNumber, message: &[u8], proof: &[u8]) -> bool {
        let mut proof_u32 = vec![];
        LittleEndian::read_u32_into(&proof, &mut proof_u32);
        self.cuckoo.verify(message, &proof_u32)
    }

    #[inline]
    fn solve(&self, _number: BlockNumber, message: &[u8]) -> Option<Vec<u8>> {
        self.cuckoo.solve(message).map(|proof| {
            let mut proof_u8 = vec![];
            LittleEndian::write_u32_into(&proof, &mut proof_u8);
            proof_u8.to_vec()
        })
    }
}

pub struct CuckooSip {
    keys: [u64; 4],
}

impl CuckooSip {
    pub fn new(key0: u64, key1: u64, key2: u64, key3: u64) -> Self {
        Self {
            keys: [key0, key1, key2, key3],
        }
    }

    // https://github.com/tromp/cuckoo/blob/master/doc/spec#L11
    pub fn hash(&self, val: u64) -> u64 {
        let mut v0 = self.keys[0];
        let mut v1 = self.keys[1];
        let mut v2 = self.keys[2];
        let mut v3 = self.keys[3] ^ val;
        CuckooSip::sipround(&mut v0, &mut v1, &mut v2, &mut v3);
        CuckooSip::sipround(&mut v0, &mut v1, &mut v2, &mut v3);
        v0 ^= val;
        v2 ^= 0xff;
        CuckooSip::sipround(&mut v0, &mut v1, &mut v2, &mut v3);
        CuckooSip::sipround(&mut v0, &mut v1, &mut v2, &mut v3);
        CuckooSip::sipround(&mut v0, &mut v1, &mut v2, &mut v3);
        CuckooSip::sipround(&mut v0, &mut v1, &mut v2, &mut v3);

        v0 ^ v1 ^ v2 ^ v3
    }

    // https://github.com/tromp/cuckoo/blob/master/doc/spec#L2
    fn sipround(v0: &mut u64, v1: &mut u64, v2: &mut u64, v3: &mut u64) {
        *v0 = v0.wrapping_add(*v1);
        *v2 = v2.wrapping_add(*v3);
        *v1 = v1.rotate_left(13);

        *v3 = v3.rotate_left(16);
        *v1 ^= *v0;
        *v3 ^= *v2;

        *v0 = v0.rotate_left(32);
        *v2 = v2.wrapping_add(*v1);
        *v0 = v0.wrapping_add(*v3);

        *v1 = v1.rotate_left(17);
        *v3 = v3.rotate_left(21);

        *v1 ^= *v2;
        *v3 ^= *v0;
        *v2 = v2.rotate_left(32);
    }
}

fn message_to_keys(message: &[u8]) -> [u64; 4] {
    let result = blake2b(message);
    [
        LittleEndian::read_u64(&result[0..8]).to_le(),
        LittleEndian::read_u64(&result[8..16]).to_le(),
        LittleEndian::read_u64(&result[16..24]).to_le(),
        LittleEndian::read_u64(&result[24..32]).to_le(),
    ]
}

pub struct Cuckoo {
    max_vertex: usize,
    max_edge: usize,
    cycle_length: usize,
}

impl Cuckoo {
    pub fn new(max_vertex: usize, max_edge: usize, cycle_length: usize) -> Self {
        Self {
            max_vertex,
            max_edge,
            cycle_length,
        }
    }

    // https://github.com/tromp/cuckoo/blob/master/doc/spec#L19
    #[inline]
    pub fn verify(&self, message: &[u8], proof: &[u32]) -> bool {
        if proof.len() != self.cycle_length {
            return false;
        }

        // Check if proof values are in valid range
        if proof.iter().any(|i| *i >= self.max_edge as u32) {
            return false;
        }

        let keys = message_to_keys(message);

        let mut from_upper: HashMap<_, Vec<_>> = HashMap::with_capacity(proof.len());
        let mut from_lower: HashMap<_, Vec<_>> = HashMap::with_capacity(proof.len());
        for (u, v) in proof.iter().map(|i| self.edge(&keys, *i)) {
            from_upper
                .entry(u)
                .and_modify(|upper| upper.push(v))
                .or_insert_with(|| vec![v]);
            from_lower
                .entry(v)
                .and_modify(|lower| lower.push(u))
                .or_insert_with(|| vec![u]);
        }
        if from_upper.values().any(|list| list.len() != 2) {
            return false;
        }
        if from_lower.values().any(|list| list.len() != 2) {
            return false;
        }

        let mut cycle_length = 0;
        let mut cur_edge = self.edge(&keys, proof[0]);
        let start = cur_edge.0;
        loop {
            let next_lower = *from_upper[&cur_edge.0]
                .iter()
                .find(|v| **v != cur_edge.1)
                .unwrap();
            let next_upper = *from_lower[&next_lower]
                .iter()
                .find(|u| **u != cur_edge.0)
                .unwrap();
            cur_edge = (next_upper, next_lower);
            cycle_length += 2;

            if start == cur_edge.0 {
                break;
            }
        }
        cycle_length == self.cycle_length
    }

    #[inline]
    pub fn solve(&self, message: &[u8]) -> Option<Vec<u32>> {
        let mut graph = vec![0; self.max_vertex].into_boxed_slice();
        let keys = message_to_keys(message);

        for nonce in 0..self.max_edge {
            let (u, v) = {
                let edge = self.edge(&keys, nonce as u32);
                (2 * edge.0, 2 * edge.1 + 1)
            };
            if u == 0 {
                continue;
            }
            let path_u = Cuckoo::path(&graph, u);
            let path_v = Cuckoo::path(&graph, v);
            if path_u.last().unwrap() == path_v.last().unwrap() {
                let common = path_u
                    .iter()
                    .rev()
                    .zip(path_v.iter().rev())
                    .take_while(|(u, v)| u == v)
                    .count();
                if (path_u.len() - common) + (path_v.len() - common) + 1 == self.cycle_length {
                    let mut cycle: Vec<_> = {
                        let list: Vec<_> = path_u
                            .iter()
                            .take(path_u.len() - common + 1)
                            .chain(path_v.iter().rev().skip(common))
                            .chain(::std::iter::once(&u))
                            .cloned()
                            .collect();
                        list.windows(2).map(|edge| (edge[0], edge[1])).collect()
                    };
                    let mut result = Vec::with_capacity(PROOF_LEN);
                    for n in 0..self.max_edge {
                        let cur_edge = {
                            let edge = self.edge(&keys, n as u32);
                            (2 * edge.0, 2 * edge.1 + 1)
                        };
                        for i in 0..cycle.len() {
                            let cycle_edge = cycle[i];
                            if cycle_edge == cur_edge || (cycle_edge.1, cycle_edge.0) == cur_edge {
                                result.push(n as u32);
                                cycle.remove(i);
                                break;
                            }
                        }
                    }
                    return Some(result);
                }
            } else if path_u.len() < path_v.len() {
                for edge in path_u.windows(2) {
                    graph[edge[1] as usize] = edge[0];
                }
                graph[u as usize] = v;
            } else {
                for edge in path_v.windows(2) {
                    graph[edge[1] as usize] = edge[0];
                }
                graph[v as usize] = u;
            }
        }
        None
    }

    fn path(graph: &[u64], start: u64) -> Vec<u64> {
        let mut node = start;
        let mut path = vec![start];
        loop {
            node = graph[node as usize];
            if node != 0 {
                path.push(node);
            } else {
                break;
            }
        }
        path
    }

    fn edge(&self, keys: &[u64; 4], index: u32) -> (u64, u64) {
        let hasher = CuckooSip::new(keys[0], keys[1], keys[2], keys[3]);
        let upper = hasher.hash(2 * u64::from(index)) % ((self.max_vertex as u64) / 2);
        let lower = hasher.hash(2 * u64::from(index) + 1) % ((self.max_vertex as u64) / 2);

        (upper, lower)
    }
}

#[cfg(test)]
mod test {
    use super::Cuckoo;

    const TESTSET: [([u8; 80], [u32; 6]); 3] = [
        (
            [
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x1c, 0, 0, 0,
            ],
            [0, 1, 2, 4, 5, 6],
        ),
        (
            [
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x36, 0, 0, 0,
            ],
            [0, 1, 2, 3, 4, 7],
        ),
        (
            [
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xf6, 0, 0, 0,
            ],
            [0, 1, 2, 4, 5, 7],
        ),
    ];

    #[test]
    fn solve_cuckoo() {
        let cuckoo = Cuckoo::new(16, 8, 6);
        for (message, proof) in TESTSET.iter() {
            assert_eq!(cuckoo.solve(message).unwrap(), proof);
        }
    }

    #[test]
    fn verify_cuckoo() {
        let cuckoo = Cuckoo::new(16, 8, 6);
        for (message, proof) in TESTSET.iter() {
            assert!(cuckoo.verify(message, proof));
        }
    }
}
