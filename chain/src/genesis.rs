use bigint::{H256, U256};
use core::block::Block;
use core::header::{Header, RawHeader, Seal};

pub fn genesis_dev() -> Block {
    let header = Header {
        raw: RawHeader {
            version: 0,
            parent_hash: H256::from(0),
            timestamp: 0,
            transactions_root: H256::from(0),
            difficulty: U256::from(0),
            height: 0,
        },
        seal: Seal {
            nonce: 0,
            mix_hash: H256::from(0),
        },
        hash: None,
    };

    Block {
        header,
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
