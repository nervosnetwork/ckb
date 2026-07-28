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

/// A non-contextual remote rejection happens before kernel admission,
/// but it still crosses the same terminal boundary as a later verifier
/// rejection. Malformed transactions are deliberately not announced as
/// retryable relayer rejections; banning the peer and recording the public
/// rejection are the stable consequences.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_remote_preflight_is_banned_recorded_and_not_relayed() {
    use crate::component::recent_reject::RecentReject;
    use crate::service::tests::support::{WorkerSet, harness};

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
    use crate::service::TxVerificationResult;
    use crate::service::tests::support::{WorkerSet, harness};

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    let id = tx.proposal_short_id();
    let peer = ckb_network::PeerIndex::from(1);

    h.service
        .submit_remote_tx(tx.clone(), TxSource::Remote { cycles: 0, peer })
        .await
        .expect("remote copy enters the kernel");
    assert!(
        h.service
            .pipeline
            .kernel
            .read(|kernel| kernel.contains_hash(&hash)),
        "the no-worker harness must leave the remote copy kernel-owned"
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
            .read(|kernel| kernel.contains_hash(&hash)),
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
/// already carrying a copyable commit ticket. The commit session now opens
/// only after the pool guard and makes that stale-ticket state unrepresentable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pipeline_commit_worker_waits_for_the_pool_sequencer() {
    use crate::component::pre_pool::PrePoolLocation;
    use crate::service::tests::support::{WorkerSet, harness};

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    stage_verified_remote_candidate(&h.service, tx, 1.into()).await;
    assert_eq!(
        h.service
            .pipeline
            .kernel
            .read(|kernel| kernel.view(&hash).unwrap().location),
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
            .read(|kernel| kernel.view(&hash).unwrap().location),
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
    h.cancel.cancel();
    tokio::time::timeout(Duration::from_secs(1), driver)
        .await
        .expect("commit worker observes cancellation")
        .expect("commit worker does not panic");
}

/// The early duplicate check can become stale before pipeline admission. The
/// authoritative admission boundary must recheck TxPool while holding its read
/// guard across the kernel mutation, so a transaction committed in that
/// window is never shadowed by a second pre-pool owner.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_precheck_cannot_readmit_an_already_accepted_transaction() {
    use crate::service::TxVerificationResult;
    use crate::service::tests::support::{WorkerSet, harness};

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
            .read(|kernel| kernel.contains_hash(&hash)),
        "TxPool and kernel must never both own the same hash"
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

/// The commit journal region is part of the Ready owner's authority, not a
/// property of the shared driver. A full Remote region must therefore leave a
/// Proposal candidate able to consume the separately provisioned trusted
/// headroom. The old worst-case driver wait always used `Remote` before it had
/// selected a copyable ticket and stranded this candidate indefinitely.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proposal_ready_commit_uses_trusted_effect_headroom() {
    use crate::component::pre_pool::{ResolveLane, WorkCapability};
    use crate::service::TxVerificationResult;
    use crate::service::effects::{EffectBatch, EffectClass, EffectJournal, TxPoolEffect};
    use crate::service::tests::support::{WorkerSet, harness};
    use ckb_types::packed::Byte32;

    let mut h = harness(1).workers(WorkerSet::None).build();
    let effects = Arc::new(EffectJournal::new_partitioned(1, 128, 1, 256, 1, 128).unwrap());
    let remote = EffectBatch::new(vec![TxPoolEffect::Relay(TxVerificationResult::Reject {
        tx_hash: Byte32::new([0x9a; 32]),
    })])
    .unwrap();
    effects
        .try_apply(Some(remote), EffectClass::Remote, || ())
        .unwrap();
    h.service.relay.effects = effects;

    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    assert!(h.service.notify_tx(tx.clone()).await.unwrap());
    let raw = h
        .service
        .pipeline
        .kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
        .unwrap();
    h.service.process_pipeline_raw_lease(raw).await;
    let verify = h
        .service
        .pipeline
        .kernel
        .mutate(|kernel| kernel.checkout_verify(WorkCapability::Any))
        .unwrap()
        .unwrap();
    let mut chunk_rx = h.chunk_rx.clone();
    tokio::time::timeout(Duration::from_secs(2), async {
        h.service
            .process_pipeline_verify_lease(verify, &mut chunk_rx)
            .await;
        assert!(h.service.drive_pipeline_commits().await);
    })
    .await
    .expect("trusted Ready admission must not wait on the full Remote region");

    assert!(
        h.service
            .pool
            .tx_pool
            .read()
            .await
            .get_tx_from_pool_by_hash(&hash)
            .is_some()
    );
    assert!(
        !h.service
            .pipeline
            .kernel
            .read(|kernel| kernel.contains_hash(&hash))
    );
    h.cancel.cancel();
}

/// Promotion changes the authoritative owner but deliberately reuses valid
/// verification output. The journal class must follow that owner rather than
/// the stale source embedded in the verified payload.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn promoted_remote_ready_commit_uses_trusted_effect_headroom() {
    use crate::service::TxVerificationResult;
    use crate::service::effects::{EffectBatch, EffectClass, EffectJournal, TxPoolEffect};
    use crate::service::tests::support::{WorkerSet, harness};
    use ckb_types::packed::Byte32;

    let mut h = harness(1).workers(WorkerSet::None).build();
    let effects = Arc::new(EffectJournal::new_partitioned(1, 128, 1, 256, 1, 128).unwrap());
    let remote = EffectBatch::new(vec![TxPoolEffect::Relay(TxVerificationResult::Reject {
        tx_hash: Byte32::new([0x9b; 32]),
    })])
    .unwrap();
    effects
        .try_apply(Some(remote), EffectClass::Remote, || ())
        .unwrap();
    h.service.relay.effects = effects;

    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    stage_verified_remote_candidate(&h.service, tx.clone(), 23.into()).await;
    assert!(!h.service.notify_tx(tx).await.unwrap());

    tokio::time::timeout(Duration::from_secs(2), h.service.drive_pipeline_commits())
        .await
        .expect("promoted owner must use trusted publication headroom");
    assert!(
        h.service
            .pool
            .tx_pool
            .read()
            .await
            .get_tx_from_pool_by_hash(&hash)
            .is_some()
    );
    h.cancel.cancel();
}

/// Backpressure belongs to one publication class, not to accepted-pool or
/// kernel authority. A Remote head waiting for journal capacity must release
/// both state guards so a later Proposal can be reselected and committed
/// through trusted headroom.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_effect_backpressure_does_not_block_later_proposal() {
    use crate::service::TxVerificationResult;
    use crate::service::effects::{EffectBatch, EffectClass, EffectJournal, TxPoolEffect};
    use crate::service::tests::support::{WorkerSet, harness};
    use ckb_types::packed::Byte32;

    let mut h = harness(2).workers(WorkerSet::None).build();
    let effects = Arc::new(EffectJournal::new_partitioned(1, 128, 1, 256, 1, 128).unwrap());
    let remote = EffectBatch::new(vec![TxPoolEffect::Relay(TxVerificationResult::Reject {
        tx_hash: Byte32::new([0x9c; 32]),
    })])
    .unwrap();
    effects
        .try_apply(Some(remote), EffectClass::Remote, || ())
        .unwrap();
    h.service.relay.effects = effects;

    let remote_tx = build_tx(&h.out_points[0], 4_000);
    stage_verified_remote_candidate(&h.service, remote_tx.clone(), 24.into()).await;

    // Queue the first driver behind the accepted-pool writer. Tokio's fair
    // write lock lets the queued driver run before our second acquisition;
    // reacquiring therefore proves its exact journal predicate returned Full
    // and it released all state authority before waiting for capacity.
    let pool_guard = h.service.pool.tx_pool.write().await;
    let remote_service = h.service.clone();
    let remote_driver = tokio::spawn(async move { remote_service.drive_pipeline_commits().await });
    tokio::task::yield_now().await;
    drop(pool_guard);
    let released = tokio::time::timeout(Duration::from_secs(1), h.service.pool.tx_pool.write())
        .await
        .expect("Remote capacity wait must release accepted-pool authority");
    drop(released);

    let proposal = build_tx(&h.out_points[1], 4_000);
    let proposal_hash = proposal.hash();
    stage_verified_candidate(&h.service, proposal, TxSource::Proposal).await;
    let trusted_service = h.service.clone();
    let trusted_driver =
        tokio::spawn(async move { trusted_service.drive_pipeline_commits().await });

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if h.service
                .pool
                .tx_pool
                .read()
                .await
                .get_tx_from_pool_by_hash(&proposal_hash)
                .is_some()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Proposal must commit while the Remote owner stays backpressured");
    assert!(
        h.service
            .pipeline
            .kernel
            .read(|kernel| kernel.contains_hash(&remote_tx.hash()))
    );

    remote_driver.abort();
    trusted_driver.abort();
    h.cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_unverified_remote_owner_is_not_acknowledged_as_accepted() {
    use crate::service::tests::support::{WorkerSet, harness};

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
        "a merely kernel-owned raw hash has no successful result yet"
    );

    h.cancel.cancel();
}

/// Proposal notification upgrades scheduling authority in place, so an active
/// lease can finish under the trusted source without repeating resolution or
/// script verification. Immutable ingress attribution remains separate and
/// is tested by the peer-revocation case below.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proposal_promotes_active_remote_owner_without_restarting_lease() {
    use crate::component::pre_pool::{PrePoolLocation, PrePoolSource, ResolveLane};
    use crate::service::TxVerificationResult;
    use crate::service::tests::support::{WorkerSet, harness};

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
            .read(|kernel| kernel.deadline_len()),
        1
    );
    let lease = h
        .service
        .pipeline
        .kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
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
            .read(|kernel| kernel.view(&hash).unwrap().source),
        PrePoolSource::Proposal
    );
    assert_eq!(
        h.service
            .pipeline
            .kernel
            .read(|kernel| kernel.deadline_len()),
        0,
        "trusted promotion must cancel the obsolete remote expiry"
    );
    h.service.process_pipeline_raw_lease(lease).await;
    let view = h
        .service
        .pipeline
        .kernel
        .read(|kernel| kernel.view(&hash).unwrap());
    assert_eq!(view.source, PrePoolSource::Proposal);
    assert_eq!(view.location, PrePoolLocation::VerifyQueued);

    let verify = h
        .service
        .pipeline
        .kernel
        .mutate(|kernel| kernel.checkout_verify(crate::component::pre_pool::WorkCapability::Any))
        .unwrap()
        .unwrap();
    let mut chunk_rx = h.chunk_rx.clone();
    h.service
        .process_pipeline_verify_lease(verify, &mut chunk_rx)
        .await;
    let ready_source = h.service.pipeline.kernel.mutate(|kernel| {
        let session = kernel.begin_next_commit().unwrap().unwrap();
        session.payload().candidate.source
    });
    assert_eq!(
        ready_source,
        TxSource::Proposal,
        "verification completion must bind Ready to the source owned by its atomic transition"
    );
    assert!(h.service.drive_pipeline_commits().await);
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

/// A source promotion changes scheduling and budget ownership, not immutable
/// ingress attribution. Banning the origin removes every still-pre-pool owner,
/// publishes Reject to release the relayer filter and permits another peer to
/// supply the same transaction again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peer_ban_removes_promoted_ingress_and_allows_refetch() {
    use crate::component::pre_pool::{PrePoolSource, ResolveLane};
    use crate::service::TxVerificationResult;
    use crate::service::tests::support::{WorkerSet, harness};

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    let banned_peer = ckb_network::PeerIndex::from(27);
    let original_source = TxSource::Remote {
        cycles: 0,
        peer: banned_peer,
    };
    h.service
        .submit_remote_tx(tx.clone(), original_source)
        .await
        .unwrap();
    let lease = h
        .service
        .pipeline
        .kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
        .unwrap();

    assert!(!h.service.notify_tx(tx.clone()).await.unwrap());
    assert_eq!(
        h.service
            .pipeline
            .kernel
            .read(|kernel| kernel.view(&hash).unwrap().source),
        PrePoolSource::Proposal
    );

    h.service
        .ban_malformed(banned_peer, "test promoted ingress revocation".to_owned())
        .await;
    h.service.relay.effects.wait_idle().await;
    h.service.pipeline.kernel.read(|kernel| {
        assert!(!kernel.contains_hash(&hash));
        assert_eq!(kernel.total_usage(), Default::default());
        assert_eq!(kernel.remote_usage(), Default::default());
        assert_eq!(kernel.peer_usage(banned_peer), Default::default());
        assert_eq!(kernel.active_work(), 0);
        kernel.audit().unwrap();
    });
    assert!(matches!(
        h.relay_rx.try_recv().unwrap(),
        TxVerificationResult::Reject { tx_hash } if tx_hash == hash
    ));
    // A late completion from the removed incarnation is stale and cannot
    // recreate ownership after the ban transition.
    h.service.process_pipeline_raw_lease(lease).await;
    assert!(
        !h.service
            .pipeline
            .kernel
            .read(|kernel| kernel.contains_hash(&hash))
    );

    let replacement_peer = ckb_network::PeerIndex::from(28);
    assert!(
        h.service
            .submit_remote_tx(
                tx,
                TxSource::Remote {
                    cycles: 0,
                    peer: replacement_peer,
                },
            )
            .await
            .unwrap(),
        "the released hash must be admissible from another peer"
    );
    h.service
        .pipeline
        .kernel
        .read(|kernel| kernel.audit().unwrap());
    h.cancel.cancel();
}

/// The ban marker is the peer-removal linearization point. A Ready candidate
/// may race the later bounded deletion slices, but a commit whose final Plan
/// starts after that marker must terminalize the immutable ingress owner
/// instead of accepting it. This closes the only window in which a
/// source-promoted owner could otherwise cross the ban fence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ready_commit_observes_ban_fence_before_acceptance() {
    use crate::component::pre_pool::{PrePoolLocation, PrePoolSource};
    use crate::service::TxVerificationResult;
    use crate::service::tests::support::{WorkerSet, harness};

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    let banned_peer = ckb_network::PeerIndex::from(29);
    stage_verified_remote_candidate(&h.service, tx.clone(), banned_peer).await;
    assert!(!h.service.notify_tx(tx.clone()).await.unwrap());
    let before = h
        .service
        .pipeline
        .kernel
        .read(|kernel| kernel.view(&hash).unwrap());
    assert_eq!(before.source, PrePoolSource::Proposal);
    assert_eq!(before.location, PrePoolLocation::Ready);

    h.service
        .record_peer_ban(banned_peer, Duration::from_secs(60));
    h.service.drive_pipeline_commits().await;
    h.service.relay.effects.wait_idle().await;

    assert!(
        h.service
            .pool
            .tx_pool
            .read()
            .await
            .get_tx_from_pool_by_hash(&hash)
            .is_none(),
        "a Plan started after the ban fence cannot accept the old ingress"
    );
    h.service.pipeline.kernel.read(|kernel| {
        assert!(!kernel.contains_hash(&hash));
        assert_eq!(kernel.total_usage(), Default::default());
        assert_eq!(kernel.active_work(), 0);
        kernel.audit().unwrap();
    });
    assert!(matches!(
        h.relay_rx.try_recv().unwrap(),
        TxVerificationResult::Reject { tx_hash } if tx_hash == hash
    ));

    let replacement_peer = ckb_network::PeerIndex::from(30);
    assert!(
        h.service
            .submit_remote_tx(
                tx,
                TxSource::Remote {
                    cycles: 0,
                    peer: replacement_peer,
                },
            )
            .await
            .unwrap(),
        "the terminal Reject leaves the hash requestable from another peer"
    );
    h.cancel.cancel();
}

/// Peer revocation governs only the pre-pool owner attributed to that ingress.
/// Once the exact Ready Plan has committed, TxPool is the sole authority and a
/// later network ban must not become a primitive for deleting a valid accepted
/// transaction.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peer_ban_does_not_rollback_an_already_accepted_transaction() {
    use crate::service::tests::support::{WorkerSet, harness};

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    let peer = ckb_network::PeerIndex::from(33);
    stage_verified_remote_candidate(&h.service, tx, peer).await;

    assert!(h.service.drive_pipeline_commits().await);
    assert!(
        h.service
            .pool
            .tx_pool
            .read()
            .await
            .get_tx_from_pool_by_hash(&hash)
            .is_some()
    );
    h.service
        .ban_malformed(peer, "test accepted-owner boundary".to_owned())
        .await;
    h.service.relay.effects.wait_idle().await;

    assert!(
        h.service
            .pool
            .tx_pool
            .read()
            .await
            .get_tx_from_pool_by_hash(&hash)
            .is_some(),
        "peer administration cannot roll back the accepted authority"
    );
    h.service.pipeline.kernel.read(|kernel| {
        assert!(!kernel.contains_hash(&hash));
        kernel.audit().unwrap();
    });
    h.cancel.cancel();
}

/// A controller message can already be queued when another transaction bans
/// its peer. Admission rechecks that external marker after taking ownership,
/// runs the ordinary bounded peer-removal transaction, and returns only after
/// the new owner is gone. The commit fence covers the interval between those
/// two operations.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_remote_admission_after_ban_is_removed_and_refetchable() {
    use crate::error::Reject;
    use crate::service::TxVerificationResult;
    use crate::service::tests::support::{WorkerSet, harness};

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    let banned_peer = ckb_network::PeerIndex::from(31);
    h.service
        .record_peer_ban(banned_peer, Duration::from_secs(60));

    let result = h
        .service
        .submit_remote_tx(
            tx.clone(),
            TxSource::Remote {
                cycles: 0,
                peer: banned_peer,
            },
        )
        .await;
    assert!(matches!(result, Err(Reject::Internal(_))));
    h.service.relay.effects.wait_idle().await;
    h.service.pipeline.kernel.read(|kernel| {
        assert!(!kernel.contains_hash(&hash));
        assert_eq!(kernel.total_usage(), Default::default());
        kernel.audit().unwrap();
    });
    assert!(matches!(
        h.relay_rx.try_recv().unwrap(),
        TxVerificationResult::Reject { tx_hash } if tx_hash == hash
    ));
    assert!(h.relay_rx.try_recv().is_err());

    assert!(
        h.service
            .submit_remote_tx(
                tx,
                TxSource::Remote {
                    cycles: 0,
                    peer: ckb_network::PeerIndex::from(32),
                },
            )
            .await
            .unwrap()
    );
    h.cancel.cancel();
}

/// A peer ban must revoke an already checked-out remote owner, release its
/// active-work accounting and make the worker's lease stale in one
/// kernel transition. Otherwise an attacker can keep its budget slot
/// resident until the expensive worker eventually returns.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn banned_peer_revokes_active_remote_lease_and_releases_budget() {
    use crate::component::pre_pool::ResolveLane;
    use crate::service::TxVerificationResult;
    use crate::service::tests::support::{WorkerSet, harness};

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
        .unwrap()
        .unwrap();
    assert_eq!(
        h.service
            .pipeline
            .kernel
            .read(|kernel| kernel.peer_active_work(peer)),
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
            .read(|kernel| kernel.contains_hash(&hash))
    );
    assert_eq!(
        h.service
            .pipeline
            .kernel
            .read(|kernel| kernel.peer_active_work(peer)),
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
            .read(|kernel| kernel.contains_hash(&hash))
    );
    assert!(matches!(
        h.relay_rx.try_recv().unwrap(),
        TxVerificationResult::Reject { tx_hash } if tx_hash == hash
    ));
    h.cancel.cancel();
}

/// Choose a total pipeline byte budget whose derived per-peer byte partition
/// admits `current_charge` but rejects `next_charge`. Production derives the
/// per-peer partition as `(total * 7 / 8) / 8`.
fn phase_growth_budget(current_charge: usize, next_charge: usize) -> usize {
    assert!(next_charge > current_charge);
    let mut total = current_charge
        .checked_mul(64)
        .and_then(|value| value.checked_div(7))
        .expect("test phase charge fits usize");
    while total.saturating_mul(7).saturating_div(8).saturating_div(8) < current_charge {
        total = total.checked_add(1).expect("test budget fits usize");
    }
    let peer_budget = total.saturating_mul(7).saturating_div(8).saturating_div(8);
    assert!(peer_budget >= current_charge);
    assert!(
        peer_budget < next_charge,
        "phase charges leave no representable per-peer budget boundary"
    );
    total
}

/// A legal raw payload can grow when resolution attaches cell metadata. If the
/// new exact charge crosses the pipeline budget, the unchanged resolve lease
/// must terminalize with an explicit Full rejection rather than disappear as
/// internal cleanup.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_phase_capacity_growth_is_public_rejection() {
    use crate::component::pre_pool::ResolveLane;
    use crate::service::TxVerificationResult;
    use crate::service::tests::support::{WorkerSet, harness};

    let probe = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&probe.out_points[0], 4_000);
    let hash = tx.hash();
    let peer = ckb_network::PeerIndex::from(73);
    submit_remote(&probe.service, tx.clone(), 0, peer)
        .await
        .unwrap();
    let raw_charge = probe
        .service
        .pipeline
        .kernel
        .read(|kernel| kernel.view(&hash).unwrap().charge_bytes);
    let raw = probe
        .service
        .pipeline
        .kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
        .unwrap();
    probe.service.process_pipeline_raw_lease(raw).await;
    let resolved_charge = probe
        .service
        .pipeline
        .kernel
        .read(|kernel| kernel.view(&hash).unwrap().charge_bytes);
    probe.cancel.cancel();

    let budget = phase_growth_budget(raw_charge, resolved_charge);
    let limited = harness(1)
        .workers(WorkerSet::None)
        .max_pipeline_resident_size(budget)
        .build();
    let tx = build_tx(&limited.out_points[0], 4_000);
    let hash = tx.hash();
    submit_remote(&limited.service, tx, 0, peer).await.unwrap();
    let raw = limited
        .service
        .pipeline
        .kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
        .unwrap();
    limited.service.process_pipeline_raw_lease(raw).await;

    assert!(
        !limited
            .service
            .pipeline
            .kernel
            .read(|kernel| kernel.contains_hash(&hash))
    );
    let relayed = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(result) = limited.relay_rx.try_recv() {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("capacity rejection releases the remote ingress filter");
    assert!(matches!(
        relayed,
        TxVerificationResult::Reject { tx_hash } if tx_hash == hash
    ));
    assert!(!limited.service.pipeline.kernel.has_failed());
    limited.cancel.cancel();
}

/// Ready payload/index charging is another legal phase-growth boundary. It
/// uses the same explicit rejection protocol as resolution and must never be
/// routed through the structural-fault or silent-invalidation path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn verify_phase_capacity_growth_is_public_rejection() {
    use crate::component::pre_pool::{ResolveLane, WorkCapability};
    use crate::service::TxVerificationResult;
    use crate::service::tests::support::{WorkerSet, harness};

    let probe = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&probe.out_points[0], 4_000);
    let hash = tx.hash();
    let peer = ckb_network::PeerIndex::from(74);
    let cycles = measured_cycles(&probe.service, tx.clone()).await;
    submit_remote(&probe.service, tx.clone(), cycles, peer)
        .await
        .unwrap();
    let raw = probe
        .service
        .pipeline
        .kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
        .unwrap();
    probe.service.process_pipeline_raw_lease(raw).await;
    let resolved_charge = probe
        .service
        .pipeline
        .kernel
        .read(|kernel| kernel.view(&hash).unwrap().charge_bytes);
    let verify = probe
        .service
        .pipeline
        .kernel
        .mutate(|kernel| kernel.checkout_verify(WorkCapability::Any))
        .unwrap()
        .unwrap();
    let snapshot = probe.service.pool.tx_pool.read().await.cloned_snapshot();
    let verified = probe
        .service
        .verify_pipeline_resolved((*verify.payload).clone(), snapshot, None)
        .await
        .unwrap();
    let charge = verified
        .candidate
        .resident_size
        .checked_add(std::mem::size_of::<
            crate::component::pre_pool::PipelineVerifiedTx,
        >())
        .unwrap();
    probe
        .service
        .pipeline
        .kernel
        .mutate(|kernel| kernel.complete_verify(&verify, verified, charge))
        .unwrap();
    let ready_charge = probe
        .service
        .pipeline
        .kernel
        .read(|kernel| kernel.view(&hash).unwrap().charge_bytes);
    probe.cancel.cancel();

    let budget = phase_growth_budget(resolved_charge, ready_charge);
    let limited = harness(1)
        .workers(WorkerSet::None)
        .max_pipeline_resident_size(budget)
        .build();
    let tx = build_tx(&limited.out_points[0], 4_000);
    let hash = tx.hash();
    submit_remote(&limited.service, tx, cycles, peer)
        .await
        .unwrap();
    let raw = limited
        .service
        .pipeline
        .kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
        .unwrap();
    limited.service.process_pipeline_raw_lease(raw).await;
    let verify = limited
        .service
        .pipeline
        .kernel
        .mutate(|kernel| kernel.checkout_verify(WorkCapability::Any))
        .unwrap()
        .unwrap();
    let mut chunk_rx = limited.chunk_rx.clone();
    limited
        .service
        .process_pipeline_verify_lease(verify, &mut chunk_rx)
        .await;

    assert!(
        !limited
            .service
            .pipeline
            .kernel
            .read(|kernel| kernel.contains_hash(&hash))
    );
    let relayed = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(result) = limited.relay_rx.try_recv() {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("capacity rejection releases the remote ingress filter");
    assert!(matches!(
        relayed,
        TxVerificationResult::Reject { tx_hash } if tx_hash == hash
    ));
    assert!(!limited.service.pipeline.kernel.has_failed());
    limited.cancel.cancel();
}

/// Resolved work can wait behind expensive verification for many blocks. It
/// must not pin one RocksDB snapshot per historical tip while queued; a stale
/// resolution returns to the ordered resolver before script execution.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_resolved_work_is_snapshot_free_and_stale_tip_requeues() {
    use crate::component::pre_pool::{PrePoolLocation, ResolveLane, WorkCapability};
    use crate::service::tests::support::{WorkerSet, harness};

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
        .unwrap()
        .unwrap();
    h.service.process_pipeline_raw_lease(raw).await;
    let verify = h
        .service
        .pipeline
        .kernel
        .mutate_authoritative(|kernel| kernel.checkout_verify(WorkCapability::Any))
        .unwrap()
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

    let mut chunk_rx = h.chunk_rx.clone();
    h.service
        .process_pipeline_verify_lease(verify, &mut chunk_rx)
        .await;
    assert_eq!(
        h.service
            .pipeline
            .kernel
            .read(|kernel| kernel.view(&hash).unwrap().location),
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
    use crate::service::tests::support::{WorkerSet, harness};

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
    use crate::service::TxVerificationResult;
    use crate::service::tests::support::{WorkerSet, harness};

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
        .expect("remote witness variant enters kernel");
    let old_lease = h
        .service
        .pipeline
        .kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
        .unwrap();
    let old_usage = h
        .service
        .pipeline
        .kernel
        .read(|kernel| kernel.total_usage());
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
        h.service.pipeline.kernel.read(|kernel| {
            let raw = kernel.raw_by_hash(&hash).unwrap();
            (
                kernel.view(&hash).unwrap(),
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
            .read(|kernel| kernel.total_usage().bytes)
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
            .mutate(|kernel| kernel.requeue_resolve(&old_lease)),
        Err(crate::component::pre_pool::PrePoolError::Stale { .. })
    ));

    let replacement = h
        .service
        .pipeline
        .kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
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
        .mutate(|kernel| kernel.checkout_verify(crate::component::pre_pool::WorkCapability::Any))
        .unwrap()
        .unwrap();
    let mut chunk_rx = h.chunk_rx.clone();
    h.service
        .process_pipeline_verify_lease(verify, &mut chunk_rx)
        .await;
    assert!(h.service.drive_pipeline_commits().await);

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
            .read(|kernel| kernel.contains_hash(&hash))
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
async fn proposal_promoted_remote_clear_uses_generation_reset_to_release_ingress_filter() {
    use crate::service::TxVerificationResult;
    use crate::service::tests::support::{WorkerSet, harness};

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
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
    .expect("promoted remote ingress generation receives one terminal settlement");
    assert!(
        matches!(relayed, TxVerificationResult::GenerationReset),
        "clear deliberately discards the complete pre-pool generation, so its constant-size reset releases every relayer filter without a population-sized hash batch"
    );

    h.cancel.cancel();
}

/// An accepted-duplicate acknowledgement is authority-dependent output, not
/// a free-standing notification. If clear has already queued for the pool
/// write guard, the later duplicate observer must see absence instead of
/// appending `Ok(old tx)` after clear's `GenerationReset`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepted_duplicate_relay_cannot_overtake_a_waiting_clear_reset() {
    use crate::service::TxVerificationResult;
    use crate::service::tests::support::{WorkerSet, harness};

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    h.service
        .process_tx(tx, TxSource::Local)
        .await
        .expect("local transaction commits synchronously");
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if matches!(
                h.relay_rx.try_recv(),
                Ok(TxVerificationResult::Ok { tx_hash, .. }) if tx_hash == hash
            ) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("initial commit publishes its atomic relay result");

    let pool_guard = h.service.pool.tx_pool.read().await;
    let snapshot = pool_guard.cloned_snapshot();
    let old_epoch = h.service.current_pipeline_epoch().unwrap();
    let mut clear_service = h.service.clone();
    let clear = tokio::spawn(async move { clear_service.clear_pool(snapshot).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        while h.service.current_pipeline_epoch().unwrap() == old_epoch {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("clear publishes its epoch barrier before waiting for the pool");

    let duplicate_service = h.service.clone();
    let duplicate_hash = hash.clone();
    let duplicate = tokio::spawn(async move {
        duplicate_service
            .publish_accepted_relay_result(duplicate_hash, Some(9.into()))
            .await
    });
    tokio::task::yield_now().await;
    drop(pool_guard);

    clear.await.unwrap();
    assert!(
        !duplicate.await.unwrap().unwrap(),
        "the writer queued before this read owns the reset ordering"
    );
    h.service.relay.effects.wait_idle().await;
    assert!(matches!(
        h.relay_rx.try_recv().unwrap(),
        TxVerificationResult::GenerationReset
    ));
    assert!(
        h.relay_rx.try_recv().is_err(),
        "no stale accepted Ok may be published after the reset"
    );

    h.cancel.cancel();
}

fn hold_kernel_read(
    runtime: Arc<crate::component::pre_pool::PrePool>,
) -> (std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        runtime.read(|_| {
            locked_tx.send(()).unwrap();
            release_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("test releases the kernel read guard");
        });
    });
    locked_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("kernel read guard is held");
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
    .expect("query acquires the pool read guard before inspecting kernel state");
}

/// Cross-authority queries hold the TxPool read guard while inspecting the
/// kernel. Clear and reorg both need the corresponding write guard for
/// their ownership handoff, so a query must observe either the complete old
/// state or the complete new state and never a transient `NotFound` gap.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cross_authority_query_is_serialized_with_clear_and_reorg() {
    use crate::component::pre_pool::ResolveLane;
    use crate::service::tests::support::{WorkerSet, harness};
    use std::collections::{HashSet, VecDeque};

    // Query started before clear: force it to pause after taking the pool read
    // guard, then prove clear cannot remove the kernel owner underneath
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
                crate::component::pre_pool::PipelineAdmissionSource::Proposal,
                h.service.current_pipeline_epoch().unwrap(),
                ResolveLane::Ingress,
            )
            .unwrap();

        let (release, kernel_thread) = hold_kernel_read(Arc::clone(&h.service.pipeline.kernel));
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
        kernel_thread.join().unwrap();
        assert_eq!(
            h.service.exclude_existing_proposal(vec![id.clone()]).await,
            vec![id]
        );
        assert!(!h.service.pipeline.kernel.read(|c| c.contains_hash(&hash)));
        h.cancel.cancel();
    }

    // Query started before an attached-block handoff: the reorg cannot acquire
    // TxPool write access until the query has completed its kernel
    // snapshot; no cross-await recovery lock participates.
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
                crate::component::pre_pool::PipelineAdmissionSource::Proposal,
                h.service.current_pipeline_epoch().unwrap(),
                ResolveLane::Ingress,
            )
            .unwrap();

        let (release, kernel_thread) = hold_kernel_read(Arc::clone(&h.service.pipeline.kernel));
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
        tokio::task::yield_now().await;
        assert!(
            !reorg.is_finished(),
            "reorg waits for the query's pool guard"
        );

        release.send(()).unwrap();
        assert!(query.await.unwrap().is_empty(), "query sees the old owner");
        reorg.await.unwrap().unwrap();
        kernel_thread.join().unwrap();
        assert_eq!(
            h.service.exclude_existing_proposal(vec![id.clone()]).await,
            vec![id]
        );
        assert!(!h.service.pipeline.kernel.read(|c| c.contains_hash(&hash)));
        h.cancel.cancel();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chain_generation_reset_retires_old_generation_outside_the_lock() {
    use crate::component::pre_pool::ResolveLane;
    use crate::service::tests::support::{WorkerSet, harness};

    let h = harness(2).workers(WorkerSet::None).build();
    let remote = build_tx(&h.out_points[0], 4_000);
    let recovery = build_tx(&h.out_points[1], 4_000);
    let epoch = h.service.current_pipeline_epoch().unwrap();
    h.service
        .pipeline
        .kernel
        .admit_transaction(
            remote,
            crate::component::pre_pool::PipelineAdmissionSource::Remote(
                crate::component::pre_pool::RemoteSource::new(12.into(), 0),
            ),
            epoch,
            ResolveLane::Ingress,
        )
        .unwrap();

    let (batch, disposal) = h
        .service
        .pipeline
        .kernel
        .reset_for_chain(|fresh| fresh.retain_recovery_batch(vec![recovery.clone()], epoch))
        .unwrap();
    assert_eq!(batch, 1);
    assert!(
        h.service
            .pipeline
            .kernel
            .read(|kernel| kernel.contains_hash(&recovery.hash()))
    );
    drop(disposal);
    h.cancel.cancel();
}

/// Entry versions are stable-shell clocks. Resetting them with an entry
/// generation lets an old worker match a newly admitted owner with the same
/// hash/location (ABA).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authoritative_generation_swap_preserves_aba_clocks() {
    use crate::component::pre_pool::ResolveLane;
    use crate::service::tests::support::{WorkerSet, harness};

    let h = harness(3).workers(WorkerSet::None).build();
    let old_tx = build_tx(&h.out_points[0], 4_000);
    let new_recovery = build_tx(&h.out_points[2], 4_000);
    let old_epoch = h.service.current_pipeline_epoch().unwrap();
    h.service
        .pipeline
        .kernel
        .admit_transaction(
            old_tx.clone(),
            crate::component::pre_pool::PipelineAdmissionSource::Remote(
                crate::component::pre_pool::RemoteSource::new(11.into(), 0),
            ),
            old_epoch,
            ResolveLane::Ingress,
        )
        .unwrap();
    let stale_lease = h
        .service
        .pipeline
        .kernel
        .checkout_resolve(ResolveLane::Ingress)
        .unwrap()
        .expect("old generation has one resolve lease");
    let (_, disposal) = h
        .service
        .pipeline
        .kernel
        .reset_for_chain(|fresh| fresh.retain_recovery_batch(vec![new_recovery.clone()], old_epoch))
        .unwrap();
    drop(disposal);

    h.service
        .pipeline
        .kernel
        .admit_transaction(
            old_tx.clone(),
            crate::component::pre_pool::PipelineAdmissionSource::Proposal,
            old_epoch,
            ResolveLane::Ingress,
        )
        .unwrap();
    assert!(
        h.service
            .pipeline
            .kernel
            .mutate_lease(
                "stale lease must not match the replacement generation",
                |kernel| kernel.terminalize_resolve(&stale_lease)
            )
            .is_none()
    );
    assert!(
        h.service
            .pipeline
            .kernel
            .read(|kernel| kernel.contains_hash(&old_tx.hash())),
        "old lease cannot erase a same-hash owner in the replacement generation"
    );
    h.cancel.cancel();
}
