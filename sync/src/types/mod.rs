use crate::block_status::BlockStatus;
use crate::orphan_block_pool::OrphanBlockPool;
use crate::utils::is_internal_db_error;
use crate::{Status, StatusCode, FAST_INDEX, LOW_INDEX, NORMAL_INDEX, TIME_TRACE_SIZE};
use ckb_app_config::SyncConfig;
use ckb_chain::chain::ChainController;
use ckb_chain_spec::consensus::Consensus;
use ckb_channel::Receiver;
use ckb_constant::sync::{
    BLOCK_DOWNLOAD_TIMEOUT, HEADERS_DOWNLOAD_HEADERS_PER_SECOND, HEADERS_DOWNLOAD_INSPECT_WINDOW,
    HEADERS_DOWNLOAD_TOLERABLE_BIAS_FOR_SINGLE_SAMPLE, INIT_BLOCKS_IN_TRANSIT_PER_PEER,
    MAX_BLOCKS_IN_TRANSIT_PER_PEER, MAX_HEADERS_LEN, MAX_OUTBOUND_PEERS_TO_PROTECT_FROM_DISCONNECT,
    MAX_UNKNOWN_TX_HASHES_SIZE, MAX_UNKNOWN_TX_HASHES_SIZE_PER_PEER, POW_INTERVAL,
    RETRY_ASK_TX_TIMEOUT_INCREASE, SUSPEND_SYNC_TIME,
};
use ckb_error::Error as CKBError;
use ckb_logger::{debug, error, trace};
use ckb_network::{CKBProtocolContext, PeerIndex, SupportProtocols};
use ckb_shared::{shared::Shared, Snapshot};
use ckb_store::{ChainDB, ChainStore};
use ckb_systemtime::unix_time_as_millis;
use ckb_traits::HeaderProvider;
use ckb_tx_pool::service::TxVerificationResult;
use ckb_types::{
    core::{self, BlockNumber, EpochExt},
    packed::{self, Byte32},
    prelude::*,
    H256, U256,
};
use ckb_util::{shrink_to_fit, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use ckb_verification_traits::Switch;
use dashmap::{self, DashMap};
use keyed_priority_queue::{self, KeyedPriorityQueue};
use lru::LruCache;
use std::collections::{btree_map::Entry, BTreeMap, HashMap, HashSet};
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::{cmp, fmt, iter};

mod header_map;

use crate::utils::send_message;
use ckb_types::core::EpochNumber;
pub use header_map::HeaderMap;

const GET_HEADERS_CACHE_SIZE: usize = 10000;
// TODO: Need discussed
const GET_HEADERS_TIMEOUT: Duration = Duration::from_secs(15);
const FILTER_SIZE: usize = 50000;
const ORPHAN_BLOCK_SIZE: usize = 1024;
// 2 ** 13 < 6 * 1800 < 2 ** 14
const ONE_DAY_BLOCK_NUMBER: u64 = 8192;
const SHRINK_THRESHOLD: usize = 300;
pub(crate) const FILTER_TTL: u64 = 4 * 60 * 60;

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
    headers_sync_state: HeadersSyncState,
}

impl ChainSyncState {
    fn can_start_sync(&self, now: u64) -> bool {
        match self.headers_sync_state {
            HeadersSyncState::Initialized => false,
            HeadersSyncState::SyncProtocolConnected => true,
            HeadersSyncState::Started => false,
            HeadersSyncState::Suspend(until) | HeadersSyncState::TipSynced(until) => until < now,
        }
    }

    fn connected(&mut self) {
        self.headers_sync_state = HeadersSyncState::SyncProtocolConnected;
    }

    fn start(&mut self) {
        self.headers_sync_state = HeadersSyncState::Started
    }

    fn suspend(&mut self, until: u64) {
        self.headers_sync_state = HeadersSyncState::Suspend(until)
    }

    fn tip_synced(&mut self) {
        let now = unix_time_as_millis();
        // use avg block interval: (MAX_BLOCK_INTERVAL + MIN_BLOCK_INTERVAL) / 2 = 28
        self.headers_sync_state = HeadersSyncState::TipSynced(now + 28000);
    }

    fn started(&self) -> bool {
        matches!(self.headers_sync_state, HeadersSyncState::Started)
    }

    fn started_or_tip_synced(&self) -> bool {
        matches!(
            self.headers_sync_state,
            HeadersSyncState::Started | HeadersSyncState::TipSynced(_)
        )
    }
}

#[derive(Clone, Debug)]
enum HeadersSyncState {
    Initialized,
    SyncProtocolConnected,
    Started,
    Suspend(u64), // suspend headers sync until this timestamp (milliseconds since unix epoch)
    TipSynced(u64), // already synced to the end, not as the sync target for the time being, until the pause time is exceeded
}

impl Default for HeadersSyncState {
    fn default() -> Self {
        HeadersSyncState::Initialized
    }
}

#[derive(Clone, Default, Debug, Copy)]
pub struct PeerFlags {
    pub is_outbound: bool,
    pub is_protect: bool,
    pub is_whitelist: bool,
}

#[derive(Clone, Default, Debug, Copy)]
pub struct HeadersSyncController {
    // The timestamp when sync started
    pub(crate) started_ts: u64,
    // The timestamp of better tip header when sync started
    pub(crate) started_tip_ts: u64,

    // The timestamp when the process last updated
    pub(crate) last_updated_ts: u64,
    // The timestamp of better tip header when the process last updated
    pub(crate) last_updated_tip_ts: u64,

    pub(crate) is_close_to_the_end: bool,
}

impl HeadersSyncController {
    #[cfg(test)]
    pub(crate) fn new(
        started_ts: u64,
        started_tip_ts: u64,
        last_updated_ts: u64,
        last_updated_tip_ts: u64,
        is_close_to_the_end: bool,
    ) -> Self {
        Self {
            started_ts,
            started_tip_ts,
            last_updated_ts,
            last_updated_tip_ts,
            is_close_to_the_end,
        }
    }

    pub(crate) fn from_header(better_tip_header: &core::HeaderView) -> Self {
        let started_ts = unix_time_as_millis();
        let started_tip_ts = better_tip_header.timestamp();
        Self {
            started_ts,
            started_tip_ts,
            last_updated_ts: started_ts,
            last_updated_tip_ts: started_tip_ts,
            is_close_to_the_end: false,
        }
    }

    // https://github.com/rust-lang/rust-clippy/pull/8738
    // wrong_self_convention allows is_* to take &mut self
    #[allow(clippy::wrong_self_convention)]
    pub(crate) fn is_timeout(&mut self, now_tip_ts: u64, now: u64) -> Option<bool> {
        let inspect_window = HEADERS_DOWNLOAD_INSPECT_WINDOW;
        let expected_headers_per_sec = HEADERS_DOWNLOAD_HEADERS_PER_SECOND;
        let tolerable_bias = HEADERS_DOWNLOAD_TOLERABLE_BIAS_FOR_SINGLE_SAMPLE;

        let expected_before_finished = now.saturating_sub(now_tip_ts);

        trace!("headers-sync: better tip ts {}; now {}", now_tip_ts, now);

        if self.is_close_to_the_end {
            let expected_in_base_time =
                expected_headers_per_sec * inspect_window * POW_INTERVAL / 1000;
            if expected_before_finished > expected_in_base_time {
                self.started_ts = now;
                self.started_tip_ts = now_tip_ts;
                self.last_updated_ts = now;
                self.last_updated_tip_ts = now_tip_ts;
                self.is_close_to_the_end = false;
                // if the node is behind the estimated tip header too much, sync again;
                trace!("headers-sync: send GetHeaders again since we behind the tip too much");
                None
            } else {
                // ignore timeout because the tip already almost reach the real time;
                // we can sync to the estimated tip in 1 inspect window by the slowest speed that we can accept.
                Some(false)
            }
        } else if expected_before_finished < inspect_window {
            self.is_close_to_the_end = true;
            trace!("headers-sync: ignore timeout because the tip almost reach the real time");
            Some(false)
        } else {
            let spent_since_last_updated = now.saturating_sub(self.last_updated_ts);

            if spent_since_last_updated < inspect_window {
                // ignore timeout because the time spent since last updated is not enough as a sample
                Some(false)
            } else {
                let synced_since_last_updated = now_tip_ts.saturating_sub(self.last_updated_tip_ts);
                let expected_since_last_updated =
                    expected_headers_per_sec * spent_since_last_updated * POW_INTERVAL / 1000;

                if synced_since_last_updated < expected_since_last_updated / tolerable_bias {
                    // if instantaneous speed is too slow, we don't care the global average speed
                    trace!("headers-sync: the instantaneous speed is too slow");
                    Some(true)
                } else {
                    self.last_updated_ts = now;
                    self.last_updated_tip_ts = now_tip_ts;

                    if synced_since_last_updated > expected_since_last_updated {
                        trace!("headers-sync: the instantaneous speed is acceptable");
                        Some(false)
                    } else {
                        // tolerate more bias for instantaneous speed, we will check the global average speed
                        let spent_since_started = now.saturating_sub(self.started_ts);
                        let synced_since_started = now_tip_ts.saturating_sub(self.started_tip_ts);

                        let expected_since_started =
                            expected_headers_per_sec * spent_since_started * POW_INTERVAL / 1000;

                        if synced_since_started < expected_since_started {
                            // the global average speed is too slow
                            trace!(
                                "headers-sync: both the global average speed and the instantaneous speed \
                                is slow than expected"
                            );
                            Some(true)
                        } else {
                            trace!("headers-sync: the global average speed is acceptable");
                            Some(false)
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct PeerState {
    pub headers_sync_controller: Option<HeadersSyncController>,
    pub peer_flags: PeerFlags,
    pub chain_sync: ChainSyncState,
    // The best known block we know this peer has announced
    pub best_known_header: Option<HeaderView>,
    // The last block we both stored
    pub last_common_header: Option<core::HeaderView>,
    // use on ibd concurrent block download
    // save `get_headers` locator hashes here
    pub unknown_header_list: Vec<Byte32>,
}

impl PeerState {
    pub fn new(peer_flags: PeerFlags) -> PeerState {
        PeerState {
            headers_sync_controller: None,
            peer_flags,
            chain_sync: ChainSyncState::default(),
            best_known_header: None,
            last_common_header: None,
            unknown_header_list: Vec::new(),
        }
    }

    pub fn can_start_sync(&self, now: u64, ibd: bool) -> bool {
        // only sync with protect/whitelist peer in IBD
        ((self.peer_flags.is_protect || self.peer_flags.is_whitelist) || !ibd)
            && self.chain_sync.can_start_sync(now)
    }

    pub fn start_sync(&mut self, headers_sync_controller: HeadersSyncController) {
        self.chain_sync.start();
        self.headers_sync_controller = Some(headers_sync_controller);
    }

    fn suspend_sync(&mut self, suspend_time: u64) {
        let now = unix_time_as_millis();
        self.chain_sync.suspend(now + suspend_time);
        self.headers_sync_controller = None;
    }

    fn tip_synced(&mut self) {
        self.chain_sync.tip_synced();
        self.headers_sync_controller = None;
    }

    pub(crate) fn sync_started(&self) -> bool {
        self.chain_sync.started()
    }

    pub(crate) fn started_or_tip_synced(&self) -> bool {
        self.chain_sync.started_or_tip_synced()
    }

    pub(crate) fn sync_connected(&mut self) {
        self.chain_sync.connected()
    }
}

pub struct TtlFilter<T> {
    inner: LruCache<T, u64>,
    ttl: u64,
}

impl<T: Eq + Hash + Clone> Default for TtlFilter<T> {
    fn default() -> Self {
        TtlFilter::new(FILTER_SIZE, FILTER_TTL)
    }
}

impl<T: Eq + Hash + Clone> TtlFilter<T> {
    pub fn new(size: usize, ttl: u64) -> Self {
        Self {
            inner: LruCache::new(size),
            ttl,
        }
    }

    pub fn contains(&self, item: &T) -> bool {
        self.inner.contains(item)
    }

    pub fn insert(&mut self, item: T) -> bool {
        let now = ckb_systemtime::unix_time().as_secs();
        self.inner.put(item, now).is_none()
    }

    pub fn remove(&mut self, item: &T) -> bool {
        self.inner.pop(item).is_some()
    }

    /// Removes expired items.
    pub fn remove_expired(&mut self) {
        let now = ckb_systemtime::unix_time().as_secs();
        let expired_keys: Vec<T> = self
            .inner
            .iter()
            .filter_map(|(key, time)| {
                if *time + self.ttl < now {
                    Some(key)
                } else {
                    None
                }
            })
            .cloned()
            .collect();

        for k in expired_keys {
            self.remove(&k);
        }
    }
}

#[derive(Default)]
pub struct Peers {
    pub state: DashMap<PeerIndex, PeerState>,
    pub n_sync_started: AtomicUsize,
    pub n_protected_outbound_peers: AtomicUsize,
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

impl BlockNumberAndHash {
    pub fn new(number: BlockNumber, hash: Byte32) -> Self {
        Self { number, hash }
    }

    pub fn number(&self) -> BlockNumber {
        self.number
    }

    pub fn hash(&self) -> Byte32 {
        self.hash.clone()
    }
}

impl From<(BlockNumber, Byte32)> for BlockNumberAndHash {
    fn from(inner: (BlockNumber, Byte32)) -> Self {
        Self {
            number: inner.0,
            hash: inner.1,
        }
    }
}

impl From<&core::HeaderView> for BlockNumberAndHash {
    fn from(header: &core::HeaderView) -> Self {
        Self {
            number: header.number(),
            hash: header.hash(),
        }
    }
}

enum TimeQuantile {
    MinToFast,
    FastToNormal,
    NormalToUpper,
    UpperToMax,
}

/// Using 512 blocks as a period, dynamically adjust the scheduler's time standard
/// Divided into three time periods, including:
///
/// | fast | normal | penalty | double penalty |
///
/// The dividing line is, 1/3 position, 4/5 position, 1/10 position.
///
/// There is 14/30 normal area, 1/10 penalty area, 1/10 double penalty area, 1/3 accelerated reward area.
///
/// Most of the nodes that fall in the normal and accelerated reward area will be retained,
/// while most of the nodes that fall in the normal and penalty zones will be slowly eliminated
///
/// The purpose of dynamic tuning is to reduce the consumption problem of sync networks
/// by retaining the vast majority of nodes with stable communications and
/// cleaning up nodes with significantly lower response times than a certain level
#[derive(Clone)]
struct TimeAnalyzer {
    trace: [u64; TIME_TRACE_SIZE],
    index: usize,
    fast_time: u64,
    normal_time: u64,
    low_time: u64,
}

impl fmt::Debug for TimeAnalyzer {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_struct("TimeAnalyzer")
            .field("fast_time", &self.fast_time)
            .field("normal_time", &self.normal_time)
            .field("low_time", &self.low_time)
            .finish()
    }
}

impl Default for TimeAnalyzer {
    fn default() -> Self {
        // Block max size about 700k, Under 10m/s bandwidth it may cost 1s to response
        Self {
            trace: [0; TIME_TRACE_SIZE],
            index: 0,
            fast_time: 1000,
            normal_time: 1250,
            low_time: 1500,
        }
    }
}

impl TimeAnalyzer {
    fn push_time(&mut self, time: u64) -> TimeQuantile {
        if self.index < TIME_TRACE_SIZE {
            self.trace[self.index] = time;
            self.index += 1;
        } else {
            self.trace.sort_unstable();
            self.fast_time = (self.fast_time.saturating_add(self.trace[FAST_INDEX])) >> 1;
            self.normal_time = (self.normal_time.saturating_add(self.trace[NORMAL_INDEX])) >> 1;
            self.low_time = (self.low_time.saturating_add(self.trace[LOW_INDEX])) >> 1;
            self.trace[0] = time;
            self.index = 1;
        }

        if time <= self.fast_time {
            TimeQuantile::MinToFast
        } else if time <= self.normal_time {
            TimeQuantile::FastToNormal
        } else if time > self.low_time {
            TimeQuantile::UpperToMax
        } else {
            TimeQuantile::NormalToUpper
        }
    }
}

#[derive(Debug, Clone)]
pub struct DownloadScheduler {
    task_count: usize,
    timeout_count: usize,
    hashes: HashSet<BlockNumberAndHash>,
}

impl Default for DownloadScheduler {
    fn default() -> Self {
        Self {
            hashes: HashSet::default(),
            task_count: INIT_BLOCKS_IN_TRANSIT_PER_PEER,
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

    pub(crate) const fn task_count(&self) -> usize {
        self.task_count
    }

    fn increase(&mut self, num: usize) {
        if self.task_count < MAX_BLOCKS_IN_TRANSIT_PER_PEER {
            self.task_count = ::std::cmp::min(
                self.task_count.saturating_add(num),
                MAX_BLOCKS_IN_TRANSIT_PER_PEER,
            )
        }
    }

    fn decrease(&mut self, num: usize) {
        self.timeout_count = self.task_count.saturating_add(num);
        if self.timeout_count > 2 {
            self.task_count = self.task_count.saturating_sub(1);
            self.timeout_count = 0;
        }
    }

    fn punish(&mut self, exp: usize) {
        self.task_count >>= exp
    }
}

#[derive(Clone)]
pub struct InflightBlocks {
    pub(crate) download_schedulers: HashMap<PeerIndex, DownloadScheduler>,
    inflight_states: BTreeMap<BlockNumberAndHash, InflightState>,
    pub(crate) trace_number: HashMap<BlockNumberAndHash, u64>,
    pub(crate) restart_number: BlockNumber,
    time_analyzer: TimeAnalyzer,
    pub(crate) adjustment: bool,
    pub(crate) protect_num: usize,
}

impl Default for InflightBlocks {
    fn default() -> Self {
        InflightBlocks {
            download_schedulers: HashMap::default(),
            inflight_states: BTreeMap::default(),
            trace_number: HashMap::default(),
            restart_number: 0,
            time_analyzer: TimeAnalyzer::default(),
            adjustment: true,
            protect_num: MAX_OUTBOUND_PEERS_TO_PROTECT_FROM_DISCONNECT,
        }
    }
}

struct DebugHashSet<'a>(&'a HashSet<BlockNumberAndHash>);

impl<'a> fmt::Debug for DebugHashSet<'a> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_set()
            .entries(self.0.iter().map(|h| format!("{}, {}", h.number, h.hash)))
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
                    .map(|(k, v)| (format!("{}, {}", k.number, k.hash), v)),
            )
            .finish()?;
        self.time_analyzer.fmt(fmt)
    }
}

impl InflightBlocks {
    pub fn blocks_iter(&self) -> impl Iterator<Item = (&PeerIndex, &HashSet<BlockNumberAndHash>)> {
        self.download_schedulers.iter().map(|(k, v)| (k, &v.hashes))
    }

    pub fn total_inflight_count(&self) -> usize {
        self.inflight_states.len()
    }

    pub fn division_point(&self) -> (u64, u64, u64) {
        (
            self.time_analyzer.fast_time,
            self.time_analyzer.normal_time,
            self.time_analyzer.low_time,
        )
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

    pub fn mark_slow_block(&mut self, tip: BlockNumber) {
        let now = ckb_systemtime::unix_time_as_millis();
        for key in self.inflight_states.keys() {
            if key.number > tip + 1 {
                break;
            }
            self.trace_number.entry(key.clone()).or_insert(now);
        }
    }

    pub fn prune(&mut self, tip: BlockNumber) -> HashSet<PeerIndex> {
        let now = unix_time_as_millis();
        let mut disconnect_list = HashSet::new();
        // Since statistics are currently disturbed by the processing block time, when the number
        // of transactions increases, the node will be accidentally evicted.
        //
        // Especially on machines with poor CPU performance, the node connection will be frequently
        // disconnected due to statistics.
        //
        // In order to protect the decentralization of the network and ensure the survival of low-performance
        // nodes, the penalty mechanism will be closed when the number of download nodes is less than the number of protected nodes
        let should_punish = self.download_schedulers.len() > self.protect_num;
        let adjustment = self.adjustment;

        let trace = &mut self.trace_number;
        let download_schedulers = &mut self.download_schedulers;
        let states = &mut self.inflight_states;

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
                if let Some(set) = download_schedulers.get_mut(&value.peer) {
                    set.hashes.remove(key);
                    if should_punish && adjustment {
                        set.punish(2);
                    }
                };
                if !trace.is_empty() {
                    trace.remove(key);
                }
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
        shrink_to_fit!(download_schedulers, SHRINK_THRESHOLD);

        if self.restart_number != 0 && tip + 1 > self.restart_number {
            self.restart_number = 0;
        }

        // Since each environment is different, the policy here must also be dynamically adjusted
        // according to the current environment, and a low-level limit is given here, since frequent
        // restarting of a task consumes more than a low-level limit
        let timeout_limit = self.time_analyzer.low_time;

        let restart_number = &mut self.restart_number;
        trace.retain(|key, time| {
            // In the normal state, trace will always empty
            //
            // When the inflight request reaches the checkpoint(inflight > tip + 512),
            // it means that there is an anomaly in the sync less than tip + 1, i.e. some nodes are stuck,
            // at which point it will be recorded as the timestamp at that time.
            //
            // If the time exceeds low time limit, delete the task and halve the number of
            // executable tasks for the corresponding node
            if now > timeout_limit + *time {
                if let Some(state) = states.remove(key) {
                    if let Some(d) = download_schedulers.get_mut(&state.peer) {
                        if should_punish && adjustment {
                            d.punish(1);
                        }
                        d.hashes.remove(key);
                    };
                }

                if key.number > *restart_number {
                    *restart_number = key.number;
                }
                return false;
            }
            true
        });
        shrink_to_fit!(trace, SHRINK_THRESHOLD);

        disconnect_list
    }

    pub fn insert(&mut self, peer: PeerIndex, block: BlockNumberAndHash) -> bool {
        let state = self.inflight_states.entry(block.clone());
        match state {
            Entry::Occupied(_entry) => return false,
            Entry::Vacant(entry) => entry.insert(InflightState::new(peer)),
        };

        if self.restart_number >= block.number {
            // All new requests smaller than restart_number mean that they are cleaned up and
            // cannot be immediately marked as cleaned up again.
            self.trace_number
                .insert(block.clone(), unix_time_as_millis());
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

        self.download_schedulers
            .remove(&peer)
            .map(|blocks| {
                for block in blocks.hashes {
                    state.remove(&block);
                    if !trace.is_empty() {
                        trace.remove(&block);
                    }
                }
            })
            .is_some()
    }

    pub fn remove_by_block(&mut self, block: BlockNumberAndHash) -> bool {
        let should_punish = self.download_schedulers.len() > self.protect_num;
        let download_schedulers = &mut self.download_schedulers;
        let trace = &mut self.trace_number;
        let time_analyzer = &mut self.time_analyzer;
        let adjustment = self.adjustment;
        self.inflight_states
            .remove(&block)
            .map(|state| {
                let elapsed = unix_time_as_millis().saturating_sub(state.timestamp);
                if let Some(set) = download_schedulers.get_mut(&state.peer) {
                    set.hashes.remove(&block);
                    if adjustment {
                        match time_analyzer.push_time(elapsed) {
                            TimeQuantile::MinToFast => set.increase(2),
                            TimeQuantile::FastToNormal => set.increase(1),
                            TimeQuantile::NormalToUpper => {
                                if should_punish {
                                    set.decrease(1)
                                }
                            }
                            TimeQuantile::UpperToMax => {
                                if should_punish {
                                    set.decrease(2)
                                }
                            }
                        }
                    }
                    if !trace.is_empty() {
                        trace.remove(&block);
                    }
                };
            })
            .is_some()
    }
}

impl Peers {
    pub fn sync_connected(&self, peer: PeerIndex, is_outbound: bool, is_whitelist: bool) {
        let protect_outbound = is_outbound
            && self
                .n_protected_outbound_peers
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |x| {
                    if x < MAX_OUTBOUND_PEERS_TO_PROTECT_FROM_DISCONNECT {
                        Some(x + 1)
                    } else {
                        None
                    }
                })
                .is_ok();

        let peer_flags = PeerFlags {
            is_outbound,
            is_whitelist,
            is_protect: protect_outbound,
        };
        self.state
            .entry(peer)
            .and_modify(|state| {
                state.peer_flags = peer_flags;
                state.sync_connected();
            })
            .or_insert_with(|| {
                let mut state = PeerState::new(peer_flags);
                state.sync_connected();
                state
            });
    }

    pub fn relay_connected(&self, peer: PeerIndex) {
        self.state
            .entry(peer)
            .or_insert_with(|| PeerState::new(PeerFlags::default()));
    }

    pub fn get_best_known_header(&self, pi: PeerIndex) -> Option<HeaderView> {
        self.state
            .get(&pi)
            .and_then(|peer_state| peer_state.best_known_header.clone())
    }

    pub fn may_set_best_known_header(&self, peer: PeerIndex, header_view: HeaderView) {
        if let Some(mut peer_state) = self.state.get_mut(&peer) {
            if let Some(ref hv) = peer_state.best_known_header {
                if header_view.is_better_than(hv.total_difficulty()) {
                    peer_state.best_known_header = Some(header_view);
                }
            } else {
                peer_state.best_known_header = Some(header_view);
            }
        }
    }

    pub fn get_last_common_header(&self, pi: PeerIndex) -> Option<core::HeaderView> {
        self.state
            .get(&pi)
            .and_then(|peer_state| peer_state.last_common_header.clone())
    }

    pub fn set_last_common_header(&self, pi: PeerIndex, header: core::HeaderView) {
        self.state
            .entry(pi)
            .and_modify(|peer_state| peer_state.last_common_header = Some(header));
    }

    pub fn getheaders_received(&self, _peer: PeerIndex) {
        // TODO:
    }

    pub fn disconnected(&self, peer: PeerIndex) {
        if let Some(peer_state) = self.state.remove(&peer).map(|(_, peer_state)| peer_state) {
            if peer_state.sync_started() {
                // It shouldn't happen
                // fetch_sub wraps around on overflow, we still check manually
                // panic here to prevent some bug be hidden silently.
                assert_ne!(
                    self.n_sync_started.fetch_sub(1, Ordering::AcqRel),
                    0,
                    "n_sync_started overflow when disconnects"
                );
            }

            // Protection node disconnected
            if peer_state.peer_flags.is_protect {
                assert_ne!(
                    self.n_protected_outbound_peers
                        .fetch_sub(1, Ordering::AcqRel),
                    0,
                    "n_protected_outbound_peers overflow when disconnects"
                );
            }
        }
    }

    pub fn insert_unknown_header_hash(&self, peer: PeerIndex, hash: Byte32) {
        self.state
            .entry(peer)
            .and_modify(|state| state.unknown_header_list.push(hash));
    }

    pub fn unknown_header_list_is_empty(&self, peer: PeerIndex) -> bool {
        self.state
            .get(&peer)
            .map(|state| state.unknown_header_list.is_empty())
            .unwrap_or(true)
    }

    pub fn clear_unknown_list(&self) {
        self.state.iter_mut().for_each(|mut state| {
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
            .iter()
            .filter_map(|kv_pair| {
                let (peer_index, state) = kv_pair.pair();
                if !state.unknown_header_list.is_empty() {
                    return None;
                }
                match state.best_known_header {
                    Some(ref header) if header.number() < tip => Some(*peer_index),
                    _ => None,
                }
            })
            .collect()
    }

    pub fn take_unknown_last(&self, peer: PeerIndex) -> Option<Byte32> {
        self.state
            .get_mut(&peer)
            .and_then(|mut state| state.unknown_header_list.pop())
    }

    pub fn get_flag(&self, peer: PeerIndex) -> Option<PeerFlags> {
        self.state.get(&peer).map(|state| state.peer_flags)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderView {
    inner: core::HeaderView,
    total_difficulty: U256,
    // pointer to the index of some further predecessor of this block
    pub(crate) skip_hash: Option<Byte32>,
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

    pub fn build_skip<F, G>(&mut self, tip_number: BlockNumber, get_header_view: F, fast_scanner: G)
    where
        F: FnMut(&Byte32, Option<bool>) -> Option<HeaderView>,
        G: Fn(BlockNumber, &HeaderView) -> Option<HeaderView>,
    {
        if self.inner.is_genesis() {
            return;
        }
        self.skip_hash = self
            .clone()
            .get_ancestor(
                tip_number,
                get_skip_height(self.number()),
                get_header_view,
                fast_scanner,
            )
            .map(|header| header.hash());
    }

    // NOTE: get_header_view may change source state, for cache or for tests
    pub fn get_ancestor<F, G>(
        self,
        tip_number: BlockNumber,
        number: BlockNumber,
        mut get_header_view: F,
        fast_scanner: G,
    ) -> Option<core::HeaderView>
    where
        F: FnMut(&Byte32, Option<bool>) -> Option<HeaderView>,
        G: Fn(BlockNumber, &HeaderView) -> Option<HeaderView>,
    {
        let mut current = self;
        if number > current.number() {
            return None;
        }

        let mut number_walk = current.number();
        while number_walk > number {
            let number_skip = get_skip_height(number_walk);
            let number_skip_prev = get_skip_height(number_walk - 1);
            let store_first = current.number() <= tip_number;
            match current.skip_hash {
                Some(ref hash)
                    if number_skip == number
                        || (number_skip > number
                            && !(number_skip_prev + 2 < number_skip
                                && number_skip_prev >= number)) =>
                {
                    // Only follow skip if parent->skip isn't better than skip->parent
                    current = get_header_view(hash, Some(store_first))?;
                    number_walk = number_skip;
                }
                _ => {
                    current = get_header_view(&current.parent_hash(), Some(store_first))?;
                    number_walk -= 1;
                }
            }
            if let Some(target) = fast_scanner(number, &current) {
                current = target;
                break;
            }
        }
        Some(current).map(HeaderView::into_inner)
    }

    pub fn is_better_than(&self, total_difficulty: &U256) -> bool {
        self.total_difficulty() > total_difficulty
    }

    fn from_slice_should_be_ok(slice: &[u8]) -> Self {
        let len_size = packed::Uint32Reader::TOTAL_SIZE;
        if slice.len() < len_size {
            panic!("failed to unpack item in header map: header part is broken");
        }
        let mut idx = 0;
        let inner_len = {
            let reader = packed::Uint32Reader::from_slice_should_be_ok(&slice[idx..idx + len_size]);
            Unpack::<u32>::unpack(&reader) as usize
        };
        idx += len_size;
        let total_difficulty_len = packed::Uint256Reader::TOTAL_SIZE;
        if slice.len() < len_size + inner_len + total_difficulty_len {
            panic!("failed to unpack item in header map: body part is broken");
        }
        let inner = {
            let reader =
                packed::HeaderViewReader::from_slice_should_be_ok(&slice[idx..idx + inner_len]);
            Unpack::<core::HeaderView>::unpack(&reader)
        };
        idx += inner_len;
        let total_difficulty = {
            let reader = packed::Uint256Reader::from_slice_should_be_ok(
                &slice[idx..idx + total_difficulty_len],
            );
            Unpack::<U256>::unpack(&reader)
        };
        idx += total_difficulty_len;
        let skip_hash = {
            packed::Byte32OptReader::from_slice_should_be_ok(&slice[idx..])
                .to_entity()
                .to_opt()
        };
        Self {
            inner,
            total_difficulty,
            skip_hash,
        }
    }

    fn to_vec(&self) -> Vec<u8> {
        let mut v = Vec::new();
        let inner: packed::HeaderView = self.inner.pack();
        let total_difficulty: packed::Uint256 = self.total_difficulty.pack();
        let skip_hash: packed::Byte32Opt = Pack::pack(&self.skip_hash);
        let inner_len: packed::Uint32 = (inner.as_slice().len() as u32).pack();
        v.extend_from_slice(inner_len.as_slice());
        v.extend_from_slice(inner.as_slice());
        v.extend_from_slice(total_difficulty.as_slice());
        v.extend_from_slice(skip_hash.as_slice());
        v
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

// <CompactBlockHash, (CompactBlock, <PeerIndex, (TransactionsIndex, UnclesIndex)>, timestamp)>
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
    /// only use on test
    pub fn new(
        shared: Shared,
        sync_config: SyncConfig,
        tx_relay_receiver: Receiver<TxVerificationResult>,
    ) -> SyncShared {
        Self::with_tmpdir::<PathBuf>(shared, sync_config, None, tx_relay_receiver)
    }

    /// Generate a global sync state through configuration
    pub fn with_tmpdir<P>(
        shared: Shared,
        sync_config: SyncConfig,
        tmpdir: Option<P>,
        tx_relay_receiver: Receiver<TxVerificationResult>,
    ) -> SyncShared
    where
        P: AsRef<Path>,
    {
        let (total_difficulty, header) = {
            let snapshot = shared.snapshot();
            (
                snapshot.total_difficulty().to_owned(),
                snapshot.tip_header().to_owned(),
            )
        };
        let shared_best_header = RwLock::new(HeaderView::new(header, total_difficulty));
        ckb_logger::info!(
            "header_map.memory_limit {}",
            sync_config.header_map.memory_limit
        );
        let header_map = HeaderMap::new(
            tmpdir,
            sync_config.header_map.memory_limit.as_u64() as usize,
            shared.async_handle(),
        );

        let state = SyncState {
            shared_best_header,
            header_map,
            block_status_map: DashMap::new(),
            tx_filter: Mutex::new(TtlFilter::default()),
            unknown_tx_hashes: Mutex::new(KeyedPriorityQueue::new()),
            peers: Peers::default(),
            pending_get_block_proposals: DashMap::new(),
            pending_compact_blocks: Mutex::new(HashMap::default()),
            orphan_block_pool: OrphanBlockPool::with_capacity(ORPHAN_BLOCK_SIZE),
            inflight_proposals: DashMap::new(),
            inflight_blocks: RwLock::new(InflightBlocks::default()),
            pending_get_headers: RwLock::new(LruCache::new(GET_HEADERS_CACHE_SIZE)),
            tx_relay_receiver,
            assume_valid_target: Mutex::new(sync_config.assume_valid_target),
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
        ActiveChain {
            shared: self.clone(),
            snapshot: Arc::clone(&self.shared.snapshot()),
            state: Arc::clone(&self.state),
        }
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

    /// Insert new block to chain store
    pub fn insert_new_block(
        &self,
        chain: &ChainController,
        block: Arc<core::BlockView>,
    ) -> Result<bool, CKBError> {
        // Insert the given block into orphan_block_pool if its parent is not found
        if !self.is_stored(&block.parent_hash()) {
            debug!(
                "insert new orphan block {} {}",
                block.header().number(),
                block.header().hash()
            );
            self.state.insert_orphan_block((*block).clone());
            return Ok(false);
        }

        // Attempt to accept the given block if its parent already exist in database
        let ret = self.accept_block(chain, Arc::clone(&block));
        if ret.is_err() {
            debug!("accept block {:?} {:?}", block, ret);
            return ret;
        }

        // The above block has been accepted. Attempt to accept its descendant blocks in orphan pool.
        // The returned blocks of `remove_blocks_by_parent` are in topology order by parents
        self.try_search_orphan_pool(chain);
        ret
    }

    /// Try to find blocks from the orphan block pool that may no longer be orphan
    pub fn try_search_orphan_pool(&self, chain: &ChainController) {
        let leaders = self.state.orphan_pool().clone_leaders();
        debug!("orphan pool leader parents hash len: {}", leaders.len());

        for hash in leaders {
            if self.state.orphan_pool().is_empty() {
                break;
            }
            if self.is_stored(&hash) {
                let descendants = self.state.remove_orphan_by_parent(&hash);
                debug!(
                    "try accepting {} descendant orphan blocks by exist parents hash",
                    descendants.len()
                );
                for block in descendants {
                    // If we can not find the block's parent in database, that means it was failed to accept
                    // its parent, so we treat it as an invalid block as well.
                    if !self.is_stored(&block.parent_hash()) {
                        debug!(
                            "parent-unknown orphan block, block: {}, {}, parent: {}",
                            block.header().number(),
                            block.header().hash(),
                            block.header().parent_hash(),
                        );
                        continue;
                    }

                    let block = Arc::new(block);
                    if let Err(err) = self.accept_block(chain, Arc::clone(&block)) {
                        debug!(
                            "accept descendant orphan block {} error {:?}",
                            block.header().hash(),
                            err
                        );
                    }
                }
            }
        }
    }

    /// cleanup orphan_pool, remove blocks with epoch less 2 than currently epoch.
    pub(crate) fn periodic_clean_orphan_pool(&self) {
        let hashes = self
            .state
            .clean_expired_blocks(self.active_chain().epoch_ext().number());
        for hash in hashes {
            self.state.remove_header_view(&hash);
        }
    }

    pub(crate) fn accept_block(
        &self,
        chain: &ChainController,
        block: Arc<core::BlockView>,
    ) -> Result<bool, CKBError> {
        let ret = {
            let mut assume_valid_target = self.state.assume_valid_target();
            if let Some(ref target) = *assume_valid_target {
                // if the target has been reached, delete it
                let switch = if target == &Unpack::<H256>::unpack(&core::BlockView::hash(&block)) {
                    assume_valid_target.take();
                    Switch::NONE
                } else {
                    Switch::DISABLE_SCRIPT
                };

                chain.internal_process_block(Arc::clone(&block), switch)
            } else {
                chain.process_block(Arc::clone(&block))
            }
        };
        if let Err(ref error) = ret {
            if !is_internal_db_error(error) {
                error!("accept block {:?} {}", block, error);
                self.state
                    .insert_block_status(block.header().hash(), BlockStatus::BLOCK_INVALID);
            }
        } else {
            // Clear the newly inserted block from block_status_map.
            //
            // We don't know whether the actual block status is BLOCK_VALID or BLOCK_INVALID.
            // So we just simply remove the corresponding in-memory block status,
            // and the next time `get_block_status` would acquire the real-time
            // status via fetching block_ext from the database.
            self.state.remove_block_status(&block.as_ref().hash());
            self.state.remove_header_view(&block.as_ref().hash());
        }

        ret
    }

    /// Sync a new valid header, try insert to sync state
    // Update the header_map
    // Update the block_status_map
    // Update the shared_best_header if need
    // Update the peer's best_known_header
    pub fn insert_valid_header(&self, peer: PeerIndex, header: &core::HeaderView) {
        let tip_number = self.active_chain().tip_number();
        let store_first = tip_number >= header.number();
        let parent_view = self
            .get_header_view(&header.data().raw().parent_hash(), Some(store_first))
            .expect("parent should be verified");
        let mut header_view = {
            let total_difficulty = parent_view.total_difficulty() + header.difficulty();
            HeaderView::new(header.clone(), total_difficulty)
        };

        let snapshot = Arc::clone(&self.shared.snapshot());
        header_view.build_skip(
            tip_number,
            |hash, store_first_opt| self.get_header_view(hash, store_first_opt),
            |number, current| {
                // shortcut to return an ancestor block
                if current.number() <= snapshot.tip_number()
                    && snapshot.is_main_chain(&current.hash())
                {
                    snapshot
                        .get_block_hash(number)
                        .and_then(|hash| self.get_header_view(&hash, Some(true)))
                } else {
                    None
                }
            },
        );
        self.state.header_map.insert(header_view.clone());
        self.state
            .peers()
            .may_set_best_known_header(peer, header_view.clone());
        self.state.may_set_shared_best_header(header_view);
    }

    /// Get header view with hash
    pub fn get_header_view(
        &self,
        hash: &Byte32,
        store_first_opt: Option<bool>,
    ) -> Option<HeaderView> {
        let store = self.store();
        if store_first_opt.unwrap_or(false) {
            store
                .get_block_header(hash)
                .and_then(|header| {
                    store
                        .get_block_ext(hash)
                        .map(|block_ext| HeaderView::new(header, block_ext.total_difficulty))
                })
                .or_else(|| self.state.header_map.get(hash))
        } else {
            self.state.header_map.get(hash).or_else(|| {
                store.get_block_header(hash).and_then(|header| {
                    store
                        .get_block_ext(hash)
                        .map(|block_ext| HeaderView::new(header, block_ext.total_difficulty))
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
}

impl HeaderProvider for SyncShared {
    fn get_header(&self, hash: &Byte32) -> Option<core::HeaderView> {
        self.state
            .header_map
            .get(hash)
            .map(HeaderView::into_inner)
            .or_else(|| self.store().get_block_header(hash))
    }
}

#[derive(Eq, PartialEq, Clone)]
pub struct UnknownTxHashPriority {
    request_time: Instant,
    peers: Vec<PeerIndex>,
    requested: bool,
}

impl UnknownTxHashPriority {
    pub fn should_request(&self, now: Instant) -> bool {
        self.next_request_at() < now
    }

    pub fn next_request_at(&self) -> Instant {
        if self.requested {
            self.request_time + RETRY_ASK_TX_TIMEOUT_INCREASE
        } else {
            self.request_time
        }
    }

    pub fn next_request_peer(&mut self) -> Option<PeerIndex> {
        if self.requested {
            if self.peers.len() > 1 {
                self.request_time = Instant::now();
                self.peers.swap_remove(0);
                self.peers.get(0).cloned()
            } else {
                None
            }
        } else {
            self.requested = true;
            self.peers.get(0).cloned()
        }
    }

    pub fn push_peer(&mut self, peer_index: PeerIndex) {
        self.peers.push(peer_index);
    }

    pub fn requesting_peer(&self) -> Option<PeerIndex> {
        if self.requested {
            self.peers.get(0).cloned()
        } else {
            None
        }
    }
}

impl Ord for UnknownTxHashPriority {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.next_request_at()
            .cmp(&other.next_request_at())
            .reverse()
    }
}

impl PartialOrd for UnknownTxHashPriority {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

pub struct SyncState {
    /* Status irrelevant to peers */
    shared_best_header: RwLock<HeaderView>,
    header_map: HeaderMap,
    block_status_map: DashMap<Byte32, BlockStatus>,
    tx_filter: Mutex<TtlFilter<Byte32>>,

    // The priority is ordering by timestamp (reversed), means do not ask the tx before this timestamp (timeout).
    unknown_tx_hashes: Mutex<KeyedPriorityQueue<Byte32, UnknownTxHashPriority>>,

    /* Status relevant to peers */
    peers: Peers,

    /* Cached items which we had received but not completely process */
    pending_get_block_proposals: DashMap<packed::ProposalShortId, HashSet<PeerIndex>>,
    pending_get_headers: RwLock<LruCache<(PeerIndex, Byte32), Instant>>,
    pending_compact_blocks: Mutex<PendingCompactBlockMap>,
    orphan_block_pool: OrphanBlockPool,

    /* In-flight items for which we request to peers, but not got the responses yet */
    inflight_proposals: DashMap<packed::ProposalShortId, BlockNumber>,
    inflight_blocks: RwLock<InflightBlocks>,

    /* cached for sending bulk */
    tx_relay_receiver: Receiver<TxVerificationResult>,
    assume_valid_target: Mutex<Option<H256>>,
    min_chain_work: U256,
}

impl SyncState {
    pub fn assume_valid_target(&self) -> MutexGuard<Option<H256>> {
        self.assume_valid_target.lock()
    }

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
        let pending = self.pending_compact_blocks.lock();
        // After compact block request 2s or pending is empty, sync can create tasks
        pending.is_empty()
            || pending
                .get(hash)
                .map(|(_, _, time)| now > time + 2000)
                .unwrap_or(true)
    }

    pub fn pending_compact_blocks(&self) -> MutexGuard<PendingCompactBlockMap> {
        self.pending_compact_blocks.lock()
    }

    pub fn read_inflight_blocks(&self) -> RwLockReadGuard<InflightBlocks> {
        self.inflight_blocks.read()
    }

    pub fn write_inflight_blocks(&self) -> RwLockWriteGuard<InflightBlocks> {
        self.inflight_blocks.write()
    }

    pub fn take_relay_tx_verify_results(&self, limit: usize) -> Vec<TxVerificationResult> {
        self.tx_relay_receiver.try_iter().take(limit).collect()
    }

    pub fn shared_best_header(&self) -> HeaderView {
        self.shared_best_header.read().to_owned()
    }

    pub fn shared_best_header_ref(&self) -> RwLockReadGuard<HeaderView> {
        self.shared_best_header.read()
    }

    pub fn header_map(&self) -> &HeaderMap {
        &self.header_map
    }

    pub fn may_set_shared_best_header(&self, header: HeaderView) {
        if !header.is_better_than(self.shared_best_header.read().total_difficulty()) {
            return;
        }

        if let Some(metrics) = ckb_metrics::handle() {
            metrics.ckb_shared_best_number.set(header.number() as i64);
        }
        *self.shared_best_header.write() = header;
    }

    pub fn remove_header_view(&self, hash: &Byte32) {
        self.header_map.remove(hash);
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
            ckb_logger::warn!(
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

    pub fn tx_filter(&self) -> MutexGuard<TtlFilter<Byte32>> {
        self.tx_filter.lock()
    }

    pub fn unknown_tx_hashes(
        &self,
    ) -> MutexGuard<KeyedPriorityQueue<Byte32, UnknownTxHashPriority>> {
        self.unknown_tx_hashes.lock()
    }

    // Return true when the block is that we have requested and received first time.
    pub fn new_block_received(&self, block: &core::BlockView) -> bool {
        if self
            .write_inflight_blocks()
            .remove_by_block((block.number(), block.hash()).into())
        {
            self.insert_block_status(block.hash(), BlockStatus::BLOCK_RECEIVED);
            true
        } else {
            false
        }
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

    pub fn insert_orphan_block(&self, block: core::BlockView) {
        self.insert_block_status(block.hash(), BlockStatus::BLOCK_RECEIVED);
        self.orphan_block_pool.insert(block);
    }

    pub fn remove_orphan_by_parent(&self, parent_hash: &Byte32) -> Vec<core::BlockView> {
        let blocks = self.orphan_block_pool.remove_blocks_by_parent(parent_hash);
        blocks.iter().for_each(|block| {
            self.block_status_map.remove(&block.hash());
        });
        shrink_to_fit!(self.block_status_map, SHRINK_THRESHOLD);
        blocks
    }

    pub fn orphan_pool(&self) -> &OrphanBlockPool {
        &self.orphan_block_pool
    }

    pub fn insert_block_status(&self, block_hash: Byte32, status: BlockStatus) {
        self.block_status_map.insert(block_hash, status);
    }

    pub fn remove_block_status(&self, block_hash: &Byte32) {
        self.block_status_map.remove(block_hash);
        shrink_to_fit!(self.block_status_map, SHRINK_THRESHOLD);
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

    pub fn disconnected(&self, pi: PeerIndex) {
        self.write_inflight_blocks().remove_by_peer(pi);
        self.peers().disconnected(pi);
    }

    pub fn get_orphan_block(&self, block_hash: &Byte32) -> Option<core::BlockView> {
        self.orphan_block_pool.get_block(block_hash)
    }

    pub fn clean_expired_blocks(&self, epoch: EpochNumber) -> Vec<packed::Byte32> {
        self.orphan_block_pool.clean_expired_blocks(epoch)
    }

    pub fn insert_peer_unknown_header_list(&self, pi: PeerIndex, header_list: Vec<Byte32>) {
        // update peer's unknown_header_list only once
        if self.peers.unknown_header_list_is_empty(pi) {
            // header list is an ordered list, sorted from highest to lowest,
            // so here you discard and exit early
            for hash in header_list {
                if let Some(header) = self.header_map.get(&hash) {
                    self.peers.may_set_best_known_header(pi, header);
                    break;
                } else {
                    self.peers.insert_unknown_header_hash(pi, hash)
                }
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

#[doc(hidden)]
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

    pub fn get_block_ext(&self, h: &packed::Byte32) -> Option<core::BlockExt> {
        self.snapshot().get_block_ext(h)
    }

    pub fn get_block_filter(&self, hash: &packed::Byte32) -> Option<packed::Bytes> {
        self.store().get_block_filter(hash)
    }

    pub fn get_block_filter_hash(&self, hash: &packed::Byte32) -> Option<packed::Byte32> {
        self.store().get_block_filter_hash(hash)
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

    pub fn is_main_chain(&self, hash: &packed::Byte32) -> bool {
        self.snapshot.is_main_chain(hash)
    }

    pub fn is_initial_block_download(&self) -> bool {
        self.shared.shared().is_initial_block_download()
    }

    pub fn get_ancestor(&self, base: &Byte32, number: BlockNumber) -> Option<core::HeaderView> {
        let tip_number = self.tip_number();
        self.shared.get_header_view(base, None)?.get_ancestor(
            tip_number,
            number,
            |hash, store_first_opt| self.shared.get_header_view(hash, store_first_opt),
            |number, current| {
                // shortcut to return an ancestor block
                if current.number() <= tip_number && self.snapshot().is_main_chain(&current.hash())
                {
                    self.get_block_hash(number)
                        .and_then(|hash| self.shared.get_header_view(&hash, Some(true)))
                } else {
                    None
                }
            },
        )
    }

    pub fn get_locator(&self, start: BlockNumberAndHash) -> Vec<Byte32> {
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
                         start: {:?}, base: {}, step: {}, locators({}): {:?}.",
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

            if index < step * 2 {
                // Insert some low-height blocks in the locator
                // to quickly start parallel ibd block downloads
                // and it should not be too much
                //
                // 100 * 365 * 86400 / 8 = 394200000  100 years block number
                // 2 ** 29 = 536870912
                // 2 ** 13 = 8192
                // 52 = 10 + 29 + 13
                if locator.len() < 52 && index > ONE_DAY_BLOCK_NUMBER {
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

    pub fn last_common_ancestor(
        &self,
        pa: &core::HeaderView,
        pb: &core::HeaderView,
    ) -> Option<core::HeaderView> {
        let (mut m_left, mut m_right) = if pa.number() > pb.number() {
            (pb.clone(), pa.clone())
        } else {
            (pa.clone(), pb.clone())
        };

        m_right = self.get_ancestor(&m_right.hash(), m_left.number())?;
        if m_left == m_right {
            return Some(m_left);
        }
        debug_assert!(m_left.number() == m_right.number());

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
        block_number_and_hash: BlockNumberAndHash,
    ) {
        if let Some(last_time) = self
            .state
            .pending_get_headers
            .write()
            .get(&(peer, block_number_and_hash.hash()))
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
            .put((peer, block_number_and_hash.hash()), Instant::now());

        debug!(
            "send_getheaders_to_peer peer={}, hash={}",
            peer,
            block_number_and_hash.hash()
        );
        let locator_hash = self.get_locator(block_number_and_hash);
        let content = packed::GetHeaders::new_builder()
            .block_locator_hashes(locator_hash.pack())
            .hash_stop(packed::Byte32::zero())
            .build();
        let message = packed::SyncMessage::new_builder().set(content).build();
        let _status = send_message(SupportProtocols::Sync.protocol_id(), nc, peer, &message);
    }

    pub fn get_block_status(&self, block_hash: &Byte32) -> BlockStatus {
        match self.state.block_status_map.get(block_hash) {
            Some(status_ref) => *status_ref.value(),
            None => {
                if self.state.header_map.contains_key(block_hash) {
                    BlockStatus::HEADER_VALID
                } else {
                    let verified = self
                        .snapshot
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

    pub fn contains_block_status(&self, block_hash: &Byte32, status: BlockStatus) -> bool {
        self.get_block_status(block_hash).contains(status)
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

impl From<IBDState> for bool {
    fn from(s: IBDState) -> bool {
        match s {
            IBDState::In => true,
            IBDState::Out => false,
        }
    }
}
