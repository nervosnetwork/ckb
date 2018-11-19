use bigint::{H256, U256};
use core::block::IndexedBlock;
use core::header::{Header, RawHeader};
use core::transaction::Transaction;

// This function creates a dummy transaction, we can then
// tweak inputs and outputs later
pub fn create_dummy_transaction() -> Transaction {
    Transaction::new(0, vec![], vec![], vec![])
}

pub fn create_dummy_block() -> IndexedBlock {
    let raw_header = RawHeader {
        version: 0,
        parent_hash: H256::zero(),
        timestamp: 0,
        number: 123,
        txs_commit: H256::zero(),
        txs_proposal: H256::zero(),
        difficulty: U256::zero(),
        cellbase_id: H256::zero(),
        uncles_hash: H256::zero(),
    };
    let header = Header {
        raw: raw_header,
        seal: Default::default(),
    };
    IndexedBlock {
        header: header.into(),
        commit_transactions: vec![],
        proposal_transactions: vec![],
        uncles: vec![],
    }
}
