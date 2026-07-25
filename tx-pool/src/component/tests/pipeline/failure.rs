use super::*;

#[test]
fn unusable_pipeline_residency_budget_fails_at_startup() {
    let (consensus, _) = test_consensus(1);
    let mut config = tx_pool_config();
    config.max_tx_pipeline_resident_size = 0;
    let shutdown = CancellationToken::new();
    let result = catch_unwind(AssertUnwindSafe(|| {
        crate::component::pipeline_runtime::PipelineRuntime::new(&config, &consensus, shutdown)
    }));
    assert!(
        result.is_err(),
        "zero must not be silently promoted to an unusable one-byte budget"
    );
}

#[test]
fn pipeline_runtime_panics_fail_closed_instead_of_recovering_poisoned_state() {
    let (consensus, _) = test_consensus(1);
    let shutdown = CancellationToken::new();
    let runtime = crate::component::pipeline_runtime::PipelineRuntime::new(
        &tx_pool_config(),
        &consensus,
        shutdown.clone(),
    );
    let tx = TransactionBuilder::default().build();

    let injected = catch_unwind(AssertUnwindSafe(|| {
        let _ = runtime.admit_transaction_journaled(
            tx,
            TxSource::Local,
            0,
            crate::component::pipeline_coordinator::RawStage::PreCheck,
            |_| panic!("injected journal failure after coordinator admission"),
        );
    }));
    assert!(
        injected.is_err(),
        "the injected panic must escape the boundary"
    );
    assert!(runtime.is_failed(), "the runtime must latch failure");
    assert!(
        runtime.pool_persistence_safe(),
        "a coordinator-only panic must not discard a coherent accepted pool"
    );
    assert!(
        shutdown.is_cancelled(),
        "a fatal coordinator failure must stop the tx-pool service generation"
    );

    let reused = catch_unwind(AssertUnwindSafe(|| runtime.read(|_| ())));
    assert!(
        reused.is_err(),
        "poisoned coordinator state must never be recovered into service"
    );
}

#[test]
fn authoritative_boundary_failure_disables_pool_persistence() {
    let (consensus, _) = test_consensus(1);
    let shutdown = CancellationToken::new();
    let runtime = crate::component::pipeline_runtime::PipelineRuntime::new(
        &tx_pool_config(),
        &consensus,
        shutdown.clone(),
    );

    let injected = catch_unwind(AssertUnwindSafe(|| {
        runtime.guard_authoritative_mutation("injected pool boundary", || {
            panic!("injected partial PoolMap mutation")
        });
    }));
    assert!(injected.is_err());
    assert!(runtime.is_failed());
    assert!(shutdown.is_cancelled());
    assert!(
        !runtime.pool_persistence_safe(),
        "an interrupted authoritative pool mutation is not a recovery point"
    );
}

#[test]
fn stable_effect_journal_failure_preserves_pool_persistence() {
    let (consensus, _) = test_consensus(1);
    let shutdown = CancellationToken::new();
    let runtime = crate::component::pipeline_runtime::PipelineRuntime::new(
        &tx_pool_config(),
        &consensus,
        shutdown.clone(),
    );

    let injected = catch_unwind(AssertUnwindSafe(|| {
        runtime.guard_stable_effect_journal("injected stable effect boundary", || {
            panic!("injected effect journal failure")
        });
    }));
    assert!(injected.is_err());
    assert!(runtime.is_failed());
    assert!(shutdown.is_cancelled());
    assert!(
        runtime.pool_persistence_safe(),
        "effect publication cannot invalidate an already-stable accepted pool"
    );
}

#[test]
fn inconsistent_ingress_source_attribution_is_fail_closed() {
    use crate::component::pipeline_coordinator::CoordinatorSource;
    use crate::component::pipeline_runtime::PipelineRawTx;

    let (consensus, _) = test_consensus(1);
    let shutdown = CancellationToken::new();
    let runtime = crate::component::pipeline_runtime::PipelineRuntime::new(
        &tx_pool_config(),
        &consensus,
        shutdown.clone(),
    );
    let raw = PipelineRawTx::new(TransactionBuilder::default().build(), TxSource::Local, 0);

    let mismatch = catch_unwind(AssertUnwindSafe(|| {
        runtime.require_authoritative_source(
            &raw,
            CoordinatorSource::Remote(ckb_network::PeerIndex::from(7)),
        );
    }));
    assert!(mismatch.is_err());
    assert!(runtime.is_failed());
    assert!(shutdown.is_cancelled());
}

#[test]
fn coordinator_invariant_error_cannot_be_downgraded_to_transaction_reject() {
    use crate::component::pipeline_coordinator::{
        CoordinatorError, CoordinatorLocation, QueueKind, RawStage,
    };

    assert!(
        !CoordinatorError::LocationMismatch {
            expected: CoordinatorLocation::VerifyActive,
            actual: CoordinatorLocation::RawActive(RawStage::Resolve),
        }
        .is_stale_lease(),
        "a matching-version location mismatch is an internal protocol failure"
    );

    let (consensus, _) = test_consensus(1);
    let shutdown = CancellationToken::new();
    let runtime = crate::component::pipeline_runtime::PipelineRuntime::new(
        &tx_pool_config(),
        &consensus,
        shutdown.clone(),
    );
    let failed = catch_unwind(AssertUnwindSafe(|| {
        runtime.reject_or_fail(
            "injected production adapter invariant",
            CoordinatorError::QueueInvariant(QueueKind::Resolve),
        );
    }));
    assert!(failed.is_err());
    assert!(runtime.is_failed());
    assert!(shutdown.is_cancelled());
}

#[test]
fn weaker_duplicate_source_cannot_amplify_into_pipeline_fail_stop() {
    use crate::component::pipeline_coordinator::{CoordinatorSource, RawStage};

    let (consensus, _) = test_consensus(1);
    let shutdown = CancellationToken::new();
    let runtime = crate::component::pipeline_runtime::PipelineRuntime::new(
        &tx_pool_config(),
        &consensus,
        shutdown.clone(),
    );
    let proposal = TransactionBuilder::default()
        .witness(Bytes::from_static(b"proposal"))
        .build();
    let hash = proposal.hash();
    let proposal_witness = proposal.witness_hash();
    let local_variant = proposal
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"local-history").pack()])
        .build();
    assert_eq!(local_variant.hash(), hash);
    assert_ne!(local_variant.witness_hash(), proposal_witness);

    assert!(
        runtime
            .admit_transaction(proposal, TxSource::Proposal, 0, RawStage::PreCheck)
            .expect("proposal admission")
            .0
    );
    let (added, terminal) = runtime
        .admit_transaction(local_variant, TxSource::Local, 0, RawStage::Resolve)
        .expect("weaker duplicate is an ownership no-op");
    assert!(!added);
    assert!(terminal.is_empty());
    runtime.read(|coordinator| {
        let view = coordinator.view(&hash).expect("proposal owner remains");
        assert_eq!(view.source, CoordinatorSource::Proposal);
        assert_eq!(
            coordinator
                .raw_by_hash(&hash)
                .expect("raw payload")
                .tx
                .witness_hash(),
            proposal_witness,
            "a weaker duplicate cannot replace the authoritative witness"
        );
    });
    assert!(!runtime.is_failed());
    assert!(!shutdown.is_cancelled());
}

#[test]
fn retryable_capacity_classification_excludes_fixed_payload_limits() {
    use crate::component::pipeline_coordinator::CoordinatorError;

    assert!(
        CoordinatorError::ParentFanoutLimitExceeded(ckb_types::packed::Byte32::zero())
            .is_retryable_capacity_rejection()
    );
    assert!(CoordinatorError::GlobalBudgetExceeded.is_retryable_capacity_rejection());
    assert!(CoordinatorError::DependencyLimitExceeded.is_capacity_rejection());
    assert!(CoordinatorError::ConflictInputLimitExceeded.is_capacity_rejection());
    assert!(CoordinatorError::ResidencyChargeOverflow.is_capacity_rejection());
    assert!(
        !CoordinatorError::DependencyLimitExceeded.is_retryable_capacity_rejection(),
        "an identical payload cannot retry its way below a fixed dependency limit"
    );
    assert!(!CoordinatorError::ConflictInputLimitExceeded.is_retryable_capacity_rejection());
    assert!(!CoordinatorError::ResidencyChargeOverflow.is_retryable_capacity_rejection());
}

#[test]
fn rejected_commit_terminal_failure_is_fail_closed_not_best_effort() {
    use crate::component::entry::resolved_transaction_charge_bytes;
    use crate::component::pipeline_coordinator::{
        CoordinatorFeeGate, RawStage, TerminalDisposition, VerifySchedule, WorkerCapability,
    };
    use crate::component::pipeline_runtime::PipelineVerifiedTx;
    use crate::resolved_tx::ResolvedTx;
    use ckb_types::core::cell::ResolvedTransaction;
    use std::collections::HashSet;
    use std::time::Instant;

    let (consensus, out_points) = test_consensus(1);
    let (_store, snapshot) = snapshot_with_genesis(Arc::new(consensus.clone()));
    let shutdown = CancellationToken::new();
    let runtime = crate::component::pipeline_runtime::PipelineRuntime::new(
        &tx_pool_config(),
        &consensus,
        shutdown.clone(),
    );
    let tx = build_tx(&out_points[0], 4_000);
    let hash = tx.hash();
    runtime
        .admit_transaction(tx.clone(), TxSource::Local, 0, RawStage::PreCheck)
        .unwrap();
    let raw_lease = runtime.checkout_raw(RawStage::PreCheck).unwrap();
    let rtx = Arc::new(ResolvedTransaction::dummy_resolve(tx.clone()));
    let tx_size = tx.data().serialized_size_in_block();
    let resident_size = resolved_transaction_charge_bytes(tx_size, &rtx);
    let resolved = ResolvedTx {
        tx: tx.clone(),
        rtx,
        status: Status::Pending,
        fee: Capacity::zero(),
        tx_size,
        resident_size,
        pre_resolve_tip: snapshot.tip_hash(),
        source: TxSource::Local,
        epoch: 0,
    };
    runtime
        .mutate(|coordinator| {
            coordinator.complete_raw(
                &raw_lease,
                resolved,
                resident_size,
                VerifySchedule::default(),
            )
        })
        .unwrap();
    let verify_lease = runtime
        .mutate(|coordinator| coordinator.checkout_verify(WorkerCapability::Any))
        .unwrap()
        .unwrap();
    let candidate = (*verify_lease.payload).clone().into_pool_candidate();
    let candidate_charge = candidate.resident_size;
    let meta = CoordinatorFeeGate::new(0, 0)
        .validate(
            hash.clone(),
            tx.input_pts_iter().collect::<HashSet<_>>(),
            0,
            tx_size,
        )
        .unwrap();
    runtime
        .mutate(|coordinator| {
            coordinator.complete_verification_candidate(
                &verify_lease,
                PipelineVerifiedTx {
                    candidate,
                    completed: Completed {
                        cycles: 0,
                        fee: Capacity::zero(),
                    },
                    verify_cache_hit: false,
                    started_at: Instant::now(),
                },
                candidate_charge,
                meta,
            )
        })
        .unwrap();
    let commit = runtime.mutate_required("test commit checkout", |coordinator| {
        coordinator.begin_next_commit()
    });
    let commit = commit.unwrap();

    // Inject the report's closest version-mismatch leaf after checkout. The
    // coordinator transaction correctly leaves the entry Committing on Err;
    // production policy must therefore stop the service instead of warning
    // and leaking the active slot indefinitely.
    runtime.mutate(|coordinator| {
        coordinator
            .set_revision_for_test(&hash, commit.version.revision + 1)
            .unwrap();
    });
    let failure = catch_unwind(AssertUnwindSafe(|| {
        runtime.mutate_required(
            "rejected pipeline commit could not leave Committing",
            |state| state.fail_commit(&commit, TerminalDisposition::Rejected),
        );
    }));
    assert!(failure.is_err());
    assert!(runtime.is_failed());
    assert!(shutdown.is_cancelled());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pool_commit_panic_fails_closed_instead_of_stranding_committing() {
    let (service, _relay, signal, _store, issue_out_points) = service_with_pipeline(1);
    let tx = build_tx(&issue_out_points[0], 4_000);
    let cycles = measured_cycles(&service, tx.clone()).await;
    service
        .pool
        .tx_pool
        .write()
        .await
        .fail_next_pool_commit_panic = true;

    submit_remote(&service, tx, cycles, 1.into())
        .await
        .expect("fault-injected transaction should reach the asynchronous pipeline");

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if service.pipeline.runtime.is_failed() && signal.is_cancelled() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("an authoritative pool panic must stop the complete tx-pool service");
    assert!(
        !service.pipeline.runtime.pool_persistence_safe(),
        "an unwind inside the PoolMap mutation boundary has no proven recovery point"
    );
}

/// Exercise the recoverable error edge of the real production cutover, not
/// only the isolated coordinator undo. The injected error is raised after the
/// coordinator handoff has run its complete apply path; its outer undo must
/// restore `Committing`, the PoolMap journal must remove the tentative insert,
/// required failure settlement must consume the coordinator owner, and the
/// reserved outbox credit must publish exactly that stable terminal result.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_pool_coordinator_outbox_fault_matrix_is_atomic() {
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::service::TxVerificationResult;

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let tx_hash = tx.hash();
    let peer = ckb_network::PeerIndex::from(41);
    stage_verified_remote_candidate(&h.service, tx, peer).await;
    h.service.pipeline.runtime.mutate(|coordinator| {
        coordinator.fail_next_handoff_after_apply_for_test(
            crate::component::pipeline_coordinator::CoordinatorError::QueueReservationFailed,
        );
    });

    h.service.drive_pipeline_commits().await;

    assert!(
        h.service
            .pool
            .tx_pool
            .read()
            .await
            .get_tx_from_pool_by_hash(&tx_hash)
            .is_none(),
        "the tentative PoolMap insert must be rolled back before unlock"
    );
    h.service.pipeline.runtime.read(|coordinator| {
        assert!(
            !coordinator.contains_hash(&tx_hash),
            "required failed-commit settlement must consume restored Committing ownership"
        );
        coordinator.audit().unwrap();
    });
    assert!(
        !h.service.pipeline.runtime.is_failed(),
        "an error returned through a proven undo boundary is a settled attempt, not poisoned state"
    );

    let relayed = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(result) = h.relay_rx.try_recv() {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the stable failed outcome must consume its reserved effect credit");
    assert!(matches!(
        relayed,
        TxVerificationResult::Reject { tx_hash: rejected } if rejected == tx_hash
    ));
    h.service.relay.effects.wait_idle().await;
    let effect_usage = h.service.relay.effects.usage();
    assert_eq!(effect_usage.batches, 0);
    assert_eq!(
        effect_usage.bytes, 0,
        "publication must release the complete reserved/queued/active charge"
    );

    h.cancel.cancel();
}

/// A panic inside an undo-protected coordinator handoff stops the pipeline,
/// but the exact PoolMap journal still proves the accepted pool is a safe
/// recovery point. The commit driver must not re-enter the already failed
/// runtime and accidentally escalate this to authoritative uncertainty.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn coordinator_handoff_panic_preserves_pool_recovery_point() {
    use crate::component::tests::harness::{WorkerSet, harness};

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let tx_hash = tx.hash();
    stage_verified_remote_candidate(&h.service, tx, ckb_network::PeerIndex::from(42)).await;
    h.service.pipeline.runtime.mutate(|coordinator| {
        coordinator.set_apply_fault_for_test(Some(1));
    });

    h.service.drive_pipeline_commits().await;

    assert!(h.service.pipeline.runtime.is_failed());
    assert!(h.cancel.is_cancelled());
    assert!(
        h.service.pipeline.runtime.pool_persistence_safe(),
        "coordinator undo plus exact PoolMap rollback must not be escalated to authoritative failure"
    );
    assert!(
        h.service
            .pool
            .tx_pool
            .read()
            .await
            .get_tx_from_pool_by_hash(&tx_hash)
            .is_none(),
        "the tentative pool insertion must be gone before the failed driver exits"
    );
    h.service.relay.effects.wait_idle().await;
    let effect_usage = h.service.relay.effects.usage();
    assert_eq!(effect_usage.batches, 0);
    assert_eq!(effect_usage.bytes, 0);
    assert!(
        h.relay_rx.try_recv().is_err(),
        "an interrupted attempt has no stable terminal effect to publish"
    );
}

/// A returned invariant error is different from a capacity-class rollback:
/// first restore PoolMap, settle the live commit lease and bind its terminal
/// effect, then stop the service outside the authoritative lock domain.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_handoff_invariant_settles_then_fails_closed() {
    use crate::component::pipeline_coordinator::CoordinatorError;
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::service::TxVerificationResult;

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let tx_hash = tx.hash();
    stage_verified_remote_candidate(&h.service, tx, ckb_network::PeerIndex::from(43)).await;
    h.service.pipeline.runtime.mutate(|coordinator| {
        coordinator.fail_next_handoff_after_apply_for_test(CoordinatorError::ConflictInvariant);
    });

    let service = h.service.clone();
    let join = tokio::spawn(async move { service.drive_pipeline_commits().await }).await;
    assert!(join.is_err_and(|error| error.is_panic()));
    assert!(h.service.pipeline.runtime.is_failed());
    assert!(h.cancel.is_cancelled());
    assert!(
        h.service.pipeline.runtime.pool_persistence_safe(),
        "exact rollback completes before invariant fail-stop leaves the pool guard"
    );
    assert!(
        h.service
            .pool
            .tx_pool
            .read()
            .await
            .get_tx_from_pool_by_hash(&tx_hash)
            .is_none()
    );
    let relayed = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(result) = h.relay_rx.try_recv() {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("lease settlement must be journaled before invariant fail-stop");
    assert!(matches!(
        relayed,
        TxVerificationResult::Reject { tx_hash: rejected } if rejected == tx_hash
    ));
}
