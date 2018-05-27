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
    pub transactions_root: H256,
    pub difficulty: U256,
    pub height: u64,
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
            transactions_root: H256::from(0),
            difficulty: U256::from(0),
            height: 0,
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
                transactions_root: self.transactions_root,
                difficulty: self.difficulty,
                height: self.height,
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
