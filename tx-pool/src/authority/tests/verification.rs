use super::*;
use crate::authority::{model::Source, tests::common::*};
use ckb_types::{
    core::Capacity,
    packed::{CellDep, CellOutput, OutPoint, OutPointVec},
};
use ckb_verification::cache::init_cache;

fn fixture() -> (Arc<Store>, Arc<Entry>, Arc<Snapshot>) {
    let snapshot = chain_snapshot();
    let store = Store::new(Arc::clone(&snapshot), &config()).unwrap();
    let parent = funded_parent(1000, 20_000_000_000);
    accept(&store, parent.clone(), 1, 1, Status::Pending);
    let candidate = entry(
        &store,
        funded_tx(OutPoint::new(parent.hash(), 0), 19_999_999_000),
        Source::Local,
    );
    (store, candidate, snapshot)
}
fn resolved(store: &Store, candidate: &Entry, config: &TxPoolConfig) -> Arc<Resolved> {
    match resolve(store, candidate, config).unwrap() {
        Resolution::Ready(resolved) => resolved,
        _ => panic!("canonical fixture did not resolve"),
    }
}
fn key(candidate: &Entry, snapshot: &Snapshot) -> TxVerificationCacheKey {
    TxVerificationCacheKey::from_transaction(
        &candidate.transaction,
        ScriptVerificationRules::from_env(
            snapshot.consensus(),
            &environment(Status::Pending, snapshot),
        ),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canonical_vm_success_produces_witness_bound_cache_proof_and_current_fee() {
    let (store, candidate, snapshot) = fixture();
    let config = config();
    let resolved = resolved(&store, &candidate, &config);
    assert_eq!(resolved.fee, Capacity::shannons(1000));
    let cache = RwLock::new(init_cache());
    let (_sender, mut commands) = watch::channel(ChunkCommand::Resume);
    let first = verify(
        &store,
        &candidate,
        Arc::clone(&resolved),
        &config,
        &cache,
        &mut commands,
        TxPoolVmExecutionMode::Inline,
    )
    .await
    .unwrap();
    assert!(first.cycles() > 0);
    assert_eq!(
        cache
            .read()
            .await
            .lookup(&key(&candidate, &snapshot))
            .unwrap()
            .cycles(),
        first.cycles()
    );
    let second = verify(
        &store,
        &candidate,
        resolved,
        &config,
        &cache,
        &mut commands,
        TxPoolVmExecutionMode::Inline,
    )
    .await
    .unwrap();
    assert_eq!(second.cycles(), first.cycles());
    let changed = candidate
        .transaction
        .as_ref()
        .clone()
        .as_advanced_builder()
        .witness(Bytes::from_static(b"changed witness").pack())
        .build();
    assert_eq!(changed.hash(), candidate.hash());
    assert_ne!(changed.witness_hash(), candidate.transaction.witness_hash());
    let changed = entry(&store, changed, Source::Local);
    assert!(
        cache
            .read()
            .await
            .lookup(&key(&changed, &snapshot))
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_initial_load_refusal_is_not_cached_and_allows_a_later_normal_attempt() {
    let (store, candidate, snapshot) = fixture();
    let mut config = config();
    let resolved = resolved(&store, &candidate, &config);
    let cache = RwLock::new(init_cache());
    let (_sender, mut commands) = watch::channel(ChunkCommand::Resume);
    config.max_tx_verify_initial_load_bytes = 1;
    let result = verify(
        &store,
        &candidate,
        Arc::clone(&resolved),
        &config,
        &cache,
        &mut commands,
        TxPoolVmExecutionMode::Inline,
    )
    .await;
    assert!(matches!(result, Err(Error::Rejected(Reject::Full(_)))));
    assert!(
        cache
            .read()
            .await
            .lookup(&key(&candidate, &snapshot))
            .is_none()
    );
    config.max_tx_verify_initial_load_bytes = 256 * 1024 * 1024;
    assert!(
        verify(
            &store,
            &candidate,
            resolved,
            &config,
            &cache,
            &mut commands,
            TxPoolVmExecutionMode::Inline
        )
        .await
        .is_ok()
    );
    assert!(!store.is_faulted());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_cycle_declaration_is_exact_even_when_reusing_local_cache_success() {
    let (store, candidate, _) = fixture();
    let config = config();
    let resolved = resolved(&store, &candidate, &config);
    let cache = RwLock::new(init_cache());
    let (_sender, mut commands) = watch::channel(ChunkCommand::Resume);
    let first = verify(
        &store,
        &candidate,
        Arc::clone(&resolved),
        &config,
        &cache,
        &mut commands,
        TxPoolVmExecutionMode::Inline,
    )
    .await
    .unwrap();
    let wrong = Entry {
        source: Source::Remote {
            peer: ckb_network::PeerIndex::from(1),
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(30),
            cycles: Some(first.cycles() + 1),
        },
        ..candidate.as_ref().clone()
    };
    assert!(matches!(
        verify(
            &store,
            &wrong,
            resolved,
            &config,
            &cache,
            &mut commands,
            TxPoolVmExecutionMode::Inline
        )
        .await,
        Err(Error::Rejected(Reject::DeclaredWrongCycles(_, _)))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn producer_reentry_invalidates_queued_resolution_before_vm_or_cache_publication() {
    let (store, candidate, snapshot) = fixture();
    let config = config();
    let resolved = resolved(&store, &candidate, &config);
    let parent = candidate
        .transaction
        .input_pts_iter()
        .next()
        .unwrap()
        .tx_hash();
    let before = store.point(&parent).1.unwrap();
    replace(&store, Arc::clone(&before), before.phase.clone());
    let cache = RwLock::new(init_cache());
    let (_sender, mut commands) = watch::channel(ChunkCommand::Resume);
    assert!(matches!(
        verify(
            &store,
            &candidate,
            resolved,
            &config,
            &cache,
            &mut commands,
            TxPoolVmExecutionMode::Inline
        )
        .await,
        Err(Error::Stale)
    ));
    assert!(
        cache
            .read()
            .await
            .lookup(&key(&candidate, &snapshot))
            .is_none()
    );
}

#[test]
fn resolution_rejects_low_fee_before_executing_scripts() {
    let (store, candidate, _) = fixture();
    let config = TxPoolConfig {
        min_fee_rate: ckb_types::core::FeeRate::from_u64(1_000_000),
        ..config()
    };
    assert!(matches!(
        resolve(&store, &candidate, &config).unwrap(),
        Resolution::Rejected(Reject::LowFeeRate(_, _, _), _)
    ));
}

#[test]
fn missing_frontier_expands_dep_groups_and_keeps_original_group_producer_reads() {
    let (store, candidate, _) = fixture();
    let missing = OutPoint::new(ckb_types::packed::Byte32::new([89; 32]), 0);
    let members = OutPointVec::new_builder().push(missing.clone()).build();
    let group = ckb_types::core::TransactionBuilder::default()
        .version(1001u32)
        .output(CellOutput::default())
        .output_data(members.as_bytes().pack())
        .build();
    let group_hash = accept(&store, group.clone(), 1, 1, Status::Pending);
    let transaction = candidate
        .transaction
        .as_ref()
        .clone()
        .as_advanced_builder()
        .cell_dep(
            CellDep::new_builder()
                .out_point(OutPoint::new(group.hash(), 0))
                .dep_type(DepType::DepGroup)
                .build(),
        )
        .build();
    let candidate = entry(
        &store,
        transaction,
        super::super::ingress::remote_source(91.into(), 1).unwrap(),
    );
    let Resolution::Waiting(keys, reads) = resolve(&store, &candidate, &config()).unwrap() else {
        panic!("missing expanded member must wait")
    };
    assert_eq!(keys, BTreeSet::from([DependencyKey::Cell(missing)]));
    let before = store.point(&group_hash).1.unwrap();
    replace(&store, Arc::clone(&before), before.phase.clone());
    assert!(matches!(
        store.read_selected(store.snapshot().0, &reads, || ()),
        Err(Error::Stale)
    ));
}

#[test]
fn repeated_materialization_shares_one_precharged_detached_cell() {
    let store = super::super::tests::common::store();
    let parent = ckb_types::core::TransactionBuilder::default()
        .version(1002u32)
        .output(CellOutput::default())
        .output_data(Bytes::from(vec![0x7a; 4096]).pack())
        .build();
    accept(&store, parent.clone(), 1, 1, Status::Pending);
    let (_, snapshot) = store.snapshot();
    let mut provider = Provider {
        store: &store,
        snapshot: &snapshot,
        observed: RefCell::new(Observed::default()),
        max_bytes: 100_000,
        max_edges: 4,
    };
    let point = OutPoint::new(parent.hash(), 0);
    let first = provider.materialize(&point, true).unwrap();
    provider.max_bytes = provider.observed.borrow().bytes;
    let second = provider.materialize(&point, true).unwrap();
    let (CellStatus::Live(first), CellStatus::Live(second)) = (first, second) else {
        panic!("accepted output is live")
    };
    assert_eq!(
        first.mem_cell_data.as_ref().unwrap().as_ptr(),
        second.mem_cell_data.as_ref().unwrap().as_ptr()
    );
    assert_eq!(provider.observed.borrow().cells.len(), 1);
    assert_eq!(provider.observed.borrow().bytes, provider.max_bytes);
}

#[test]
fn resolution_shares_one_detached_cell_across_input_and_dependency_roles() {
    let store = Store::new(chain_snapshot(), &config()).unwrap();
    let parent = funded_parent(1005, 20_000_000_000)
        .as_advanced_builder()
        .set_outputs_data(vec![Bytes::from_static(b"shared cell data").pack()])
        .build();
    accept(&store, parent.clone(), 1, 1, Status::Pending);
    let point = OutPoint::new(parent.hash(), 0);
    let transaction = funded_tx(point.clone(), 19_999_999_000)
        .as_advanced_builder()
        .cell_dep(CellDep::new_builder().out_point(point.clone()).build())
        .build();
    let candidate = entry(&store, transaction, Source::Local);
    let resolved = resolved(&store, &candidate, &config());
    let input = &resolved.transaction.resolved_inputs[0];
    let dependency = resolved
        .transaction
        .resolved_cell_deps
        .iter()
        .find(|cell| cell.out_point == point)
        .unwrap();
    assert_eq!(input, dependency);
    assert_eq!(
        input.cell_output.as_slice().as_ptr(),
        dependency.cell_output.as_slice().as_ptr()
    );
    let input_data = input.mem_cell_data.as_ref().unwrap();
    assert_eq!(input_data.as_ref(), b"shared cell data");
    assert_eq!(
        input_data.as_ptr(),
        dependency.mem_cell_data.as_ref().unwrap().as_ptr()
    );
    let producer = parent.data();
    let start = producer.as_slice().as_ptr() as usize;
    let end = start + producer.total_size();
    for address in [input.cell_output.as_slice().as_ptr(), input_data.as_ptr()] {
        assert!(!(start..end).contains(&(address as usize)));
    }
    assert_eq!(resolved.fee, Capacity::shannons(1000));
    assert!(resolved.pool_cells.contains(&point));
    store
        .read_selected(resolved.view, &resolved.reads, || ())
        .unwrap();
    store
        .budget
        .limits
        .resolved_fits(&candidate.with_phase(Phase::Verify(resolved)))
        .unwrap();
}

#[test]
fn unknown_inputs_reject_but_dependency_readers_can_precede_pool_spenders() {
    let (store, candidate, _) = fixture();
    let missing = entry(
        &store,
        funded_tx(
            OutPoint::new(ckb_types::packed::Byte32::new([90; 32]), 0),
            10_000_000_000,
        ),
        Source::Local,
    );
    assert!(matches!(
        resolve(&store, &missing, &config()).unwrap(),
        Resolution::Rejected(Reject::Resolve(OutPointError::Unknown(_)), _)
    ));
    accept(
        &store,
        candidate.transaction.as_ref().clone(),
        1000,
        1,
        Status::Pending,
    );
    let input = candidate.transaction.input_pts_iter().next().unwrap();
    let reader = entry(
        &store,
        ckb_types::core::TransactionBuilder::default()
            .cell_dep(CellDep::new_builder().out_point(input).build())
            .build(),
        Source::Local,
    );
    assert!(matches!(
        resolve(&store, &reader, &config()).unwrap(),
        Resolution::Ready(_)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dependency_spent_after_verification_cannot_commit_the_old_live_observation() {
    let (store, original, _) = fixture();
    let dependency = funded_parent(1003, 20_000_000_000);
    accept(&store, dependency.clone(), 1, 1, Status::Pending);
    let transaction = original
        .transaction
        .as_ref()
        .clone()
        .as_advanced_builder()
        .cell_dep(
            CellDep::new_builder()
                .out_point(OutPoint::new(dependency.hash(), 0))
                .build(),
        )
        .build();
    let candidate = entry(&store, transaction, Source::Local);
    let config = config();
    let resolved = resolved(&store, &candidate, &config);
    let cache = RwLock::new(init_cache());
    let (_sender, mut commands) = watch::channel(ChunkCommand::Resume);
    let proof = verify(
        &store,
        &candidate,
        resolved,
        &config,
        &cache,
        &mut commands,
        TxPoolVmExecutionMode::Inline,
    )
    .await
    .unwrap();
    accept(
        &store,
        spend(1004, &[OutPoint::new(dependency.hash(), 0)], &[]),
        1,
        1,
        Status::Pending,
    );
    let prepared =
        crate::authority::membership::admission(&store, &candidate, None, &proof, &config, true);
    match prepared {
        Err(Error::Stale) => {}
        Ok((plan, reject)) => {
            assert!(
                reject.is_some() || matches!(store.apply(plan), Err(Error::Stale)),
                "a previously live dependency was spent before the candidate commit"
            );
        }
        _ => panic!("unexpected admission result"),
    }
    assert!(store.point(&candidate.hash()).1.is_none());
}

#[test]
fn trusted_missing_resolution_waits_only_for_a_known_producer_with_a_valid_output_index() {
    let store = Store::new(chain_snapshot(), &config()).unwrap();
    let parent = entry(
        &store,
        funded_parent(9012, 20_000_000_000),
        Source::Recovery,
    );
    insert(&store, Arc::clone(&parent));
    for (index, may_wait) in [(0, true), (9, false)] {
        let candidate = entry(
            &store,
            funded_tx(OutPoint::new(parent.hash(), index), 19_999_999_000),
            Source::Recovery,
        );
        let result = resolve(&store, &candidate, &config()).unwrap();
        if may_wait {
            assert!(matches!(result, Resolution::Waiting(_, _)));
        } else {
            assert!(matches!(
                result,
                Resolution::Rejected(Reject::Resolve(OutPointError::Unknown(_)), _)
            ));
        }
    }
}

#[test]
fn materialization_keeps_current_spender_in_original_reads() {
    let store = super::super::tests::common::store();
    let parent = output_tx(1010);
    let point = OutPoint::new(parent.hash(), 0);
    accept(&store, parent, 1, 1, Status::Pending);
    let consumer = accept(
        &store,
        spend(1011, std::slice::from_ref(&point), &[]),
        1,
        1,
        Status::Pending,
    );
    let (view, snapshot) = store.snapshot();
    let provider = Provider {
        store: &store,
        snapshot: &snapshot,
        observed: RefCell::new(Observed::default()),
        max_bytes: 100_000,
        max_edges: 4,
    };
    assert!(provider.materialize(&point, true).unwrap().is_live());
    let reads = provider.observed.into_inner().reads;
    assert_eq!(reads.spent().collect::<Vec<_>>(), vec![(&point, &consumer)]);
    let before = store.point(&consumer).1.unwrap();
    let mut plan = super::super::store::Plan::new(
        view,
        super::super::notice::Class::Trusted,
        Default::default(),
    );
    plan.edit(Some(before), None, None).unwrap();
    store.apply(plan).unwrap();
    assert!(matches!(
        store.read_selected(view, &reads, || ()),
        Err(Error::Stale)
    ));
}

#[test]
fn expanded_missing_dependencies_fit_the_job_envelope_or_return_capacity_without_retention() {
    let configuration = config();
    for count in [32, 128] {
        let store = store_with_pipeline_limit(chain_snapshot(), &configuration, 1_000_000);
        let parent = funded_parent(1100, 20_000_000_000);
        accept(&store, parent.clone(), 1, 1, Status::Pending);
        let members: OutPointVec = (0..count)
            .map(|index| OutPoint::new(tx(1101 + index).hash(), 0))
            .collect::<Vec<_>>()
            .pack();
        let group = ckb_types::core::TransactionBuilder::default()
            .version(1400u32)
            .output(CellOutput::default())
            .output_data(members.as_bytes().pack())
            .build();
        accept(&store, group.clone(), 1, 1, Status::Pending);
        let transaction = funded_tx(OutPoint::new(parent.hash(), 0), 19_999_999_000)
            .as_advanced_builder()
            .cell_dep(
                CellDep::new_builder()
                    .out_point(OutPoint::new(group.hash(), 0))
                    .dep_type(DepType::DepGroup)
                    .build(),
            )
            .build();
        let candidate = entry(
            &store,
            transaction,
            super::super::ingress::remote_source(96.into(), 1).unwrap(),
        );
        let before = store.budget.accepted_usage();
        let result = resolve(&store, &candidate, &configuration);
        if count == 32 {
            let Resolution::Waiting(keys, _) = result.unwrap() else {
                panic!("bounded missing frontier waits");
            };
            assert_eq!(keys.len(), count as usize);
            store
                .budget
                .limits
                .resolved_fits(&candidate.with_phase(Phase::Waiting(keys)))
                .unwrap();
        } else {
            assert!(matches!(result, Err(Error::Full(_))));
        }
        assert!(store.point(&candidate.hash()).1.is_none());
        assert_eq!(store.budget.accepted_usage(), before);
        assert!(!store.is_faulted());
    }
}

#[test]
fn oversized_direct_edge_footprint_refuses_before_resolution_and_missing_input_is_never_fabricated_by_rbf()
 {
    let configuration = TxPoolConfig {
        min_rbf_rate: ckb_types::core::FeeRate::from_u64(1000),
        ..config()
    };
    let store = store_with_pipeline_limit(chain_snapshot(), &configuration, 1_000_000);
    let oversized = spend(
        1500,
        &(0..128)
            .map(|n| OutPoint::new(tx(1501 + n).hash(), 0))
            .collect::<Vec<_>>(),
        &[],
    );
    let candidate = entry(&store, oversized, Source::Local);
    assert!(matches!(
        resolve(&store, &candidate, &configuration),
        Err(Error::Full(_))
    ));
    let absent = OutPoint::new(tx(1700).hash(), 0);
    let candidate = entry(
        &store,
        funded_tx(absent.clone(), 20_000_000_000),
        Source::Local,
    );
    assert!(
        matches!(resolve(&store, &candidate, &configuration).unwrap(),
        Resolution::Rejected(Reject::Resolve(OutPointError::Unknown(point)), _) if point == absent)
    );
    assert!(store.capture(false).2.is_empty());
    assert!(!store.is_faulted());
}

use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_test_chain_utils::{MockMedianTime, MockStore};
use ckb_types::{
    U256,
    core::{EpochNumberWithFraction, HeaderView, TransactionBuilder, TransactionInfo},
    packed::{Byte32, CellInput},
};
use ckb_verification::TimeRelativeTransactionVerifier;

fn sensitivity_fixture(since: u64) -> ResolvedTransaction {
    let input = OutPoint::new(Byte32::new([80; 32]), 0);
    let code = OutPoint::new(Byte32::new([81; 32]), 0);
    let group = OutPoint::new(Byte32::new([82; 32]), 0);
    ResolvedTransaction::dummy_resolve(
        TransactionBuilder::default()
            .input(CellInput::new(input, since))
            .cell_dep(CellDep::new_builder().out_point(code).build())
            .cell_dep(
                CellDep::new_builder()
                    .out_point(group)
                    .dep_type(DepType::DepGroup)
                    .build(),
            )
            .build(),
    )
}

fn sensitivity_info(block_number: u64, index: usize) -> TransactionInfo {
    TransactionInfo::new(
        block_number,
        EpochNumberWithFraction::new(1, 0, 1),
        Byte32::new([83; 32]),
        index,
    )
}

fn time_relative_verifier_accepts(
    resolved: Arc<ResolvedTransaction>,
    block_number: u64,
    epoch_number: u64,
    cellbase_maturity: EpochNumberWithFraction,
) -> bool {
    let data_loader = MockMedianTime::new(vec![0; 11]);
    let parent_hash = data_loader.get_last_block_hash();
    let consensus = Arc::new(
        ConsensusBuilder::default()
            .median_time_block_count(11)
            .cellbase_maturity(cellbase_maturity)
            .build(),
    );
    let header = HeaderView::new_advanced_builder()
        .number(block_number)
        .epoch(EpochNumberWithFraction::new(epoch_number, 0, 1))
        .parent_hash(parent_hash)
        .build();
    TimeRelativeTransactionVerifier::new(
        resolved,
        consensus,
        data_loader,
        Arc::new(TxVerifyEnv::new_commit(&header)),
    )
    .verify()
    .is_ok()
}

fn assert_sensitivity_matches_verifier_observation(
    resolved: ResolvedTransaction,
    first_context: (u64, u64),
    second_context: (u64, u64),
    cellbase_maturity: EpochNumberWithFraction,
) {
    let resolved = Arc::new(resolved);
    let first = time_relative_verifier_accepts(
        Arc::clone(&resolved),
        first_context.0,
        first_context.1,
        cellbase_maturity,
    );
    let second = time_relative_verifier_accepts(
        Arc::clone(&resolved),
        second_context.0,
        second_context.1,
        cellbase_maturity,
    );
    assert_eq!(
        context_sensitive(&resolved),
        first != second,
        "the retained sensitivity bit must equal an observed change in the real time-relative verifier"
    );
}

#[test]
fn chain_sensitivity_matches_canonical_since_and_maturity_observations() {
    let no_cellbase_maturity = EpochNumberWithFraction::new(0, 0, 1);
    let two_epoch_maturity = EpochNumberWithFraction::new(2, 0, 1);

    let stable = sensitivity_fixture(0);
    assert_sensitivity_matches_verifier_observation(stable, (4, 2), (5, 3), no_cellbase_maturity);

    let since = sensitivity_fixture(5);
    assert_sensitivity_matches_verifier_observation(since, (4, 3), (5, 3), no_cellbase_maturity);

    let mut regular_input = sensitivity_fixture(0);
    regular_input
        .resolved_inputs
        .first_mut()
        .expect("the fixture has one input")
        .transaction_info = Some(sensitivity_info(1, 1));
    assert_sensitivity_matches_verifier_observation(
        regular_input,
        (5, 2),
        (5, 3),
        two_epoch_maturity,
    );

    let mut genesis_cellbase = sensitivity_fixture(0);
    genesis_cellbase
        .resolved_inputs
        .first_mut()
        .expect("the fixture has one input")
        .transaction_info = Some(sensitivity_info(0, 0));
    assert_sensitivity_matches_verifier_observation(
        genesis_cellbase,
        (5, 2),
        (5, 3),
        two_epoch_maturity,
    );

    let mut input_cellbase = sensitivity_fixture(0);
    input_cellbase
        .resolved_inputs
        .first_mut()
        .expect("the fixture has one input")
        .transaction_info = Some(sensitivity_info(1, 0));
    assert_sensitivity_matches_verifier_observation(
        input_cellbase,
        (5, 2),
        (5, 3),
        two_epoch_maturity,
    );

    // Direct code deps and members expanded from a dep group share the
    // `resolved_cell_deps` role consumed by `MaturityVerifier`.
    let mut expanded_cellbase = sensitivity_fixture(0);
    expanded_cellbase
        .resolved_cell_deps
        .first_mut()
        .expect("the fixture has one resolved code dependency")
        .transaction_info = Some(sensitivity_info(1, 0));
    assert_sensitivity_matches_verifier_observation(
        expanded_cellbase,
        (5, 2),
        (5, 3),
        two_epoch_maturity,
    );

    // The dep-group container is location evidence but is not read by the
    // consensus maturity verifier. Marking it contextual would only cause
    // unnecessary revalidation after a payload-neutral detach.
    let mut group_container = sensitivity_fixture(0);
    group_container
        .resolved_dep_groups
        .first_mut()
        .expect("the fixture has one dep-group container")
        .transaction_info = Some(sensitivity_info(1, 0));
    assert_sensitivity_matches_verifier_observation(
        group_container,
        (5, 2),
        (5, 3),
        two_epoch_maturity,
    );
}

#[test]
fn verification_environment_obeys_phase_owned_commit_bounds() {
    for closest in 1..=4 {
        let consensus = Arc::new(
            ConsensusBuilder::default()
                .tx_proposal_window(ckb_chain_spec::consensus::ProposalWindow(
                    closest,
                    closest + 2,
                ))
                .build(),
        );
        for (status, tip) in [
            (Status::Pending, 41),
            (Status::Gap, 42),
            (Status::Proposed, 43),
        ] {
            let store = MockStore::default();
            let header = consensus
                .genesis_block()
                .header()
                .as_advanced_builder()
                .number(tip)
                .epoch(EpochNumberWithFraction::new(
                    tip / 1_000,
                    tip % 1_000,
                    1_000,
                ))
                .build();
            let snapshot = Snapshot::new(
                header,
                U256::zero(),
                consensus.genesis_epoch_ext().clone(),
                store.store().get_snapshot(),
                Default::default(),
                Arc::clone(&consensus),
            );
            let window = consensus.tx_proposal_window();
            let production = environment(status, &snapshot).block_number(window);
            let expected = match status {
                Status::Pending => tip + 1 + closest,
                Status::Gap => tip + closest,
                Status::Proposed => tip + 1,
            };
            assert_eq!(production, expected);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canonical_conflicting_input_obeys_final_replacement_policy() {
    let (store, original, _) = fixture();
    let input = original.transaction.input_pts_iter().next().unwrap();
    let incumbent = accept(
        &store,
        original.transaction.as_ref().clone(),
        1000,
        1,
        Status::Pending,
    );
    let candidate = entry(
        &store,
        funded_tx(input.clone(), 19_999_998_000),
        Source::Local,
    );
    let mut configuration = config();
    let backing = resolved(&store, &candidate, &configuration);
    assert_eq!(backing.fee, Capacity::shannons(2000));
    let cache = RwLock::new(init_cache());
    let (_sender, mut commands) = watch::channel(ChunkCommand::Resume);
    let proof = verify(
        &store,
        &candidate,
        backing,
        &configuration,
        &cache,
        &mut commands,
        TxPoolVmExecutionMode::Inline,
    )
    .await
    .unwrap();
    let (plan, reject) =
        super::super::membership::admission(&store, &candidate, None, &proof, &configuration, true)
            .unwrap();
    assert!(matches!(reject, Some(Reject::Resolve(OutPointError::Dead(point))) if point == input));
    store.apply(plan).unwrap();
    assert!(store.point(&incumbent).1.unwrap().accepted().is_some());
    assert!(store.point(&candidate.hash()).1.is_none());

    configuration.min_rbf_rate = ckb_types::core::FeeRate::from_u64(1000);
    let (plan, reject) =
        super::super::membership::admission(&store, &candidate, None, &proof, &configuration, true)
            .unwrap();
    assert!(reject.is_none());
    store.apply(plan).unwrap();
    assert!(
        store
            .point(&incumbent)
            .1
            .is_none_or(|owner| owner.accepted().is_none())
    );
    assert!(
        store
            .point(&candidate.hash())
            .1
            .unwrap()
            .accepted()
            .is_some()
    );
    assert!(!store.is_faulted());
}

#[test]
fn canonical_conflicting_input_preserves_missing_duplicate_and_group_errors() {
    let (store, original, _) = fixture();
    let input = original.transaction.input_pts_iter().next().unwrap();
    let incumbent = accept(
        &store,
        original.transaction.as_ref().clone(),
        1000,
        1,
        Status::Pending,
    );
    let absent = OutPoint::new(tx(1800).hash(), 0);
    for inputs in [
        [absent.clone(), input.clone()],
        [input.clone(), absent.clone()],
    ] {
        let candidate = entry(
            &store,
            spend(1801, &inputs, &[]),
            super::super::ingress::remote_source(97.into(), 1).unwrap(),
        );
        let Resolution::Waiting(keys, reads) = resolve(&store, &candidate, &config()).unwrap()
        else {
            panic!("the missing input must remain a wakeable frontier")
        };
        assert_eq!(keys, BTreeSet::from([DependencyKey::Cell(absent.clone())]));
        assert!(
            reads
                .spent()
                .any(|(point, spender)| point == &input && spender == &incumbent)
        );
    }
    let duplicate = entry(
        &store,
        spend(1802, &[input.clone(), input.clone()], &[]),
        Source::Local,
    );
    assert!(matches!(
        resolve(&store, &duplicate, &config()).unwrap(),
        Resolution::Rejected(Reject::Resolve(OutPointError::Dead(point)), _) if point == input
    ));
    let group = ckb_types::core::TransactionBuilder::default()
        .version(1803u32)
        .output(CellOutput::default())
        .output_data(Bytes::from_static(b"invalid group").pack())
        .build();
    accept(&store, group.clone(), 1, 1, Status::Pending);
    let group_point = OutPoint::new(group.hash(), 0);
    let candidate = entry(
        &store,
        funded_tx(input, 19_999_998_000)
            .as_advanced_builder()
            .cell_dep(
                CellDep::new_builder()
                    .out_point(group_point.clone())
                    .dep_type(DepType::DepGroup)
                    .build(),
            )
            .build(),
        Source::Local,
    );
    assert!(matches!(
        resolve(&store, &candidate, &config()).unwrap(),
        Resolution::Rejected(Reject::Resolve(OutPointError::InvalidDepGroup(point)), _) if point == group_point
    ));
    assert!(store.point(&incumbent).1.unwrap().accepted().is_some());
    assert!(!store.is_faulted());
}
