use super::*;
use crate::authority::{
    model::{RemoteOrigin, Source},
    tests::common::*,
};
use crate::verification::ComputeMode;
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, cell::ResolvedTransaction},
    packed::{CellDep, CellOutput, OutPoint, OutPointVec},
    prelude::*,
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
    TxVerificationCacheKey::from_resolved(
        &ResolvedTransaction::dummy_resolve(candidate.transaction.as_ref().clone()),
        ScriptVerificationRules::from_env(
            snapshot.consensus(),
            &environment(Status::Pending, snapshot),
        ),
    )
}

#[test]
fn only_network_sources_select_a_time_budget() {
    let config = config();
    let cap = std::time::Duration::from_millis(u64::from(config.max_tx_verify_time_ms));
    assert_eq!(Source::Local.verification_time_limit(&config), None);
    assert_eq!(Source::Recovery.verification_time_limit(&config), None);
    assert_eq!(
        Source::Proposal { remote: None }.verification_time_limit(&config),
        Some(cap)
    );
    let remote = |cycles| Source::Remote {
        origin: RemoteOrigin {
            peer: ckb_network::PeerIndex::from(1),
            deadline: std::time::Instant::now(),
        },
        cycles,
    };
    assert_eq!(remote(None).verification_time_limit(&config), Some(cap));
    let limit = remote(Some(1)).verification_time_limit(&config).unwrap();
    assert!(!limit.is_zero());
    assert!(limit <= cap);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dao_lock_size_is_checked_before_local_and_network_script_execution() {
    use crate::verification::TxPoolVerificationBudget;
    use ckb_types::{
        core::{
            ScriptHashType, TransactionBuilder, cell::CellMetaBuilder, error::TransactionError,
        },
        packed::{CellInput, Script},
    };
    use std::time::Duration;

    let snapshot = chain_snapshot();
    let dao = Script::new_builder()
        .code_hash(snapshot.consensus().dao_type_hash())
        .hash_type(ScriptHashType::Type)
        .build();
    let deposit = CellOutput::new_builder()
        .capacity(20_000_000_000u64)
        .type_(Some(dao).pack())
        .build();
    let withdrawing = deposit
        .clone()
        .as_builder()
        .lock(
            Script::new_builder()
                .args(Bytes::from_static(b"longer lock").pack())
                .build(),
        )
        .build();
    let rtx = Arc::new(ResolvedTransaction {
        transaction: TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(tx(9600).hash(), 0), 0))
            .output(withdrawing)
            .output_data(Bytes::from_static(&[1; 8]).pack())
            .build(),
        resolved_inputs: vec![
            CellMetaBuilder::from_cell_output(deposit, Bytes::from_static(&[0; 8])).build(),
        ],
        resolved_cell_deps: vec![],
        resolved_dep_groups: vec![],
    });
    let env = Arc::new(environment(Status::Pending, &snapshot));
    for budget in [
        None,
        Some(TxPoolVerificationBudget::new(
            Duration::from_secs(1),
            ComputeMode::Inline,
        )),
        Some(TxPoolVerificationBudget::new(
            Duration::from_secs(1),
            ComputeMode::YieldRuntimeWorker,
        )),
    ] {
        let (_sender, mut commands) = watch::channel(ChunkCommand::Resume);
        // No script deps and no VM cycles are available. Reaching script
        // verification would produce a different error before the DAO check.
        let reject = verify_rtx(
            Arc::clone(&snapshot),
            Arc::clone(&rtx),
            Arc::clone(&env),
            None,
            0,
            &mut commands,
            budget,
        )
        .await
        .unwrap_err();
        let Reject::Verification(error) = reject else {
            panic!("expected the DAO verification error, got {reject:?}");
        };
        ckb_error::assert_error_eq!(error, TransactionError::DaoLockSizeMismatch { index: 0 });
    }
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
        &ComputePermit::for_test(ComputeMode::Inline),
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
        &ComputePermit::for_test(ComputeMode::Inline),
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
async fn local_and_recovery_verification_ignore_network_time_limit() {
    let (store, candidate, _) = fixture();
    // A zero limit distinguishes policy without depending on VM speed. It is
    // passed directly to verification, not through service configuration.
    let config = TxPoolConfig {
        max_tx_verify_time_ms: 0,
        ..config()
    };
    let resolved = resolved(&store, &candidate, &config);
    for source in [Source::Local, Source::Recovery] {
        let candidate = Entry {
            source,
            ..candidate.as_ref().clone()
        };
        let cache = RwLock::new(init_cache());
        let (_sender, mut commands) = watch::channel(ChunkCommand::Resume);
        let verified = verify(
            &store,
            &candidate,
            Arc::clone(&resolved),
            &config,
            &cache,
            &mut commands,
            &ComputePermit::for_test(ComputeMode::YieldRuntimeWorker),
        )
        .await
        .unwrap();
        assert!(verified.cycles() > 0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_time_budget_refusal_is_not_cached_and_allows_a_later_normal_attempt() {
    let (store, candidate, snapshot) = fixture();
    let peer = ckb_network::PeerIndex::from(1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let candidate = Entry {
        source: Source::Remote {
            origin: RemoteOrigin { peer, deadline },
            cycles: None,
        },
        ..candidate.as_ref().clone()
    };
    let mut config = config();
    let resolved = resolved(&store, &candidate, &config);
    let cache = RwLock::new(init_cache());
    let (_sender, mut commands) = watch::channel(ChunkCommand::Resume);
    // Exercise the verifier's zero-budget boundary directly; this is not a
    // service configuration and does not depend on how fast the fixture runs.
    config.max_tx_verify_time_ms = 0;
    for source in [
        candidate.source,
        Source::Remote {
            origin: RemoteOrigin { peer, deadline },
            cycles: Some(1),
        },
        Source::Proposal { remote: None },
        Source::Proposal {
            remote: Some(RemoteOrigin { peer, deadline }),
        },
    ] {
        let attempted = Entry {
            source,
            ..candidate.clone()
        };
        let result = verify(
            &store,
            &attempted,
            Arc::clone(&resolved),
            &config,
            &cache,
            &mut commands,
            &ComputePermit::for_test(ComputeMode::Inline),
        )
        .await;
        assert!(
            matches!(result, Err(Error::Rejected(Reject::ExcessiveVerifyTime))),
            "{source:?}"
        );
        assert!(
            cache
                .read()
                .await
                .lookup(&key(&candidate, &snapshot))
                .is_none()
        );
    }
    config.max_tx_verify_time_ms = 8_000;
    assert!(
        verify(
            &store,
            &candidate,
            resolved,
            &config,
            &cache,
            &mut commands,
            &ComputePermit::for_test(ComputeMode::Inline)
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
        &ComputePermit::for_test(ComputeMode::Inline),
    )
    .await
    .unwrap();
    let wrong = Entry {
        source: Source::Remote {
            origin: RemoteOrigin {
                peer: ckb_network::PeerIndex::from(1),
                deadline: std::time::Instant::now() + std::time::Duration::from_secs(30),
            },
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
            &ComputePermit::for_test(ComputeMode::Inline)
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
            &ComputePermit::for_test(ComputeMode::Inline)
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
fn resolution_rejects_unknown_inputs_and_retains_spender_observations() {
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
    let spender = accept(
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
            .cell_dep(CellDep::new_builder().out_point(input.clone()).build())
            .build(),
        Source::Local,
    );
    let backing = resolved(&store, &reader, &config());
    assert!(
        backing
            .reads
            .spent()
            .any(|(point, hash)| point == &input && hash == &spender)
    );
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
        &ComputePermit::for_test(ComputeMode::Inline),
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
    let before = store.capture_all().owners;
    let usage = store.budget.owner_usage();
    let prepared =
        crate::authority::membership::admission(&store, &candidate, None, &proof, &config, true);
    match prepared {
        Err(Error::Stale) => {}
        Ok((plan, reject)) => {
            let applied = store.apply(plan);
            assert!(
                matches!(applied, Err(Error::Stale)) || (reject.is_some() && applied.is_ok()),
                "a previously live dependency was spent before the candidate commit"
            );
        }
        _ => panic!("unexpected admission result"),
    }
    assert!(store.point(&candidate.hash()).1.is_none());
    assert_eq!(store.capture_all().owners.len(), before.len());
    for owner in before {
        assert!(Arc::ptr_eq(&store.point(&owner.hash()).1.unwrap(), &owner));
    }
    assert_eq!(store.budget.owner_usage(), usage);
    assert!(!store.is_faulted());
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
    let first_missing_output = parent.transaction.outputs().len().try_into().unwrap();
    for (index, may_wait) in [(0, true), (first_missing_output, false), (9, false)] {
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
fn network_missing_dependencies_wait_independently_of_proposal_and_cycles() {
    let store = Store::new(chain_snapshot(), &config()).unwrap();
    let remote = super::super::ingress::remote_source(98.into(), 1).unwrap();
    let Source::Remote { origin, .. } = remote else {
        unreachable!()
    };
    let absent = OutPoint::new(tx(1804).hash(), 0);
    let parent = funded_parent(1805, 20_000_000_000);
    accept(&store, parent.clone(), 1, 1, Status::Pending);
    for source in [
        remote,
        Source::Remote {
            origin,
            cycles: None,
        },
        Source::Proposal {
            remote: Some(origin),
        },
        Source::Proposal { remote: None },
    ] {
        for is_dep in [false, true] {
            let transaction = if is_dep {
                funded_tx(OutPoint::new(parent.hash(), 0), 19_999_999_000)
                    .as_advanced_builder()
                    .cell_dep(CellDep::new_builder().out_point(absent.clone()).build())
                    .build()
            } else {
                funded_tx(absent.clone(), 19_999_999_000)
            };
            let candidate = entry(&store, transaction, source);
            let Resolution::Waiting(keys, _) = resolve(&store, &candidate, &config()).unwrap()
            else {
                panic!("network work must retain its missing frontier");
            };
            assert_eq!(keys, BTreeSet::from([DependencyKey::Cell(absent.clone())]));
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
    assert!(store.capture_all().owners.is_empty());
    assert!(!store.is_faulted());
}

use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_test_chain_utils::MockStore;
use ckb_types::{U256, core::EpochNumberWithFraction};

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
        &ComputePermit::for_test(ComputeMode::Inline),
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
