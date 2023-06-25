use crate::callback::Callbacks;
use crate::component::entry::TxEntry;
use crate::component::orphan::Entry as OrphanEntry;
use crate::component::pool_map::Status;
use crate::error::Reject;
use crate::pool::TxPool;
use crate::service::{BlockAssemblerMessage, TxPoolService, TxVerificationResult};
use crate::try_or_return_with_snapshot;
use crate::util::{
    after_delay_window, check_tx_fee, check_txid_collision, is_missing_input,
    non_contextual_verify, time_relative_verify, verify_rtx,
};
use ckb_chain_spec::consensus::MAX_BLOCK_PROPOSALS_LIMIT;
use ckb_error::{AnyError, InternalErrorKind};
use ckb_jsonrpc_types::BlockTemplate;
use ckb_logger::Level::Trace;
use ckb_logger::{debug, error, info, log_enabled_target, trace_target};
use ckb_network::PeerIndex;
use ckb_snapshot::Snapshot;
use ckb_store::data_loader_wrapper::AsDataLoader;
use ckb_store::ChainStore;
use ckb_types::{
    core::{cell::ResolvedTransaction, BlockView, Capacity, Cycle, HeaderView, TransactionView},
    packed::{Byte32, ProposalShortId},
};
use ckb_util::LinkedHashSet;
use ckb_verification::{
    cache::{CacheEntry, Completed},
    ContextualTransactionVerifier, ScriptVerifyResult, TimeRelativeTransactionVerifier,
    TxVerifyEnv,
};
use std::collections::HashSet;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::block_in_place;

const DELAY_LIMIT: usize = 1_500 * 21; // 1_500 per block, 21 blocks

/// A list for plug target for `plug_entry` method
pub enum PlugTarget {
    /// Pending pool
    Pending,
    /// Proposed pool
    Proposed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxStatus {
    Fresh,
    Gap,
    Proposed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProcessResult {
    Suspended,
    Completed(Completed),
}

impl TxStatus {
    fn with_env(self, header: &HeaderView) -> TxVerifyEnv {
        match self {
            TxStatus::Fresh => TxVerifyEnv::new_submit(header),
            TxStatus::Gap => TxVerifyEnv::new_proposed(header, 0),
            TxStatus::Proposed => TxVerifyEnv::new_proposed(header, 1),
        }
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

    pub(crate) async fn fetch_tx_verify_cache(&self, hash: &Byte32) -> Option<CacheEntry> {
        let guard = self.txs_verify_cache.read().await;
        guard.peek(hash).cloned()
    }

    async fn fetch_txs_verify_cache(
        &self,
        txs: impl Iterator<Item = &TransactionView>,
    ) -> HashMap<Byte32, CacheEntry> {
        let guard = self.txs_verify_cache.read().await;
        txs.filter_map(|tx| {
            let hash = tx.hash();
            guard.peek(&hash).cloned().map(|value| (hash, value))
        })
        .collect()
    }

    pub(crate) async fn submit_entry(
        &self,
        pre_resolve_tip: Byte32,
        entry: TxEntry,
        mut status: TxStatus,
        conflicts: HashSet<ProposalShortId>,
    ) -> (Result<(), Reject>, Arc<Snapshot>) {
        let (ret, snapshot) = self
            .with_tx_pool_write_lock(move |tx_pool, snapshot| {
                // if snapshot changed by context switch
                // we need redo time_relative verify
                let tip_hash = snapshot.tip_hash();
                if pre_resolve_tip != tip_hash {
                    debug!(
                        "submit_entry {} context changed previous:{} now:{}",
                        entry.proposal_short_id(),
                        pre_resolve_tip,
                        tip_hash
                    );

                    // destructuring assignments are not currently supported
                    status = check_rtx(tx_pool, &snapshot, &entry.rtx)?;

                    let tip_header = snapshot.tip_header();
                    let tx_env = status.with_env(tip_header);
                    time_relative_verify(snapshot, Arc::clone(&entry.rtx), tx_env)?;
                }

                // try to remove conflicted tx here
                for r in conflicts.iter() {
                    eprintln!("removeing : {:?}", r);
                }
                _submit_entry(tx_pool, status, entry.clone(), &self.callbacks)?;
                Ok(())
            })
            .await;

        (ret, snapshot)
    }

    pub(crate) async fn notify_block_assembler(&self, status: TxStatus) {
        if self.should_notify_block_assembler() {
            match status {
                TxStatus::Fresh => {
                    if self
                        .block_assembler_sender
                        .send(BlockAssemblerMessage::Pending)
                        .await
                        .is_err()
                    {
                        error!("block_assembler receiver dropped");
                    }
                }
                TxStatus::Proposed => {
                    if self
                        .block_assembler_sender
                        .send(BlockAssemblerMessage::Proposed)
                        .await
                        .is_err()
                    {
                        error!("block_assembler receiver dropped");
                    }
                }
                _ => {}
            }
        }
    }

    pub(crate) async fn orphan_contains(&self, tx: &TransactionView) -> bool {
        let orphan = self.orphan.read().await;
        orphan.contains_key(&tx.proposal_short_id())
    }

    pub(crate) async fn chunk_contains(&self, tx: &TransactionView) -> bool {
        let chunk = self.chunk.read().await;
        chunk.contains_key(&tx.proposal_short_id())
    }

    pub(crate) async fn with_tx_pool_read_lock<U, F: FnMut(&TxPool, Arc<Snapshot>) -> U>(
        &self,
        mut f: F,
    ) -> (U, Arc<Snapshot>) {
        let tx_pool = self.tx_pool.read().await;
        let snapshot = tx_pool.cloned_snapshot();

        let ret = f(&tx_pool, Arc::clone(&snapshot));
        (ret, snapshot)
    }

    pub(crate) async fn with_tx_pool_write_lock<U, F: FnMut(&mut TxPool, Arc<Snapshot>) -> U>(
        &self,
        mut f: F,
    ) -> (U, Arc<Snapshot>) {
        let mut tx_pool = self.tx_pool.write().await;
        let snapshot = tx_pool.cloned_snapshot();

        let ret = f(&mut tx_pool, Arc::clone(&snapshot));
        (ret, snapshot)
    }

    pub(crate) async fn pre_check(
        &self,
        tx: &TransactionView,
    ) -> (Result<PreCheckedTx, Reject>, Arc<Snapshot>) {
        // Acquire read lock for cheap check
        let tx_size = tx.data().serialized_size_in_block();

        let (ret, snapshot) = self
            .with_tx_pool_read_lock(|tx_pool, snapshot| {
                let tip_hash = snapshot.tip_hash();

                // Same txid means exactly the same transaction, including inputs, outputs, witnesses, etc.
                // It's not possible for RBF, reject it directly
                check_txid_collision(tx_pool, tx)?;

                // Try normal path first, if double-spending check success we don't need RBF check
                // this make sure RBF won't introduce extra performance cost for hot path
                let res = resolve_tx(tx_pool, &snapshot, tx.clone(), false);
                if let Ok((rtx, status)) = res {
                    let fee = check_tx_fee(tx_pool, &snapshot, &rtx, tx_size)?;
                    return Ok((tip_hash, rtx, status, fee, tx_size, HashSet::new()));
                } else {
                    // Try RBF check
                    let conflicts = tx_pool.pool_map.find_conflict_tx(tx);
                    let (rtx, status) = resolve_tx(tx_pool, &snapshot, tx.clone(), false)?;
                    let fee = check_tx_fee(tx_pool, &snapshot, &rtx, tx_size)?;
                    tx_pool.check_rbf(&rtx, &conflicts, fee.into())?;
                    return Ok((tip_hash, rtx, status, fee, tx_size, conflicts));
                }
            })
            .await;

        (ret, snapshot)
    }

    pub(crate) fn non_contextual_verify(
        &self,
        tx: &TransactionView,
        remote: Option<(Cycle, PeerIndex)>,
    ) -> Result<(), Reject> {
        if let Err(reject) = non_contextual_verify(&self.consensus, tx) {
            if reject.is_malformed_tx() {
                if let Some(remote) = remote {
                    self.ban_malformed(remote.1, format!("reject {reject}"));
                }
            }
            return Err(reject);
        }
        Ok(())
    }

    pub(crate) async fn resumeble_process_tx(
        &self,
        tx: TransactionView,
        remote: Option<(Cycle, PeerIndex)>,
    ) -> Result<(), Reject> {
        // non contextual verify first
        self.non_contextual_verify(&tx, None)?;

        // eprintln!(
        //     "resumeble_process_tx: {:?} id: {:?}",
        //     tx.hash(),
        //     tx.proposal_short_id()
        // );
        if self.chunk_contains(&tx).await || self.orphan_contains(&tx).await {
            return Err(Reject::Duplicated(tx.hash()));
        }

        if let Some((ret, snapshot)) = self._resumeble_process_tx(tx.clone(), remote).await {
            match ret {
                Ok(processed) => {
                    if let ProcessResult::Completed(completed) = processed {
                        self.after_process(tx, remote, &snapshot, &Ok(completed))
                            .await;
                    }
                    Ok(())
                }
                Err(e) => {
                    self.after_process(tx, remote, &snapshot, &Err(e.clone()))
                        .await;
                    Err(e)
                }
            }
        } else {
            Ok(())
        }
    }

    pub(crate) async fn process_tx(
        &self,
        tx: TransactionView,
        remote: Option<(Cycle, PeerIndex)>,
    ) -> Result<Completed, Reject> {
        // non contextual verify first
        self.non_contextual_verify(&tx, remote)?;

        if self.chunk_contains(&tx).await || self.orphan_contains(&tx).await {
            return Err(Reject::Duplicated(tx.hash()));
        }

        if let Some((ret, snapshot)) = self._process_tx(tx.clone(), remote.map(|r| r.0)).await {
            self.after_process(tx, remote, &snapshot, &ret).await;
            ret
        } else {
            // currently, the returned cycles is not been used, mock 0 if delay
            Ok(Completed {
                cycles: 0,
                fee: Capacity::zero(),
            })
        }
    }

    pub(crate) async fn put_recent_reject(&self, tx_hash: &Byte32, reject: &Reject) {
        let mut tx_pool = self.tx_pool.write().await;
        if let Some(ref mut recent_reject) = tx_pool.recent_reject {
            if let Err(e) = recent_reject.put(tx_hash, reject.clone()) {
                error!("record recent_reject failed {} {} {}", tx_hash, reject, e);
            }
        }
    }

    pub(crate) async fn remove_tx(&self, tx_hash: Byte32) -> bool {
        let id = ProposalShortId::from_tx_hash(&tx_hash);
        {
            let mut chunk = self.chunk.write().await;
            if chunk.remove_chunk_tx(&id).is_some() {
                return true;
            }
        }
        {
            let mut orphan = self.orphan.write().await;
            if orphan.remove_orphan_tx(&id).is_some() {
                return true;
            }
        }
        let mut tx_pool = self.tx_pool.write().await;
        tx_pool.remove_tx(&id)
    }

    pub(crate) async fn after_process(
        &self,
        tx: TransactionView,
        remote: Option<(Cycle, PeerIndex)>,
        snapshot: &Snapshot,
        ret: &Result<Completed, Reject>,
    ) {
        let tx_hash = tx.hash();

        // The network protocol is switched after tx-pool confirms the cache,
        // there will be no problem with the current state as the choice of the broadcast protocol.
        let with_vm_2023 = {
            let epoch = snapshot
                .tip_header()
                .epoch()
                .minimum_epoch_number_after_n_blocks(1);

            self.consensus
                .hardfork_switch
                .ckb2023
                .is_vm_version_2_and_syscalls_3_enabled(epoch)
        };

        // log tx verification result for monitor node
        if log_enabled_target!("ckb_tx_monitor", Trace) {
            if let Ok(c) = ret {
                trace_target!(
                    "ckb_tx_monitor",
                    r#"{{"tx_hash":"{:#x}","cycles":{}}}"#,
                    tx_hash,
                    c.cycles
                );
            }
        }

        match remote {
            Some((declared_cycle, peer)) => match ret {
                Ok(_) => {
                    self.send_result_to_relayer(TxVerificationResult::Ok {
                        original_peer: Some(peer),
                        with_vm_2023,
                        tx_hash,
                    });
                    self.process_orphan_tx(&tx).await;
                }
                Err(reject) => {
                    if is_missing_input(reject) && all_inputs_is_unknown(snapshot, &tx) {
                        self.add_orphan(tx, peer, declared_cycle).await;
                    } else {
                        if reject.is_malformed_tx() {
                            self.ban_malformed(peer, format!("reject {reject}"));
                        }
                        if reject.is_allowed_relay() {
                            self.send_result_to_relayer(TxVerificationResult::Reject {
                                tx_hash: tx_hash.clone(),
                            });
                        }

                        if matches!(reject, Reject::Resolve(..) | Reject::Verification(..)) {
                            self.put_recent_reject(&tx_hash, reject).await;
                        }
                    }
                }
            },
            None => {
                match ret {
                    Ok(_) => {
                        self.send_result_to_relayer(TxVerificationResult::Ok {
                            original_peer: None,
                            with_vm_2023,
                            tx_hash,
                        });
                        self.process_orphan_tx(&tx).await;
                    }
                    Err(Reject::Duplicated(_)) => {
                        // re-broadcast tx when it's duplicated and submitted through local rpc
                        self.send_result_to_relayer(TxVerificationResult::Ok {
                            original_peer: None,
                            with_vm_2023,
                            tx_hash,
                        });
                    }
                    Err(reject) => {
                        if matches!(reject, Reject::Resolve(..) | Reject::Verification(..)) {
                            self.put_recent_reject(&tx_hash, reject).await;
                        }
                    }
                }
            }
        }
    }

    pub(crate) async fn add_orphan(
        &self,
        tx: TransactionView,
        peer: PeerIndex,
        declared_cycle: Cycle,
    ) {
        self.orphan
            .write()
            .await
            .add_orphan_tx(tx, peer, declared_cycle)
    }

    pub(crate) async fn find_orphan_by_previous(
        &self,
        tx: &TransactionView,
    ) -> Option<OrphanEntry> {
        let orphan = self.orphan.read().await;
        if let Some(id) = orphan.find_by_previous(tx) {
            return orphan.get(&id).cloned();
        }
        None
    }

    pub(crate) async fn remove_orphan_tx(&self, id: &ProposalShortId) {
        self.orphan.write().await.remove_orphan_tx(id);
    }

    pub(crate) async fn process_orphan_tx(&self, tx: &TransactionView) {
        let mut orphan_queue: VecDeque<TransactionView> = VecDeque::new();
        orphan_queue.push_back(tx.clone());

        while let Some(previous) = orphan_queue.pop_front() {
            if let Some(orphan) = self.find_orphan_by_previous(&previous).await {
                if orphan.cycle > self.tx_pool_config.max_tx_verify_cycles {
                    debug!(
                        "process_orphan {} add to chunk,  find previous from {}",
                        tx.hash(),
                        orphan.tx.hash()
                    );
                    self.remove_orphan_tx(&orphan.tx.proposal_short_id()).await;
                    self.chunk
                        .write()
                        .await
                        .add_tx(orphan.tx, Some((orphan.cycle, orphan.peer)));
                } else if let Some((ret, snapshot)) = self
                    ._process_tx(orphan.tx.clone(), Some(orphan.cycle))
                    .await
                {
                    match ret {
                        Ok(_) => {
                            let with_vm_2023 = {
                                let epoch = snapshot
                                    .tip_header()
                                    .epoch()
                                    .minimum_epoch_number_after_n_blocks(1);

                                self.consensus
                                    .hardfork_switch
                                    .ckb2023
                                    .is_vm_version_2_and_syscalls_3_enabled(epoch)
                            };
                            self.send_result_to_relayer(TxVerificationResult::Ok {
                                original_peer: Some(orphan.peer),
                                with_vm_2023,
                                tx_hash: orphan.tx.hash(),
                            });
                            debug!(
                                "process_orphan {} success, find previous from {}",
                                tx.hash(),
                                orphan.tx.hash()
                            );
                            self.remove_orphan_tx(&orphan.tx.proposal_short_id()).await;
                            orphan_queue.push_back(orphan.tx);
                        }
                        Err(reject) => {
                            debug!(
                                "process_orphan {} reject {}, find previous from {}",
                                tx.hash(),
                                reject,
                                orphan.tx.hash()
                            );
                            if !is_missing_input(&reject) {
                                self.remove_orphan_tx(&orphan.tx.proposal_short_id()).await;
                                if reject.is_malformed_tx() {
                                    self.ban_malformed(orphan.peer, format!("reject {reject}"));
                                }
                                if reject.is_allowed_relay() {
                                    self.send_result_to_relayer(TxVerificationResult::Reject {
                                        tx_hash: orphan.tx.hash(),
                                    });
                                }
                                if matches!(reject, Reject::Resolve(..) | Reject::Verification(..))
                                {
                                    self.put_recent_reject(&orphan.tx.hash(), &reject).await;
                                }
                            }
                            break;
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn send_result_to_relayer(&self, result: TxVerificationResult) {
        if let Err(e) = self.tx_relay_sender.send(result) {
            error!("tx-pool tx_relay_sender internal error {}", e);
        }
    }

    fn ban_malformed(&self, peer: PeerIndex, reason: String) {
        const DEFAULT_BAN_TIME: Duration = Duration::from_secs(3600 * 24 * 3);

        #[cfg(feature = "with_sentry")]
        use sentry::{capture_message, with_scope, Level};

        #[cfg(feature = "with_sentry")]
        with_scope(
            |scope| scope.set_fingerprint(Some(&["ckb-tx-pool", "receive-invalid-remote-tx"])),
            || {
                capture_message(
                    &format!(
                        "Ban peer {} for {} seconds, reason: \
                        {}",
                        peer,
                        DEFAULT_BAN_TIME.as_secs(),
                        reason
                    ),
                    Level::Info,
                )
            },
        );
        self.network.ban_peer(peer, DEFAULT_BAN_TIME, reason);
    }

    async fn _resumeble_process_tx(
        &self,
        tx: TransactionView,
        remote: Option<(Cycle, PeerIndex)>,
    ) -> Option<(Result<ProcessResult, Reject>, Arc<Snapshot>)> {
        let limit_cycles = self.tx_pool_config.max_tx_verify_cycles;
        let tx_hash = tx.hash();

        let (ret, snapshot) = self.pre_check(&tx).await;
        let (tip_hash, rtx, status, fee, tx_size, conflicts) =
            try_or_return_with_snapshot!(ret, snapshot);

        if self.is_in_delay_window(&snapshot) {
            let mut delay = self.delay.write().await;
            if delay.len() < DELAY_LIMIT {
                delay.insert(tx.proposal_short_id(), tx);
            }
            return None;
        }

        let cached = self.fetch_tx_verify_cache(&tx_hash).await;
        let tip_header = snapshot.tip_header();
        let tx_env = Arc::new(status.with_env(tip_header));

        let data_loader = snapshot.as_data_loader();

        let completed = if let Some(ref entry) = cached {
            match entry {
                CacheEntry::Completed(completed) => {
                    let ret = TimeRelativeTransactionVerifier::new(
                        Arc::clone(&rtx),
                        Arc::clone(&self.consensus),
                        data_loader,
                        tx_env,
                    )
                    .verify()
                    .map_err(Reject::Verification);
                    try_or_return_with_snapshot!(ret, snapshot);
                    *completed
                }
                CacheEntry::Suspended(_) => {
                    return Some((Ok(ProcessResult::Suspended), snapshot));
                }
            }
        } else {
            let is_chunk_full = self.is_chunk_full().await;

            let ret = block_in_place(|| {
                let verifier = ContextualTransactionVerifier::new(
                    Arc::clone(&rtx),
                    Arc::clone(&self.consensus),
                    data_loader,
                    tx_env,
                );

                let (ret, fee) = verifier
                    .resumable_verify(limit_cycles)
                    .map_err(Reject::Verification)?;

                match ret {
                    ScriptVerifyResult::Completed(cycles) => {
                        if let Some((declared, _)) = remote {
                            if declared != cycles {
                                return Err(Reject::DeclaredWrongCycles(declared, cycles));
                            }
                        }
                        Ok(CacheEntry::completed(cycles, fee))
                    }
                    ScriptVerifyResult::Suspended(state) => {
                        if is_chunk_full {
                            Err(Reject::Full("chunk".to_owned()))
                        } else {
                            let snap = Arc::new(state.try_into().map_err(Reject::Verification)?);
                            Ok(CacheEntry::suspended(snap, fee))
                        }
                    }
                }
            });

            let entry = try_or_return_with_snapshot!(ret, snapshot);
            match entry {
                cached @ CacheEntry::Suspended(_) => {
                    let ret = self
                        .enqueue_suspended_tx(rtx.transaction.clone(), cached, remote)
                        .await;
                    try_or_return_with_snapshot!(ret, snapshot);
                    return Some((Ok(ProcessResult::Suspended), snapshot));
                }
                CacheEntry::Completed(completed) => completed,
            }
        };

        let entry = TxEntry::new(rtx, completed.cycles, fee, tx_size);

        let (ret, submit_snapshot) = self.submit_entry(tip_hash, entry, status, conflicts).await;
        try_or_return_with_snapshot!(ret, submit_snapshot);

        self.notify_block_assembler(status).await;
        if cached.is_none() {
            // update cache
            let txs_verify_cache = Arc::clone(&self.txs_verify_cache);
            tokio::spawn(async move {
                let mut guard = txs_verify_cache.write().await;
                guard.put(tx_hash, CacheEntry::Completed(completed));
            });
        }

        Some((Ok(ProcessResult::Completed(completed)), submit_snapshot))
    }

    pub(crate) async fn is_chunk_full(&self) -> bool {
        self.chunk.read().await.is_full()
    }

    pub(crate) async fn enqueue_suspended_tx(
        &self,
        tx: TransactionView,
        cached: CacheEntry,
        remote: Option<(Cycle, PeerIndex)>,
    ) -> Result<(), Reject> {
        let tx_hash = tx.hash();
        let mut chunk = self.chunk.write().await;
        if chunk.add_tx(tx, remote) {
            let mut guard = self.txs_verify_cache.write().await;
            guard.put(tx_hash, cached);
        }

        Ok(())
    }

    pub(crate) async fn _process_tx(
        &self,
        tx: TransactionView,
        declared_cycles: Option<Cycle>,
    ) -> Option<(Result<Completed, Reject>, Arc<Snapshot>)> {
        let tx_hash = tx.hash();

        let (ret, snapshot) = self.pre_check(&tx).await;

        let (tip_hash, rtx, status, fee, tx_size, conflicts) =
            try_or_return_with_snapshot!(ret, snapshot);

        if self.is_in_delay_window(&snapshot) {
            let mut delay = self.delay.write().await;
            if delay.len() < DELAY_LIMIT {
                delay.insert(tx.proposal_short_id(), tx);
            }
            return None;
        }

        let verify_cache = self.fetch_tx_verify_cache(&tx_hash).await;
        let max_cycles = declared_cycles.unwrap_or_else(|| self.consensus.max_block_cycles());
        let tip_header = snapshot.tip_header();
        let tx_env = Arc::new(status.with_env(tip_header));

        let verified_ret = verify_rtx(
            Arc::clone(&snapshot),
            Arc::clone(&rtx),
            tx_env,
            &verify_cache,
            max_cycles,
        );

        let verified = try_or_return_with_snapshot!(verified_ret, snapshot);

        if let Some(declared) = declared_cycles {
            if declared != verified.cycles {
                return Some((
                    Err(Reject::DeclaredWrongCycles(declared, verified.cycles)),
                    snapshot,
                ));
            }
        }

        let entry = TxEntry::new(rtx, verified.cycles, fee, tx_size);

        let (ret, submit_snapshot) = self.submit_entry(tip_hash, entry, status, conflicts).await;
        try_or_return_with_snapshot!(ret, submit_snapshot);

        self.notify_block_assembler(status).await;

        if verify_cache.is_none() {
            // update cache
            let txs_verify_cache = Arc::clone(&self.txs_verify_cache);
            tokio::spawn(async move {
                let mut guard = txs_verify_cache.write().await;
                guard.put(tx_hash, CacheEntry::Completed(verified));
            });
        }

        Some((Ok(verified), submit_snapshot))
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

        let epoch_of_next_block = snapshot
            .tip_header()
            .epoch()
            .minimum_epoch_number_after_n_blocks(1);

        let new_tip_after_delay = after_delay_window(&snapshot);
        let is_in_delay_window = self.is_in_delay_window(&snapshot);

        let detached_headers: HashSet<Byte32> = detached_blocks
            .iter()
            .map(|blk| blk.header().hash())
            .collect();

        for blk in detached_blocks {
            detached.extend(blk.transactions().into_iter().skip(1))
        }

        for blk in attached_blocks {
            attached.extend(blk.transactions().into_iter().skip(1));
        }
        let retain: Vec<TransactionView> = detached.difference(&attached).cloned().collect();

        let fetched_cache = if is_in_delay_window {
            // If in delay_window, don't use the cache.
            HashMap::new()
        } else {
            self.fetch_txs_verify_cache(retain.iter()).await
        };

        // If there are any transactions requires re-process, return them.
        //
        // At present, there is only one situation:
        // - If the hardfork was happened, then re-process all transactions.
        let txs_opt = {
            // This closure is used to limit the lifetime of mutable tx_pool.
            let mut tx_pool = self.tx_pool.write().await;

            let txs_opt = if is_in_delay_window {
                {
                    self.chunk.write().await.clear();
                }
                Some(tx_pool.drain_all_transactions())
            } else {
                None
            };

            _update_tx_pool_for_reorg(
                &mut tx_pool,
                &attached,
                &detached_headers,
                detached_proposal_id,
                snapshot,
                &self.callbacks,
                mine_mode,
            );

            // Updates network fork switch if required.
            //
            // This operation should be ahead of any transaction which is processed with new
            // hardfork features.
            if !self.network.load_ckb2023()
                && self
                    .consensus
                    .hardfork_switch
                    .ckb2023
                    .is_vm_version_2_and_syscalls_3_enabled(epoch_of_next_block)
            {
                self.network.init_ckb2023()
            }

            // notice: readd_detached_tx don't update cache
            self.readd_detached_tx(&mut tx_pool, retain, fetched_cache);

            txs_opt
        };

        if let Some(txs) = txs_opt {
            let mut delay = self.delay.write().await;
            if delay.len() < DELAY_LIMIT {
                for tx in txs {
                    delay.insert(tx.proposal_short_id(), tx);
                }
            }
        }

        {
            let delay_txs = if !self.after_delay() && new_tip_after_delay {
                let limit = MAX_BLOCK_PROPOSALS_LIMIT as usize;
                let mut txs = Vec::with_capacity(limit);
                let mut delay = self.delay.write().await;
                let keys: Vec<_> = { delay.keys().take(limit).cloned().collect() };
                for k in keys {
                    if let Some(v) = delay.remove(&k) {
                        txs.push(v);
                    }
                }
                if delay.is_empty() {
                    self.set_after_delay_true();
                }
                Some(txs)
            } else {
                None
            };
            if let Some(txs) = delay_txs {
                self.try_process_txs(txs).await;
            }
        }

        {
            let mut orphan = self.orphan.write().await;
            orphan.remove_orphan_txs(attached.iter().map(|tx| tx.proposal_short_id()));
        }

        {
            let mut chunk = self.chunk.write().await;
            chunk.remove_chunk_txs(attached.iter().map(|tx| tx.proposal_short_id()));
        }
    }

    fn readd_detached_tx(
        &self,
        tx_pool: &mut TxPool,
        txs: Vec<TransactionView>,
        fetched_cache: HashMap<Byte32, CacheEntry>,
    ) {
        let max_cycles = self.tx_pool_config.max_tx_verify_cycles;
        for tx in txs {
            let tx_size = tx.data().serialized_size_in_block();
            let tx_hash = tx.hash();
            if let Ok((rtx, status)) = resolve_tx(tx_pool, tx_pool.snapshot(), tx, false) {
                if let Ok(fee) = check_tx_fee(tx_pool, tx_pool.snapshot(), &rtx, tx_size) {
                    let verify_cache = fetched_cache.get(&tx_hash).cloned();
                    let snapshot = tx_pool.cloned_snapshot();
                    let tip_header = snapshot.tip_header();
                    let tx_env = Arc::new(status.with_env(tip_header));
                    if let Ok(verified) = verify_rtx(
                        snapshot,
                        Arc::clone(&rtx),
                        tx_env,
                        &verify_cache,
                        max_cycles,
                    ) {
                        let entry = TxEntry::new(rtx, verified.cycles, fee, tx_size);
                        if let Err(e) = _submit_entry(tx_pool, status, entry, &self.callbacks) {
                            error!("readd_detached_tx submit_entry {} error {}", tx_hash, e);
                        } else {
                            debug!("readd_detached_tx submit_entry {}", tx_hash);
                        }
                    }
                }
            }
        }
    }

    pub(crate) async fn clear_pool(&mut self, new_snapshot: Arc<Snapshot>) {
        {
            let mut tx_pool = self.tx_pool.write().await;
            tx_pool.clear(Arc::clone(&new_snapshot));
        }
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
            info!("TxPool save successfully")
        }
    }

    // # Notice
    //
    // This method assumes that the inputs transactions are sorted.
    async fn try_process_txs(&self, txs: Vec<TransactionView>) {
        if txs.is_empty() {
            return;
        }
        let total = txs.len();
        let mut count = 0usize;
        for tx in txs {
            let tx_hash = tx.hash();
            if let Err(err) = self.process_tx(tx, None).await {
                error!("failed to process {:#x}, error: {:?}", tx_hash, err);
                count += 1;
            }
        }
        if count != 0 {
            info!("{}/{} transactions are failed to process", count, total);
        }
    }

    pub(crate) fn is_in_delay_window(&self, snapshot: &Snapshot) -> bool {
        let epoch = snapshot.tip_header().epoch();
        self.consensus.is_in_delay_window(&epoch)
    }
}

type PreCheckedTx = (
    Byte32,
    Arc<ResolvedTransaction>,
    TxStatus,
    Capacity,
    usize,
    HashSet<ProposalShortId>,
);

type ResolveResult = Result<(Arc<ResolvedTransaction>, TxStatus), Reject>;

fn get_tx_status(snapshot: &Snapshot, short_id: &ProposalShortId) -> TxStatus {
    if snapshot.proposals().contains_proposed(&short_id) {
        TxStatus::Proposed
    } else if snapshot.proposals().contains_gap(&short_id) {
        TxStatus::Gap
    } else {
        TxStatus::Fresh
    }
}

fn check_rtx(
    tx_pool: &TxPool,
    snapshot: &Snapshot,
    rtx: &ResolvedTransaction,
) -> Result<TxStatus, Reject> {
    let short_id = rtx.transaction.proposal_short_id();
    let tx_status = get_tx_status(snapshot, &short_id);
    tx_pool.check_rtx_from_pool(rtx).map(|_| tx_status)
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

fn _submit_entry(
    tx_pool: &mut TxPool,
    status: TxStatus,
    entry: TxEntry,
    callbacks: &Callbacks,
) -> Result<(), Reject> {
    //eprintln!("_submit_entry: {:?}", entry.proposal_short_id());
    match status {
        TxStatus::Fresh => {
            if tx_pool.add_pending(entry.clone())? {
                callbacks.call_pending(tx_pool, &entry);
            }
        }
        TxStatus::Gap => {
            if tx_pool.add_gap(entry.clone())? {
                callbacks.call_pending(tx_pool, &entry);
            }
        }
        TxStatus::Proposed => {
            if tx_pool.add_proposed(entry.clone())? {
                callbacks.call_proposed(tx_pool, &entry, true);
            }
        }
    }
    //eprintln!("finished submit: {:?}", entry.proposal_short_id());
    Ok(())
}

fn _update_tx_pool_for_reorg(
    tx_pool: &mut TxPool,
    attached: &LinkedHashSet<TransactionView>,
    detached_headers: &HashSet<Byte32>,
    detached_proposal_id: HashSet<ProposalShortId>,
    snapshot: Arc<Snapshot>,
    callbacks: &Callbacks,
    mine_mode: bool,
) {
    tx_pool.snapshot = Arc::clone(&snapshot);

    // NOTE: `remove_by_detached_proposal` will try to re-put the given expired/detached proposals into
    // pending-pool if they can be found within txpool. As for a transaction
    // which is both expired and committed at the one time(commit at its end of commit-window),
    // we should treat it as a committed and not re-put into pending-pool. So we should ensure
    // that involves `remove_committed_txs` before `remove_expired`.
    tx_pool.remove_committed_txs(attached.iter(), callbacks, detached_headers);
    tx_pool.remove_by_detached_proposal(detached_proposal_id.iter());

    // mine mode:
    // pending ---> gap ----> proposed
    // try move gap to proposed
    if mine_mode {
        let mut proposals = Vec::new();
        let mut gaps = Vec::new();

        for entry in tx_pool.pool_map.entries.get_by_status(&Status::Gap) {
            let short_id = entry.inner.proposal_short_id();
            if snapshot.proposals().contains_proposed(&short_id) {
                proposals.push((short_id, entry.inner.clone()));
            }
        }

        for entry in tx_pool.pool_map.entries.get_by_status(&Status::Pending) {
            let short_id = entry.inner.proposal_short_id();
            let elem = (short_id.clone(), entry.inner.clone());
            if snapshot.proposals().contains_proposed(&short_id) {
                proposals.push(elem);
            } else if snapshot.proposals().contains_gap(&short_id) {
                gaps.push(elem);
            }
        }

        for (id, entry) in proposals {
            debug!("begin to proposed: {:x}", id);
            if let Err(e) = tx_pool.proposed_rtx(&id) {
                callbacks.call_reject(tx_pool, &entry, e);
            } else {
                callbacks.call_proposed(tx_pool, &entry, false)
            }
        }

        for (id, entry) in gaps {
            debug!("begin to gap: {:x}", id);
            if let Err(e) = tx_pool.gap_rtx(&id) {
                debug!(
                    "Failed to add tx to gap {}, reason: {}",
                    entry.transaction().hash(),
                    e
                );
                callbacks.call_reject(tx_pool, &entry, e.clone());
            }
        }
    }

    // Remove expired transaction from pending
    tx_pool.remove_expired(callbacks);

    // Remove transactions from the pool until its size <= size_limit.
    tx_pool.limit_size(callbacks);
}

pub fn all_inputs_is_unknown(snapshot: &Snapshot, tx: &TransactionView) -> bool {
    !tx.input_pts_iter()
        .any(|pt| snapshot.transaction_exists(&pt.tx_hash()))
}
