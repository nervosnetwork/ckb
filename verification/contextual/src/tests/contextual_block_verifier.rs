use super::super::contextual_block_verifier::{EpochVerifier, TwoPhaseCommitVerifier};
use crate::contextual_block_verifier::{RewardVerifier, VerifyContext};
use ckb_chain::ChainServiceScope;
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_error::assert_error_eq;
use ckb_shared::{Shared, SharedBuilder};
use ckb_store::{ChainDB, ChainStore};
use ckb_test_chain_utils::always_success_cell;
use ckb_types::{
    bytes::Bytes,
    core::{
        BlockBuilder, BlockNumber, BlockView, Capacity, EpochExt, EpochNumberWithFraction,
        HeaderBuilder, HeaderView, TransactionBuilder, TransactionView, UncleBlockView,
        capacity_bytes, cell::ResolvedTransaction,
    },
    packed::{
        Byte32, CellDep, CellInput, CellOutput, CellOutputBuilder, OutPoint, ProposalShortId,
        Script,
    },
    prelude::*,
    utilities::DIFF_TWO,
};
use ckb_verification::{CellbaseError, CommitError, EpochError};
use ckb_verification_traits::Switch;
use std::sync::Arc;

fn gen_block(
    parent_header: &HeaderView,
    transactions: Vec<TransactionView>,
    proposals: Vec<ProposalShortId>,
    uncles: Vec<UncleBlockView>,
) -> BlockView {
    let now = 1 + parent_header.timestamp();
    let number = parent_header.number() + 1;
    let nonce = parent_header.nonce() + 1;
    let compact_target = parent_header.compact_target() - 1;
    let cellbase = create_cellbase(number);
    let header = HeaderBuilder::default()
        .parent_hash(parent_header.hash())
        .timestamp(now)
        .number(number)
        .epoch(EpochNumberWithFraction::new(
            number / 1000,
            number % 1000,
            1000,
        ))
        .compact_target(compact_target)
        .nonce(nonce)
        .build();

    BlockBuilder::default()
        .transaction(cellbase)
        .transactions(transactions)
        .proposals(proposals)
        .uncles(uncles)
        .header(header)
        .build()
}

fn create_transaction(
    parent: &Byte32,
    always_success_script: &Script,
    always_success_out_point: &OutPoint,
) -> TransactionView {
    let capacity = 100_000_000 / 100_usize;
    let output = CellOutputBuilder::default()
        .capacity(Capacity::bytes(capacity).unwrap())
        .lock(always_success_script.to_owned())
        .type_(Some(always_success_script.to_owned()))
        .build();
    let inputs: Vec<CellInput> = (0..100)
        .map(|index| CellInput::new(OutPoint::new(parent.clone(), index), 0))
        .collect();
    let cell_dep = CellDep::new_builder()
        .out_point(always_success_out_point.to_owned())
        .build();

    TransactionBuilder::default()
        .inputs(inputs)
        .outputs(vec![output; 100])
        .outputs_data(vec![Bytes::new().into(); 100])
        .cell_dep(cell_dep)
        .build()
}

fn start_chain(consensus: Option<Consensus>) -> (ChainServiceScope, Shared) {
    let mut builder = SharedBuilder::with_temp_db();
    if let Some(consensus) = consensus {
        builder = builder.consensus(consensus);
    }
    let (shared, mut pack) = builder.build().unwrap();

    let chain = ChainServiceScope::new(pack.take_chain_services_builder());
    (chain, shared)
}

fn dummy_context(shared: &Shared) -> VerifyContext<ChainDB> {
    VerifyContext::new(Arc::new(shared.store().clone()), shared.cloned_consensus())
}

fn create_cellbase(number: BlockNumber) -> TransactionView {
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .output(CellOutputBuilder::default().build())
        .output_data(Bytes::new())
        .build()
}

fn setup_env() -> (ChainServiceScope, Shared, Byte32, Script, OutPoint) {
    let (always_success_cell, always_success_cell_data, always_success_script) =
        always_success_cell();
    let tx = TransactionBuilder::default()
        .witness(always_success_script.clone().into_witness())
        .input(CellInput::new(OutPoint::null(), 0))
        .output(always_success_cell.clone())
        .outputs(vec![
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(1_000_000))
                .lock(always_success_script.clone())
                .type_(Some(always_success_script.clone()))
                .build();
            100
        ])
        .output_data(always_success_cell_data.to_owned())
        .outputs_data(vec![Bytes::new().into(); 100])
        .build();
    let tx_hash = tx.data().calc_tx_hash();
    let genesis_block = BlockBuilder::default().transaction(tx).build();
    let consensus = ConsensusBuilder::default()
        .genesis_block(genesis_block)
        .build();
    let (chain, shared) = start_chain(Some(consensus));
    (
        chain,
        shared,
        tx_hash.to_owned(),
        always_success_script.clone(),
        OutPoint::new(tx_hash, 0),
    )
}

#[test]
pub fn test_should_have_no_output_in_cellbase_no_finalization_target() {
    let (_chain, shared) = start_chain(None);
    let context = dummy_context(&shared);

    let parent = shared.consensus().genesis_block().header();
    let number = parent.number() + 1;
    let cellbase = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .output(CellOutput::default())
        .output_data(Bytes::default())
        .build();

    let cellbase = ResolvedTransaction {
        transaction: cellbase,
        resolved_cell_deps: vec![],
        resolved_inputs: vec![],
        resolved_dep_groups: vec![],
    };

    let ret = RewardVerifier::new(&context, &[Arc::new(cellbase)], &parent).verify();

    assert_error_eq!(ret.unwrap_err(), CellbaseError::InvalidRewardTarget,);
}

#[test]
fn test_epoch_number() {
    let actual_epoch = EpochNumberWithFraction::new(2, 0, 1);
    let block = BlockBuilder::default().epoch(actual_epoch).build();
    let mut epoch = EpochExt::default();
    epoch.set_length(1);

    assert_error_eq!(
        EpochVerifier::new(&epoch, &block).verify().unwrap_err(),
        EpochError::NumberMismatch {
            expected: 1_099_511_627_776,
            actual: 1_099_511_627_778,
        },
    )
}

#[test]
fn test_epoch_difficulty() {
    let mut epoch = EpochExt::default();
    epoch.set_compact_target(DIFF_TWO);
    epoch.set_length(1);

    let block = BlockBuilder::default()
        .epoch(epoch.number_with_fraction(0))
        .compact_target(0x200c_30c3u32)
        .build();

    assert_error_eq!(
        EpochVerifier::new(&epoch, &block).verify().unwrap_err(),
        EpochError::TargetMismatch {
            expected: DIFF_TWO,
            actual: 0x200c_30c3u32,
        },
    );
}

#[test]
fn test_proposal() {
    let (chain, shared, mut prev_tx_hash, always_success_script, always_success_out_point) =
        setup_env();
    let chain_controller = chain.chain_controller();

    let mut txs20 = Vec::new();
    for _ in 0..20 {
        let tx = create_transaction(
            &prev_tx_hash,
            &always_success_script,
            &always_success_out_point,
        );
        txs20.push(tx.clone());
        prev_tx_hash = tx.data().calc_tx_hash();
    }

    let proposal_window = shared.consensus().tx_proposal_window();

    let mut parent = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();

    //proposal in block(1)
    let proposed = 1;
    let proposal_ids: Vec<_> = txs20
        .iter()
        .map(|tx| tx.data().proposal_short_id())
        .collect();
    let block = gen_block(&parent, vec![], proposal_ids, vec![]);
    chain_controller
        .blocking_process_block_with_switch(Arc::new(block.clone()), Switch::DISABLE_ALL)
        .unwrap();
    parent = block.header();

    let context = dummy_context(&shared);

    //commit in proposal gap is invalid
    for _ in (proposed + 1)..(proposed + proposal_window.closest()) {
        let block = gen_block(&parent, txs20.clone(), vec![], vec![]);
        assert_error_eq!(
            TwoPhaseCommitVerifier::new(&context, &block)
                .verify()
                .unwrap_err(),
            CommitError::Invalid,
        );

        //test chain forward
        let new_block = gen_block(&parent, vec![], vec![], vec![]);
        chain_controller
            .blocking_process_block_with_switch(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
            .unwrap();
        parent = new_block.header().to_owned();
    }

    //commit in proposal window
    for _ in 0..(proposal_window.farthest() - proposal_window.closest()) {
        let block = gen_block(&parent, txs20.clone(), vec![], vec![]);
        let verifier = TwoPhaseCommitVerifier::new(&context, &block);
        assert!(verifier.verify().is_ok());

        //test chain forward
        let new_block = gen_block(&parent, vec![], vec![], vec![]);
        chain_controller
            .blocking_process_block_with_switch(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
            .unwrap();
        parent = new_block.header().to_owned();
    }

    //proposal expired
    let block = gen_block(&parent, txs20, vec![], vec![]);
    let verifier = TwoPhaseCommitVerifier::new(&context, &block);
    assert!(verifier.verify().is_ok());
}

#[test]
fn test_uncle_proposal() {
    let (chain, shared, mut prev_tx_hash, always_success_script, always_success_out_point) =
        setup_env();
    let chain_controller = chain.chain_controller();

    let mut txs20 = Vec::new();
    for _ in 0..20 {
        let tx = create_transaction(
            &prev_tx_hash,
            &always_success_script,
            &always_success_out_point,
        );
        txs20.push(tx.clone());
        prev_tx_hash = tx.data().calc_tx_hash();
    }

    let proposal_window = shared.consensus().tx_proposal_window();

    let mut parent = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();

    //proposal in block(1)
    let proposed = 1;
    let proposal_ids: Vec<_> = txs20
        .iter()
        .map(|tx| tx.data().proposal_short_id())
        .collect();
    let uncle = gen_block(&parent, vec![], proposal_ids, vec![]);
    let block = gen_block(&parent, vec![], vec![], vec![uncle.as_uncle()]);
    chain_controller
        .blocking_process_block_with_switch(Arc::new(block.clone()), Switch::DISABLE_ALL)
        .unwrap();
    parent = block.header();

    let context = dummy_context(&shared);

    //commit in proposal gap is invalid
    for _ in (proposed + 1)..(proposed + proposal_window.closest()) {
        let block = gen_block(&parent, txs20.clone(), vec![], vec![]);
        let verifier = TwoPhaseCommitVerifier::new(&context, &block);
        assert_error_eq!(verifier.verify().unwrap_err(), CommitError::Invalid);

        //test chain forward
        let new_block = gen_block(&parent, vec![], vec![], vec![]);
        chain_controller
            .blocking_process_block_with_switch(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
            .unwrap();
        parent = new_block.header().to_owned();
    }

    //commit in proposal window
    for _ in 0..(proposal_window.farthest() - proposal_window.closest()) {
        let block = gen_block(&parent, txs20.clone(), vec![], vec![]);
        let verifier = TwoPhaseCommitVerifier::new(&context, &block);
        assert!(verifier.verify().is_ok());

        //test chain forward
        let new_block = gen_block(&parent, vec![], vec![], vec![]);
        chain_controller
            .blocking_process_block_with_switch(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
            .unwrap();
        parent = new_block.header().to_owned();
    }

    //proposal expired
    let block = gen_block(&parent, txs20, vec![], vec![]);
    let verifier = TwoPhaseCommitVerifier::new(&context, &block);
    assert!(verifier.verify().is_ok());
}
