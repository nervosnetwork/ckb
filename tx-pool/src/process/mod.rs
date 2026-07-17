use crate::component::entry::TxEntry;
use crate::component::pipeline_queue::PipelineQueue;
use crate::component::pool_map::Status;
use crate::component::pre_check_queue::PreCheckJob;
use crate::constants::{GAP_PROPOSAL_INDEX, PROPOSED_PROPOSAL_INDEX};
use crate::error::Reject;
use crate::pool::TxPool;
use crate::service::{BlockAssemblerMessage, TxPoolService, TxVerificationResult};
use crate::tx_source::TxSource;
use crate::util::{
    check_tx_fee, check_tx_fee_with_min_fee_rate, check_txid_collision, non_contextual_verify,
};
use ckb_error::{AnyError, InternalErrorKind};
use ckb_fee_estimator::FeeEstimator;
use ckb_jsonrpc_types::BlockTemplate;
use ckb_logger::{debug, error, info};
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::core::error::OutPointError;
use ckb_types::packed::OutPoint;
use ckb_types::{
    core::{
        BlockView, Capacity, EstimateMode, FeeRate, HeaderView, TransactionView,
        cell::{ResolvedTransaction, resolve_transaction},
    },
    packed::{Byte32, ProposalShortId},
};
use ckb_util::LinkedHashSet;
use ckb_verification::{TxVerifyEnv, cache::Completed};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

mod orphan;
mod post_process;
mod rbf;
mod reorg;
mod submit;

/// A list for plug target for `plug_entry` method
pub enum PlugTarget {
    /// Pending pool
    Pending,
    /// Proposed pool
    Proposed,
}

/// Routing decision from [`TxPoolService::check_and_route_dependent`].
#[derive(Debug)]
enum RouteDecision {
    /// The tx does not depend on any in-flight pipeline tx.
    Independent,
    /// The tx was enqueued in the ordered resolve queue.
    Enqueued,
    /// The tx is a duplicate of an already-queued tx.
    Duplicate,
}

/// Map a pool status to the verification environment used for contextual checks.
fn status_to_verify_env(status: Status, header: &HeaderView) -> TxVerifyEnv {
    match status {
        Status::Pending => TxVerifyEnv::new_submit(header),
        Status::Gap => TxVerifyEnv::new_proposed(header, GAP_PROPOSAL_INDEX),
        Status::Proposed => TxVerifyEnv::new_proposed(header, PROPOSED_PROPOSAL_INDEX),
    }
}

/// Map a pool status to the block-assembler notification that should be sent
/// when a transaction reaches this status.
fn status_to_block_assembler_message(status: Status) -> Option<BlockAssemblerMessage> {
    match status {
        Status::Pending => Some(BlockAssemblerMessage::Pending),
        Status::Proposed => Some(BlockAssemblerMessage::Proposed),
        Status::Gap => None,
    }
}

impl TxPoolService {
    pub(crate) async fn get_block_template(&self) -> Result<BlockTemplate, AnyError> {
        if let Some(ref block_assembler) = self.block_assembler {
            Ok(block_assembler.get_current().await)
        } else {
            Err(InternalErrorKind::Config
                .other("BlockAssembler disabled")
                .into())
        }
    }

    pub(crate) async fn notify_block_assembler(&self, status: Status) {
        if !self.should_notify_block_assembler() {
            return;
        }
        if let Some(message) = status_to_block_assembler_message(status) {
            // try_send on purpose: the consumer deduplicates messages into
            // Pending/Proposed/Uncle variants on each interval, so dropping a
            // duplicate notification when the channel is full is harmless.
            // Using send().await here would backpressure verify workers
            // whenever the block-assembler loop is slow.
            if let Err(err) = self.block_assembler_sender.try_send(message) {
                match err {
                    mpsc::error::TrySendError::Full(_) => {
                        debug!("block_assembler channel full, skip duplicate notification")
                    }
                    mpsc::error::TrySendError::Closed(_) => {
                        error!("block_assembler receiver dropped")
                    }
                }
            }
        }
    }

    pub(crate) async fn verify_queue_contains(&self, tx: &TransactionView) -> bool {
        let queue = self.queues.verify_queue.read().await;
        queue.contains_key(&tx.proposal_short_id())
    }

    /// Read-lock the pool and run `f` without cloning the snapshot.
    pub(crate) async fn read_tx_pool<U, F: FnOnce(&TxPool) -> U>(&self, f: F) -> U {
        let tx_pool = self.tx_pool.read().await;
        f(&tx_pool)
    }

    /// Read-lock the pool and return the result along with a cloned snapshot.
    pub(crate) async fn read_tx_pool_with_snapshot<U, F: FnMut(&TxPool, Arc<Snapshot>) -> U>(
        &self,
        mut f: F,
    ) -> (U, Arc<Snapshot>) {
        let tx_pool = self.tx_pool.read().await;
        let snapshot = tx_pool.cloned_snapshot();

        let ret = f(&tx_pool, Arc::clone(&snapshot));
        (ret, snapshot)
    }

    pub(crate) async fn pre_check(
        &self,
        tx: &TransactionView,
        tx_size: usize,
    ) -> (Result<PreCheckedTx, Reject>, Arc<Snapshot>) {
        // Fast path: for transactions whose inputs and cell deps all come from the
        // chain (not from any tx currently in the pool), we can resolve and compute
        // the fee without holding the tx_pool read lock.  We only take the lock
        // briefly to check for txid collisions.
        let (collision, snapshot) = self
            .read_tx_pool_with_snapshot(|tx_pool, _snapshot| {
                check_txid_collision(tx_pool, tx).err()
            })
            .await;
        if let Some(reject) = collision {
            return (Err(reject), snapshot);
        }

        let short_id = tx.proposal_short_id();
        let mut seen_inputs =
            HashSet::with_capacity(tx.inputs().len().saturating_add(tx.cell_deps().len()));
        match resolve_transaction(
            tx.clone(),
            &mut seen_inputs,
            snapshot.as_ref(),
            snapshot.as_ref(),
        ) {
            Ok(rtx) => {
                let rtx = Arc::new(rtx);
                let fee = match check_tx_fee_with_min_fee_rate(
                    &snapshot,
                    &rtx,
                    tx_size,
                    self.tx_pool_config.min_fee_rate,
                ) {
                    Ok(fee) => fee,
                    Err(reject) => return (Err(reject), snapshot),
                };
                let status = get_tx_status(&snapshot, &short_id);
                (
                    Ok(PreCheckedTx {
                        pre_resolve_tip: snapshot.tip_hash(),
                        rtx,
                        status,
                        fee,
                        tx_size,
                    }),
                    snapshot,
                )
            }
            Err(OutPointError::Unknown(_)) => {
                // At least one input/cell dep is not in the chain snapshot.  It may
                // be an output of a tx currently in the pool, so fall back to the
                // locked path which can resolve through the pool.
                self.pre_check_with_pool_lock(tx, tx_size).await
            }
            Err(err) => (Err(Reject::Resolve(err)), snapshot),
        }
    }

    async fn pre_check_with_pool_lock(
        &self,
        tx: &TransactionView,
        tx_size: usize,
    ) -> (Result<PreCheckedTx, Reject>, Arc<Snapshot>) {
        let (ret, snapshot) = self
            .read_tx_pool_with_snapshot(|tx_pool, snapshot| {
                let tip_hash = snapshot.tip_hash();

                // Same txid means exactly the same transaction, including inputs, outputs, witnesses, etc.
                // It's also not possible for RBF, reject it directly
                check_txid_collision(tx_pool, tx)?;

                // Try normal path first, if double-spending check success we don't need RBF check
                // this make sure RBF won't introduce extra performance cost for hot path
                let res = resolve_tx(tx_pool, &snapshot, tx.clone(), false);
                match res {
                    Ok((rtx, status)) => {
                        let fee = check_tx_fee(tx_pool, &snapshot, &rtx, tx_size)?;
                        Ok(PreCheckedTx {
                            pre_resolve_tip: tip_hash,
                            rtx,
                            status,
                            fee,
                            tx_size,
                        })
                    }
                    Err(Reject::Resolve(OutPointError::Dead(out))) => {
                        let (rtx, status) = resolve_tx(tx_pool, &snapshot, tx.clone(), true)?;
                        let fee = check_tx_fee(tx_pool, &snapshot, &rtx, tx_size)?;
                        let conflicts = tx_pool.pool_map.find_conflict_outpoint(tx);
                        if conflicts.is_none() {
                            // this mean one input's outpoint is dead, but there is no direct conflicted tx in tx_pool
                            // we should reject it directly and don't need to put it into conflicts pool
                            error!(
                                "{} is resolved as Dead, but there is no direct conflicted tx",
                                rtx.transaction.proposal_short_id()
                            );
                            return Err(Reject::Resolve(OutPointError::Dead(out)));
                        }
                        // we also return Ok here, so that the entry will be continue to be verified before submit
                        // we only want to put it into conflicts pool after the verification stage passed
                        // then we will double-check conflicts txs in `submit_entry`

                        Ok(PreCheckedTx {
                            pre_resolve_tip: tip_hash,
                            rtx,
                            status,
                            fee,
                            tx_size,
                        })
                    }
                    Err(err) => Err(err),
                }
            })
            .await;
        (ret, snapshot)
    }

    pub(crate) async fn non_contextual_verify(
        &self,
        tx: &TransactionView,
        source: TxSource,
    ) -> Result<(), Reject> {
        if let Err(reject) = non_contextual_verify(&self.consensus, tx) {
            if reject.is_malformed_tx()
                && let Some(peer) = source.peer()
            {
                self.ban_malformed(peer, format!("reject {reject}")).await;
            }
            return Err(reject);
        }
        Ok(())
    }

    /// Common pre-flight checks shared by all transaction submission paths.
    ///
    /// Runs non-contextual verification and rejects duplicates that are
    /// already in any pipeline queue (ordered resolve, pre-check, verify)
    /// or the orphan pool.
    pub(crate) async fn check_tx_basic_validity(
        &self,
        tx: &TransactionView,
        source: TxSource,
    ) -> Result<(), Reject> {
        self.non_contextual_verify(tx, source).await?;

        let dup = || Reject::Duplicated(tx.hash());
        let id = tx.proposal_short_id();

        {
            let ordered = self.queues.ordered_resolve_queue.read().await;
            if ordered.contains_key(&id) {
                return Err(dup());
            }
        }

        if self.queues.pre_check_queue.contains_key(&id) {
            return Err(dup());
        }

        if self.verify_queue_contains(tx).await {
            return Err(dup());
        }

        if self.orphan_contains(tx).await {
            return Err(dup());
        }

        Ok(())
    }

    async fn classify_and_enqueue_with_full_reject_notification(
        &self,
        tx: TransactionView,
        source: TxSource,
    ) -> Result<bool, Reject> {
        let tx_hash = tx.hash();
        let ret = self.classify_and_enqueue_tx_spawn(tx, source).await;

        if matches!(ret, Err(Reject::Full(_))) {
            self.send_result_to_relayer(TxVerificationResult::Reject { tx_hash });
        }

        ret
    }

    pub(crate) async fn submit_remote_tx(
        &self,
        tx: TransactionView,
        source: TxSource,
    ) -> Result<bool, Reject> {
        self.check_tx_basic_validity(&tx, source).await?;
        self.classify_and_enqueue_with_full_reject_notification(tx, source)
            .await
    }

    pub(crate) async fn notify_tx(&self, tx: TransactionView) -> Result<bool, Reject> {
        self.check_tx_basic_validity(&tx, TxSource::Proposal)
            .await?;
        self.classify_and_enqueue_with_full_reject_notification(tx, TxSource::Proposal)
            .await
    }

    pub(crate) async fn process_tx(
        &self,
        tx: TransactionView,
        source: TxSource,
    ) -> Result<Completed, Reject> {
        self.check_tx_basic_validity(&tx, source).await?;

        let ret = self.process_tx_direct(tx.clone(), source, None).await;
        self.after_process(tx, source, &ret).await;
        ret
    }

    pub(crate) fn put_recent_reject(&self, tx_hash: &Byte32, reject: &Reject) {
        if let Some(ref recent_reject) = self.aux.recent_reject
            && let Err(e) = recent_reject.put(tx_hash, reject.clone())
        {
            error!(
                "Failed to record recent_reject {} {} {}",
                tx_hash, reject, e
            );
        }
    }

    /// Notify the ordered resolver if there are jobs waiting.
    ///
    /// Must be called after a transaction is removed from the verify queue or
    /// the in-pool set: the removed tx may have had descendants waiting in the
    /// ordered resolve queue, and waking the resolver lets them be retried
    /// (and rejected if the parent is gone) promptly.
    async fn wake_ordered_resolver_if_needed(&self) {
        let ordered = self.queues.ordered_resolve_queue.read().await;
        if !ordered.is_empty() {
            ordered.subscribe().notify_one();
        }
    }

    pub(crate) async fn remove_tx(&self, tx_hash: Byte32) -> bool {
        let id = ProposalShortId::from_tx_hash(&tx_hash);
        if self.queues.pre_check_queue.remove_by_id(&id).is_some() {
            return true;
        }
        {
            let mut queue = self.queues.ordered_resolve_queue.write().await;
            if queue.remove_tx(&id).is_some() {
                return true;
            }
        }
        {
            // Lock hierarchy: rbf_candidates must be acquired before
            // verify_queue. Holding rbf_candidates while checking verify_queue
            // prevents a deadlock with register_rbf_candidate / update_reorg,
            // which take rbf_candidates.write() before verify_queue.write().
            // Orphan and tx_pool are checked after verify_queue so that the
            // global order remains consistent across remove_tx and
            // ban_malformed: ordered -> rbf -> verify -> orphan -> tx_pool.
            let mut rbf = self.queues.rbf_candidates.write().await;
            let mut queue = self.queues.verify_queue.write().await;
            if queue.remove_tx(&id).is_some() {
                drop(queue);
                rbf.remove(&id);
                // The removed tx may have had descendants waiting in the
                // ordered resolve queue. Wake the resolver so they can be
                // retried (and rejected if the parent is gone) promptly.
                self.wake_ordered_resolver_if_needed().await;
                return true;
            }
        }
        {
            let mut orphan = self.orphan.write().await;
            if orphan.remove_orphan_tx(&id).is_some() {
                return true;
            }
        }
        let removed_entries = {
            let mut tx_pool = self.tx_pool.write().await;
            tx_pool.remove_tx(&id)
        };
        if !removed_entries.is_empty() {
            // The removed pool entries have released their inputs. Clean up
            // any in-flight RBF candidates targeting those inputs so they do
            // not block future replacements.
            self.cleanup_rbf_for_removed_entries(removed_entries.iter())
                .await;
            self.wake_ordered_resolver_if_needed().await;
        }
        !removed_entries.is_empty()
    }

    /// Returns true if every parent of `tx` that is not already on-chain is
    /// currently in the tx-pool or one of the pipeline queues.
    ///
    /// This is used by the ordered resolver to decide whether a local orphan
    /// with missing inputs should retry without burning an attempt. We only
    /// skip the attempt counter when all missing parents are actually in flight;
    /// if any parent is permanently missing, the attempt counter must advance so
    /// that the orphan is eventually rejected.
    pub(crate) async fn all_missing_parents_in_flight(&self, parents: &HashSet<Byte32>) -> bool {
        // Collect parents that are neither on-chain nor already in the pool, while
        // holding a single read guard. This avoids re-acquiring the tx_pool lock
        // for every missing parent.
        let missing_ids: Vec<ProposalShortId> = {
            let pool = self.tx_pool.read().await;
            let snapshot = pool.cloned_snapshot();
            parents
                .iter()
                .filter(|h| !snapshot.transaction_exists(h))
                .map(ProposalShortId::from_tx_hash)
                .filter(|id| !pool.contains_proposal_id(id))
                .collect()
        };
        if missing_ids.is_empty() {
            return true;
        }

        for parent_id in missing_ids {
            if self
                .queues
                .ordered_resolve_queue
                .read()
                .await
                .contains_key(&parent_id)
            {
                continue;
            }
            if self
                .queues
                .verify_queue
                .read()
                .await
                .contains_key(&parent_id)
            {
                continue;
            }
            if self.queues.pre_check_queue.contains_key(&parent_id) {
                continue;
            }
            return false;
        }
        true
    }

    pub(crate) fn send_result_to_relayer(&self, result: TxVerificationResult) {
        if let Err(e) = self.tx_relay_sender.send(result) {
            error!("tx-pool tx_relay_sender internal error {}", e);
        }
    }

    pub(crate) async fn process_tx_direct(
        &self,
        tx: TransactionView,
        source: TxSource,
        command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    ) -> Result<Completed, Reject> {
        let tx_size = tx.data().serialized_size_in_block();
        let (ret, snapshot) = self.pre_check(&tx, tx_size).await;

        let PreCheckedTx {
            pre_resolve_tip,
            rtx,
            status,
            fee,
            tx_size,
        } = ret?;

        self.verify_and_submit_core(
            crate::resolved_tx::ResolvedTx {
                tx,
                rtx,
                status,
                fee,
                tx_size,
                pre_resolve_tip,
                snapshot,
                source,
            },
            command_rx,
        )
        .await
    }

    /// Topologically sort transactions so that parents are placed before their
    /// children. This is required when re-adding detached transactions into the
    /// pipeline: a child must not be classified before its parent has had a
    /// chance to enter the in-flight pipeline, otherwise it will be treated as a
    /// local orphan and have to wait for a retry.
    pub(crate) fn sort_txs_by_dependencies(txs: &mut Vec<TransactionView>) {
        Self::sort_by_dependencies(txs, |tx| tx);
    }

    /// Topologically sort a list of items that wrap transactions so that
    /// parents are placed before their children. `tx_of` extracts the
    /// transaction reference from each item.
    pub(crate) fn sort_by_dependencies<T>(
        items: &mut Vec<T>,
        tx_of: impl Fn(&T) -> &TransactionView,
    ) {
        if items.len() <= 1 {
            return;
        }

        let mut output_to_index: HashMap<OutPoint, usize> =
            HashMap::with_capacity(items.len().saturating_mul(2));
        for (i, item) in items.iter().enumerate() {
            let tx_hash = tx_of(item).hash();
            for idx in 0..tx_of(item).outputs().len() {
                output_to_index.insert(OutPoint::new(tx_hash.clone(), idx as u32), i);
            }
        }

        let mut in_degree = vec![0usize; items.len()];
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); items.len()];
        for (i, item) in items.iter().enumerate() {
            let tx = tx_of(item);
            for input in tx.input_pts_iter() {
                if let Some(&parent) = output_to_index.get(&input)
                    && parent != i
                {
                    in_degree[i] += 1;
                    children[parent].push(i);
                }
            }
            for dep in tx.cell_deps_iter() {
                let out_point = dep.out_point();
                if let Some(&parent) = output_to_index.get(&out_point)
                    && parent != i
                {
                    in_degree[i] += 1;
                    children[parent].push(i);
                }
            }
        }

        let mut ready: VecDeque<usize> = (0..items.len()).filter(|&i| in_degree[i] == 0).collect();
        let mut sorted = Vec::with_capacity(items.len());
        while let Some(i) = ready.pop_front() {
            sorted.push(i);
            for &child in &children[i] {
                in_degree[child] -= 1;
                if in_degree[child] == 0 {
                    ready.push_back(child);
                }
            }
        }

        if sorted.len() != items.len() {
            // A cycle should never happen in valid detached blocks, but if it
            // does we keep the original order rather than losing transactions.
            return;
        }

        let mut remaining: Vec<Option<T>> = items.drain(..).map(Some).collect();
        items.extend(
            sorted
                .into_iter()
                .map(|i| remaining[i].take().expect("index valid")),
        );
    }

    pub(crate) async fn update_tx_pool_for_reorg(
        &self,
        detached_blocks: VecDeque<BlockView>,
        attached_blocks: VecDeque<BlockView>,
        detached_proposal_id: HashSet<ProposalShortId>,
        snapshot: Arc<Snapshot>,
    ) {
        let mine_mode = self.block_assembler.is_some();
        let mut detached = LinkedHashSet::default();
        let mut attached = LinkedHashSet::default();

        let detached_headers: HashSet<Byte32> = detached_blocks
            .iter()
            .map(|blk| blk.header().hash())
            .collect();

        for blk in detached_blocks {
            detached.extend(blk.transactions().into_iter().skip(1))
        }

        for blk in attached_blocks {
            self.aux.fee_estimator.commit_block(&blk);
            attached.extend(blk.transactions().into_iter().skip(1));
        }
        let mut retain: Vec<TransactionView> = detached.difference(&attached).cloned().collect();

        let reject_events;
        {
            // This closure is used to limit the lifetime of mutable tx_pool.
            let mut tx_pool = self.tx_pool.write().await;

            reject_events = reorg::update_tx_pool_for_reorg(
                &mut tx_pool,
                &attached,
                &detached_headers,
                detached_proposal_id,
                snapshot,
                &self.callbacks,
                mine_mode,
            );
        }

        // Dispatch reject callbacks outside the write lock, then clean up
        // in-flight RBF candidates targeting inputs that have just been freed
        // by the reorg (committed, expired, detached, or evicted transactions).
        for (entry, reject) in &reject_events {
            self.callbacks.call_reject(entry, reject.clone());
        }
        self.cleanup_rbf_for_removed_entries(reject_events.iter().map(|(entry, _)| entry))
            .await;

        self.remove_orphan_txs_by_attach(&attached).await;
        {
            let mut queue = self.queues.verify_queue.write().await;
            queue.remove_txs(attached.iter().map(|tx| tx.proposal_short_id()));
        }

        // Recover detached transactions through the direct per-tx entry point.
        // Dependent transactions must be processed after their parents have
        // already been submitted to the pool; a topological sort guarantees this
        // ordering. Using `process_tx_direct` here keeps the recovery logic simple
        // and correct while still releasing the tx-pool write lock between
        // transactions.
        {
            Self::sort_txs_by_dependencies(&mut retain);
            let mut chunk_rx = self.chunk_rx.clone();
            for tx in retain {
                let ret = self
                    .process_tx_direct(tx.clone(), TxSource::Local, Some(&mut chunk_rx))
                    .await;
                if let Err(ref reject) = ret {
                    debug!("reorg re-add failed: {}", reject);
                    self.after_process(tx, TxSource::Local, &ret).await;
                } else {
                    // The detached tx is now back in the pool. Wake up any
                    // orphans that depend on it (including via cell dep).
                    self.process_orphan_tx(&tx).await;
                }
            }
        }
    }

    /// Check if a transaction depends on any in-flight pipeline transaction
    /// (ordered resolve queue, verify queue, or pre-check queue).
    pub(crate) async fn depends_on_pipeline(&self, tx: &TransactionView) -> bool {
        let ordered = self.queues.ordered_resolve_queue.read().await;
        if ordered.depends_on(tx) {
            return true;
        }
        drop(ordered);
        let verify_queue = self.queues.verify_queue.read().await;
        if verify_queue.depends_on(tx) {
            return true;
        }
        drop(verify_queue);
        self.queues.pre_check_queue.depends_on(tx)
    }

    /// Check if a transaction depends on any in-flight pipeline transaction.
    /// If so, route it to the ordered resolve queue.
    async fn check_and_route_dependent(
        &self,
        tx: &TransactionView,
        source: TxSource,
    ) -> Result<RouteDecision, Reject> {
        let id = tx.proposal_short_id();

        if self.depends_on_pipeline(tx).await {
            let mut ordered = self.queues.ordered_resolve_queue.write().await;
            if ordered.contains_key(&id) {
                return Ok(RouteDecision::Duplicate);
            }
            return ordered
                .add_tx(crate::resolved_tx::ResolveJob::new(tx.clone(), source))
                .map(|_| RouteDecision::Enqueued);
        }

        Ok(RouteDecision::Independent)
    }

    /// Enqueue a resolved transaction into the verify queue, applying the
    /// in-flight RBF fee-ordering gate first for remote replacements.
    ///
    /// This is the single entry into the verify queue: both the entry
    /// classifier and the ordered resolver go through here, so the RBF gate
    /// cannot be bypassed.
    ///
    /// For RBF replacements, the candidate is validated and the displacement
    /// set computed while holding `rbf_candidates.write()`, then inserted into
    /// the verify queue and the registration committed atomically. This
    /// guarantees that lower-fee-rate displaced candidates are only removed
    /// from the pipeline once the higher-fee-rate candidate is successfully
    /// queued (P0-2 fix), and maintains the global lock order
    /// `rbf_candidates → verify_queue` (P0-1 fix). Only remote txs register:
    /// local and proposal txs skip the in-flight fee-rate gate.
    pub(crate) async fn enqueue_resolved_tx(
        &self,
        resolved: crate::resolved_tx::ResolvedTx,
    ) -> Result<bool, Reject> {
        let source = resolved.source;
        let tx = resolved.tx.clone();

        if matches!(source, TxSource::Remote { .. }) {
            match self
                .register_rbf_candidate(
                    tx.clone(),
                    source,
                    &resolved,
                    resolved.fee,
                    resolved.tx_size,
                )
                .await
            {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(reject) => return Err(reject),
            }
        }

        // Release the verify_queue lock before running after_process:
        // after_process may acquire tx_pool and other locks, and must never
        // run while holding a pipeline-queue write lock.
        let add_result = {
            let mut verify_queue = self.queues.verify_queue.write().await;
            verify_queue.add_tx(resolved)
        };
        match add_result {
            Ok(added) => Ok(added),
            Err(reject) => {
                self.after_process(tx, source, &Err(reject.clone())).await;
                Err(reject)
            }
        }
    }

    /// Classify a transaction and enqueue it for verification or ordered resolve.
    ///
    /// This is the core entry-point classifier.  It checks whether the tx
    /// depends on an in-flight pipeline tx, runs the shared resolve step, and
    /// routes the result to the appropriate queue.
    pub(crate) async fn classify_and_enqueue_tx(
        &self,
        tx: TransactionView,
        source: TxSource,
    ) -> Result<bool, Reject> {
        let id = tx.proposal_short_id();

        match self.check_and_route_dependent(&tx, source).await? {
            RouteDecision::Independent => {}
            RouteDecision::Enqueued => return Ok(true),
            RouteDecision::Duplicate => return Ok(false),
        }

        // The resolve step is shared with the ordered resolver so the
        // pre_check logic exists in exactly one place.
        match crate::resolve_mgr::resolve_job(
            self,
            crate::resolved_tx::ResolveJob::new(tx.clone(), source),
        )
        .await
        {
            crate::resolve_mgr::ResolveStageResult::Ready(resolved) => {
                self.enqueue_resolved_tx(resolved).await
            }
            crate::resolve_mgr::ResolveStageResult::Orphan(..) => {
                // Missing inputs: park the tx in the ordered resolve queue so
                // the ordered resolver retries it once its parents land.
                let mut ordered = self.queues.ordered_resolve_queue.write().await;
                if ordered.contains_key(&id) {
                    return Ok(false);
                }
                ordered.add_tx(crate::resolved_tx::ResolveJob::new(tx, source))
            }
            crate::resolve_mgr::ResolveStageResult::Reject(tx, reject) => {
                self.after_process(tx, source, &Err(reject.clone())).await;
                Err(reject)
            }
        }
    }

    /// Entry-point classifier used by remote/local submission.
    ///
    /// Dependent transactions (those that spend an output currently in flight)
    /// are handled synchronously so they land in the ordered resolve queue in
    /// arrival order and errors propagate to the caller.  Independent
    /// transactions are sent to a fixed-size worker pool so that the expensive
    /// `pre_check` work does not serialize inside the service actor.
    pub(crate) async fn classify_and_enqueue_tx_spawn(
        &self,
        tx: TransactionView,
        source: TxSource,
    ) -> Result<bool, Reject> {
        match self.check_and_route_dependent(&tx, source).await? {
            RouteDecision::Independent => {}
            RouteDecision::Enqueued => return Ok(true),
            RouteDecision::Duplicate => return Ok(false),
        }

        let job = PreCheckJob { tx, source };
        self.queues.pre_check_queue.push(job)?;

        // Returning Ok(true) only means the tx was accepted into the pipeline;
        // actual classification/verification happens in the worker pool.
        Ok(true)
    }

    /// Clear all pipeline queues without touching the already-accepted pool.
    ///
    /// Locks are acquired one at a time in the documented hierarchy
    /// (`ordered_resolve_queue → rbf_candidates → verify_queue → orphan`),
    /// with the synchronous `pre_check_queue` mutex last. Each guard is
    /// released immediately after its `clear()`, so there is no deadlock
    /// risk, but the operation is *not* atomic: workers may keep moving
    /// transactions between queues while the clear is in progress, and
    /// transactions already popped by a worker are unaffected. Callers that
    /// need a guaranteed-empty pipeline must additionally quiesce the
    /// pipeline workers (not implemented here).
    pub(crate) async fn clear_pipeline_queues(&self) {
        self.queues.ordered_resolve_queue.write().await.clear();
        self.queues.rbf_candidates.write().await.clear();
        self.queues.verify_queue.write().await.clear();
        self.orphan.write().await.clear();
        // `pre_check_queue` uses a std::sync::Mutex, independent of the async
        // lock hierarchy; keep it last so it can never be held across an
        // `.await`.
        self.queues.pre_check_queue.clear();
    }

    pub(crate) async fn clear_pool(&mut self, new_snapshot: Arc<Snapshot>) {
        {
            let mut tx_pool = self.tx_pool.write().await;
            tx_pool.clear(Arc::clone(&new_snapshot));
        }
        self.clear_pipeline_queues().await;
        // reset block_assembler
        if self
            .block_assembler_sender
            .send(BlockAssemblerMessage::Reset(new_snapshot))
            .await
            .is_err()
        {
            error!("block_assembler receiver dropped");
        }
    }

    pub(crate) async fn save_pool(&self) {
        let mut tx_pool = self.tx_pool.write().await;
        if let Err(err) = tx_pool.save_into_file() {
            error!("failed to save pool, error: {:?}", err)
        } else {
            info!("TxPool saved successfully")
        }
    }

    pub(crate) async fn update_ibd_state(&self, in_ibd: bool) {
        self.aux.fee_estimator.update_ibd_state(in_ibd);
    }

    pub(crate) async fn estimate_fee_rate(
        &self,
        estimate_mode: EstimateMode,
        enable_fallback: bool,
    ) -> Result<FeeRate, AnyError> {
        let all_entry_info = self
            .read_tx_pool(|tx_pool| tx_pool.get_all_entry_info())
            .await;
        match self
            .aux
            .fee_estimator
            .estimate_fee_rate(estimate_mode, all_entry_info)
        {
            Ok(fee_rate) => Ok(fee_rate),
            Err(err) => {
                if enable_fallback {
                    let target_blocks =
                        FeeEstimator::target_blocks_for_estimate_mode(estimate_mode);
                    self.tx_pool
                        .read()
                        .await
                        .estimate_fee_rate(target_blocks)
                        .map_err(Into::into)
                } else {
                    Err(err.into())
                }
            }
        }
    }
}

pub(crate) struct PreCheckedTx {
    /// Tip hash at the time the transaction was pre-checked.
    pub(crate) pre_resolve_tip: Byte32,
    /// Fully resolved transaction.
    pub(crate) rtx: Arc<ResolvedTransaction>,
    /// Current status (fresh / gap / proposed) relative to the proposal window.
    pub(crate) status: Status,
    /// Transaction fee.
    pub(crate) fee: Capacity,
    /// Transaction size in bytes as serialized in a block.
    pub(crate) tx_size: usize,
}

/// Transactions recovered from the conflict cache after a failed or partial
/// RBF replacement, paired with their original pipeline source.
pub(crate) type RecoveredTxs = Vec<(TransactionView, TxSource)>;

/// Outcome of [`TxPoolService::try_submit_entry`].
pub(crate) type SubmitEntryOutcome = (Result<(), Reject>, RecoveredTxs, Vec<(TxEntry, Reject)>);

type ResolveResult = Result<(Arc<ResolvedTransaction>, Status), Reject>;

fn get_tx_status(snapshot: &Snapshot, short_id: &ProposalShortId) -> Status {
    if snapshot.proposals().contains_proposed(short_id) {
        Status::Proposed
    } else if snapshot.proposals().contains_gap(short_id) {
        Status::Gap
    } else {
        Status::Pending
    }
}

fn resolve_tx(
    tx_pool: &TxPool,
    snapshot: &Snapshot,
    tx: TransactionView,
    rbf: bool,
) -> ResolveResult {
    let short_id = tx.proposal_short_id();
    let tx_status = get_tx_status(snapshot, &short_id);
    tx_pool
        .resolve_tx_from_pool(tx, rbf)
        .map(|rtx| (rtx, tx_status))
}
