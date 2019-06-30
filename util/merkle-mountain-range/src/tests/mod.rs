use crate::{MMRStore, MMR};
use bytes::Bytes;
use ckb_db::MemoryKeyValueDB;
use faster_hex::hex_string;

#[test]
fn test_mmr() {
    fn serialize(elem: u32) -> Bytes {
        Bytes::from(&elem.to_le_bytes()[..])
    }
    // TODO
    // 1. optimize backend interface and performance
    // 2. simulate block header accumulation
    // 3. benchmark

    let mut mmr = MMR::new(0, MMRStore::new(MemoryKeyValueDB::open(1), 0));
    let positions: Vec<u64> = (0u32..11)
        .map(|i| mmr.push(&serialize(i)).unwrap())
        .collect();
    let root = mmr.get_root().expect("get root").unwrap();
    let hex_root = hex_string(&root).unwrap();
    assert_eq!(
        "d4aa7a8acce692f046d3b968650723b627b1a0431a659f190823a3bf4c918f0b",
        hex_root
    );
    let proof_elem = 5u32;
    let proof = mmr
        .gen_proof(positions[proof_elem as usize])
        .expect("gen proof");
    let result = proof
        .verify(root, positions[proof_elem as usize], &serialize(proof_elem))
        .unwrap();
    assert!(result);
}
