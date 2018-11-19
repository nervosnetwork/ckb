use bigint::{H256, U256};
use core::block::IndexedBlock;
use core::header::{Header, RawHeader, Seal};
use core::transaction::Transaction;

// This function creates a dummy transaction, we can then
// tweak inputs and outputs later
pub fn create_dummy_transaction() -> Transaction {
    Transaction::new(0, Vec::new(), Vec::new(), Vec::new())
}

pub fn create_dummy_block() -> IndexedBlock {
    let raw_header = RawHeader {
        version: 0,
        parent_hash: H256::from(0),
        timestamp: 0,
        number: 123,
        txs_commit: H256::from(0),
        difficulty: U256::from(0),
    };
    let header = Header {
        raw: raw_header,
        seal: Seal {
            nonce: 0,
            mix_hash: H256::from(0),
        },
    };
    IndexedBlock {
        header: header.into(),
        transactions: Vec::new(),
    }
}
