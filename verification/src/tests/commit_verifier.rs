use super::super::block_verifier::CommitVerifier;
use super::super::error::{CommitError, Error};
use bigint::{H256, U256};
use chain::chain::{ChainBuilder, ChainProvider};
use chain::consensus::Consensus;
use chain::store::ChainKVStore;
use core::block::{Block, BlockBuilder};
use core::header::{Header, HeaderBuilder};
use core::transaction::{
    CellInput, CellOutput, OutPoint, ProposalShortId, Transaction, TransactionBuilder,
};
use core::uncle::UncleBlock;
use core::BlockNumber;
use db::memorydb::MemoryKeyValueDB;
use fnv::FnvHashMap;
use std::sync::Arc;

fn gen_block(
    parent_header: Header,
    commit_transactions: Vec<Transaction>,
    proposal_transactions: Vec<ProposalShortId>,
    uncles: Vec<UncleBlock>,
) -> Block {
    let now = 1 + parent_header.timestamp();
    let number = parent_header.number() + 1;
    let nonce = parent_header.nonce() + 1;
    let difficulty = parent_header.difficulty() + U256::from(1);
    let cellbase = create_cellbase(number);
    let header_builder = HeaderBuilder::default()
        .parent_hash(&parent_header.hash())
        .timestamp(now)
        .number(number)
        .difficulty(&difficulty)
        .nonce(nonce);

    BlockBuilder::default()
        .commit_transaction(cellbase)
        .commit_transactions(commit_transactions)
        .proposal_transactions(proposal_transactions)
        .uncles(uncles)
        .with_header_builder(header_builder)
}

fn create_transaction(parent: H256) -> Transaction {
    let mut output = CellOutput::default();
    output.capacity = 100_000_000 / 100 as u64;

    TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new(parent, 0), Default::default()))
        .outputs(vec![output.clone(); 100])
        .build()
}

fn create_cellbase(number: BlockNumber) -> Transaction {
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .outputs(vec![CellOutput::new(0, vec![], H256::from(0), None)])
        .build()
}

#[test]
fn test_blank_proposal() {
    let tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), Default::default()))
        .outputs(vec![
            CellOutput::new(
                100_000_000,
                Vec::new(),
                H256::default(),
                None,
            );
            100
        ]).build();
    let mut root_hash = tx.hash();
    let genesis_block = BlockBuilder::default().commit_transaction(tx).build();
    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let chain = Arc::new(
        ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .consensus(consensus)
            .build()
            .unwrap(),
    );

    let mut txs = FnvHashMap::default();
    let end = 21;

    let mut blocks: Vec<Block> = Vec::new();
    let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
    for i in 1..end {
        txs.insert(i, Vec::new());
        let tx = create_transaction(root_hash);
        root_hash = tx.hash();
        txs.get_mut(&i).unwrap().push(tx.clone());
        let new_block = gen_block(parent, vec![tx], vec![], vec![]);
        blocks.push(new_block.clone());
        parent = new_block.header().clone();
    }

    for block in &blocks[0..10] {
        assert!(chain.process_block(&block).is_ok());
    }

    let verify = CommitVerifier::new(&blocks[10], Arc::clone(&chain)).verify();

    assert_eq!(verify, Err(Error::Commit(CommitError::Invalid)));
}

#[test]
fn test_uncle_proposal() {
    let tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), Default::default()))
        .outputs(vec![
            CellOutput::new(
                100_000_000,
                Vec::new(),
                H256::default(),
                None,
            );
            100
        ]).build();
    let mut root_hash = tx.hash();
    let genesis_block = BlockBuilder::default().commit_transaction(tx).build();
    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let chain = Arc::new(
        ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .consensus(consensus)
            .build()
            .unwrap(),
    );

    let mut txs = Vec::new();

    let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();

    for _ in 0..20 {
        let tx = create_transaction(root_hash);
        txs.push(tx.clone());
        root_hash = tx.hash();
    }

    let proposal_ids: Vec<_> = txs.iter().map(|tx| tx.proposal_short_id()).collect();
    let uncle = gen_block(parent.clone(), vec![], proposal_ids, vec![]).into();
    let block = gen_block(parent.clone(), vec![], vec![], vec![uncle]);

    assert!(chain.process_block(&block).is_ok());

    parent = block.header().clone();

    let new_block = gen_block(parent, txs, vec![], vec![]);

    let verify = CommitVerifier::new(&new_block, Arc::clone(&chain)).verify();

    assert_eq!(verify, Ok(()));
}

#[test]
fn test_block_proposal() {
    let tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), Default::default()))
        .outputs(vec![
            CellOutput::new(
                100_000_000,
                Vec::new(),
                H256::default(),
                None,
            );
            100
        ]).build();
    let mut root_hash = tx.hash();
    let genesis_block = BlockBuilder::default().commit_transaction(tx).build();
    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let chain = Arc::new(
        ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .consensus(consensus)
            .build()
            .unwrap(),
    );

    let mut txs = Vec::new();

    let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();

    for _ in 0..20 {
        let tx = create_transaction(root_hash);
        txs.push(tx.clone());
        root_hash = tx.hash();
    }

    let proposal_ids: Vec<_> = txs.iter().map(|tx| tx.proposal_short_id()).collect();
    let block = gen_block(parent.clone(), vec![], proposal_ids, vec![]);

    assert!(chain.process_block(&block).is_ok());

    parent = block.header().clone();

    let new_block = gen_block(parent, txs, vec![], vec![]);

    let verify = CommitVerifier::new(&new_block, Arc::clone(&chain)).verify();

    assert_eq!(verify, Ok(()));
}

#[test]
fn test_proposal_timeout() {
    let tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), Default::default()))
        .outputs(vec![
            CellOutput::new(
                100_000_000,
                Vec::new(),
                H256::default(),
                None,
            );
            100
        ]).build();
    let mut root_hash = tx.hash();
    let genesis_block = BlockBuilder::default().commit_transaction(tx).build();
    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let chain = Arc::new(
        ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .consensus(consensus)
            .build()
            .unwrap(),
    );

    let mut txs = Vec::new();

    let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();

    for _ in 0..20 {
        let tx = create_transaction(root_hash);
        txs.push(tx.clone());
        root_hash = tx.hash();
    }

    let proposal_ids: Vec<_> = txs.iter().map(|tx| tx.proposal_short_id()).collect();
    let block = gen_block(parent.clone(), vec![], proposal_ids, vec![]);
    assert!(chain.process_block(&block).is_ok());
    parent = block.header().clone();

    let timeout = chain.consensus().transaction_propagation_timeout;

    for _ in 0..timeout - 1 {
        let block = gen_block(parent, vec![], vec![], vec![]);
        assert!(chain.process_block(&block).is_ok());
        parent = block.header().clone();
    }

    let new_block = gen_block(parent.clone(), txs.clone(), vec![], vec![]);
    let verify = CommitVerifier::new(&new_block, Arc::clone(&chain)).verify();

    assert_eq!(verify, Ok(()));

    let block = gen_block(parent, vec![], vec![], vec![]);
    assert!(chain.process_block(&block).is_ok());
    parent = block.header().clone();

    let new_block = gen_block(parent, txs, vec![], vec![]);
    let verify = CommitVerifier::new(&new_block, Arc::clone(&chain)).verify();

    assert_eq!(verify, Err(Error::Commit(CommitError::Invalid)));
}
