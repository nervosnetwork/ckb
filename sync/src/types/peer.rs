use ckb_chain_spec::consensus::{MAX_BLOCK_INTERVAL, MIN_BLOCK_INTERVAL};
use ckb_constant::sync::{
    HEADERS_DOWNLOAD_HEADERS_PER_SECOND, HEADERS_DOWNLOAD_INSPECT_WINDOW,
    HEADERS_DOWNLOAD_TOLERABLE_BIAS_FOR_SINGLE_SAMPLE,
    MAX_OUTBOUND_PEERS_TO_PROTECT_FROM_DISCONNECT, POW_INTERVAL,
};
use ckb_logger::trace;
use ckb_network::PeerIndex;
use ckb_shared::types::{HeaderIndex, HeaderIndexView};
use ckb_systemtime::unix_time_as_millis;
use ckb_types::{
    U256,
    core::{self, BlockNumber},
    packed::Byte32,
};
use dashmap::DashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use ckb_types::BlockNumberAndHash;

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
    pub(super) fn can_start_sync(&self, now: u64) -> bool {
        match self.headers_sync_state {
            HeadersSyncState::Initialized => false,
            HeadersSyncState::SyncProtocolConnected => true,
            HeadersSyncState::Started => false,
            HeadersSyncState::Suspend(until) | HeadersSyncState::TipSynced(until) => until < now,
        }
    }

    pub(super) fn connected(&mut self) {
        self.headers_sync_state = HeadersSyncState::SyncProtocolConnected;
    }

    pub(super) fn start(&mut self) {
        self.headers_sync_state = HeadersSyncState::Started
    }

    pub(super) fn suspend(&mut self, until: u64) {
        self.headers_sync_state = HeadersSyncState::Suspend(until)
    }

    pub(super) fn tip_synced(&mut self) {
        let now = unix_time_as_millis();
        let avg_interval = (MAX_BLOCK_INTERVAL + MIN_BLOCK_INTERVAL) / 2;
        self.headers_sync_state = HeadersSyncState::TipSynced(now + avg_interval * 1000);
    }

    pub(super) fn started(&self) -> bool {
        matches!(self.headers_sync_state, HeadersSyncState::Started)
    }

    pub(super) fn started_or_tip_synced(&self) -> bool {
        matches!(
            self.headers_sync_state,
            HeadersSyncState::Started | HeadersSyncState::TipSynced(_)
        )
    }
}

#[derive(Default, Clone, Debug)]
enum HeadersSyncState {
    #[default]
    Initialized,
    SyncProtocolConnected,
    Started,
    Suspend(u64), // suspend headers sync until this timestamp (milliseconds since unix epoch)
    TipSynced(u64), // already synced to the end, not as the sync target for the time being, until the pause time is exceeded
}

#[derive(Clone, Default, Debug, Copy)]
pub struct PeerFlags {
    pub is_outbound: bool,
    pub is_protect: bool,
    pub is_whitelist: bool,
    pub is_2023edition: bool,
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

    pub(crate) fn from_header(better_tip_header: &HeaderIndexView) -> Self {
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
                trace!(
                    "headers-sync: send GetHeaders again since we are significantly behind the tip"
                );
                None
            } else {
                // ignore timeout because the tip already almost reach the real time;
                // we can sync to the estimated tip in 1 inspect window by the slowest speed that we can accept.
                Some(false)
            }
        } else if expected_before_finished < inspect_window {
            self.is_close_to_the_end = true;
            trace!("headers-sync: ignore timeout because the tip almost reaches the real time");
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
                                are slower than expected"
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
    pub best_known_header: Option<HeaderIndex>,
    // The last block we both stored
    pub last_common_header: Option<BlockNumberAndHash>,
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

    pub(super) fn suspend_sync(&mut self, suspend_time: u64) {
        let now = unix_time_as_millis();
        self.chain_sync.suspend(now + suspend_time);
        self.headers_sync_controller = None;
    }

    pub(super) fn tip_synced(&mut self) {
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

#[derive(Default)]
pub struct Peers {
    pub state: DashMap<PeerIndex, PeerState>,
    pub n_sync_started: AtomicUsize,
    pub n_protected_outbound_peers: AtomicUsize,
}

impl Peers {
    pub fn sync_connected(
        &self,
        peer: PeerIndex,
        is_outbound: bool,
        is_whitelist: bool,
        is_2023edition: bool,
    ) {
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
            is_2023edition,
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

    pub fn get_best_known_header(&self, pi: PeerIndex) -> Option<HeaderIndex> {
        self.state
            .get(&pi)
            .and_then(|peer_state| peer_state.best_known_header.clone())
    }

    pub fn may_set_best_known_header(&self, peer: PeerIndex, header_index: HeaderIndex) {
        if let Some(mut peer_state) = self.state.get_mut(&peer) {
            if let Some(ref known) = peer_state.best_known_header {
                if header_index.is_better_chain(known) {
                    peer_state.best_known_header = Some(header_index);
                }
            } else {
                peer_state.best_known_header = Some(header_index);
            }
        }
    }

    pub fn get_last_common_header(&self, pi: PeerIndex) -> Option<BlockNumberAndHash> {
        self.state
            .get(&pi)
            .and_then(|peer_state| peer_state.last_common_header.clone())
    }

    pub fn set_last_common_header(&self, pi: PeerIndex, header: BlockNumberAndHash) {
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
                if let Some(ref header) = state.best_known_header
                    && header.number() < tip
                {
                    return Some(*peer_index);
                }
                None
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
