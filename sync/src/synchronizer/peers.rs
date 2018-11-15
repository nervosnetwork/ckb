use super::header_view::HeaderView;
use bigint::H256;
use ckb_shared::shared::TipHeader;
use ckb_time::now_ms;
use core::block::Block;
use core::header::Header;
use fnv::{FnvHashMap, FnvHashSet};
use network::PeerIndex;
use util::RwLock;

// const BANSCORE: u32 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Negotiate {
    pub prefer_headers: bool,
    //     pub want_cmpct: bool,
    //     pub have_witness: bool,
}

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
    pub work_header: Option<TipHeader>,
    pub sent_getheaders: bool,
    pub protect: bool,
}

impl Default for ChainSyncState {
    fn default() -> Self {
        ChainSyncState {
            timeout: 0,
            work_header: None,
            sent_getheaders: false,
            protect: false,
        }
    }
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct PeerState {
    pub negotiate: Negotiate,
    pub sync_started: bool,
    pub last_block_announcement: Option<u64>, //ms
    pub headers_sync_timeout: Option<u64>,
    pub disconnect: bool,
    pub chain_sync: ChainSyncState,
}

#[derive(Debug, Default)]
pub struct Peers {
    pub state: RwLock<FnvHashMap<PeerIndex, PeerState>>,
    pub misbehavior: RwLock<FnvHashMap<PeerIndex, u32>>,
    pub blocks_inflight: RwLock<FnvHashMap<PeerIndex, BlocksInflight>>,
    pub best_known_headers: RwLock<FnvHashMap<PeerIndex, HeaderView>>,
    pub last_common_headers: RwLock<FnvHashMap<PeerIndex, Header>>,
}

#[derive(Debug, Clone)]
pub struct BlocksInflight {
    pub timestamp: u64,
    pub blocks: FnvHashSet<H256>,
}

impl Default for BlocksInflight {
    fn default() -> Self {
        BlocksInflight {
            blocks: FnvHashSet::default(),
            timestamp: now_ms(),
        }
    }
}

impl BlocksInflight {
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn insert(&mut self, hash: H256) -> bool {
        self.blocks.insert(hash)
    }

    pub fn remove(&mut self, hash: &H256) -> bool {
        self.blocks.remove(hash)
    }

    pub fn update_timestamp(&mut self) {
        self.timestamp = now_ms();
    }

    pub fn clear(&mut self) {
        self.blocks.clear();
    }
}

impl Peers {
    pub fn misbehavior(&self, peer: PeerIndex, score: u32) {
        if score == 0 {
            return;
        }

        let mut map = self.misbehavior.write();
        map.entry(peer)
            .and_modify(|e| *e += score)
            .or_insert_with(|| score);
    }

    pub fn on_connected(&self, peer: PeerIndex, headers_sync_timeout: u64, protect: bool) {
        self.state
            .write()
            .entry(peer)
            .and_modify(|state| {
                state.headers_sync_timeout = Some(headers_sync_timeout);
                state.chain_sync.protect = protect;
            }).or_insert_with(|| {
                let mut chain_sync = ChainSyncState::default();
                chain_sync.protect = protect;
                PeerState {
                    negotiate: Negotiate::default(),
                    sync_started: true,
                    last_block_announcement: None,
                    headers_sync_timeout: Some(headers_sync_timeout),
                    disconnect: false,
                    chain_sync,
                }
            });
    }

    pub fn best_known_header(&self, peer: PeerIndex) -> Option<HeaderView> {
        self.best_known_headers.read().get(&peer).cloned()
    }

    pub fn new_header_received(&self, peer: PeerIndex, header_view: &HeaderView) {
        self.best_known_headers
            .write()
            .entry(peer)
            .and_modify(|hv| {
                if header_view.total_difficulty() > hv.total_difficulty()
                    || (header_view.total_difficulty() == hv.total_difficulty()
                        && header_view.hash() < hv.hash())
                {
                    *hv = header_view.clone();
                }
            }).or_insert_with(|| header_view.clone());
    }

    pub fn getheaders_received(&self, _peer: PeerIndex) {
        // TODO:
    }

    pub fn connected(&self, peer: PeerIndex) {
        self.state.write().entry(peer).or_insert_with(|| PeerState {
            negotiate: Negotiate::default(),
            sync_started: true,
            last_block_announcement: None,
            headers_sync_timeout: None,
            disconnect: false,
            chain_sync: ChainSyncState::default(),
        });
    }

    pub fn disconnected(&self, peer: PeerIndex) {
        self.state.write().remove(&peer);
        self.best_known_headers.write().remove(&peer);
        // self.misbehavior.write().remove(peer);
        self.blocks_inflight.write().remove(&peer);
        self.last_common_headers.write().remove(&peer);
    }

    pub fn block_received(&self, peer: PeerIndex, block: &Block) {
        let mut blocks_inflight = self.blocks_inflight.write();
        debug!(target: "sync", "block_received from peer {} {} {:?}", peer, block.header().number(), block.header().hash());
        blocks_inflight.entry(peer).and_modify(|inflight| {
            inflight.remove(&block.header().hash());
            inflight.update_timestamp();
        });
    }

    pub fn set_last_common_header(&self, peer: PeerIndex, header: &Header) {
        let mut last_common_headers = self.last_common_headers.write();
        last_common_headers
            .entry(peer)
            .and_modify(|last_common_header| *last_common_header = header.clone())
            .or_insert_with(|| header.clone());
    }
}
