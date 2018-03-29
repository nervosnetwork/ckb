use bigint::{H256, H520, U256};
use core::block::{Block, Header, RawHeader};
use core::proof::Proof;

pub fn genesis_dev() -> Block {
    let raw = RawHeader {
        pre_hash: H256::from(0),
        timestamp: 0,
        transactions_root: H256::from(0),
        difficulty: U256::from(0),
        challenge: H256::from(0),
        proof: Proof::default(),
        height: 0,
    };

    Block {
        header: Header::new(raw, U256::from(0), Some(H520::from(0))),
        transactions: vec![],
    }
}

// TODO: should be const
pub fn genesis_hash() -> H256 {
    genesis_dev().hash()
}

pub fn genesis_testnet() -> Block {
    unimplemented!()
}

pub fn genesis_main() -> Block {
    unimplemented!()
}
