//! Provide Shared
#![allow(missing_docs)]
use crate::block_status::BlockStatus;
use crate::{HeaderMap, Snapshot, SnapshotMgr};
use arc_swap::{ArcSwap, Guard};
use ckb_async_runtime::Handle;
use ckb_chain_spec::consensus::Consensus;
use ckb_constant::sync::MAX_TIP_AGE;
use ckb_db_schema::{COLUMN_BLOCK_ARCHIVE, COLUMN_META, META_ARCHIVE_TIP};
use ckb_error::{AnyError, Error, InternalErrorKind};
use ckb_logger::debug;
use ckb_notify::NotifyController;
use ckb_proposal_table::ProposalView;
use ckb_stop_handler::{has_received_stop_signal, new_crossbeam_exit_rx, register_thread};
use ckb_store::{ChainDB, ChainStore, FreezerController, FreezerServiceConfig};
use ckb_systemtime::unix_time_as_millis;
use ckb_tx_pool::{BlockTemplate, TokioRwLock, TxPoolController};
use ckb_types::{
    H256, U256,
    core::{BlockNumber, EpochExt, EpochNumber, HeaderView, Version},
    packed::{self, Byte32},
    prelude::*,
};
use ckb_util::{Mutex, MutexGuard, shrink_to_fit};
use ckb_verification::cache::TxVerificationCache;
use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

const FREEZER_INTERVAL: Duration = Duration::from_secs(60);
// Retain the current epoch and the preceding 100 complete epochs in the hot store.
const THRESHOLD_EPOCH: EpochNumber = 100;
const MAX_FREEZE_LIMIT: BlockNumber = 30_000;

pub const SHRINK_THRESHOLD: usize = 300;

/// An owned permission to close on a freezer thread
pub struct FreezerClose {
    stopped: Arc<AtomicBool>,
    stop: ckb_channel::Sender<()>,
    finished: ckb_channel::Receiver<()>,
}

impl Drop for FreezerClose {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        let _ = self.stop.try_send(());
        let _ = self.finished.recv();
    }
}

/// Shared blockchain state accessible across the CKB node.
///
/// Provides thread-safe access to the chain store, transaction pool, consensus parameters,
/// and other core components needed throughout the node.
#[derive(Clone)]
pub struct Shared {
    pub(crate) store: ChainDB,
    pub(crate) tx_pool_controller: TxPoolController,
    pub(crate) notify_controller: NotifyController,
    pub(crate) txs_verify_cache: Arc<TokioRwLock<TxVerificationCache>>,
    pub(crate) consensus: Arc<Consensus>,
    pub(crate) snapshot_mgr: Arc<SnapshotMgr>,
    pub(crate) async_handle: Handle,
    pub(crate) ibd_finished: Arc<AtomicBool>,

    pub(crate) assume_valid_targets: Arc<Mutex<Option<Vec<H256>>>>,
    pub(crate) assume_valid_target_specified: Arc<Option<H256>>,

    pub header_map: Arc<HeaderMap>,
    pub(crate) block_status_map: Arc<DashMap<Byte32, BlockStatus>>,
    pub(crate) unverified_tip: Arc<ArcSwap<crate::HeaderIndex>>,
}

impl Shared {
    /// Construct new Shared
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: ChainDB,
        tx_pool_controller: TxPoolController,
        notify_controller: NotifyController,
        txs_verify_cache: Arc<TokioRwLock<TxVerificationCache>>,
        consensus: Arc<Consensus>,
        snapshot_mgr: Arc<SnapshotMgr>,
        async_handle: Handle,
        ibd_finished: Arc<AtomicBool>,

        assume_valid_targets: Arc<Mutex<Option<Vec<H256>>>>,
        assume_valid_target_specified: Arc<Option<H256>>,
        header_map: Arc<HeaderMap>,
        block_status_map: Arc<DashMap<Byte32, BlockStatus>>,
    ) -> Shared {
        let header = store
            .get_tip_header()
            .unwrap_or(consensus.genesis_block().header());
        let unverified_tip = Arc::new(ArcSwap::new(Arc::new(crate::HeaderIndex::new(
            header.number(),
            header.hash(),
            header.difficulty(),
        ))));

        Shared {
            store,
            tx_pool_controller,
            notify_controller,
            txs_verify_cache,
            consensus,
            snapshot_mgr,
            async_handle,
            ibd_finished,
            assume_valid_targets,
            assume_valid_target_specified,
            header_map,
            block_status_map,
            unverified_tip,
        }
    }
    /// Start the archiver; the close guard waits for accepted I/O and final sync.
    pub fn spawn_freeze(&self) -> Option<FreezerClose> {
        let freezer = self.store.freezer()?;
        let controller = FreezerController::start(
            freezer.clone(),
            self.async_handle.clone().into_inner(),
            FreezerServiceConfig::default(),
        )
        .expect("valid freezer service configuration");
        let signal_receiver = new_crossbeam_exit_rx();
        let (stop, stopping) = ckb_channel::bounded(1);
        let (finished, done) = ckb_channel::bounded(1);
        let shared = self.clone();
        let stopped = Arc::clone(&freezer.stopped);
        if let Some(metrics) = ckb_metrics::handle() {
            metrics.ckb_freezer_state.set(1);
            if let Some(raw) = self.store.get(COLUMN_META, META_ARCHIVE_TIP) {
                let tip = packed::NumberHashReader::from_slice_should_be_ok(&raw);
                let number: u64 = tip.number().into();
                metrics
                    .ckb_freezer_number
                    .set(number.min(i64::MAX as u64) as i64);
            }
        }
        let handle = thread::Builder::new()
            .name("Freezer".to_owned())
            .spawn(move || {
                let mut failed = false;
                loop {
                    ckb_channel::select! {
                        recv(signal_receiver) -> _ => break,
                        recv(stopping) -> _ => break,
                        default(FREEZER_INTERVAL) => {
                            if let Err(error) = shared.freeze(&controller) {
                                ckb_logger::error!("Freezer stopped after error: {error}");
                                failed = true;
                                break;
                            }
                        }
                    }
                }
                if let Err(error) = shared.async_handle.block_on(controller.shutdown()) {
                    ckb_logger::error!("Freezer shutdown failed: {error}");
                    failed = true;
                }
                if let Some(metrics) = ckb_metrics::handle() {
                    metrics.ckb_freezer_state.set(if failed { 4 } else { 0 });
                }
                let _ = finished.send(());
            })
            .expect("start freezer service");
        register_thread("freeze", handle);
        Some(FreezerClose {
            stopped,
            stop,
            finished: done,
        })
    }

    fn freeze(&self, controller: &FreezerController) -> Result<(), Error> {
        if let Some(metrics) = ckb_metrics::handle() {
            metrics.ckb_freezer_state.set(2);
        }
        let indexed = self
            .store
            .recover_archive_with_cancel(|| self.freezer_stopping())?;
        if indexed != self.store.freezer().expect("archive open").number()
            || self.freezer_stopping()
        {
            self.report_freezer_idle();
            return Ok(());
        }
        let snapshot = self.snapshot();
        let current_epoch = snapshot.epoch_ext().number();
        if self.is_initial_block_download() || current_epoch <= THRESHOLD_EPOCH {
            if let Some(metrics) = ckb_metrics::handle() {
                metrics.ckb_freezer_backlog.set(0);
            }
            self.report_freezer_idle();
            return Ok(());
        }
        let limit_hash = snapshot
            .get_epoch_index(current_epoch - THRESHOLD_EPOCH)
            .and_then(|index| snapshot.get_epoch_ext(&index))
            .expect("epoch exists")
            .last_block_hash_in_previous_epoch();
        let limit = snapshot
            .get_block_number(&limit_hash)
            .expect("epoch boundary exists");
        self.archive_through(controller, &snapshot, limit)?;
        drop(snapshot);
        ckb_logger::debug!(
            "Freezer I/O {:?}; collection {:?}",
            controller.status(),
            self.store.db().collection_status()
        );
        if !self.freezer_stopping() && self.store.archive_collection_due()? {
            if let Some(metrics) = ckb_metrics::handle() {
                metrics.ckb_freezer_state.set(3);
            }
            match self.store.collect_archive_with_cancel(
                &Default::default(),
                || self.freezer_stopping(),
                || self.refresh_snapshot(),
            ) {
                Ok(stats) => {
                    if let Some(metrics) = ckb_metrics::handle() {
                        metrics
                            .ckb_freezer_collection_total
                            .with_label_values(&["success"])
                            .inc();
                    }
                    ckb_logger::info!(
                        "Freezer collection {:?}; retention {:?}",
                        stats,
                        self.store.db().collection_status()
                    );
                }
                Err(error) => {
                    use ckb_db::CollectionAbort;
                    let outcome = match CollectionAbort::from_error(&error) {
                        Some(CollectionAbort::Cancelled) => "cancelled",
                        Some(CollectionAbort::RetainedReaders) => "retained_readers",
                        Some(CollectionAbort::WriterWait) => "writer_wait",
                        Some(CollectionAbort::DirtyKeys) => "dirty_keys",
                        Some(CollectionAbort::Reconciliation) => "reconciliation",
                        _ => "error",
                    };
                    if let Some(metrics) = ckb_metrics::handle() {
                        metrics
                            .ckb_freezer_collection_total
                            .with_label_values(&[outcome])
                            .inc();
                    }
                    ckb_logger::warn!(
                        "Freezer collection deferred: {error}; {:?}",
                        self.store.db().collection_status()
                    );
                    self.store.db().check_writable()?;
                }
            }
        }
        self.report_freezer_idle();
        Ok(())
    }

    fn report_freezer_idle(&self) {
        if let Some(metrics) = ckb_metrics::handle() {
            let status = self.store.db().collection_status();
            metrics.ckb_freezer_state.set(1);
            metrics
                .ckb_freezer_retired_generations
                .set(status.pinned_retired_generations as i64);
            metrics
                .ckb_freezer_oldest_retired_seconds
                .set(status.oldest_retired_age.as_secs().min(i64::MAX as u64) as i64);
        }
    }

    fn freezer_stopping(&self) -> bool {
        has_received_stop_signal()
            || self
                .store
                .freezer()
                .expect("archive open")
                .stopped
                .load(Ordering::SeqCst)
    }

    fn archive_through(
        &self,
        controller: &FreezerController,
        snapshot: &Snapshot,
        limit: BlockNumber,
    ) -> Result<(), Error> {
        let cursor = self.store.get(COLUMN_META, META_ARCHIVE_TIP).map(|raw| {
            packed::NumberHashReader::from_slice_should_be_ok(&raw)
                .block_hash()
                .to_entity()
        });
        let mut previous = cursor.and_then(|hash| snapshot.get_block_header(&hash));
        // Hash-addressed records remain valid on an old branch. Walk its headers
        // to the canonical fork and continue; no archive file is truncated.
        while let Some(header) = &previous {
            if self.freezer_stopping() {
                return Ok(());
            }
            if snapshot.get_block_hash(header.number()).as_ref() == Some(&header.hash()) {
                break;
            }
            previous = snapshot.get_block_header(&header.parent_hash());
        }
        let start = previous.map_or(1, |header| header.number().saturating_add(1));
        if let Some(metrics) = ckb_metrics::handle() {
            metrics.ckb_freezer_backlog.set(
                limit
                    .saturating_sub(start.saturating_sub(1))
                    .min(i64::MAX as u64) as i64,
            );
        }
        let end = limit.min(start.saturating_add(MAX_FREEZE_LIMIT - 1));
        let mut pending = Vec::new();
        let mut bytes = 0;
        let mut progress = None;
        for number in start..=end {
            if self.freezer_stopping() {
                break;
            }
            let hash = snapshot.get_block_hash(number).ok_or_else(|| {
                InternalErrorKind::DataCorrupted.other("canonical archive block is missing")
            })?;
            if snapshot
                .get_block_ext(&hash)
                .is_none_or(|ext| ext.verified != Some(true))
            {
                break;
            }
            if snapshot
                .get(COLUMN_BLOCK_ARCHIVE, hash.as_slice())
                .is_none()
            {
                let block = snapshot.get_block(&hash).ok_or_else(|| {
                    InternalErrorKind::DataCorrupted.other("verified archive body is missing")
                })?;
                let data = block.data();
                if !pending.is_empty()
                    && (pending.len() == 512 || bytes + data.as_slice().len() > 16 << 20)
                {
                    if !self.commit_archive_batch(
                        controller,
                        std::mem::take(&mut pending),
                        progress.take(),
                        limit,
                    )? {
                        return Ok(());
                    }
                    bytes = 0;
                }
                bytes += data.as_slice().len();
                pending.push(data);
            }
            progress = Some((number, hash));
        }
        self.commit_archive_batch(controller, pending, progress, limit)?;
        Ok(())
    }

    fn commit_archive_batch(
        &self,
        controller: &FreezerController,
        blocks: Vec<packed::Block>,
        progress: Option<(u64, packed::Byte32)>,
        limit: BlockNumber,
    ) -> Result<bool, Error> {
        if !blocks.is_empty() {
            self.async_handle.block_on(controller.append(blocks))?;
            let indexed = self
                .store
                .recover_archive_with_cancel(|| self.freezer_stopping())?;
            if indexed != controller.number() {
                return Ok(false);
            }
        }
        if let Some((number, hash)) = progress {
            self.store.set_archive_tip(number, hash)?;
            if let Some(metrics) = ckb_metrics::handle() {
                metrics
                    .ckb_freezer_number
                    .set(number.min(i64::MAX as u64) as i64);
                metrics
                    .ckb_freezer_backlog
                    .set(limit.saturating_sub(number).min(i64::MAX as u64) as i64);
                metrics
                    .ckb_freezer_last_progress_timestamp
                    .set((unix_time_as_millis() / 1000).min(i64::MAX as u64) as i64);
            }
        }
        Ok(true)
    }

    /// Returns a reference to the transaction pool controller.
    pub fn tx_pool_controller(&self) -> &TxPoolController {
        &self.tx_pool_controller
    }

    /// Returns the transaction verification cache.
    pub fn txs_verify_cache(&self) -> Arc<TokioRwLock<TxVerificationCache>> {
        Arc::clone(&self.txs_verify_cache)
    }

    /// Returns a reference to the notification controller.
    pub fn notify_controller(&self) -> &NotifyController {
        &self.notify_controller
    }

    /// Returns a guard to the current snapshot of the blockchain state.
    pub fn snapshot(&self) -> Guard<Arc<Snapshot>> {
        self.snapshot_mgr.load()
    }

    /// Return arc cloned snapshot
    pub fn cloned_snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.snapshot())
    }

    /// Stores a new snapshot of the blockchain state.
    pub fn store_snapshot(&self, snapshot: Arc<Snapshot>) {
        self.snapshot_mgr.store(snapshot)
    }

    /// Refreshes the current snapshot with the latest store data.
    pub fn refresh_snapshot(&self) {
        let new = self.snapshot().refresh(self.store.get_snapshot());
        self.store_snapshot(Arc::new(new));
    }

    /// Creates a new snapshot with the given tip and metadata.
    pub fn new_snapshot(
        &self,
        tip_header: HeaderView,
        total_difficulty: U256,
        epoch_ext: EpochExt,
        proposals: ProposalView,
    ) -> Arc<Snapshot> {
        Arc::new(Snapshot::new(
            tip_header,
            total_difficulty,
            epoch_ext,
            self.store.get_snapshot(),
            proposals,
            Arc::clone(&self.consensus),
        ))
    }

    /// Returns a reference to the consensus parameters.
    pub fn consensus(&self) -> &Consensus {
        &self.consensus
    }

    /// Return arc cloned consensus re
    pub fn cloned_consensus(&self) -> Arc<Consensus> {
        Arc::clone(&self.consensus)
    }

    /// Return async runtime handle
    pub fn async_handle(&self) -> &Handle {
        &self.async_handle
    }

    /// Returns the hash of the genesis block.
    pub fn genesis_hash(&self) -> Byte32 {
        self.consensus.genesis_hash()
    }

    /// Returns a reference to the chain store.
    pub fn store(&self) -> &ChainDB {
        &self.store
    }

    /// Return whether chain is in initial block download
    pub fn is_initial_block_download(&self) -> bool {
        // Once this function has returned false, it must remain false.
        if self.ibd_finished.load(Ordering::Acquire) {
            false
        } else if unix_time_as_millis().saturating_sub(self.snapshot().tip_header().timestamp())
            > MAX_TIP_AGE
        {
            true
        } else {
            self.ibd_finished.store(true, Ordering::Release);
            false
        }
    }

    /// Generate and return block_template
    pub fn get_block_template(
        &self,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<Version>,
    ) -> Result<Result<BlockTemplate, AnyError>, AnyError> {
        self.tx_pool_controller()
            .get_block_template(bytes_limit, proposals_limit, max_version)
    }

    pub fn set_unverified_tip(&self, header: crate::HeaderIndex) {
        self.unverified_tip.store(Arc::new(header));
    }
    pub fn get_unverified_tip(&self) -> crate::HeaderIndex {
        self.unverified_tip.load().as_ref().clone()
    }

    pub fn header_map(&self) -> &HeaderMap {
        &self.header_map
    }
    pub fn remove_header_view(&self, hash: &Byte32) {
        self.header_map.remove(hash);
    }

    pub fn block_status_map(&self) -> &DashMap<Byte32, BlockStatus> {
        &self.block_status_map
    }

    pub fn get_block_status(&self, block_hash: &Byte32) -> BlockStatus {
        match self.block_status_map().get(block_hash) {
            Some(status_ref) => *status_ref.value(),
            None => {
                if self.header_map().contains_key(block_hash) {
                    BlockStatus::HEADER_VALID
                } else {
                    let verified = self
                        .snapshot()
                        .get_block_ext(block_hash)
                        .map(|block_ext| block_ext.verified);
                    match verified {
                        None => BlockStatus::UNKNOWN,
                        Some(None) => BlockStatus::BLOCK_STORED,
                        Some(Some(true)) => BlockStatus::BLOCK_VALID,
                        Some(Some(false)) => BlockStatus::BLOCK_INVALID,
                    }
                }
            }
        }
    }

    pub fn contains_block_status<T: ChainStore>(
        &self,
        block_hash: &Byte32,
        status: BlockStatus,
    ) -> bool {
        self.get_block_status(block_hash).contains(status)
    }

    pub fn insert_block_status(&self, block_hash: Byte32, status: BlockStatus) {
        self.block_status_map.insert(block_hash, status);
    }

    pub fn remove_block_status(&self, block_hash: &Byte32) {
        let log_now = std::time::Instant::now();
        self.block_status_map.remove(block_hash);
        debug!("remove_block_status cost {:?}", log_now.elapsed());
        shrink_to_fit!(self.block_status_map, SHRINK_THRESHOLD);
        debug!(
            "remove_block_status shrink_to_fit cost {:?}",
            log_now.elapsed()
        );
    }

    pub fn assume_valid_targets(&self) -> MutexGuard<'_, Option<Vec<H256>>> {
        self.assume_valid_targets.lock()
    }

    pub fn assume_valid_target_specified(&self) -> Arc<Option<H256>> {
        Arc::clone(&self.assume_valid_target_specified)
    }
}

#[cfg(test)]
mod freezer_tests;
