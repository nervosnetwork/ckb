use crate::relayer::compact_block::CompactBlock;
use crate::synchronizer::OrphanBlockPool;
use crate::NetworkProtocol;
use crate::BLOCK_DOWNLOAD_TIMEOUT;
use crate::MAX_PEERS_PER_BLOCK;
use crate::{MAX_HEADERS_LEN, MAX_TIP_AGE};
use bitflags::bitflags;
use ckb_chain::chain::ChainController;
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::extras::EpochExt;
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::ProposalShortId;
use ckb_core::Cycle;
use ckb_logger::{debug, debug_target};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::SyncMessage;
use ckb_shared::chain_state::ChainState;
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_traits::ChainProvider;
use ckb_util::{Mutex, MutexGuard};
use ckb_util::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use failure::Error as FailureError;
use faketime::unix_time_as_millis;
use flatbuffers::FlatBufferBuilder;
use fnv::{FnvHashMap, FnvHashSet};
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::cmp;
use std::collections::{hash_map::HashMap, hash_set::HashSet, BTreeMap};
use std::fmt;
use std::hash::Hash;
use std::mem::swap;
use std::ops::DerefMut;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const FILTER_SIZE: usize = 20000;
const MAX_ASK_MAP_SIZE: usize = 50000;
const MAX_ASK_SET_SIZE: usize = MAX_ASK_MAP_SIZE * 2;
const GET_HEADERS_CACHE_SIZE: usize = 10000;
// TODO: Need discussed
const GET_HEADERS_TIMEOUT: Duration = Duration::from_secs(15);
const TX_FILTER_SIZE: usize = 50000;
const TX_ASKED_SIZE: usize = TX_FILTER_SIZE;
const ORPHAN_BLOCK_SIZE: usize = 1024;

// State used to enforce CHAIN_SYNC_TIMEOUT
// Only in effect for outbound, non-manual connections, with
// m_protect == false
// Algorithm: if a peer's best known block has less work than our tip,
// set a timeout CHAIN_SYNC_TIMEOUT seconds in the future:
//   - If at timeout their best known block now has more work than our tip
//     when the timeout was set, then either reset the timeout or clear it
//     (after comparing against our current tip's work)
//   - If at timeout their best known block still has less work than our
//     tip did when the timeout was set, then send a getheaders message,
//     and set a shorter timeout, HEADERS_RESPONSE_TIME seconds in future.
//     If their best known block is still behind when that new timeout is
//     reached, disconnect.

#[derive(Clone, Debug, PartialEq)]
pub struct ChainSyncState {
    pub timeout: u64,
    pub work_header: Option<Header>,
    pub total_difficulty: Option<U256>,
    pub sent_getheaders: bool,
    pub not_sync_until: Option<u64>,
    pub protect: bool,
}

impl Default for ChainSyncState {
    fn default() -> Self {
        ChainSyncState {
            timeout: 0,
            work_header: None,
            total_difficulty: None,
            sent_getheaders: false,
            not_sync_until: None,
            protect: false,
        }
    }
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct PeerState {
    pub sync_started: bool,
    pub headers_sync_timeout: Option<u64>,
    pub last_block_announcement: Option<u64>, //ms
    pub is_outbound: bool,
    pub disconnect: bool,
    pub chain_sync: ChainSyncState,
    // The key is a `timeout`, means do not ask the tx before `timeout`.
    tx_ask_for_map: BTreeMap<Instant, Vec<H256>>,
    tx_ask_for_set: HashSet<H256>,

    pub best_known_header: Option<HeaderView>,
    pub last_common_header: Option<Header>,
}

impl PeerState {
    pub fn new(
        is_outbound: bool,
        chain_sync: ChainSyncState,
        headers_sync_timeout: Option<u64>,
    ) -> PeerState {
        PeerState {
            sync_started: false,
            headers_sync_timeout,
            last_block_announcement: None,
            is_outbound,
            disconnect: false,
            chain_sync,
            tx_ask_for_map: BTreeMap::default(),
            tx_ask_for_set: HashSet::default(),
            best_known_header: None,
            last_common_header: None,
        }
    }

    pub fn can_sync(&self, now: u64, ibd: bool) -> bool {
        // only sync with outbound peer in IBD
        (self.is_outbound || !ibd)
            && !self.sync_started
            && self
                .chain_sync
                .not_sync_until
                .map(|ts| ts < now)
                .unwrap_or(true)
    }

    pub fn start_sync(&mut self, headers_sync_timeout: u64) {
        self.sync_started = true;
        self.chain_sync.not_sync_until = None;
        self.headers_sync_timeout = Some(headers_sync_timeout);
    }

    pub fn stop_sync(&mut self, not_sync_until: u64) {
        self.sync_started = false;
        self.chain_sync.not_sync_until = Some(not_sync_until);
        self.headers_sync_timeout = None;
    }

    // Not use yet
    pub fn caught_up_sync(&mut self) {
        self.headers_sync_timeout = Some(std::u64::MAX);
    }

    pub fn add_ask_for_tx(
        &mut self,
        tx_hash: H256,
        last_ask_timeout: Option<Instant>,
    ) -> Option<Instant> {
        if self.tx_ask_for_map.len() > MAX_ASK_MAP_SIZE {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "this peer tx_ask_for_map is full, ignore {:#x}",
                tx_hash
            );
            return None;
        }
        if self.tx_ask_for_set.len() > MAX_ASK_SET_SIZE {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "this peer tx_ask_for_set is full, ignore {:#x}",
                tx_hash
            );
            return None;
        }
        // This peer already register asked for this tx
        if self.tx_ask_for_set.contains(&tx_hash) {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "this peer already register ask tx({:#x})",
                tx_hash
            );
            return None;
        }

        // Retry ask tx 30 seconds later
        let next_ask_timeout = last_ask_timeout
            .map(|time| cmp::max(time + Duration::from_secs(30), Instant::now()))
            .unwrap_or_else(Instant::now);
        self.tx_ask_for_map
            .entry(next_ask_timeout)
            .or_default()
            .push(tx_hash.clone());
        self.tx_ask_for_set.insert(tx_hash);
        Some(next_ask_timeout)
    }

    pub fn remove_ask_for_tx(&mut self, tx_hash: &H256) {
        self.tx_ask_for_set.remove(tx_hash);
    }

    pub fn pop_ask_for_txs(&mut self) -> Vec<H256> {
        let mut all_txs = Vec::new();
        let mut timeouts = Vec::new();
        let now = Instant::now();
        for (timeout, txs) in &self.tx_ask_for_map {
            if *timeout >= now {
                break;
            }
            timeouts.push(timeout.clone());
            all_txs.extend(txs.clone());
        }
        for timeout in timeouts {
            self.tx_ask_for_map.remove(&timeout);
        }
        all_txs
    }
}

#[derive(Default)]
pub struct Filter<T: Eq + Hash> {
    inner: LruCache<T, ()>,
}

impl<T: Eq + Hash> Filter<T> {
    pub fn new(size: usize) -> Self {
        Self {
            inner: LruCache::new(size),
        }
    }

    pub fn contains(&self, item: &T) -> bool {
        self.inner.contains_key(item)
    }

    pub fn insert(&mut self, item: T) -> bool {
        self.inner.insert(item, ()).is_none()
    }
}

#[derive(Default)]
pub struct KnownFilter {
    inner: FnvHashMap<PeerIndex, Filter<H256>>,
}

impl KnownFilter {
    /// Adds a value to the filter.
    /// If the filter did not have this value present, `true` is returned.
    /// If the filter did have this value present, `false` is returned.
    pub fn insert(&mut self, index: PeerIndex, hash: H256) -> bool {
        self.inner
            .entry(index)
            .or_insert_with(|| Filter::new(FILTER_SIZE))
            .insert(hash)
    }
}

#[derive(Default)]
pub struct Peers {
    pub state: RwLock<FnvHashMap<PeerIndex, PeerState>>,
}

#[derive(Debug, Clone)]
pub struct InflightState {
    pub(crate) peers: FnvHashSet<PeerIndex>,
    pub(crate) timestamp: u64,
}

impl Default for InflightState {
    fn default() -> Self {
        InflightState {
            peers: FnvHashSet::default(),
            timestamp: unix_time_as_millis(),
        }
    }
}

impl InflightState {
    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub fn remove(&mut self, peer: &PeerIndex) {
        self.peers.remove(peer);
    }
}

#[derive(Clone)]
pub struct InflightBlocks {
    blocks: FnvHashMap<PeerIndex, FnvHashSet<H256>>,
    states: FnvHashMap<H256, InflightState>,
}

impl Default for InflightBlocks {
    fn default() -> Self {
        InflightBlocks {
            blocks: FnvHashMap::default(),
            states: FnvHashMap::default(),
        }
    }
}

struct DebugHastSet<'a>(&'a FnvHashSet<H256>);

impl<'a> fmt::Debug for DebugHastSet<'a> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_set()
            .entries(self.0.iter().map(|h| format!("{:#x}", h)))
            .finish()
    }
}

impl fmt::Debug for InflightBlocks {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_map()
            .entries(self.blocks.iter().map(|(k, v)| (k, DebugHastSet(v))))
            .finish()?;
        fmt.debug_map()
            .entries(self.states.iter().map(|(k, v)| (format!("{:#x}", k), v)))
            .finish()
    }
}

impl InflightBlocks {
    pub fn blocks_iter(&self) -> impl Iterator<Item = (&PeerIndex, &FnvHashSet<H256>)> {
        self.blocks.iter()
    }

    pub fn total_inflight_count(&self) -> usize {
        self.states.len()
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub fn peer_inflight_count(&self, peer: &PeerIndex) -> usize {
        self.blocks.get(peer).map(HashSet::len).unwrap_or(0)
    }
    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub fn inflight_block_by_peer(&self, peer: &PeerIndex) -> Option<&FnvHashSet<H256>> {
        self.blocks.get(peer)
    }

    pub fn inflight_state_by_block(&self, block: &H256) -> Option<&InflightState> {
        self.states.get(block)
    }

    pub fn prune(&mut self) {
        let now = unix_time_as_millis();
        let block = &mut self.blocks;
        self.states.retain(|k, v| {
            let outdate = (v.timestamp + BLOCK_DOWNLOAD_TIMEOUT) < now;
            if outdate {
                for peer in &v.peers {
                    block.get_mut(peer).map(|set| set.remove(&k));
                }
            }
            !outdate
        });
    }

    pub fn insert(&mut self, peer: PeerIndex, hash: H256) -> bool {
        let state = self
            .states
            .entry(hash.clone())
            .or_insert_with(InflightState::default);
        if state.peers.len() >= MAX_PEERS_PER_BLOCK {
            return false;
        }

        let blocks = self.blocks.entry(peer).or_insert_with(FnvHashSet::default);
        let ret = blocks.insert(hash);
        if ret {
            state.peers.insert(peer);
        }
        ret
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub fn remove_by_peer(&mut self, peer: &PeerIndex) -> bool {
        self.blocks
            .remove(peer)
            .map(|blocks| {
                for block in blocks {
                    if let Some(state) = self.states.get_mut(&block) {
                        state.remove(peer)
                    }
                }
            })
            .is_some()
    }

    pub fn remove_by_block(&mut self, block: &H256) -> bool {
        self.states
            .remove(block)
            .map(|state| {
                for peer in state.peers {
                    self.blocks.get_mut(&peer).map(|set| set.remove(block));
                }
            })
            .is_some()
    }
}

impl Peers {
    pub fn on_connected(
        &self,
        peer: PeerIndex,
        headers_sync_timeout: Option<u64>,
        protect: bool,
        is_outbound: bool,
    ) {
        self.state
            .write()
            .entry(peer)
            .and_modify(|state| {
                state.headers_sync_timeout = headers_sync_timeout;
                state.chain_sync.protect = protect;
            })
            .or_insert_with(|| {
                let mut chain_sync = ChainSyncState::default();
                chain_sync.protect = protect;
                PeerState::new(is_outbound, chain_sync, headers_sync_timeout)
            });
    }

    pub fn get_best_known_header(&self, pi: PeerIndex) -> Option<HeaderView> {
        self.state
            .read()
            .get(&pi)
            .and_then(|peer_state| peer_state.best_known_header.clone())
    }

    pub fn set_best_known_header(&self, pi: PeerIndex, header_view: HeaderView) {
        self.state
            .write()
            .entry(pi)
            .and_modify(|peer_state| peer_state.best_known_header = Some(header_view));
    }

    pub fn get_last_common_header(&self, pi: PeerIndex) -> Option<Header> {
        self.state
            .read()
            .get(&pi)
            .and_then(|peer_state| peer_state.last_common_header.clone())
    }

    pub fn set_last_common_header(&self, pi: PeerIndex, header: Header) {
        self.state
            .write()
            .entry(pi)
            .and_modify(|peer_state| peer_state.last_common_header = Some(header));
    }

    pub fn new_header_received(&self, peer: PeerIndex, header_view: &HeaderView) {
        if let Some(peer_state) = self.state.write().get_mut(&peer) {
            if let Some(ref hv) = peer_state.best_known_header {
                if header_view.total_difficulty() > hv.total_difficulty()
                    || (header_view.total_difficulty() == hv.total_difficulty()
                        && header_view.hash() < hv.hash())
                {
                    peer_state.best_known_header = Some(header_view.clone());
                }
            } else {
                peer_state.best_known_header = Some(header_view.clone());
            }
        }
    }

    pub fn getheaders_received(&self, _peer: PeerIndex) {
        // TODO:
    }

    pub fn disconnected(&self, peer: PeerIndex) -> Option<PeerState> {
        self.state.write().remove(&peer)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderView {
    inner: Header,
    total_difficulty: U256,
    total_uncles_count: u64,
    // pointer to the index of some further predecessor of this block
    skip_hash: Option<H256>,
}

impl HeaderView {
    pub fn new(inner: Header, total_difficulty: U256, total_uncles_count: u64) -> Self {
        HeaderView {
            inner,
            total_difficulty,
            total_uncles_count,
            skip_hash: None,
        }
    }

    pub fn number(&self) -> BlockNumber {
        self.inner.number()
    }

    pub fn hash(&self) -> &H256 {
        self.inner.hash()
    }

    pub fn parent_hash(&self) -> &H256 {
        self.inner.parent_hash()
    }

    pub fn timestamp(&self) -> u64 {
        self.inner.timestamp()
    }

    pub fn total_uncles_count(&self) -> u64 {
        self.total_uncles_count
    }

    pub fn total_difficulty(&self) -> &U256 {
        &self.total_difficulty
    }

    pub fn inner(&self) -> &Header {
        &self.inner
    }

    pub fn into_inner(self) -> Header {
        self.inner
    }

    pub fn build_skip<F>(&mut self, mut get_header_view: F)
    where
        F: FnMut(&H256) -> Option<HeaderView>,
    {
        self.skip_hash = get_header_view(self.parent_hash())
            .and_then(|parent| parent.get_ancestor(get_skip_height(self.number()), get_header_view))
            .map(|header| header.hash().clone());
    }

    // NOTE: get_header_view may change source state, for cache or for tests
    pub fn get_ancestor<F>(self, number: BlockNumber, mut get_header_view: F) -> Option<Header>
    where
        F: FnMut(&H256) -> Option<HeaderView>,
    {
        let mut current = self;
        if number > current.number() {
            return None;
        }
        let mut number_walk = current.number();
        while number_walk > number {
            let number_skip = get_skip_height(number_walk);
            let number_skip_prev = get_skip_height(number_walk - 1);
            match current.skip_hash {
                Some(ref hash)
                    if number_skip == number
                        || (number_skip > number
                            && !(number_skip_prev + 2 < number_skip
                                && number_skip_prev >= number)) =>
                {
                    // Only follow skip if parent->skip isn't better than skip->parent
                    current = get_header_view(hash)?;
                    number_walk = number_skip;
                }
                _ => {
                    current = get_header_view(current.parent_hash())?;
                    number_walk -= 1;
                }
            }
        }
        Some(current.clone()).map(HeaderView::into_inner)
    }

    pub fn is_better_than(&self, total_difficulty: &U256, hash: &H256) -> bool {
        self.total_difficulty() > total_difficulty
            || (self.total_difficulty() == total_difficulty && self.hash() < hash)
    }
}

// Compute what height to jump back to with the skip pointer.
fn get_skip_height(height: BlockNumber) -> BlockNumber {
    // Turn the lowest '1' bit in the binary representation of a number into a '0'.
    fn invert_lowest_one(n: i64) -> i64 {
        n & (n - 1)
    }

    if height < 2 {
        return 0;
    }

    // Determine which height to jump back to. Any number strictly lower than height is acceptable,
    // but the following expression seems to perform well in simulations (max 110 steps to go back
    // up to 2**18 blocks).
    if (height & 1) > 0 {
        invert_lowest_one(invert_lowest_one(height as i64 - 1)) as u64 + 1
    } else {
        invert_lowest_one(height as i64) as u64
    }
}

#[derive(Default)]
pub struct EpochIndices {
    epoch: HashMap<H256, EpochExt>,
    indices: HashMap<H256, H256>,
}

impl EpochIndices {
    pub fn get_epoch_ext(&self, hash: &H256) -> Option<&EpochExt> {
        self.indices.get(hash).and_then(|h| self.epoch.get(h))
    }

    fn insert_index(&mut self, block_hash: H256, epoch_hash: H256) -> Option<H256> {
        self.indices.insert(block_hash, epoch_hash)
    }

    fn insert_epoch(&mut self, hash: H256, epoch: EpochExt) -> Option<EpochExt> {
        self.epoch.insert(hash, epoch)
    }
}

bitflags! {
    pub struct BlockStatus: u32 {
        const UNKNOWN            = 0;
        const VALID_HEADER       = 1;
        const VALID_TREE         = 2;
        const VALID_TRANSACTIONS = 3;
        const VALID_CHAIN        = 4;
        const VALID_SCRIPTS      = 5;

        const VALID_MASK         = Self::VALID_HEADER.bits | Self::VALID_TREE.bits | Self::VALID_TRANSACTIONS.bits |
                                   Self::VALID_CHAIN.bits | Self::VALID_SCRIPTS.bits;
        const BLOCK_HAVE_DATA    = 8;
        const BLOCK_HAVE_UNDO    = 16;
        const BLOCK_HAVE_MASK    = Self::BLOCK_HAVE_DATA.bits | Self::BLOCK_HAVE_UNDO.bits;
        const FAILED_VALID       = 32;
        const FAILED_CHILD       = 64;
        const FAILED_MASK        = Self::FAILED_VALID.bits | Self::FAILED_CHILD.bits;
    }
}

pub struct SyncSharedState<CS> {
    shared: Shared<CS>,

    n_sync_started: AtomicUsize,
    n_protected_outbound_peers: AtomicUsize,
    ibd_finished: AtomicBool,

    /* Status irrelevant to peers */
    shared_best_header: RwLock<HeaderView>,
    epoch_map: RwLock<EpochIndices>,
    header_map: RwLock<HashMap<H256, HeaderView>>,
    block_status_map: Mutex<hashbrown::HashMap<H256, BlockStatus>>,
    tx_filter: Mutex<Filter<H256>>,

    /* Status relevant to peers */
    peers: Peers,
    misbehavior: RwLock<FnvHashMap<PeerIndex, u32>>,
    known_blocks: Mutex<KnownFilter>,
    known_txs: Mutex<KnownFilter>,

    /* Cached items which we had received but not completely process */
    pending_get_block_proposals: Mutex<FnvHashMap<ProposalShortId, FnvHashSet<PeerIndex>>>,
    pending_get_headers: RwLock<LruCache<(PeerIndex, H256), Instant>>,
    pending_compact_blocks: Mutex<FnvHashMap<H256, (CompactBlock, FnvHashSet<PeerIndex>)>>,
    orphan_block_pool: OrphanBlockPool,

    /* In-flight items for which we request to peers, but not got the responses yet */
    inflight_proposals: Mutex<FnvHashSet<ProposalShortId>>,
    inflight_transactions: Mutex<LruCache<H256, Instant>>,
    inflight_blocks: RwLock<InflightBlocks>,
}

impl<CS: ChainStore> SyncSharedState<CS> {
    pub fn new(shared: Shared<CS>) -> SyncSharedState<CS> {
        let (total_difficulty, header, total_uncles_count) = {
            let chain_state = shared.lock_chain_state();
            let block_ext = shared
                .store()
                .get_block_ext(&chain_state.tip_hash())
                .expect("tip block_ext must exist");
            (
                chain_state.total_difficulty().to_owned(),
                chain_state.tip_header().to_owned(),
                block_ext.total_uncles_count,
            )
        };
        let shared_best_header = RwLock::new(HeaderView::new(
            header,
            total_difficulty,
            total_uncles_count,
        ));

        SyncSharedState {
            shared,
            n_sync_started: AtomicUsize::new(0),
            n_protected_outbound_peers: AtomicUsize::new(0),
            ibd_finished: AtomicBool::new(false),
            shared_best_header,
            header_map: RwLock::new(HashMap::new()),
            epoch_map: RwLock::new(EpochIndices::default()),
            block_status_map: Mutex::new(hashbrown::HashMap::new()),
            tx_filter: Mutex::new(Filter::new(TX_FILTER_SIZE)),
            peers: Peers::default(),
            misbehavior: RwLock::new(FnvHashMap::default()),
            known_blocks: Mutex::new(KnownFilter::default()),
            known_txs: Mutex::new(KnownFilter::default()),
            pending_get_block_proposals: Mutex::new(FnvHashMap::default()),
            pending_compact_blocks: Mutex::new(FnvHashMap::default()),
            orphan_block_pool: OrphanBlockPool::with_capacity(ORPHAN_BLOCK_SIZE),
            inflight_proposals: Mutex::new(FnvHashSet::default()),
            inflight_transactions: Mutex::new(LruCache::new(TX_ASKED_SIZE)),
            inflight_blocks: RwLock::new(InflightBlocks::default()),
            pending_get_headers: RwLock::new(LruCache::new(GET_HEADERS_CACHE_SIZE)),
        }
    }

    pub fn shared(&self) -> &Shared<CS> {
        &self.shared
    }
    pub fn n_sync_started(&self) -> &AtomicUsize {
        &self.n_sync_started
    }
    pub fn n_protected_outbound_peers(&self) -> &AtomicUsize {
        &self.n_protected_outbound_peers
    }
    pub fn peers(&self) -> &Peers {
        &self.peers
    }
    pub fn misbehavior(&self, pi: PeerIndex, score: u32) {
        if score != 0 {
            self.misbehavior
                .write()
                .entry(pi)
                .and_modify(|s| *s += score)
                .or_insert_with(|| score);
        }
    }
    pub fn known_blocks(&self) -> MutexGuard<KnownFilter> {
        self.known_blocks.lock()
    }
    pub fn known_txs(&self) -> MutexGuard<KnownFilter> {
        self.known_txs.lock()
    }
    pub fn pending_compact_blocks(
        &self,
    ) -> MutexGuard<FnvHashMap<H256, (CompactBlock, FnvHashSet<PeerIndex>)>> {
        self.pending_compact_blocks.lock()
    }
    pub fn inflight_transactions(&self) -> MutexGuard<LruCache<H256, Instant>> {
        self.inflight_transactions.lock()
    }
    pub fn read_inflight_blocks(&self) -> RwLockReadGuard<InflightBlocks> {
        self.inflight_blocks.read()
    }
    pub fn write_inflight_blocks(&self) -> RwLockWriteGuard<InflightBlocks> {
        self.inflight_blocks.write()
    }
    pub fn store(&self) -> &Arc<CS> {
        self.shared.store()
    }
    pub fn lock_chain_state(&self) -> MutexGuard<ChainState<CS>> {
        self.shared.lock_chain_state()
    }
    pub fn lock_txs_verify_cache(&self) -> MutexGuard<LruCache<H256, Cycle>> {
        self.shared.lock_txs_verify_cache()
    }
    pub fn tip_header(&self) -> Header {
        self.shared
            .store()
            .get_tip_header()
            .expect("get_tip_header")
    }
    pub fn consensus(&self) -> &Consensus {
        self.shared.consensus()
    }

    pub fn is_initial_block_download(&self) -> bool {
        // Once this function has returned false, it must remain false.
        if self.ibd_finished.load(Ordering::Relaxed) {
            false
        } else if unix_time_as_millis().saturating_sub(self.tip_header().timestamp()) > MAX_TIP_AGE
        {
            true
        } else {
            self.ibd_finished.store(true, Ordering::Relaxed);
            false
        }
    }

    pub fn is_initial_header_sync(&self) -> bool {
        unix_time_as_millis().saturating_sub(self.shared_best_header().timestamp()) > MAX_TIP_AGE
    }

    pub fn shared_best_header(&self) -> HeaderView {
        self.shared_best_header.read().to_owned()
    }
    pub fn set_shared_best_header(&self, header: HeaderView) {
        *self.shared_best_header.write() = header;
    }

    pub fn insert_header_view(&self, hash: H256, mut view: HeaderView) {
        view.build_skip(|hash| self.get_header_view(hash));
        self.header_map.write().insert(hash, view);
    }
    pub fn remove_header_view(&self, hash: &H256) {
        self.header_map.write().remove(hash);
    }
    pub fn get_header_view(&self, hash: &H256) -> Option<HeaderView> {
        self.header_map.read().get(hash).cloned().or_else(|| {
            self.shared
                .store()
                .get_block_header(hash)
                .and_then(|header| {
                    self.shared.store().get_block_ext(&hash).map(|block_ext| {
                        HeaderView::new(
                            header,
                            block_ext.total_difficulty,
                            block_ext.total_uncles_count,
                        )
                    })
                })
        })
    }
    pub fn get_header(&self, hash: &H256) -> Option<Header> {
        self.header_map
            .read()
            .get(hash)
            .map(HeaderView::inner)
            .cloned()
            .or_else(|| self.shared.store().get_block_header(hash))
    }

    pub fn get_epoch_ext(&self, hash: &H256) -> Option<EpochExt> {
        self.epoch_map
            .read()
            .get_epoch_ext(hash)
            .cloned()
            .or_else(|| self.shared.get_block_epoch(hash))
    }

    pub fn insert_epoch(&self, header: &Header, epoch: EpochExt) {
        let mut epoch_map = self.epoch_map.write();
        epoch_map.insert_index(
            header.hash().to_owned(),
            epoch.last_block_hash_in_previous_epoch().clone(),
        );
        epoch_map.insert_epoch(epoch.last_block_hash_in_previous_epoch().clone(), epoch);
    }

    pub fn next_epoch_ext(&self, last_epoch: &EpochExt, header: &Header) -> Option<EpochExt> {
        let consensus = self.shared.consensus();
        consensus.next_epoch_ext(
            last_epoch,
            header,
            |hash| self.get_header(hash),
            |hash| {
                self.get_header_view(hash)
                    .map(|view| view.total_uncles_count())
            },
        )
    }

    pub fn get_ancestor(&self, base: &H256, number: BlockNumber) -> Option<Header> {
        self.get_header_view(base)?
            .get_ancestor(number, |hash| self.get_header_view(hash))
    }

    pub fn get_locator(&self, start: &Header) -> Vec<H256> {
        let mut step = 1;
        let mut locator = Vec::with_capacity(32);
        let mut index = start.number();
        let mut base = start.hash().to_owned();
        loop {
            let header = self
                .get_ancestor(&base, index)
                .expect("index calculated in get_locator");
            locator.push(header.hash().to_owned());

            if locator.len() >= 10 {
                step <<= 1;
            }

            if index < step {
                // always include genesis hash
                if index != 0 {
                    locator.push(self.shared.genesis_hash().to_owned());
                }
                break;
            }
            index -= step;
            base = header.hash().to_owned();
        }
        locator
    }

    // If the peer reorganized, our previous last_common_header may not be an ancestor
    // of its current best_known_header. Go back enough to fix that.
    pub fn last_common_ancestor(
        &self,
        last_common_header: &Header,
        best_known_header: &Header,
    ) -> Option<Header> {
        debug_assert!(best_known_header.number() >= last_common_header.number());

        let mut m_right =
            self.get_ancestor(&best_known_header.hash(), last_common_header.number())?;

        if &m_right == last_common_header {
            return Some(m_right);
        }

        let mut m_left = self.get_header(&last_common_header.hash())?;
        debug_assert!(m_right.number() == m_left.number());

        while m_left != m_right {
            m_left = self.get_ancestor(&m_left.hash(), m_left.number() - 1)?;
            m_right = self.get_ancestor(&m_right.hash(), m_right.number() - 1)?;
        }
        Some(m_left)
    }

    pub fn locate_latest_common_block(
        &self,
        _hash_stop: &H256,
        locator: &[H256],
    ) -> Option<BlockNumber> {
        if locator.is_empty() {
            return None;
        }

        if locator.last().expect("empty checked") != self.shared.genesis_hash() {
            return None;
        }

        // iterator are lazy
        let (index, latest_common) = locator
            .iter()
            .enumerate()
            .map(|(index, hash)| (index, self.shared.store().get_block_number(hash)))
            .find(|(_index, number)| number.is_some())
            .expect("locator last checked");

        if index == 0 || latest_common == Some(0) {
            return latest_common;
        }

        if let Some(header) = locator
            .get(index - 1)
            .and_then(|hash| self.shared.store().get_block_header(hash))
        {
            let mut block_hash = header.parent_hash().to_owned();
            loop {
                let block_header = match self.shared.store().get_block_header(&block_hash) {
                    None => break latest_common,
                    Some(block_header) => block_header,
                };

                if let Some(block_number) = self.shared.store().get_block_number(&block_hash) {
                    return Some(block_number);
                }

                block_hash = block_header.parent_hash().to_owned();
            }
        } else {
            latest_common
        }
    }

    pub fn get_locator_response(&self, block_number: BlockNumber, hash_stop: &H256) -> Vec<Header> {
        let tip_number = self.tip_header().number();
        let max_height = cmp::min(
            block_number + 1 + MAX_HEADERS_LEN as BlockNumber,
            tip_number + 1,
        );
        (block_number + 1..max_height)
            .filter_map(|block_number| self.shared.store().get_block_hash(block_number))
            .take_while(|block_hash| block_hash != hash_stop)
            .filter_map(|block_hash| self.shared.store().get_block_header(&block_hash))
            .collect()
    }

    pub fn send_getheaders_to_peer(
        &self,
        nc: &CKBProtocolContext,
        peer: PeerIndex,
        header: &Header,
    ) {
        if let Some(last_time) = self
            .pending_get_headers
            .write()
            .get_refresh(&(peer, header.hash().to_owned()))
        {
            if Instant::now() < *last_time + GET_HEADERS_TIMEOUT {
                debug!(
                    "last send get headers from {} less than {:?} ago, ignore it",
                    peer, GET_HEADERS_TIMEOUT,
                );
                return;
            } else {
                debug!(
                    "Can not get headers from {} in {:?}, retry",
                    peer, GET_HEADERS_TIMEOUT,
                );
            }
        }
        self.pending_get_headers
            .write()
            .insert((peer, header.hash().to_owned()), Instant::now());

        debug!(
            "send_getheaders_to_peer peer={}, hash={:x}",
            peer,
            header.hash()
        );
        let locator_hash = self.get_locator(header);
        let fbb = &mut FlatBufferBuilder::new();
        let message = SyncMessage::build_get_headers(fbb, &locator_hash);
        fbb.finish(message, None);
        if let Err(err) = nc.send_message(
            NetworkProtocol::SYNC.into(),
            peer,
            fbb.finished_data().into(),
        ) {
            debug!("synchronizer send get_headers error: {:?}", err);
        }
    }

    pub fn mark_as_known_tx(&self, hash: H256) {
        self.inflight_transactions().remove(&hash);
        self.tx_filter.lock().insert(hash);
    }

    pub fn already_known_tx(&self, hash: &H256) -> bool {
        self.tx_filter.lock().contains(hash)
    }

    // Return true when the block is that we have requested and received first time.
    pub fn new_block_received(&self, block: &Block) -> bool {
        self.write_inflight_blocks()
            .remove_by_block(block.header().hash())
    }

    pub fn insert_inflight_proposals(&self, ids: Vec<ProposalShortId>) -> Vec<bool> {
        let mut locked = self.inflight_proposals.lock();
        ids.into_iter().map(|id| locked.insert(id)).collect()
    }

    pub fn remove_inflight_proposals(&self, ids: &[ProposalShortId]) -> Vec<bool> {
        let mut locked = self.inflight_proposals.lock();
        ids.iter().map(|id| locked.remove(id)).collect()
    }

    pub fn contains_orphan_block(&self, header: &Header) -> bool {
        self.orphan_block_pool.contains(header)
    }

    pub fn insert_orphan_block(&self, block: Block) {
        self.orphan_block_pool.insert(block)
    }

    pub fn remove_orphan_by_parent(&self, parent_hash: &H256) -> Vec<Block> {
        self.orphan_block_pool.remove_blocks_by_parent(parent_hash)
    }

    pub fn get_block_status(&self, block_hash: &H256) -> BlockStatus {
        let mut locked = self.block_status_map.lock();
        match locked.get(block_hash).cloned() {
            Some(status) => status,
            None => {
                if self.shared.store().get_block_header(block_hash).is_some() {
                    locked.insert(block_hash.clone(), BlockStatus::BLOCK_HAVE_MASK);
                    BlockStatus::BLOCK_HAVE_MASK
                } else {
                    BlockStatus::UNKNOWN
                }
            }
        }
    }

    pub fn insert_block_status(&self, block_hash: H256, status: BlockStatus) {
        self.block_status_map.lock().insert(block_hash, status);
    }

    pub fn clear_get_block_proposals(&self) -> FnvHashMap<ProposalShortId, FnvHashSet<PeerIndex>> {
        let mut locked = self.pending_get_block_proposals.lock();
        let old = locked.deref_mut();
        let mut ret = FnvHashMap::default();
        swap(old, &mut ret);
        ret
    }

    pub fn insert_get_block_proposals(&self, pi: PeerIndex, ids: Vec<ProposalShortId>) {
        let mut locked = self.pending_get_block_proposals.lock();
        for id in ids.into_iter() {
            locked.entry(id).or_default().insert(pi);
        }
    }

    pub fn disconnected(&self, pi: PeerIndex) -> Option<PeerState> {
        self.known_txs.lock().inner.remove(&pi);
        self.known_blocks.lock().inner.remove(&pi);
        self.inflight_blocks.write().remove_by_peer(&pi);
        self.peers().disconnected(pi)
    }

    pub fn insert_new_block(
        &self,
        chain: &ChainController,
        pi: PeerIndex,
        block: Arc<Block>,
    ) -> Result<(), FailureError> {
        let known_parent = |block: &Block| {
            self.store()
                .get_block_header(block.header().parent_hash())
                .is_some()
        };

        // Insert the given block into orphan_block_pool if its parent is not found
        if !known_parent(&block) {
            debug!(
                "insert new orphan block {} {:x}",
                block.header().number(),
                block.header().hash()
            );
            self.insert_orphan_block((*block).clone());
            return Ok(());
        }

        // Attempt to accept the given block if its parent already exist in database
        if let Err(err) = self.accept_block(chain, pi, Arc::clone(&block)) {
            debug!("accept block {:?} error {:?}", block, err);
            return Err(err);
        }

        // The above block has been accepted. Attempt to accept its descendant blocks in orphan pool.
        // The returned blocks of `remove_blocks_by_parent` are in topology order by parents
        let descendants = self.remove_orphan_by_parent(block.header().hash());
        for block in descendants {
            // If we can not find the block's parent in database, that means it was failed to accept
            // its parent, so we treat it as a invalid block as well.
            if !known_parent(&block) {
                debug!(
                    "parent-unknown orphan block, block: {}, {:x}, parent: {:x}",
                    block.header().number(),
                    block.header().hash(),
                    block.header().parent_hash(),
                );
                continue;
            }

            let block = Arc::new(block);
            if let Err(err) = self.accept_block(chain, pi, Arc::clone(&block)) {
                debug!(
                    "accept descendant orphan block {:#x} error {:?}",
                    block.header().hash(),
                    err
                );
            }
        }

        Ok(())
    }

    fn accept_block(
        &self,
        chain: &ChainController,
        peer: PeerIndex,
        block: Arc<Block>,
    ) -> Result<(), FailureError> {
        if let Err(err) = chain.process_block(Arc::clone(&block), true) {
            self.insert_block_status(block.header().hash().to_owned(), BlockStatus::FAILED_MASK);
            return Err(err);
        }

        self.remove_header_view(block.header().hash());
        self.insert_block_status(
            block.header().hash().to_owned(),
            BlockStatus::BLOCK_HAVE_MASK,
        );
        self.peers()
            .set_last_common_header(peer, block.header().clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use ckb_core::header::HeaderBuilder;
    use rand::{thread_rng, Rng};

    const SKIPLIST_LENGTH: u64 = 500_000;

    #[test]
    fn test_get_ancestor_use_skip_list() {
        let mut header_map: HashMap<H256, HeaderView> = HashMap::default();
        let mut hashes: BTreeMap<BlockNumber, H256> = BTreeMap::default();

        let mut parent_hash = None;
        for number in 0..SKIPLIST_LENGTH {
            let mut header_builder = HeaderBuilder::default().number(number);
            if let Some(parent_hash) = parent_hash.take() {
                header_builder = header_builder.parent_hash(parent_hash);
            }
            let header = header_builder.build();
            hashes.insert(number, header.hash().clone());
            parent_hash = Some(header.hash().clone());

            let mut view = HeaderView::new(header, U256::zero(), 0);
            view.build_skip(|hash| header_map.get(hash).cloned());
            header_map.insert(view.hash().clone(), view);
        }

        for (number, hash) in &hashes {
            if *number > 0 {
                let skip_view = header_map
                    .get(hash)
                    .and_then(|view| header_map.get(view.skip_hash.as_ref().unwrap()))
                    .unwrap();
                assert_eq!(Some(skip_view.hash()), hashes.get(&skip_view.number()));
                assert!(skip_view.number() < *number);
            } else {
                assert!(header_map[hash].skip_hash.is_none());
            }
        }

        let mut rng = thread_rng();
        let a_to_b = |a, b, limit| {
            let mut count = 0;
            let header = header_map
                .get(&hashes[&a])
                .cloned()
                .unwrap()
                .get_ancestor(b, |hash| {
                    count += 1;
                    header_map.get(hash).cloned()
                })
                .unwrap();

            // Search must finished in <limit> steps
            assert!(count <= limit);

            header
        };
        for _ in 0..1000 {
            let from: u64 = rng.gen_range(0, SKIPLIST_LENGTH);
            let to: u64 = rng.gen_range(0, from);
            let view_from = &header_map[&hashes[&from]];
            let view_to = &header_map[&hashes[&to]];
            let view_0 = &header_map[&hashes[&0]];

            let found_from_header = a_to_b(SKIPLIST_LENGTH - 1, from, 120);
            assert_eq!(found_from_header.hash(), view_from.hash());

            let found_to_header = a_to_b(from, to, 120);
            assert_eq!(found_to_header.hash(), view_to.hash());

            let found_0_header = a_to_b(from, 0, 120);
            assert_eq!(found_0_header.hash(), view_0.hash());
        }
    }
}
