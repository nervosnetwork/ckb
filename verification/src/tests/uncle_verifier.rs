use super::super::block_verifier::UnclesVerifier;
use super::super::error::{Error, UnclesError};
use super::super::pow_verifier::EthashVerifier;
use bigint::{H256, U256};
use chain::chain::{ChainBuilder, ChainProvider};
use chain::store::ChainKVStore;
use core::block::IndexedBlock;
use core::header::{Header, IndexedHeader, RawHeader, Seal};
use core::transaction::{CellInput, CellOutput, Transaction, VERSION};
use core::uncle::UncleBlock;
use core::BlockNumber;
use db::memorydb::MemoryKeyValueDB;
use std::sync::Arc;
use time::set_mock_timer;

fn gen_block(parent_header: IndexedHeader, nonce: u64, difficulty: U256) -> IndexedBlock {
    let now = 1 + parent_header.timestamp;
    let number = parent_header.number + 1;
    let cellbase = create_cellbase(number);
    let header = Header {
        raw: RawHeader {
            number,
            difficulty,
            version: 0,
            parent_hash: parent_header.hash(),
            timestamp: now,
            txs_commit: H256::zero(),
            cellbase_id: cellbase.hash(),
            uncles_hash: H256::zero(),
        },
        seal: Seal {
            nonce,
            mix_hash: H256::from(nonce),
        },
    };

    IndexedBlock {
        header: header.into(),
        transactions: vec![cellbase],
        uncles: vec![],
    }
}

fn create_cellbase(number: BlockNumber) -> Transaction {
    let inputs = vec![CellInput::new_cellbase_input(number)];
    let outputs = vec![CellOutput::new(0, vec![], H256::from(0))];
    Transaction::new(VERSION, Vec::new(), inputs, outputs)
}

fn push_uncle(block: &mut IndexedBlock, uncle: &IndexedBlock) {
    let uncle = UncleBlock {
        header: uncle.header.header.clone(),
        cellbase: uncle.transactions.first().cloned().unwrap(),
    };

    block.uncles.push(uncle);
    block.header.uncles_hash = block.cal_uncles_hash();
    block.finalize_dirty();
}

#[test]
fn test_uncle_verifier() {
    let chain = Arc::new(
        ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .build()
            .unwrap(),
    );
    let number = 20;
    let mut chain1: Vec<IndexedBlock> = Vec::new();
    let mut chain2: Vec<IndexedBlock> = Vec::new();

    set_mock_timer(10);

    let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
    for i in 1..number {
        let difficulty = chain.calculate_difficulty(&parent).unwrap();
        let new_block = gen_block(parent, i, difficulty);
        chain1.push(new_block.clone());
        parent = new_block.header;
    }

    parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();

    // if block_number < 11 { chain1 == chain2 } else { chain1 != chain2 }
    for i in 1..number {
        let difficulty = chain.calculate_difficulty(&parent).unwrap();
        let new_block = gen_block(parent, i + 1000, difficulty);

        chain2.push(new_block.clone());
        parent = new_block.header;
    }

    let mut block = chain1.last().cloned().unwrap();
    let uncle = chain2.last().cloned().unwrap();

    let uncle_block = UncleBlock {
        header: uncle.header.header.clone(),
        cellbase: uncle.transactions.first().cloned().unwrap(),
    };

    block.uncles.push(uncle_block);
    let verify =
        UnclesVerifier::new(&block, Arc::clone(&chain), None as Option<EthashVerifier>).verify();
    // Uncles not match uncles_hash
    assert_eq!(
        verify,
        Err(Error::Uncles(UnclesError::InvalidHash {
            expected: H256::zero(),
            actual: block.cal_uncles_hash()
        }))
    );

    let mut block = chain2.last().cloned().unwrap();
    let uncle = chain1.last().cloned().unwrap();

    push_uncle(&mut block, &uncle);
    let verify =
        UnclesVerifier::new(&block, Arc::clone(&chain), None as Option<EthashVerifier>).verify();

    // Uncle depth is invalid
    assert_eq!(
        verify,
        Err(Error::Uncles(UnclesError::InvalidDepth {
            max: block.number() - 1,
            min: block.number() - chain.consensus().max_uncles_age() as u64,
            actual: 19
        }))
    );

    let mut block = chain2.last().cloned().unwrap();
    let uncle = chain1.get(17).cloned().unwrap();

    push_uncle(&mut block, &uncle);
    let verify =
        UnclesVerifier::new(&block, Arc::clone(&chain), None as Option<EthashVerifier>).verify();
    // Uncle's parent not found
    assert_eq!(verify, Err(Error::UnknownParent(uncle.header.parent_hash)));

    let mut block = chain2.get(10).cloned().unwrap();
    let uncle = chain1.get(8).cloned().unwrap();

    push_uncle(&mut block, &uncle);
    let verify =
        UnclesVerifier::new(&block, Arc::clone(&chain), None as Option<EthashVerifier>).verify();
    // Uncle's parent not found
    assert_eq!(verify, Err(Error::UnknownParent(uncle.header.parent_hash)));

    for block in &chain1 {
        chain.process_block(&block).expect("process block ok");
    }

    // chain2's block in index now
    for block in &chain2 {
        chain.process_block(&block).expect("process block ok");
    }

    let mut block = chain1.get(10).cloned().unwrap();
    let uncle = chain2.get(8).cloned().unwrap();

    push_uncle(&mut block, &uncle);
    let verify =
        UnclesVerifier::new(&block, Arc::clone(&chain), None as Option<EthashVerifier>).verify();

    assert_eq!(verify, Ok(()));

    let mut block = chain2.get(10).cloned().unwrap();
    let uncle = chain1.get(8).cloned().unwrap();

    push_uncle(&mut block, &uncle);
    let verify =
        UnclesVerifier::new(&block, Arc::clone(&chain), None as Option<EthashVerifier>).verify();

    assert_eq!(verify, Ok(()));

    let mut block = chain2.get(8).cloned().unwrap();
    let uncle = chain1.get(8).cloned().unwrap();

    let number = block.number();
    push_uncle(&mut block, &uncle);
    let verify =
        UnclesVerifier::new(&block, Arc::clone(&chain), None as Option<EthashVerifier>).verify();
    assert_eq!(
        verify,
        Err(Error::Uncles(UnclesError::InvalidDepth {
            max: number - 1,
            min: number - 6,
            actual: number
        }))
    );

    let mut block = chain2.last().cloned().unwrap();
    let uncle = chain1.get(8).cloned().unwrap();

    let number = block.number();
    push_uncle(&mut block, &uncle);
    let verify =
        UnclesVerifier::new(&block, Arc::clone(&chain), None as Option<EthashVerifier>).verify();
    assert_eq!(
        verify,
        Err(Error::Uncles(UnclesError::InvalidDepth {
            max: number - 1,
            min: number - 6,
            actual: uncle.number()
        }))
    );

    let mut block = chain2.get(12).cloned().unwrap();
    let uncle = chain1.get(10).cloned().unwrap();

    push_uncle(&mut block, &uncle);
    let verify =
        UnclesVerifier::new(&block, Arc::clone(&chain), None as Option<EthashVerifier>).verify();
    assert_eq!(verify, Ok(()));

    let mut block = chain2.get(12).cloned().unwrap();
    let uncle1 = chain1.get(10).cloned().unwrap();
    let uncle2 = chain1.get(10).cloned().unwrap();
    push_uncle(&mut block, &uncle1);
    push_uncle(&mut block, &uncle2);
    let verify =
        UnclesVerifier::new(&block, Arc::clone(&chain), None as Option<EthashVerifier>).verify();

    // uncle duplicate
    assert_eq!(
        verify,
        Err(Error::Uncles(UnclesError::Duplicate(uncle.header.hash())))
    );

    let mut block = chain2.get(12).cloned().unwrap();

    let max_uncles_len = chain.consensus().max_uncles_len();

    for _ in 0..max_uncles_len + 1 {
        let uncle = chain1.get(10).cloned().unwrap();
        push_uncle(&mut block, &uncle);
    }

    let verify =
        UnclesVerifier::new(&block, Arc::clone(&chain), None as Option<EthashVerifier>).verify();

    // uncle overlength
    assert_eq!(
        verify,
        Err(Error::Uncles(UnclesError::OverLength {
            max: max_uncles_len,
            actual: max_uncles_len + 1
        }))
    );
}
