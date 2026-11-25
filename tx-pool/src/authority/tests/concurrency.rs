use super::*;
use crate::{
    authority::{
        chain::{self, ClearScope},
        ingress, membership,
        model::{Phase, Premises, RecoveryTriggers, RemoteOrigin, Source, Status, hash_key},
        notice::{Class, Effect},
        ranks::RankedPriorities,
        service::{Endpoints, Pool},
        state::OwnerEdit,
        tests::common::*,
        waiting,
    },
    callback::Callbacks,
    network::DummyTxPoolNetwork,
    service::BoundedTransaction,
};
use ckb_async_runtime::Handle;
use ckb_fee_estimator::FeeEstimator;
use ckb_types::{
    core::{TransactionBuilder, TransactionView},
    packed::{CellDep, CellOutput},
};
use ckb_util::parking_lot::Mutex;
use ckb_verification::cache::init_cache;
use std::{collections::BTreeSet, time::Duration};
use tokio::sync::{RwLock as AsyncRwLock, mpsc};
use tokio_util::sync::CancellationToken;

#[test]
fn exhausted_wake_counter_faults_and_notifies_every_progress_waiter() {
    use std::{
        future::Future,
        task::{Context, Waker},
    };

    let store = store();
    let key = OutPoint::new(tx(9810).hash(), 0);
    let owner = entry(&store, tx(9811), Source::Local)
        .with_phase(Phase::Waiting(BTreeSet::from([key.clone()])));
    insert(&store, owner);
    let key_op = OutPointKey::from_out_point(&key);
    store
        .state_write()
        .waiters
        .get_mut(&key_op)
        .unwrap()
        .next_pass = u64::MAX;
    let mut work = std::pin::pin!(store.work.notified());
    let mut changed = std::pin::pin!(store.changed.notified());
    let mut template = std::pin::pin!(store.template_changed.notified());
    let mut publication = std::pin::pin!(store.outbox.failed.notified());
    let mut context = Context::from_waker(Waker::noop());
    let mut waiters = [
        ("work", &mut work),
        ("changed", &mut changed),
        ("template", &mut template),
        ("publication", &mut publication),
    ];
    for (name, future) in &mut waiters {
        assert!(future.as_mut().poll(&mut context).is_pending(), "{name}");
    }
    let result = store.commit_with(|tx| {
        tx.state.start_wake(&key_op)?;
        Ok(())
    });
    assert!(matches!(result, Err(Error::Fault("wake pass"))));
    assert!(store.is_faulted());
    for (name, future) in &mut waiters {
        assert!(future.as_mut().poll(&mut context).is_ready(), "{name}");
    }
}

#[test]
fn a_missing_child_in_a_coherent_descendant_capture_is_a_fault() {
    let store = store();
    let parent = accept(&store, output_tx(9812), 1, 1, Status::Pending);
    let child = accept(
        &store,
        spend(9813, &[OutPoint::new(parent.clone(), 0)], &[]),
        1,
        1,
        Status::Pending,
    );
    assert_eq!(store.capture_descendants(&parent).unwrap().1.len(), 2);
    // Simulate a broken owner/index invariant directly in state.
    store.state_write().owners.remove(&hash_key(&child));
    assert!(matches!(
        store.capture_descendants(&parent),
        Err(Error::Fault("accepted descendant projection"))
    ));
}

#[test]
fn batch_lookups_preserve_key_order_and_duplicates() {
    let store = store();
    let mut hashes: Vec<_> = (0_u32..256)
        .rev()
        .map(|nonce| {
            let mut bytes = [0; 32];
            bytes[..4].copy_from_slice(&nonce.to_be_bytes());
            Byte32::new(bytes)
        })
        .collect();
    let present: BTreeSet<_> = hashes.iter().step_by(2).cloned().collect();
    let edits: Vec<_> = present
        .iter()
        .map(|hash| {
            (
                None,
                Some(entry(
                    &store,
                    tx(7749).fake_hash(hash.clone()),
                    Source::Recovery,
                )),
            )
        })
        .collect();
    commit_pairs(&store, edits).unwrap();
    hashes.extend(hashes.iter().take(4).cloned().collect::<Vec<_>>());

    let expected: Vec<_> = hashes
        .iter()
        .filter(|hash| present.contains(*hash))
        .cloned()
        .collect();
    assert_eq!(
        store
            .points(&hashes)
            .iter()
            .map(|owner| owner.hash())
            .collect::<Vec<_>>(),
        expected
    );
    let ids: Vec<_> = hashes.iter().map(ProposalShortId::from_tx_hash).collect();
    let (_, live, committed) = store.compact_lookup(&ids);
    assert!(committed.is_empty());
    assert_eq!(
        live.iter()
            .map(|(_, owner)| owner.hash())
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        store.filter_fresh_proposals(ids),
        hashes
            .iter()
            .filter(|hash| !present.contains(*hash))
            .map(ProposalShortId::from_tx_hash)
            .collect::<Vec<_>>()
    );
}

#[test]
fn proposal_lookups_check_the_prefix_at_hash_boundaries_and_between_neighbors() {
    for suffix in [0, u8::MAX] {
        let store = store();
        let mut low = [suffix; 32];
        low[..ProposalShortId::TOTAL_SIZE].fill(0);
        let low = Byte32::new(low);
        let mut mid = [suffix; 32];
        mid[..4].copy_from_slice(&1_u32.to_be_bytes());
        let missing = ProposalShortId::from_tx_hash(&Byte32::new(mid));
        let mut high = [suffix; 32];
        high[..4].copy_from_slice(&2_u32.to_be_bytes());
        let high = Byte32::new(high);
        let maximum = Byte32::new([u8::MAX; 32]);
        let owners: Vec<_> = [low, high, maximum]
            .into_iter()
            .map(|hash| entry(&store, tx(7748).fake_hash(hash), Source::Recovery))
            .collect();
        for owner in &owners {
            insert(&store, Arc::clone(owner));
        }
        let ids = vec![
            missing.clone(),
            owners[1].proposal(),
            owners[2].proposal(),
            owners[0].proposal(),
            missing.clone(),
            owners[2].proposal(),
        ];
        let (_, live, committed) = store.compact_lookup(&ids);
        assert!(committed.is_empty());
        assert_eq!(
            live.iter()
                .map(|(_, owner)| owner.hash())
                .collect::<Vec<_>>(),
            [1, 2, 0, 2].map(|index| owners[index].hash())
        );
        assert_eq!(
            store.filter_fresh_proposals(ids),
            vec![missing.clone(), missing]
        );
        for owner in owners {
            let id = owner.proposal();
            delete(&store, owner).unwrap();
            assert!(store.compact_lookup(std::slice::from_ref(&id)).1.is_empty());
            assert_eq!(store.filter_fresh_proposals(vec![id.clone()]), vec![id]);
        }
    }
}

#[test]
fn coupled_spender_transfer_preserves_indexes_in_both_hash_orders() {
    for (old_id, new_id) in [(1, 2), (2, 1)] {
        // First verify a double-spend attempt faults before any mutation.
        {
            let store = store();
            let point = OutPoint::new(tx(7710).hash(), 0);
            let transaction = spend(
                7711,
                std::slice::from_ref(&point),
                &[point.clone(), point.clone()],
            )
            .fake_hash(Byte32::new([old_id; 32]));
            let old_hash = accept(&store, transaction, 1, 1, Status::Pending);
            let old = store.point(&old_hash).1.unwrap();
            let transaction = old
                .transaction
                .as_ref()
                .clone()
                .fake_hash(Byte32::new([new_id; 32]));
            let mut accepted = old.accepted().unwrap().clone();
            let mut resolved = accepted.transaction.as_ref().clone();
            resolved.transaction = transaction.clone();
            accepted.transaction = Arc::new(resolved);
            let new =
                entry(&store, transaction, Source::Local).with_phase(Phase::Accepted(accepted));
            let queued = entry(
                &store,
                tx(7712),
                ingress::remote_source(94.into(), 1).unwrap(),
            );
            let budget = store.accepted_usage();
            assert!(matches!(
                commit_pairs(
                    &store,
                    vec![
                        (None, Some(Arc::clone(&queued))),
                        (None, Some(Arc::clone(&new))),
                    ],
                ),
                Err(Error::Fault("multiple accepted spenders"))
            ));
            assert!(store.is_faulted());
            assert_eq!(store.accepted_usage(), budget);
            assert!(store.point(&queued.hash()).1.is_none());
            assert!(store.point(&new.hash()).1.is_none());
            assert!(!store.state_read().peer_members.contains_key(&94.into()));
            assert_eq!(
                store.spender(&point, &mut Premises::default()).unwrap(),
                Some(old_hash)
            );
        }

        // Now verify atomic spender handoff succeeds in both hash orders.
        let store = store();
        let point = OutPoint::new(tx(7710).hash(), 0);
        let transaction = spend(
            7711,
            std::slice::from_ref(&point),
            &[point.clone(), point.clone()],
        )
        .fake_hash(Byte32::new([old_id; 32]));
        let old_hash = accept(&store, transaction, 1, 1, Status::Pending);
        let old = store.point(&old_hash).1.unwrap();
        let transaction = old
            .transaction
            .as_ref()
            .clone()
            .fake_hash(Byte32::new([new_id; 32]));
        let mut accepted = old.accepted().unwrap().clone();
        let mut resolved = accepted.transaction.as_ref().clone();
        resolved.transaction = transaction.clone();
        accepted.transaction = Arc::new(resolved);
        let new = entry(&store, transaction, Source::Local).with_phase(Phase::Accepted(accepted));
        let queued = entry(
            &store,
            tx(7712),
            ingress::remote_source(94.into(), 1).unwrap(),
        );

        commit_pairs(
            &store,
            vec![
                (Some(old), None),
                (None, Some(Arc::clone(&new))),
                (None, Some(Arc::clone(&queued))),
            ],
        )
        .unwrap();
        assert!(store.point(&old_hash).1.is_none());
        assert!(Arc::ptr_eq(&store.point(&new.hash()).1.unwrap(), &new));
        assert_eq!(
            store.spender(&point, &mut Premises::default()).unwrap(),
            Some(new.hash())
        );
        {
            let state = store.state_read();
            let point_key = OutPointKey::from_out_point(&point);
            let dep_readers: Vec<_> = state
                .cell_deps
                .get(&point_key)
                .unwrap()
                .iter()
                .copied()
                .collect();
            assert_eq!(dep_readers, vec![new.hash_bytes()]);
            let members: Vec<_> = state
                .peer_members
                .get(&94.into())
                .unwrap()
                .iter()
                .copied()
                .collect();
            assert_eq!(members, vec![queued.hash_bytes()]);
        }
        commit_pairs(&store, vec![(Some(new), None), (Some(queued), None)]).unwrap();
        {
            let state = store.state_read();
            let point_key = OutPointKey::from_out_point(&point);
            assert!(!state.spenders.contains_key(&point_key));
            assert!(!state.cell_deps.contains_key(&point_key));
            assert!(!state.peer_members.contains_key(&94.into()));
        }
        assert!(!store.is_faulted());
    }
}

#[test]
fn spender_only_relation_survives_reader_and_wake_retirement() {
    let store = store();
    let point = OutPoint::new(tx(7713).hash(), 0);
    let point_key = OutPointKey::from_out_point(&point);
    let spender = accept(
        &store,
        spend(7714, std::slice::from_ref(&point), &[]).fake_hash(Byte32::new([2; 32])),
        1,
        1,
        Status::Pending,
    );
    assert!(!store.state_read().cell_deps.contains_key(&point_key));
    let readers = [1, 3].map(|id| {
        accept(
            &store,
            spend(7715, &[], std::slice::from_ref(&point)).fake_hash(Byte32::new([id; 32])),
            1,
            1,
            Status::Pending,
        )
    });
    let mut dep_readers: Vec<_> = store
        .state_read()
        .cell_deps
        .get(&point_key)
        .unwrap()
        .iter()
        .map(|h| Byte32::new(*h))
        .collect();
    dep_readers.sort();
    assert_eq!(dep_readers, vec![readers[0].clone(), readers[1].clone()]);
    commit_pairs(
        &store,
        readers.iter().map(|r| (store.point(r).1, None)).collect(),
    )
    .unwrap();
    assert!(!store.state_read().cell_deps.contains_key(&point_key));
    let waiter = entry(&store, tx(7716), Source::Local)
        .with_phase(Phase::Waiting(BTreeSet::from([point.clone()])));
    insert(&store, Arc::clone(&waiter));
    store
        .commit_with(|tx| {
            tx.state.start_wake(&point_key)?;
            Ok(())
        })
        .unwrap();
    delete(&store, waiter).unwrap();
    assert!(store.wake_page(&mut None).is_none());
    assert_eq!(
        store.spender(&point, &mut Premises::default()).unwrap(),
        Some(spender.clone())
    );
    assert!(!store.state_read().waiters.contains_key(&point_key));
    delete(&store, store.point(&spender).1.unwrap()).unwrap();
    assert!(!store.state_read().spenders.contains_key(&point_key));
    assert!(!store.is_faulted());
}

#[test]
fn index_roles_survive_phase_changes_and_duplicate_dependencies() {
    let store = store();
    let parent = accept(&store, output_tx(7700), 1, 1, Status::Pending);
    let point = OutPoint::new(parent.clone(), 0);
    let point_key = OutPointKey::from_out_point(&point);
    let hash = accept(
        &store,
        spend(
            7701,
            std::slice::from_ref(&point),
            &[point.clone(), point.clone()],
        ),
        1,
        1,
        Status::Pending,
    );
    let hash_bytes = hash_key(&hash);
    let parent_bytes = hash_key(&parent);
    let check_roles = |spender: bool, dep: bool, child: bool, waiter: bool| {
        let state = store.state_read();
        assert_eq!(state.spenders.get(&point_key) == Some(&hash_bytes), spender);
        assert_eq!(
            state
                .cell_deps
                .get(&point_key)
                .is_some_and(|r| r.iter().any(|h| *h == hash_bytes)),
            dep
        );
        assert_eq!(
            state
                .children
                .get(&parent_bytes)
                .is_some_and(|c| c.iter().any(|h| *h == hash_bytes)),
            child
        );
        assert_eq!(
            state
                .waiters
                .get(&point_key)
                .is_some_and(|row| row.waiters.contains_key(&hash_bytes)),
            waiter
        );
    };
    check_roles(true, true, true, false);
    let owner = store.point(&hash).1.unwrap();
    let mut accepted = owner.accepted().unwrap().clone();
    accepted.timestamp += 1;
    let owner = replace(&store, owner, Phase::Accepted(accepted.clone()));
    check_roles(true, true, true, false);
    let keys = BTreeSet::from([point.clone()]);
    let owner = replace(&store, owner, Phase::Waiting(keys.clone()));
    check_roles(false, false, false, true);
    let owner = replace(
        &store,
        owner,
        Phase::Replaced(RecoveryTriggers::RetryInputs(keys)),
    );
    check_roles(false, false, false, true);
    let owner = replace(&store, owner, Phase::Resolve);
    check_roles(false, false, false, false);
    let owner = replace(&store, owner, Phase::Accepted(accepted));
    check_roles(true, true, true, false);
    delete(&store, owner).unwrap();
    check_roles(false, false, false, false);
    assert!(!store.is_faulted());
}

fn accepted(store: &Store, tx: TransactionView) {
    let hash = accept(store, tx, 1, 1, Status::Pending);
    let owner = store.point(&hash).1.unwrap();
    let mut value = owner.accepted().unwrap().clone();
    value.timestamp = ckb_systemtime::unix_time_as_millis();
    replace(store, owner, Phase::Accepted(value));
}

#[test]
fn history_pressure_drops_optional_history_inline_without_retry() {
    let configuration = TxPoolConfig {
        max_tx_verify_workers: 1,
        min_rbf_rate: ckb_types::core::FeeRate::from_u64(1_000),
        ..config()
    };
    let store = store_with_pipeline_limit(chain_snapshot(), &configuration, 64_000);
    let parent = funded_parent(7400, 20_000_000_000);
    accepted(&store, parent.clone());
    let transaction = funded_tx(OutPoint::new(parent.hash(), 0), 19_999_999_000);
    let victim = accept(&store, transaction.clone(), 1_000, 1, Status::Pending);
    let input = transaction.input_pts_iter().next().unwrap();
    let candidate = entry(
        &store,
        funded_tx(input.clone(), 19_999_990_000),
        Source::Local,
    );
    // Single-pass commit_admission detects history budget pressure inline and
    // omits optional replacement history without failing or retrying.
    let (_, reject) = commit_admission(
        &store,
        &candidate,
        10_000,
        1,
        Status::Pending,
        &configuration,
    )
    .unwrap();
    assert!(reject.is_none());
    assert!(store.point(&victim).1.is_none());
    assert!(
        store
            .point(&candidate.hash())
            .1
            .unwrap()
            .accepted()
            .is_some()
    );
    assert!(
        !store
            .state_read()
            .waiters
            .contains_key(&OutPointKey::from_out_point(&input))
    );
    assert!(!store.is_faulted());
}

fn code(nonce: u32) -> TransactionView {
    TransactionBuilder::default()
        .version(nonce)
        .output(CellOutput::default())
        .output_data(ckb_test_chain_utils::always_success_cell().1.pack())
        .build()
}

fn candidate(store: &Store, nonce: u32, code: &TransactionView) -> TransactionView {
    let parent = funded_parent(nonce, 20_000_000_000);
    accepted(store, parent.clone());
    funded_tx(OutPoint::new(parent.hash(), 0), 19_999_999_000)
        .as_advanced_builder()
        .set_cell_deps(vec![
            CellDep::new_builder()
                .out_point(OutPoint::new(code.hash(), 0))
                .build(),
        ])
        .build()
}

fn install_verify_rendezvous(
    store: &Store,
    target: [Byte32; 2],
) -> (
    mpsc::UnboundedReceiver<usize>,
    [std::sync::mpsc::Sender<()>; 2],
) {
    let (entered, events) = mpsc::unbounded_channel();
    let (release_a, wait_a) = std::sync::mpsc::channel();
    let (release_b, wait_b) = std::sync::mpsc::channel();
    let waits = [Mutex::new(wait_a), Mutex::new(wait_b)];
    *store.verify_observer.lock() = Some(Arc::new(move |hash| {
        if let Some((index, _)) = target
            .iter()
            .enumerate()
            .find(|(_, expected)| *expected == hash)
        {
            entered.send(index).unwrap();
            waits[index]
                .lock()
                .recv_timeout(Duration::from_secs(10))
                .expect("test releases both concurrent verifications only after overlap");
        }
    }));
    (events, [release_a, release_b])
}

async fn concurrent_remote_pipeline(shared_code: bool) {
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let configuration = config();
    let cache = Arc::new(AsyncRwLock::new(init_cache()));
    let (pool, sink, _drain) = Pool::new(
        configuration,
        chain_snapshot(),
        &handle,
        Arc::clone(&cache),
        None,
        None,
        FeeEstimator::new_dummy(),
    )
    .unwrap();
    let store = Arc::clone(&pool.store);
    let a_code = code(6100);
    let b_code = if shared_code {
        a_code.clone()
    } else {
        code(6101)
    };
    accepted(&store, a_code.clone());
    if !shared_code {
        accepted(&store, b_code.clone());
    }
    let a = candidate(&store, 6200, &a_code);
    let b = candidate(&store, 6300, &b_code);
    let pair = [a.clone(), b.clone()];
    let hashes = [a.hash(), b.hash()];
    let mut cycles = Vec::new();
    for tx in &pair {
        cycles.push(
            pool.test_accept(BoundedTransaction::try_new(tx.clone()).unwrap())
                .await
                .unwrap()
                .unwrap()
                .cycles,
        );
    }
    // Clear cache so both production workers execute VM verification concurrently.
    *cache.write().await = init_cache();

    // Install a real 2-party verification rendezvous barrier: each target tx
    // signals arrival in canonical verification and blocks until the test
    // observes BOTH transactions concurrently inside verification before
    // releasing either one. Serial execution cannot pass this barrier.
    let (mut events, [release_a, release_b]) = install_verify_rendezvous(&store, hashes.clone());

    let (published, mut publications) = mpsc::unbounded_channel();
    let target = hashes.clone();
    let mut callbacks = Callbacks::new();
    callbacks.register_pending(Box::new(move |entry| {
        if target.contains(&entry.transaction.hash()) {
            published.send(entry.transaction.hash()).unwrap();
        }
    }));
    let endpoints = Endpoints::new(
        Arc::new(DummyTxPoolNetwork),
        sink,
        Arc::new(callbacks),
        None,
        FeeEstimator::new_dummy(),
    );
    let (_chain, chain) = mpsc::channel(2);
    let (mut tasks, publisher) =
        pool.start_background(&handle, endpoints, chain, &CancellationToken::new());
    for (index, tx) in pair.into_iter().enumerate() {
        pool.submit_remote(
            BoundedTransaction::try_new(tx).unwrap(),
            cycles[index],
            (index + 70).into(),
        )
        .await
        .unwrap();
    }

    let mut overlapping = BTreeSet::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while overlapping.len() != 2 {
            overlapping.insert(events.recv().await.unwrap());
        }
    })
    .await
    .expect("both transactions must overlap inside off-lock verification");
    assert_eq!(overlapping, [0, 1].into());
    release_a.send(()).unwrap();
    release_b.send(()).unwrap();
    *store.verify_observer.lock() = None;

    let mut completed = BTreeSet::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while completed.len() != 2 {
            completed.insert(publications.recv().await.unwrap());
        }
    })
    .await
    .unwrap();
    assert_eq!(completed, hashes.into());
    pool.stop();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(result) = tasks.join_next().await {
            result.unwrap().unwrap();
        }
    })
    .await
    .unwrap();
    pool.close_outbox();
    tokio::time::timeout(Duration::from_secs(5), publisher)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(pool.persistence_eligible());
    assert!(!store.is_faulted());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn independent_remote_jobs_verify_and_publish_concurrently() {
    concurrent_remote_pipeline(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn remote_jobs_sharing_read_only_cell_dep_verify_and_publish_concurrently() {
    concurrent_remote_pipeline(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn local_submissions_sharing_read_only_cell_dep_verify_concurrently() {
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let configuration = config();
    let cache = Arc::new(AsyncRwLock::new(init_cache()));
    let (pool, sink, _drain) = Pool::new(
        configuration,
        chain_snapshot(),
        &handle,
        Arc::clone(&cache),
        None,
        None,
        FeeEstimator::new_dummy(),
    )
    .unwrap();
    let store = Arc::clone(&pool.store);
    let shared_code = code(6150);
    accepted(&store, shared_code.clone());
    let a = candidate(&store, 6250, &shared_code);
    let b = candidate(&store, 6350, &shared_code);
    let hashes = [a.hash(), b.hash()];

    let (mut events, [release_a, release_b]) = install_verify_rendezvous(&store, hashes);

    let endpoints = Endpoints::new(
        Arc::new(DummyTxPoolNetwork),
        sink,
        Arc::new(Callbacks::new()),
        None,
        FeeEstimator::new_dummy(),
    );
    let (_chain, chain) = mpsc::channel(2);
    let (mut tasks, publisher) =
        pool.start_background(&handle, endpoints, chain, &CancellationToken::new());

    let pool_a = Arc::clone(&pool);
    let tx_a = BoundedTransaction::try_new(a).unwrap();
    let task_a = tokio::spawn(async move { pool_a.submit_local_request(tx_a).await });

    let pool_b = Arc::clone(&pool);
    let tx_b = BoundedTransaction::try_new(b).unwrap();
    let task_b = tokio::spawn(async move { pool_b.submit_local_request(tx_b).await });

    let mut overlapping = BTreeSet::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while overlapping.len() != 2 {
            overlapping.insert(events.recv().await.unwrap());
        }
    })
    .await
    .expect("both local submissions must overlap inside off-lock verification");
    assert_eq!(overlapping, [0, 1].into());
    release_a.send(()).unwrap();
    release_b.send(()).unwrap();
    *store.verify_observer.lock() = None;

    tokio::time::timeout(Duration::from_secs(5), task_a)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), task_b)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .unwrap();

    pool.stop();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(result) = tasks.join_next().await {
            result.unwrap().unwrap();
        }
    })
    .await
    .unwrap();
    pool.close_outbox();
    tokio::time::timeout(Duration::from_secs(5), publisher)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(pool.persistence_eligible());
    assert!(!store.is_faulted());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn test_accept_tx_uses_read_lock_and_runs_concurrently_without_blocking_readers() {
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let configuration = config();
    let cache = Arc::new(AsyncRwLock::new(init_cache()));
    let (pool, _sink, _drain) = Pool::new(
        configuration,
        chain_snapshot(),
        &handle,
        Arc::clone(&cache),
        None,
        None,
        FeeEstimator::new_dummy(),
    )
    .unwrap();
    let store = Arc::clone(&pool.store);
    let shared_code = code(6450);
    accepted(&store, shared_code.clone());
    let a = candidate(&store, 6550, &shared_code);
    let b = candidate(&store, 6650, &shared_code);
    let hashes = [a.hash(), b.hash()];

    // 1. Holding a concurrent state read guard on another thread must never
    //    block `test_accept`: dry-run preview validates strictly under `state.read()`.
    let (read_entered_tx, read_entered_rx) = std::sync::mpsc::channel();
    let (read_release_tx, read_release_rx) = std::sync::mpsc::channel();
    let store_reader = Arc::clone(&store);
    let reader_thread = std::thread::spawn(move || {
        let _guard = store_reader.state_read();
        read_entered_tx.send(()).unwrap();
        read_release_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
    });
    read_entered_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    let single = tokio::time::timeout(
        Duration::from_secs(5),
        pool.test_accept(BoundedTransaction::try_new(a.clone()).unwrap()),
    )
    .await
    .expect("test_accept must not block on an active state.read() guard")
    .unwrap()
    .unwrap();
    assert!(single.cycles > 0);
    read_release_tx.send(()).unwrap();
    reader_thread.join().unwrap();

    // 2. Two concurrent `test_accept` calls sharing a read-only cell-dep must
    //    overlap simultaneously inside `preview_with` while holding `state.read()`.
    let (tx, mut events) = mpsc::unbounded_channel();
    let (release_a, wait_a) = std::sync::mpsc::channel();
    let (release_b, wait_b) = std::sync::mpsc::channel();
    let waits = Arc::new([Mutex::new(Some(wait_a)), Mutex::new(Some(wait_b))]);
    *store.preview_observer.lock() = Some(Arc::new(move |hash: &Byte32| {
        let Some(index) = hashes.iter().position(|target| target == hash) else {
            return;
        };
        let Some(wait) = waits[index].lock().take() else {
            return;
        };
        let _ = tx.send(index);
        let _ = wait.recv_timeout(Duration::from_secs(5));
    }));

    let pool_a = Arc::clone(&pool);
    let tx_a = BoundedTransaction::try_new(a).unwrap();
    let task_a = tokio::spawn(async move { pool_a.test_accept(tx_a).await });

    let pool_b = Arc::clone(&pool);
    let tx_b = BoundedTransaction::try_new(b).unwrap();
    let task_b = tokio::spawn(async move { pool_b.test_accept(tx_b).await });

    let mut overlapping = BTreeSet::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while overlapping.len() != 2 {
            overlapping.insert(events.recv().await.unwrap());
        }
    })
    .await
    .expect("both test_accept calls must overlap inside read-locked preview");
    assert_eq!(overlapping, [0, 1].into());

    // Even while both previews are paused inside `state.read()`, another
    // reader can acquire `state.read()` immediately.
    let _concurrent_read = store.state_read();
    drop(_concurrent_read);

    release_a.send(()).unwrap();
    release_b.send(()).unwrap();
    *store.preview_observer.lock() = None;

    tokio::time::timeout(Duration::from_secs(5), task_a)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), task_b)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!store.is_faulted());
}

#[test]
fn ban_deadlines_never_shorten_and_saturation_discards_only_the_oldest_marker() {
    let store = store();
    let now = Instant::now();
    store
        .commit_with(|tx| {
            tx.state
                .apply_ban(1.into(), now + Duration::from_secs(10_000), now);
            Ok(())
        })
        .unwrap();
    store
        .commit_with(|tx| {
            tx.state
                .apply_ban(1.into(), now + Duration::from_secs(1), now);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        store.state_read().bans.get(&1.into()),
        Some(&(now + Duration::from_secs(10_000)))
    );
    for index in 2..=crate::constants::PEER_BAN_FENCE_CAPACITY + 1 {
        store
            .commit_with(|tx| {
                tx.state
                    .apply_ban(index.into(), now + Duration::from_secs(index as u64), now);
                Ok(())
            })
            .unwrap();
    }
    assert_eq!(
        store.state_read().bans.len(),
        crate::constants::PEER_BAN_FENCE_CAPACITY
    );
    assert!(store.peer_banned(1.into()));
    assert!(!store.peer_banned(2.into()));
    assert!(store.peer_banned(3.into()));
    let delayed = funded_tx(OutPoint::new(tx(6999).hash(), 0), 20_000_000_000);
    let source = ingress::remote_source(2.into(), 1).unwrap();
    ingress::commit_ingress(&store, Arc::new(delayed.clone()), source).unwrap();
    assert!(
        store.point(&delayed.hash()).1.is_some(),
        "an evicted marker uses normal admission"
    );
}

#[test]
fn remaining_counter_exhaustion_rejects_before_any_owner_or_notice_change() {
    for counter in [
        "view counter",
        "accepted revision",
        "accepted removal revision",
        "wake pass",
    ] {
        let store = store();
        let hash = accept(&store, output_tx(6400), 1000, 7, Status::Pending);
        let owner = store.point(&hash).1.unwrap();
        let key = OutPoint::new(tx(6401).hash(), 0);
        let waiting = entry(&store, tx(6402), Source::Recovery)
            .with_phase(Phase::Waiting([key.clone()].into()));
        insert(&store, waiting);
        let key_op = OutPointKey::from_out_point(&key);
        {
            let mut state = store.state_write();
            match counter {
                "view counter" => state.view = u64::MAX,
                "accepted revision" => state.accepted_revision = u64::MAX,
                "accepted removal revision" => state.accepted_removal_revision = u64::MAX,
                "wake pass" => state.waiters.get_mut(&key_op).unwrap().next_pass = u64::MAX,
                _ => unreachable!(),
            }
        }
        let before = store.capture_all();
        let charge = store.accepted_usage();
        let result = if counter == "view counter" {
            chain::commit_clear(&store, Some(Arc::clone(&before.snapshot)), ClearScope::All)
        } else {
            store
                .commit_with(|tx| {
                    let edits = vec![OwnerEdit {
                        hash: owner.hash_bytes(),
                        before: Some(Arc::clone(&owner)),
                        after: Some(owner.with_phase(Phase::Resolve)),
                        ancestors: None,
                    }];
                    let wake_keys = vec![key_op];
                    let effect = Effect::removed(
                        &owner,
                        crate::error::Reject::Malformed("counter fixture".into(), String::new()),
                        None,
                    )?;
                    tx.commit_edits(edits, wake_keys, vec![effect], Class::Trusted)
                })
                .map(|(_, batch)| batch)
        };
        assert!(
            matches!(result, Err(Error::Fault(reason)) if reason == counter),
            "{counter}: expected fault"
        );
        assert!(store.is_faulted());
        assert_eq!(store.snapshot().0, before.view);
        assert!(Arc::ptr_eq(&store.point(&hash).1.unwrap(), &owner));
        assert_eq!(store.accepted_usage(), charge);
        assert!(store.outbox.pending_reject(&hash).is_none());
        assert_eq!(store.capture_all().owners.len(), before.owners.len());
    }
    let store = store();
    store.arrival.store(u64::MAX - 1, Ordering::SeqCst);
    assert_eq!(store.next_arrival().unwrap(), u64::MAX - 1);
    assert!(matches!(
        store.next_arrival(),
        Err(Error::Fault("arrival counter"))
    ));
    assert_eq!(store.arrival.load(Ordering::SeqCst), u64::MAX);
    assert!(store.is_faulted());
    assert!(store.capture_all().owners.is_empty());
}

#[test]
fn concurrent_arrival_allocation_is_unique_and_monotonic_without_owner_locks() {
    let store = store();
    let mut values = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let store = &store;
                scope.spawn(move || {
                    (0..256)
                        .map(|_| store.next_arrival().unwrap())
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>()
    });
    values.sort_unstable();
    assert_eq!(values, (0..2048).collect::<Vec<_>>());
    assert_eq!(store.next_arrival().unwrap(), 2048);
}

#[test]
fn full_query_waits_for_an_atomic_commit_and_returns_one_coherent_cut() {
    type ReadUsage = fn(&Store) -> (usize, usize, u64);

    fn captured_usage(captured: Captured) -> (usize, usize, u64) {
        use crate::authority::budget::{Amount, owner_amount};
        let expected = captured
            .owners
            .iter()
            .filter(|entry| entry.accepted().is_some())
            .fold(Amount::default(), |total, entry| {
                total.checked_add(owner_amount(entry)).unwrap()
            });
        (expected.items, expected.serialized, expected.cycles)
    }
    let readers: [ReadUsage; 3] = [
        |store| {
            let summary = crate::authority::query::summary(store, &config()).unwrap();
            (
                summary.pending_size,
                summary.total_tx_size,
                summary.total_tx_cycles,
            )
        },
        |store| captured_usage(store.capture_accepted()),
        |store| captured_usage(store.capture_all()),
    ];
    for read in readers {
        let store = store();
        let a = accept(&store, tx(6406), 1000, 7, Status::Pending);
        let b = accept(&store, tx(6407), 2000, 11, Status::Pending);
        let edits: Vec<_> = [&a, &b]
            .into_iter()
            .map(|hash| {
                let before = store.point(hash).1.unwrap();
                OwnerEdit {
                    hash: before.hash_bytes(),
                    before: Some(Arc::clone(&before)),
                    after: Some(before.with_phase(Phase::Resolve)),
                    ancestors: None,
                }
            })
            .collect();
        let (entered, ready) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let wait = Mutex::new(wait);
        std::thread::scope(|scope| {
            let store_ref = &store;
            let writer = scope.spawn(move || {
                store_ref.commit_with(|tx| {
                    entered.send(()).unwrap();
                    wait.lock().recv_timeout(Duration::from_secs(5)).unwrap();
                    tx.commit_edits(edits, Vec::new(), Vec::new(), Class::Trusted)
                })
            });
            ready.recv_timeout(Duration::from_secs(5)).unwrap();
            let (started, beginning) = std::sync::mpsc::channel();
            let (result, received) = std::sync::mpsc::channel();
            let observed = &store;
            let reader = scope.spawn(move || {
                started.send(()).unwrap();
                result.send(read(observed)).unwrap();
            });
            beginning.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(received.try_recv().is_err());
            release.send(()).unwrap();
            writer.join().unwrap().unwrap();
            let summary = received.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(summary, (0, 0, 0));
            reader.join().unwrap();
        });
        for hash in [a, b] {
            assert!(matches!(
                store.point(&hash).1.unwrap().phase,
                Phase::Resolve
            ));
        }
    }
}

#[test]
fn conflicting_input_commits_admit_exactly_one_winner() {
    let store = store();
    let input = OutPoint::new(tx(6700).hash(), 0);
    let a = entry(
        &store,
        spend(6701, std::slice::from_ref(&input), &[]),
        Source::Local,
    );
    let b = entry(&store, spend(6702, &[input], &[]), Source::Local);
    // Both verify against the same unspent input before either commits.
    let verified_a = verification_fixture(&store, &a, 1000, 1, Status::Pending);
    let verified_b = verification_fixture(&store, &b, 1000, 1, Status::Pending);
    let cfg = config();
    let (_, reject_a) = membership::commit_admission(
        &store,
        &a,
        None,
        &verified_a,
        &cfg,
        membership::AdmissionOptions::commit(),
    )
    .unwrap();
    assert!(reject_a.is_none());
    let second = membership::commit_admission(
        &store,
        &b,
        None,
        &verified_b,
        &cfg,
        membership::AdmissionOptions::commit(),
    );
    assert!(matches!(second, Err(Error::Stale)));
    assert!(store.point(&a.hash()).1.unwrap().accepted().is_some());
    assert!(store.point(&b.hash()).1.is_none());
    assert!(!store.is_faulted());
}

#[test]
fn phase_and_source_updates_keep_current_proposal_and_exact_expiry() {
    for accept_final in [false, true] {
        let store = store();
        let due = Instant::now() + Duration::from_secs(60);
        let later = due + Duration::from_secs(60);
        let remote = |deadline| Source::Remote {
            origin: RemoteOrigin {
                peer: 96.into(),
                deadline,
            },
            cycles: Some(1),
        };
        let owner = entry(&store, tx(7720), remote(due));
        let proposal = owner.proposal();
        insert(&store, Arc::clone(&owner));
        let phase = Phase::Verify(Arc::clone(
            verification_fixture(&store, &owner, 1, 1, Status::Pending).resolved(),
        ));
        let verifying = replace(&store, owner, phase);
        let lookup = |expected: &Arc<Entry>| {
            let (_, found, committed) = store.compact_lookup(std::slice::from_ref(&proposal));
            assert!(committed.is_empty());
            assert_eq!(found.len(), 1);
            assert!(Arc::ptr_eq(&found[0].1, expected));
        };
        lookup(&verifying);
        assert!(
            store
                .expired(due - Duration::from_nanos(1), 0, 10)
                .is_empty()
        );
        let expired = store.expired(due, 0, 10);
        assert_eq!(expired.len(), 1);
        assert!(Arc::ptr_eq(&expired[0], &verifying));

        let extended = verifying.with_source(remote(later));
        commit_pairs(
            &store,
            vec![(Some(Arc::clone(&verifying)), Some(Arc::clone(&extended)))],
        )
        .unwrap();
        lookup(&extended);
        assert!(store.expired(due, 0, 10).is_empty());
        let expired = store.expired(later, 0, 10);
        assert_eq!(expired.len(), 1);
        assert!(Arc::ptr_eq(&expired[0], &extended));

        assert!(matches!(
            commit_pairs(&store, vec![(Some(verifying), None)]),
            Err(Error::Stale)
        ));
        lookup(&extended);
        let expired = store.expired(later, 0, 10);
        assert_eq!(expired.len(), 1);
        assert!(Arc::ptr_eq(&expired[0], &extended));

        let final_owner = if accept_final {
            let (_, reject) =
                commit_admission(&store, &extended, 1, 1, Status::Pending, &config()).unwrap();
            assert!(reject.is_none());
            let accepted = store.point(&extended.hash()).1.unwrap();
            assert!(accepted.accepted().is_some());
            accepted
        } else {
            let local = extended.with_source(Source::Local);
            commit_pairs(&store, vec![(Some(extended), Some(Arc::clone(&local)))]).unwrap();
            local
        };
        lookup(&final_owner);
        assert!(store.expired(later, 0, 10).is_empty());
        chain::commit_clear(&store, None, ClearScope::All).unwrap();
        assert!(store.compact_lookup(&[proposal]).1.is_empty());
        assert!(store.expired(later, 0, 10).is_empty());
        assert!(!store.is_faulted());
    }
}

#[test]
fn accepted_expiry_tracks_current_owners_through_metadata_and_phase_changes() {
    let store = store();
    let parent = accept(&store, output_tx(6890), 1, 1, Status::Pending);
    let hash = accept(
        &store,
        spend(6891, &[OutPoint::new(parent.clone(), 0)], &[]),
        1,
        1,
        Status::Pending,
    );
    let assert_expiry = |before, expected: Option<&Arc<Entry>>| {
        let owners: Vec<_> = store
            .expired(Instant::now(), before, 10)
            .into_iter()
            .filter(|entry| entry.hash() == hash)
            .collect();
        match expected {
            Some(expected) => {
                assert_eq!(owners.len(), 1);
                assert!(Arc::ptr_eq(&owners[0], expected));
            }
            None => assert!(owners.is_empty()),
        }
    };
    let owner = store.point(&hash).1.unwrap();
    assert_expiry(0, None);
    assert_expiry(1, Some(&owner));
    let mut accepted = owner.accepted().unwrap().clone();
    accepted.parents.clear();
    let after = owner.with_phase(Phase::Accepted(accepted.clone()));
    commit_pairs(
        &store,
        vec![
            (store.point(&parent).1, None),
            (Some(owner), Some(Arc::clone(&after))),
        ],
    )
    .unwrap();
    let owner = after;
    assert_expiry(1, Some(&owner));

    let view = store.snapshot().0;
    let mut premises = Premises::default();
    assert!(Arc::ptr_eq(
        &store.get(&hash, &mut premises).unwrap().unwrap(),
        &owner
    ));
    accepted.forced_status = Some(Status::Gap);
    let owner = replace(&store, owner, Phase::Accepted(accepted.clone()));
    assert_expiry(1, Some(&owner));
    assert!(matches!(
        store.check_premises(view, &premises),
        Err(Error::Stale)
    ));
    accepted.timestamp = 10;
    let owner = replace(&store, owner, Phase::Accepted(accepted.clone()));
    assert_expiry(10, None);
    assert_expiry(11, Some(&owner));
    let owner = replace(&store, owner, Phase::Resolve);
    assert_expiry(u64::MAX, None);
    let owner = replace(&store, owner, Phase::Accepted(accepted));
    assert_expiry(10, None);
    assert_expiry(11, Some(&owner));
    delete(&store, Arc::clone(&owner)).unwrap();
    assert_expiry(u64::MAX, None);
    let mut accepted = owner.accepted().unwrap().clone();
    accepted.timestamp = 20;
    let successor = owner.with_phase(Phase::Accepted(accepted));
    insert(&store, Arc::clone(&successor));
    assert_expiry(20, None);
    assert_expiry(21, Some(&successor));
    assert!(!store.is_faulted());
}

#[test]
fn expiry_capture_and_metrics_are_bounded_read_only_projections() {
    let store = store();
    let now = Instant::now();
    let mut due = BTreeSet::new();
    for nonce in 0..70 {
        let source = Source::Remote {
            origin: RemoteOrigin {
                peer: (nonce as usize).into(),
                deadline: now - Duration::from_secs(1),
            },
            cycles: Some(1),
        };
        let owner = entry(&store, tx(6800 + nonce), source);
        due.insert(owner.hash());
        insert(&store, owner);
    }
    let future = entry(
        &store,
        tx(6900),
        Source::Remote {
            origin: RemoteOrigin {
                peer: 71.into(),
                deadline: now + Duration::from_secs(60),
            },
            cycles: Some(1),
        },
    );
    insert(&store, Arc::clone(&future));
    accepted(&store, output_tx(6901));
    let before = store.capture_all().owners;
    let charge = store.accepted_usage();
    assert!(store.expired(now, 0, 0).is_empty());
    let first = store.expired(now, 0, 32);
    assert_eq!(first.len(), 32);
    assert!(first.iter().all(|entry| due.contains(&entry.hash())));
    store.publish_budget_metrics();
    store.outbox.publish_metrics();
    assert_eq!(store.accepted_usage(), charge);
    assert!(
        before
            .iter()
            .all(|old| Arc::ptr_eq(old, &store.point(&old.hash()).1.unwrap()))
    );
    let first_hashes: BTreeSet<_> = first.iter().map(|entry| entry.hash()).collect();
    for owner in first {
        delete(&store, owner).unwrap();
    }
    let remaining = store.expired(now, 0, 100);
    let remaining_hashes: BTreeSet<_> = remaining.iter().map(|entry| entry.hash()).collect();
    assert_eq!(
        remaining_hashes,
        due.difference(&first_hashes).cloned().collect()
    );
    assert!(store.point(&future.hash()).1.is_some());
}

#[test]
fn capacity_pressure_evicts_or_rejects_by_fee_policy_in_one_cut() {
    for fee in [1, 1_000] {
        let configuration = TxPoolConfig {
            max_tx_pool_size: tx(7100).data().serialized_size_in_block(),
            ..config()
        };
        let store = Store::new(chain_snapshot(), &configuration).unwrap();
        let candidate = entry(&store, tx(7100), Source::Local);
        let incumbent = accept(&store, tx(7101), 100, 1, Status::Pending);
        let (_, reject) =
            commit_admission(&store, &candidate, fee, 1, Status::Pending, &configuration).unwrap();
        assert_eq!(reject.is_some(), fee < 100);
        let expected = if fee < 100 {
            incumbent
        } else {
            candidate.hash()
        };
        let accepted = store.capture_accepted().owners;
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].hash(), expected);
        assert_eq!(store.accepted_usage().items, 1);
        assert!(!store.is_faulted());
    }
}

#[test]
fn dry_run_admission_preserves_entire_state_and_outbox() {
    let configuration = TxPoolConfig {
        max_tx_pool_size: tx(7200).data().serialized_size_in_block(),
        ..config()
    };
    let store = Store::new(chain_snapshot(), &configuration).unwrap();
    let candidate = entry(&store, tx(7200), Source::Local);
    let reject =
        preview_admission(&store, &candidate, 100, 1, Status::Pending, &configuration).unwrap();
    assert!(reject.is_none());
    assert_eq!(store.accepted_usage(), Default::default());
    assert!(store.capture_all().owners.is_empty());
    assert!(store.compact_lookup(&[candidate.proposal()]).1.is_empty());
    store.outbox.close();
    assert!(store.outbox.drained());
    assert!(!store.is_faulted());
}

#[test]
fn transient_wake_views_release_payload_before_dirty_keys_and_publication() {
    use ckb_types::{bytes::Bytes, packed::Transaction};
    use std::{future::Future, task::Context};

    let store = store();
    let input = OutPoint::new(tx(30_070).hash(), 0);
    let transaction = spend(30_071, std::slice::from_ref(&input), &[]);
    let bytes = transaction.data().as_bytes();
    let mut source = vec![0; 128 * 1024];
    source[1_024..1_024 + bytes.len()].copy_from_slice(&bytes);
    let source: Arc<[u8]> = source.into();
    let address = source.as_ptr() as usize;
    let backing = Arc::downgrade(&source);
    let transaction =
        Transaction::new_unchecked(Bytes::from_owner(source).slice(1_024..1_024 + bytes.len()))
            .into_view();
    let point = transaction.input_pts_iter().next().unwrap();
    assert!(
        (address..address + 128 * 1024).contains(&(point.as_slice().as_ptr() as usize)),
        "the input view must retain the fixture's large backing"
    );
    drop(point);
    let candidate = entry(&store, transaction, Source::Local);
    let hash = candidate.hash();
    let verified = verification_fixture(&store, &candidate, 1, 1, Status::Pending);
    let (_, rejected) = membership::commit_admission(
        &store,
        &candidate,
        None,
        &verified,
        &config(),
        membership::AdmissionOptions {
            retain_history: false,
            dry_run: false,
            silence_effects: true,
            forbid_displacement: false,
        },
    )
    .unwrap();
    assert!(rejected.is_none());
    drop(candidate);
    drop(verified);
    assert_eq!(backing.strong_count(), 1);

    let waiter = entry(&store, tx(30_072), Source::Recovery)
        .with_phase(Phase::Waiting([input.clone()].into()));
    insert(&store, Arc::clone(&waiter));
    assert!(store.state_read().dirty_wakes.is_empty());
    let accepted = store.point(&hash).1.unwrap();
    let (_, batch) = store
        .commit_with(|tx| {
            tx.commit_single(
                accepted.hash_bytes(),
                Some(Arc::clone(&accepted)),
                None,
                vec![Effect::reset()],
                Class::Trusted,
            )
        })
        .unwrap();
    drop(accepted);
    let batch = batch.unwrap();
    let input_key = OutPointKey::from_out_point(&input);
    assert!(store.state_read().dirty_wakes.contains(&input_key));
    assert!(store.state_read().waiters.contains_key(&input_key));
    let mut completion = std::pin::pin!(batch.wait(&store.outbox));
    assert!(
        completion
            .as_mut()
            .poll(&mut Context::from_waker(std::task::Waker::noop()))
            .is_pending()
    );
    assert!(
        backing.upgrade().is_none(),
        "dirty wake keys and an unpublished batch must not retain the retired payload"
    );
    delete(&store, waiter).unwrap();
    assert!(store.state_read().dirty_wakes.is_empty());
    assert!(!store.is_faulted());
}

#[test]
fn dirty_keys_retire_with_their_last_owner_without_maintenance() {
    let store = store();
    for nonce in 30_000..30_064 {
        let parent = output_tx(nonce);
        let key = OutPoint::new(parent.hash(), 0);
        let waiter = entry(&store, tx(nonce + 100), Source::Recovery)
            .with_phase(Phase::Waiting([key.clone()].into()));
        insert(&store, Arc::clone(&waiter));
        let producer = accept(&store, parent, 1, 1, Status::Pending);
        assert_eq!(store.state_read().dirty_wakes.len(), 1);
        delete(&store, waiter).unwrap();
        assert!(store.state_read().dirty_wakes.is_empty());
        assert!(
            !store
                .state_read()
                .waiters
                .contains_key(&OutPointKey::from_out_point(&key))
        );
        delete(&store, store.point(&producer).1.unwrap()).unwrap();
        assert!(store.capture_all().owners.is_empty());
    }
    assert!(store.state_read().waiters.is_empty());
    assert!(!store.is_faulted());
}

#[test]
fn wake_completion_retires_row_and_old_pass_cannot_erase_reinserted_work() {
    let store = store();
    let parent = output_tx(30_200);
    let key = OutPoint::new(parent.hash(), 0);
    let waiter =
        entry(&store, tx(30_201), remote(1, 1)).with_phase(Phase::Waiting([key.clone()].into()));
    insert(&store, Arc::clone(&waiter));
    let producer = accept(&store, parent.clone(), 1, 1, Status::Pending);
    let old_page = store.wake_page(&mut None).unwrap();
    waiting::commit_wake(&store, &mut None).unwrap().unwrap();
    assert!(store.state_read().dirty_wakes.is_empty());
    assert!(
        !store
            .state_read()
            .waiters
            .contains_key(&OutPointKey::from_out_point(&key))
    );
    assert!(matches!(
        store.point(&waiter.hash()).1.unwrap().phase,
        Phase::Resolve
    ));
    delete(&store, store.point(&producer).1.unwrap()).unwrap();
    replace(
        &store,
        store.point(&waiter.hash()).1.unwrap(),
        Phase::Waiting([key].into()),
    );
    accept(&store, parent, 1, 1, Status::Pending);
    // Advancing with the old pass is a no-op because the active wake has a new pass.
    store
        .commit_with(|tx| {
            tx.advance_wake(
                &old_page.key(),
                old_page.pass(),
                old_page.hashes().last().copied(),
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(store.state_read().dirty_wakes.len(), 1);
    waiting::commit_wake(&store, &mut None).unwrap().unwrap();
    assert!(store.state_read().dirty_wakes.is_empty());
    assert!(matches!(
        store.point(&waiter.hash()).1.unwrap().phase,
        Phase::Resolve
    ));
    assert!(!store.is_faulted());
}

#[test]
fn dirty_retirement_preserves_spender_and_clear_releases_all_queued_keys() {
    let store = store();
    let parent = output_tx(30_300);
    let point = OutPoint::new(parent.hash(), 0);
    let waiter =
        entry(&store, tx(30_301), remote(1, 1)).with_phase(Phase::Waiting([point.clone()].into()));
    insert(&store, Arc::clone(&waiter));
    accept(&store, parent, 1, 1, Status::Pending);
    let spender = accept(
        &store,
        spend(30_302, std::slice::from_ref(&point), &[]),
        1,
        1,
        Status::Pending,
    );
    delete(&store, waiter).unwrap();
    assert_eq!(
        store.spender(&point, &mut Premises::default()).unwrap(),
        Some(spender.clone())
    );
    let point_key = OutPointKey::from_out_point(&point);
    assert!(store.state_read().dirty_wakes.is_empty());
    assert!(!store.state_read().waiters.contains_key(&point_key));
    delete(&store, store.point(&spender).1.unwrap()).unwrap();
    assert!(!store.state_read().spenders.contains_key(&point_key));

    let waiter =
        entry(&store, tx(30_303), remote(1, 1)).with_phase(Phase::Waiting([point.clone()].into()));
    insert(&store, waiter);
    store
        .commit_with(|tx| {
            tx.state.start_wake(&point_key)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(store.state_read().dirty_wakes.len(), 1);
    chain::commit_clear(&store, None, ClearScope::All).unwrap();
    assert!(store.state_read().dirty_wakes.is_empty());
    assert!(store.state_read().waiters.is_empty());
    let waiter =
        entry(&store, tx(30_304), remote(1, 1)).with_phase(Phase::Waiting([point.clone()].into()));
    insert(&store, waiter);
    store
        .commit_with(|tx| {
            tx.state.start_wake(&point_key)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(store.state_read().dirty_wakes.len(), 1);
    assert_eq!(store.wake_page(&mut None).unwrap().key(), point_key);
    assert!(!store.is_faulted());
}

#[test]
fn repeated_availability_advances_pass_without_duplicating_dirty_membership() {
    let store = store();
    let key = OutPoint::new(tx(30_400).hash(), 0);
    let key_op = OutPointKey::from_out_point(&key);
    let waiter =
        entry(&store, tx(30_401), remote(1, 1)).with_phase(Phase::Waiting([key.clone()].into()));
    insert(&store, waiter);
    store
        .commit_with(|tx| {
            tx.state.start_wake(&key_op)?;
            Ok(())
        })
        .unwrap();
    let first = store.wake_page(&mut None).unwrap();
    for _ in 0..64 {
        store
            .commit_with(|tx| {
                tx.state.start_wake(&key_op)?;
                Ok(())
            })
            .unwrap();
    }
    assert_eq!(store.state_read().dirty_wakes.len(), 1);
    let current = store.wake_page(&mut None).unwrap();
    assert!(current.pass() > first.pass());
    // Advancing with the older pass cannot clear the newer wake pass.
    store
        .commit_with(|tx| {
            tx.advance_wake(&first.key(), first.pass(), first.hashes().last().copied());
            Ok(())
        })
        .unwrap();
    assert_eq!(store.state_read().dirty_wakes.len(), 1);
    assert!(!store.is_faulted());
}

#[test]
fn pending_ranks_cannot_cross_a_clear_that_reuses_accepted_revision() {
    let pool = store();
    let transaction = tx(10639);
    let hash = accept(&pool, transaction.clone(), 1, 1, Status::Pending);
    let (view, accepted_revision, rank_generation) = {
        let state = pool.state_read();
        (state.view, state.accepted_revision, state.rank_generation)
    };
    pool.ranked_detail(&hash, 25).unwrap();
    chain::commit_clear(&pool, None, ClearScope::All).unwrap();
    assert_eq!(pool.state_read().accepted_revision, 0);
    accept(&pool, transaction, 10_000, 1, Status::Pending);
    assert_eq!(pool.state_read().accepted_revision, accepted_revision);
    // Attempting to install ranks from the pre-clear view must be rejected.
    pool.install_pending_ranks(
        view,
        accepted_revision,
        rank_generation,
        RankedPriorities::empty(rank_generation),
    );
    let competitor = accept(&pool, tx(10640), 10, 1, Status::Pending);
    assert_eq!(
        pool.ranked_detail(&competitor, 25).unwrap().rank_in_pending,
        2
    );
}
