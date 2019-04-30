use crate::NetworkProtocol;
use crate::{MAX_HEADERS_LEN, MAX_TIP_AGE};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::extras::BlockExt;
use ckb_core::header::{BlockNumber, Header};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::SyncMessage;
use ckb_shared::chain_state::ChainState;
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_traits::ChainProvider;
use ckb_util::Mutex;
use ckb_util::RwLock;
use faketime::unix_time_as_millis;
use flatbuffers::FlatBufferBuilder;
use fnv::{FnvHashMap, FnvHashSet};
use log::debug;
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::cmp;
use std::collections::{
    hash_map::{Entry, HashMap},
    hash_set::HashSet,
    BTreeMap,
};
use std::time::{Duration, Instant};

const FILTER_SIZE: usize = 20000;
const MAX_ASK_MAP_SIZE: usize = 30000;
const MAX_ASK_SET_SIZE: usize = MAX_ASK_MAP_SIZE * 2;

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
    pub protect: bool,
}

impl Default for ChainSyncState {
    fn default() -> Self {
        ChainSyncState {
            timeout: 0,
            work_header: None,
            total_difficulty: None,
            sent_getheaders: false,
            protect: false,
        }
    }
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct PeerState {
    pub sync_started: bool,
    pub last_block_announcement: Option<u64>, //ms
    pub headers_sync_timeout: Option<u64>,
    pub disconnect: bool,
    pub chain_sync: ChainSyncState,
    // The key is a `timeout`, means do not ask the tx before `timeout`.
    tx_ask_for_map: BTreeMap<Instant, Vec<H256>>,
    tx_ask_for_set: HashSet<H256>,
}

impl PeerState {
    pub fn new(headers_sync_timeout: Option<u64>, chain_sync: ChainSyncState) -> PeerState {
        PeerState {
            sync_started: false,
            last_block_announcement: None,
            headers_sync_timeout,
            disconnect: false,
            chain_sync,
            tx_ask_for_map: BTreeMap::default(),
            tx_ask_for_set: HashSet::default(),
        }
    }

    pub fn add_ask_for_tx(
        &mut self,
        tx_hash: H256,
        last_ask_timeout: Option<Instant>,
    ) -> Option<Instant> {
        if self.tx_ask_for_map.len() > MAX_ASK_MAP_SIZE {
            debug!(target: "relay", "this peer tx_ask_for_map is full, ignore {:#x}", tx_hash);
            return None;
        }
        if self.tx_ask_for_set.len() > MAX_ASK_SET_SIZE {
            debug!(target: "relay", "this peer tx_ask_for_set is full, ignore {:#x}", tx_hash);
            return None;
        }
        // This peer already register asked for this tx
        if self.tx_ask_for_set.contains(&tx_hash) {
            debug!(target: "relay", "this peer already register ask tx({:#x})", tx_hash);
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

#[derive(Clone, Default)]
pub struct KnownFilter {
    inner: FnvHashMap<PeerIndex, LruCache<H256, ()>>,
}

impl KnownFilter {
    /// Adds a value to the filter.
    /// If the filter did not have this value present, `true` is returned.
    /// If the filter did have this value present, `false` is returned.
    pub fn insert(&mut self, index: PeerIndex, hash: H256) -> bool {
        match self.inner.entry(index) {
            Entry::Occupied(mut o) => o.get_mut().insert(hash, ()).is_none(),
            Entry::Vacant(v) => {
                let mut lru = LruCache::new(FILTER_SIZE);
                lru.insert(hash, ());
                v.insert(lru);
                true
            }
        }
    }
}

#[derive(Default)]
pub struct Peers {
    pub state: RwLock<FnvHashMap<PeerIndex, PeerState>>,
    pub misbehavior: RwLock<FnvHashMap<PeerIndex, u32>>,
    pub blocks_inflight: RwLock<FnvHashMap<PeerIndex, BlocksInflight>>,
    pub best_known_headers: RwLock<FnvHashMap<PeerIndex, HeaderView>>,
    pub last_common_headers: RwLock<FnvHashMap<PeerIndex, Header>>,
    pub known_txs: Mutex<KnownFilter>,
    pub known_blocks: Mutex<KnownFilter>,
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
            timestamp: unix_time_as_millis(),
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
        self.timestamp = unix_time_as_millis();
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

    pub fn on_connected(&self, peer: PeerIndex, predicted_headers_sync_time: u64, protect: bool) {
        self.state
            .write()
            .entry(peer)
            .and_modify(|state| {
                state.headers_sync_timeout = Some(predicted_headers_sync_time);
                state.chain_sync.protect = protect;
            })
            .or_insert_with(|| {
                let mut chain_sync = ChainSyncState::default();
                chain_sync.protect = protect;
                PeerState::new(Some(predicted_headers_sync_time), chain_sync)
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
            })
            .or_insert_with(|| header_view.clone());
    }

    pub fn getheaders_received(&self, _peer: PeerIndex) {
        // TODO:
    }

    pub fn disconnected(&self, peer: PeerIndex) {
        self.best_known_headers.write().remove(&peer);
        // self.misbehavior.write().remove(peer);
        self.blocks_inflight.write().remove(&peer);
        self.last_common_headers.write().remove(&peer);
    }

    pub fn block_received(&self, peer: PeerIndex, block: &Block) {
        let mut blocks_inflight = self.blocks_inflight.write();
        debug!(target: "sync", "block_received from peer {} {} {:x}", peer, block.header().number(), block.header().hash());
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeaderView {
    inner: Header,
    total_difficulty: U256,
    total_uncles_count: u64,
}

impl HeaderView {
    pub fn new(inner: Header, total_difficulty: U256, total_uncles_count: u64) -> Self {
        HeaderView {
            inner,
            total_difficulty,
            total_uncles_count,
        }
    }

    pub fn number(&self) -> BlockNumber {
        self.inner.number()
    }

    pub fn hash(&self) -> H256 {
        self.inner.hash()
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
}

pub struct SyncSharedState<CS> {
    shared: Shared<CS>,
    header_map: RwLock<HashMap<H256, HeaderView>>,
    best_known_header: RwLock<HeaderView>,
}

impl<CS: ChainStore> SyncSharedState<CS> {
    pub fn new(shared: Shared<CS>) -> SyncSharedState<CS> {
        let (total_difficulty, header, total_uncles_count) = {
            let chain_state = shared.chain_state().lock();
            let block_ext = shared
                .block_ext(&chain_state.tip_hash())
                .expect("tip block_ext must exist");
            (
                chain_state.total_difficulty().to_owned(),
                chain_state.tip_header().to_owned(),
                block_ext.total_uncles_count,
            )
        };
        let best_known_header = RwLock::new(HeaderView::new(
            header,
            total_difficulty,
            total_uncles_count,
        ));
        let header_map = RwLock::new(HashMap::new());
        SyncSharedState {
            shared,
            header_map,
            best_known_header,
        }
    }

    pub fn shared(&self) -> &Shared<CS> {
        &self.shared
    }
    pub fn chain_state(&self) -> &Mutex<ChainState<CS>> {
        self.shared.chain_state()
    }
    pub fn block_header(&self, hash: &H256) -> Option<Header> {
        self.shared.block_header(hash)
    }
    pub fn block_ext(&self, hash: &H256) -> Option<BlockExt> {
        self.shared.block_ext(hash)
    }
    pub fn block_hash(&self, number: BlockNumber) -> Option<H256> {
        self.shared.block_hash(number)
    }
    pub fn get_block(&self, hash: &H256) -> Option<Block> {
        self.shared.block(hash)
    }
    pub fn tip_header(&self) -> Header {
        self.shared.chain_state().lock().tip_header().to_owned()
    }
    pub fn consensus(&self) -> &Consensus {
        self.shared.consensus()
    }
    pub fn is_initial_block_download(&self) -> bool {
        unix_time_as_millis()
            .saturating_sub(self.shared.chain_state().lock().tip_header().timestamp())
            > MAX_TIP_AGE
    }

    pub fn best_known_header(&self) -> HeaderView {
        self.best_known_header.read().to_owned()
    }
    pub fn set_best_known_header(&self, header: HeaderView) {
        *self.best_known_header.write() = header;
    }

    pub fn insert_header_view(&self, hash: H256, header: HeaderView) {
        self.header_map.write().insert(hash, header);
    }
    pub fn remove_header_view(&self, hash: &H256) {
        self.header_map.write().remove(hash);
    }
    pub fn get_header_view(&self, hash: &H256) -> Option<HeaderView> {
        self.header_map.read().get(hash).cloned().or_else(|| {
            self.shared.block_header(hash).and_then(|header| {
                self.shared.block_ext(&hash).map(|block_ext| {
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
            .or_else(|| self.shared.block_header(hash))
    }

    pub fn get_ancestor(&self, base: &H256, number: BlockNumber) -> Option<Header> {
        if let Some(header) = self.get_header(base) {
            let mut n_number = header.number();
            let mut index_walk = header;
            if number > n_number {
                return None;
            }

            while n_number > number {
                if let Some(header) = self.get_header(&index_walk.parent_hash()) {
                    index_walk = header;
                    n_number -= 1;
                } else {
                    return None;
                }
            }
            return Some(index_walk);
        }
        None
    }

    pub fn get_locator(&self, start: &Header) -> Vec<H256> {
        let mut step = 1;
        let mut locator = Vec::with_capacity(32);
        let mut index = start.number();
        let base = start.hash();
        loop {
            let header = self
                .get_ancestor(&base, index)
                .expect("index calculated in get_locator");
            locator.push(header.hash());

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
            .map(|(index, hash)| (index, self.shared.block_number(hash)))
            .find(|(_index, number)| number.is_some())
            .expect("locator last checked");

        if index == 0 || latest_common == Some(0) {
            return latest_common;
        }

        if let Some(header) = locator
            .get(index - 1)
            .and_then(|hash| self.shared.block_header(hash))
        {
            let mut block_hash = header.parent_hash().to_owned();
            loop {
                let block_header = match self.shared.block_header(&block_hash) {
                    None => break latest_common,
                    Some(block_header) => block_header,
                };

                if let Some(block_number) = self.shared.block_number(&block_hash) {
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
            .filter_map(|block_number| self.shared.block_hash(block_number))
            .take_while(|block_hash| block_hash != hash_stop)
            .filter_map(|block_hash| self.shared.block_header(&block_hash))
            .collect()
    }

    pub fn send_getheaders_to_peer(
        &self,
        nc: &CKBProtocolContext,
        peer: PeerIndex,
        header: &Header,
    ) {
        debug!(target: "sync", "send_getheaders_to_peer peer={}, hash={}", peer, header.hash());
        let locator_hash = self.get_locator(header);
        let fbb = &mut FlatBufferBuilder::new();
        let message = SyncMessage::build_get_headers(fbb, &locator_hash);
        fbb.finish(message, None);
        nc.send_message(
            NetworkProtocol::SYNC.into(),
            peer,
            fbb.finished_data().into(),
        );
    }
}
