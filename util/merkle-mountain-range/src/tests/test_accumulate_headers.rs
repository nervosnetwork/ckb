use super::new_blake2b;
use crate::{leaf_index_to_pos, util::MemStore, MMRStore, Merge, MerkleProof, Result, MMR};
use bytes::Bytes;
use std::fmt::{self, Debug};

#[derive(Clone)]
struct Header {
    number: u64,
    parent_hash: Bytes,
    difficulty: u64,
    // MMR root
    chain_root: Bytes,
}

impl Header {
    fn default() -> Self {
        Header {
            number: 0,
            parent_hash: vec![0; 32].into(),
            difficulty: 0,
            chain_root: vec![0; 32].into(),
        }
    }

    fn hash(&self) -> Bytes {
        let mut hasher = new_blake2b();
        let mut hash = [0u8; 32];
        hasher.update(&self.number.to_le_bytes());
        hasher.update(&self.parent_hash);
        hasher.update(&self.difficulty.to_le_bytes());
        hasher.update(&self.chain_root);
        hasher.finalize(&mut hash);
        hash.to_vec().into()
    }
}

#[derive(Eq, PartialEq, Clone, Default)]
struct HashWithTD {
    hash: Bytes,
    td: u64,
}

impl HashWithTD {
    fn serialize(&self) -> Bytes {
        let mut data = self.hash.clone();
        data.extend(&self.td.to_le_bytes());
        data
    }

    fn deserialize(mut data: Bytes) -> Self {
        assert_eq!(data.len(), 40);
        let mut td_bytes = [0u8; 8];
        td_bytes.copy_from_slice(&data[32..]);
        let td = u64::from_le_bytes(td_bytes);
        data.truncate(32);
        HashWithTD { hash: data, td }
    }
}

impl Debug for HashWithTD {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "HashWithTD {{ hash: {}, td: {} }}",
            faster_hex::hex_string(&self.hash).unwrap(),
            self.td
        )
    }
}

struct MergeHashWithTD;

impl Merge for MergeHashWithTD {
    type Item = HashWithTD;
    fn merge(lhs: &Self::Item, rhs: &Self::Item) -> Self::Item {
        let mut hasher = new_blake2b();
        let mut hash = [0u8; 32];
        hasher.update(&lhs.serialize());
        hasher.update(&rhs.serialize());
        hasher.finalize(&mut hash);
        let td = lhs.td + rhs.td;
        HashWithTD {
            hash: hash.to_vec().into(),
            td,
        }
    }
}

struct Prover {
    headers: Vec<(Header, u64)>,
    positions: Vec<u64>,
    store: MemStore<HashWithTD>,
}

impl Prover {
    fn new() -> Prover {
        let store = MemStore::default();
        Prover {
            headers: Vec::new(),
            positions: Vec::new(),
            store,
        }
    }

    fn gen_blocks(&mut self, count: u64) -> Result<()> {
        let mut mmr = MMR::<_, MergeHashWithTD, _>::new(self.positions.len() as u64, &self.store);
        // get previous element
        let mut previous = if let Some(pos) = self.positions.last() {
            MMRStore::<_>::get_elem(&&self.store, *pos)?.expect("exists")
        } else {
            let genesis = Header::default();

            let previous = HashWithTD {
                hash: genesis.hash(),
                td: genesis.difficulty,
            };
            self.headers.push((genesis, previous.td));
            let pos = mmr.push(previous.clone())?;
            self.positions.push(pos);
            previous
        };
        let last_number = self.headers.last().unwrap().0.number;
        for i in (last_number + 1)..=(last_number + count) {
            let block = Header {
                number: i,
                parent_hash: previous.hash.clone(),
                difficulty: i,
                chain_root: mmr.get_root()?.serialize(),
            };
            previous = HashWithTD {
                hash: block.hash(),
                td: block.difficulty,
            };
            let pos = mmr.push(previous.clone())?;
            self.positions.push(pos);
            self.headers.push((block, previous.td));
        }
        mmr.commit()
    }

    fn get_header(&self, number: u64) -> (Header, u64) {
        self.headers[number as usize].clone()
    }

    // generate proof that headers are in same chain
    fn gen_proof(
        &mut self,
        number: u64,
        later_number: u64,
    ) -> Result<MerkleProof<HashWithTD, MergeHashWithTD>> {
        assert!(number < later_number);
        let pos = self.positions[number as usize];
        let later_pos = self.positions[later_number as usize];
        let mmr = MMR::new(later_pos, &self.store);
        assert_eq!(
            mmr.get_root()?.serialize(),
            self.headers[later_number as usize].0.chain_root
        );
        mmr.gen_proof(pos)
    }

    fn get_pos(&self, number: u64) -> u64 {
        self.positions[number as usize]
    }
}

#[test]
fn test_insert_header() {
    let mut prover = Prover::new();
    prover.gen_blocks(30).expect("gen blocks");
    let h1 = 11;
    let h2 = 19;

    // get headers from prover
    let prove_elem = {
        let (header, td) = prover.get_header(h1);
        HashWithTD {
            hash: header.hash(),
            td,
        }
    };
    let root = {
        let (later_header, _later_td) = prover.get_header(h2);
        HashWithTD::deserialize(later_header.chain_root)
    };
    // gen proof,  blocks are in the same chain
    let proof = prover.gen_proof(h1, h2).expect("gen proof");
    let pos = leaf_index_to_pos(h1);
    assert_eq!(pos, prover.get_pos(h1));
    assert_eq!(prove_elem, (&prover.store).get_elem(pos).unwrap().unwrap());
    let result = proof.verify(root, pos, prove_elem).expect("verify");
    assert!(result);
}
