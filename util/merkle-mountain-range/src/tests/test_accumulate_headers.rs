// TODO
// Done 1. optimize backend interface and performance
// 2. simulate block header accumulation
// 3. benchmark

// use ckb_hash::Blake2bWriter;
// use failure::Error;
// use std::io::Write;
//
// struct Header {
//     number: u64,
//     parent_hash: Vec<u8>,
//     difficulty: u64,
//     // MMR root
//     chain_commitment: Vec<u8>,
// }
//
// impl Header {
//     fn default() -> Self {
//         Header {
//             number: 0,
//             parent_hash: vec![0; 32].into(),
//             difficulty: 0,
//             chain_commitment: vec![0; 32].into(),
//         }
//     }
//
//     fn hash(&self) -> Vec<u8> {
//         let mut hasher = Blake2bWriter::new();
//         hasher.write_all(&self.number.to_le_bytes()).expect("write");
//         hasher.write_all(&self.parent_hash).expect("write");
//         hasher
//             .write_all(&self.difficulty.to_le_bytes())
//             .expect("write");
//         hasher.write_all(&self.chain_commitment).expect("write");
//         &hasher.finalize()[..]
//     }
// }
//
// struct Elem {
//     hash: Vec<u8>,
//     td: u64,
// }
//
// #[test]
// fn test_insert_header() {
//     let mut mmr = MMR::new(0, MMRStore::new(MemoryKeyValueDB::open(1), 0));
//     let genesis = Header::default();
//     mmr.push(&genesis).expect("push header");
//     let _root = mmr.get_root().expect("get root").unwrap();
// }
