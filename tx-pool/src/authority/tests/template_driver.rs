use super::super::{
    state::AcceptedStatus,
    template::TemplateComponent,
    template_driver::{AuthorityBlockAssembler, AuthorityTemplateStep},
};
use super::foundation::{
    accept_remote_transaction, accept_remote_transaction_with_payload, apply_without_work,
    owner_version, resolved_payload_with_facts, runtime_config, tx,
};
use crate::block_assembler::{BlockAssembler, ResetEpoch};
use ckb_app_config::BlockAssemblerConfig;
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_jsonrpc_types::ScriptHashType;
use ckb_snapshot::Snapshot;
use ckb_store::{ChainStore, attach_block_cell};
use ckb_test_chain_utils::MockStore;
use ckb_types::{U256, core::BlockExt, h256, utilities::merkle_mountain_range::ChainRootMMR};
use ckb_types::{
    core::{BlockBuilder, Capacity, TransactionBuilder, UncleBlockView},
    packed::{Byte32, CellInput, OutPoint, ProposalShortId},
    prelude::Entity,
};
use std::sync::Arc;

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
        apply_without_work(
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
) -> UncleBlockView {
    BlockBuilder::default()
        .number(snapshot.tip_number())
        .parent_hash(snapshot.tip_hash())
        .timestamp(timestamp)
        .compact_target(snapshot.tip_header().compact_target())
        .epoch(snapshot.tip_header().epoch())
        .proposals(proposals)
        .build()
        .as_uncle()
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
    for handle in [
        cancelled_handles.replacement,
        cancelled_handles.proposals,
        cancelled_handles.transactions,
        cancelled_handles.uncles,
    ] {
        handle
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
    for handle in [
        handles.replacement,
        handles.proposals,
        handles.transactions,
        handles.uncles,
    ] {
        handle
            .await
            .expect("the template lane task does not panic")
            .expect("cancellation is a clean template-lane outcome");
    }
}
