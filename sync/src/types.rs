use crate::block_status::BlockStatus;
use crate::orphan_block_pool::OrphanBlockPool;
use crate::BLOCK_DOWNLOAD_TIMEOUT;
use crate::{NetworkProtocol, SUSPEND_SYNC_TIME};
use crate::{
    FIRST_LEVEL_MAX, INIT_BLOCKS_IN_TRANSIT_PER_PEER, MAX_BLOCKS_IN_TRANSIT_PER_PEER,
    MAX_HEADERS_LEN, MAX_TIP_AGE, RETRY_ASK_TX_TIMEOUT_INCREASE,
};
use ckb_chain::chain::ChainController;
use ckb_chain_spec::consensus::Consensus;
use ckb_logger::{debug, debug_target, error, metric};
use ckb_network::{bytes, CKBProtocolContext, PeerIndex};
use ckb_shared::{shared::Shared, Snapshot};
use ckb_store::{ChainDB, ChainStore};
use ckb_types::{
    core::{self, BlockNumber, EpochExt},
    packed::{self, Byte32},
    prelude::*,
    U256,
};
use ckb_util::LinkedHashSet;
use ckb_util::{Mutex, MutexGuard};
use ckb_util::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use ckb_verification::HeaderResolverWrapper;
use failure::Error as FailureError;
use faketime::unix_time_as_millis;
use lru_cache::LruCache;
use std::cmp;
use std::collections::{btree_map::Entry, BTreeMap, HashMap, HashSet};
use std::fmt;
use std::hash::Hash;
use std::mem;
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
// 2 ** 13 < 6 * 1800 < 2 ** 14
const ONE_DAY_BLOCK_NUMBER: u64 = 8192;

// State used to enforce CHAIN_SYNC_TIMEOUT
// Only in effect for connections that are outbound, non-manual,
// non-protected and non-whitelist.
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

#[derive(Clone, Debug, Default)]
pub struct ChainSyncState {
    pub timeout: u64,
    pub work_header: Option<core::HeaderView>,
    pub total_difficulty: Option<U256>,
    pub sent_getheaders: bool,
    pub not_sync_until: Option<u64>,
}

#[derive(Clone, Default, Debug, Copy)]
pub struct PeerFlags {
    pub is_outbound: bool,
    pub is_protect: bool,
    pub is_whitelist: bool,
}

#[derive(Clone, Default, Debug)]
pub struct PeerState {
    // only use on header sync
    pub sync_started: bool,
    pub headers_sync_timeout: Option<u64>,
    pub peer_flags: PeerFlags,
    pub disconnect: bool,
    pub chain_sync: ChainSyncState,
    // The key is a `timeout`, means do not ask the tx before `timeout`.
    tx_ask_for_map: BTreeMap<Instant, Vec<Byte32>>,
    tx_ask_for_set: HashSet<Byte32>,

    pub best_known_header: Option<HeaderView>,
    pub last_common_header: Option<core::HeaderView>,
    // use on ibd concurrent block download
    // save `get_headers` locator hashes here
    pub unknown_header_list: Vec<Byte32>,
}

impl PeerState {
    pub fn new(peer_flags: PeerFlags) -> PeerState {
        PeerState {
            sync_started: false,
            headers_sync_timeout: None,
            peer_flags,
            disconnect: false,
            chain_sync: ChainSyncState::default(),
            tx_ask_for_map: BTreeMap::default(),
            tx_ask_for_set: HashSet::new(),
            best_known_header: None,
            last_common_header: None,
            unknown_header_list: Vec::new(),
        }
    }

    pub fn can_sync(&self, now: u64, ibd: bool) -> bool {
        // only sync with protect/whitelist peer in IBD
        ((self.peer_flags.is_protect || self.peer_flags.is_whitelist) || !ibd)
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

    pub fn suspend_sync(&mut self, suspend_time: u64) {
        let now = unix_time_as_millis();
        self.sync_started = false;
        self.chain_sync.not_sync_until = Some(now + suspend_time);
        self.headers_sync_timeout = None;
    }

    // Not use yet
    pub fn caught_up_sync(&mut self) {
        self.headers_sync_timeout = Some(std::u64::MAX);
    }

    pub fn add_ask_for_tx(
        &mut self,
        tx_hash: Byte32,
        last_ask_timeout: Option<Instant>,
    ) -> Option<Instant> {
        if self.tx_ask_for_map.len() > MAX_ASK_MAP_SIZE {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "this peer tx_ask_for_map is full, ignore {}",
                tx_hash
            );
            return None;
        }
        if self.tx_ask_for_set.len() > MAX_ASK_SET_SIZE {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "this peer tx_ask_for_set is full, ignore {}",
                tx_hash
            );
            return None;
        }
        // This peer already register asked for this tx
        if self.tx_ask_for_set.contains(&tx_hash) {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "this peer already register ask tx({})",
                tx_hash
            );
            return None;
        }

        // Retry ask tx `RETRY_ASK_TX_TIMEOUT_INCREASE` later
        //  NOTE: last_ask_timeout is some when other peer already asked for this tx_hash
        let next_ask_timeout = last_ask_timeout
            .map(|time| cmp::max(time + RETRY_ASK_TX_TIMEOUT_INCREASE, Instant::now()))
            .unwrap_or_else(Instant::now);
        self.tx_ask_for_map
            .entry(next_ask_timeout)
            .or_default()
            .push(tx_hash.clone());
        self.tx_ask_for_set.insert(tx_hash);
        Some(next_ask_timeout)
    }

    pub fn remove_ask_for_tx(&mut self, tx_hash: &Byte32) {
        self.tx_ask_for_set.remove(tx_hash);
    }

    pub fn pop_ask_for_txs(&mut self) -> Vec<Byte32> {
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
    inner: HashMap<PeerIndex, Filter<Byte32>>,
}

impl KnownFilter {
    /// Adds a value to the filter.
    /// If the filter did not have this value present, `true` is returned.
    /// If the filter did have this value present, `false` is returned.
    pub fn insert(&mut self, index: PeerIndex, hash: Byte32) -> bool {
        self.inner
            .entry(index)
            .or_insert_with(|| Filter::new(FILTER_SIZE))
            .insert(hash)
    }
}

#[derive(Default)]
pub struct Peers {
    pub state: RwLock<HashMap<PeerIndex, PeerState>>,
}

#[derive(Debug, Clone)]
pub struct InflightState {
    pub(crate) peer: PeerIndex,
    pub(crate) timestamp: u64,
}

impl InflightState {
    fn new(peer: PeerIndex) -> Self {
        Self {
            peer,
            timestamp: unix_time_as_millis(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockNumberAndHash {
    pub number: BlockNumber,
    pub hash: Byte32,
}

impl From<(BlockNumber, Byte32)> for BlockNumberAndHash {
    fn from(inner: (BlockNumber, Byte32)) -> Self {
        Self {
            number: inner.0,
            hash: inner.1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DownloadScheduler {
    task_count: usize,
    timeout_count: usize,
    breakthroughs_count: usize,
    hashes: HashSet<BlockNumberAndHash>,
}

impl Default for DownloadScheduler {
    fn default() -> Self {
        Self {
            hashes: HashSet::default(),
            task_count: INIT_BLOCKS_IN_TRANSIT_PER_PEER,
            breakthroughs_count: 0,
            timeout_count: 0,
        }
    }
}

impl DownloadScheduler {
    fn inflight_count(&self) -> usize {
        self.hashes.len()
    }

    fn can_fetch(&self) -> usize {
        self.task_count.saturating_sub(self.hashes.len())
    }

    pub(crate) fn task_count(&self) -> usize {
        self.task_count
    }

    fn adjust(&mut self, time: u64, len: u64) {
        let now = unix_time_as_millis();
        // 8 means default max outbound
        // All synchronization tests are based on the assumption of 8 nodes.
        // If the number of nodes is increased, the number of requests and processing time will increase,
        // and the corresponding adjustment criteria are needed, so the adjustment is based on 8.
        //
        // note: This is an interim scenario and the next step is to consider the median response time
        // within a certain interval as the adjustment criterion
        let (quotient, remainder) = if len > 8 { (len >> 3, len % 8) } else { (1, 0) };
        // Dynamically adjust download tasks based on response time.
        //
        // Block max size about 700k, Under 10m/s bandwidth it may cost 1s to response
        // But the mark is on the same time, so multiple tasks may be affected by one task,
        // causing all responses to be in the range of reduced tasks.
        // So the adjustment to reduce the number of tasks will be calculated by modulo 3 with the actual number of triggers.
        // Adjust for each block received.
        match ((now - time) / quotient).saturating_sub(remainder * 100) {
            // Within 500ms of response, considered better to communicate with that node's network
            0..=500 => {
                if self.task_count < FIRST_LEVEL_MAX - 1 {
                    self.task_count += 2
                } else {
                    self.breakthroughs_count += 1
                }
            }
            // Within 1000ms of response time, the network is relatively good and communication should be maintained
            501..=1000 => {
                if self.task_count < FIRST_LEVEL_MAX {
                    self.task_count += 1
                } else {
                    self.breakthroughs_count += 1
                }
            }
            // Response time within 1500ms, acceptable range, but not relatively good for nodes and do not adjust
            1001..=1500 => (),
            // Above 1500ms, relatively poor network, reduced
            _ => {
                self.timeout_count += 1;
                if self.timeout_count > 3 {
                    self.task_count = self.task_count.saturating_sub(1);
                    self.timeout_count = 0;
                }
            }
        }
        if self.breakthroughs_count > 4 {
            if self.task_count < MAX_BLOCKS_IN_TRANSIT_PER_PEER {
                self.task_count += 1;
            }
            self.breakthroughs_count = 0;
        }
    }

    fn punish(&mut self) {
        self.task_count >>= 1
    }
}

#[derive(Clone)]
pub struct InflightBlocks {
    pub(crate) download_schedulers: HashMap<PeerIndex, DownloadScheduler>,
    inflight_states: BTreeMap<BlockNumberAndHash, InflightState>,
    pub(crate) trace_number: HashMap<BlockNumberAndHash, u64>,
    compact_reconstruct_inflight: HashMap<Byte32, HashSet<PeerIndex>>,
    pub(crate) restart_number: BlockNumber,
    pub(crate) adjustment: bool,
}

impl Default for InflightBlocks {
    fn default() -> Self {
        InflightBlocks {
            download_schedulers: HashMap::default(),
            inflight_states: BTreeMap::default(),
            trace_number: HashMap::default(),
            compact_reconstruct_inflight: HashMap::default(),
            restart_number: 0,
            adjustment: true,
        }
    }
}

struct DebugHashSet<'a>(&'a HashSet<BlockNumberAndHash>);

impl<'a> fmt::Debug for DebugHashSet<'a> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_set()
            .entries(self.0.iter().map(|h| format!("{}", h.hash)))
            .finish()
    }
}

impl fmt::Debug for InflightBlocks {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_map()
            .entries(
                self.download_schedulers
                    .iter()
                    .map(|(k, v)| (k, DebugHashSet(&v.hashes))),
            )
            .finish()?;
        fmt.debug_map()
            .entries(
                self.inflight_states
                    .iter()
                    .map(|(k, v)| (format!("{}", k.hash), v)),
            )
            .finish()
    }
}

impl InflightBlocks {
    pub fn blocks_iter(&self) -> impl Iterator<Item = (&PeerIndex, &HashSet<BlockNumberAndHash>)> {
        self.download_schedulers.iter().map(|(k, v)| (k, &v.hashes))
    }

    pub fn total_inflight_count(&self) -> usize {
        self.inflight_states.len()
    }

    pub fn peer_inflight_count(&self, peer: PeerIndex) -> usize {
        self.download_schedulers
            .get(&peer)
            .map(DownloadScheduler::inflight_count)
            .unwrap_or(0)
    }

    pub fn peer_can_fetch_count(&self, peer: PeerIndex) -> usize {
        self.download_schedulers.get(&peer).map_or(
            INIT_BLOCKS_IN_TRANSIT_PER_PEER,
            DownloadScheduler::can_fetch,
        )
    }

    pub fn inflight_block_by_peer(&self, peer: PeerIndex) -> Option<&HashSet<BlockNumberAndHash>> {
        self.download_schedulers.get(&peer).map(|d| &d.hashes)
    }

    pub fn inflight_state_by_block(&self, block: &BlockNumberAndHash) -> Option<&InflightState> {
        self.inflight_states.get(block)
    }

    pub fn compact_reconstruct(&mut self, peer: PeerIndex, hash: Byte32) -> bool {
        let entry = self.compact_reconstruct_inflight.entry(hash).or_default();
        if entry.len() >= 2 {
            return false;
        }

        entry.insert(peer)
    }

    pub fn remove_compact(&mut self, peer: PeerIndex, hash: &Byte32) {
        self.compact_reconstruct_inflight
            .get_mut(&hash)
            .map(|peers| peers.remove(&peer));
        self.trace_number.retain(|k, _| &k.hash != hash)
    }

    pub fn inflight_compact_by_block(&self, hash: &Byte32) -> Option<&HashSet<PeerIndex>> {
        self.compact_reconstruct_inflight.get(hash)
    }

    pub fn mark_slow_block(&mut self, tip: BlockNumber) {
        let now = faketime::unix_time_as_millis();
        for key in self.inflight_states.keys() {
            if key.number > tip + 1 {
                break;
            }
            self.trace_number.entry(key.clone()).or_insert(now);
        }
    }

    pub fn prune(&mut self, tip: BlockNumber) -> HashSet<PeerIndex> {
        let now = unix_time_as_millis();
        let prev_count = self.total_inflight_count();
        let mut disconnect_list = HashSet::new();

        let trace = &mut self.trace_number;
        let download_schedulers = &mut self.download_schedulers;
        let states = &mut self.inflight_states;
        let compact_inflight = &mut self.compact_reconstruct_inflight;

        let mut remove_key = Vec::new();
        // Since this is a btreemap, with the data already sorted,
        // we don't have to worry about missing points, and we don't need to
        // iterate through all the data each time, just check within tip + 20,
        // with the checkpoint marking possible blocking points, it's enough
        let end = tip + 20;
        for (key, value) in states.iter() {
            if key.number > end {
                break;
            }
            if value.timestamp + BLOCK_DOWNLOAD_TIMEOUT < now {
                download_schedulers
                    .get_mut(&value.peer)
                    .map(|set| set.hashes.remove(key));
                if !trace.is_empty() {
                    trace.remove(&key);
                }
                disconnect_list.insert(value.peer);
                remove_key.push(key.clone());
            }
        }

        for key in remove_key {
            states.remove(&key);
        }

        download_schedulers.retain(|k, v| {
            // task number zero means this peer's response is very slow
            if v.task_count == 0 {
                disconnect_list.insert(*k);
                false
            } else {
                true
            }
        });

        if self.restart_number != 0 && tip + 1 > self.restart_number {
            self.restart_number = 0;
        }

        let restart_number = &mut self.restart_number;
        trace.retain(|key, time| {
            // In the normal state, trace will always empty
            //
            // When the inflight request reaches the checkpoint(inflight > tip + 512),
            // it means that there is an anomaly in the sync less than tip + 1, i.e. some nodes are stuck,
            // at which point it will be recorded as the timestamp at that time.
            //
            // If the time exceeds 1s, delete the task and halve the number of
            // executable tasks for the corresponding node
            if now > 1000 + *time {
                if let Some(state) = states.remove(key) {
                    if let Some(d) = download_schedulers.get_mut(&state.peer) {
                        d.punish();
                        d.hashes.remove(key);
                    };
                } else if let Some(v) = compact_inflight.remove(&key.hash) {
                    for peer in v {
                        if let Some(d) = download_schedulers.get_mut(&peer) {
                            d.punish();
                        }
                    }
                }
                if key.number > *restart_number {
                    *restart_number = key.number;
                }
                return false;
            }
            true
        });

        if prev_count == 0 {
            metric!({
                "topic": "blocks_in_flight",
                "fields": { "total": 0, "elapsed": 0 }
            });
        } else if prev_count != self.total_inflight_count() {
            metric!({
                "topic": "blocks_in_flight",
                "fields": { "total": self.total_inflight_count(), "elapsed": BLOCK_DOWNLOAD_TIMEOUT }
            });
        }

        disconnect_list
    }

    pub fn insert(&mut self, peer: PeerIndex, block: BlockNumberAndHash) -> bool {
        if !self.compact_reconstruct_inflight.is_empty()
            && self.compact_reconstruct_inflight.contains_key(&block.hash)
        {
            // Give the compact block a deadline of 1.5 seconds
            self.trace_number
                .entry(block)
                .or_insert(unix_time_as_millis() + 500);
            return false;
        }
        let state = self.inflight_states.entry(block.clone());
        match state {
            Entry::Occupied(_entry) => return false,
            Entry::Vacant(entry) => entry.insert(InflightState::new(peer)),
        };

        if self.restart_number >= block.number {
            // All new requests smaller than restart_number mean that they are cleaned up and
            // cannot be immediately marked as cleaned up again, so give it a normal response time of 1.5s.
            // (timeout check is 1s, plus 0.5s given in advance)
            self.trace_number
                .insert(block.clone(), unix_time_as_millis() + 500);
        }

        let download_scheduler = self
            .download_schedulers
            .entry(peer)
            .or_insert_with(DownloadScheduler::default);
        download_scheduler.hashes.insert(block)
    }

    pub fn remove_by_peer(&mut self, peer: PeerIndex) -> bool {
        let trace = &mut self.trace_number;
        let state = &mut self.inflight_states;
        let compact = &mut self.compact_reconstruct_inflight;
        self.download_schedulers
            .remove(&peer)
            .map(|blocks| {
                for block in blocks.hashes {
                    if !compact.is_empty() {
                        compact
                            .get_mut(&block.hash)
                            .map(|peers| peers.remove(&peer));
                    }
                    state.remove(&block);
                    if !trace.is_empty() {
                        trace.remove(&block);
                    }
                }
            })
            .is_some()
    }

    pub fn remove_by_block(&mut self, block: BlockNumberAndHash) -> bool {
        let download_schedulers = &mut self.download_schedulers;
        let trace = &mut self.trace_number;
        let compact = &mut self.compact_reconstruct_inflight;
        let len = download_schedulers.len() as u64;
        let adjustment = self.adjustment;
        self.inflight_states
            .remove(&block)
            .map(|state| {
                if let Some(set) = download_schedulers.get_mut(&state.peer) {
                    set.hashes.remove(&block);
                    if !compact.is_empty() {
                        compact.remove(&block.hash);
                    }
                    if adjustment {
                        set.adjust(state.timestamp, len);
                    }
                    if !trace.is_empty() {
                        trace.remove(&block);
                    }
                };
                state.timestamp
            })
            .map(|timestamp| {
                let elapsed = unix_time_as_millis().saturating_sub(timestamp);
                metric!({
                    "topic": "blocks_in_flight",
                    "fields": { "total": self.total_inflight_count(), "elapsed": elapsed }
                });
            })
            .is_some()
    }
}

impl Peers {
    pub fn on_connected(&self, peer: PeerIndex, peer_flags: PeerFlags) {
        self.state
            .write()
            .entry(peer)
            .and_modify(|state| {
                state.peer_flags = peer_flags;
            })
            .or_insert_with(|| PeerState::new(peer_flags));
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

    pub fn get_last_common_header(&self, pi: PeerIndex) -> Option<core::HeaderView> {
        self.state
            .read()
            .get(&pi)
            .and_then(|peer_state| peer_state.last_common_header.clone())
    }

    pub fn set_last_common_header(&self, pi: PeerIndex, header: core::HeaderView) {
        self.state
            .write()
            .entry(pi)
            .and_modify(|peer_state| peer_state.last_common_header = Some(header));
    }

    pub fn new_header_received(&self, peer: PeerIndex, header_view: &HeaderView) {
        if let Some(peer_state) = self.state.write().get_mut(&peer) {
            if let Some(ref hv) = peer_state.best_known_header {
                if header_view.is_better_than(&hv.total_difficulty()) {
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

    pub fn insert_unknown_header_hash(&self, peer: PeerIndex, hash: Byte32) {
        self.state
            .write()
            .entry(peer)
            .and_modify(|state| state.unknown_header_list.push(hash));
    }

    pub fn clear_unknown_list(&self) {
        self.state.write().values_mut().for_each(|state| {
            if !state.unknown_header_list.is_empty() {
                state.unknown_header_list.clear()
            }
        })
    }

    pub fn get_best_known_less_than_tip_and_unknown_empty(
        &self,
        tip: BlockNumber,
    ) -> Vec<PeerIndex> {
        self.state
            .read()
            .iter()
            .filter_map(|(pi, state)| {
                if !state.unknown_header_list.is_empty() {
                    return None;
                }
                match state.best_known_header {
                    Some(ref header) if header.number() < tip => Some(*pi),
                    _ => None,
                }
            })
            .collect()
    }

    pub fn take_unknown_last(&self, peer: PeerIndex) -> Option<Byte32> {
        self.state
            .write()
            .get_mut(&peer)
            .and_then(|state| state.unknown_header_list.pop())
    }

    pub fn get_flag(&self, peer: PeerIndex) -> Option<PeerFlags> {
        self.state.read().get(&peer).map(|state| state.peer_flags)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderView {
    inner: core::HeaderView,
    total_difficulty: U256,
    // pointer to the index of some further predecessor of this block
    skip_hash: Option<Byte32>,
}

impl HeaderView {
    pub fn new(inner: core::HeaderView, total_difficulty: U256) -> Self {
        HeaderView {
            inner,
            total_difficulty,
            skip_hash: None,
        }
    }

    pub fn number(&self) -> BlockNumber {
        self.inner.number()
    }

    pub fn hash(&self) -> Byte32 {
        self.inner.hash()
    }

    pub fn parent_hash(&self) -> Byte32 {
        self.inner.data().raw().parent_hash()
    }

    pub fn timestamp(&self) -> u64 {
        self.inner.timestamp()
    }

    pub fn total_difficulty(&self) -> &U256 {
        &self.total_difficulty
    }

    pub fn inner(&self) -> &core::HeaderView {
        &self.inner
    }

    pub fn into_inner(self) -> core::HeaderView {
        self.inner
    }

    pub fn build_skip<F, G>(&mut self, mut get_header_view: F, fast_scanner: G)
    where
        F: FnMut(&Byte32) -> Option<HeaderView>,
        G: Fn(BlockNumber, &HeaderView) -> Option<HeaderView>,
    {
        self.skip_hash = get_header_view(&self.parent_hash())
            .and_then(|parent| {
                parent.get_ancestor(
                    get_skip_height(self.number()),
                    get_header_view,
                    fast_scanner,
                )
            })
            .map(|header| header.hash());
    }

    // NOTE: get_header_view may change source state, for cache or for tests
    pub fn get_ancestor<F, G>(
        self,
        number: BlockNumber,
        mut get_header_view: F,
        fast_scanner: G,
    ) -> Option<core::HeaderView>
    where
        F: FnMut(&Byte32) -> Option<HeaderView>,
        G: Fn(BlockNumber, &HeaderView) -> Option<HeaderView>,
    {
        let timestamp = unix_time_as_millis();
        let mut current = self;
        if number > current.number() {
            return None;
        }

        let base_number = current.number();
        let mut steps = 0u64;
        let mut number_walk = current.number();
        while number_walk > number {
            let number_skip = get_skip_height(number_walk);
            let number_skip_prev = get_skip_height(number_walk - 1);
            steps += 1;
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
                    current = get_header_view(&current.parent_hash())?;
                    number_walk -= 1;
                }
            }
            if let Some(target) = fast_scanner(number, &current) {
                current = target;
                break;
            }
        }
        metric!({
            "topic": "get_ancestor",
            "fields": {
                "steps": steps,
                "elapsed": unix_time_as_millis().saturating_sub(timestamp),
                "base_number": base_number,
                "target_number": number,
                "ancestor_number": current.number()
            },
        });
        Some(current).map(HeaderView::into_inner)
    }

    pub fn is_better_than(&self, total_difficulty: &U256) -> bool {
        self.total_difficulty() > total_difficulty
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

// <CompactBlockHash, (CompactBlock, <PeerIndex, (TransactionsIndex, UnclesIndex)>)>
type PendingCompactBlockMap = HashMap<
    Byte32,
    (
        packed::CompactBlock,
        HashMap<PeerIndex, (Vec<u32>, Vec<u32>)>,
    ),
>;

#[derive(Clone)]
pub struct SyncShared {
    shared: Shared,
    state: Arc<SyncState>,
}

impl SyncShared {
    pub fn new(shared: Shared) -> SyncShared {
        let (total_difficulty, header) = {
            let snapshot = shared.snapshot();
            (
                snapshot.total_difficulty().to_owned(),
                snapshot.tip_header().to_owned(),
            )
        };
        let shared_best_header = RwLock::new(HeaderView::new(header, total_difficulty));

        let state = SyncState {
            n_sync_started: AtomicUsize::new(0),
            n_protected_outbound_peers: AtomicUsize::new(0),
            ibd_finished: AtomicBool::new(false),
            shared_best_header,
            header_map: RwLock::new(HashMap::new()),
            block_status_map: Mutex::new(HashMap::new()),
            tx_filter: Mutex::new(Filter::new(TX_FILTER_SIZE)),
            peers: Peers::default(),
            misbehavior: RwLock::new(HashMap::default()),
            known_txs: Mutex::new(KnownFilter::default()),
            pending_get_block_proposals: Mutex::new(HashMap::default()),
            pending_compact_blocks: Mutex::new(HashMap::default()),
            orphan_block_pool: OrphanBlockPool::with_capacity(ORPHAN_BLOCK_SIZE),
            inflight_proposals: Mutex::new(HashSet::default()),
            inflight_transactions: Mutex::new(LruCache::new(TX_ASKED_SIZE)),
            inflight_blocks: RwLock::new(InflightBlocks::default()),
            pending_get_headers: RwLock::new(LruCache::new(GET_HEADERS_CACHE_SIZE)),
            tx_hashes: Mutex::new(HashMap::default()),
        };

        SyncShared {
            shared,
            state: Arc::new(state),
        }
    }

    pub fn shared(&self) -> &Shared {
        &self.shared
    }

    pub fn active_chain(&self) -> ActiveChain {
        ActiveChain {
            shared: self.clone(),
            snapshot: Arc::clone(&self.shared.snapshot()),
            state: Arc::clone(&self.state),
        }
    }

    pub fn store(&self) -> &ChainDB {
        self.shared.store()
    }

    pub fn state(&self) -> &SyncState {
        &self.state
    }

    pub fn consensus(&self) -> &Consensus {
        self.shared.consensus()
    }

    pub fn insert_new_block(
        &self,
        chain: &ChainController,
        pi: PeerIndex,
        block: Arc<core::BlockView>,
    ) -> Result<bool, FailureError> {
        // Insert the given block into orphan_block_pool if its parent is not found
        if !self.is_parent_stored(&block) {
            debug!(
                "insert new orphan block {} {}",
                block.header().number(),
                block.header().hash()
            );
            self.state.insert_orphan_block(pi, (*block).clone());
            return Ok(false);
        }

        // Attempt to accept the given block if its parent already exist in database
        let ret = self.accept_block(chain, pi, Arc::clone(&block));
        if ret.is_err() {
            debug!("accept block {:?} {:?}", block, ret);
            return ret;
        }

        // The above block has been accepted. Attempt to accept its descendant blocks in orphan pool.
        // The returned blocks of `remove_blocks_by_parent` are in topology order by parents
        let descendants = self.state.remove_orphan_by_parent(&block.as_ref().hash());
        for (peer, block) in descendants {
            // If we can not find the block's parent in database, that means it was failed to accept
            // its parent, so we treat it as an invalid block as well.
            if !self.is_parent_stored(&block) {
                debug!(
                    "parent-unknown orphan block, block: {}, {}, parent: {}",
                    block.header().number(),
                    block.header().hash(),
                    block.header().parent_hash(),
                );
                continue;
            }

            let block = Arc::new(block);
            if let Err(err) = self.accept_block(chain, peer, Arc::clone(&block)) {
                debug!(
                    "accept descendant orphan block {} error {:?}",
                    block.header().hash(),
                    err
                );
            }
        }
        ret
    }

    fn accept_block(
        &self,
        chain: &ChainController,
        peer: PeerIndex,
        block: Arc<core::BlockView>,
    ) -> Result<bool, FailureError> {
        let ret = chain.process_block(Arc::clone(&block));
        if ret.is_err() {
            error!("accept block {:?} {:?}", block, ret);
            self.state
                .insert_block_status(block.header().hash(), BlockStatus::BLOCK_INVALID);
        } else {
            // Clear the newly inserted block from block_status_map.
            //
            // We don't know whether the actual block status is BLOCK_VALID or BLOCK_INVALID.
            // So we just simply remove the corresponding in-memory block status,
            // and the next time `get_block_status` would acquire the real-time
            // status via fetching block_ext from the database.
            self.state.remove_block_status(&block.as_ref().hash());
            self.state.remove_header_view(&block.as_ref().hash());
            self.state
                .peers()
                .set_last_common_header(peer, block.header());
        }

        Ok(ret?)
    }

    // Update the header_map
    // Update the block_status_map
    // Update the shared_best_header if need
    // Update the peer's best_known_header
    pub fn insert_valid_header(&self, peer: PeerIndex, header: &core::HeaderView) {
        let parent_view = self
            .get_header_view(&header.data().raw().parent_hash())
            .expect("parent should be verified");
        let mut header_view = {
            let total_difficulty = parent_view.total_difficulty() + header.difficulty();
            HeaderView::new(header.clone(), total_difficulty)
        };

        let snapshot = Arc::clone(&self.shared.snapshot());
        header_view.build_skip(
            |hash| self.get_header_view(hash),
            |number, current| {
                // shortcut to return an ancestor block
                if current.number() <= snapshot.tip_number()
                    && snapshot.is_main_chain(&current.hash())
                {
                    snapshot
                        .get_block_hash(number)
                        .and_then(|hash| self.get_header_view(&hash))
                } else {
                    None
                }
            },
        );
        self.state
            .header_map
            .write()
            .insert(header.hash(), header_view.clone());
        self.state
            .insert_block_status(header.hash(), BlockStatus::HEADER_VALID);

        // NOTE: Must update best headers(peers/global) after update header_map, otherwise will have
        //   multiple threads inconsistent bug.

        // Update shared_best_header if the arrived header has greater difficulty
        let shared_best_header = self.state().shared_best_header();
        if header_view.is_better_than(&shared_best_header.total_difficulty()) {
            self.state.set_shared_best_header(header_view.clone());
        }
        self.state.peers().new_header_received(peer, &header_view);
    }

    pub fn get_header_view(&self, hash: &Byte32) -> Option<HeaderView> {
        let store = self.store();
        self.state.header_map.read().get(hash).cloned().or_else(|| {
            store.get_block_header(hash).and_then(|header| {
                store
                    .get_block_ext(&hash)
                    .map(|block_ext| HeaderView::new(header, block_ext.total_difficulty))
            })
        })
    }

    pub fn get_header(&self, hash: &Byte32) -> Option<core::HeaderView> {
        self.state
            .header_map
            .read()
            .get(hash)
            .map(HeaderView::inner)
            .cloned()
            .or_else(|| self.store().get_block_header(hash))
    }

    pub fn is_parent_stored(&self, block: &core::BlockView) -> bool {
        self.store()
            .get_block_header(&block.data().header().raw().parent_hash())
            .is_some()
    }

    pub fn get_epoch_ext(&self, hash: &Byte32) -> Option<EpochExt> {
        self.store().get_block_epoch(&hash)
    }

    pub(crate) fn new_header_resolver<'a>(
        &'a self,
        header: &'a core::HeaderView,
        parent: core::HeaderView,
    ) -> HeaderResolverWrapper<'a> {
        HeaderResolverWrapper::build(header, Some(parent))
    }
}

pub struct SyncState {
    n_sync_started: AtomicUsize,
    n_protected_outbound_peers: AtomicUsize,
    ibd_finished: AtomicBool,

    /* Status irrelevant to peers */
    shared_best_header: RwLock<HeaderView>,
    header_map: RwLock<HashMap<Byte32, HeaderView>>,
    block_status_map: Mutex<HashMap<Byte32, BlockStatus>>,
    tx_filter: Mutex<Filter<Byte32>>,

    /* Status relevant to peers */
    peers: Peers,
    misbehavior: RwLock<HashMap<PeerIndex, u32>>,
    known_txs: Mutex<KnownFilter>,

    /* Cached items which we had received but not completely process */
    pending_get_block_proposals: Mutex<HashMap<packed::ProposalShortId, HashSet<PeerIndex>>>,
    pending_get_headers: RwLock<LruCache<(PeerIndex, Byte32), Instant>>,
    pending_compact_blocks: Mutex<PendingCompactBlockMap>,
    orphan_block_pool: OrphanBlockPool,

    /* In-flight items for which we request to peers, but not got the responses yet */
    inflight_proposals: Mutex<HashSet<packed::ProposalShortId>>,
    inflight_transactions: Mutex<LruCache<Byte32, Instant>>,
    inflight_blocks: RwLock<InflightBlocks>,

    /* cached for sending bulk */
    tx_hashes: Mutex<HashMap<PeerIndex, LinkedHashSet<Byte32>>>,
}

impl SyncState {
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

    pub fn known_txs(&self) -> MutexGuard<KnownFilter> {
        self.known_txs.lock()
    }

    pub fn pending_compact_blocks(&self) -> MutexGuard<PendingCompactBlockMap> {
        self.pending_compact_blocks.lock()
    }

    pub fn inflight_transactions(&self) -> MutexGuard<LruCache<Byte32, Instant>> {
        self.inflight_transactions.lock()
    }

    pub fn read_inflight_blocks(&self) -> RwLockReadGuard<InflightBlocks> {
        self.inflight_blocks.read()
    }

    pub fn write_inflight_blocks(&self) -> RwLockWriteGuard<InflightBlocks> {
        self.inflight_blocks.write()
    }

    pub fn inflight_proposals(&self) -> MutexGuard<HashSet<packed::ProposalShortId>> {
        self.inflight_proposals.lock()
    }

    pub fn tx_hashes(&self) -> MutexGuard<HashMap<PeerIndex, LinkedHashSet<Byte32>>> {
        self.tx_hashes.lock()
    }

    pub fn take_tx_hashes(&self) -> HashMap<PeerIndex, LinkedHashSet<Byte32>> {
        let mut map = self.tx_hashes.lock();
        mem::replace(&mut *map, HashMap::default())
    }

    pub fn is_initial_header_sync(&self) -> bool {
        unix_time_as_millis().saturating_sub(self.shared_best_header().timestamp()) > MAX_TIP_AGE
    }

    pub fn shared_best_header(&self) -> HeaderView {
        self.shared_best_header.read().to_owned()
    }

    pub fn set_shared_best_header(&self, header: HeaderView) {
        assert!(
            self.header_map.read().contains_key(&header.hash()),
            "HeaderView must exists in header_map before set best header"
        );
        metric!({
            "topic": "chain",
            "fields": { "header_chain_tip": header.number(), }
        });
        *self.shared_best_header.write() = header;
    }

    pub fn remove_header_view(&self, hash: &Byte32) {
        self.header_map.write().remove(hash);
    }

    pub(crate) fn suspend_sync(&self, peer_state: &mut PeerState) {
        peer_state.suspend_sync(SUSPEND_SYNC_TIME);
        assert_ne!(
            self.n_sync_started().fetch_sub(1, Ordering::Release),
            0,
            "n_sync_started overflow when suspend_sync"
        );
    }

    pub fn mark_as_known_tx(&self, hash: Byte32) {
        self.mark_as_known_txs(vec![hash]);
    }

    pub fn mark_as_known_txs(&self, hashes: Vec<Byte32>) {
        {
            let mut inflight_transactions = self.inflight_transactions.lock();
            for hash in hashes.iter() {
                inflight_transactions.remove(&hash);
            }
        }

        let mut tx_filter = self.tx_filter.lock();

        for hash in hashes {
            tx_filter.insert(hash);
        }
    }

    pub fn already_known_tx(&self, hash: &Byte32) -> bool {
        self.tx_filter.lock().contains(hash)
    }

    pub fn tx_filter(&self) -> MutexGuard<Filter<Byte32>> {
        self.tx_filter.lock()
    }

    // Return true when the block is that we have requested and received first time.
    pub fn new_block_received(&self, block: &core::BlockView) -> bool {
        self.write_inflight_blocks()
            .remove_by_block((block.number(), block.hash()).into())
    }

    pub fn insert_inflight_proposals(&self, ids: Vec<packed::ProposalShortId>) -> Vec<bool> {
        let mut locked = self.inflight_proposals.lock();
        ids.into_iter().map(|id| locked.insert(id)).collect()
    }

    pub fn remove_inflight_proposals(&self, ids: &[packed::ProposalShortId]) -> Vec<bool> {
        let mut locked = self.inflight_proposals.lock();
        ids.iter().map(|id| locked.remove(id)).collect()
    }

    pub fn insert_orphan_block(&self, peer: PeerIndex, block: core::BlockView) {
        self.insert_block_status(block.hash(), BlockStatus::BLOCK_RECEIVED);
        self.orphan_block_pool.insert(peer, block);
    }

    pub fn remove_orphan_by_parent(
        &self,
        parent_hash: &Byte32,
    ) -> Vec<(PeerIndex, core::BlockView)> {
        let blocks = self.orphan_block_pool.remove_blocks_by_parent(parent_hash);
        let mut block_status_map = self.block_status_map.lock();
        blocks.iter().for_each(|(_, b)| {
            block_status_map.remove(&b.hash());
        });
        blocks
    }

    pub fn insert_block_status(&self, block_hash: Byte32, status: BlockStatus) {
        self.block_status_map.lock().insert(block_hash, status);
    }

    pub fn remove_block_status(&self, block_hash: &Byte32) {
        self.block_status_map.lock().remove(block_hash);
    }

    pub fn clear_get_block_proposals(
        &self,
    ) -> HashMap<packed::ProposalShortId, HashSet<PeerIndex>> {
        let mut locked = self.pending_get_block_proposals.lock();
        let old = locked.deref_mut();
        let mut ret = HashMap::default();
        mem::swap(old, &mut ret);
        ret
    }

    pub fn insert_get_block_proposals(&self, pi: PeerIndex, ids: Vec<packed::ProposalShortId>) {
        let mut locked = self.pending_get_block_proposals.lock();
        for id in ids.into_iter() {
            locked.entry(id).or_default().insert(pi);
        }
    }

    pub fn disconnected(&self, pi: PeerIndex) -> Option<PeerState> {
        self.known_txs().inner.remove(&pi);
        self.write_inflight_blocks().remove_by_peer(pi);
        self.peers().disconnected(pi)
    }

    pub fn get_orphan_block(&self, block_hash: &Byte32) -> Option<core::BlockView> {
        self.orphan_block_pool.get_block(block_hash)
    }

    pub fn insert_peer_unknown_header_list(&self, pi: PeerIndex, header_list: Vec<Byte32>) {
        // header list Is an ordered list, sorted from highest to lowest,
        // so here you discard and exit early
        for hash in header_list {
            if let Some(header) = self.header_map.read().get(&hash) {
                self.peers.new_header_received(pi, header);
                break;
            } else {
                self.peers.insert_unknown_header_hash(pi, hash)
            }
        }
    }

    pub fn try_update_best_known_with_unknown_header_list(&self, pi: PeerIndex) {
        // header list Is an ordered list, sorted from highest to lowest,
        // when header hash unknown, break loop is ok
        while let Some(hash) = self.peers().take_unknown_last(pi) {
            if let Some(header) = self.header_map.read().get(&hash) {
                self.peers.new_header_received(pi, header);
            } else {
                self.peers.insert_unknown_header_hash(pi, hash);
                break;
            }
        }
    }
}

/** ActiveChain captures a point-in-time view of indexed chain of blocks. */
#[derive(Clone)]
pub struct ActiveChain {
    shared: SyncShared,
    snapshot: Arc<Snapshot>,
    state: Arc<SyncState>,
}

impl ActiveChain {
    fn store(&self) -> &ChainDB {
        self.shared.store()
    }

    fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn get_block_hash(&self, number: BlockNumber) -> Option<packed::Byte32> {
        self.snapshot().get_block_hash(number)
    }

    pub fn get_block(&self, h: &packed::Byte32) -> Option<core::BlockView> {
        self.store().get_block(h)
    }

    pub fn get_block_header(&self, h: &packed::Byte32) -> Option<core::HeaderView> {
        self.store().get_block_header(h)
    }

    pub fn shared(&self) -> &SyncShared {
        &self.shared
    }

    pub fn total_difficulty(&self) -> &U256 {
        self.snapshot.total_difficulty()
    }

    pub fn tip_header(&self) -> core::HeaderView {
        self.snapshot.tip_header().clone()
    }

    pub fn tip_hash(&self) -> Byte32 {
        self.snapshot.tip_hash()
    }

    pub fn tip_number(&self) -> BlockNumber {
        self.snapshot.tip_number()
    }

    pub fn epoch_ext(&self) -> core::EpochExt {
        self.snapshot.epoch_ext().clone()
    }

    pub fn is_initial_block_download(&self) -> bool {
        // Once this function has returned false, it must remain false.
        if self.state.ibd_finished.load(Ordering::Relaxed) {
            false
        } else if unix_time_as_millis().saturating_sub(self.tip_header().timestamp()) > MAX_TIP_AGE
        {
            true
        } else {
            self.state.ibd_finished.store(true, Ordering::Relaxed);
            false
        }
    }

    pub fn get_ancestor(&self, base: &Byte32, number: BlockNumber) -> Option<core::HeaderView> {
        let tip_number = self.tip_number();
        self.shared.get_header_view(base)?.get_ancestor(
            number,
            |hash| self.shared.get_header_view(hash),
            |number, current| {
                // shortcut to return an ancestor block
                if current.number() <= tip_number && self.snapshot().is_main_chain(&current.hash())
                {
                    self.get_block_hash(number)
                        .and_then(|hash| self.shared.get_header_view(&hash))
                } else {
                    None
                }
            },
        )
    }

    pub fn get_locator(&self, start: &core::HeaderView) -> Vec<Byte32> {
        let mut step = 1;
        let mut locator = Vec::with_capacity(32);
        let mut index = start.number();
        let mut base = start.hash();
        loop {
            let header_hash = self
                .get_ancestor(&base, index)
                .unwrap_or_else(|| {
                    panic!(
                        "index calculated in get_locator: \
                         start: {}, base: {}, step: {}, locators({}): {:?}.",
                        start,
                        base,
                        step,
                        locator.len(),
                        locator,
                    )
                })
                .hash();
            locator.push(header_hash.clone());

            if locator.len() >= 10 {
                step <<= 1;
            }

            if index < step {
                // Insert some low-height blocks in the locator
                // to quickly start parallel ibd block downloads
                // and it should not be too much
                if locator.len() < 31 && index > ONE_DAY_BLOCK_NUMBER {
                    index >>= 1;
                    base = header_hash;
                    continue;
                }
                // always include genesis hash
                if index != 0 {
                    locator.push(self.shared.consensus().genesis_hash());
                }
                break;
            }
            index -= step;
            base = header_hash;
        }
        locator
    }

    // If the peer reorganized, our previous last_common_header may not be an ancestor
    // of its current best_known_header. Go back enough to fix that.
    pub fn last_common_ancestor(
        &self,
        last_common_header: &core::HeaderView,
        best_known_header: &core::HeaderView,
    ) -> Option<core::HeaderView> {
        debug_assert!(best_known_header.number() >= last_common_header.number());

        let mut m_right =
            self.get_ancestor(&best_known_header.hash(), last_common_header.number())?;

        if &m_right == last_common_header {
            return Some(m_right);
        }

        let mut m_left = self.shared.get_header(&last_common_header.hash())?;
        debug_assert!(m_right.number() == m_left.number());

        while m_left != m_right {
            m_left = self.get_ancestor(&m_left.hash(), m_left.number() - 1)?;
            m_right = self.get_ancestor(&m_right.hash(), m_right.number() - 1)?;
        }
        Some(m_left)
    }

    pub fn locate_latest_common_block(
        &self,
        _hash_stop: &Byte32,
        locator: &[Byte32],
    ) -> Option<BlockNumber> {
        if locator.is_empty() {
            return None;
        }

        let locator_hash = locator.last().expect("empty checked");
        if locator_hash != &self.shared.consensus().genesis_hash() {
            return None;
        }

        // iterator are lazy
        let (index, latest_common) = locator
            .iter()
            .enumerate()
            .map(|(index, hash)| (index, self.snapshot.get_block_number(hash)))
            .find(|(_index, number)| number.is_some())
            .expect("locator last checked");

        if index == 0 || latest_common == Some(0) {
            return latest_common;
        }

        if let Some(header) = locator
            .get(index - 1)
            .and_then(|hash| self.shared.store().get_block_header(hash))
        {
            let mut block_hash = header.data().raw().parent_hash();
            loop {
                let block_header = match self.shared.store().get_block_header(&block_hash) {
                    None => break latest_common,
                    Some(block_header) => block_header,
                };

                if let Some(block_number) = self.snapshot.get_block_number(&block_hash) {
                    return Some(block_number);
                }

                block_hash = block_header.data().raw().parent_hash();
            }
        } else {
            latest_common
        }
    }

    pub fn get_locator_response(
        &self,
        block_number: BlockNumber,
        hash_stop: &Byte32,
    ) -> Vec<core::HeaderView> {
        let tip_number = self.tip_header().number();
        let max_height = cmp::min(
            block_number + 1 + MAX_HEADERS_LEN as BlockNumber,
            tip_number + 1,
        );
        (block_number + 1..max_height)
            .filter_map(|block_number| self.snapshot.get_block_hash(block_number))
            .take_while(|block_hash| block_hash != hash_stop)
            .filter_map(|block_hash| self.shared.store().get_block_header(&block_hash))
            .collect()
    }

    pub fn send_getheaders_to_peer(
        &self,
        nc: &dyn CKBProtocolContext,
        peer: PeerIndex,
        header: &core::HeaderView,
    ) {
        if let Some(last_time) = self
            .state
            .pending_get_headers
            .write()
            .get_refresh(&(peer, header.hash()))
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
        self.state
            .pending_get_headers
            .write()
            .insert((peer, header.hash()), Instant::now());

        debug!(
            "send_getheaders_to_peer peer={}, hash={}",
            peer,
            header.hash()
        );
        let locator_hash = self.get_locator(header);
        let content = packed::GetHeaders::new_builder()
            .block_locator_hashes(locator_hash.pack())
            .hash_stop(packed::Byte32::zero())
            .build();
        let message = packed::SyncMessage::new_builder().set(content).build();
        let data = bytes::Bytes::from(message.as_slice().to_vec());
        if let Err(err) = nc.send_message(NetworkProtocol::SYNC.into(), peer, data) {
            debug!("synchronizer send get_headers error: {:?}", err);
        }
    }

    pub fn get_block_status(&self, block_hash: &Byte32) -> BlockStatus {
        let mut locked = self.state.block_status_map.lock();
        match locked.get(block_hash).cloned() {
            Some(status) => status,
            None => {
                let verified = self
                    .snapshot
                    .get_block_ext(block_hash)
                    .map(|block_ext| block_ext.verified);
                match verified {
                    None => BlockStatus::UNKNOWN,
                    // NOTE: Don't insert `BLOCK_STORED` inside `block_status_map`.
                    Some(None) => BlockStatus::BLOCK_STORED,
                    Some(Some(true)) => {
                        locked.insert(block_hash.clone(), BlockStatus::BLOCK_VALID);
                        BlockStatus::BLOCK_VALID
                    }
                    Some(Some(false)) => {
                        locked.insert(block_hash.clone(), BlockStatus::BLOCK_INVALID);
                        BlockStatus::BLOCK_INVALID
                    }
                }
            }
        }
    }

    pub fn contains_block_status(&self, block_hash: &Byte32, status: BlockStatus) -> bool {
        self.get_block_status(block_hash).contains(status)
    }

    pub fn unknown_block_status(&self, block_hash: &Byte32) -> bool {
        self.get_block_status(block_hash) == BlockStatus::UNKNOWN
    }
}

#[derive(Clone, Copy, Debug)]
pub enum IBDState {
    In,
    Out,
}

impl From<bool> for IBDState {
    fn from(src: bool) -> Self {
        if src {
            IBDState::In
        } else {
            IBDState::Out
        }
    }
}

impl Into<bool> for IBDState {
    fn into(self) -> bool {
        match self {
            IBDState::In => true,
            IBDState::Out => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::HeaderView;
    use ckb_types::{
        core::{BlockNumber, HeaderBuilder},
        packed::Byte32,
        prelude::*,
        U256,
    };
    use rand::{thread_rng, Rng};
    use std::collections::{BTreeMap, HashMap};

    const SKIPLIST_LENGTH: u64 = 10_000;

    #[test]
    fn test_get_ancestor_use_skip_list() {
        let mut header_map: HashMap<Byte32, HeaderView> = HashMap::default();
        let mut hashes: BTreeMap<BlockNumber, Byte32> = BTreeMap::default();

        let mut parent_hash = None;
        for number in 0..SKIPLIST_LENGTH {
            let mut header_builder = HeaderBuilder::default().number(number.pack());
            if let Some(parent_hash) = parent_hash.take() {
                header_builder = header_builder.parent_hash(parent_hash);
            }
            let header = header_builder.build();
            hashes.insert(number, header.hash());
            parent_hash = Some(header.hash());

            let mut view = HeaderView::new(header, U256::zero());
            view.build_skip(|hash| header_map.get(hash).cloned(), |_, _| None);
            header_map.insert(view.hash(), view);
        }

        for (number, hash) in &hashes {
            if *number > 0 {
                let skip_view = header_map
                    .get(hash)
                    .and_then(|view| header_map.get(view.skip_hash.as_ref().unwrap()))
                    .unwrap();
                assert_eq!(
                    Some(skip_view.hash()).as_ref(),
                    hashes.get(&skip_view.number())
                );
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
                .get_ancestor(
                    b,
                    |hash| {
                        count += 1;
                        header_map.get(hash).cloned()
                    },
                    |_, _| None,
                )
                .unwrap();

            // Search must finished in <limit> steps
            assert!(count <= limit);

            header
        };
        for _ in 0..100 {
            let from: u64 = rng.gen_range(0, SKIPLIST_LENGTH);
            let to: u64 = rng.gen_range(0, from + 1);
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
