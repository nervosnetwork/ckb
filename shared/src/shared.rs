//! TODO(doc): @quake
use crate::PeerIndex;
use crate::{Snapshot, SnapshotMgr};
use arc_swap::Guard;
use ckb_async_runtime::Handle;
use ckb_chain_spec::consensus::Consensus;
use ckb_chain_spec::SpecError;
use ckb_channel::Sender;
use ckb_constant::store::TX_INDEX_UPPER_BOUND;
use ckb_constant::sync::MAX_TIP_AGE;
use ckb_db::{Direction, IteratorMode};
use ckb_db_schema::{COLUMN_BLOCK_BODY, COLUMN_NUMBER_HASH};
use ckb_error::{Error, InternalErrorKind};
use ckb_notify::NotifyController;
use ckb_proposal_table::{ProposalTable, ProposalView};
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_store::{ChainDB, ChainStore};
use ckb_tx_pool::{TokioRwLock, TxPoolController};
use ckb_types::{
    core::{service, BlockNumber, EpochExt, EpochNumber, HeaderView},
    packed::{self, Byte32},
    prelude::*,
    U256,
};
use ckb_verification::cache::TxVerificationCache;
use faketime::unix_time_as_millis;
use std::cmp;
use std::collections::BTreeMap;
use std::collections::HashSet;
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
        self.stop.try_send();
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        if let Some(ref mut stop) = self.async_stop {
            stop.try_send();
        }
    }
}

/// TODO(doc): @quake
#[derive(Clone)]
pub struct Shared {
    pub(crate) store: ChainDB,
    pub(crate) tx_pool_controller: TxPoolController,
    pub(crate) notify_controller: NotifyController,
    pub(crate) txs_verify_cache: Arc<TokioRwLock<TxVerificationCache>>,
    pub(crate) consensus: Arc<Consensus>,
    pub(crate) snapshot_mgr: Arc<SnapshotMgr>,
    pub(crate) async_handle: Handle,
    // async stop handle, only test will be assigned
    pub(crate) async_stop: Option<StopHandler<()>>,
    pub(crate) ibd_finished: Arc<AtomicBool>,
    pub(crate) relay_tx_sender: Sender<(Option<PeerIndex>, Byte32)>,
}

impl Shared {
    pub(crate) fn init_snapshot(
        store: &ChainDB,
        consensus: Arc<Consensus>,
    ) -> Result<(Snapshot, ProposalTable), Error> {
        let (tip_header, epoch) = Self::init_store(&store, &consensus)?;
        let total_difficulty = store
            .get_block_ext(&tip_header.hash())
            .ok_or_else(|| InternalErrorKind::Database.other("failed to get tip's block_ext"))?
            .total_difficulty;
        let (proposal_table, proposal_view) = Self::init_proposal_table(&store, &consensus);

        let snapshot = Snapshot::new(
            tip_header,
            total_difficulty,
            epoch,
            store.get_snapshot(),
            proposal_view,
            consensus,
        );

        Ok((snapshot, proposal_table))
    }

    pub(crate) fn init_proposal_table(
        store: &ChainDB,
        consensus: &Consensus,
    ) -> (ProposalTable, ProposalView) {
        let proposal_window = consensus.tx_proposal_window();
        let tip_number = store.get_tip_header().expect("store inited").number();
        let mut proposal_ids = ProposalTable::new(proposal_window);
        let proposal_start = tip_number.saturating_sub(proposal_window.farthest());
        for bn in proposal_start..=tip_number {
            if let Some(hash) = store.get_block_hash(bn) {
                let mut ids_set = HashSet::new();
                if let Some(ids) = store.get_block_proposal_txs_ids(&hash) {
                    ids_set.extend(ids)
                }

                if let Some(us) = store.get_block_uncles(&hash) {
                    for u in us.data().into_iter() {
                        ids_set.extend(u.proposals().into_iter());
                    }
                }
                proposal_ids.insert(bn, ids_set);
            }
        }
        let dummy_proposals = ProposalView::default();
        let (_, proposals) = proposal_ids.finalize(&dummy_proposals, tip_number);
        (proposal_ids, proposals)
    }

    pub(crate) fn init_store(
        store: &ChainDB,
        consensus: &Consensus,
    ) -> Result<(HeaderView, EpochExt), Error> {
        match store
            .get_tip_header()
            .and_then(|header| store.get_current_epoch_ext().map(|epoch| (header, epoch)))
        {
            Some((tip_header, epoch)) => {
                if let Some(genesis_hash) = store.get_block_hash(0) {
                    let expect_genesis_hash = consensus.genesis_hash();
                    if genesis_hash == expect_genesis_hash {
                        Ok((tip_header, epoch))
                    } else {
                        Err(SpecError::GenesisMismatch {
                            expected: expect_genesis_hash,
                            actual: genesis_hash,
                        }
                        .into())
                    }
                } else {
                    Err(InternalErrorKind::Database
                        .other("genesis does not exist in database")
                        .into())
                }
            }
            None => store.init(&consensus).map(|_| {
                (
                    consensus.genesis_block().header(),
                    consensus.genesis_epoch_ext().to_owned(),
                )
            }),
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

            let stop = StopHandler::new(SignalSender::Crossbeam(signal_sender), Some(thread));
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
}
