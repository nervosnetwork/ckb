// TODO
// Done 1. optimize backend interface and performance
// 2. simulate block header accumulation
// 3. benchmark

use crate::{MMRStore, MerkleElem, MerkleProof, Result, MMR};
use ckb_db::MemoryKeyValueDB;
use ckb_hash::Blake2bWriter;
use std::io::Write;
use std::sync::Arc;

#[derive(Clone)]
struct Header {
    number: u64,
    parent_hash: Vec<u8>,
    difficulty: u64,
    // MMR root
    chain_commitment: Vec<u8>,
}

impl Header {
    fn default() -> Self {
        Header {
            number: 0,
            parent_hash: vec![0; 32].into(),
            difficulty: 0,
            chain_commitment: vec![0; 32].into(),
        }
    }

    fn hash(&self) -> Vec<u8> {
        let mut hasher = Blake2bWriter::new();
        hasher.write_all(&self.number.to_le_bytes()).expect("write");
        hasher.write_all(&self.parent_hash).expect("write");
        hasher
            .write_all(&self.difficulty.to_le_bytes())
            .expect("write");
        hasher.write_all(&self.chain_commitment).expect("write");
        hasher.finalize().to_vec()
    }
}

#[derive(Eq, PartialEq, Clone)]
struct HashWithTD {
    hash: Vec<u8>,
    td: u64,
}

use std::fmt::{self, Debug};

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

impl MerkleElem for HashWithTD {
    fn serialize(&self) -> Result<Vec<u8>> {
        let mut data = self.hash.clone();
        data.extend(&self.td.to_le_bytes());
        Ok(data)
    }

    fn deserialize(mut data: Vec<u8>) -> Result<Self> {
        assert_eq!(data.len(), 40);
        let mut td_bytes = [0u8; 8];
        td_bytes.copy_from_slice(&data[32..]);
        let td = u64::from_le_bytes(td_bytes);
        data.truncate(32);
        Ok(HashWithTD { hash: data, td })
    }

    fn merge(lhs: &Self, rhs: &Self) -> Result<Self> {
        let mut hasher = Blake2bWriter::new();
        hasher.write_all(&lhs.serialize()?)?;
        hasher.write_all(&rhs.serialize()?)?;
        let hash = hasher.finalize().to_vec();
        let td = lhs.td + rhs.td;
        let parent = HashWithTD { hash, td };
        Ok(parent)
    }
}

struct Prover {
    headers: Vec<(Header, u64)>,
    positions: Vec<u64>,
    mmr_store: Arc<MMRStore<HashWithTD, MemoryKeyValueDB>>,
    mmr: MMR<HashWithTD, MemoryKeyValueDB>,
}

impl Prover {
    fn new() -> Self {
        let mmr_store = Arc::new(MMRStore::new(MemoryKeyValueDB::open(1), 0));
        let mmr = MMR::new(0, Arc::clone(&mmr_store));
        Prover {
            headers: Vec::new(),
            positions: Vec::new(),
            mmr,
            mmr_store,
        }
    }

    fn gen_blocks(&mut self, count: u64) -> Result<()> {
        let mut previous = if let Some(pos) = self.positions.last() {
            self.mmr_store.get_elem(*pos)?.expect("exists")
        } else {
            let genesis = Header::default();

            let previous = HashWithTD {
                hash: genesis.hash(),
                td: genesis.difficulty,
            };
            self.headers.push((genesis, previous.td));
            let pos = self.mmr.push(previous.clone())?;
            self.positions.push(pos);
            previous
        };
        let last_number = self.headers.last().unwrap().0.number;
        for i in (last_number + 1)..=(last_number + count) {
            let block = Header {
                number: i,
                parent_hash: previous.hash.clone(),
                difficulty: i,
                chain_commitment: self.mmr.get_root()?.unwrap().serialize()?,
            };
            previous = HashWithTD {
                hash: block.hash(),
                td: previous.td + block.difficulty,
            };
            let pos = self.mmr.push(previous.clone())?;
            self.positions.push(pos);
            self.headers.push((block, previous.td));
        }
        Ok(())
    }

    fn get_header(&self, number: u64) -> (Header, u64) {
        self.headers[number as usize].clone()
    }

    // generate proof that headers are in same chain
    fn gen_proof(&self, number: u64, later_number: u64) -> Result<MerkleProof<HashWithTD>> {
        assert!(number < later_number);
        let pos = self.positions[number as usize];
        let later_pos = self.positions[later_number as usize];
        let mmr = MMR::new(later_pos, Arc::clone(&self.mmr_store));
        assert_eq!(
            mmr.get_root()?.unwrap().serialize()?,
            self.headers[later_number as usize].0.chain_commitment
        );
        mmr.gen_proof(pos)
    }

    fn get_pos(&self, number: u64) -> u64 {
        self.positions[number as usize]
    }
}

#[test]
fn test_insert_header() {
    fn leaf_index_to_pos(index: u64) -> u64 {
        if index == 0 {
            return 0;
        }
        let mut count = index + 1;
        let mut pos = 0;
        while count > 1 {
            let height = (count as f64).log2() as u64;
            let peak_leafs = 1 << height;
            pos += (1 << (height + 1)) - 1;
            count -= peak_leafs;
        }
        if count > 0 {
            pos += 1;
        } else {
            pos -= 3;
        }
        pos
    }
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
        HashWithTD::deserialize(later_header.chain_commitment).expect("deserialize")
    };
    // gen proof,  blocks are in the same chain
    let proof = prover.gen_proof(h1, h2).expect("gen proof");
    let pos = leaf_index_to_pos(h1);
    assert_eq!(pos, prover.get_pos(h1));
    assert_eq!(
        prove_elem,
        prover.mmr_store.get_elem(pos).expect("get elem").unwrap()
    );
    let result = proof.verify(root, pos, prove_elem).expect("verify");
    assert!(result);
}
