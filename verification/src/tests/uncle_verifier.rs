use super::super::block_verifier::UnclesVerifier;
use super::super::error::{Error, UnclesError};
use ckb_chain::chain::{ChainBuilder, ChainController};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::header::{Header, HeaderBuilder};
use ckb_core::transaction::{
    CellInput, CellOutput, ProposalShortId, Transaction, TransactionBuilder,
};
use ckb_core::BlockNumber;
use ckb_db::memorydb::MemoryKeyValueDB;
use ckb_shared::shared::{ChainProvider, Shared, SharedBuilder};
use ckb_shared::store::ChainKVStore;
#[cfg(not(disable_faketime))]
use faketime;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::sync::Arc;

fn gen_block(parent_header: &Header, nonce: u64, difficulty: U256) -> Block {
    let now = 1 + parent_header.timestamp();
    let number = parent_header.number() + 1;
    let cellbase = create_cellbase(number);
    let header_builder = HeaderBuilder::default()
        .parent_hash(parent_header.hash().clone())
        .timestamp(now)
        .number(number)
        .difficulty(difficulty)
        .cellbase_id(cellbase.hash().clone())
        .nonce(nonce);

    BlockBuilder::default()
        .commit_transaction(cellbase)
        .proposal_transaction(ProposalShortId::from_slice(&[1; 10]).unwrap())
        .with_header_builder(header_builder)
}

fn start_chain(
    consensus: Option<Consensus>,
) -> (ChainController, Shared<ChainKVStore<MemoryKeyValueDB>>) {
    let mut builder = SharedBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory();
    if let Some(consensus) = consensus {
        builder = builder.consensus(consensus);
    }
    let shared = builder.build();

    let (chain_controller, chain_receivers) = ChainController::build();
    let chain_service = ChainBuilder::new(shared.clone()).build();
    let _handle = chain_service.start::<&str>(None, chain_receivers);
    (chain_controller, shared)
}

fn create_cellbase(number: BlockNumber) -> Transaction {
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .output(CellOutput::new(0, vec![], H256::zero(), None))
        .build()
}

#[cfg(not(disable_faketime))]
#[test]
fn test_uncle_verifier() {
    let faketime_file = faketime::millis_tempfile(0).expect("create faketime file");
    faketime::enable(&faketime_file);

    let mut consensus = Consensus::default();
    consensus.pow_time_span = 10;
    consensus.pow_spacing = 1;

    let (chain_controller, shared) = start_chain(Some(consensus));

    assert_eq!(shared.consensus().difficulty_adjustment_interval(), 10);
    let number = 20;
    let mut chain1: Vec<Block> = Vec::new();
    let mut chain2: Vec<Block> = Vec::new();

    faketime::write_millis(&faketime_file, 10).expect("write millis");

    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    for i in 1..number {
        let difficulty = shared.calculate_difficulty(&parent).unwrap();
        let new_block = gen_block(&parent, i, difficulty);
        chain_controller
            .process_block(Arc::new(new_block.clone()))
            .expect("process block ok");
        chain1.push(new_block.clone());
        parent = new_block.header().clone();
    }

    parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();

    // if block_number < 11 { chain1 == chain2 } else { chain1 != chain2 }
    for i in 1..number {
        let difficulty = shared.calculate_difficulty(&parent).unwrap();
        let new_block = gen_block(&parent, i + 1000, difficulty);
        chain_controller
            .process_block(Arc::new(new_block.clone()))
            .expect("process block ok");
        chain2.push(new_block.clone());
        parent = new_block.header().clone();
    }

    let verifier = UnclesVerifier::new(shared.clone());

    let block = BlockBuilder::default()
        .block(chain1.last().cloned().unwrap())
        .uncle(chain2.last().cloned().unwrap().into())
        .build();

    // Uncles not match uncles_count
    assert_eq!(
        verifier.verify(&block),
        Err(Error::Uncles(UnclesError::MissMatchCount {
            expected: 0,
            actual: 1
        }))
    );

    let verifier = UnclesVerifier::new(shared.clone());

    let block = BlockBuilder::default()
        .block(chain1.last().cloned().unwrap())
        .header(HeaderBuilder::default().uncles_count(1).build())
        .uncle(chain2.last().cloned().unwrap().into())
        .build();
    // Uncles not match uncles_hash
    assert_eq!(
        verifier.verify(&block),
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
    // Uncle depth is invalid
    assert_eq!(
        verifier.verify(&block),
        Err(Error::Uncles(UnclesError::InvalidDepth {
            max: block.header().number() - 1,
            min: block.header().number() - shared.consensus().max_uncles_age() as u64,
            actual: 19
        }))
    );

    let block = BlockBuilder::default()
        .block(chain1[8].clone())
        .uncle(chain2[6].clone().into())
        .with_header_builder(
            HeaderBuilder::default()
                .header(chain1[8].header().clone())
                .difficulty(U256::from(2u64)),
        );
    assert_eq!(
        verifier.verify(&block),
        Err(Error::Uncles(UnclesError::InvalidDifficulty))
    );

    let block = BlockBuilder::default()
        .block(chain1.get(9).cloned().unwrap())          // block.number 10 epoch 1
        .uncle(chain2.get(6).cloned().unwrap().into())   // block.number 7 epoch 0
        .with_header_builder(HeaderBuilder::default().header(
            chain1[9].header().clone()
        ));
    assert_eq!(
        verifier.verify(&block),
        Err(Error::Uncles(UnclesError::InvalidDifficultyEpoch))
    );

    let uncle = BlockBuilder::default()
        .block(chain2.get(6).cloned().unwrap())
        .proposal_transaction(ProposalShortId::from_slice(&[1; 10]).unwrap())
        .build();
    let block = BlockBuilder::default()
        .block(chain1.get(8).cloned().unwrap())
        .uncle(uncle.into())
        .with_header_builder(HeaderBuilder::default().header(chain1[8].header().clone()));
    assert_eq!(
        verifier.verify(&block),
        Err(Error::Uncles(UnclesError::ProposalTransactionsRoot))
    );

    let uncle = BlockBuilder::default()
        .block(chain2.get(6).cloned().unwrap())
        .proposal_transaction(ProposalShortId::from_slice(&[1; 10]).unwrap())
        .with_header_builder(HeaderBuilder::default().header(chain2[6].header().clone()));
    let block = BlockBuilder::default()
        .block(chain1.get(8).cloned().unwrap())
        .uncle(uncle.into())
        .with_header_builder(HeaderBuilder::default().header(chain1[8].header().clone()));
    assert_eq!(
        verifier.verify(&block),
        Err(Error::Uncles(UnclesError::ProposalTransactionDuplicate))
    );

    let uncle = BlockBuilder::default()
        .block(chain2.get(6).cloned().unwrap())
        .with_header_builder(HeaderBuilder::default().header(chain2[6].header().clone()));
    let block = BlockBuilder::default()
        .block(chain1.get(8).cloned().unwrap())
        .uncle(uncle.into())
        .with_header_builder(HeaderBuilder::default().header(chain1[8].header().clone()));
    assert_eq!(verifier.verify(&block), Ok(()));

    let uncle = BlockBuilder::default()
        .block(chain1.get(8).cloned().unwrap())
        .with_header_builder(HeaderBuilder::default().header(chain1[8].header().clone()));
    let block = BlockBuilder::default()
        .block(chain2.get(8).cloned().unwrap())
        .uncle(uncle.into())
        .with_header_builder(HeaderBuilder::default().header(chain2[8].header().clone()));
    let number = block.header().number();
    assert_eq!(
        verifier.verify(&block),
        Err(Error::Uncles(UnclesError::InvalidDepth {
            max: number - 1,
            min: number - 6,
            actual: number
        }))
    );

    let uncle = BlockBuilder::default()
        .block(chain1[10].clone())
        .with_header_builder(HeaderBuilder::default().header(chain1[10].header().clone()));
    let uncle_number = uncle.header().number();
    let block = BlockBuilder::default()
        .block(chain2.last().cloned().unwrap())
        .uncle(uncle.into())
        .with_header_builder(
            HeaderBuilder::default().header(chain2.last().unwrap().header().clone()),
        );
    let number = block.header().number();
    assert_eq!(
        verifier.verify(&block),
        Err(Error::Uncles(UnclesError::InvalidDepth {
            max: number - 1,
            min: number - 6,
            actual: uncle_number
        }))
    );

    let uncle = BlockBuilder::default()
        .block(chain1[10].clone())
        .with_header_builder(HeaderBuilder::default().header(chain1[10].header().clone()));
    let block = BlockBuilder::default()
        .block(chain2.get(12).cloned().unwrap())
        .uncle(uncle.into())
        .with_header_builder(HeaderBuilder::default().header(chain2[12].header().clone()));
    assert_eq!(verifier.verify(&block), Ok(()));

    let uncle1 = BlockBuilder::default()
        .block(chain1.get(10).cloned().unwrap())
        .with_header_builder(HeaderBuilder::default().header(chain1[10].header().clone()));
    let uncle2 = BlockBuilder::default()
        .block(chain1.get(10).cloned().unwrap())
        .with_header_builder(HeaderBuilder::default().header(chain1[10].header().clone()));
    let block = BlockBuilder::default()
        .block(chain2.get(12).cloned().unwrap())
        .uncle(uncle1.into())
        .uncle(uncle2.into())
        .with_header_builder(HeaderBuilder::default().header(chain2[12].header().clone()));
    // uncle duplicate
    assert_eq!(
        verifier.verify(&block),
        Err(Error::Uncles(UnclesError::Duplicate(
            block.uncles()[1].header().hash().clone()
        )))
    );

    let max_uncles_len = shared.consensus().max_uncles_len();
    let mut uncles = Vec::new();
    for _ in 0..=max_uncles_len {
        let uncle = BlockBuilder::default()
            .block(chain1.get(10).cloned().unwrap())
            .with_header_builder(HeaderBuilder::default().header(chain1[10].header().clone()));
        uncles.push(uncle.into());
    }
    let block = BlockBuilder::default()
        .block(chain2.get(12).cloned().unwrap())
        .uncles(uncles)
        .with_header_builder(HeaderBuilder::default().header(chain2[12].header().clone()));
    // uncle overlength
    assert_eq!(
        verifier.verify(&block),
        Err(Error::Uncles(UnclesError::OverCount {
            max: max_uncles_len,
            actual: max_uncles_len + 1
        }))
    );
}
