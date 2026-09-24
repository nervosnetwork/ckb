//! Repeated mixed work through one public service generation.
//!
//! Short runs protect ordering and cleanup in the normal suite. The explicit
//! extended run observes the same workload over many rounds; RSS and latency
//! are diagnostics, not a replacement for uninstrumented benchmark comparisons.
use super::*;
use crate::{
    authority::budget::OwnerUsage,
    network::TxPoolNetwork,
    service::{TxPoolController, TxPoolServiceBuilder, TxVerificationResultReceiver},
};
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_dao_utils::genesis_dao_data;
use ckb_store::{attach_block_cell, detach_block_cell};
use ckb_test_chain_utils::{MockStore, always_success_cell, always_success_consensus};
use ckb_types::{
    bytes::Bytes,
    core::{BlockBuilder, BlockView, EpochNumberWithFraction, FeeRate, TransactionBuilder},
    packed::{CellInput, CellOutput},
    prelude::*,
};
use futures_util::FutureExt;
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::Write,
    panic::{AssertUnwindSafe, resume_unwind},
    sync::{
        Mutex as StdMutex, Weak,
        atomic::{AtomicUsize, Ordering},
    },
};

#[path = "../../../benches/resource_phases/memory.rs"]
mod memory;

const COHORTS: usize = 4;
const FUNDING_CAPACITY: u64 = 100_000_000_000_000;
const PRESSURE_BYTES: usize = 256 * 1024;
const MAX_PRESSURE_TRANSACTIONS: usize = 256;
const PRESSURE_PEER: usize = 41;

struct ChainFixture {
    database: MockStore,
    base: Arc<Snapshot>,
    funding_hash: Byte32,
}

impl ChainFixture {
    fn new() -> Self {
        let funding = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::null(), 0))
            .outputs(std::iter::repeat_n(
                CellOutput::new_builder()
                    .capacity(FUNDING_CAPACITY)
                    .lock(always_success_cell().2.clone())
                    .build(),
                COHORTS + 3,
            ))
            .outputs_data(std::iter::repeat_n(Bytes::new().pack(), COHORTS + 3))
            .build();
        let original = always_success_consensus();
        let funding_hash = funding.hash();
        let mut transactions = original.genesis_block().transactions();
        transactions.push(funding);
        let dao = genesis_dao_data(transactions.iter().collect()).unwrap();
        let genesis = original
            .genesis_block()
            .as_advanced_builder()
            .set_transactions(transactions)
            .dao(dao)
            .build();
        let consensus: Arc<Consensus> = Arc::new(
            ConsensusBuilder::default()
                .genesis_block(genesis)
                .cellbase_maturity(EpochNumberWithFraction::new(0, 0, 1))
                .build(),
        );
        let (database, base) = chain_store(consensus);
        Self {
            database,
            base,
            funding_hash,
        }
    }

    fn transaction(&self, input: OutPoint, capacity: u64, round: usize) -> TransactionView {
        funded_tx(input, capacity)
            .as_advanced_builder()
            .set_outputs_data(vec![
                Bytes::copy_from_slice(&(round as u64).to_le_bytes()).pack(),
            ])
            .build()
    }

    fn root(&self, slot: usize, round: usize) -> TransactionView {
        self.transaction(
            OutPoint::new(self.funding_hash.clone(), slot as u32),
            FUNDING_CAPACITY - 1_000,
            round,
        )
    }

    fn attach(
        &self,
        transactions: Vec<TransactionView>,
        round: usize,
    ) -> (BlockView, Arc<Snapshot>) {
        let genesis = self.base.consensus().genesis_block();
        let cellbase = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(1))
            .witness(Bytes::copy_from_slice(&(round as u64).to_le_bytes()).pack())
            .build();
        let block = BlockBuilder::default()
            .number(1)
            .parent_hash(genesis.hash())
            .timestamp(genesis.timestamp() + 1)
            .epoch(self.base.epoch_ext().number_with_fraction(1))
            .compact_target(genesis.compact_target())
            .dao(genesis.dao())
            .transaction(cellbase)
            .transactions(transactions)
            .build();
        self.database.insert_block(&block, self.base.epoch_ext());
        let write = self.database.store().begin_transaction();
        attach_block_cell(&write, &block).unwrap();
        write.commit().unwrap();
        let snapshot = Arc::new(Snapshot::new(
            block.header(),
            self.base.total_difficulty().clone(),
            self.base.epoch_ext().clone(),
            self.database.store().get_snapshot(),
            Default::default(),
            self.base.cloned_consensus(),
        ));
        (block, snapshot)
    }

    fn detach(&self, block: &BlockView) {
        // Restore the live cells before deleting the transaction locations used
        // by detach_block_cell. The next round reuses this bounded chain height.
        let write = self.database.store().begin_transaction();
        detach_block_cell(&write, block).unwrap();
        write.commit().unwrap();
        self.database.remove_block(block);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn persistence_replay_keeps_readers_before_spenders() {
    use ckb_types::{
        core::DepType,
        packed::{CellDep, OutPointVec},
    };

    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    for (dep_type, spend_group) in [
        (DepType::Code, false),
        (DepType::DepGroup, false),
        (DepType::DepGroup, true),
    ] {
        let chain = ChainFixture::new();
        let directory = tempfile::tempdir().unwrap();
        let configuration = TxPoolConfig {
            persisted_data: directory.path().join("pool"),
            ..config()
        };
        let parent = chain.root(0, 0);
        let parent_point = OutPoint::new(parent.hash(), 0);
        // The expanded-member case makes P -> R visible only through group
        // contents: R spends independent funding, not P's output.
        let expanded_parent = dep_type == DepType::DepGroup && !spend_group;
        let member = if expanded_parent {
            parent_point.clone()
        } else {
            OutPoint::new(chain.funding_hash.clone(), 1)
        };
        let group = (dep_type == DepType::DepGroup).then(|| {
            chain
                .root(2, 0)
                .as_advanced_builder()
                .set_outputs_data(vec![
                    OutPointVec::new_builder()
                        .push(member.clone())
                        .build()
                        .as_bytes()
                        .pack(),
                ])
                .build()
        });
        let dependency = group
            .as_ref()
            .map_or_else(|| member.clone(), |group| OutPoint::new(group.hash(), 0));
        let reader = if expanded_parent {
            chain.root(3, 0)
        } else {
            chain.transaction(parent_point, FUNDING_CAPACITY - 2_000, 0)
        }
        .as_advanced_builder()
        .cell_dep(
            CellDep::new_builder()
                .out_point(dependency.clone())
                .dep_type(dep_type)
                .build(),
        )
        .build();
        let spent = if spend_group { dependency } else { member };
        let spender_capacity = if dep_type == DepType::Code {
            FUNDING_CAPACITY - 1_000
        } else {
            FUNDING_CAPACITY - 2_000
        };
        let spender = chain.transaction(spent, spender_capacity, 0);
        let mut transactions = vec![parent];
        transactions.extend(group);
        transactions.extend([reader, spender]);

        for restarting in [false, true] {
            let store = Store::new(Arc::clone(&chain.base), &configuration).unwrap();
            let (pool, sink, _) = Pool::with_store(
                configuration.clone(),
                store,
                &handle,
                Arc::new(RwLock::new(init_cache())),
                None,
                None,
                FeeEstimator::new_dummy(),
            )
            .unwrap();
            let publisher =
                handle.spawn(Arc::clone(&pool.store.outbox).run(endpoints(sink, Callbacks::new())));
            if restarting {
                let replay = crate::persisted::load_persistence_snapshot(&configuration)
                    .unwrap()
                    .prepare_replay()
                    .unwrap();
                assert_eq!(
                    within(pool.replay(replay)).await.unwrap(),
                    (transactions.len(), 0),
                    "{dep_type:?}, spend_group={spend_group}"
                );
            } else {
                for transaction in &transactions {
                    within(pool.submit_local(bounded(transaction.clone())))
                        .await
                        .unwrap()
                        .unwrap();
                }
                within(pool.save()).await.unwrap();
            }
            for transaction in &transactions {
                let owner = pool.store.point(&transaction.hash()).1.unwrap();
                assert!(owner.accepted().unwrap().cycles > 0);
            }
            shutdown(&pool, JoinSet::new(), publisher).await;
        }
    }
}

#[derive(Default)]
struct Counts {
    accepted: BTreeMap<Byte32, usize>,
    rejected: BTreeMap<Byte32, usize>,
}

fn count<K: Ord>(counts: &mut BTreeMap<K, usize>, key: K) {
    *counts.entry(key).or_default() += 1;
}

#[derive(Default)]
struct RelayCounts {
    accepted: BTreeMap<(Byte32, Option<PeerIndex>), usize>,
    rejected: BTreeMap<Byte32, usize>,
    parents: BTreeMap<(PeerIndex, Byte32), usize>,
    resets: usize,
}

impl RelayCounts {
    fn drain(&mut self, receiver: &TxVerificationResultReceiver) {
        while let Some(result) = receiver.try_recv() {
            match result {
                TxVerificationResult::Ok {
                    original_peer,
                    tx_hash,
                } => count(&mut self.accepted, (tx_hash, original_peer)),
                TxVerificationResult::Reject { tx_hash } => count(&mut self.rejected, tx_hash),
                TxVerificationResult::UnknownParents { peer, parents } => {
                    for parent in parents {
                        count(&mut self.parents, (peer, parent));
                    }
                }
                TxVerificationResult::GenerationReset => self.resets += 1,
            }
        }
    }
}

/// Fixture events determine both notification populations independently of
/// production publication. Counts retain repeated acceptance after recovery.
#[derive(Default)]
struct ExpectedTerminals {
    callbacks: Counts,
    relay: RelayCounts,
}

impl ExpectedTerminals {
    fn accepted(&mut self, hash: Byte32, peer: Option<PeerIndex>) {
        count(&mut self.callbacks.accepted, hash.clone());
        count(&mut self.relay.accepted, (hash, peer));
    }

    fn replaced(&mut self, hash: Byte32) {
        count(&mut self.callbacks.rejected, hash.clone());
        count(&mut self.relay.rejected, hash);
    }

    fn assert_matches(&self, callbacks: &StdMutex<Counts>, relay: &RelayCounts) {
        assert_eq!(
            relay.accepted, self.relay.accepted,
            "exact relay acceptance population"
        );
        assert_eq!(
            relay.rejected, self.relay.rejected,
            "exact relay rejection population"
        );
        assert_eq!(
            relay.parents, self.relay.parents,
            "exact parent requests without duplicates"
        );
        assert_eq!(relay.resets, self.relay.resets);
        let actual = std::mem::take(&mut *callbacks.lock().unwrap());
        assert_eq!(
            actual.accepted, self.callbacks.accepted,
            "exact callback acceptance population"
        );
        assert_eq!(
            actual.rejected, self.callbacks.rejected,
            "exact replacement callback population"
        );
    }
}

struct HeldCallback {
    hash: Byte32,
    entered: tokio::sync::oneshot::Sender<()>,
    released: std::sync::mpsc::Receiver<()>,
}

/// Even an assertion unwind releases the callback before joined shutdown.
struct ReleaseCallback(std::sync::mpsc::Sender<()>);
impl Drop for ReleaseCallback {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

struct NoBans(Arc<AtomicUsize>);
impl TxPoolNetwork for NoBans {
    fn ban_peer(&self, _: PeerIndex, _: Duration, _: String) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct Retired {
    owners: Vec<Weak<Entry>>,
    transactions: Vec<Weak<TransactionView>>,
    resolutions: Vec<Weak<ckb_types::core::cell::ResolvedTransaction>>,
}

impl Retired {
    fn capture(&mut self, pool: &Pool) {
        for owner in pool.store.capture_all().owners {
            self.owners.push(Arc::downgrade(&owner));
            self.transactions.push(Arc::downgrade(&owner.transaction));
            match &owner.phase {
                Phase::Accepted(accepted) => {
                    self.resolutions.push(Arc::downgrade(&accepted.transaction))
                }
                Phase::Verify(resolved) => {
                    self.resolutions.push(Arc::downgrade(&resolved.transaction))
                }
                _ => {}
            }
        }
    }

    fn released(&self) -> bool {
        self.owners.iter().all(|owner| owner.strong_count() == 0)
            && self.transactions.iter().all(|tx| tx.strong_count() == 0)
            && self
                .resolutions
                .iter()
                .all(|resolved| resolved.strong_count() == 0)
    }
}

async fn until(pool: &Pool, event: &str, condition: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            assert!(!pool.is_faulted(), "service generation faulted");
            if condition() {
                break;
            }
            // Each predicate observes the actual state/lifetime in question.
            // Yielding only schedules its producer; elapsed time is not proof.
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {event}"));
}

fn submit_local(controller: &TxPoolController, tx: TransactionView) -> JoinHandle<Duration> {
    let controller = controller.clone();
    tokio::task::spawn_blocking(move || {
        let start = Instant::now();
        controller.submit_local_tx(tx).unwrap().unwrap();
        start.elapsed()
    })
}

fn reconcile(
    controller: &TxPoolController,
    detached: Vec<BlockView>,
    attached: Vec<BlockView>,
    snapshot: Arc<Snapshot>,
) -> JoinHandle<Duration> {
    let controller = controller.clone();
    tokio::task::spawn_blocking(move || {
        let start = Instant::now();
        controller
            .update_tx_pool_for_reorg(detached.into(), attached.into(), snapshot)
            .unwrap();
        start.elapsed()
    })
}

async fn clear(controller: &TxPoolController, snapshot: Option<Arc<Snapshot>>) -> Duration {
    let controller = controller.clone();
    within(tokio::task::spawn_blocking(move || {
        let start = Instant::now();
        if let Some(snapshot) = snapshot {
            controller.clear_pool(snapshot).unwrap();
        } else {
            controller.clear_verify_queue().unwrap();
        }
        start.elapsed()
    }))
    .await
    .unwrap()
}

struct RoundResult {
    local: Duration,
    attach: Duration,
    detach: Duration,
    recovery: Duration,
    clear: Duration,
    pressure_accepted: usize,
    owner_observations: usize,
}

async fn round(
    index: usize,
    chain: &ChainFixture,
    controller: &TxPoolController,
    pool: &Pool,
    receiver: &TxVerificationResultReceiver,
    callbacks: &StdMutex<Counts>,
    gate: &StdMutex<Option<HeldCallback>>,
) -> RoundResult {
    let mut expected = ExpectedTerminals::default();
    let mut relay = RelayCounts::default();
    let mut retired = Retired::default();
    let mut survivors = Vec::new();
    let mut replacements = Vec::new();

    for slot in 0..COHORTS {
        let parent = chain.root(slot, index);
        let child = chain.transaction(
            OutPoint::new(parent.hash(), 0),
            FUNDING_CAPACITY - 2_000,
            index,
        );
        let grandchild = chain.transaction(
            OutPoint::new(child.hash(), 0),
            FUNDING_CAPACITY - 3_000,
            index,
        );
        let replacement = chain.transaction(
            OutPoint::new(parent.hash(), 0),
            FUNDING_CAPACITY - 11_000,
            index,
        );
        for tx in [&parent, &child, &grandchild] {
            within(submit_local(controller, tx.clone())).await.unwrap();
            expected.accepted(tx.hash(), None);
        }
        for tx in [&child, &grandchild] {
            expected.replaced(tx.hash());
        }
        survivors.extend([parent, replacement.clone()]);
        replacements.push(replacement);
    }
    retired.capture(pool);
    let cycles = pool
        .store
        .point(&survivors[0].hash())
        .1
        .unwrap()
        .accepted()
        .unwrap()
        .cycles;
    assert!(cycles > 0, "initial transactions ran the canonical VM");

    let late_parent = chain.root(COHORTS, index);
    let late_child = chain.transaction(
        OutPoint::new(late_parent.hash(), 0),
        FUNDING_CAPACITY - 2_000,
        index,
    );
    within(controller.submit_remote_tx(late_child.clone(), cycles, 2.into()))
        .await
        .unwrap();
    observe(pool, &late_child.hash(), |entry| {
        matches!(entry.phase, Phase::Waiting(_))
    })
    .await;
    count(&mut expected.relay.parents, (2.into(), late_parent.hash()));

    // Occupy a real peer's queued residency with valid, missing-parent bodies.
    // The producer stops at the first observed terminal refusal, with a fixed
    // maximum population protecting the harness itself from an unbounded loop.
    let mut pressure_accepted = 0;
    for item in 0..MAX_PRESSURE_TRANSACTIONS {
        let mut bytes = [0xf1; 32];
        bytes[..8].copy_from_slice(&(index as u64).to_le_bytes());
        bytes[8..16].copy_from_slice(&(item as u64).to_le_bytes());
        let parent = Byte32::new(bytes);
        let transaction = funded_tx(OutPoint::new(parent.clone(), 0), FUNDING_CAPACITY - 1_000)
            .as_advanced_builder()
            .set_outputs_data(vec![Bytes::from(vec![0x5a; PRESSURE_BYTES]).pack()])
            .build();
        within(controller.submit_remote_tx(transaction.clone(), cycles, PRESSURE_PEER.into()))
            .await
            .unwrap();
        relay.drain(receiver);
        if relay.rejected.contains_key(&transaction.hash()) {
            // A refused candidate has no removal callback.
            count(&mut expected.relay.rejected, transaction.hash());
            break;
        }
        observe(pool, &transaction.hash(), |entry| {
            matches!(entry.phase, Phase::Waiting(_))
        })
        .await;
        count(&mut expected.relay.parents, (PRESSURE_PEER.into(), parent));
        pressure_accepted += 1;
    }
    assert!(
        pressure_accepted < MAX_PRESSURE_TRANSACTIONS,
        "the declared pressure population reaches a terminal refusal"
    );
    assert!(
        pressure_accepted > 0,
        "pressure includes admitted owners before refusal"
    );
    until(pool, "publication drain", || {
        pool.store.outbox.idle_for_test()
    })
    .await;
    relay.drain(receiver);
    retired.capture(pool);

    let probe = chain.root(COHORTS + 1, index);
    let (entered, entry) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    assert!(
        gate.lock()
            .unwrap()
            .replace(HeldCallback {
                hash: probe.hash(),
                entered,
                released
            })
            .is_none()
    );
    let release = ReleaseCallback(release);
    // Drive the real direct-submission future ourselves for the ordering probe.
    // Once its callback has entered, polling Pending proves publication blocks
    // completion without depending on another task being scheduled. Other local
    // submissions below use the public controller and supply the latency sample.
    let mut probe_submission = Box::pin(pool.submit_local(bounded(probe.clone())));
    tokio::select! {
        result = within(entry) => result.unwrap(),
        result = probe_submission.as_mut() => panic!("local completed before callback entry: {result:?}"),
    }
    assert!(
        futures_util::poll!(probe_submission.as_mut()).is_pending(),
        "the local response waits for its entered callback"
    );
    expected.accepted(probe.hash(), None);
    survivors.push(probe);

    // Wake and verify the remote child while the blocked local callers still
    // leave an active-work envelope. Each direct caller retains its verified
    // data and active charge until its publication response completes.
    let late = submit_local(controller, late_parent.clone());
    observe(pool, &late_parent.hash(), |entry| {
        entry.accepted().is_some()
    })
    .await;
    observe(pool, &late_child.hash(), |entry| entry.accepted().is_some()).await;
    for (tx, peer) in [(&late_parent, None), (&late_child, Some(2.into()))] {
        expected.accepted(tx.hash(), peer);
    }
    let mut submissions = vec![late];
    survivors.extend([late_parent, late_child]);
    for tx in replacements {
        let submission = submit_local(controller, tx.clone());
        observe(pool, &tx.hash(), |entry| entry.accepted().is_some()).await;
        assert!(
            !submission.is_finished(),
            "replacement committed behind the blocked publisher"
        );
        expected.accepted(tx.hash(), None);
        submissions.push(submission);
    }
    retired.capture(pool);

    let (block, attached) = chain.attach(survivors.clone(), index);
    let attachment = reconcile(controller, Vec::new(), vec![block.clone()], attached);
    until(pool, "chain attachment commit", || {
        pool.store.snapshot().1.tip_hash() == block.hash()
    })
    .await;
    assert!(
        !attachment.is_finished(),
        "chain commit precedes its blocked publication response"
    );
    assert!(futures_util::poll!(probe_submission.as_mut()).is_pending());
    assert!(submissions.iter().all(|request| !request.is_finished()));
    assert_eq!(pool.store.budget.accepted_usage().items, 0);
    drop(release);
    within(probe_submission).await.unwrap().unwrap();
    let local = within(submissions.remove(0)).await.unwrap();
    for submission in submissions {
        within(submission).await.unwrap();
    }
    let attach = within(attachment).await.unwrap();
    until(pool, "publication drain", || {
        pool.store.outbox.idle_for_test()
    })
    .await;
    relay.drain(receiver);

    clear(controller, None).await;
    expected.relay.resets += 1;
    until(pool, "active work and publication release", || {
        pool.store.budget.active_is_empty_for_test() && pool.store.outbox.idle_for_test()
    })
    .await;
    pool.store.assert_empty_for_test();
    relay.drain(receiver);
    chain.detach(&block);
    let recovery_start = Instant::now();
    let detach = within(reconcile(
        controller,
        vec![block],
        Vec::new(),
        Arc::clone(&chain.base),
    ))
    .await
    .unwrap();
    for tx in &survivors {
        observe(pool, &tx.hash(), |entry| entry.accepted().is_some()).await;
        expected.accepted(tx.hash(), None);
    }
    until(pool, "active work and publication release", || {
        pool.store.budget.active_is_empty_for_test() && pool.store.outbox.idle_for_test()
    })
    .await;
    let recovery = recovery_start.elapsed();
    retired.capture(pool);

    // The same peer can submit valid work after pressure is removed. A ban or
    // leaked per-peer charge cannot be hidden by aggregate pool cleanup.
    let retry = chain.root(COHORTS + 2, index);
    within(controller.submit_remote_tx(retry.clone(), cycles, PRESSURE_PEER.into()))
        .await
        .unwrap();
    observe(pool, &retry.hash(), |entry| entry.accepted().is_some()).await;
    expected.accepted(retry.hash(), Some(PRESSURE_PEER.into()));
    until(pool, "publication drain", || {
        pool.store.outbox.idle_for_test()
    })
    .await;
    retired.capture(pool);
    relay.drain(receiver);

    let clear = clear(controller, Some(Arc::clone(&chain.base))).await;
    expected.relay.resets += 1;
    drop((survivors, retry));
    until(
        pool,
        "retired payload, active work and outbox release",
        || {
            retired.released()
                && pool.store.budget.active_is_empty_for_test()
                && pool.store.outbox.idle_for_test()
        },
    )
    .await;
    pool.store.assert_empty_for_test();
    assert_eq!(pool.store.budget.owner_usage(), OwnerUsage::default());
    relay.drain(receiver);
    expected.assert_matches(callbacks, &relay);
    assert!(gate.lock().unwrap().is_none());
    RoundResult {
        local,
        attach,
        detach,
        recovery,
        clear,
        pressure_accepted,
        owner_observations: retired.owners.len(),
    }
}

fn latency(values: &[u64]) -> serde_json::Value {
    let mut ordered = values.to_vec();
    ordered.sort_unstable();
    let percentile =
        |percent: usize| ordered[(ordered.len() * percent).div_ceil(100).saturating_sub(1)];
    serde_json::json!({"unit": "nanoseconds", "count": values.len(), "p50": percentile(50), "p95": percentile(95), "p99": percentile(99), "max": ordered.last().unwrap(), "observations": values})
}

fn record(name: &str, value: serde_json::Value, trace: &mut Option<File>) {
    println!("{name} {value}");
    if let Some(trace) = trace {
        writeln!(trace, "{name} {value}").unwrap();
    }
}

fn memory_snapshot(round: usize, phase: &str, trace: &mut Option<File>) {
    let (resident, high_water) = memory::memory_bytes().unwrap();
    record(
        "TX_POOL_STABILITY_MEMORY",
        serde_json::json!({
            "round": round, "phase": phase, "resident_bytes": resident, "lifetime_peak_rss_bytes": high_water,
        }),
        trace,
    );
}

async fn mixed_load(rounds: usize, observe_memory: bool) {
    assert!((1..=65_536).contains(&rounds));
    // An explicit, new-only trace permits inspecting a long captured Nextest run.
    // Opening it before startup cannot strand service tasks on an invalid path.
    let mut trace = observe_memory
        .then(|| std::env::var_os("TX_POOL_STABILITY_TRACE"))
        .flatten()
        .map(|path| {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .unwrap()
        });
    let directory = tempfile::tempdir().unwrap();
    let chain = ChainFixture::new();
    let configuration = TxPoolConfig {
        max_tx_pool_size: 64_000_000,
        max_tx_verify_workers: 4,
        min_rbf_rate: FeeRate::from_u64(1_000),
        persisted_data: directory.path().join("pool"),
        recent_reject: Default::default(),
        ..config()
    };
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let (mut builder, controller, receiver) = TxPoolServiceBuilder::new(
        configuration,
        Arc::clone(&chain.base),
        None,
        Arc::new(RwLock::new(init_cache())),
        &handle,
        FeeEstimator::new_dummy(),
    )
    .unwrap();
    let pool = builder.pool_for_test();
    let callbacks = Arc::new(StdMutex::new(Counts::default()));
    let gate: Arc<StdMutex<Option<HeldCallback>>> = Arc::new(StdMutex::new(None));
    let pending = Arc::clone(&callbacks);
    let held = Arc::clone(&gate);
    builder.register_pending(Box::new(move |entry| {
        let hash = entry.transaction.hash();
        count(&mut pending.lock().unwrap().accepted, hash.clone());
        let barrier = {
            let mut gate = held.lock().unwrap();
            if gate.as_ref().is_some_and(|gate| gate.hash == hash) {
                gate.take()
            } else {
                None
            }
        };
        if let Some(barrier) = barrier {
            barrier.entered.send(()).unwrap();
            barrier
                .released
                .recv_timeout(Duration::from_secs(20))
                .unwrap();
        }
    }));
    let proposed = Arc::clone(&callbacks);
    builder.register_proposed(Box::new(move |entry| {
        count(
            &mut proposed.lock().unwrap().accepted,
            entry.transaction.hash(),
        )
    }));
    let rejected = Arc::clone(&callbacks);
    builder.register_reject(Box::new(move |entry, reason| {
        assert!(
            matches!(reason, Reject::RBFRejected(_)),
            "unexpected rejection callback: {reason:?}"
        );
        count(
            &mut rejected.lock().unwrap().rejected,
            entry.transaction.hash(),
        );
    }));
    let bans = Arc::new(AtomicUsize::new(0));
    let generation = builder.start_with_handle(NoBans(Arc::clone(&bans)));
    until(&pool, "service startup", || controller.service_started()).await;
    // Reserve all retained observer arrays before the first residency sample.
    let mut locals = Vec::with_capacity(rounds);
    let mut attachments = Vec::with_capacity(rounds);
    let mut detachments = Vec::with_capacity(rounds);
    let mut recoveries = Vec::with_capacity(rounds);
    let mut clears = Vec::with_capacity(rounds);
    let sample_every = rounds.div_ceil(64).max(1);
    let mut pressure_accepted = 0;
    let mut owner_observations = 0;
    if observe_memory {
        memory_snapshot(0, "service_ready", &mut trace);
    }
    let start = Instant::now();
    let mut completed_rounds = 0;
    let result = AssertUnwindSafe(async {
        for index in 0..rounds {
            let result = round(
                index,
                &chain,
                &controller,
                &pool,
                &receiver,
                &callbacks,
                &gate,
            )
            .await;
            let nanos = |duration: Duration| u64::try_from(duration.as_nanos()).unwrap();
            locals.push(nanos(result.local));
            attachments.push(nanos(result.attach));
            detachments.push(nanos(result.detach));
            recoveries.push(nanos(result.recovery));
            clears.push(nanos(result.clear));
            pressure_accepted += result.pressure_accepted;
            owner_observations += result.owner_observations;
            assert_eq!(
                bans.load(Ordering::SeqCst),
                0,
                "capacity pressure must not ban a peer"
            );
            completed_rounds += 1;
            if observe_memory && ((index + 1) % sample_every == 0 || index + 1 == rounds) {
                memory_snapshot(index + 1, "released", &mut trace);
            }
        }
    })
    .catch_unwind()
    .await;
    let elapsed = start.elapsed();
    controller.stop();
    within(generation).await.unwrap();
    assert!(
        pool.persistence_eligible(),
        "joined generation is eligible for persistence"
    );
    assert!(!controller.service_started());
    if let Err(error) = result {
        eprintln!("TX_POOL_STABILITY_FAILURE at zero-based round {completed_rounds}");
        resume_unwind(error);
    }
    pool.store.assert_empty_for_test();
    if observe_memory {
        memory_snapshot(rounds, "joined", &mut trace);
    }
    record(
        "TX_POOL_STABILITY_RESULT",
        serde_json::json!({
            "schema_version": 1, "rounds": rounds, "cohorts_per_round": COHORTS,
            "pressure_payload_bytes": PRESSURE_BYTES, "pressure_accepted": pressure_accepted,
            "pressure_refused": rounds, "released_owner_observations": owner_observations,
            "verified_unique_hashes": rounds * (4 * COHORTS + 4),
            "elapsed_ns": elapsed.as_nanos(), "bans": bans.load(Ordering::SeqCst),
            "local_response": latency(&locals), "attach_response": latency(&attachments),
            "detach_response": latency(&detachments), "detached_recovery": latency(&recoveries), "clear_response": latency(&clears),
            "scope": "one service generation; exact callbacks/relay, post-clear owner/account/active/outbox release each round; callback gating and OS observations are diagnostic",
        }),
        &mut trace,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn mixed_load_releases_resources_and_restores_progress() {
    mixed_load(2, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "long-lived mixed service workload with RSS and latency observations"]
async fn extended_mixed_load_stability() {
    let rounds =
        std::env::var("TX_POOL_STABILITY_ROUNDS").map_or(4_096, |value| value.parse().unwrap());
    mixed_load(rounds, true).await;
}
