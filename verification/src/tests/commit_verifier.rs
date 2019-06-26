use super::super::contextual_block_verifier::CommitVerifier;
use super::super::error::{CommitError, Error};
use ckb_chain::chain::{ChainController, ChainService};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::header::{Header, HeaderBuilder};
use ckb_core::script::Script;
use ckb_core::transaction::{
    CellInput, CellOutput, OutPoint, ProposalShortId, Transaction, TransactionBuilder,
};
use ckb_core::uncle::UncleBlock;
use ckb_core::{capacity_bytes, BlockNumber, Bytes, Capacity};
use ckb_db::memorydb::MemoryKeyValueDB;
use ckb_notify::NotifyService;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_store::ChainKVStore;
use ckb_store::ChainStore;
use ckb_test_chain_utils::create_always_success_cell;
use ckb_traits::ChainProvider;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::sync::Arc;

fn gen_block(
    parent_header: &Header,
    transactions: Vec<Transaction>,
    proposals: Vec<ProposalShortId>,
    uncles: Vec<UncleBlock>,
) -> Block {
    let now = 1 + parent_header.timestamp();
    let number = parent_header.number() + 1;
    let nonce = parent_header.nonce() + 1;
    let difficulty = parent_header.difficulty() + U256::from(1u64);
    let cellbase = create_cellbase(number);
    let header_builder = HeaderBuilder::default()
        .parent_hash(parent_header.hash().to_owned())
        .timestamp(now)
        .number(number)
        .difficulty(difficulty)
        .nonce(nonce);

    BlockBuilder::default()
        .transaction(cellbase)
        .transactions(transactions)
        .proposals(proposals)
        .uncles(uncles)
        .header_builder(header_builder)
        .build()
}

fn create_transaction(
    parent: &H256,
    always_success_script: &Script,
    always_success_out_point: &OutPoint,
) -> Transaction {
    let capacity = 100_000_000 / 100 as usize;
    let output = CellOutput::new(
        Capacity::bytes(capacity).unwrap(),
        Bytes::default(),
        always_success_script.to_owned(),
        Some(always_success_script.to_owned()),
    );
    let inputs: Vec<CellInput> = (0..100)
        .map(|index| CellInput::new(OutPoint::new_cell(parent.clone(), index), 0))
        .collect();

    TransactionBuilder::default()
        .inputs(inputs)
        .outputs(vec![output; 100])
        .dep(always_success_out_point.to_owned())
        .build()
}

fn start_chain(
    consensus: Option<Consensus>,
) -> (ChainController, Shared<ChainKVStore<MemoryKeyValueDB>>) {
    let mut builder = SharedBuilder::<MemoryKeyValueDB>::new();
    if let Some(consensus) = consensus {
        builder = builder.consensus(consensus);
    }
    let shared = builder.build().unwrap();

    let notify = NotifyService::default().start::<&str>(None);
    let chain_service = ChainService::new(shared.clone(), notify);
    let chain_controller = chain_service.start::<&str>(None);
    (chain_controller, shared)
}

fn create_cellbase(number: BlockNumber) -> Transaction {
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .outputs(vec![CellOutput::new(
            Capacity::zero(),
            Bytes::default(),
            Script::default(),
            None,
        )])
        .build()
}

fn setup_env() -> (
    ChainController,
    Shared<ChainKVStore<MemoryKeyValueDB>>,
    H256,
    Script,
    OutPoint,
) {
    let (always_success_cell, always_success_script) = create_always_success_cell();
    let tx = TransactionBuilder::default()
        .witness(always_success_script.clone().into_witness())
        .input(CellInput::new(OutPoint::null(), 0))
        .output(always_success_cell.clone())
        .outputs(vec![
            CellOutput::new(
                capacity_bytes!(1_000_000),
                Bytes::default(),
                always_success_script.clone(),
                Some(always_success_script.clone()),
            );
            100
        ])
        .build();
    let tx_hash = tx.hash().to_owned();
    let genesis_block = BlockBuilder::default().transaction(tx).build();
    let consensus = Consensus::default().set_genesis_block(genesis_block);
    let (chain_controller, shared) = start_chain(Some(consensus));
    (
        chain_controller,
        shared,
        tx_hash.to_owned(),
        always_success_script.clone(),
        OutPoint::new_cell(tx_hash, 0),
    )
}

#[test]
fn test_proposal() {
    let (
        chain_controller,
        shared,
        mut prev_tx_hash,
        always_success_script,
        always_success_out_point,
    ) = setup_env();

    let mut txs20 = Vec::new();
    for _ in 0..20 {
        let tx = create_transaction(
            &prev_tx_hash,
            &always_success_script,
            &always_success_out_point,
        );
        txs20.push(tx.clone());
        prev_tx_hash = tx.hash().to_owned();
    }

    let proposal_window = shared.consensus().tx_proposal_window();

    let mut parent = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();

    //proposal in block(1)
    let proposed = 1;
    let proposal_ids: Vec<_> = txs20.iter().map(Transaction::proposal_short_id).collect();
    let block: Block = gen_block(&parent, vec![], proposal_ids, vec![]);
    chain_controller
        .process_block(Arc::new(block.clone()), false)
        .unwrap();
    parent = block.header().to_owned();

    //commit in proposal gap is invalid
    for _ in (proposed + 1)..(proposed + proposal_window.closest()) {
        let block: Block = gen_block(&parent, txs20.clone(), vec![], vec![]);
        assert_eq!(
            CommitVerifier::new(&shared, &block).verify(),
            Err(Error::Commit(CommitError::Invalid))
        );

        //test chain forward
        let new_block: Block = gen_block(&parent, vec![], vec![], vec![]);
        chain_controller
            .process_block(Arc::new(new_block.clone()), false)
            .unwrap();
        parent = new_block.header().to_owned();
    }

    //commit in proposal window
    for _ in 0..(proposal_window.farthest() - proposal_window.closest()) {
        let block: Block = gen_block(&parent, txs20.clone(), vec![], vec![]);
        let verifier = CommitVerifier::new(&shared, &block);
        assert_eq!(verifier.verify(), Ok(()));

        //test chain forward
        let new_block: Block = gen_block(&parent, vec![], vec![], vec![]);
        chain_controller
            .process_block(Arc::new(new_block.clone()), false)
            .unwrap();
        parent = new_block.header().to_owned();
    }

    //proposal expired
    let block: Block = gen_block(&parent, txs20.clone(), vec![], vec![]);
    let verifier = CommitVerifier::new(&shared, &block);
    assert_eq!(verifier.verify(), Ok(()));
}

#[test]
fn test_uncle_proposal() {
    let (
        chain_controller,
        shared,
        mut prev_tx_hash,
        always_success_script,
        always_success_out_point,
    ) = setup_env();

    let mut txs20 = Vec::new();
    for _ in 0..20 {
        let tx = create_transaction(
            &prev_tx_hash,
            &always_success_script,
            &always_success_out_point,
        );
        txs20.push(tx.clone());
        prev_tx_hash = tx.hash().to_owned();
    }

    let proposal_window = shared.consensus().tx_proposal_window();

    let mut parent = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();

    //proposal in block(1)
    let proposed = 1;
    let proposal_ids: Vec<_> = txs20.iter().map(Transaction::proposal_short_id).collect();
    let uncle: Block = gen_block(&parent, vec![], proposal_ids, vec![]);
    let block: Block = gen_block(&parent, vec![], vec![], vec![uncle.into()]);
    chain_controller
        .process_block(Arc::new(block.clone()), false)
        .unwrap();
    parent = block.header().to_owned();

    //commit in proposal gap is invalid
    for _ in (proposed + 1)..(proposed + proposal_window.closest()) {
        let block: Block = gen_block(&parent, txs20.clone(), vec![], vec![]);
        let verifier = CommitVerifier::new(&shared, &block);
        assert_eq!(verifier.verify(), Err(Error::Commit(CommitError::Invalid)));

        //test chain forward
        let new_block: Block = gen_block(&parent, vec![], vec![], vec![]);
        chain_controller
            .process_block(Arc::new(new_block.clone()), false)
            .unwrap();
        parent = new_block.header().to_owned();
    }

    //commit in proposal window
    for _ in 0..(proposal_window.farthest() - proposal_window.closest()) {
        let block: Block = gen_block(&parent, txs20.clone(), vec![], vec![]);
        let verifier = CommitVerifier::new(&shared, &block);
        assert_eq!(verifier.verify(), Ok(()));

        //test chain forward
        let new_block: Block = gen_block(&parent, vec![], vec![], vec![]);
        chain_controller
            .process_block(Arc::new(new_block.clone()), false)
            .unwrap();
        parent = new_block.header().to_owned();
    }

    //proposal expired
    let block: Block = gen_block(&parent, txs20.clone(), vec![], vec![]);
    let verifier = CommitVerifier::new(&shared, &block);
    assert_eq!(verifier.verify(), Ok(()));
}
