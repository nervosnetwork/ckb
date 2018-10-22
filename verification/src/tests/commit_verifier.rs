use super::super::block_verifier::CommitVerifier;
use super::super::error::{CommitError, Error};
use bigint::{H256, U256};
use chain::chain::{ChainBuilder, ChainController};
use ckb_shared::consensus::Consensus;
use ckb_shared::shared::{ChainProvider, Shared, SharedBuilder};
use ckb_shared::store::ChainKVStore;
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

fn start_chain(
    consensus: Option<Consensus>,
) -> (ChainController, Shared<ChainKVStore<MemoryKeyValueDB>>) {
    let mut builder = SharedBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory();
    if let Some(consensus) = consensus {
        builder = builder.consensus(consensus);
    }
    let shared = builder.build();

    let (chain_controller, chain_receivers) = ChainController::new();
    let chain_service = ChainBuilder::new(shared.clone()).build();
    let _handle = chain_service.start::<&str>(None, chain_receivers);
    (chain_controller, shared)
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
    let (chain_controller, shared) = start_chain(Some(consensus));

    let mut txs = FnvHashMap::default();
    let end = 21;

    let mut blocks: Vec<Block> = Vec::new();
    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
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
        assert!(
            chain_controller
                .process_block(Arc::new(block.clone()))
                .is_ok()
        );
    }

    let verifier = CommitVerifier::new(shared.clone(), shared.consensus().clone());
    assert_eq!(
        verifier.verify(&blocks[10]),
        Err(Error::Commit(CommitError::Invalid))
    );
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
    let (chain_controller, shared) = start_chain(Some(consensus));

    let mut txs = Vec::new();

    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();

    for _ in 0..20 {
        let tx = create_transaction(root_hash);
        txs.push(tx.clone());
        root_hash = tx.hash();
    }

    let proposal_ids: Vec<_> = txs.iter().map(|tx| tx.proposal_short_id()).collect();
    let uncle = gen_block(parent.clone(), vec![], proposal_ids, vec![]).into();
    let block = gen_block(parent.clone(), vec![], vec![], vec![uncle]);

    assert!(
        chain_controller
            .process_block(Arc::new(block.clone()))
            .is_ok()
    );

    parent = block.header().clone();

    let new_block = gen_block(parent, txs, vec![], vec![]);
    let verifier = CommitVerifier::new(shared.clone(), shared.consensus().clone());
    assert_eq!(verifier.verify(&new_block), Ok(()));
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
    let (chain_controller, shared) = start_chain(Some(consensus));

    let mut txs = Vec::new();

    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();

    for _ in 0..20 {
        let tx = create_transaction(root_hash);
        txs.push(tx.clone());
        root_hash = tx.hash();
    }

    let proposal_ids: Vec<_> = txs.iter().map(|tx| tx.proposal_short_id()).collect();
    let block = gen_block(parent.clone(), vec![], proposal_ids, vec![]);

    assert!(
        chain_controller
            .process_block(Arc::new(block.clone()))
            .is_ok()
    );

    parent = block.header().clone();

    let new_block = gen_block(parent, txs, vec![], vec![]);
    let verifier = CommitVerifier::new(shared.clone(), shared.consensus().clone());
    assert_eq!(verifier.verify(&new_block), Ok(()));
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
    let (chain_controller, shared) = start_chain(Some(consensus));

    let mut txs = Vec::new();

    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();

    for _ in 0..20 {
        let tx = create_transaction(root_hash);
        txs.push(tx.clone());
        root_hash = tx.hash();
    }

    let proposal_ids: Vec<_> = txs.iter().map(|tx| tx.proposal_short_id()).collect();
    let block = gen_block(parent.clone(), vec![], proposal_ids, vec![]);
    assert!(
        chain_controller
            .process_block(Arc::new(block.clone()))
            .is_ok()
    );
    parent = block.header().clone();

    let timeout = shared.consensus().transaction_propagation_timeout;

    for _ in 0..timeout - 1 {
        let block = gen_block(parent, vec![], vec![], vec![]);
        assert!(
            chain_controller
                .process_block(Arc::new(block.clone()))
                .is_ok()
        );
        parent = block.header().clone();
    }

    let verifier = CommitVerifier::new(shared.clone(), shared.consensus().clone());

    let new_block = gen_block(parent.clone(), txs.clone(), vec![], vec![]);
    assert_eq!(verifier.verify(&new_block), Ok(()));

    let block = gen_block(parent, vec![], vec![], vec![]);
    assert!(
        chain_controller
            .process_block(Arc::new(block.clone()))
            .is_ok()
    );
    parent = block.header().clone();

    let new_block = gen_block(parent, txs, vec![], vec![]);
    assert_eq!(
        verifier.verify(&new_block),
        Err(Error::Commit(CommitError::Invalid))
    );
}
