use super::PowEngine;
use byteorder::{ByteOrder, LittleEndian};
use ckb_core::header::BlockNumber;
use hash::blake2b;
use serde::{de, Deserialize};
use serde_derive::Deserialize;
use std::any::Any;
use std::collections::HashMap;

// Cuckatoo proofs take the form of a length 42 off-by-1-cycle in a bipartite graph with
// 2^N+2^N nodes and 2^N edges, with N ranging from 10 up to 64.
#[derive(Copy, Clone, Deserialize, Eq, PartialEq, Hash, Debug)]
pub struct CuckooParams {
    // the main parameter is the 2-log of the graph size,
    // which is the size in bits of the node identifiers
    edge_bits: u8,
    // the next most important parameter is the (even) length
    // of the cycle to be found. a minimum of 12 is recommended
    #[serde(deserialize_with = "validate_cycle_length")]
    cycle_length: u32,
}

fn validate_cycle_length<'de, D>(d: D) -> Result<u32, D::Error>
where
    D: de::Deserializer<'de>,
{
    let value = u32::deserialize(d)?;

    if value & 1 == 1 {
        Err(de::Error::invalid_value(
            de::Unexpected::Unsigned(value.into()),
            &"cycle_length must be even",
        ))
    } else {
        Ok(value)
    }
}

pub struct CuckooEngine {
    cuckoo: Cuckoo,
}

impl CuckooEngine {
    pub fn new(params: CuckooParams) -> Self {
        CuckooEngine {
            cuckoo: Cuckoo::new(params.edge_bits, params.cycle_length as usize),
        }
    }
}

impl Default for CuckooParams {
    fn default() -> Self {
        CuckooParams {
            edge_bits: 29,
            cycle_length: 42,
        }
    }
}

impl PowEngine for CuckooEngine {
    fn init(&self, _number: BlockNumber) {}

    #[inline]
    fn verify(&self, _number: BlockNumber, message: &[u8], proof: &[u8]) -> bool {
        let mut proof_u32 = vec![0u32; self.cuckoo.cycle_length];
        LittleEndian::read_u32_into(&proof, &mut proof_u32);
        self.cuckoo.verify(message, &proof_u32)
    }

    #[inline]
    fn solve(&self, _number: BlockNumber, message: &[u8]) -> Option<Vec<u8>> {
        self.cuckoo.solve(message).map(|proof| {
            let mut proof_u8 = vec![0u8; self.cuckoo.cycle_length * 4];
            LittleEndian::write_u32_into(&proof, &mut proof_u8);
            proof_u8
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
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
    fn hash(&self, val: u64) -> u64 {
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

    pub fn edge(&self, val: u32, edge_mask: u64) -> (u64, u64) {
        let upper = self.hash(u64::from(val) << 1) & edge_mask;
        let lower = self.hash((u64::from(val) << 1) + 1) & edge_mask;

        (upper, lower)
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
    max_edge: u64,
    edge_mask: u64,
    cycle_length: usize,
}

impl Cuckoo {
    pub fn new(edge_bits: u8, cycle_length: usize) -> Self {
        Self {
            max_edge: 1 << edge_bits,
            edge_mask: (1 << edge_bits) - 1,
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
        let hasher = CuckooSip::new(keys[0], keys[1], keys[2], keys[3]);

        let mut from_upper: HashMap<_, Vec<_>> = HashMap::with_capacity(proof.len());
        let mut from_lower: HashMap<_, Vec<_>> = HashMap::with_capacity(proof.len());
        for (u, v) in proof.iter().map(|i| hasher.edge(*i, self.edge_mask)) {
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
        let mut cur_edge = hasher.edge(proof[0], self.edge_mask);
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
        let mut graph = vec![0; (self.max_edge << 1) as usize].into_boxed_slice();
        let keys = message_to_keys(message);
        let hasher = CuckooSip::new(keys[0], keys[1], keys[2], keys[3]);

        for nonce in 0..self.max_edge {
            let (u, v) = {
                let edge = hasher.edge(nonce as u32, self.edge_mask);
                (edge.0 << 1, (edge.1 << 1) + 1)
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
                    let mut result = Vec::with_capacity(self.cycle_length);
                    for n in 0..self.max_edge {
                        let cur_edge = {
                            let edge = hasher.edge(n as u32, self.edge_mask);
                            (edge.0 << 1, (edge.1 << 1) + 1)
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
}

#[cfg(test)]
mod test {
    use super::Cuckoo;

    use proptest::{collection::size_range, prelude::*};

    fn _cuckoo_solve(message: &[u8]) -> Result<(), TestCaseError> {
        let cuckoo = Cuckoo::new(3, 6);
        if let Some(proof) = cuckoo.solve(message) {
            prop_assert!(cuckoo.verify(message, &proof));
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn cuckoo_solve(ref message in any_with::<Vec<u8>>(size_range(80).lift())) {
            _cuckoo_solve(message)?;
        }
    }

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
        let cuckoo = Cuckoo::new(3, 6);
        for (message, proof) in TESTSET.iter() {
            assert_eq!(cuckoo.solve(message).unwrap(), proof);
        }
    }

    #[test]
    fn verify_cuckoo() {
        let cuckoo = Cuckoo::new(3, 6);
        for (message, proof) in TESTSET.iter() {
            assert!(cuckoo.verify(message, proof));
        }
    }
}
