use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_processes_independent_remote_txs() {
    let (service, _relay, signal, _store, issue_out_points) = service_with_pipeline(5);

    let txs: Vec<_> = issue_out_points
        .iter()
        .map(|out_point| build_tx(out_point, 4_000))
        .collect();

    for tx in &txs {
        let cycles = measured_cycles(&service, tx.clone()).await;
        submit_remote(&service, tx.clone(), cycles, 1.into())
            .await
            .expect("enqueue remote tx should succeed");
    }

    wait_for_pending(&service, txs.len(), Duration::from_secs(10))
        .await
        .expect("pipeline should process all independent txs in time");

    let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(pending, txs.len());

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// A non-contextual remote rejection happens before coordinator admission,
/// but it still crosses the same terminal boundary as a later verifier
/// rejection. Malformed transactions are deliberately not announced as
/// retryable relayer rejections; banning the peer and recording the public
/// rejection are the stable consequences.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_remote_preflight_is_banned_recorded_and_not_relayed() {
    use crate::component::recent_reject::RecentReject;
    use crate::component::tests::harness::{WorkerSet, harness};

    let temp = tempfile::Builder::new().tempdir().unwrap();
    let recent_reject = Arc::new(RecentReject::build(temp.path(), 1, 100, -1).unwrap());
    let mut h = harness(0).workers(WorkerSet::None).build();
    h.service.aux.recent_reject = Some(Arc::clone(&recent_reject));

    let tx = TransactionBuilder::default().build();
    let tx_hash = tx.hash();
    let peer = ckb_network::PeerIndex::from(91);
    let source = TxSource::Remote { cycles: 0, peer };
    let reject = h
        .service
        .submit_remote_tx(tx, source)
        .await
        .expect_err("an empty loose transaction must fail non-contextual verification");
    assert!(reject.is_malformed_tx());

    h.service.relay.effects.wait_idle().await;
    assert!(h.service.is_recently_banned(source));
    assert!(recent_reject.get(&tx_hash).unwrap().is_some());
    assert!(
        h.relay_rx.try_recv().is_err(),
        "malformed transactions are not eligible for relayer retry"
    );

    h.cancel.cancel();
}

/// Local RPC submission is intentionally synchronous. An older asynchronous
/// remote owner for the same hash must neither turn the local call into a
/// duplicate error nor survive after the local transaction enters TxPool.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_submit_bypasses_and_settles_matching_remote_owner() {
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::service::TxVerificationResult;

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    let id = tx.proposal_short_id();
    let peer = ckb_network::PeerIndex::from(1);

    h.service
        .submit_remote_tx(tx.clone(), TxSource::Remote { cycles: 0, peer })
        .await
        .expect("remote copy enters the coordinator");
    assert!(
        h.service
            .pipeline
            .kernel
            .read(|coordinator| coordinator.contains_hash(&hash)),
        "the no-worker harness must leave the remote copy coordinator-owned"
    );

    let completed = h
        .service
        .process_tx(tx, TxSource::Local)
        .await
        .expect("local submission must execute synchronously");
    assert!(completed.cycles > 0);
    assert!(
        h.service
            .pool
            .tx_pool
            .read()
            .await
            .get_tx_from_pool(&id)
            .is_some(),
        "the local call must return only after authoritative pool insertion"
    );
    assert!(
        !h.service
            .pipeline
            .kernel
            .read(|coordinator| coordinator.contains_hash(&hash)),
        "successful local insertion must invalidate the older async owner"
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
    .expect("local handoff must settle the consumed remote ingress");
    assert!(matches!(
        relayed,
        TxVerificationResult::Ok {
            original_peer: Some(relayed_peer),
            tx_hash,
        } if relayed_peer == peer && tx_hash == hash
    ));

    h.cancel.cancel();
}

/// Candidate checkout is part of the authoritative TxPool write transaction.
/// If it happened before waiting for that guard, a synchronous Local/clear/
/// reorg handoff could consume the Ready owner while the old driver was
/// already carrying a commit ticket. Its later failure settlement would then
/// confuse a legitimate stale ticket with kernel corruption.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pipeline_commit_worker_waits_for_the_pool_sequencer() {
    use crate::component::pre_pool::PrePoolLocation;
    use crate::component::tests::harness::{WorkerSet, harness};

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    stage_verified_remote_candidate(&h.service, tx, 1.into()).await;
    assert_eq!(
        h.service
            .pipeline
            .kernel
            .read(|coordinator| coordinator.view(&hash).unwrap().location),
        PrePoolLocation::Ready
    );

    let pool_guard = h.service.pool.tx_pool.write().await;
    let commit_cancel = h.cancel.child_token();
    let service = h.service.clone();
    let driver = tokio::spawn(crate::service::workers::run_pipeline_commit_worker(
        service,
        commit_cancel,
    ));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        h.service
            .pipeline
            .kernel
            .read(|coordinator| coordinator.view(&hash).unwrap().location),
        PrePoolLocation::Ready,
        "waiting for TxPool must not consume the Ready owner"
    );
    assert!(!driver.is_finished());

    drop(pool_guard);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if h.service
                .pool
                .tx_pool
                .read()
                .await
                .get_tx_from_pool_by_hash(&hash)
                .is_some()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("commit worker resumes after the pool sequencer is released");
    assert!(
        h.service
            .pool
            .tx_pool
            .read()
            .await
            .get_tx_from_pool_by_hash(&hash)
            .is_some()
    );
    assert!(!h.service.pipeline.kernel.is_failed());
    h.cancel.cancel();
    tokio::time::timeout(Duration::from_secs(1), driver)
        .await
        .expect("commit worker observes cancellation")
        .expect("commit worker does not panic");
}

/// The early duplicate check can become stale before pipeline admission. The
/// authoritative admission boundary must recheck TxPool while holding its read
/// guard across the coordinator mutation, so a transaction committed in that
/// window is never shadowed by a second pre-pool owner.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_precheck_cannot_readmit_an_already_accepted_transaction() {
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::service::TxVerificationResult;

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    h.service
        .process_tx(tx.clone(), TxSource::Local)
        .await
        .expect("local transaction enters the authoritative pool");

    // Consume the local success publication before observing the synthetic
    // stale-precheck ingress below.
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(result) = h.relay_rx.try_recv() {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("local success is published");

    let peer = ckb_network::PeerIndex::from(17);
    assert!(
        !h.service
            .classify_and_enqueue_tx_spawn(tx, TxSource::Remote { cycles: 0, peer },)
            .await
            .expect("already accepted ingress settles without readmission")
    );
    assert!(
        !h.service
            .pipeline
            .kernel
            .read(|coordinator| coordinator.contains_hash(&hash)),
        "TxPool and coordinator must never both own the same hash"
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
    .expect("the stale remote ingress receives a terminal settlement");
    assert!(matches!(
        relayed,
        TxVerificationResult::Ok {
            original_peer: Some(relayed_peer),
            tx_hash,
        } if relayed_peer == peer && tx_hash == hash
    ));

    h.cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_unverified_remote_owner_is_not_acknowledged_as_accepted() {
    use crate::component::tests::harness::{WorkerSet, harness};

    let h = harness(1).workers(WorkerSet::None).build();
    let first = build_tx(&h.out_points[0], 4_000)
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"first").pack()])
        .build();
    let second = first
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"second").pack()])
        .build();
    assert_eq!(first.hash(), second.hash());
    assert_ne!(first.witness_hash(), second.witness_hash());

    submit_remote(&h.service, first, 0, 19.into())
        .await
        .unwrap();
    assert!(matches!(
        submit_remote(&h.service, second, 0, 20.into()).await,
        Err(crate::error::Reject::Duplicated(_))
    ));
    tokio::task::yield_now().await;
    assert!(
        h.relay_rx.try_recv().is_err(),
        "a merely coordinator-owned raw hash has no successful result yet"
    );

    h.cancel.cancel();
}

/// Proposal notification upgrades an existing remote owner in place. The
/// old peer can then be banned without revoking the trusted transaction, and
/// a lease checked out before promotion must settle under the new source.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proposal_promotes_active_remote_owner_and_detaches_peer_ban() {
    use crate::component::pre_pool::{PrePoolLocation, PrePoolSource, ResolveLane};
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::service::TxVerificationResult;

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    let peer = ckb_network::PeerIndex::from(7);
    h.service
        .submit_remote_tx(tx.clone(), TxSource::Remote { cycles: 0, peer })
        .await
        .unwrap();
    assert_eq!(
        h.service
            .pipeline
            .kernel
            .read(|coordinator| coordinator.deadline_len()),
        1
    );
    let lease = h
        .service
        .pipeline
        .kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap();

    assert!(
        !h.service
            .notify_tx(tx)
            .await
            .expect("proposal promotes the existing hash")
    );
    assert_eq!(
        h.service
            .pipeline
            .kernel
            .read(|coordinator| coordinator.view(&hash).unwrap().source),
        PrePoolSource::Proposal
    );
    assert_eq!(
        h.service
            .pipeline
            .kernel
            .read(|coordinator| coordinator.deadline_len()),
        0,
        "trusted promotion must cancel the obsolete remote expiry"
    );
    h.service
        .ban_malformed(peer, "test old remote owner ban".to_string())
        .await;
    h.service.process_pipeline_raw_lease(lease).await;
    let view = h
        .service
        .pipeline
        .kernel
        .read(|coordinator| coordinator.view(&hash).unwrap());
    assert_eq!(view.source, PrePoolSource::Proposal);
    assert_eq!(view.location, PrePoolLocation::VerifyQueued);

    let verify = h
        .service
        .pipeline
        .kernel
        .mutate(|coordinator| {
            coordinator.checkout_verify(crate::component::pre_pool::WorkCapability::Any)
        })
        .unwrap()
        .unwrap();
    let mut chunk_rx = h.service.pipeline.chunk_rx.clone();
    h.service
        .process_pipeline_verify_lease(verify, &mut chunk_rx)
        .await;
    let relayed = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(result) = h.relay_rx.try_recv() {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("promoted remote ingress receives one successful settlement");
    assert!(
        matches!(
            relayed,
            TxVerificationResult::Ok {
                original_peer: Some(relayed_peer),
                tx_hash,
            } if relayed_peer == peer && tx_hash == hash
        ),
        "trusted scheduling priority must not erase immutable relay attribution"
    );

    h.cancel.cancel();
}

/// A peer ban must revoke an already checked-out remote owner, release its
/// active-work accounting and make the worker's lease stale in one
/// coordinator transition. Otherwise an attacker can keep its budget slot
/// resident until the expensive worker eventually returns.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn banned_peer_revokes_active_remote_lease_and_releases_budget() {
    use crate::component::pre_pool::ResolveLane;
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::service::TxVerificationResult;

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    let peer = ckb_network::PeerIndex::from(17);
    h.service
        .submit_remote_tx(tx, TxSource::Remote { cycles: 0, peer })
        .await
        .unwrap();
    let lease = h
        .service
        .pipeline
        .kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap();
    assert_eq!(
        h.service
            .pipeline
            .kernel
            .read(|coordinator| coordinator.peer_active_work(peer)),
        1
    );

    h.service
        .ban_malformed(peer, "focused active-owner revocation".to_string())
        .await;
    h.service.relay.effects.wait_idle().await;
    assert!(
        !h.service
            .pipeline
            .kernel
            .read(|coordinator| coordinator.contains_hash(&hash))
    );
    assert_eq!(
        h.service
            .pipeline
            .kernel
            .read(|coordinator| coordinator.peer_active_work(peer)),
        0,
        "revocation must refund the peer's active-work slot immediately"
    );

    // Late worker completion observes no owner and cannot resurrect or
    // terminalize a newer incarnation.
    h.service.process_pipeline_raw_lease(lease).await;
    assert!(
        !h.service
            .pipeline
            .kernel
            .read(|coordinator| coordinator.contains_hash(&hash))
    );
    assert!(matches!(
        h.relay_rx.try_recv().unwrap(),
        TxVerificationResult::Reject { tx_hash } if tx_hash == hash
    ));
    h.cancel.cancel();
}

/// Resolved work can wait behind expensive verification for many blocks. It
/// must not pin one RocksDB snapshot per historical tip while queued; a stale
/// resolution returns to the ordered resolver before script execution.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_resolved_work_is_snapshot_free_and_stale_tip_requeues() {
    use crate::component::pre_pool::{PrePoolLocation, ResolveLane, WorkCapability};
    use crate::component::tests::harness::{WorkerSet, harness};

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    let peer = ckb_network::PeerIndex::from(42);
    h.service
        .submit_remote_tx(tx, TxSource::Remote { cycles: 0, peer })
        .await
        .unwrap();
    let raw = h
        .service
        .pipeline
        .kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap();
    h.service.process_pipeline_raw_lease(raw).await;
    let verify = h
        .service
        .pipeline
        .kernel
        .mutate_required("test verify checkout", |coordinator| {
            coordinator.checkout_verify(WorkCapability::Any)
        })
        .unwrap();

    let old_snapshot = h.service.pool.tx_pool.read().await.cloned_snapshot();
    let old_snapshot_weak = Arc::downgrade(&old_snapshot);
    let next_block = BlockBuilder::default()
        .parent_hash(old_snapshot.tip_hash())
        .number(old_snapshot.tip_number() + 1)
        .epoch(EpochNumberWithFraction::new(0, 0, 1))
        .build();
    let next_snapshot = Arc::new(Snapshot::new(
        next_block.header(),
        old_snapshot.total_difficulty().clone(),
        old_snapshot.epoch_ext().clone(),
        h.store.store().get_snapshot(),
        Default::default(),
        old_snapshot.cloned_consensus(),
    ));
    h.service.pool.tx_pool.write().await.snapshot = next_snapshot;
    drop(old_snapshot);
    assert!(
        old_snapshot_weak.upgrade().is_none(),
        "queued/active verification payload must not retain the old database snapshot"
    );

    let mut chunk_rx = h.service.pipeline.chunk_rx.clone();
    h.service
        .process_pipeline_verify_lease(verify, &mut chunk_rx)
        .await;
    assert_eq!(
        h.service
            .pipeline
            .kernel
            .read(|coordinator| coordinator.view(&hash).unwrap().location),
        PrePoolLocation::ResolveQueued
    );
    h.cancel.cancel();
}

/// Availability is a post-mutation level, not a physical "block contained
/// this output" event. In particular, an output created and consumed within
/// one attached branch is absent from the resulting snapshot and must not
/// wake a conflict-history owner into a second, misleading rejection.
#[tokio::test]
async fn dependency_availability_uses_the_authoritative_overlay_level() {
    use crate::component::pre_pool::DependencyKey;
    use crate::component::tests::harness::{WorkerSet, harness};

    let h = harness(1).workers(WorkerSet::None).build();
    let live = h.out_points[0].clone();
    let absent_after_delta = OutPoint::new(ckb_types::packed::Byte32::new([91; 32]), 0);
    let pool = h.service.pool.tx_pool.read().await;
    let available = crate::service::pipeline_ops::available_cell_dependencies(
        &pool,
        [live.clone(), absent_after_delta.clone()],
    );

    assert!(available.contains(&DependencyKey::Cell(live)));
    assert!(!available.contains(&DependencyKey::Cell(absent_after_delta)));
    assert!(crate::service::pipeline_ops::dependency_is_available(
        &pool,
        &DependencyKey::Header(pool.snapshot().tip_hash()),
    ));
    assert!(!crate::service::pipeline_ops::dependency_is_available(
        &pool,
        &DependencyKey::Header(ckb_types::packed::Byte32::new([92; 32])),
    ));
    h.cancel.cancel();
}

/// Raw hash equality is insufficient for source promotion because witnesses
/// remain verification inputs. A proposal carrying another witness variant
/// must atomically restart normal bounded processing with the trusted payload,
/// rather than synchronously verifying on the dispatcher or continuing an old
/// remote lease.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proposal_witness_variant_replaces_remote_payload_at_authoritative_handoff() {
    use crate::component::pre_pool::{PrePoolLocation, PrePoolSource, ResolveLane};
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::service::TxVerificationResult;

    let h = harness(1).workers(WorkerSet::None).build();
    let remote = build_tx(&h.out_points[0], 4_000)
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"remote").pack()])
        .build();
    let proposal = remote
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from(vec![0x50; 4_096]).pack()])
        .build();
    assert_eq!(remote.hash(), proposal.hash());
    assert_ne!(remote.witness_hash(), proposal.witness_hash());
    let hash = remote.hash();
    let id = remote.proposal_short_id();
    let peer = ckb_network::PeerIndex::from(18);

    h.service
        .submit_remote_tx(remote, TxSource::Remote { cycles: 0, peer })
        .await
        .expect("remote witness variant enters coordinator");
    let old_lease = h
        .service
        .pipeline
        .kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap();
    let old_usage = h
        .service
        .pipeline
        .kernel
        .read(|coordinator| coordinator.total_usage());
    assert!(!h.service.notify_tx(proposal.clone()).await.unwrap());

    assert!(
        h.service
            .pool
            .tx_pool
            .read()
            .await
            .get_tx_from_pool(&id)
            .is_none(),
        "proposal notification must not synchronously execute script verification"
    );
    let (view, payload_witness, ingress_peer, blame_peer) =
        h.service.pipeline.kernel.read(|coordinator| {
            let raw = coordinator.raw_by_hash(&hash).unwrap();
            (
                coordinator.view(&hash).unwrap(),
                raw.tx.witness_hash(),
                raw.ingress_peer(),
                raw.blame_peer(),
            )
        });
    assert_eq!(view.source, PrePoolSource::Proposal);
    assert_eq!(view.location, PrePoolLocation::ResolveQueued);
    assert!(
        h.service
            .pipeline
            .kernel
            .read(|coordinator| coordinator.total_usage().bytes)
            > old_usage.bytes,
        "the replacement witness, not the displaced witness, owns the payload charge"
    );
    assert_eq!(payload_witness, proposal.witness_hash());
    assert_eq!(ingress_peer, Some(peer));
    assert_eq!(
        blame_peer, None,
        "a trusted replacement witness must not blame its old ingress peer"
    );
    assert!(matches!(
        h.service
            .pipeline
            .kernel
            .mutate(|coordinator| coordinator.requeue_resolve(&old_lease)),
        Err(crate::component::pre_pool::PrePoolError::Stale { .. })
    ));

    let replacement = h
        .service
        .pipeline
        .kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap();
    assert_eq!(
        replacement.payload.tx.witness_hash(),
        proposal.witness_hash()
    );
    h.service.process_pipeline_raw_lease(replacement).await;
    let verify = h
        .service
        .pipeline
        .kernel
        .mutate(|coordinator| {
            coordinator.checkout_verify(crate::component::pre_pool::WorkCapability::Any)
        })
        .unwrap()
        .unwrap();
    let mut chunk_rx = h.service.pipeline.chunk_rx.clone();
    h.service
        .process_pipeline_verify_lease(verify, &mut chunk_rx)
        .await;

    let resident = h
        .service
        .pool
        .tx_pool
        .read()
        .await
        .get_tx_from_pool(&id)
        .cloned()
        .expect("trusted proposal variant commits");
    assert_eq!(resident.witness_hash(), proposal.witness_hash());
    assert!(
        !h.service
            .pipeline
            .kernel
            .read(|coordinator| coordinator.contains_hash(&hash))
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
    .expect("the older remote ingress is settled by the trusted handoff");
    assert!(matches!(
        relayed,
        TxVerificationResult::Ok {
            original_peer: Some(relayed_peer),
            tx_hash,
        } if relayed_peer == peer && tx_hash == hash
    ));

    h.cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proposal_promoted_remote_terminal_still_releases_ingress_filter() {
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::service::TxVerificationResult;

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    let peer = ckb_network::PeerIndex::from(8);
    h.service
        .submit_remote_tx(tx.clone(), TxSource::Remote { cycles: 0, peer })
        .await
        .unwrap();
    assert!(!h.service.notify_tx(tx).await.unwrap());

    h.service.clear_pipeline().await;
    let relayed = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(result) = h.relay_rx.try_recv() {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("promoted remote ingress receives one terminal settlement");
    assert!(matches!(
        relayed,
        TxVerificationResult::Reject { tx_hash } if tx_hash == hash
    ));

    h.cancel.cancel();
}

/// A saturated external-effect budget must backpressure before the
/// authoritative pool mutation. Otherwise cancellation while waiting to
/// journal the callback/relay result could leave an accepted transaction with
/// no terminal publication record.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_commit_waits_for_effect_credit_before_mutating_pool() {
    use crate::component::tests::harness::{WorkerSet, harness};

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let tx_hash = tx.hash();
    let held = h
        .service
        .relay
        .effects
        .reserve(512_000_000)
        .await
        .expect("test owns the complete outbox byte budget");

    let service = h.service.clone();
    let submit = tokio::spawn(async move { service.process_tx(tx, TxSource::Local).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !h.service
            .pool
            .tx_pool
            .read()
            .await
            .pool_map
            .iter()
            .any(|entry| entry.inner.transaction().hash() == tx_hash),
        "pool membership must not change while effect preflight is blocked"
    );

    drop(held);
    tokio::time::timeout(Duration::from_secs(5), submit)
        .await
        .expect("submission resumes after effect credit is released")
        .expect("submission task joins")
        .expect("local transaction commits");
    assert!(
        h.service
            .pool
            .tx_pool
            .read()
            .await
            .pool_map
            .iter()
            .any(|entry| entry.inner.transaction().hash() == tx_hash)
    );

    h.cancel.cancel();
}

fn hold_coordinator_read(
    runtime: Arc<crate::component::pre_pool::PrePool>,
) -> (std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        runtime.read(|_| {
            locked_tx.send(()).unwrap();
            release_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("test releases the coordinator read guard");
        });
    });
    locked_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("coordinator read guard is held");
    (release_tx, thread)
}

async fn wait_for_cross_authority_query_pool_guard(service: &TxPoolService) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match service.pool.tx_pool.try_write() {
                Ok(guard) => drop(guard),
                Err(_) => break,
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("query acquires the pool read guard before inspecting coordinator state");
}

/// Cross-authority queries hold the TxPool read guard while inspecting the
/// coordinator. Clear and reorg both need the corresponding write guard for
/// their ownership handoff, so a query must observe either the complete old
/// state or the complete new state and never a transient `NotFound` gap.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cross_authority_query_is_serialized_with_clear_and_reorg() {
    use crate::component::pre_pool::ResolveLane;
    use crate::component::tests::harness::{WorkerSet, harness};
    use std::collections::{HashSet, VecDeque};

    // Query started before clear: force it to pause after taking the pool read
    // guard, then prove clear cannot remove the coordinator owner underneath
    // it.
    {
        let h = harness(1).workers(WorkerSet::None).build();
        let tx = build_tx(&h.out_points[0], 4_000);
        let hash = tx.hash();
        let id = tx.proposal_short_id();
        h.service
            .pipeline
            .kernel
            .admit_transaction(
                tx,
                TxSource::Proposal,
                h.service.current_pipeline_epoch().unwrap(),
                ResolveLane::Ingress,
            )
            .unwrap();

        let (release, coordinator_thread) =
            hold_coordinator_read(Arc::clone(&h.service.pipeline.kernel));
        let query_service = h.service.clone();
        let query_id = id.clone();
        let query = tokio::spawn(async move {
            query_service
                .exclude_existing_proposal(vec![query_id])
                .await
        });
        wait_for_cross_authority_query_pool_guard(&h.service).await;

        let old_epoch = h.service.current_pipeline_epoch().unwrap();
        let snapshot = h.service.pool.tx_pool.read().await.cloned_snapshot();
        let mut clear_service = h.service.clone();
        let clear = tokio::spawn(async move { clear_service.clear_pool(snapshot).await });
        tokio::time::timeout(Duration::from_secs(5), async {
            while h.service.current_pipeline_epoch().unwrap() == old_epoch {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("clear reaches its epoch barrier while waiting for the query");

        release.send(()).unwrap();
        assert!(query.await.unwrap().is_empty(), "query sees the old owner");
        clear.await.unwrap();
        coordinator_thread.join().unwrap();
        assert_eq!(
            h.service.exclude_existing_proposal(vec![id.clone()]).await,
            vec![id]
        );
        assert!(!h.service.pipeline.kernel.read(|c| c.contains_hash(&hash)));
        h.cancel.cancel();
    }

    // Query started before an attached-block handoff: the reorg owns the
    // recovery lock but cannot acquire TxPool write access until the query has
    // completed its coordinator snapshot.
    {
        let h = harness(1).workers(WorkerSet::None).build();
        let tx = build_tx(&h.out_points[0], 4_000);
        let hash = tx.hash();
        let id = tx.proposal_short_id();
        h.service
            .pipeline
            .kernel
            .admit_transaction(
                tx.clone(),
                TxSource::Proposal,
                h.service.current_pipeline_epoch().unwrap(),
                ResolveLane::Ingress,
            )
            .unwrap();

        let (release, coordinator_thread) =
            hold_coordinator_read(Arc::clone(&h.service.pipeline.kernel));
        let query_service = h.service.clone();
        let query_id = id.clone();
        let query = tokio::spawn(async move {
            query_service
                .exclude_existing_proposal(vec![query_id])
                .await
        });
        wait_for_cross_authority_query_pool_guard(&h.service).await;

        let attached = BlockBuilder::default()
            .transaction(TransactionBuilder::default().build())
            .transaction(tx)
            .build();
        let snapshot = h.service.pool.tx_pool.read().await.cloned_snapshot();
        let reorg_service = h.service.clone();
        let reorg = tokio::spawn(async move {
            reorg_service
                .update_tx_pool_for_reorg(
                    VecDeque::new(),
                    VecDeque::from([attached]),
                    HashSet::new(),
                    snapshot,
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match h.service.recovery_lock.try_lock() {
                    Ok(guard) => drop(guard),
                    Err(_) => break,
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("reorg owns recovery serialization while waiting for the query");

        release.send(()).unwrap();
        assert!(query.await.unwrap().is_empty(), "query sees the old owner");
        reorg.await.unwrap().unwrap();
        coordinator_thread.join().unwrap();
        assert_eq!(
            h.service.exclude_existing_proposal(vec![id.clone()]).await,
            vec![id]
        );
        assert!(!h.service.pipeline.kernel.read(|c| c.contains_hash(&hash)));
        h.cancel.cancel();
    }
}
