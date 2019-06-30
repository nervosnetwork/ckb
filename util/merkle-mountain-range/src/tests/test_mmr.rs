use crate::{MMRStore, MerkleElem, MMR};
use ckb_db::MemoryKeyValueDB;
use ckb_hash::Blake2bWriter;
use failure::Error;
use faster_hex::hex_string;
use std::io::Write;

#[derive(Eq, PartialEq, Clone)]
struct NumberHash(Vec<u8>);
impl From<u32> for NumberHash {
    fn from(num: u32) -> Self {
        let mut hasher = Blake2bWriter::new();
        hasher.write_all(&num.to_le_bytes()).unwrap();
        NumberHash(hasher.finalize().to_vec())
    }
}
impl MerkleElem for NumberHash {
    fn merge(lhs: &Self, rhs: &Self) -> Result<Self, Error> {
        let mut hasher = Blake2bWriter::new();
        hasher.write_all(&lhs.0)?;
        hasher.write_all(&rhs.0)?;
        Ok(NumberHash(hasher.finalize().to_vec()))
    }
    fn deserialize(data: Vec<u8>) -> Result<Self, Error> {
        Ok(NumberHash(data))
    }
    fn serialize(&self) -> Result<Vec<u8>, Error> {
        Ok(self.0.clone())
    }
}

fn test_mmr(count: u32, proof_elem: u32) {
    let mut mmr = MMR::new(0, MMRStore::new(MemoryKeyValueDB::open(1), 0));
    let positions: Vec<u64> = (0u32..count)
        .map(|i| mmr.push(NumberHash::from(i)).unwrap())
        .collect();
    let root = mmr.get_root().expect("get root").unwrap();
    let proof = mmr
        .gen_proof(positions[proof_elem as usize])
        .expect("gen proof");
    let result = proof
        .verify(
            root,
            positions[proof_elem as usize],
            NumberHash::from(proof_elem),
        )
        .unwrap();
    assert!(result);
}

#[test]
fn test_mmr_root() {
    let mut mmr = MMR::new(0, MMRStore::new(MemoryKeyValueDB::open(1), 0));
    (0u32..11).for_each(|i| {
        mmr.push(NumberHash::from(i)).unwrap();
    });
    let root = mmr.get_root().expect("get root").unwrap();
    let hex_root = hex_string(&root.0).unwrap();
    assert_eq!(
        "d4aa7a8acce692f046d3b968650723b627b1a0431a659f190823a3bf4c918f0b",
        hex_root
    );
}

#[test]
fn test_mmr_3_peaks() {
    test_mmr(11, 5);
}

#[test]
fn test_mmr_2_peaks() {
    test_mmr(10, 5);
}

#[test]
fn test_mmr_1_peak() {
    test_mmr(8, 5);
}

#[test]
fn test_mmr_first_elem_proof() {
    test_mmr(11, 0);
}

#[test]
fn test_mmr_last_elem_proof() {
    test_mmr(11, 10);
}

#[test]
fn test_mmr_1_elem() {
    test_mmr(1, 0);
}

#[test]
fn test_mmr_2_elems() {
    test_mmr(2, 0);
    test_mmr(2, 1);
}
