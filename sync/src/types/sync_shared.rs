use super::inflight_blocks::InflightBlocks;
use super::peer::{PeerState, Peers};
use super::util::{TtlFilter, UnknownTxHashPriority};
use crate::{Status, StatusCode};
use ckb_app_config::SyncConfig;
#[cfg(test)]
use ckb_chain::VerifyResult;
use ckb_chain::{ChainController, RemoteBlock};
use ckb_chain_spec::consensus::Consensus;
use ckb_channel::Receiver;
use ckb_constant::sync::{
    MAX_UNKNOWN_TX_HASHES_SIZE, MAX_UNKNOWN_TX_HASHES_SIZE_PER_PEER, SUSPEND_SYNC_TIME,
};
use ckb_logger::{debug, info, warn};
use ckb_network::PeerIndex;
use ckb_shared::{block_status::BlockStatus, shared::Shared, types::HeaderIndexView};
use ckb_store::{ChainDB, ChainStore};
use ckb_traits::{HeaderFields, HeaderFieldsProvider};
use ckb_tx_pool::service::TxVerificationResult;
use ckb_types::{
    U256,
    core::{self, BlockNumber, EpochExt},
    packed::{self, Byte32},
    prelude::*,
};
use ckb_util::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use dashmap::{self, DashMap};
use keyed_priority_queue::{self, KeyedPriorityQueue};
use lru::LruCache;
use std::collections::{HashMap, HashSet};
use std::iter;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use super::active_chain::ActiveChain;

const GET_HEADERS_CACHE_SIZE: usize = 10000;

// <CompactBlockHash, (CompactBlock, <PeerIndex, (Vec<TransactionsIndex>, Vec<UnclesIndex>)>, timestamp)>
pub(crate) type PendingCompactBlockMap = HashMap<
    Byte32,
    (
        packed::CompactBlock,
        HashMap<PeerIndex, (Vec<u32>, Vec<u32>)>,
        u64,
    ),
>;

/// Sync state shared between sync and relayer protocol
#[derive(Clone)]
pub struct SyncShared {
    shared: Shared,
    state: Arc<SyncState>,
}

impl SyncShared {
    /// Create a SyncShared
    pub fn new(
        shared: Shared,
        sync_config: SyncConfig,
        tx_relay_receiver: Receiver<TxVerificationResult>,
    ) -> SyncShared {
        let (total_difficulty, header) = {
            let snapshot = shared.snapshot();
            (
                snapshot.total_difficulty().to_owned(),
                snapshot.tip_header().to_owned(),
            )
        };
        let shared_best_header = RwLock::new((header, total_difficulty).into());
        info!(
            "header_map.memory_limit {}",
            sync_config.header_map.memory_limit
        );

        let state = SyncState {
            shared_best_header,
            tx_filter: Mutex::new(TtlFilter::default()),
            unknown_tx_hashes: Mutex::new(KeyedPriorityQueue::new()),
            peers: Peers::default(),
            pending_get_block_proposals: DashMap::new(),
            pending_compact_blocks: tokio::sync::Mutex::new(HashMap::default()),
            inflight_proposals: DashMap::new(),
            inflight_blocks: RwLock::new(InflightBlocks::default()),
            pending_get_headers: RwLock::new(LruCache::new(GET_HEADERS_CACHE_SIZE)),
            tx_relay_receiver,
            min_chain_work: sync_config.min_chain_work,
        };

        SyncShared {
            shared,
            state: Arc::new(state),
        }
    }

    /// Shared chain db/config
    pub fn shared(&self) -> &Shared {
        &self.shared
    }

    /// Get snapshot with current chain
    pub fn active_chain(&self) -> ActiveChain {
        ActiveChain::new(self.clone(), Arc::clone(&self.shared.snapshot()))
    }

    /// Get chain store
    pub fn store(&self) -> &ChainDB {
        self.shared.store()
    }

    /// Get sync state
    pub fn state(&self) -> &SyncState {
        &self.state
    }

    /// Get consensus config
    pub fn consensus(&self) -> &Consensus {
        self.shared.consensus()
    }

    // Only used by unit test
    // Blocking insert a new block, return the verify result
    #[cfg(test)]
    pub(crate) fn blocking_insert_new_block(
        &self,
        chain: &ChainController,
        block: Arc<core::BlockView>,
    ) -> VerifyResult {
        chain.blocking_process_block(block)
    }

    pub(crate) fn accept_remote_block(&self, chain: &ChainController, remote_block: RemoteBlock) {
        {
            let entry = self
                .shared()
                .block_status_map()
                .entry(remote_block.block.header().hash());
            if let dashmap::mapref::entry::Entry::Vacant(entry) = entry {
                entry.insert(BlockStatus::BLOCK_RECEIVED);
            }
        }

        chain.asynchronous_process_remote_block(remote_block)
    }

    /// Sync a new valid header, try insert to sync state
    // Update the header_map
    // Update the block_status_map
    // Update the shared_best_header if need
    // Update the peer's best_known_header
    pub fn insert_valid_header(&self, peer: PeerIndex, header: &core::HeaderView) {
        let tip_number = self.active_chain().tip_number();
        let store_first = tip_number >= header.number();
        // We don't use header#parent_hash clone here because it will hold the arc counter of the SendHeaders message
        // which will cause the 2000 headers to be held in memory for a long time
        let parent_hash = Byte32::from_slice(header.data().raw().parent_hash().as_slice())
            .expect("checked slice length");
        let parent_header_index = self
            .get_header_index_view(&parent_hash, store_first)
            .expect("parent should be verified");
        let mut header_view = HeaderIndexView::new(
            header.hash(),
            header.number(),
            header.epoch(),
            header.timestamp(),
            parent_hash,
            parent_header_index.total_difficulty() + header.difficulty(),
        );

        let snapshot = Arc::clone(&self.shared.snapshot());
        header_view.build_skip(
            tip_number,
            |hash, store_first| self.get_header_index_view(hash, store_first),
            |number, current| {
                // shortcut to return an ancestor block
                if current.number <= snapshot.tip_number() && snapshot.is_main_chain(&current.hash)
                {
                    snapshot
                        .get_block_hash(number)
                        .and_then(|hash| self.get_header_index_view(&hash, true))
                } else {
                    None
                }
            },
        );
        self.shared.header_map().insert(header_view.clone());
        self.state
            .peers()
            .may_set_best_known_header(peer, header_view.as_header_index());
        self.state.may_set_shared_best_header(header_view);
    }

    pub(crate) fn get_header_index_view(
        &self,
        hash: &Byte32,
        store_first: bool,
    ) -> Option<HeaderIndexView> {
        let store = self.store();
        if store_first {
            store
                .get_block_header(hash)
                .and_then(|header| {
                    store
                        .get_block_ext(hash)
                        .map(|block_ext| (header, block_ext.total_difficulty).into())
                })
                .or_else(|| self.shared.header_map().get(hash))
        } else {
            self.shared.header_map().get(hash).or_else(|| {
                store.get_block_header(hash).and_then(|header| {
                    store
                        .get_block_ext(hash)
                        .map(|block_ext| (header, block_ext.total_difficulty).into())
                })
            })
        }
    }

    /// Check whether block has been inserted to chain store
    pub fn is_stored(&self, hash: &packed::Byte32) -> bool {
        let status = self.active_chain().get_block_status(hash);
        status.contains(BlockStatus::BLOCK_STORED)
    }

    /// Get epoch ext by block hash
    pub fn get_epoch_ext(&self, hash: &Byte32) -> Option<EpochExt> {
        self.store().get_block_epoch(hash)
    }

    /// Insert peer's unknown_header_list
    pub fn insert_peer_unknown_header_list(&self, pi: PeerIndex, header_list: Vec<Byte32>) {
        // update peer's unknown_header_list only once
        if self.state().peers.unknown_header_list_is_empty(pi) {
            // header list is an ordered list, sorted from highest to lowest,
            // so here you discard and exit early
            for hash in header_list {
                if let Some(header) = self.shared().header_map().get(&hash) {
                    self.state()
                        .peers
                        .may_set_best_known_header(pi, header.as_header_index());
                    break;
                } else {
                    self.state().peers.insert_unknown_header_hash(pi, hash)
                }
            }
        }
    }

    /// Return true when the block is that we have requested and received first time.
    pub fn new_block_received(&self, block: &core::BlockView) -> bool {
        if !self
            .state()
            .write_inflight_blocks()
            .remove_by_block((block.number(), block.hash()).into())
        {
            return false;
        }

        let status = self.active_chain().get_block_status(&block.hash());
        debug!(
            "new_block_received {}-{}, status: {:?}",
            block.number(),
            block.hash(),
            status
        );
        if !BlockStatus::HEADER_VALID.eq(&status) {
            return false;
        }

        if let dashmap::mapref::entry::Entry::Vacant(status) =
            self.shared().block_status_map().entry(block.hash())
        {
            status.insert(BlockStatus::BLOCK_RECEIVED);
            return true;
        }
        false
    }
}

impl HeaderFieldsProvider for SyncShared {
    fn get_header_fields(&self, hash: &Byte32) -> Option<HeaderFields> {
        self.shared
            .header_map()
            .get(hash)
            .map(|header| HeaderFields {
                hash: header.hash(),
                number: header.number(),
                epoch: header.epoch(),
                timestamp: header.timestamp(),
                parent_hash: header.parent_hash(),
            })
            .or_else(|| {
                self.store()
                    .get_block_header(hash)
                    .map(|header| HeaderFields {
                        hash: header.hash(),
                        number: header.number(),
                        epoch: header.epoch(),
                        timestamp: header.timestamp(),
                        parent_hash: header.parent_hash(),
                    })
            })
    }
}

pub struct SyncState {
    /* Status irrelevant to peers */
    shared_best_header: RwLock<HeaderIndexView>,
    tx_filter: Mutex<TtlFilter<Byte32>>,

    // The priority is ordering by timestamp (reversed), means do not ask the tx before this timestamp (timeout).
    unknown_tx_hashes: Mutex<KeyedPriorityQueue<Byte32, UnknownTxHashPriority>>,

    /* Status relevant to peers */
    peers: Peers,

    /* Cached items which we had received but not completely process */
    pending_get_block_proposals: DashMap<packed::ProposalShortId, HashSet<PeerIndex>>,
    pub(crate) pending_get_headers: RwLock<LruCache<(PeerIndex, Byte32), Instant>>,
    pending_compact_blocks: tokio::sync::Mutex<PendingCompactBlockMap>,

    /* In-flight items for which we request to peers, but not got the responses yet */
    inflight_proposals: DashMap<packed::ProposalShortId, BlockNumber>,
    inflight_blocks: RwLock<InflightBlocks>,

    /* cached for sending bulk */
    tx_relay_receiver: Receiver<TxVerificationResult>,
    pub(crate) min_chain_work: U256,
}

impl SyncState {
    pub fn min_chain_work(&self) -> &U256 {
        &self.min_chain_work
    }

    pub fn min_chain_work_ready(&self) -> bool {
        self.shared_best_header
            .read()
            .is_better_than(&self.min_chain_work)
    }

    pub fn n_sync_started(&self) -> &AtomicUsize {
        &self.peers.n_sync_started
    }

    pub fn peers(&self) -> &Peers {
        &self.peers
    }

    pub fn compare_with_pending_compact(&self, hash: &Byte32, now: u64) -> bool {
        let pending = self.pending_compact_blocks.blocking_lock();
        // After compact block request 2s or pending is empty, sync can create tasks
        pending.is_empty()
            || pending
                .get(hash)
                .map(|(_, _, time)| now > time + 2000)
                .unwrap_or(true)
    }

    pub async fn pending_compact_blocks(
        &self,
    ) -> tokio::sync::MutexGuard<'_, PendingCompactBlockMap> {
        self.pending_compact_blocks.lock().await
    }

    pub fn read_inflight_blocks(&self) -> RwLockReadGuard<'_, InflightBlocks> {
        self.inflight_blocks.read()
    }

    pub fn write_inflight_blocks(&self) -> RwLockWriteGuard<'_, InflightBlocks> {
        self.inflight_blocks.write()
    }

    pub fn take_relay_tx_verify_results(&self, limit: usize) -> Vec<TxVerificationResult> {
        self.tx_relay_receiver.try_iter().take(limit).collect()
    }

    pub fn shared_best_header(&self) -> HeaderIndexView {
        self.shared_best_header.read().to_owned()
    }

    pub fn shared_best_header_ref(&self) -> RwLockReadGuard<'_, HeaderIndexView> {
        self.shared_best_header.read()
    }

    pub fn may_set_shared_best_header(&self, header: HeaderIndexView) {
        let mut shared_best_header = self.shared_best_header.write();
        if !header.is_better_than(shared_best_header.total_difficulty()) {
            return;
        }

        if let Some(metrics) = ckb_metrics::handle() {
            metrics.ckb_shared_best_number.set(header.number() as i64);
        }
        *shared_best_header = header;
    }

    pub(crate) fn suspend_sync(&self, peer_state: &mut PeerState) {
        if peer_state.sync_started() {
            assert_ne!(
                self.peers.n_sync_started.fetch_sub(1, Ordering::AcqRel),
                0,
                "n_sync_started overflow when suspend_sync"
            );
        }
        peer_state.suspend_sync(SUSPEND_SYNC_TIME);
    }

    pub(crate) fn tip_synced(&self, peer_state: &mut PeerState) {
        if peer_state.sync_started() {
            assert_ne!(
                self.peers.n_sync_started.fetch_sub(1, Ordering::AcqRel),
                0,
                "n_sync_started overflow when tip_synced"
            );
        }
        peer_state.tip_synced();
    }

    pub fn mark_as_known_tx(&self, hash: Byte32) {
        self.mark_as_known_txs(iter::once(hash));
    }

    pub fn remove_from_known_txs(&self, hash: &Byte32) {
        self.tx_filter.lock().remove(hash);
    }

    // maybe someday we can use
    // where T: Iterator<Item=Byte32>,
    // for<'a> &'a T: Iterator<Item=&'a Byte32>,
    pub fn mark_as_known_txs(&self, hashes: impl Iterator<Item = Byte32> + std::clone::Clone) {
        let mut unknown_tx_hashes = self.unknown_tx_hashes.lock();
        let mut tx_filter = self.tx_filter.lock();

        for hash in hashes {
            unknown_tx_hashes.remove(&hash);
            tx_filter.insert(hash);
        }
    }

    pub fn pop_ask_for_txs(&self) -> HashMap<PeerIndex, Vec<Byte32>> {
        let mut unknown_tx_hashes = self.unknown_tx_hashes.lock();
        let mut result: HashMap<PeerIndex, Vec<Byte32>> = HashMap::new();
        let now = Instant::now();

        if !unknown_tx_hashes
            .peek()
            .map(|(_tx_hash, priority)| priority.should_request(now))
            .unwrap_or_default()
        {
            return result;
        }

        while let Some((tx_hash, mut priority)) = unknown_tx_hashes.pop() {
            if priority.should_request(now) {
                if let Some(peer_index) = priority.next_request_peer() {
                    result
                        .entry(peer_index)
                        .and_modify(|hashes| hashes.push(tx_hash.clone()))
                        .or_insert_with(|| vec![tx_hash.clone()]);
                    unknown_tx_hashes.push(tx_hash, priority);
                }
            } else {
                unknown_tx_hashes.push(tx_hash, priority);
                break;
            }
        }
        result
    }

    pub fn add_ask_for_txs(&self, peer_index: PeerIndex, tx_hashes: Vec<Byte32>) -> Status {
        let mut unknown_tx_hashes = self.unknown_tx_hashes.lock();

        for tx_hash in tx_hashes
            .into_iter()
            .take(MAX_UNKNOWN_TX_HASHES_SIZE_PER_PEER)
        {
            match unknown_tx_hashes.entry(tx_hash) {
                keyed_priority_queue::Entry::Occupied(entry) => {
                    let mut priority = entry.get_priority().clone();
                    priority.push_peer(peer_index);
                    entry.set_priority(priority);
                }
                keyed_priority_queue::Entry::Vacant(entry) => {
                    entry.set_priority(UnknownTxHashPriority {
                        request_time: Instant::now(),
                        peers: vec![peer_index],
                        requested: false,
                    })
                }
            }
        }

        // Check `unknown_tx_hashes`'s length after inserting the arrival `tx_hashes`
        if unknown_tx_hashes.len() >= MAX_UNKNOWN_TX_HASHES_SIZE
            || unknown_tx_hashes.len()
                >= self.peers.state.len() * MAX_UNKNOWN_TX_HASHES_SIZE_PER_PEER
        {
            warn!(
                "unknown_tx_hashes is too long, len: {}",
                unknown_tx_hashes.len()
            );

            let mut peer_unknown_counter = 0;
            for (_hash, priority) in unknown_tx_hashes.iter() {
                for peer in priority.peers.iter() {
                    if *peer == peer_index {
                        peer_unknown_counter += 1;
                    }
                }
            }
            if peer_unknown_counter >= MAX_UNKNOWN_TX_HASHES_SIZE_PER_PEER {
                return StatusCode::TooManyUnknownTransactions.into();
            }

            return Status::ignored();
        }

        Status::ok()
    }

    pub fn already_known_tx(&self, hash: &Byte32) -> bool {
        self.tx_filter.lock().contains(hash)
    }

    pub fn tx_filter(&self) -> MutexGuard<'_, TtlFilter<Byte32>> {
        self.tx_filter.lock()
    }

    pub fn unknown_tx_hashes(
        &self,
    ) -> MutexGuard<'_, KeyedPriorityQueue<Byte32, UnknownTxHashPriority>> {
        self.unknown_tx_hashes.lock()
    }

    pub fn insert_inflight_proposals(
        &self,
        ids: Vec<packed::ProposalShortId>,
        block_number: BlockNumber,
    ) -> Vec<bool> {
        ids.into_iter()
            .map(|id| match self.inflight_proposals.entry(id) {
                dashmap::mapref::entry::Entry::Occupied(mut occupied) => {
                    if *occupied.get() < block_number {
                        occupied.insert(block_number);
                        true
                    } else {
                        false
                    }
                }
                dashmap::mapref::entry::Entry::Vacant(vacant) => {
                    vacant.insert(block_number);
                    true
                }
            })
            .collect()
    }

    pub fn remove_inflight_proposals(&self, ids: &[packed::ProposalShortId]) -> Vec<bool> {
        ids.iter()
            .map(|id| self.inflight_proposals.remove(id).is_some())
            .collect()
    }

    pub fn clear_expired_inflight_proposals(&self, keep_min_block_number: BlockNumber) {
        self.inflight_proposals
            .retain(|_, block_number| *block_number >= keep_min_block_number);
    }

    pub fn contains_inflight_proposal(&self, proposal_id: &packed::ProposalShortId) -> bool {
        self.inflight_proposals.contains_key(proposal_id)
    }

    pub fn drain_get_block_proposals(
        &self,
    ) -> DashMap<packed::ProposalShortId, HashSet<PeerIndex>> {
        let ret = self.pending_get_block_proposals.clone();
        self.pending_get_block_proposals.clear();
        ret
    }

    pub fn insert_get_block_proposals(&self, pi: PeerIndex, ids: Vec<packed::ProposalShortId>) {
        for id in ids.into_iter() {
            self.pending_get_block_proposals
                .entry(id)
                .or_default()
                .insert(pi);
        }
    }

    // Disconnect this peer and remove inflight blocks by peer
    //
    // TODO: record peer's connection duration (disconnect time - connect established time)
    // and report peer's connection duration to ckb_metrics
    pub fn disconnected(&self, pi: PeerIndex) {
        let removed_inflight_blocks_count = self.write_inflight_blocks().remove_by_peer(pi);
        if removed_inflight_blocks_count > 0 {
            debug!(
                "disconnected {}, remove {} inflight blocks",
                pi, removed_inflight_blocks_count
            )
        }
        self.peers().disconnected(pi);
    }
}
