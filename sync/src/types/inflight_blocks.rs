use crate::{FAST_INDEX, LOW_INDEX, NORMAL_INDEX, TIME_TRACE_SIZE};
use ckb_constant::sync::{
    BLOCK_DOWNLOAD_TIMEOUT, INIT_BLOCKS_IN_TRANSIT_PER_PEER, MAX_BLOCKS_IN_TRANSIT_PER_PEER,
    MAX_OUTBOUND_PEERS_TO_PROTECT_FROM_DISCONNECT,
};
use ckb_logger::debug;
use ckb_network::PeerIndex;
use ckb_shared::types::SHRINK_THRESHOLD;
use ckb_systemtime::unix_time_as_millis;
use ckb_types::BlockNumberAndHash;
use ckb_types::core::BlockNumber;
use ckb_util::shrink_to_fit;
use std::collections::{BTreeMap, HashMap, HashSet, btree_map::Entry};
use std::fmt;

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
                debug!(
                    "prune: remove InflightState: remove {}-{} from {}",
                    key.number, key.hash, value.peer
                );

                if let Some(metrics) = ckb_metrics::handle() {
                    metrics.ckb_inflight_timeout_count.inc();
                }
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
                if let Some(state) = states.remove(key)
                    && let Some(d) = download_schedulers.get_mut(&state.peer)
                {
                    if should_punish && adjustment {
                        d.punish(1);
                    }
                    d.hashes.remove(key);
                    debug!(
                        "prune: remove download_schedulers: remove {}-{} from {}",
                        key.number, key.hash, state.peer
                    );
                };

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

        let download_scheduler = self.download_schedulers.entry(peer).or_default();
        download_scheduler.hashes.insert(block)
    }

    pub fn remove_by_peer(&mut self, peer: PeerIndex) -> usize {
        let trace = &mut self.trace_number;
        let state = &mut self.inflight_states;

        self.download_schedulers
            .remove(&peer)
            .map(|blocks| {
                let blocks_count = blocks.hashes.iter().len();
                for block in blocks.hashes {
                    state.remove(&block);
                    if !trace.is_empty() {
                        trace.remove(&block);
                    }
                }
                blocks_count
            })
            .unwrap_or_default()
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
