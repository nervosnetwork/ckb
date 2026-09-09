use super::*;
use crate::service::BoundedTransaction;
use crate::{
    authority::{
        model::{Source, Status},
        service::{Endpoints, Pool},
        tests::common::*,
    },
    callback::Callbacks,
    network::DummyTxPoolNetwork,
};
use ckb_async_runtime::Handle;
use ckb_fee_estimator::FeeEstimator;
use ckb_types::{
    core::{TransactionBuilder, TransactionView},
    packed::{CellDep, CellOutput},
};
use ckb_verification::cache::init_cache;
use std::{collections::BTreeMap, time::Duration};
use tokio::sync::{RwLock as AsyncRwLock, mpsc};

// Independent ordered-map reference for lock-footprint selection in these tests.
fn add_lock_request(footprint: &mut BTreeMap<usize, bool>, index: usize, write: bool) {
    footprint
        .entry(index)
        .and_modify(|old| *old |= write)
        .or_insert(write);
}

#[test]
fn lock_footprint_preserves_order_and_monotone_write_requests() {
    let mut footprint = LockFootprint::default();
    let mut reference = BTreeMap::new();
    let locks = std::array::from_fn(|_| RwLock::new(()));
    assert_eq!(footprint.len(), 0);
    assert!(footprint.iter().next().is_none());
    assert!(acquire(&locks, &footprint).is_empty());
    for pass in 0..3 {
        for index in (0..SHARDS).rev() {
            let write = pass == 1 && index % 3 == 0;
            footprint.insert(index, write);
            add_lock_request(&mut reference, index, write);
            assert_eq!(footprint.len(), reference.len());
            assert_eq!(
                footprint.iter().collect::<Vec<_>>(),
                reference.iter().map(|(i, w)| (*i, *w)).collect::<Vec<_>>()
            );
        }
    }
    let guards = acquire(&locks, &footprint);
    assert_eq!(guards.len(), reference.len());
    for ((index, guard), (expected, write)) in guards.iter().zip(&reference) {
        assert_eq!(index, expected);
        assert_eq!(matches!(guard, Guard::Write(_)), *write);
        assert_eq!(locks[*index].try_read().is_none(), *write);
        assert!(locks[*index].try_write().is_none());
    }
    drop(guards);
    assert!(locks.iter().all(|lock| lock.try_write().is_some()));
}

#[test]
fn owner_routing_matches_packed_proposal_hashing() {
    for _ in 0..2 {
        let store = store();
        for position in 0..32 {
            for value in 0..=u8::MAX {
                let mut bytes = [7; 32];
                bytes[position] = value;
                let hash = Byte32::new(bytes);
                let proposal = ProposalShortId::from_tx_hash(&hash);
                assert_eq!(store.owner_shard(&hash), store.route(&proposal));
            }
        }
    }
}

fn shard_identities(store: &Store) -> BTreeMap<usize, Byte32> {
    let mut identities = BTreeMap::new();
    for nonce in 0_u32..65_536 {
        let mut bytes = [0; 32];
        bytes[..4].copy_from_slice(&nonce.to_be_bytes());
        let hash = Byte32::new(bytes);
        identities.entry(store.owner_shard(&hash)).or_insert(hash);
        if identities.len() == SHARDS {
            return identities;
        }
    }
    panic!("fixture finds an identity for each physical owner shard");
}

#[test]
fn owner_edit_groups_cover_mixed_guards_and_empty_clear_shards() {
    let store = store();
    let owners: Vec<_> = shard_identities(&store)
        .into_iter()
        .map(|(index, hash)| {
            (
                index,
                entry(&store, tx(7750).fake_hash(hash), Source::Recovery),
            )
        })
        .collect();
    let mut initial = Plan::new(store.snapshot().0, Class::Trusted);
    for (_, owner) in owners.iter().rev() {
        initial.edit(None, Some(Arc::clone(owner))).unwrap();
    }
    store.apply(initial).unwrap();
    let (view, _, _, reads) = store.capture(false);
    let mut removal = Plan::new(view, Class::Trusted);
    removal.reads = reads;
    for (index, owner) in &owners {
        if index % 2 == 1 {
            removal.edit(Some(Arc::clone(owner)), None).unwrap();
        }
    }
    store.apply(removal).unwrap();
    for (index, owner) in &owners {
        let current = store.point(&owner.hash()).1;
        if index % 2 == 1 {
            assert!(current.is_none());
        } else {
            assert!(Arc::ptr_eq(&current.unwrap(), owner));
        }
    }
    // Clear takes all write guards, including shards with no remaining edits.
    // The second pass also checks a completely empty edit table.
    for _ in 0..2 {
        let (view, _, owners, reads) = store.capture(false);
        let mut clear = Plan::new(view, Class::Trusted);
        clear.reads = reads;
        clear.invalidate_view = true;
        clear.clear_all = true;
        for owner in owners {
            clear.edit(Some(owner), None).unwrap();
        }
        store.apply(clear).unwrap();
        assert_eq!(store.snapshot().0, view + 1);
        assert!(store.capture(false).2.is_empty());
    }
}

#[test]
fn owner_preflight_preserves_shard_order_between_collision_and_counter_errors() {
    for collision_first in [false, true] {
        let store = store();
        let identities = shard_identities(&store);
        let (first, last) = identities
            .iter()
            .flat_map(|first| identities.iter().map(move |last| (first, last)))
            .find(|(first, last)| first.0 < last.0 && first.1 > last.1)
            .expect("fixture finds opposite shard and full-hash orders");
        let (collision, exhausted) = if collision_first {
            (first, last)
        } else {
            (last, first)
        };
        let mut plan = Plan::new(store.snapshot().0, Class::Trusted);
        for suffix in [1, 2] {
            let mut bytes: [u8; 32] = collision.1.as_slice().try_into().unwrap();
            bytes[31] = suffix;
            plan.edit(
                None,
                Some(entry(
                    &store,
                    tx(7751).fake_hash(Byte32::new(bytes)),
                    Source::Recovery,
                )),
            )
            .unwrap();
        }
        plan.edit(
            None,
            Some(entry(
                &store,
                tx(7752).fake_hash(exhausted.1.clone()),
                Source::Recovery,
            )),
        )
        .unwrap();
        store.shards[*exhausted.0].write().revision = u64::MAX;
        let before = store.capture(false);
        let result = store.apply(plan);
        if collision_first {
            assert!(matches!(
                result,
                Err(Error::Full(FullReason::Other(
                    "proposal short-ID collision"
                )))
            ));
        } else {
            assert!(matches!(result, Err(Error::Fault("owner revision"))));
        }
        assert!(store.capture(false).2.is_empty());
        assert!(store.read_selected(before.0, &before.3, || ()).is_ok());
        assert!(!store.is_faulted());
    }
}

#[test]
fn projection_role_union_survives_phase_changes_and_duplicate_dependencies() {
    let store = store();
    let parent = accept(&store, output_tx(7700), 1, 1, Status::Pending);
    let point = OutPoint::new(parent.clone(), 0);
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
    let cell = DependencyKey::Cell(point.clone());
    let header = DependencyKey::Header(tx(7702).hash());
    let cell_key = RelationKey::Dependency(cell.clone());
    let child_key = RelationKey::Children(parent);
    let header_key = RelationKey::Dependency(header.clone());
    let roles = |key: &RelationKey| {
        store
            .relation(key)
            .and_then(|row| row.lock().members.get(&hash).map(|member| member.roles))
    };
    assert_eq!(roles(&cell_key), Some(INPUT | DEP));
    assert_eq!(roles(&child_key), Some(CHILD));
    assert_eq!(roles(&header_key), None);
    let owner = store.point(&hash).1.unwrap();
    let mut accepted = owner.accepted().unwrap().clone();
    let version = Arc::clone(&store.relation(&cell_key).unwrap().lock().accepted_version);
    accepted.timestamp += 1;
    let owner = replace(&store, owner, Phase::Accepted(accepted.clone()));
    assert_eq!(roles(&cell_key), Some(INPUT | DEP));
    assert!(Arc::ptr_eq(
        &version,
        &store.relation(&cell_key).unwrap().lock().accepted_version
    ));
    let keys = BTreeSet::from([cell, header]);
    let owner = replace(&store, owner, Phase::Waiting(keys.clone()));
    assert_eq!(roles(&cell_key), Some(WAIT));
    assert_eq!(roles(&child_key), None);
    assert_eq!(roles(&header_key), Some(WAIT));
    assert!(
        store
            .spender(&point, &mut ReadSet::default())
            .unwrap()
            .is_none()
    );
    let owner = replace(
        &store,
        owner,
        Phase::Replaced {
            triggers: keys,
            require_all: false,
        },
    );
    assert_eq!(roles(&cell_key), Some(WAIT));
    assert_eq!(roles(&header_key), Some(WAIT));
    let owner = replace(&store, owner, Phase::Resolve);
    assert_eq!(roles(&cell_key), None);
    assert_eq!(roles(&header_key), None);
    let owner = replace(&store, owner, Phase::Accepted(accepted));
    assert_eq!(roles(&cell_key), Some(INPUT | DEP));
    assert_eq!(roles(&child_key), Some(CHILD));
    assert_eq!(
        store.spender(&point, &mut ReadSet::default()).unwrap(),
        Some(hash.clone())
    );
    let mut removal = Plan::new(store.snapshot().0, Class::Trusted);
    removal.edit(Some(owner), None).unwrap();
    store.apply(removal).unwrap();
    assert_eq!(roles(&cell_key), None);
    assert_eq!(roles(&child_key), None);
    assert!(!store.is_faulted());
}

fn available_notice_batches(store: &Store) -> usize {
    let mut held = Vec::new();
    loop {
        match store.outbox.reserve(vec![Effect::reset()], Class::Trusted) {
            Ok(Some(reservation)) => held.push(reservation),
            Err(Error::Full(FullReason::NoticeOutbox)) => return held.len(),
            _ => panic!("the fixed tiny notice must reach aggregate capacity"),
        }
    }
}

fn history_pressure_plan() -> (Arc<Store>, Plan, Byte32, Arc<Entry>) {
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
    let candidate = entry(
        &store,
        funded_tx(transaction.input_pts_iter().next().unwrap(), 19_999_990_000),
        Source::Local,
    );
    let (plan, reject) = admission(
        &store,
        &candidate,
        10_000,
        1,
        Status::Pending,
        &configuration,
    )
    .unwrap();
    assert!(reject.is_none());
    assert!(matches!(
        store.apply(plan.clone()),
        Err(Error::Full(FullReason::History))
    ));
    (store, plan, victim, candidate)
}

#[test]
fn history_retry_observes_stop_before_reusing_admission() {
    let (store, plan, victim, candidate) = history_pressure_plan();
    let slots = available_notice_batches(&store);
    let original = store.point(&victim).1.unwrap();
    let stopping = Arc::downgrade(&store);
    *store.commit_observer.lock() = Some(Arc::new(move |_, locked| {
        if locked {
            stopping.upgrade().unwrap().stop();
        }
    }));
    let mut retain_history = true;
    assert!(matches!(
        store.apply_admission(plan, &mut retain_history),
        Err(Error::Stale)
    ));
    assert!(!retain_history);
    assert!(Arc::ptr_eq(&store.point(&victim).1.unwrap(), &original));
    assert!(store.point(&candidate.hash()).1.is_none());
    assert!(store.outbox.pending_reject(&victim).is_none());
    assert_eq!(available_notice_batches(&store), slots);
    assert!(!store.is_faulted());
}

#[test]
fn history_retry_revalidates_before_mutation_and_refunds_on_failure() {
    for chain_transition in [false, true] {
        let (store, plan, victim, candidate) = history_pressure_plan();
        let slots = available_notice_batches(&store);
        let charge = store.budget.accepted_usage();
        let target = candidate.hash();
        let attempts = Arc::new(AtomicU64::new(0));
        let observed = Arc::clone(&attempts);
        let (entered, events) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let wait = Mutex::new(wait);
        *store.commit_observer.lock() = Some(Arc::new(move |plan, locked| {
            if !locked
                && plan.edits.contains_key(&target)
                && observed.fetch_add(1, Ordering::AcqRel) == 1
            {
                entered.send(()).unwrap();
                wait.lock()
                    .recv_timeout(Duration::from_secs(10))
                    .expect("the test releases the second synchronous attempt");
            }
        }));
        std::thread::scope(|scope| {
            let pending = scope.spawn(|| {
                let mut retain_history = true;
                let result = store.apply_admission(plan, &mut retain_history);
                (result, retain_history)
            });
            events.recv_timeout(Duration::from_secs(5)).unwrap();
            // The failed first attempt released all authority guards, while
            // retaining exactly its original unpublished notice reservation.
            assert_eq!(available_notice_batches(&store), slots - 1);
            let pause = chain_transition.then(|| store.begin_chain().unwrap());
            let owner = store.point(&victim).1.unwrap();
            let expected = if chain_transition {
                owner
            } else {
                let mut value = owner.accepted().unwrap().clone();
                value.timestamp += 1;
                replace(&store, owner, Phase::Accepted(value))
            };
            release.send(()).unwrap();
            let (result, retain_history) = pending.join().unwrap();
            if chain_transition {
                assert!(matches!(
                    result,
                    Err(Error::Full(FullReason::ChainTransition))
                ));
            } else {
                assert!(matches!(result, Err(Error::Stale)));
            }
            assert!(!retain_history);
            assert!(Arc::ptr_eq(&store.point(&victim).1.unwrap(), &expected));
            assert!(store.point(&candidate.hash()).1.is_none());
            assert!(store.outbox.pending_reject(&victim).is_none());
            assert_eq!(store.budget.accepted_usage(), charge);
            assert_eq!(available_notice_batches(&store), slots);
            drop(pause);
        });
        *store.commit_observer.lock() = None;
        assert_eq!(attempts.load(Ordering::Acquire), 2);
        assert!(!store.is_faulted());
    }
}

#[test]
fn history_retry_without_optional_history_is_bounded_and_refunds_notices() {
    let configuration = TxPoolConfig {
        max_tx_verify_workers: 1,
        ..config()
    };
    let store = store_with_pipeline_limit(chain_snapshot(), &configuration, 64_000);
    let mut plan = Plan::new(store.snapshot().0, Class::Trusted);
    for nonce in 7500..7570 {
        plan.edit(None, Some(entry(&store, tx(nonce), Source::Local)))
            .unwrap();
    }
    plan.effects.push(Effect::reset());
    let attempts = Arc::new(AtomicU64::new(0));
    let observed = Arc::clone(&attempts);
    *store.commit_observer.lock() = Some(Arc::new(move |_, locked| {
        if !locked {
            observed.fetch_add(1, Ordering::AcqRel);
        }
    }));
    let mut retain_history = true;
    assert!(matches!(
        store.apply_admission(plan, &mut retain_history),
        Err(Error::Full(FullReason::Pipeline))
    ));
    assert!(!retain_history);
    assert_eq!(attempts.load(Ordering::Acquire), 2);
    for nonce in 7500..7570 {
        assert!(store.point(&tx(nonce).hash()).1.is_none());
    }
    store.outbox.close();
    assert!(store.outbox.drained());
    assert!(!store.is_faulted());
}

fn accepted(store: &Store, tx: TransactionView) {
    let hash = accept(store, tx, 1, 1, Status::Pending);
    let owner = store.point(&hash).1.unwrap();
    let mut value = owner.accepted().unwrap().clone();
    value.timestamp = ckb_systemtime::unix_time_as_millis();
    replace(store, owner, Phase::Accepted(value));
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
// Select physical shard footprints from the fixture's actual independent
// inputs/outputs and parents. The barrier below observes the production cut.
fn footprint(store: &Store, tx: &TransactionView) -> [BTreeMap<usize, bool>; 2] {
    let mut owners = BTreeMap::new();
    let mut dependencies = BTreeMap::new();
    for point in tx
        .input_pts_iter()
        .chain(tx.cell_deps_iter().map(|dep| dep.out_point()))
    {
        add_lock_request(&mut owners, store.owner_shard(&point.tx_hash()), false);
        add_lock_request(
            &mut dependencies,
            store.route(&RelationKey::Dependency(DependencyKey::Cell(point.clone()))),
            false,
        );
        add_lock_request(
            &mut dependencies,
            store.route(&RelationKey::Children(point.tx_hash())),
            false,
        );
    }
    add_lock_request(&mut owners, store.owner_shard(&tx.hash()), true);
    for point in tx.input_pts_iter().chain(tx.output_pts_iter()) {
        add_lock_request(
            &mut dependencies,
            store.route(&RelationKey::Dependency(DependencyKey::Cell(point))),
            true,
        );
    }
    [owners, dependencies]
}
fn compatible(a: &[BTreeMap<usize, bool>; 2], b: &[BTreeMap<usize, bool>; 2]) -> bool {
    a.iter().zip(b).all(|(a, b)| {
        a.iter()
            .all(|(index, write)| b.get(index).is_none_or(|other| !write && !other))
    })
}
async fn production_overlap(shared_code: bool) {
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
    let mut first = Vec::new();
    let mut second = Vec::new();
    for nonce in 6200..6240 {
        first.push(candidate(&store, nonce, &a_code));
        second.push(candidate(&store, nonce + 100, &b_code));
    }
    let (a, b) = first
        .iter()
        .flat_map(|a| second.iter().map(move |b| (a, b)))
        .find(|(a, b)| compatible(&footprint(&store, a), &footprint(&store, b)))
        .expect("fixture finds compatible physical shards");
    let pair = [a.clone(), b.clone()];
    let hashes = [a.hash(), b.hash()];
    let mut cycles = Vec::new();
    for tx in &pair {
        cycles.push(
            pool.submit_local(BoundedTransaction::try_new(tx.clone()).unwrap(), true)
                .await
                .unwrap()
                .unwrap()
                .cycles,
        );
    }
    // Cycle declarations are known, but both production jobs must execute the VM.
    *cache.write().await = init_cache();
    let (entered, mut events) = mpsc::unbounded_channel();
    let (release_a, wait_a) = std::sync::mpsc::channel();
    let (release_b, wait_b) = std::sync::mpsc::channel();
    let waits = [Mutex::new(wait_a), Mutex::new(wait_b)];
    let target = hashes.clone();
    *store.commit_observer.lock() = Some(Arc::new(move |plan, locked| {
        if let Some((index, _)) = target.iter().enumerate().find(|(_, hash)| {
            plan.edits.get(*hash).is_some_and(|edit| {
                edit.after
                    .as_ref()
                    .is_some_and(|entry| entry.accepted().is_some())
            })
        }) {
            entered.send((index, locked)).unwrap();
            if locked {
                waits[index]
                    .lock()
                    .recv_timeout(Duration::from_secs(10))
                    .expect("the test releases both live production cuts");
            }
        }
    }));
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
    let (mut tasks, publisher) = pool.start_background(&handle, endpoints, chain);
    for (index, tx) in pair.into_iter().enumerate() {
        pool.submit_remote(
            BoundedTransaction::try_new(tx).unwrap(),
            cycles[index],
            (index + 70).into(),
        )
        .await
        .unwrap();
    }
    let mut began = BTreeSet::new();
    let mut locked = BTreeSet::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while locked.len() != 2 {
            let (index, holds_cut) = events.recv().await.unwrap();
            if holds_cut {
                assert!(began.contains(&index));
                locked.insert(index);
            } else {
                began.insert(index);
            }
        }
    })
    .await
    .expect("both independently received jobs hold their final owner/dependency cuts at once");
    assert_eq!(began.len(), 2);
    assert!(publications.try_recv().is_err());
    release_a.send(()).unwrap();
    release_b.send(()).unwrap();
    let mut completed = BTreeSet::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while completed.len() != 2 {
            completed.insert(publications.recv().await.unwrap());
        }
    })
    .await
    .unwrap();
    assert_eq!(completed, hashes.into());
    *store.commit_observer.lock() = None;
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
async fn independent_remote_jobs_overlap_from_receive_through_both_final_commit_cuts() {
    production_overlap(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn remote_jobs_sharing_only_read_only_cell_dep_hold_final_commit_cuts_together() {
    production_overlap(true).await;
}

#[test]
fn ban_deadlines_never_shorten_and_saturation_discards_only_the_oldest_marker() {
    let store = store();
    let now = Instant::now();
    let mut initial = Plan::new(store.snapshot().0, Class::Remote);
    initial.ban = Some((1.into(), now + Duration::from_secs(10_000)));
    store.apply(initial).unwrap();
    let mut shorter = Plan::new(store.snapshot().0, Class::Remote);
    shorter.ban = Some((1.into(), now + Duration::from_secs(1)));
    store.apply(shorter).unwrap();
    assert_eq!(
        store.bans.lock().get(&1.into()),
        Some(&(now + Duration::from_secs(10_000)))
    );
    for index in 2..=crate::constants::PEER_BAN_FENCE_CAPACITY + 1 {
        let mut plan = Plan::new(store.snapshot().0, Class::Remote);
        plan.ban = Some((index.into(), now + Duration::from_secs(index as u64)));
        store.apply(plan).unwrap();
    }
    assert_eq!(
        store.bans.lock().len(),
        crate::constants::PEER_BAN_FENCE_CAPACITY
    );
    assert!(store.peer_banned(1.into()));
    assert!(!store.peer_banned(2.into()));
    assert!(store.peer_banned(3.into()));
    let delayed = funded_tx(OutPoint::new(tx(6999).hash(), 0), 20_000_000_000);
    let source = crate::authority::ingress::remote_source(2.into(), 1).unwrap();
    let plan =
        crate::authority::ingress::prepare(&store, Arc::new(delayed.clone()), source).unwrap();
    store.apply(plan).unwrap();
    assert!(
        store.point(&delayed.hash()).1.is_some(),
        "an evicted marker uses normal admission"
    );
}

#[test]
fn remaining_counter_exhaustion_rejects_before_any_owner_or_notice_change() {
    use crate::authority::notice::Effect;
    for counter in [
        "view counter",
        "owner revision",
        "accepted revision",
        "wake pass",
    ] {
        let store = store();
        let hash = accept(&store, output_tx(6400), 1000, 7, Status::Pending);
        let owner = store.point(&hash).1.unwrap();
        let key = DependencyKey::Cell(OutPoint::new(tx(6401).hash(), 0));
        let waiting = entry(&store, tx(6402), Source::Recovery)
            .with_phase(Phase::Waiting([key.clone()].into()));
        insert(&store, waiting);
        match counter {
            "view counter" => store.view.write().revision = u64::MAX,
            "owner revision" => store.shards[store.owner_shard(&hash)].write().revision = u64::MAX,
            "accepted revision" => {
                store.shards[store.owner_shard(&hash)]
                    .write()
                    .accepted_revision = u64::MAX
            }
            "wake pass" => {
                store
                    .relation(&RelationKey::Dependency(key.clone()))
                    .unwrap()
                    .lock()
                    .next_pass = u64::MAX
            }
            _ => unreachable!(),
        }
        let before = store.capture(false);
        let charge = store.budget.accepted_usage();
        let mut plan = Plan::new(before.0, Class::Trusted);
        plan.edit(
            Some(Arc::clone(&owner)),
            Some(owner.with_phase(Phase::Resolve)),
        )
        .unwrap();
        if counter == "view counter" {
            plan.invalidate_view = true;
        }
        plan.wake.insert(key);
        plan.effects.push(
            Effect::rejected(
                &hash,
                crate::error::Reject::Malformed("counter fixture".into(), String::new()),
                None,
                false,
            )
            .unwrap(),
        );
        assert!(
            matches!(store.apply(plan), Err(Error::Fault(reason)) if reason == counter),
            "{counter}"
        );
        assert_eq!(store.snapshot().0, before.0);
        assert!(Arc::ptr_eq(&store.point(&hash).1.unwrap(), &owner));
        assert_eq!(store.budget.accepted_usage(), charge);
        assert!(store.outbox.pending_reject(&hash).is_none());
        assert_eq!(store.capture(false).2.len(), before.2.len());
        assert!(store.read_selected(before.0, &before.3, || ()).is_ok());
        assert!(
            crate::authority::waiting::wake(&store, &mut None)
                .unwrap()
                .is_none()
        );
    }
    let store = store();
    store.arrival.store(u64::MAX - 1, Ordering::Release);
    assert_eq!(store.next_arrival().unwrap(), u64::MAX - 1);
    assert!(matches!(
        store.next_arrival(),
        Err(Error::Fault("arrival counter"))
    ));
    assert_eq!(store.arrival.load(Ordering::Acquire), u64::MAX);
    assert!(store.is_faulted());
    assert!(store.capture(false).2.is_empty());
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
fn projection_versions_do_not_alias_after_complete_relation_or_peer_reentry() {
    let store = store();
    let point = OutPoint::new(tx(6403).hash(), 0);
    let transaction = spend(6404, &[], std::slice::from_ref(&point));
    let hash = accept(&store, transaction.clone(), 1, 1, Status::Pending);
    let mut relation = ReadSet::default();
    let key = RelationKey::Dependency(DependencyKey::Cell(point));
    assert_eq!(
        store.members(&key, DEP, &mut relation).unwrap(),
        vec![hash.clone()]
    );
    let owner = store.point(&hash).1.unwrap();
    let mut removal = Plan::new(store.snapshot().0, Class::Trusted);
    removal.edit(Some(owner), None).unwrap();
    store.apply(removal).unwrap();
    accept(&store, transaction, 1, 1, Status::Pending);
    let mut stale = Plan::new(store.snapshot().0, Class::Trusted);
    stale.reads = relation;
    assert!(matches!(store.apply(stale), Err(Error::Stale)));
    let remote = entry(
        &store,
        tx(6405),
        crate::authority::ingress::remote_source(93.into(), 1).unwrap(),
    );
    insert(&store, Arc::clone(&remote));
    let mut peers = ReadSet::default();
    assert_eq!(
        store.peer_members(93.into(), &mut peers).unwrap(),
        vec![remote.hash()]
    );
    let mut removal = Plan::new(store.snapshot().0, Class::Trusted);
    removal.edit(Some(Arc::clone(&remote)), None).unwrap();
    store.apply(removal).unwrap();
    insert(&store, remote);
    let mut stale = Plan::new(store.snapshot().0, Class::Trusted);
    stale.reads = peers;
    assert!(matches!(store.apply(stale), Err(Error::Stale)));
}

#[test]
fn full_query_waits_for_an_atomic_multi_shard_change_and_returns_one_coherent_cut() {
    let store = store();
    let a = accept(&store, tx(6406), 1000, 7, Status::Pending);
    let b = (6407..6500)
        .map(tx)
        .find(|tx| store.owner_shard(&tx.hash()) != store.owner_shard(&a))
        .unwrap();
    let b = accept(&store, b, 2000, 11, Status::Pending);
    let mut plan = Plan::new(store.snapshot().0, Class::Trusted);
    for hash in [&a, &b] {
        let before = store.point(hash).1.unwrap();
        plan.edit(
            Some(Arc::clone(&before)),
            Some(before.with_phase(Phase::Resolve)),
        )
        .unwrap();
    }
    let (entered, ready) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let wait = Mutex::new(wait);
    *store.commit_observer.lock() = Some(Arc::new(move |_, locked| {
        if locked {
            entered.send(()).unwrap();
            wait.lock().recv_timeout(Duration::from_secs(5)).unwrap();
        }
    }));
    std::thread::scope(|scope| {
        let writer = scope.spawn(|| store.apply(plan));
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        let (started, beginning) = std::sync::mpsc::channel();
        let (result, received) = std::sync::mpsc::channel();
        let observed = &store;
        let reader = scope.spawn(move || {
            started.send(()).unwrap();
            result
                .send(crate::authority::query::summary(observed, &config()).unwrap())
                .unwrap();
        });
        beginning.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(received.try_recv().is_err());
        release.send(()).unwrap();
        writer.join().unwrap().unwrap();
        let summary = received.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            (
                summary.pending_size,
                summary.total_tx_size,
                summary.total_tx_cycles
            ),
            (0, 0, 0)
        );
        reader.join().unwrap();
    });
    *store.commit_observer.lock() = None;
    for hash in [a, b] {
        assert!(matches!(
            store.point(&hash).1.unwrap().phase,
            Phase::Resolve
        ));
    }
}

#[derive(Clone, Copy)]
enum Mutation {
    Remove,
    Promote,
    Receive { same_peer: bool },
}

async fn production_mutation_overlap(mutation: Mutation) {
    let removal = matches!(mutation, Mutation::Remove);
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let (pool, sink, _) = Pool::new(
        config(),
        chain_snapshot(),
        &handle,
        Arc::new(AsyncRwLock::new(init_cache())),
        None,
        None,
        FeeEstimator::new_dummy(),
    )
    .unwrap();
    let store = &pool.store;
    let make = |nonce| funded_tx(OutPoint::new(tx(nonce).hash(), 0), 20_000_000_000);
    let mutation_footprint = |transaction: &TransactionView| {
        let mut footprint = footprint(store, transaction);
        if removal {
            add_lock_request(
                &mut footprint[1],
                store.route(&RelationKey::Children(transaction.hash())),
                true,
            );
        }
        footprint
    };
    let candidates: Vec<_> = (6500..6756)
        .map(|nonce| {
            let transaction = make(nonce);
            let footprint = mutation_footprint(&transaction);
            (transaction, footprint)
        })
        .collect();
    // Either first candidate can collide with the shared read-only code row.
    // Select both endpoints using the full final cut, including Children(self).
    let (a, b) = candidates
        .iter()
        .enumerate()
        .find_map(|(index, a)| {
            candidates
                .iter()
                .skip(index + 1)
                .find(|b| compatible(&a.1, &b.1))
                .map(|b| (&a.0, &b.0))
        })
        .expect("fixture finds compatible complete mutation cuts");
    let pair = [a.clone(), b.clone()];
    for transaction in &pair {
        if removal {
            accepted(store, transaction.clone());
        } else if matches!(mutation, Mutation::Promote) {
            pool.submit_remote(
                BoundedTransaction::try_new(transaction.clone()).unwrap(),
                1,
                95.into(),
            )
            .await
            .unwrap();
        }
    }
    let publisher = tokio::spawn(Arc::clone(&store.outbox).run(Endpoints::new(
        Arc::new(DummyTxPoolNetwork),
        sink,
        Arc::new(Callbacks::new()),
        None,
        FeeEstimator::new_dummy(),
    )));
    let hashes = pair.clone().map(|tx| tx.hash());
    let (entered, mut events) = mpsc::unbounded_channel();
    let (release_a, wait_a) = std::sync::mpsc::channel();
    let (release_b, wait_b) = std::sync::mpsc::channel();
    let waits = [Mutex::new(wait_a), Mutex::new(wait_b)];
    let target = hashes.clone();
    *store.commit_observer.lock() = Some(Arc::new(move |plan, locked| {
        if let Some(index) = target.iter().position(|hash| plan.edits.contains_key(hash)) {
            entered.send((index, locked)).unwrap();
            if locked {
                waits[index]
                    .lock()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }
        }
    }));
    let submit = |transaction: TransactionView, index: usize| {
        let pool = Arc::clone(&pool);
        tokio::spawn(async move {
            if removal {
                assert!(
                    pool.remove_local(&transaction.hash())
                        .await
                        .unwrap()
                        .unwrap()
                );
            } else if let Mutation::Receive { same_peer } = mutation {
                pool.submit_remote(
                    BoundedTransaction::try_new(transaction).unwrap(),
                    1,
                    (95 + if same_peer { 0 } else { index }).into(),
                )
                .await
                .unwrap();
            } else {
                pool.submit_proposal_batch(vec![BoundedTransaction::try_new(transaction).unwrap()])
                    .await
                    .unwrap();
            }
        })
    };
    let first = submit(pair[0].clone(), 0);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap(),
        (0, false)
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap(),
        (0, true)
    );
    // The second request arrives only after the first holds its final cut.
    let second = submit(pair[1].clone(), 1);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap(),
        (1, false)
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap(),
        (1, true)
    );
    release_a.send(()).unwrap();
    release_b.send(()).unwrap();
    for task in [first, second] {
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
    }
    *store.commit_observer.lock() = None;
    for (index, hash) in hashes.into_iter().enumerate() {
        let current = store.point(&hash).1;
        if removal {
            assert!(current.is_none());
        } else if let Mutation::Receive { same_peer } = mutation {
            let current = current.unwrap();
            assert!(matches!(current.phase, Phase::Resolve));
            assert!(matches!(current.source, Source::Remote { peer, .. }
                if peer == (95 + if same_peer { 0 } else { index }).into()));
        } else {
            assert!(
                matches!(current.unwrap().source, Source::Proposal { remote: Some((peer, _)) } if peer == 95.into())
            );
        }
    }
    pool.stop();
    pool.close_outbox();
    tokio::time::timeout(Duration::from_secs(5), publisher)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(pool.persistence_eligible());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_same_peer_proposal_promotions_hold_their_final_production_cuts_together() {
    production_mutation_overlap(Mutation::Promote).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_disjoint_direct_removals_hold_their_final_production_cuts_together() {
    production_mutation_overlap(Mutation::Remove).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_initial_remote_ingress_from_distinct_peers_holds_final_cuts_together() {
    production_mutation_overlap(Mutation::Receive { same_peer: false }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_initial_remote_ingress_from_the_same_peer_holds_final_cuts_together() {
    production_mutation_overlap(Mutation::Receive { same_peer: true }).await;
}

#[test]
fn conflicting_input_commits_cannot_both_enter_the_validated_owner_cut() {
    let store = store();
    let input = OutPoint::new(tx(6700).hash(), 0);
    let a = entry(
        &store,
        spend(6701, std::slice::from_ref(&input), &[]),
        Source::Local,
    );
    let b = entry(&store, spend(6702, &[input], &[]), Source::Local);
    let (first, _) = admission(&store, &a, 1000, 1, Status::Pending, &config()).unwrap();
    let (second, _) = admission(&store, &b, 1000, 1, Status::Pending, &config()).unwrap();
    let hashes = [a.hash(), b.hash()];
    let (entered, events) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let wait = Mutex::new(wait);
    let target = hashes.clone();
    *store.commit_observer.lock() = Some(Arc::new(move |plan, locked| {
        let index = target
            .iter()
            .position(|hash| plan.edits.contains_key(hash))
            .unwrap();
        entered.send((index, locked)).unwrap();
        if locked {
            assert_eq!(index, 0);
            wait.lock().recv_timeout(Duration::from_secs(5)).unwrap();
        }
    }));
    std::thread::scope(|scope| {
        let one = scope.spawn(|| store.apply(first));
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)).unwrap(),
            (0, false)
        );
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)).unwrap(),
            (0, true)
        );
        let two = scope.spawn(|| store.apply(second));
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)).unwrap(),
            (1, false)
        );
        assert!(events.try_recv().is_err());
        release.send(()).unwrap();
        one.join().unwrap().unwrap();
        assert!(matches!(two.join().unwrap(), Err(Error::Stale)));
    });
    *store.commit_observer.lock() = None;
    assert!(store.point(&hashes[0]).1.unwrap().accepted().is_some());
    assert!(store.point(&hashes[1]).1.is_none());
    assert!(!store.is_faulted());
}

#[test]
fn expiry_capture_and_metrics_are_bounded_read_only_projections() {
    let store = store();
    let now = Instant::now();
    let mut due = BTreeSet::new();
    for nonce in 0..70 {
        let source = Source::Remote {
            peer: (nonce as usize).into(),
            deadline: now - Duration::from_secs(1),
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
            peer: 71.into(),
            deadline: now + Duration::from_secs(60),
            cycles: Some(1),
        },
    );
    insert(&store, Arc::clone(&future));
    accepted(&store, output_tx(6901));
    let (view, _, before, reads) = store.capture(false);
    let charge = store.budget.accepted_usage();
    assert!(store.expired(now, 0, 0).is_empty());
    let first = store.expired(now, 0, 32);
    assert_eq!(first.len(), 32);
    assert!(first.iter().all(|entry| due.contains(&entry.hash())));
    store.budget.publish_metrics();
    store.outbox.publish_metrics();
    assert_eq!(store.budget.accepted_usage(), charge);
    let mut unchanged = Plan::new(view, Class::Remote);
    unchanged.reads = reads;
    assert!(store.apply(unchanged).unwrap().is_none());
    assert!(
        before
            .iter()
            .all(|old| Arc::ptr_eq(old, &store.point(&old.hash()).1.unwrap()))
    );
    let first_hashes: BTreeSet<_> = first.iter().map(|entry| entry.hash()).collect();
    for owner in first {
        let mut plan = Plan::new(view, Class::Remote);
        plan.edit(Some(owner), None).unwrap();
        store.apply(plan).unwrap();
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
fn sparse_admission_replans_settled_capacity_using_fee_policy() {
    for fee in [1, 1_000] {
        let configuration = TxPoolConfig {
            max_tx_pool_size: tx(7100).data().serialized_size_in_block(),
            ..config()
        };
        let store = Store::new(chain_snapshot(), &configuration).unwrap();
        let candidate = entry(&store, tx(7100), Source::Local);
        let competing = (7101..7160)
            .map(tx)
            .find(|tx| store.owner_shard(&tx.hash()) != store.owner_shard(&candidate.hash()))
            .expect("independent fixture uses a disjoint owner shard");
        let (plan, reject) =
            admission(&store, &candidate, fee, 1, Status::Pending, &configuration).unwrap();
        assert!(reject.is_none());
        assert!(plan.reads.accepted.is_none() && plan.reads.all.is_none());
        let target = candidate.hash();
        let (entered, events) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let wait = Mutex::new(wait);
        *store.commit_observer.lock() = Some(Arc::new(move |plan, locked| {
            if plan.edits.contains_key(&target) {
                entered.send(locked).unwrap();
                if !locked {
                    wait.lock()
                        .recv_timeout(Duration::from_secs(10))
                        .expect("test releases the prepared admission");
                }
            }
        }));
        let incumbent = std::thread::scope(|scope| {
            let pending = scope.spawn(|| store.apply(plan));
            assert!(!events.recv_timeout(Duration::from_secs(5)).unwrap());
            // Prepared footprint and notices do not own accepted capacity. A
            // complete capture can finish while this operation has no guards.
            assert_eq!(store.budget.accepted_usage(), Default::default());
            assert!(store.capture(true).2.is_empty());
            let incumbent = accept(&store, competing, 100, 1, Status::Pending);
            let owner = store.point(&incumbent).1.unwrap();
            let charge = store.budget.accepted_usage();
            release.send(()).unwrap();
            assert!(matches!(pending.join().unwrap(), Err(Error::Stale)));
            // The second event follows successful read validation: capacity,
            // rather than a changed point/relation, required replanning.
            assert!(events.recv_timeout(Duration::from_secs(5)).unwrap());
            assert!(store.point(&candidate.hash()).1.is_none());
            assert!(Arc::ptr_eq(&store.point(&incumbent).1.unwrap(), &owner));
            assert_eq!(store.budget.accepted_usage(), charge);
            incumbent
        });
        *store.commit_observer.lock() = None;
        let (fresh, reject) =
            admission(&store, &candidate, fee, 1, Status::Pending, &configuration).unwrap();
        assert!(fresh.reads.accepted.is_some());
        assert_eq!(reject.is_some(), fee < 100);
        store.apply(fresh).unwrap();
        let expected = if fee < 100 {
            incumbent
        } else {
            candidate.hash()
        };
        let accepted = store.capture(true).2;
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].hash(), expected);
        assert_eq!(store.budget.accepted_usage().items, 1);
        assert!(!store.is_faulted());
    }
}

#[test]
fn settled_capacity_refusal_and_dry_run_preserve_the_entire_owner_cut() {
    let configuration = TxPoolConfig {
        max_tx_pool_size: tx(7200).data().serialized_size_in_block(),
        ..config()
    };
    let store = Store::new(chain_snapshot(), &configuration).unwrap();
    let candidate = entry(&store, tx(7200), Source::Local);
    let (plan, reject) =
        admission(&store, &candidate, 100, 1, Status::Pending, &configuration).unwrap();
    assert!(reject.is_none());
    let (_, _, _, empty) = store.capture(false);
    let mut dry_run = plan.clone();
    dry_run.dry_run = true;
    assert!(store.apply(dry_run).unwrap().is_none());
    assert_eq!(store.budget.accepted_usage(), Default::default());
    assert!(store.read_selected(plan.view, &empty, || ()).is_ok());
    assert!(store.capture(false).2.is_empty());
    assert!(store.compact_lookup(&[candidate.proposal()]).1.is_empty());

    store.outbox.close();
    assert!(store.outbox.drained());

    let store = Store::new(chain_snapshot(), &configuration).unwrap();
    let candidate = entry(&store, tx(7200), Source::Local);
    let (mut plan, reject) =
        admission(&store, &candidate, 100, 1, Status::Pending, &configuration).unwrap();
    assert!(reject.is_none());
    plan.effects.push(
        Effect::rejected(
            &candidate.hash(),
            crate::error::Reject::Full("uncommitted fixture".into()),
            None,
            false,
        )
        .unwrap(),
    );
    let incumbent = accept(&store, tx(7201), 1, 1, Status::Pending);
    for all in [false, true] {
        let (view, _, owners, reads) = store.capture(!all);
        let charge = store.budget.accepted_usage();
        let mut full = plan.clone();
        full.reads.merge(&reads).unwrap();
        // A full observation makes this deliberately over-capacity flat plan
        // conclusive; Store must not turn it into an endless stale retry.
        assert!(matches!(
            store.apply(full),
            Err(Error::Full(FullReason::Accepted))
        ));
        assert!(store.read_selected(view, &reads, || ()).is_ok());
        assert_eq!(store.budget.accepted_usage(), charge);
        assert!(
            owners
                .iter()
                .all(|owner| Arc::ptr_eq(owner, &store.point(&owner.hash()).1.unwrap()))
        );
        assert!(store.point(&candidate.hash()).1.is_none());
        assert!(store.compact_lookup(&[candidate.proposal()]).1.is_empty());
        assert!(store.outbox.pending_reject(&candidate.hash()).is_none());
    }
    assert!(store.point(&incumbent).1.is_some());
    assert!(!store.is_faulted());
}
