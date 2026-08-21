use super::super::{
    state::{AcceptedStatus, ValidatedAdmission},
    template::TemplateComponent,
    template_driver::{
        AuthorityBlockAssembler, AuthorityTemplateReadFailure, AuthorityTemplateStep,
    },
};
use super::foundation::{
    accept_remote_transaction, accept_remote_transaction_with_payload, apply_plan, owner_version,
    resolved_payload_with_facts, runtime_config, tx,
};
use crate::block_assembler::{BlockAssembler, BoundedCandidateUncle, ResetEpoch};
use ckb_app_config::BlockAssemblerConfig;
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_jsonrpc_types::ScriptHashType;
use ckb_network::PeerIndex;
use ckb_snapshot::Snapshot;
use ckb_stop_handler::CancellationToken;
use ckb_store::{ChainStore, attach_block_cell};
use ckb_test_chain_utils::MockStore;
use ckb_types::{U256, core::BlockExt, h256, utilities::merkle_mountain_range::ChainRootMMR};
use ckb_types::{
    core::{BlockBuilder, Capacity, TransactionBuilder},
    packed::{Byte32, CellInput, OutPoint, ProposalShortId},
    prelude::Entity,
};
use std::{sync::Arc, time::Duration};

fn template_config() -> BlockAssemblerConfig {
    BlockAssemblerConfig {
        code_hash: h256!("0x0"),
        args: Default::default(),
        hash_type: ScriptHashType::Data,
        message: Default::default(),
        use_binary_version_as_message_prefix: true,
        binary_version: "TEST".to_string(),
        update_interval_millis: 60_000,
        notify: vec![],
        notify_scripts: vec![],
        notify_timeout_millis: 800,
        notify_auth_token: None,
    }
}

fn template_snapshot() -> Arc<Snapshot> {
    template_snapshot_with_child(None)
}

fn template_snapshot_with_child(child_timestamp: Option<u64>) -> Arc<Snapshot> {
    let consensus = Arc::new(ConsensusBuilder::default().build());
    let store = MockStore::default();
    let genesis = consensus.genesis_block();
    let epoch_ext = consensus.genesis_epoch_ext().clone();
    {
        let db_txn = store.store().begin_transaction();
        let previous_epoch_hash = epoch_ext.last_block_hash_in_previous_epoch();
        db_txn
            .insert_block(genesis)
            .expect("the fixture stores genesis");
        db_txn
            .attach_block(genesis)
            .expect("the fixture attaches genesis");
        attach_block_cell(&db_txn, genesis).expect("the fixture stores genesis cells");
        db_txn
            .insert_block_epoch_index(&genesis.hash(), &previous_epoch_hash)
            .expect("the fixture stores the epoch index");
        db_txn
            .insert_epoch_ext(&previous_epoch_hash, &epoch_ext)
            .expect("the fixture stores the epoch extension");
        db_txn
            .insert_block_ext(
                &genesis.hash(),
                &BlockExt {
                    received_at: 0,
                    total_difficulty: U256::zero(),
                    total_uncles_count: 0,
                    verified: Some(true),
                    txs_fees: vec![],
                    cycles: None,
                    txs_sizes: None,
                },
            )
            .expect("the fixture stores the block extension");
        let mut mmr = ChainRootMMR::new(0, &db_txn);
        mmr.push(genesis.digest())
            .expect("the fixture appends the genesis digest");
        mmr.commit().expect("the fixture commits the chain root");
        db_txn.commit().expect("the fixture commits genesis");
    }
    let child = child_timestamp.map(|timestamp| {
        BlockBuilder::default()
            .number(1)
            .parent_hash(genesis.hash())
            .timestamp(timestamp)
            .compact_target(genesis.compact_target())
            .epoch(epoch_ext.number_with_fraction(1))
            .dao(genesis.dao())
            .build()
    });
    if let Some(child) = &child {
        store.insert_block(child, &epoch_ext);
        let db_txn = store.store().begin_transaction();
        // Genesis occupies the first MMR node; append the height-one tip.
        let mut mmr = ChainRootMMR::new(1, &db_txn);
        mmr.push(child.digest())
            .expect("the fixture appends the child digest");
        mmr.commit().expect("the fixture commits the child root");
        db_txn.commit().expect("the fixture commits the child MMR");
        for position in 0..3 {
            assert!(
                store.store().get_header_digest(position).is_some(),
                "the fixture stores child MMR position {position}"
            );
        }
    }
    let tip = child.map_or_else(|| genesis.header(), |child| child.header());
    Arc::new(Snapshot::new(
        tip,
        U256::zero(),
        epoch_ext,
        store.store().get_snapshot(),
        Default::default(),
        consensus,
    ))
}

fn set_status(
    runtime: &super::super::runtime::AuthorityRuntime,
    hash: &super::super::state::RawTxHash,
    status: AcceptedStatus,
) {
    runtime.with_authority_for_foundation(|authority| {
        let version = owner_version(authority, hash);
        apply_plan(
            authority
                .plan_status_for_foundation(hash, version, status)
                .expect("the fixture status transition plans"),
        );
    });
}

fn candidate_uncle(
    snapshot: &Snapshot,
    timestamp: u64,
    proposals: Vec<ProposalShortId>,
) -> BoundedCandidateUncle {
    let uncle = BlockBuilder::default()
        .number(snapshot.tip_number())
        .parent_hash(snapshot.tip_hash())
        .timestamp(timestamp)
        .compact_target(snapshot.tip_header().compact_target())
        .epoch(snapshot.tip_header().epoch())
        .proposals(proposals)
        .build()
        .as_uncle();
    BoundedCandidateUncle::try_new(uncle, usize::MAX).expect("candidate-uncle fixture is bounded")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_template_failure_wait_ignores_unrelated_authority_mutation() {
    let snapshot = template_snapshot();
    let runtime = super::super::runtime::AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the authority runtime fixture is valid");
    let assembler = BlockAssembler::new(template_config(), Arc::clone(&snapshot))
        .expect("the block assembler fixture is valid");
    let driver = AuthorityBlockAssembler::new(runtime.clone(), assembler)
        .await
        .expect("the authority template adapter is valid");
    let failed = driver.retry_source_cut_for_foundation().await;
    let cancel = CancellationToken::new();
    let wait_driver = driver.clone();
    let wait_cancel = cancel.clone();
    let mut waiter = tokio::spawn(async move {
        wait_driver
            .wait_template_source_change_for_foundation(&wait_cancel, failed)
            .await
    });
    tokio::task::yield_now().await;

    runtime
        .queue_generation_reset_for_foundation()
        .expect("an effect-only Apply publishes an unrelated authority wake");
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut waiter)
            .await
            .is_err(),
        "an unchanged template source must not repeat a failed build"
    );

    driver
        .receive_candidate_uncle(candidate_uncle(&snapshot, 1, Vec::new()))
        .expect("a candidate source advance is typed")
        .then_some(())
        .expect("the new candidate advances the exact retry source");
    assert!(
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("the relevant source change wakes the retry level")
            .expect("the source waiter joins")
    );
    cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_proposal_failure_wait_uses_the_minimum_component_source_cut() {
    let snapshot = template_snapshot();
    let runtime = super::super::runtime::AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the authority runtime fixture is valid");
    let hash = runtime.with_authority_for_foundation(|authority| {
        accept_remote_transaction(authority, tx(1_907), 907, AcceptedStatus::Gap, Vec::new())
    });
    let assembler = BlockAssembler::new(template_config(), snapshot)
        .expect("the block assembler fixture is valid");
    let driver = AuthorityBlockAssembler::new(runtime.clone(), assembler)
        .await
        .expect("the authority template adapter is valid");
    let failed = driver
        .component_retry_source_for_foundation(TemplateComponent::Proposals)
        .await;
    let cancel = CancellationToken::new();
    let wait_driver = driver.clone();
    let wait_cancel = cancel.clone();
    let mut waiter = tokio::spawn(async move {
        wait_driver
            .wait_template_source_change_for_foundation(&wait_cancel, failed)
            .await
    });
    tokio::task::yield_now().await;

    runtime.set_accepted_status_for_foundation(&hash, AcceptedStatus::Proposed);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut waiter)
            .await
            .is_err(),
        "a transaction-only source advance must not repeat failed proposal work"
    );

    runtime.set_accepted_status_for_foundation(&hash, AcceptedStatus::Pending);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("the proposal source change wakes the retry level")
            .expect("the source waiter joins")
    );
    cancel.cancel();
}

#[tokio::test]
async fn uak_template_source_probe_skips_irrelevant_population_captures() {
    let snapshot = template_snapshot();
    let runtime = super::super::runtime::AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the authority runtime fixture is valid");
    let assembler = BlockAssembler::new(template_config(), Arc::clone(&snapshot))
        .expect("the block assembler fixture is valid");
    let driver = AuthorityBlockAssembler::new(runtime.clone(), assembler)
        .await
        .expect("the authority template adapter is valid");

    assert_eq!(
        driver
            .drive_replacement_once()
            .await
            .expect("the initial full level builds"),
        AuthorityTemplateStep::Published
    );
    let converged_captures = runtime.template_capture_count_for_foundation();
    for component in [
        TemplateComponent::Proposals,
        TemplateComponent::Transactions,
        TemplateComponent::Uncles,
    ] {
        assert_eq!(
            driver
                .drive_component_once(component)
                .await
                .expect("the covered component probes without capture"),
            AuthorityTemplateStep::Idle
        );
    }
    assert_eq!(
        driver
            .drive_replacement_once()
            .await
            .expect("the covered replacement probes without capture"),
        AuthorityTemplateStep::Idle
    );
    assert_eq!(
        runtime.template_capture_count_for_foundation(),
        converged_captures
    );

    runtime
        .admit(
            ValidatedAdmission::remote(tx(1_906), PeerIndex::from(906))
                .expect("the hostile-work fixture is a valid Remote admission"),
        )
        .expect("PreAccepted ownership commits and publishes its generic wake");
    for component in [
        TemplateComponent::Proposals,
        TemplateComponent::Transactions,
        TemplateComponent::Uncles,
    ] {
        assert_eq!(
            driver
                .drive_component_once(component)
                .await
                .expect("PreAccepted-only movement is template-irrelevant"),
            AuthorityTemplateStep::Idle
        );
    }
    assert_eq!(
        driver
            .drive_replacement_once()
            .await
            .expect("PreAccepted-only movement cannot require full replacement"),
        AuthorityTemplateStep::Idle
    );
    assert_eq!(
        runtime.template_capture_count_for_foundation(),
        converged_captures,
        "a generic PreAccepted wake must perform zero accepted-pool captures"
    );

    assert!(
        driver
            .receive_candidate_uncle(candidate_uncle(&snapshot, 1, Vec::new()))
            .expect("the candidate source advance is typed")
    );
    assert_eq!(
        driver
            .drive_replacement_once()
            .await
            .expect("candidate-only movement does not require a full build"),
        AuthorityTemplateStep::Idle
    );
    for component in [
        TemplateComponent::Proposals,
        TemplateComponent::Transactions,
    ] {
        assert_eq!(
            driver
                .drive_component_once(component)
                .await
                .expect("candidate-only movement is irrelevant to this lane"),
            AuthorityTemplateStep::Idle
        );
    }
    assert_eq!(
        runtime.template_capture_count_for_foundation(),
        converged_captures
    );
    assert_eq!(
        driver
            .drive_component_once(TemplateComponent::Uncles)
            .await
            .expect("the uncle lane captures its relevant source"),
        AuthorityTemplateStep::Published
    );
    assert_eq!(
        runtime.template_capture_count_for_foundation(),
        converged_captures + 1
    );
}

#[tokio::test]
async fn uak_template_driver_reproposes_recovered_gap_and_commits_after_proposal() {
    let snapshot = template_snapshot();
    let runtime = super::super::runtime::AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the authority runtime fixture is valid");
    let transaction = tx(1_901);
    let hash = runtime.with_authority_for_foundation(|authority| {
        accept_remote_transaction(authority, transaction, 901, AcceptedStatus::Gap, Vec::new())
    });
    let assembler = BlockAssembler::new(template_config(), Arc::clone(&snapshot))
        .expect("the block assembler fixture is valid");
    let driver = AuthorityBlockAssembler::new(runtime.clone(), assembler.clone())
        .await
        .expect("the authority template adapter is valid");

    assert_eq!(
        driver
            .drive_replacement_once()
            .await
            .expect("the initial full level builds"),
        AuthorityTemplateStep::Published
    );
    assert!(assembler.get_current().await.proposals.is_empty());

    set_status(&runtime, &hash, AcceptedStatus::Pending);
    for component in [
        TemplateComponent::Proposals,
        TemplateComponent::Uncles,
        TemplateComponent::Transactions,
    ] {
        assert_eq!(
            driver
                .drive_component_once(component)
                .await
                .expect("the component level builds"),
            AuthorityTemplateStep::Published
        );
    }
    let proposed = assembler.get_current().await;
    assert_eq!(proposed.proposals.len(), 1);
    assert!(proposed.transactions.is_empty());

    set_status(&runtime, &hash, AcceptedStatus::Proposed);
    for component in [
        TemplateComponent::Proposals,
        TemplateComponent::Uncles,
        TemplateComponent::Transactions,
    ] {
        assert_eq!(
            driver
                .drive_component_once(component)
                .await
                .expect("the proposed transaction level builds"),
            AuthorityTemplateStep::Published
        );
    }
    let committed = assembler.get_current().await;
    assert!(committed.proposals.is_empty());
    assert_eq!(committed.transactions.len(), 1);
}

#[tokio::test]
async fn uak_template_driver_degrades_to_the_finally_live_subset_without_lane_failure() {
    let snapshot = template_snapshot();
    let runtime = super::super::runtime::AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the authority runtime fixture is valid");
    let missing_input = OutPoint::new(Byte32::new([199; 32]), 0);
    let unavailable = TransactionBuilder::default()
        .version(1_904u32)
        .input(CellInput::new(missing_input.clone(), 0))
        .build();
    let unavailable_payload = resolved_payload_with_facts(
        &unavailable,
        Vec::new(),
        vec![missing_input],
        Capacity::shannons(1),
    );
    runtime.with_authority_for_foundation(|authority| {
        accept_remote_transaction_with_payload(
            authority,
            unavailable,
            904,
            AcceptedStatus::Proposed,
            unavailable_payload,
        );
        accept_remote_transaction(
            authority,
            tx(1_905),
            905,
            AcceptedStatus::Proposed,
            Vec::new(),
        );
    });
    let assembler = BlockAssembler::new(template_config(), snapshot)
        .expect("the block assembler fixture is valid");
    let driver = AuthorityBlockAssembler::new(runtime, assembler.clone())
        .await
        .expect("the authority template adapter is valid");

    assert_eq!(
        driver
            .drive_replacement_once()
            .await
            .expect("a projection liveness miss does not kill the replacement lane"),
        AuthorityTemplateStep::Published
    );
    assert_eq!(assembler.get_current().await.transactions.len(), 1);
    assert_eq!(
        driver
            .drive_replacement_once()
            .await
            .expect("the evaluated source does not enter a retry loop"),
        AuthorityTemplateStep::Idle
    );
}

#[tokio::test]
async fn uak_template_driver_full_priority_and_partial_occ_use_one_output_revision() {
    let snapshot = template_snapshot();
    let runtime = super::super::runtime::AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the authority runtime fixture is valid");
    let transaction = tx(1_902);
    let hash = runtime.with_authority_for_foundation(|authority| {
        accept_remote_transaction(
            authority,
            transaction,
            902,
            AcceptedStatus::Pending,
            Vec::new(),
        )
    });
    let assembler = BlockAssembler::new(template_config(), Arc::clone(&snapshot))
        .expect("the block assembler fixture is valid");
    let driver = AuthorityBlockAssembler::new(runtime.clone(), assembler.clone())
        .await
        .expect("the authority template adapter is valid");

    let full = driver
        .prepare_full_for_foundation()
        .await
        .expect("the initial full plan reads")
        .expect("initial construction requires a full build");
    let proposal = driver
        .prepare_component_for_foundation(TemplateComponent::Proposals)
        .await
        .expect("the proposal plan reads")
        .expect("the empty output does not cover proposals");
    assert_eq!(
        driver
            .publish_component_for_foundation(proposal)
            .await
            .expect("the proposal plan publishes"),
        AuthorityTemplateStep::Published
    );
    assert_eq!(
        driver
            .publish_full_for_foundation(full)
            .await
            .expect("the full plan publishes"),
        AuthorityTemplateStep::Published,
        "full publication deliberately wins over a partial-only revision"
    );

    set_status(&runtime, &hash, AcceptedStatus::Proposed);
    let proposal = driver
        .prepare_component_for_foundation(TemplateComponent::Proposals)
        .await
        .expect("the proposal plan reads")
        .expect("the status change dirties proposals");
    let transactions = driver
        .prepare_component_for_foundation(TemplateComponent::Transactions)
        .await
        .expect("the transaction plan reads")
        .expect("the status change dirties transactions");
    assert_eq!(
        driver
            .publish_component_for_foundation(proposal)
            .await
            .expect("one partial wins"),
        AuthorityTemplateStep::Published
    );
    assert_eq!(
        driver
            .publish_component_for_foundation(transactions)
            .await
            .expect("the racing partial is a typed outcome"),
        AuthorityTemplateStep::Stale,
        "a plan built from old template content cannot bind a newer revision"
    );
    assert_eq!(
        driver
            .drive_component_once(TemplateComponent::Transactions)
            .await
            .expect("the level-triggered transaction retry builds"),
        AuthorityTemplateStep::Published
    );
    assert_eq!(assembler.get_current().await.transactions.len(), 1);

    let snapshot = template_snapshot();
    let runtime = super::super::runtime::AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the second runtime fixture is valid");
    let assembler = BlockAssembler::new(template_config(), snapshot)
        .expect("the second assembler fixture is valid");
    let driver = AuthorityBlockAssembler::new(runtime, assembler)
        .await
        .expect("the second adapter is valid");
    drop(
        driver
            .prepare_full_for_foundation()
            .await
            .expect("the full plan reads")
            .expect("the initial full level is pending"),
    );
    assert_eq!(
        driver
            .drive_replacement_once()
            .await
            .expect("a dropped build is reconstructed from the level"),
        AuthorityTemplateStep::Published
    );
}

#[tokio::test]
async fn uak_template_driver_rechecks_candidate_source_and_filters_proposal_conflicts() {
    let snapshot = template_snapshot();
    let runtime = super::super::runtime::AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the authority runtime fixture is valid");
    let transaction = tx(1_903);
    let proposal_id = transaction.proposal_short_id();
    runtime.with_authority_for_foundation(|authority| {
        accept_remote_transaction(
            authority,
            transaction,
            903,
            AcceptedStatus::Pending,
            Vec::new(),
        );
    });
    let assembler = BlockAssembler::new(template_config(), Arc::clone(&snapshot))
        .expect("the block assembler fixture is valid");
    let driver = AuthorityBlockAssembler::new(runtime, assembler.clone())
        .await
        .expect("the authority template adapter is valid");
    assert_eq!(
        driver
            .drive_replacement_once()
            .await
            .expect("the initial full level builds"),
        AuthorityTemplateStep::Published
    );

    let first = candidate_uncle(&snapshot, 1, Vec::new());
    let second = candidate_uncle(&snapshot, 2, Vec::new());
    assert!(
        driver
            .receive_candidate_uncle(first)
            .expect("the first candidate is bounded")
    );
    let stale_source = driver
        .prepare_component_for_foundation(TemplateComponent::Uncles)
        .await
        .expect("the uncle plan reads")
        .expect("the first candidate dirties the uncle level");
    assert!(
        driver
            .receive_candidate_uncle(second)
            .expect("the second candidate is bounded")
    );
    assert_eq!(
        driver
            .publish_component_for_foundation(stale_source)
            .await
            .expect("the sealed old source may publish its exact content"),
        AuthorityTemplateStep::Published
    );
    assert_eq!(
        driver
            .drive_component_once(TemplateComponent::Uncles)
            .await
            .expect("the newer candidate source remains a visible level"),
        AuthorityTemplateStep::Published
    );
    assert_eq!(assembler.get_current().await.uncles.len(), 2);

    let conflicting = candidate_uncle(&snapshot, 3, vec![proposal_id.clone()]);
    assert!(
        driver
            .receive_candidate_uncle(conflicting)
            .expect("the conflicting candidate is bounded")
    );
    assert_eq!(
        driver
            .drive_component_once(TemplateComponent::Uncles)
            .await
            .expect("proposal conflict filtering builds"),
        AuthorityTemplateStep::Published
    );
    let current = assembler.get_current().await;
    assert_eq!(current.proposals.len(), 1);
    assert!(!current.uncles.is_empty());
    assert!(current.uncles.iter().all(|uncle| {
        !uncle
            .proposals
            .iter()
            .any(|proposal| proposal.0.as_slice() == proposal_id.as_slice())
    }));
}

#[tokio::test]
async fn uak_template_driver_publishes_the_exact_latest_reset_generation() {
    let original = template_snapshot();
    let runtime = super::super::runtime::AuthorityRuntime::new(
        &runtime_config(),
        original.consensus(),
        Arc::clone(&original),
    )
    .expect("the authority runtime fixture is valid");
    let assembler = BlockAssembler::new(template_config(), Arc::clone(&original))
        .expect("the block assembler fixture is valid");
    let driver = AuthorityBlockAssembler::new(runtime.clone(), assembler.clone())
        .await
        .expect("the authority template adapter is valid");
    let old_chain_full = driver
        .prepare_full_for_foundation()
        .await
        .expect("the old-chain full plan reads")
        .expect("initial construction requires a full build");
    let old_chain_partial = driver
        .prepare_component_for_foundation(TemplateComponent::Proposals)
        .await
        .expect("the old-chain partial plan reads")
        .expect("initial construction has uncovered proposals");

    let first_tip = template_snapshot_with_child(Some(1));
    runtime
        .clear_pool(first_tip)
        .expect("the first chain replacement commits");
    assert_eq!(
        driver
            .publish_full_for_foundation(old_chain_full)
            .await
            .expect("the old-chain build is a typed stale outcome"),
        AuthorityTemplateStep::Stale,
        "a committed chain transition fences an unobserved old full build"
    );
    assert_eq!(
        driver
            .publish_component_for_foundation(old_chain_partial)
            .await
            .expect("the old-chain partial is a typed stale outcome"),
        AuthorityTemplateStep::Stale,
        "a committed chain transition fences an unobserved old partial build"
    );
    let first_reset = driver
        .prepare_reset_for_foundation()
        .await
        .expect("the first reset reads")
        .expect("the first chain replacement requires a reset");

    let latest_tip = template_snapshot_with_child(Some(2));
    let latest_hash = latest_tip.tip_hash();
    let latest_reset_epoch = ResetEpoch::INITIAL
        .next()
        .and_then(ResetEpoch::next)
        .expect("two reset generations fit in the monotonic epoch");
    runtime
        .clear_pool(latest_tip)
        .expect("the latest chain replacement commits");
    let latest_reset = driver
        .prepare_reset_for_foundation()
        .await
        .expect("the latest reset reads")
        .expect("the latest chain replacement supersedes the first reset");

    assert_eq!(
        driver
            .publish_reset_for_foundation(latest_reset)
            .await
            .expect("the latest reset publishes"),
        AuthorityTemplateStep::Published
    );
    {
        let current = assembler.current.read().await;
        assert_eq!(current.snapshot.tip_hash(), latest_hash);
        assert_eq!(current.reset_epoch, latest_reset_epoch);
    }
    assert_eq!(
        driver
            .publish_reset_for_foundation(first_reset)
            .await
            .expect("the superseded reset is a typed stale outcome"),
        AuthorityTemplateStep::Stale
    );
    assert_eq!(
        driver
            .drive_replacement_once()
            .await
            .expect("the replacement level rebuilds after the exact reset"),
        AuthorityTemplateStep::Published
    );
    let current = assembler.current.read().await;
    assert_eq!(current.snapshot.tip_hash(), latest_hash);
    assert_eq!(current.reset_epoch, latest_reset_epoch);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_template_read_waits_for_the_exact_chain_source_publication() {
    let original = template_snapshot();
    let runtime = super::super::runtime::AuthorityRuntime::new(
        &runtime_config(),
        original.consensus(),
        Arc::clone(&original),
    )
    .expect("the authority runtime fixture is valid");
    let assembler = BlockAssembler::new(template_config(), original)
        .expect("the block assembler fixture is valid");
    let driver = AuthorityBlockAssembler::new(runtime.clone(), assembler)
        .await
        .expect("the authority template adapter is valid");
    runtime
        .clear_pool(template_snapshot_with_child(Some(1)))
        .expect("the chain source advances atomically");

    let cancel = CancellationToken::new();
    let read_driver = driver.clone();
    let read_cancel = cancel.clone();
    let mut read = tokio::spawn(async move { read_driver.current_template(&read_cancel).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut read)
            .await
            .is_err(),
        "a reader cannot observe the last template after its chain source became stale"
    );

    assert_eq!(
        driver
            .drive_replacement_once()
            .await
            .expect("the current-chain reset is publishable"),
        AuthorityTemplateStep::Published
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut read)
            .await
            .is_err(),
        "a reset publication cannot release a reader before all component receipts agree"
    );
    assert_eq!(
        driver
            .drive_replacement_once()
            .await
            .expect("the current-chain full rebuild is publishable"),
        AuthorityTemplateStep::Published
    );
    let template = tokio::time::timeout(Duration::from_secs(1), read)
        .await
        .expect("the replacement publication releases the exact source waiter")
        .expect("the template reader task joins")
        .expect("the current source has a valid underfilled template");
    assert_eq!(u64::from(template.number), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_template_read_returns_unavailable_after_same_source_reset_failure() {
    let original = template_snapshot();
    let runtime = super::super::runtime::AuthorityRuntime::new(
        &runtime_config(),
        original.consensus(),
        Arc::clone(&original),
    )
    .expect("the authority runtime fixture is valid");
    let assembler = BlockAssembler::new(template_config(), original)
        .expect("the block assembler fixture is valid");
    let driver = AuthorityBlockAssembler::new(runtime.clone(), assembler.clone())
        .await
        .expect("the authority template adapter is valid");
    assembler
        .work_id
        .store(u64::MAX, std::sync::atomic::Ordering::Relaxed);
    runtime
        .clear_pool(template_snapshot_with_child(Some(1)))
        .expect("the chain source advances atomically");

    let runtime_handle = ckb_async_runtime::Handle::new(tokio::runtime::Handle::current(), None);
    let cancel = CancellationToken::new();
    let handles = driver.spawn_drivers(&runtime_handle, cancel.clone());
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), driver.current_template(&cancel))
            .await
            .expect("the failed source publishes a terminal read state"),
        Err(AuthorityTemplateReadFailure::Unavailable)
    );

    cancel.cancel();
    for task in handles.tasks {
        task.handle
            .await
            .expect("the template lane task does not panic")
            .expect("a rebuildable template failure parks until cancellation");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_replacement_publication_wakes_the_coalesced_template_observer() {
    let snapshot = template_snapshot();
    let runtime = super::super::runtime::AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the authority runtime fixture is valid");
    let assembler = BlockAssembler::new(template_config(), snapshot)
        .expect("the block assembler fixture is valid");
    let driver = AuthorityBlockAssembler::new(runtime, assembler.clone())
        .await
        .expect("the authority template adapter is valid");
    let cancel = CancellationToken::new();
    let notification_driver = driver.clone();
    let notification_cancel = cancel.clone();
    let notification = tokio::spawn(async move {
        notification_driver
            .run_notification_lane_for_foundation(notification_cancel)
            .await
    });

    assert_eq!(
        driver
            .drive_replacement_once()
            .await
            .expect("the initial full template is publishable"),
        AuthorityTemplateStep::Published
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while assembler
            .notify_count
            .load(std::sync::atomic::Ordering::SeqCst)
            == 0
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("replacement publication wakes the observer without waiting for its interval");

    cancel.cancel();
    notification
        .await
        .expect("the notification lane does not panic")
        .expect("notification cancellation is clean");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_template_drivers_cancel_cleanly_without_idle_publication() {
    let snapshot = template_snapshot();
    let runtime = super::super::runtime::AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the authority runtime fixture is valid");
    let assembler = BlockAssembler::new(template_config(), snapshot)
        .expect("the block assembler fixture is valid");
    let driver = AuthorityBlockAssembler::new(runtime, assembler.clone())
        .await
        .expect("the authority template adapter is valid");
    let runtime_handle = ckb_async_runtime::Handle::new(tokio::runtime::Handle::current(), None);

    let initial_work_id = assembler.get_current().await.work_id;
    let cancelled_before_spawn = ckb_stop_handler::CancellationToken::new();
    cancelled_before_spawn.cancel();
    let cancelled_handles = driver.spawn_drivers(&runtime_handle, cancelled_before_spawn.clone());
    for task in cancelled_handles.tasks {
        task.handle
            .await
            .expect("the pre-cancelled template lane task does not panic")
            .expect("pre-cancellation is a clean template-lane outcome");
    }
    assert_eq!(assembler.get_current().await.work_id, initial_work_id);

    let cancel = ckb_stop_handler::CancellationToken::new();
    let handles = driver.spawn_drivers(&runtime_handle, cancel.clone());

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !driver.is_converged_for_foundation().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the fixed lanes converge from initial construction");
    let stable_work_id = assembler.get_current().await.work_id;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(assembler.get_current().await.work_id, stable_work_id);

    cancel.cancel();
    for task in handles.tasks {
        task.handle
            .await
            .expect("the template lane task does not panic")
            .expect("cancellation is a clean template-lane outcome");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uak_template_rebuild_failure_retains_the_last_projection_and_lane() {
    let snapshot = template_snapshot();
    let runtime = super::super::runtime::AuthorityRuntime::new(
        &runtime_config(),
        snapshot.consensus(),
        Arc::clone(&snapshot),
    )
    .expect("the authority runtime fixture is valid");
    let assembler = BlockAssembler::new(template_config(), snapshot)
        .expect("the block assembler fixture is valid");
    let initial = assembler.get_current().await;
    assembler
        .work_id
        .store(u64::MAX, std::sync::atomic::Ordering::Relaxed);
    let driver = AuthorityBlockAssembler::new(runtime, assembler.clone())
        .await
        .expect("the authority template adapter is valid");
    let runtime_handle = ckb_async_runtime::Handle::new(tokio::runtime::Handle::current(), None);
    let cancel = ckb_stop_handler::CancellationToken::new();
    let handles = driver.spawn_drivers(&runtime_handle, cancel.clone());

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(
        assembler.get_current().await.work_id,
        initial.work_id,
        "a rebuildable projection failure must retain the last valid template"
    );
    assert!(
        driver.current_template(&cancel).await.is_ok(),
        "a same-chain rebuild failure retains a template that is still valid for that source"
    );
    cancel.cancel();
    for task in handles.tasks {
        task.handle
            .await
            .expect("the template lane task does not panic")
            .expect("a rebuildable template failure waits for a new source cut");
    }
}
