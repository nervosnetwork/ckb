use bigint::{H256, U256};
use core::block::Block;
use core::header::{Header, RawHeader, Seal};
use nervos_verification::VerifierType;

#[derive(Debug, Clone)]
pub struct Spec {
    // genesis data
    pub version: u32,
    pub parent_hash: H256,
    pub hash: H256,
    pub timestamp: u64,
    pub txs_commit: H256,
    pub difficulty: U256,
    pub number: u64,
    pub nonce: u64,
    pub mix_hash: H256,
    // other config
    pub verifier_type: VerifierType,
}

impl Spec {
    // TODO load from json file
    pub fn default() -> Self {
        Spec {
            version: 0,
            parent_hash: H256::from(0),
            hash: H256::from("0x1d3c78fcf6a6c98b53aed1bfebe53d5d7a1a0b8dced33576e3806915ce51aa00"),
            timestamp: 0,
            txs_commit: H256::from(0),
            difficulty: U256::from(0),
            number: 0,
            nonce: 0,
            mix_hash: H256::from(0),
            verifier_type: VerifierType::Normal,
        }
    }

    pub fn genesis_block(&self) -> Block {
        let header = Header {
            raw: RawHeader {
                version: self.version,
                parent_hash: self.parent_hash,
                timestamp: self.timestamp,
                txs_commit: self.txs_commit,
                difficulty: self.difficulty,
                number: self.number,
            },
            seal: Seal {
                nonce: self.nonce,
                mix_hash: self.mix_hash,
            },
            hash: Some(self.hash),
        };

        Block {
            header,
            transactions: vec![],
        }
    }
}
