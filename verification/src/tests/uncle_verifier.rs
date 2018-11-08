use super::super::block_verifier::UnclesVerifier;
use super::super::error::{Error, UnclesError};
use super::utils::dummy_pow_engine;
use bigint::{H256, U256};
use chain::chain::{ChainBuilder, ChainProvider};
use chain::consensus::Consensus;
use chain::store::ChainKVStore;
use core::block::{Block, BlockBuilder};
use core::header::{Header, HeaderBuilder};
use core::transaction::{CellInput, CellOutput, ProposalShortId, Transaction, TransactionBuilder};
use core::BlockNumber;
use db::memorydb::MemoryKeyValueDB;
use std::sync::Arc;
use time::set_mock_timer;

fn gen_block(parent_header: Header, nonce: u64, difficulty: U256) -> Block {
    let now = 1 + parent_header.timestamp();
    let number = parent_header.number() + 1;
    let cellbase = create_cellbase(number);
    let header_builder = HeaderBuilder::default()
        .parent_hash(&parent_header.hash())
        .timestamp(now)
        .number(number)
        .difficulty(&difficulty)
        .cellbase_id(&cellbase.hash())
        .nonce(nonce);

    BlockBuilder::default()
        .commit_transaction(cellbase)
        .proposal_transaction(ProposalShortId::from_slice(&[1; 10]).unwrap())
        .with_header_builder(header_builder)
}

fn create_cellbase(number: BlockNumber) -> Transaction {
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .output(CellOutput::new(0, vec![], H256::from(0), None))
        .build()
}

#[test]
fn test_uncle_verifier() {
    let mut consensus = Consensus::default();
    consensus.pow_time_span = 10;
    consensus.pow_spacing = 1;
    let chain = Arc::new(
        ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .consensus(consensus)
            .build()
            .unwrap(),
    );

    assert_eq!(chain.consensus().difficulty_adjustment_interval(), 10);
    let pow = dummy_pow_engine();
    let number = 20;
    let mut chain1: Vec<Block> = Vec::new();
    let mut chain2: Vec<Block> = Vec::new();

    set_mock_timer(10);

    let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
    for i in 1..number {
        let difficulty = chain.calculate_difficulty(&parent).unwrap();
        let new_block = gen_block(parent, i, difficulty);
        chain.process_block(&new_block).expect("process block ok");
        chain1.push(new_block.clone());
        parent = new_block.header().clone();
    }

    parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();

    // if block_number < 11 { chain1 == chain2 } else { chain1 != chain2 }
    for i in 1..number {
        let difficulty = chain.calculate_difficulty(&parent).unwrap();
        let new_block = gen_block(parent, i + 1000, difficulty);
        chain.process_block(&new_block).expect("process block ok");
        chain2.push(new_block.clone());
        parent = new_block.header().clone();
    }

    let block = BlockBuilder::default()
        .block(chain1.last().cloned().unwrap())
        .uncle(chain2.last().cloned().unwrap().into())
        .build();

    let verify = UnclesVerifier::new(&block, &chain, &pow).verify();
    // Uncles not match uncles_hash
    assert_eq!(
        verify,
        Err(Error::Uncles(UnclesError::InvalidHash {
            expected: H256::zero(),
            actual: block.cal_uncles_hash()
        }))
    );

    let block = BlockBuilder::default()
        .block(chain2.last().cloned().unwrap())
        .uncle(chain1.last().cloned().unwrap().into())
        .with_header_builder(
            HeaderBuilder::default().header(chain2.last().unwrap().header().clone()),
        );

    let verify = UnclesVerifier::new(&block, &chain, &pow).verify();
    // Uncle depth is invalid
    assert_eq!(
        verify,
        Err(Error::Uncles(UnclesError::InvalidDepth {
            max: block.header().number() - 1,
            min: block.header().number() - chain.consensus().max_uncles_age() as u64,
            actual: 19
        }))
    );

    let block = BlockBuilder::default()
        .block(chain1.get(8).cloned().unwrap())
        .uncle(chain2.get(6).cloned().unwrap().into())
        .with_header_builder(
            HeaderBuilder::default()
                .header(chain1.get(8).unwrap().header().clone())
                .difficulty(&U256::from(2)),
        );

    let verify = UnclesVerifier::new(&block, &chain, &pow).verify();
    assert_eq!(verify, Err(Error::Uncles(UnclesError::InvalidDifficulty)));

    let block = BlockBuilder::default()
        .block(chain1.get(9).cloned().unwrap())          // block.number 10 epoch 1
        .uncle(chain2.get(6).cloned().unwrap().into())   // block.number 7 epoch 0
        .with_header_builder(HeaderBuilder::default().header(
            chain1.get(9).unwrap().header().clone()
        ));

    let verify = UnclesVerifier::new(&block, &chain, &pow).verify();
    assert_eq!(
        verify,
        Err(Error::Uncles(UnclesError::InvalidDifficultyEpoch))
    );

    let uncle = BlockBuilder::default()
        .block(chain2.get(6).cloned().unwrap())
        .proposal_transaction(ProposalShortId::from_slice(&[1; 10]).unwrap())
        .build();

    let block = BlockBuilder::default()
        .block(chain1.get(8).cloned().unwrap())
        .uncle(uncle.into())
        .with_header_builder(
            HeaderBuilder::default().header(chain1.get(8).unwrap().header().clone()),
        );

    let verify = UnclesVerifier::new(&block, &chain, &pow).verify();
    assert_eq!(
        verify,
        Err(Error::Uncles(UnclesError::ProposalTransactionsRoot))
    );

    let uncle = BlockBuilder::default()
        .block(chain2.get(6).cloned().unwrap())
        .proposal_transaction(ProposalShortId::from_slice(&[1; 10]).unwrap())
        .with_header_builder(
            HeaderBuilder::default().header(chain2.get(6).unwrap().header().clone()),
        );

    let block = BlockBuilder::default()
        .block(chain1.get(8).cloned().unwrap())
        .uncle(uncle.into())
        .with_header_builder(
            HeaderBuilder::default().header(chain1.get(8).unwrap().header().clone()),
        );

    let verify = UnclesVerifier::new(&block, &chain, &pow).verify();
    assert_eq!(
        verify,
        Err(Error::Uncles(UnclesError::ProposalTransactionDuplicate))
    );

    let uncle = BlockBuilder::default()
        .block(chain2.get(6).cloned().unwrap())
        .with_header_builder(
            HeaderBuilder::default().header(chain2.get(6).unwrap().header().clone()),
        );

    let block = BlockBuilder::default()
        .block(chain1.get(8).cloned().unwrap())
        .uncle(uncle.into())
        .with_header_builder(
            HeaderBuilder::default().header(chain1.get(8).unwrap().header().clone()),
        );

    let verify = UnclesVerifier::new(&block, &chain, &pow).verify();
    assert_eq!(verify, Ok(()));

    let uncle = BlockBuilder::default()
        .block(chain1.get(8).cloned().unwrap())
        .with_header_builder(
            HeaderBuilder::default().header(chain1.get(8).unwrap().header().clone()),
        );

    let block = BlockBuilder::default()
        .block(chain2.get(8).cloned().unwrap())
        .uncle(uncle.into())
        .with_header_builder(
            HeaderBuilder::default().header(chain2.get(8).unwrap().header().clone()),
        );
    let number = block.header().number();

    let verify = UnclesVerifier::new(&block, &chain, &pow).verify();
    assert_eq!(
        verify,
        Err(Error::Uncles(UnclesError::InvalidDepth {
            max: number - 1,
            min: number - 6,
            actual: number
        }))
    );

    let uncle = BlockBuilder::default()
        .block(chain1.get(10).cloned().unwrap())
        .with_header_builder(
            HeaderBuilder::default().header(chain1.get(10).unwrap().header().clone()),
        );
    let uncle_number = uncle.header().number();

    let block = BlockBuilder::default()
        .block(chain2.last().cloned().unwrap())
        .uncle(uncle.into())
        .with_header_builder(
            HeaderBuilder::default().header(chain2.last().unwrap().header().clone()),
        );
    let number = block.header().number();

    let verify = UnclesVerifier::new(&block, &chain, &pow).verify();
    assert_eq!(
        verify,
        Err(Error::Uncles(UnclesError::InvalidDepth {
            max: number - 1,
            min: number - 6,
            actual: uncle_number
        }))
    );

    let uncle = BlockBuilder::default()
        .block(chain1.get(10).cloned().unwrap())
        .with_header_builder(
            HeaderBuilder::default().header(chain1.get(10).unwrap().header().clone()),
        );

    let block = BlockBuilder::default()
        .block(chain2.get(12).cloned().unwrap())
        .uncle(uncle.into())
        .with_header_builder(
            HeaderBuilder::default().header(chain2.get(12).unwrap().header().clone()),
        );

    let verify = UnclesVerifier::new(&block, &chain, &pow).verify();
    assert_eq!(verify, Ok(()));

    let uncle1 = BlockBuilder::default()
        .block(chain1.get(10).cloned().unwrap())
        .with_header_builder(
            HeaderBuilder::default().header(chain1.get(10).unwrap().header().clone()),
        );

    let uncle2 = BlockBuilder::default()
        .block(chain1.get(10).cloned().unwrap())
        .with_header_builder(
            HeaderBuilder::default().header(chain1.get(10).unwrap().header().clone()),
        );

    let block = BlockBuilder::default()
        .block(chain2.get(12).cloned().unwrap())
        .uncle(uncle1.into())
        .uncle(uncle2.into())
        .with_header_builder(
            HeaderBuilder::default().header(chain2.get(12).unwrap().header().clone()),
        );

    let verify = UnclesVerifier::new(&block, &chain, &pow).verify();
    // uncle duplicate
    assert_eq!(
        verify,
        Err(Error::Uncles(UnclesError::Duplicate(
            block.uncles()[1].header().hash()
        )))
    );

    let max_uncles_len = chain.consensus().max_uncles_len();
    let mut uncles = Vec::new();
    for _ in 0..max_uncles_len + 1 {
        let uncle = BlockBuilder::default()
            .block(chain1.get(10).cloned().unwrap())
            .with_header_builder(
                HeaderBuilder::default().header(chain1.get(10).unwrap().header().clone()),
            );
        uncles.push(uncle.into());
    }

    let block = BlockBuilder::default()
        .block(chain2.get(12).cloned().unwrap())
        .uncles(uncles)
        .with_header_builder(
            HeaderBuilder::default().header(chain2.get(12).unwrap().header().clone()),
        );
    let verify = UnclesVerifier::new(&block, &chain, &pow).verify();
    // uncle overlength
    assert_eq!(
        verify,
        Err(Error::Uncles(UnclesError::OverLength {
            max: max_uncles_len,
            actual: max_uncles_len + 1
        }))
    );
}
