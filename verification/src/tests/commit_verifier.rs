use super::super::block_verifier::CommitVerifier;
use super::super::error::{CommitError, Error};
use ckb_chain::chain::{ChainBuilder, ChainController};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::header::{Header, HeaderBuilder};
use ckb_core::script::Script;
use ckb_core::transaction::{
    CellInput, CellOutput, OutPoint, ProposalShortId, Transaction, TransactionBuilder,
};
use ckb_core::uncle::UncleBlock;
use ckb_core::BlockNumber;
use ckb_db::memorydb::MemoryKeyValueDB;
use ckb_shared::shared::{ChainProvider, Shared, SharedBuilder};
use ckb_shared::store::ChainKVStore;
use fnv::FnvHashMap;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::fs::File;
use std::io::Read;
use std::path::Path;
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
    let difficulty = parent_header.difficulty() + U256::from(1u64);
    let cellbase = create_cellbase(number);
    let header_builder = HeaderBuilder::default()
        .parent_hash(parent_header.hash().clone())
        .timestamp(now)
        .number(number)
        .difficulty(difficulty)
        .nonce(nonce);

    BlockBuilder::default()
        .commit_transaction(cellbase)
        .commit_transactions(commit_transactions)
        .proposal_transactions(proposal_transactions)
        .uncles(uncles)
        .with_header_builder(header_builder)
}

fn get_script() -> Script {
    let mut file = File::open(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../nodes_template/spec/cells/always_success"),
    )
    .unwrap();
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).unwrap();

    Script::new(0, Vec::new(), None, Some(buffer), Vec::new(), 0)
}

fn create_transaction(parent: H256) -> Transaction {
    let script = get_script();
    let capacity = 100_000_000 / 100 as u64;
    let output = CellOutput::new(
        capacity,
        Vec::new(),
        script.type_hash(),
        Some(script.clone()),
    );
    let inputs: Vec<CellInput> = (0..100)
        .map(|index| CellInput::new(OutPoint::new(parent.clone(), index), script.clone()))
        .collect();

    TransactionBuilder::default()
        .inputs(inputs)
        .outputs(vec![output; 100])
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

    let (chain_controller, chain_receivers) = ChainController::build();
    let chain_service = ChainBuilder::new(shared.clone()).build();
    let _handle = chain_service.start::<&str>(None, chain_receivers);
    (chain_controller, shared)
}

fn create_cellbase(number: BlockNumber) -> Transaction {
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .outputs(vec![CellOutput::new(0, vec![], H256::zero(), None)])
        .build()
}

#[test]
fn test_blank_proposal() {
    let script = get_script();
    let tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), Default::default()))
        .outputs(vec![
            CellOutput::new(
                1_000_000,
                Vec::new(),
                script.type_hash(),
                Some(script),
            );
            100
        ])
        .build();
    let mut root_hash = tx.hash().clone();
    let genesis_block = BlockBuilder::default().commit_transaction(tx).build();
    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let (chain_controller, shared) = start_chain(Some(consensus));

    let mut txs = FnvHashMap::default();

    let mut blocks: Vec<Block> = Vec::new();
    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
    let mut prev_txs = Vec::new();
    for i in 1..6 {
        txs.insert(i, Vec::new());
        let tx = create_transaction(root_hash);
        root_hash = tx.hash().clone();
        txs.get_mut(&i).unwrap().push(tx.clone());
        let next_proposal_ids = vec![tx.proposal_short_id()];
        let new_block = gen_block(parent, prev_txs, next_proposal_ids, vec![]);
        prev_txs = vec![tx];
        blocks.push(new_block.clone());
        parent = new_block.header().clone();
    }

    for block in &blocks[0..5] {
        let result = chain_controller.process_block(Arc::new(block.clone()));
        if result.is_err() {
            println!("number: {}, result: {:?}", block.header().number(), result);
        }
        assert!(result.is_ok());
    }

    let blank_proposal_block = gen_block(parent, prev_txs, Vec::new(), Vec::new());
    parent = blank_proposal_block.header().clone();
    let result = chain_controller.process_block(Arc::new(blank_proposal_block.clone()));
    if result.is_err() {
        println!(
            "[blank proposal] number: {}, result: {:?}",
            blank_proposal_block.header().number(),
            result
        );
    }
    assert!(result.is_ok());

    let tx = create_transaction(root_hash);
    let invalid_block = gen_block(parent, vec![tx], Vec::new(), Vec::new());
    let verifier = CommitVerifier::new(shared.clone());
    assert_eq!(
        verifier.verify(&invalid_block),
        Err(Error::Commit(CommitError::Invalid))
    );
}

#[test]
fn test_uncle_proposal() {
    let script = get_script();
    let tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), Default::default()))
        .outputs(vec![
            CellOutput::new(
                1_000_000,
                Vec::new(),
                script.type_hash(),
                Some(script),
            );
            100
        ])
        .build();
    let mut root_hash = tx.hash().clone();
    let genesis_block = BlockBuilder::default().commit_transaction(tx).build();
    let consensus = Consensus::default()
        .set_genesis_block(genesis_block)
        .set_verification(false);
    let (chain_controller, shared) = start_chain(Some(consensus));

    let mut txs = Vec::new();

    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();

    for _ in 0..20 {
        let tx = create_transaction(root_hash);
        txs.push(tx.clone());
        root_hash = tx.hash().clone();
    }

    let proposal_ids: Vec<_> = txs.iter().map(|tx| tx.proposal_short_id()).collect();
    let uncle: Block = gen_block(parent.clone(), vec![], proposal_ids, vec![]).into();
    let result = chain_controller.process_block(Arc::new(uncle.clone()));
    if result.is_err() {
        println!(
            "[uncle] number: {}, result: {:?}",
            uncle.header().number(),
            result
        );
    }
    assert!(result.is_ok());

    let block1 = gen_block(parent.clone(), vec![], vec![], vec![]);
    parent = block1.header().clone();
    let result = chain_controller.process_block(Arc::new(block1.clone()));
    if result.is_err() {
        println!(
            "[block1] number: {}, result: {:?}",
            block1.header().number(),
            result
        );
    }
    assert!(result.is_ok());

    let block2 = gen_block(parent.clone(), vec![], vec![], vec![uncle.into()]);
    let result = chain_controller.process_block(Arc::new(block2.clone()));
    if result.is_err() {
        println!(
            "[block2] number: {}, result: {:?}",
            block2.header().number(),
            result
        );
    }
    assert!(result.is_ok());
    parent = block2.header().clone();

    let new_block = gen_block(parent, txs, vec![], vec![]);
    let verifier = CommitVerifier::new(shared.clone());
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
        ])
        .build();
    let mut root_hash = tx.hash().clone();
    let genesis_block = BlockBuilder::default().commit_transaction(tx).build();
    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let (chain_controller, shared) = start_chain(Some(consensus));

    let mut txs = Vec::new();

    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();

    for _ in 0..20 {
        let tx = create_transaction(root_hash);
        txs.push(tx.clone());
        root_hash = tx.hash().clone();
    }

    let proposal_ids: Vec<_> = txs.iter().map(|tx| tx.proposal_short_id()).collect();
    let block = gen_block(parent.clone(), vec![], proposal_ids, vec![]);

    assert!(chain_controller
        .process_block(Arc::new(block.clone()))
        .is_ok());

    parent = block.header().clone();

    let new_block = gen_block(parent, txs, vec![], vec![]);
    let verifier = CommitVerifier::new(shared.clone());
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
        ])
        .build();
    let mut root_hash = tx.hash().clone();
    let genesis_block = BlockBuilder::default().commit_transaction(tx).build();
    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let (chain_controller, shared) = start_chain(Some(consensus));

    let mut txs = Vec::new();

    let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();

    for _ in 0..20 {
        let tx = create_transaction(root_hash);
        txs.push(tx.clone());
        root_hash = tx.hash().clone();
    }

    let proposal_ids: Vec<_> = txs.iter().map(|tx| tx.proposal_short_id()).collect();
    let block = gen_block(parent.clone(), vec![], proposal_ids, vec![]);
    assert!(chain_controller
        .process_block(Arc::new(block.clone()))
        .is_ok());
    parent = block.header().clone();

    let timeout = shared.consensus().transaction_propagation_timeout;

    for _ in 0..timeout - 1 {
        let block = gen_block(parent, vec![], vec![], vec![]);
        assert!(chain_controller
            .process_block(Arc::new(block.clone()))
            .is_ok());
        parent = block.header().clone();
    }

    let verifier = CommitVerifier::new(shared.clone());

    let new_block = gen_block(parent.clone(), txs.clone(), vec![], vec![]);
    assert_eq!(verifier.verify(&new_block), Ok(()));

    let block = gen_block(parent, vec![], vec![], vec![]);
    assert!(chain_controller
        .process_block(Arc::new(block.clone()))
        .is_ok());
    parent = block.header().clone();

    let new_block = gen_block(parent, txs, vec![], vec![]);
    assert_eq!(
        verifier.verify(&new_block),
        Err(Error::Commit(CommitError::Invalid))
    );
}
