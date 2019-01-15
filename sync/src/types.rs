use bloom_filters::{
    BloomFilter, ClassicBloomFilter, DefaultBuildHashKernels, UpdatableBloomFilter,
};
use ckb_core::block::Block;
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::Transaction;
use ckb_network::PeerIndex;
use ckb_util::RwLock;
use faketime::unix_time_as_millis;
use fnv::{FnvHashMap, FnvHashSet};
use log::debug;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::hash::{BuildHasher, Hasher};

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
    pub work_header: Option<(Header, U256)>,
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
    pub sync_started: bool,
    pub last_block_announcement: Option<u64>, //ms
    pub headers_sync_timeout: Option<u64>,
    pub disconnect: bool,
    pub chain_sync: ChainSyncState,
}

#[derive(Default)]
pub struct Peers {
    pub state: RwLock<FnvHashMap<PeerIndex, PeerState>>,
    pub misbehavior: RwLock<FnvHashMap<PeerIndex, u32>>,
    pub blocks_inflight: RwLock<FnvHashMap<PeerIndex, BlocksInflight>>,
    pub best_known_headers: RwLock<FnvHashMap<PeerIndex, HeaderView>>,
    pub last_common_headers: RwLock<FnvHashMap<PeerIndex, Header>>,
    pub transaction_filters: RwLock<FnvHashMap<PeerIndex, TransactionFilter>>,
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
                PeerState {
                    sync_started: false,
                    last_block_announcement: None,
                    headers_sync_timeout: Some(predicted_headers_sync_time),
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
            })
            .or_insert_with(|| header_view.clone());
    }

    pub fn getheaders_received(&self, _peer: PeerIndex) {
        // TODO:
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

pub struct TransactionFilter {
    filter: ClassicBloomFilter<DefaultBuildHashKernels<HighLowBytesBuildHasher>>,
}

impl TransactionFilter {
    pub fn new(raw_data: &[u8], k: usize, hash_seed: usize) -> Self {
        Self {
            filter: ClassicBloomFilter::with_raw_data(
                raw_data,
                k,
                DefaultBuildHashKernels::new(hash_seed, HighLowBytesBuildHasher),
            ),
        }
    }

    pub fn update(&mut self, raw_data: &[u8]) {
        self.filter.update(raw_data)
    }

    pub fn insert(&mut self, hash: &H256) {
        self.filter.insert(hash);
    }

    pub fn contains(&self, transaction: &Transaction) -> bool {
        self.filter.contains(&transaction.hash())
            || transaction.inputs().iter().any(|input| {
                self.filter.contains(&input.previous_output.hash)
                    || self.filter.contains(&input.unlock.type_hash())
            })
            || transaction
                .outputs()
                .iter()
                .any(|output| self.filter.contains(&output.lock))
    }
}

struct HighLowBytesBuildHasher;

impl BuildHasher for HighLowBytesBuildHasher {
    type Hasher = HighLowBytesHasher;

    fn build_hasher(&self) -> Self::Hasher {
        HighLowBytesHasher(0)
    }
}

/// a hasher which only accepts H256 bytes and use high / low bytes as hash value
struct HighLowBytesHasher(u64);

impl Hasher for HighLowBytesHasher {
    fn write(&mut self, bytes: &[u8]) {
        if bytes.len() == 32 {
            self.0 = (u64::from(bytes[0]) << 56)
                + (u64::from(bytes[1]) << 48)
                + (u64::from(bytes[2]) << 40)
                + (u64::from(bytes[3]) << 32)
                + (u64::from(bytes[28]) << 24)
                + (u64::from(bytes[29]) << 16)
                + (u64::from(bytes[30]) << 8)
                + u64::from(bytes[31]);
        }
    }

    fn finish(&self) -> u64 {
        println!("finish {:?}", self.0);
        self.0
    }
}
