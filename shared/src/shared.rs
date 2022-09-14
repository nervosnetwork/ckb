//! TODO(doc): @quake
use crate::{Snapshot, SnapshotMgr};
use arc_swap::Guard;
use ckb_async_runtime::Handle;
use ckb_chain_spec::consensus::Consensus;
use ckb_constant::store::TX_INDEX_UPPER_BOUND;
use ckb_constant::sync::MAX_TIP_AGE;
use ckb_db::{Direction, IteratorMode};
use ckb_db_schema::{COLUMN_BLOCK_BODY, COLUMN_NUMBER_HASH};
use ckb_error::{AnyError, Error};
use ckb_notify::NotifyController;
use ckb_proposal_table::ProposalView;
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_store::{ChainDB, ChainStore};
use ckb_tx_pool::{BlockTemplate, TokioRwLock, TxPoolController};
use ckb_types::{
    core::{service, BlockNumber, EpochExt, EpochNumber, HeaderView, Version},
    packed::{self, Byte32},
    prelude::*,
    U256,
};
use ckb_verification::cache::TxVerificationCache;
use faketime::unix_time_as_millis;
use std::cmp;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const FREEZER_INTERVAL: Duration = Duration::from_secs(60);
const THRESHOLD_EPOCH: EpochNumber = 2;
const MAX_FREEZE_LIMIT: BlockNumber = 30_000;

/// An owned permission to close on a freezer thread
pub struct FreezerClose {
    stopped: Arc<AtomicBool>,
    stop: StopHandler<()>,
}

impl Drop for FreezerClose {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.stop.try_send(());
    }
}

/// TODO(doc): @quake
#[derive(Clone)]
pub struct Shared {
    pub(crate) store: Arc<ChainDB>,
    pub(crate) tx_pool_controller: TxPoolController,
    pub(crate) notify_controller: NotifyController,
    pub(crate) txs_verify_cache: Arc<TokioRwLock<TxVerificationCache>>,
    pub(crate) consensus: Arc<Consensus>,
    pub(crate) snapshot_mgr: Arc<SnapshotMgr>,
    pub(crate) async_handle: Handle,
    pub(crate) ibd_finished: Arc<AtomicBool>,
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
    ) -> Shared {
        Shared {
            store: Arc::new(store),
            tx_pool_controller,
            notify_controller,
            txs_verify_cache,
            consensus,
            snapshot_mgr,
            async_handle,
            ibd_finished,
        }
    }
    /// Spawn freeze background thread that periodically checks and moves ancient data from the kv database into the freezer.
    pub fn spawn_freeze(&self) -> Option<FreezerClose> {
        if let Some(freezer) = self.store.freezer() {
            ckb_logger::info!("Freezer enable");
            let (signal_sender, signal_receiver) =
                ckb_channel::bounded::<()>(service::SIGNAL_CHANNEL_SIZE);
            let shared = self.clone();
            let thread = thread::Builder::new()
                .spawn(move || loop {
                    match signal_receiver.recv_timeout(FREEZER_INTERVAL) {
                        Err(_) => {
                            if let Err(e) = shared.freeze() {
                                ckb_logger::error!("Freezer error {}", e);
                                break;
                            }
                        }
                        Ok(_) => {
                            ckb_logger::info!("Freezer closing");
                            break;
                        }
                    }
                })
                .expect("Start FreezerService failed");

            let stop = StopHandler::new(
                SignalSender::Crossbeam(signal_sender),
                Some(thread),
                "freezer".to_string(),
            );
            return Some(FreezerClose {
                stopped: Arc::clone(&freezer.stopped),
                stop,
            });
        }
        None
    }

    fn freeze(&self) -> Result<(), Error> {
        let freezer = self.store.freezer().expect("freezer inited");
        let snapshot = self.snapshot();
        let current_epoch = snapshot.epoch_ext().number();

        if self.is_initial_block_download() {
            ckb_logger::trace!("is_initial_block_download freeze skip");
            return Ok(());
        }

        if current_epoch <= THRESHOLD_EPOCH {
            ckb_logger::trace!("freezer loaf");
            return Ok(());
        }

        let limit_block_hash = snapshot
            .get_epoch_index(current_epoch + 1 - THRESHOLD_EPOCH)
            .and_then(|index| snapshot.get_epoch_ext(&index))
            .expect("get_epoch_ext")
            .last_block_hash_in_previous_epoch();

        let frozen_number = freezer.number();

        let threshold = cmp::min(
            snapshot
                .get_block_number(&limit_block_hash)
                .expect("get_block_number"),
            frozen_number + MAX_FREEZE_LIMIT,
        );

        ckb_logger::trace!(
            "freezer current_epoch {} number {} threshold {}",
            current_epoch,
            frozen_number,
            threshold
        );

        let store = self.store();
        let get_unfrozen_block = |number: BlockNumber| {
            store
                .get_block_hash(number)
                .and_then(|hash| store.get_unfrozen_block(&hash))
        };

        let ret = freezer.freeze(threshold, get_unfrozen_block)?;

        let stopped = freezer.stopped.load(Ordering::SeqCst);

        // Wipe out frozen data
        self.wipe_out_frozen_data(&snapshot, ret, stopped)?;

        ckb_logger::trace!("freezer finish");

        Ok(())
    }

    fn wipe_out_frozen_data(
        &self,
        snapshot: &Snapshot,
        frozen: BTreeMap<packed::Byte32, (BlockNumber, u32)>,
        stopped: bool,
    ) -> Result<(), Error> {
        let mut side = BTreeMap::new();
        let mut batch = self.store.new_write_batch();

        ckb_logger::trace!("freezer wipe_out_frozen_data {} ", frozen.len());

        if !frozen.is_empty() {
            // remain header
            for (hash, (number, txs)) in &frozen {
                batch.delete_block_body(*number, hash, *txs).map_err(|e| {
                    ckb_logger::error!("freezer delete_block_body failed {}", e);
                    e
                })?;

                let pack_number: packed::Uint64 = number.pack();
                let prefix = pack_number.as_slice();
                for (key, value) in snapshot
                    .get_iter(
                        COLUMN_NUMBER_HASH,
                        IteratorMode::From(prefix, Direction::Forward),
                    )
                    .take_while(|(key, _)| key.starts_with(prefix))
                {
                    let reader = packed::NumberHashReader::from_slice_should_be_ok(key.as_ref());
                    let block_hash = reader.block_hash().to_entity();
                    if &block_hash != hash {
                        let txs =
                            packed::Uint32Reader::from_slice_should_be_ok(value.as_ref()).unpack();
                        side.insert(block_hash, (reader.number().to_entity(), txs));
                    }
                }
            }
            self.store.write_sync(&batch).map_err(|e| {
                ckb_logger::error!("freezer write_batch delete failed {}", e);
                e
            })?;
            batch.clear()?;

            if !stopped {
                let start = frozen.keys().min().expect("frozen empty checked");
                let end = frozen.keys().max().expect("frozen empty checked");
                self.compact_block_body(start, end);
            }
        }

        if !side.is_empty() {
            // Wipe out side chain
            for (hash, (number, txs)) in &side {
                batch
                    .delete_block(number.unpack(), hash, *txs)
                    .map_err(|e| {
                        ckb_logger::error!("freezer delete_block_body failed {}", e);
                        e
                    })?;
            }

            self.store.write(&batch).map_err(|e| {
                ckb_logger::error!("freezer write_batch delete failed {}", e);
                e
            })?;

            if !stopped {
                let start = side.keys().min().expect("side empty checked");
                let end = side.keys().max().expect("side empty checked");
                self.compact_block_body(start, end);
            }
        }
        Ok(())
    }

    fn compact_block_body(&self, start: &packed::Byte32, end: &packed::Byte32) {
        let start_t = packed::TransactionKey::new_builder()
            .block_hash(start.clone())
            .index(0u32.pack())
            .build();

        let end_t = packed::TransactionKey::new_builder()
            .block_hash(end.clone())
            .index(TX_INDEX_UPPER_BOUND.pack())
            .build();

        if let Err(e) = self.store.compact_range(
            COLUMN_BLOCK_BODY,
            Some(start_t.as_slice()),
            Some(end_t.as_slice()),
        ) {
            ckb_logger::error!("freezer compact_range {}-{} error {}", start, end, e);
        }
    }

    /// TODO(doc): @quake
    pub fn tx_pool_controller(&self) -> &TxPoolController {
        &self.tx_pool_controller
    }

    /// TODO(doc): @quake
    pub fn txs_verify_cache(&self) -> Arc<TokioRwLock<TxVerificationCache>> {
        Arc::clone(&self.txs_verify_cache)
    }

    /// TODO(doc): @quake
    pub fn notify_controller(&self) -> &NotifyController {
        &self.notify_controller
    }

    /// TODO(doc): @quake
    pub fn snapshot(&self) -> Guard<Arc<Snapshot>> {
        self.snapshot_mgr.load()
    }

    /// TODO(doc): @quake
    pub fn store_snapshot(&self, snapshot: Arc<Snapshot>) {
        self.snapshot_mgr.store(snapshot)
    }

    /// TODO(doc): @quake
    pub fn refresh_snapshot(&self) {
        let new = self.snapshot().refresh(self.store.get_snapshot());
        self.store_snapshot(Arc::new(new));
    }

    /// TODO(doc): @quake
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

    /// TODO(doc): @quake
    pub fn consensus(&self) -> &Consensus {
        &self.consensus
    }

    /// Makes a clone of the `Arc<Consensus>`
    pub fn cloned_consensus(&self) -> Arc<Consensus> {
        Arc::clone(&self.consensus)
    }

    /// Return async runtime handle
    pub fn async_handle(&self) -> &Handle {
        &self.async_handle
    }

    /// TODO(doc): @quake
    pub fn genesis_hash(&self) -> Byte32 {
        self.consensus.genesis_hash()
    }

    /// TODO(doc): @quake
    pub fn store(&self) -> &ChainDB {
        &self.store
    }

    /// Return arc cloned store
    pub fn cloned_store(&self) -> Arc<ChainDB> {
        Arc::clone(&self.store)
    }

    /// Return whether chain is in initial block download
    pub fn is_initial_block_download(&self) -> bool {
        // Once this function has returned false, it must remain false.
        if self.ibd_finished.load(Ordering::Relaxed) {
            false
        } else if unix_time_as_millis().saturating_sub(self.snapshot().tip_header().timestamp())
            > MAX_TIP_AGE
        {
            true
        } else {
            self.ibd_finished.store(true, Ordering::Relaxed);
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
        self.tx_pool_controller().get_block_template(
            bytes_limit,
            proposals_limit,
            max_version.map(Into::into),
        )
    }
}
