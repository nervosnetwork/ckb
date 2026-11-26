//! Production-controller chain transitions against real attached/detached cells.
//!
//! Blocks are trusted synthetic chain facts, as in the service contract tests.
//! Storage preparation and full-block consensus validation are outside the
//! measured controller call. Recovery completion has its own timed window.

use super::*;
use ckb_proposal_table::ProposalTable;
use ckb_store::detach_block_cell;
use ckb_types::{core::BlockView, core::tx_pool::Reject, packed::ProposalShortId};

const DEPTH: usize = 4;
const PRESSURE_WITNESS_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy)]
enum Work {
    Roundtrip { count: usize, roots: usize },
    RecoveryPressure { count: usize },
}

fn options(args: &[String]) -> BenchResult<(Work, usize)> {
    let (work, workers) = match args {
        [mode, count, roots, workers] if mode == "roundtrip" => {
            let count: usize = count.parse()?;
            let roots: usize = roots.parse()?;
            require(
                (DEPTH..=65_536).contains(&count)
                    && count.is_multiple_of(DEPTH)
                    && (1..=(count / DEPTH).min(1024)).contains(&roots),
                "roundtrip requires complete depth-four chains and 1..=1024 roots",
            )?;
            (Work::Roundtrip { count, roots }, workers.parse()?)
        }
        [mode, count, workers] if mode == "recovery-pressure" => {
            let count = count.parse()?;
            require(
                (1100..=2048).contains(&count),
                "recovery pressure count must be 1100..=2048",
            )?;
            (Work::RecoveryPressure { count }, workers.parse()?)
        }
        _ => {
            return Err(bench_error(
                "chain-transition roundtrip COUNT ROOTS WORKERS | recovery-pressure COUNT WORKERS",
            ));
        }
    };
    require(
        (1..=64).contains(&workers),
        "chain worker count must be 1..=64",
    )?;
    Ok((work, workers))
}

struct Chain {
    store: MockStore,
    base: Arc<Snapshot>,
    tip: Arc<Snapshot>,
    blocks: Vec<BlockView>,
    proposals: ProposalTable,
}

impl Chain {
    fn new(consensus: Arc<Consensus>) -> BenchResult<Self> {
        let proposals = ProposalTable::new(consensus.tx_proposal_window())?;
        let (store, base) = snapshot_with_genesis(consensus);
        Ok(Self {
            store,
            tip: Arc::clone(&base),
            base,
            blocks: Vec::new(),
            proposals,
        })
    }

    fn snapshot(&mut self, header: ckb_types::core::HeaderView) -> Arc<Snapshot> {
        let view = self
            .proposals
            .finalize(self.tip.proposals(), header.number());
        self.tip = Arc::new(Snapshot::new(
            header,
            U256::zero(),
            self.base.epoch_ext().clone(),
            self.store.store().get_snapshot(),
            view,
            self.base.cloned_consensus(),
        ));
        Arc::clone(&self.tip)
    }

    fn attach(
        &mut self,
        transactions: &[TransactionView],
        proposals: Vec<ProposalShortId>,
    ) -> BenchResult<BlockView> {
        let number = self.tip.tip_number() + 1;
        let cellbase = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(number))
            .build();
        let block = BlockBuilder::default()
            .number(number)
            .parent_hash(self.tip.tip_hash())
            .timestamp(self.tip.tip_header().timestamp() + 1)
            .epoch(self.base.epoch_ext().number_with_fraction(number))
            .compact_target(self.base.tip_header().compact_target())
            .dao(self.base.tip_header().dao())
            .transaction(cellbase)
            .transactions(transactions.iter().cloned())
            .proposals(proposals)
            .build();
        require(
            block.data().serialized_size_without_uncle_proposals() as u64
                <= self.base.consensus().max_block_bytes(),
            "chain fixture block exceeds consensus byte bound",
        )?;
        self.store.insert_block(&block, self.base.epoch_ext());
        let write = self.store.store().begin_transaction();
        attach_block_cell(&write, &block)?;
        write.commit()?;
        self.proposals.insert(number, block.data().proposals());
        self.snapshot(block.header());
        self.blocks.push(block.clone());
        Ok(block)
    }

    fn detach(&mut self, count: usize) -> BenchResult<VecDeque<BlockView>> {
        require(
            count <= self.blocks.len(),
            "detachment exceeds fixture height",
        )?;
        let mut detached = VecDeque::with_capacity(count);
        for _ in 0..count {
            let block = self
                .blocks
                .pop()
                .ok_or_else(|| bench_error("fixture detachment is empty"))?;
            let write = self.store.store().begin_transaction();
            detach_block_cell(&write, &block)?;
            write.commit()?;
            self.store.remove_block(&block);
            self.proposals.remove(block.number());
            detached.push_front(block);
        }
        let header = self
            .blocks
            .last()
            .map_or_else(|| self.base.tip_header().clone(), BlockView::header);
        self.snapshot(header);
        Ok(detached)
    }
}

#[derive(Default)]
struct Events {
    records: Mutex<HashMap<Byte32, CallbackRecord>>,
    changed: tokio::sync::Notify,
}

struct CallbackRecord {
    acceptances: usize,
    refusals: usize,
    outcome: Outcome,
}

enum Outcome {
    Accepted { proposed: bool },
    Rejected(Reject),
}

impl Events {
    fn record(&self, hash: Byte32, outcome: Outcome) {
        let mut records = lock(&self.records);
        let acceptances = records.get(&hash).map_or(0, |record| record.acceptances)
            + usize::from(matches!(outcome, Outcome::Accepted { .. }));
        let refusals = records.get(&hash).map_or(0, |record| record.refusals)
            + usize::from(matches!(outcome, Outcome::Rejected(_)));
        records.insert(
            hash,
            CallbackRecord {
                acceptances,
                refusals,
                outcome,
            },
        );
        drop(records);
        self.changed.notify_one();
    }
}

fn hashes(transactions: &[TransactionView]) -> HashSet<Byte32> {
    transactions.iter().map(TransactionView::hash).collect()
}

fn encoded_hashes(values: &HashSet<Byte32>) -> Vec<String> {
    let mut values: Vec<_> = values
        .iter()
        .map(|hash| hex_bytes(hash.as_slice()))
        .collect();
    values.sort_unstable();
    values
}

fn accepted(controller: &TxPoolController) -> BenchResult<(HashSet<Byte32>, HashSet<Byte32>)> {
    let ids = controller.get_all_ids().map_err(bench_error)?;
    Ok((
        ids.pending.into_iter().collect(),
        ids.proposed.into_iter().collect(),
    ))
}

fn expect_membership(
    controller: &TxPoolController,
    all: &HashSet<Byte32>,
    proposed: &HashSet<Byte32>,
) -> BenchResult<()> {
    let (pending, actual_proposed) = accepted(controller)?;
    require(
        &actual_proposed == proposed && pending == all.difference(proposed).cloned().collect(),
        "chain transition changed exact accepted/proposed membership",
    )
}

fn reconcile(
    controller: &TxPoolController,
    chain: &Chain,
    detached: VecDeque<BlockView>,
    attached: VecDeque<BlockView>,
    stage: &str,
) -> BenchResult<()> {
    let detached_blocks = detached.len();
    let attached_blocks = attached.len();
    let detached_count = detached
        .iter()
        .map(|block| block.transactions().len() - 1)
        .sum::<usize>();
    let attached_count = attached
        .iter()
        .map(|block| block.transactions().len() - 1)
        .sum::<usize>();
    let (user_before, system_before) = process_cpu_nanos()?;
    let start = Instant::now();
    controller
        .update_tx_pool_for_reorg(detached, attached, Arc::clone(&chain.tip))
        .map_err(bench_error)?;
    let elapsed = start.elapsed().as_nanos();
    let (user_after, system_after) = process_cpu_nanos()?;
    println!(
        "CHAIN_SAMPLE {}",
        serde_json::json!({"schema":1,"stage":stage,"elapsed_ns":elapsed,
        "cpu_ns":user_after + system_after - user_before - system_before,"tip_hash":hex_bytes(chain.tip.tip_hash().as_slice()),
        "tip_number":chain.tip.tip_number(),"detached_blocks":detached_blocks,"attached_blocks":attached_blocks,
        "detached_transactions":detached_count,"attached_transactions":attached_count,
        "scope":"controller_reconciliation_and_required_publication"})
    );
    require(
        controller.get_tx_pool_info().map_err(bench_error)?.tip_hash == chain.tip.tip_hash(),
        "acknowledged chain tip was not published",
    )
}

fn wait_recovery(
    runtime: &tokio::runtime::Runtime,
    controller: &TxPoolController,
    events: &Events,
    complete: impl Fn() -> BenchResult<bool>,
) -> BenchResult<()> {
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                // Register before reading durable state, so publication between
                // the check and await cannot be missed.
                let changed = events.changed.notified();
                let rejected = rejection_diagnostics::notified();
                tokio::pin!(changed, rejected);
                changed.as_mut().enable();
                rejected.as_mut().enable();
                let info = controller.get_tx_pool_info().map_err(bench_error)?;
                if info.verify_queue_size == 0 && info.orphan_size == 0 && complete()? {
                    return Ok(());
                }
                tokio::select! {
                    _ = changed => {},
                    _ = rejected => {},
                }
            }
        })
        .await?
    })
}

fn roundtrip(
    runtime: &tokio::runtime::Runtime,
    controller: &TxPoolController,
    events: &Events,
    chain: &mut Chain,
    transactions: &[TransactionView],
    roots: usize,
) -> BenchResult<()> {
    for transaction in transactions {
        controller
            .submit_local_tx(transaction.clone())
            .map_err(bench_error)?
            .map_err(bench_error)?;
    }
    let all = hashes(transactions);
    let committed: Vec<_> = transactions
        .chunks_exact(DEPTH)
        .take(roots)
        .map(|chain| chain[0].clone())
        .collect();
    let root_hashes = hashes(&committed);
    let recovering = hashes(&transactions[..roots * DEPTH]);
    expect_membership(controller, &all, &HashSet::new())?;

    let block = chain.attach(
        &[],
        committed
            .iter()
            .map(TransactionView::proposal_short_id)
            .collect(),
    )?;
    reconcile(
        controller,
        chain,
        VecDeque::new(),
        [block].into(),
        "pending_to_gap",
    )?;
    expect_membership(controller, &all, &HashSet::new())?;
    require(
        committed
            .iter()
            .all(|tx| chain.tip.proposals().contains_gap(&tx.proposal_short_id())),
        "fixture proposals did not enter Gap",
    )?;
    let block = chain.attach(&[], Vec::new())?;
    reconcile(
        controller,
        chain,
        VecDeque::new(),
        [block].into(),
        "gap_to_proposed",
    )?;
    expect_membership(controller, &all, &root_hashes)?;
    require(
        root_hashes.iter().all(|hash| {
            matches!(
                lock(&events.records).get(hash),
                Some(CallbackRecord {
                    outcome: Outcome::Accepted { proposed: true },
                    ..
                })
            )
        }),
        "proposed callbacks were not published before chain acknowledgment",
    )?;

    let block = chain.attach(&committed, Vec::new())?;
    reconcile(
        controller,
        chain,
        VecDeque::new(),
        [block].into(),
        "attach_committed_roots",
    )?;
    expect_membership(
        controller,
        &all.difference(&root_hashes).cloned().collect(),
        &HashSet::new(),
    )?;
    controller.suspend_chunk_process().map_err(bench_error)?;
    let before: HashMap<_, _> = {
        let records = lock(&events.records);
        recovering
            .iter()
            .map(|hash| {
                (
                    hash.clone(),
                    records.get(hash).map_or(0, |record| record.acceptances),
                )
            })
            .collect()
    };
    let detached = chain.detach(1)?;
    let result = (|| {
        reconcile(
            controller,
            chain,
            detached,
            VecDeque::new(),
            "detach_roots_with_descendants_paused",
        )?;
        expect_membership(
            controller,
            &all.difference(&recovering).cloned().collect(),
            &HashSet::new(),
        )?;
        let queued = controller
            .get_tx_pool_info()
            .map_err(bench_error)?
            .verify_queue_size;
        require(
            queued == recovering.len(),
            "detachment did not queue the exact affected dependency closure",
        )
    })();
    let recovery_start = Instant::now();
    controller.continue_chunk_process().map_err(bench_error)?;
    result?;
    wait_recovery(runtime, controller, events, || {
        let records = lock(&events.records);
        Ok(before.iter().all(|(hash, count)| {
            records
                .get(hash)
                .is_some_and(|record| record.acceptances > *count)
        }))
    })?;
    let elapsed = recovery_start.elapsed().as_nanos();
    expect_membership(controller, &all, &root_hashes)?;
    println!(
        "CHAIN_RECOVERY {}",
        serde_json::json!({"schema":1,"stage":"recover_detached_roots_and_descendants",
        "elapsed_ns":elapsed,"offered":encoded_hashes(&recovering),"retained":encoded_hashes(&recovering),
        "accepted":encoded_hashes(&all),"refused":[],"scope":"resume_to_observed_empty_recovery_queue"})
    );

    let detached = chain.detach(2)?;
    reconcile(
        controller,
        chain,
        detached,
        VecDeque::new(),
        "proposed_to_pending",
    )?;
    expect_membership(controller, &all, &HashSet::new())?;
    require(
        lock(&events.records)
            .values()
            .all(|record| record.refusals == 0),
        "roundtrip unexpectedly rejected a valid transaction",
    )?;
    Ok(())
}

fn pressure(
    runtime: &tokio::runtime::Runtime,
    controller: &TxPoolController,
    events: &Events,
    chain: &mut Chain,
    transactions: &[TransactionView],
) -> BenchResult<()> {
    let offered = hashes(transactions);
    controller.suspend_chunk_process().map_err(bench_error)?;
    let detached = chain.detach(chain.blocks.len())?;
    let captured = (|| {
        reconcile(
            controller,
            chain,
            detached,
            VecDeque::new(),
            "detach_recovery_pressure_paused",
        )?;
        let mut retained = HashSet::new();
        for batch in transactions.chunks(512) {
            let fetched = runtime
                .block_on(
                    controller.fetch_txs(
                        batch
                            .iter()
                            .map(TransactionView::proposal_short_id)
                            .collect(),
                    ),
                )
                .map_err(bench_error)?;
            for transaction in fetched.into_values() {
                require(
                    offered.contains(&transaction.hash()),
                    "recovery returned an unrelated transaction",
                )?;
                retained.insert(transaction.hash());
            }
        }
        require(
            !retained.is_empty() && retained.len() < offered.len(),
            "pressure did not exercise bounded recovery retention",
        )?;
        let info = controller.get_tx_pool_info().map_err(bench_error)?;
        require(
            info.verify_queue_size == retained.len()
                && info.pending_size == 0
                && info.proposed_size == 0
                && rejection_diagnostics::count() == 0,
            "paused recovery counts differ from retained bodies",
        )?;
        Ok::<_, Box<dyn Error>>(retained)
    })();
    let start = Instant::now();
    controller.continue_chunk_process().map_err(bench_error)?;
    let retained = captured?;
    wait_recovery(runtime, controller, events, || {
        let published = {
            let records = lock(&events.records);
            records
                .iter()
                .filter_map(|(hash, record)| {
                    matches!(record.outcome, Outcome::Accepted { .. }).then_some(hash.clone())
                })
                .collect::<HashSet<_>>()
        };
        // Admission may publish a candidate before its capacity victims. Do not
        // finish until those rejection callbacks catch up with public membership.
        // Unaccepted candidates have no reject callback; the committed diagnostic
        // counts them too. Replay checks their exact hashes against the final set.
        let (pending, proposed) = accepted(controller)?;
        Ok(proposed.is_empty()
            && published == pending
            && pending.len() + rejection_diagnostics::count() == retained.len())
    })?;
    let elapsed = start.elapsed().as_nanos();
    let (pending, proposed) = accepted(controller)?;
    require(
        proposed.is_empty() && !pending.is_empty() && pending.is_subset(&retained),
        "recovered accepted population differs",
    )?;
    let refused = retained
        .difference(&pending)
        .cloned()
        .collect::<HashSet<_>>();
    {
        let records = lock(&events.records);
        for (hash, record) in records.iter() {
            if let Outcome::Rejected(reason) = &record.outcome {
                require(
                    refused.contains(hash) && matches!(reason, Reject::Full(_)),
                    "recovery pressure emitted a non-capacity refusal",
                )?;
            }
        }
    }
    let info = controller.get_tx_pool_info().map_err(bench_error)?;
    require(
        info.total_tx_size as u64 <= info.max_tx_pool_size,
        "recovery exceeded the accepted serialized bound",
    )?;
    println!(
        "CHAIN_RECOVERY {}",
        serde_json::json!({"schema":1,"stage":"bounded_recovery_pressure",
        "elapsed_ns":elapsed,"offered":encoded_hashes(&offered),"retained":encoded_hashes(&retained),
        "accepted":encoded_hashes(&pending),"refused":encoded_hashes(&refused),
        "serialized_bytes":info.total_tx_size,"serialized_limit":info.max_tx_pool_size,
        "scope":"resume_to_observed_empty_recovery_queue"})
    );
    Ok(())
}

pub(super) fn run(args: Vec<String>) -> BenchResult<()> {
    let (work, workers) = options(&args)?;
    let (consensus, transactions) = match work {
        Work::Roundtrip { count, .. } => build_workload(
            Workload::Forest {
                depth: DEPTH,
                reverse: false,
            },
            count,
        )?,
        Work::RecoveryPressure { count } => {
            let (consensus, funding) = test_consensus(count)?;
            let transactions = (0..count)
                .map(|index| {
                    build_tx(OutPoint::new(funding.hash(), index as u32))
                        .as_advanced_builder()
                        .witness(Bytes::from(vec![0; PRESSURE_WITNESS_BYTES]).pack())
                        .build()
                })
                .collect();
            (consensus, transactions)
        }
    };
    let consensus = Arc::new(consensus);
    let mut digest = ckb_hash::new_blake2b();
    digest.update(consensus.genesis_block().data().as_slice());
    for transaction in &transactions {
        digest.update(transaction.data().as_slice());
    }
    let runtime_threads = std::thread::available_parallelism().map_or(8, |count| count.get());
    println!(
        "CHAIN_CORPUS {}",
        serde_json::json!({"schema":1,"options":args,"workers":workers,
        "runtime_threads":runtime_threads,"transactions":transactions.len(),"fixture_blake2b":finish_blake2b(digest),
        "transaction_hashes":transactions.iter().map(|tx|hex_bytes(tx.hash().as_slice())).collect::<Vec<_>>(),
        "profiling":cfg!(feature="profiling"),"allocation_observation":cfg!(feature="allocation-observation"),
        "debug_assertions":cfg!(debug_assertions),"trusted_synthetic_blocks":true})
    );
    let mut chain = Chain::new(Arc::clone(&consensus))?;
    if matches!(work, Work::RecoveryPressure { .. }) {
        for batch in transactions.chunks(2) {
            chain.attach(batch, Vec::new())?;
        }
    }
    // Store and backing directories outlive every runtime task on all paths.
    let directory = TempDir::new()?;
    let (handle, handle_stop, runtime) = new_global_runtime(Some(runtime_threads));
    let network = start_network(&consensus, &handle, directory.path())?;
    let config = TxPoolConfig {
        persisted_data: directory.path().join("tx-pool.data"),
        ..tx_pool_config(workers, false)
    };
    let (mut builder, controller, receiver) = TxPoolServiceBuilder::new(
        config,
        Arc::clone(&chain.tip),
        None,
        Arc::new(RwLock::new(init_cache())),
        &handle,
        FeeEstimator::new_dummy(),
    )
    .map_err(bench_error)?;
    let relay = RelayDrainGuard::start(receiver)?;
    let events = Arc::new(Events::default());
    let pending = Arc::clone(&events);
    builder.register_pending(Box::new(move |entry| {
        pending.record(
            entry.transaction().hash(),
            Outcome::Accepted { proposed: false },
        );
    }));
    let proposed = Arc::clone(&events);
    builder.register_proposed(Box::new(move |entry| {
        proposed.record(
            entry.transaction().hash(),
            Outcome::Accepted { proposed: true },
        );
    }));
    let rejected = Arc::clone(&events);
    builder.register_reject(Box::new(move |entry, reason| {
        rejected.record(entry.transaction().hash(), Outcome::Rejected(reason));
    }));
    builder.start(network);
    let result = (|| {
        controller.get_tx_pool_info().map_err(bench_error)?;
        match work {
            Work::Roundtrip { roots, .. } => roundtrip(
                &runtime,
                &controller,
                &events,
                &mut chain,
                &transactions,
                roots,
            ),
            Work::RecoveryPressure { .. } => {
                pressure(&runtime, &controller, &events, &mut chain, &transactions)
            }
        }
    })();
    let shutdown_result = shutdown(&runtime, controller, relay, handle, handle_stop);
    result?;
    shutdown_result?;
    println!(
        "CHAIN_COMPLETE {}",
        serde_json::json!({"schema":1,"validated":true})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn transition_shapes_reject_incomplete_or_unbounded_populations() {
        use super::*;
        for arguments in [
            "roundtrip 3 1 8",
            "roundtrip 4 2 8",
            "roundtrip 8 0 8",
            "roundtrip 8 1 0",
            "roundtrip 65540 1 8",
            "recovery-pressure 1099 8",
            "recovery-pressure 2049 8",
            "recovery-pressure 1200 65",
        ] {
            assert!(
                options(
                    &arguments
                        .split_whitespace()
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                )
                .is_err(),
                "{arguments}"
            );
        }
        assert!(options(&["roundtrip", "8", "1", "8"].map(str::to_owned)).is_ok());
    }
}
