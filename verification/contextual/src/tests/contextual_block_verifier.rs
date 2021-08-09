use super::super::contextual_block_verifier::{EpochVerifier, TwoPhaseCommitVerifier};
use crate::contextual_block_verifier::{RewardVerifier, VerifyContext};
use ckb_chain::chain::{ChainController, ChainService};
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_error::assert_error_eq;
use ckb_launcher::SharedBuilder;
use ckb_shared::Shared;
use ckb_store::{ChainDB, ChainStore};
use ckb_test_chain_utils::always_success_cell;
use ckb_types::{
    bytes::Bytes,
    core::{
        capacity_bytes, cell::ResolvedTransaction, BlockBuilder, BlockNumber, BlockView, Capacity,
        EpochExt, HeaderBuilder, HeaderView, TransactionBuilder, TransactionView, UncleBlockView,
    },
    packed::{Byte32, CellDep, CellInput, CellOutputBuilder, OutPoint, ProposalShortId, Script},
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
        .timestamp(now.pack())
        .number(number.pack())
        .compact_target(compact_target.pack())
        .nonce(nonce.pack())
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
        .capacity(Capacity::bytes(capacity).unwrap().pack())
        .lock(always_success_script.to_owned())
        .type_(Some(always_success_script.to_owned()).pack())
        .build();
    let inputs: Vec<CellInput> = (0..100)
        .map(|index| CellInput::new(OutPoint::new(parent.clone(), index), 0))
        .collect();
    let cell_dep = CellDep::new_builder()
        .out_point(always_success_out_point.to_owned())
        .build();

    TransactionBuilder::default()
        .inputs(inputs.pack())
        .outputs(vec![output; 100].pack())
        .outputs_data(vec![Bytes::new(); 100].pack())
        .cell_dep(cell_dep)
        .build()
}

fn start_chain(consensus: Option<Consensus>) -> (ChainController, Shared) {
    let mut builder = SharedBuilder::with_temp_db();
    if let Some(consensus) = consensus {
        builder = builder.consensus(consensus);
    }
    let (shared, mut pack) = builder.build().unwrap();

    let chain_service = ChainService::new(shared.clone(), pack.take_proposal_table());
    let chain_controller = chain_service.start::<&str>(None);
    (chain_controller, shared)
}

fn dummy_context(shared: &Shared) -> VerifyContext<'_, ChainDB> {
    VerifyContext::new(shared.store(), shared.consensus())
}

fn create_cellbase(number: BlockNumber) -> TransactionView {
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .output(CellOutputBuilder::default().build())
        .output_data(Bytes::new().pack())
        .build()
}

fn setup_env() -> (ChainController, Shared, Byte32, Script, OutPoint) {
    let (always_success_cell, always_success_cell_data, always_success_script) =
        always_success_cell();
    let tx = TransactionBuilder::default()
        .witness(always_success_script.clone().into_witness())
        .input(CellInput::new(OutPoint::null(), 0))
        .output(always_success_cell.clone())
        .outputs(vec![
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(1_000_000).pack())
                .lock(always_success_script.clone())
                .type_(Some(always_success_script.clone()).pack())
                .build();
            100
        ])
        .output_data(always_success_cell_data.to_owned().pack())
        .outputs_data(vec![Bytes::new().pack(); 100])
        .build();
    let tx_hash = tx.data().calc_tx_hash();
    let genesis_block = BlockBuilder::default().transaction(tx).build();
    let consensus = ConsensusBuilder::default()
        .genesis_block(genesis_block)
        .build();
    let (chain_controller, shared) = start_chain(Some(consensus));
    (
        chain_controller,
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
        .output(Default::default())
        .output_data(Default::default())
        .build();

    let cellbase = ResolvedTransaction {
        transaction: cellbase,
        resolved_cell_deps: vec![],
        resolved_inputs: vec![],
        resolved_dep_groups: vec![],
    };

    let ret = RewardVerifier::new(&context, &[cellbase], &parent).verify();

    assert_error_eq!(ret.unwrap_err(), CellbaseError::InvalidRewardTarget,);
}

#[test]
fn test_epoch_number() {
    let block = BlockBuilder::default().epoch(2u64.pack()).build();
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
        .epoch(epoch.number_with_fraction(0).pack())
        .compact_target(0x200c_30c3u32.pack())
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
        .internal_process_block(Arc::new(block.clone()), Switch::DISABLE_ALL)
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
            .internal_process_block(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
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
            .internal_process_block(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
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
        .internal_process_block(Arc::new(block.clone()), Switch::DISABLE_ALL)
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
            .internal_process_block(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
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
            .internal_process_block(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
            .unwrap();
        parent = new_block.header().to_owned();
    }

    //proposal expired
    let block = gen_block(&parent, txs20, vec![], vec![]);
    let verifier = TwoPhaseCommitVerifier::new(&context, &block);
    assert!(verifier.verify().is_ok());
}
