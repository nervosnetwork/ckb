use bigint::{H256, H520, U256};
use core::block::{Block, Header, UnsignHeader};
use core::proof::Proof;

pub fn genesis_dev() -> Block {
    Block {
        header: Header {
            unsign_header: UnsignHeader {
                pre_hash: H256::from(0),
                timestamp: 0,
                transactions_root: H256::from(0),
                difficulty: U256::from(0),
                challenge: H256::from(0),
                proof: Proof::new(&[0], 0, 0, &H256::from(0)),
                height: 0,
            },
            signature: H520::from(0),
        },
        transactions: vec![],
    }
}

pub fn genesis_testnet() -> Block {
    unimplemented!()
}

pub fn genesis_main() -> Block {
    unimplemented!()
}
