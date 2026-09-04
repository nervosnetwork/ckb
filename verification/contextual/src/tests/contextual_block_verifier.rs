use super::super::contextual_block_verifier::{EpochVerifier, TwoPhaseCommitVerifier};
use crate::contextual_block_verifier::{RewardVerifier, VerifyContext};
use ckb_chain::ChainServiceScope;
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder, ProposalWindow};
use ckb_error::assert_error_eq;
use ckb_shared::{Shared, SharedBuilder};
use ckb_store::data_loader_wrapper::AsDataLoader;
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
use ckb_verification::{
    CellbaseError, CommitError, ContextualTransactionVerifier, EpochError, TxVerifyEnv,
    cache::{ScriptVerificationRules, TxVerificationCacheKey},
};
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

fn create_cache_test_transaction(
    parent: &Byte32,
    always_success_script: &Script,
    always_success_out_point: &OutPoint,
) -> TransactionView {
    TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new(parent.clone(), 1), 0))
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(999_000).unwrap())
                .lock(always_success_script.clone())
                .type_(Some(always_success_script.clone()))
                .build(),
        )
        .output_data(Bytes::new().pack())
        .cell_dep(
            CellDep::new_builder()
                .out_point(always_success_out_point.clone())
                .build(),
        )
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
        .witness(Bytes::new().pack())
        .build()
}

fn setup_env_with_proposal_window(
    proposal_window: Option<ProposalWindow>,
) -> (ChainServiceScope, Shared, Byte32, Script, OutPoint) {
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
    let mut consensus = ConsensusBuilder::default().genesis_block(genesis_block);
    if let Some(proposal_window) = proposal_window {
        consensus = consensus.tx_proposal_window(proposal_window);
    }
    let consensus = consensus.build();
    let (chain, shared) = start_chain(Some(consensus));
    (
        chain,
        shared,
        tx_hash.to_owned(),
        always_success_script.clone(),
        OutPoint::new(tx_hash, 0),
    )
}

fn setup_env() -> (ChainServiceScope, Shared, Byte32, Script, OutPoint) {
    setup_env_with_proposal_window(None)
}

#[test]
fn disabled_script_verification_does_not_publish_cache_proof() {
    let (chain, shared, parent_tx, always_success_script, always_success_out_point) = setup_env();
    let tx = create_cache_test_transaction(
        &parent_tx,
        &always_success_script,
        &always_success_out_point,
    );
    let parent = shared.consensus().genesis_block().header();
    let block = gen_block(&parent, vec![tx.clone()], vec![], vec![]);
    let rules = ScriptVerificationRules::from_env(
        shared.consensus(),
        &TxVerifyEnv::new_commit(&block.header()),
    );
    let key = TxVerificationCacheKey::from_transaction(&tx, rules);
    chain
        .chain_controller()
        .blocking_process_block_with_switch(Arc::new(block), Switch::DISABLE_ALL)
        .unwrap();

    // Cache publication is asynchronous. Give a wrongly scheduled writer a
    // chance to run before asserting the negative security property.
    let cache = shared.txs_verify_cache();
    let cached = shared.async_handle().block_on(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cache.read().await.lookup(&key)
    });
    assert!(
        cached.is_none(),
        "assume-valid script skipping is not reusable verification proof"
    );
}

#[test]
fn contextual_verification_does_not_reuse_cache_across_script_rules() {
    let (chain, shared, parent_tx, always_success_script, always_success_out_point) = setup_env();
    let tx = create_cache_test_transaction(
        &parent_tx,
        &always_success_script,
        &always_success_out_point,
    );
    let parent = shared.consensus().genesis_block().header();
    let block = gen_block(&parent, vec![tx.clone()], vec![], vec![]);
    let current_rules = ScriptVerificationRules::from_env(
        shared.consensus(),
        &TxVerifyEnv::new_commit(&block.header()),
    );
    let stale_consensus = Arc::new(
        ConsensusBuilder::new(
            shared.consensus().genesis_block().clone(),
            shared.consensus().genesis_epoch_ext().clone(),
        )
        .hardfork_switch(ckb_types::core::hardfork::HardForks::new_dev())
        .build(),
    );
    let stale_env = Arc::new(TxVerifyEnv::new_commit(&block.header()));
    let stale_rules = ScriptVerificationRules::from_env(&stale_consensus, &stale_env);
    assert_ne!(stale_rules, current_rules);
    let mut seen_inputs = std::collections::HashSet::new();
    let snapshot = shared.cloned_snapshot();
    let resolved = Arc::new(
        ckb_types::core::cell::resolve_transaction(
            tx,
            &mut seen_inputs,
            snapshot.as_ref(),
            snapshot.as_ref(),
        )
        .expect("the cache fixture resolves against genesis"),
    );
    let stale_outcome = ContextualTransactionVerifier::new(
        resolved,
        stale_consensus,
        Arc::new(shared.store().clone()).as_data_loader(),
        stale_env,
    )
    .verify_scripts(shared.consensus().max_block_cycles(), None)
    .expect("stale-rule proof must originate in a real VM success");
    let stale_proof = stale_outcome
        .executed_proof()
        .expect("a cache miss produces publishable proof");
    assert_eq!(stale_proof.key().script_rules(), stale_rules);
    let cache = shared.txs_verify_cache();
    shared.async_handle().block_on(async {
        cache.write().await.insert(stale_proof);
    });

    chain
        .chain_controller()
        .blocking_process_block_with_switch(Arc::new(block), Switch::ONLY_SCRIPT)
        .expect("mismatched cache evidence must be ignored and scripts reverified");
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

fn assert_two_phase_window(propose_in_uncle: bool) {
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

    // Propose in block 1 through exactly one of the two consensus routes.
    let proposed = 1;
    let proposal_ids: Vec<_> = txs20
        .iter()
        .map(|tx| tx.data().proposal_short_id())
        .collect();
    let block = if propose_in_uncle {
        let uncle = gen_block(&parent, vec![], proposal_ids, vec![]);
        gen_block(&parent, vec![], vec![], vec![uncle.as_uncle()])
    } else {
        gen_block(&parent, vec![], proposal_ids, vec![])
    };
    chain_controller
        .blocking_process_block_with_switch(Arc::new(block.clone()), Switch::DISABLE_ALL)
        .unwrap();
    parent = block.header();

    let context = dummy_context(&shared);

    // Every candidate before the closest distance is in Gap and invalid.
    for _ in (proposed + 1)..(proposed + proposal_window.closest()) {
        let block = gen_block(&parent, txs20.clone(), vec![], vec![]);
        assert_error_eq!(
            TwoPhaseCommitVerifier::new(&context, &block)
                .verify()
                .unwrap_err(),
            CommitError::Invalid,
        );

        let new_block = gen_block(&parent, vec![], vec![], vec![]);
        chain_controller
            .blocking_process_block_with_switch(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
            .unwrap();
        parent = new_block.header().to_owned();
    }

    // The complete inclusive commit window is legal, including farthest.
    for _ in proposal_window.closest()..=proposal_window.farthest() {
        let block = gen_block(&parent, txs20.clone(), vec![], vec![]);
        let verifier = TwoPhaseCommitVerifier::new(&context, &block);
        assert!(verifier.verify().is_ok());

        let new_block = gen_block(&parent, vec![], vec![], vec![]);
        chain_controller
            .blocking_process_block_with_switch(Arc::new(new_block.clone()), Switch::DISABLE_ALL)
            .unwrap();
        parent = new_block.header().to_owned();
    }

    // The first candidate past farthest is Outside and invalid.
    let block = gen_block(&parent, txs20, vec![], vec![]);
    assert_error_eq!(
        TwoPhaseCommitVerifier::new(&context, &block)
            .verify()
            .unwrap_err(),
        CommitError::Invalid,
    );
}

#[test]
fn test_proposal() {
    assert_two_phase_window(false);
}

#[test]
fn test_uncle_proposal() {
    assert_two_phase_window(true);
}

#[test]
fn two_phase_commit_verifier_and_live_proposal_view_agree_pointwise() {
    let windows = [
        ProposalWindow(2, 10),
        ProposalWindow(1, 10),
        ProposalWindow(3, 10),
        ProposalWindow(1, 1),
        ProposalWindow(3, 3),
        ProposalWindow(2, 2),
    ];

    for proposal_window in windows {
        for propose_in_uncle in [false, true] {
            let (chain, shared, parent_tx, always_success_script, always_success_out_point) =
                setup_env_with_proposal_window(Some(proposal_window));
            let proposed = create_transaction(
                &parent_tx,
                &always_success_script,
                &always_success_out_point,
            );
            let extra = create_transaction(
                &proposed.data().calc_tx_hash(),
                &always_success_script,
                &always_success_out_point,
            );
            let proposed_id = proposed.proposal_short_id();
            let extra_id = extra.proposal_short_id();
            let controller = chain.chain_controller();
            let mut parent = shared.consensus().genesis_block().header();

            let proposal_block = if propose_in_uncle {
                let uncle = gen_block(&parent, vec![], vec![proposed_id.clone()], vec![]);
                gen_block(&parent, vec![], vec![], vec![uncle.as_uncle()])
            } else {
                gen_block(&parent, vec![], vec![proposed_id.clone()], vec![])
            };
            controller
                .blocking_process_block_with_switch(
                    Arc::new(proposal_block.clone()),
                    Switch::DISABLE_ALL,
                )
                .expect("the proposal block installs");
            parent = proposal_block.header();

            for tip in 1..=proposal_window.farthest() + 1 {
                let snapshot = shared.snapshot();
                let view = snapshot.proposals();
                let proposed_visible = view.contains_proposed(&proposed_id);
                let extra_visible = view.contains_proposed(&extra_id);
                let context = dummy_context(&shared);
                let proposed_block = gen_block(&parent, vec![proposed.clone()], vec![], vec![]);
                let extra_block = gen_block(&parent, vec![extra.clone()], vec![], vec![]);
                let combined_block = gen_block(
                    &parent,
                    vec![proposed.clone(), extra.clone()],
                    vec![],
                    vec![],
                );

                assert_eq!(
                    TwoPhaseCommitVerifier::new(&context, &proposed_block)
                        .verify()
                        .is_ok(),
                    proposed_visible,
                    "singleton proposed id disagrees with the live view: window=({},{}) tip={tip} uncle={propose_in_uncle}",
                    proposal_window.closest(),
                    proposal_window.farthest(),
                );
                assert_eq!(
                    TwoPhaseCommitVerifier::new(&context, &extra_block)
                        .verify()
                        .is_ok(),
                    extra_visible,
                    "singleton extra id disagrees with the live view: window=({},{}) tip={tip} uncle={propose_in_uncle}",
                    proposal_window.closest(),
                    proposal_window.farthest(),
                );
                assert_eq!(
                    TwoPhaseCommitVerifier::new(&context, &combined_block)
                        .verify()
                        .is_ok(),
                    proposed_visible && extra_visible,
                    "the verifier must reject a set containing any id outside the live view: window=({},{}) tip={tip} uncle={propose_in_uncle}",
                    proposal_window.closest(),
                    proposal_window.farthest(),
                );
                assert!(
                    !extra_visible,
                    "the never-proposed control id must remain outside the live view"
                );

                if tip <= proposal_window.farthest() {
                    let next = gen_block(&parent, vec![], vec![], vec![]);
                    controller
                        .blocking_process_block_with_switch(
                            Arc::new(next.clone()),
                            Switch::DISABLE_ALL,
                        )
                        .expect("the canonical empty successor installs");
                    parent = next.header();
                }
            }
        }
    }
}

#[test]
fn two_phase_commit_verifier_and_live_proposal_view_agree_after_reorg() {
    let proposal_window = ProposalWindow(2, 4);
    let (chain, shared, parent_tx, always_success_script, always_success_out_point) =
        setup_env_with_proposal_window(Some(proposal_window));
    let old = create_transaction(
        &parent_tx,
        &always_success_script,
        &always_success_out_point,
    );
    let new = create_transaction(
        &old.data().calc_tx_hash(),
        &always_success_script,
        &always_success_out_point,
    );
    let old_id = old.proposal_short_id();
    let new_id = new.proposal_short_id();
    let controller = chain.chain_controller();
    let genesis = shared.consensus().genesis_block().header();

    let old_proposal = gen_block(&genesis, vec![], vec![old_id.clone()], vec![]);
    controller
        .blocking_process_block_with_switch(Arc::new(old_proposal.clone()), Switch::DISABLE_ALL)
        .expect("the old proposal branch installs");
    let old_tip = gen_block(&old_proposal.header(), vec![], vec![], vec![]);
    controller
        .blocking_process_block_with_switch(Arc::new(old_tip.clone()), Switch::DISABLE_ALL)
        .expect("the old branch tip installs");

    {
        let snapshot = shared.snapshot();
        assert!(snapshot.proposals().contains_proposed(&old_id));
        assert!(!snapshot.proposals().contains_proposed(&new_id));
        let context = dummy_context(&shared);
        assert!(
            TwoPhaseCommitVerifier::new(
                &context,
                &gen_block(&old_tip.header(), vec![old.clone()], vec![], vec![]),
            )
            .verify()
            .is_ok()
        );
        assert_error_eq!(
            TwoPhaseCommitVerifier::new(
                &context,
                &gen_block(&old_tip.header(), vec![new.clone()], vec![], vec![]),
            )
            .verify()
            .expect_err("the other branch proposal is not current evidence"),
            CommitError::Invalid,
        );
    }

    let new_proposal = gen_block(&genesis, vec![], vec![new_id.clone()], vec![]);
    controller
        .blocking_process_block_with_switch(Arc::new(new_proposal.clone()), Switch::DISABLE_ALL)
        .expect("the competing proposal block is retained");
    let new_second = gen_block(&new_proposal.header(), vec![], vec![], vec![]);
    controller
        .blocking_process_block_with_switch(Arc::new(new_second.clone()), Switch::DISABLE_ALL)
        .expect("the competing branch reaches the old height");
    let new_tip = gen_block(&new_second.header(), vec![], vec![], vec![]);
    controller
        .blocking_process_block_with_switch(Arc::new(new_tip.clone()), Switch::DISABLE_ALL)
        .expect("the longer competing branch becomes canonical");

    let snapshot = shared.snapshot();
    assert_eq!(snapshot.tip_header().hash(), new_tip.hash());
    assert!(!snapshot.proposals().contains_proposed(&old_id));
    assert!(snapshot.proposals().contains_proposed(&new_id));
    let context = dummy_context(&shared);
    assert_error_eq!(
        TwoPhaseCommitVerifier::new(
            &context,
            &gen_block(&new_tip.header(), vec![old], vec![], vec![]),
        )
        .verify()
        .expect_err("detached proposal evidence cannot authorize a commit"),
        CommitError::Invalid,
    );
    assert!(
        TwoPhaseCommitVerifier::new(
            &context,
            &gen_block(&new_tip.header(), vec![new], vec![], vec![]),
        )
        .verify()
        .is_ok()
    );
}

#[test]
fn two_phase_commit_verifier_does_not_read_genesis_proposals() {
    let (always_success_cell, always_success_cell_data, always_success_script) =
        always_success_cell();
    let genesis_tx = TransactionBuilder::default()
        .witness(always_success_script.clone().into_witness())
        .input(CellInput::new(OutPoint::null(), 0))
        .output(always_success_cell.clone())
        .output_data(always_success_cell_data.to_owned())
        .build();
    let candidate = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new(genesis_tx.hash(), 0), 0))
        .build();
    let genesis = BlockBuilder::default()
        .transaction(genesis_tx)
        .proposal(candidate.proposal_short_id())
        .build();
    let consensus = ConsensusBuilder::default()
        .genesis_block(genesis)
        .tx_proposal_window(ckb_chain_spec::consensus::ProposalWindow(2, 4))
        .build();
    let (chain, shared) = start_chain(Some(consensus));
    let controller = chain.chain_controller();
    let mut parent = shared.consensus().genesis_block().header();
    for _ in 1..=2 {
        let block = gen_block(&parent, vec![], vec![], vec![]);
        controller
            .blocking_process_block_with_switch(Arc::new(block.clone()), Switch::DISABLE_ALL)
            .expect("the empty canonical prefix installs");
        parent = block.header();
    }

    assert!(
        !shared
            .snapshot()
            .proposals()
            .contains_proposed(&candidate.proposal_short_id()),
        "the live proposal view must exclude genesis-only proposal evidence"
    );

    let block = gen_block(&parent, vec![candidate], vec![], vec![]);
    assert_error_eq!(
        TwoPhaseCommitVerifier::new(&dummy_context(&shared), &block)
            .verify()
            .expect_err("a genesis-only proposal is not commit evidence"),
        CommitError::Invalid,
    );
}
