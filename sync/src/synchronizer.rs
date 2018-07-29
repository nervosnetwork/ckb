use bigint::H256;
use block_pool::OrphanBlockPool;
use ckb_chain::chain::{ChainProvider, TipHeader};
use ckb_notify::Notify;
use ckb_time::now_ms;
use ckb_verification::{BlockVerifier, EthashVerifier, Verifier};
use config::Config;
use core::block::IndexedBlock;
use core::header::{BlockNumber, IndexedHeader};
use header_view::HeaderView;
use network::PeerId;
use peers::Peers;
use std::cmp;
use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use util::{RwLock, RwLockUpgradableReadGuard};
use {
    BLOCK_DOWNLOAD_WINDOW, MAX_BLOCKS_IN_TRANSIT_PER_PEER, MAX_HEADERS_LEN, MAX_TIP_AGE,
    PER_FETCH_BLOCK_LIMIT,
};

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

pub type BlockStatusMap = Arc<RwLock<HashMap<H256, BlockStatus>>>;
pub type BlockHeaderMap = Arc<RwLock<HashMap<H256, HeaderView>>>;

pub struct Synchronizer<C> {
    pub chain: Arc<C>,
    pub status_map: BlockStatusMap,
    pub header_map: BlockHeaderMap,
    pub notify: Notify,
    pub best_known_header: Arc<RwLock<HeaderView>>,
    pub n_sync: Arc<AtomicUsize>,
    pub peers: Arc<Peers>,
    pub config: Arc<Config>,
    pub ethash: Option<EthashVerifier>,
    pub orphan_block_pool: Arc<OrphanBlockPool>,
}

impl<C> Clone for Synchronizer<C>
where
    C: ChainProvider,
{
    fn clone(&self) -> Synchronizer<C> {
        Synchronizer {
            chain: Arc::clone(&self.chain),
            status_map: Arc::clone(&self.status_map),
            header_map: Arc::clone(&self.header_map),
            notify: self.notify.clone(),
            best_known_header: Arc::clone(&self.best_known_header),
            n_sync: Arc::clone(&self.n_sync),
            peers: Arc::clone(&self.peers),
            ethash: self.ethash.clone(),
            config: Arc::clone(&self.config),
            orphan_block_pool: Arc::clone(&self.orphan_block_pool),
        }
    }
}

impl<C> Synchronizer<C>
where
    C: ChainProvider,
{
    pub fn new(
        chain: &Arc<C>,
        notify: Notify,
        ethash: Option<EthashVerifier>,
        config: Config,
    ) -> Synchronizer<C> {
        let TipHeader {
            header,
            total_difficulty,
            ..
        } = chain.tip_header().read().clone();
        let best_known_header = HeaderView::new(header, total_difficulty);
        let orphan_block_limit = config.orphan_block_limit;

        Synchronizer {
            ethash,
            notify,
            config: Arc::new(config),
            chain: Arc::clone(chain),
            peers: Arc::new(Peers::default()),
            orphan_block_pool: Arc::new(OrphanBlockPool::with_capacity(orphan_block_limit)),
            best_known_header: Arc::new(RwLock::new(best_known_header)),
            status_map: Arc::new(RwLock::new(HashMap::new())),
            header_map: Arc::new(RwLock::new(HashMap::new())),
            n_sync: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn get_block_status(&self, hash: &H256) -> BlockStatus {
        let guard = self.status_map.read();
        match guard.get(hash).cloned() {
            Some(s) => s,
            None => if self.chain.block_header(hash).is_some() {
                BlockStatus::BLOCK_HAVE_MASK
            } else {
                BlockStatus::UNKNOWN
            },
        }
    }

    pub fn insert_block_status(&self, hash: H256, status: BlockStatus) {
        self.status_map.write().insert(hash, status);
    }

    pub fn best_known_header(&self) -> HeaderView {
        self.best_known_header.read().clone()
    }

    pub fn is_initial_block_download(&self) -> bool {
        now_ms().saturating_sub(self.chain.tip_header().read().header.timestamp) > MAX_TIP_AGE
    }

    pub fn tip_header(&self) -> IndexedHeader {
        self.chain.tip_header().read().header.clone()
    }

    // pub fn best_known_header(&self) -> HeaderView {

    // }

    pub fn get_locator(&self, start: &IndexedHeader) -> Vec<H256> {
        let mut step = 1;
        let mut locator = Vec::with_capacity(32);
        let mut index = start.number;
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
                    locator.push(self.chain.genesis_hash());
                }
                break;
            }
            index -= step;
        }
        locator
    }

    pub fn locate_latest_common_block(
        &self,
        _hash_stop: &H256,
        locator: &[H256],
    ) -> Option<BlockNumber> {
        if locator.is_empty() {
            return None;
        }

        if locator.last().expect("empty checked") != &self.chain.genesis_hash() {
            return None;
        }

        // iterator are lazy
        let (index, latest_common) = locator
            .iter()
            .enumerate()
            .map(|(index, hash)| (index, self.chain.block_number(hash)))
            .find(|(_index, number)| number.is_some())
            .expect("locator last checked");

        if index == 0 || latest_common == Some(0) {
            return latest_common;
        }

        if let Some(header) = locator
            .get(index - 1)
            .and_then(|hash| self.chain.block_header(&hash))
        {
            let mut block_hash = header.parent_hash;
            loop {
                let block_header = match self.chain.block_header(&block_hash) {
                    None => break latest_common,
                    Some(block_header) => block_header,
                };

                if let Some(block_number) = self.chain.block_number(&block_hash) {
                    return Some(block_number);
                }

                block_hash = block_header.parent_hash;
            }
        } else {
            latest_common
        }
    }

    pub fn get_header_view(&self, hash: &H256) -> Option<HeaderView> {
        self.header_map.read().get(hash).cloned().or_else(|| {
            self.chain.block_header(&hash).and_then(|header| {
                self.chain
                    .block_ext(&hash)
                    .map(|block_ext| HeaderView::new(header, block_ext.total_difficulty))
            })
        })
    }

    pub fn get_header(&self, hash: &H256) -> Option<IndexedHeader> {
        self.header_map
            .read()
            .get(hash)
            .map(|view| &view.header)
            .cloned()
            .or_else(|| self.chain.block_header(&hash))
    }

    pub fn get_number(&self, hash: &H256) -> Option<BlockNumber> {
        self.chain.block_number(hash)
    }

    pub fn get_hash(&self, number: BlockNumber) -> Option<H256> {
        self.chain.block_hash(number)
    }

    pub fn get_block(&self, hash: &H256) -> Option<IndexedBlock> {
        self.chain.block(hash)
    }

    pub fn get_ancestor(&self, base: &H256, number: BlockNumber) -> Option<IndexedHeader> {
        if let Some(header) = self.get_header(base) {
            let mut n_number = header.number;
            let mut index_walk = header;
            if number > n_number {
                return None;
            }

            while n_number > number {
                if let Some(header) = self.get_header(&index_walk.parent_hash) {
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

    pub fn get_locator_response(
        &self,
        block_number: BlockNumber,
        hash_stop: &H256,
    ) -> Vec<IndexedHeader> {
        let tip_number = self.tip_header().number;
        let max_height = cmp::min(
            block_number + 1 + MAX_HEADERS_LEN as BlockNumber,
            tip_number + 1,
        );
        (block_number + 1..max_height)
            .filter_map(|block_number| self.chain.block_hash(block_number))
            .take_while(|block_hash| block_hash != hash_stop)
            .filter_map(|block_hash| self.chain.block_header(&block_hash))
            .collect()
    }

    pub fn insert_header_view(&self, header: &IndexedHeader, peer: &PeerId) {
        if let Some(parent_view) = self.get_header_view(&header.parent_hash) {
            let total_difficulty = parent_view.total_difficulty + header.difficulty;
            let header_view = {
                let best_known_header = self.best_known_header.upgradable_read();
                let header_view = HeaderView::new(header.clone(), total_difficulty);

                if total_difficulty > best_known_header.total_difficulty
                    || (total_difficulty == best_known_header.total_difficulty
                        && header.hash() < best_known_header.header.hash())
                {
                    let mut best_known_header =
                        RwLockUpgradableReadGuard::upgrade(best_known_header);
                    *best_known_header = header_view.clone();
                }
                header_view
            };

            self.peers.new_header_received(peer, &header_view);
            self.insert_block_status(header.hash(), BlockStatus::VALID_MASK);

            let mut header_map = self.header_map.write();
            header_map.insert(header.hash(), header_view);

            // debug!(target: "sync", "\n\nheader_view");
            // for (k, v) in header_map.iter() {
            //     debug!(target: "sync", "   {} => {:?}", k, v);
            // }
            // debug!(target: "sync", "header_view\n\n");
        }
    }

    // If the peer reorganized, our previous last_common_header may not be an ancestor
    // of its current best_known_header. Go back enough to fix that.
    pub fn last_common_ancestor(
        &self,
        last_common_header: &IndexedHeader,
        best_known_header: &IndexedHeader,
    ) -> Option<IndexedHeader> {
        debug_assert!(best_known_header.number >= last_common_header.number);

        let mut m_right =
            try_option!(self.get_ancestor(&best_known_header.hash(), last_common_header.number));

        if &m_right == last_common_header {
            return Some(m_right);
        }

        let mut m_left = try_option!(self.get_header(&last_common_header.hash()));
        debug_assert!(m_right.header.number == m_left.header.number);

        while m_left != m_right {
            m_left =
                try_option!(self.get_ancestor(&m_left.header.hash(), m_left.header.number - 1));
            m_right =
                try_option!(self.get_ancestor(&m_right.header.hash(), m_right.header.number - 1));
        }
        Some(m_left)
    }

    // fn verification_level(&self) -> VerificationLevel {
    //     if self.config.verification_level == "Full" {
    //         VerificationLevel::Full
    //     } else if self.config.verification_level == "Header" {
    //         VerificationLevel::Header
    //     } else {
    //         VerificationLevel::Noop
    //     }
    // }

    //TODO: process block which we don't request
    #[cfg_attr(feature = "cargo-clippy", allow(single_match))]
    pub fn process_new_block(&self, _peer: PeerId, block: IndexedBlock) {
        match self.get_block_status(&block.hash()) {
            BlockStatus::VALID_MASK => {
                let verify = BlockVerifier::new(&block, &self.chain).verify();
                if verify.is_ok() {
                    self.insert_new_block(block);
                } else {
                    warn!(target: "sync", "[Synchronizer] process_new_block {:#?} verifier error {:?}", block, verify.unwrap_err());
                }
            }
            status => {
                info!(target: "sync", "[Synchronizer] process_new_block unexpect status {:?}", status);
            }
        }
    }

    //FIXME: guarantee concurrent block process
    fn insert_new_block(&self, block: IndexedBlock) {
        if self.chain.output_root(&block.header.parent_hash).is_some() {
            let process_ret = self.chain.process_block(&block);
            if process_ret.is_ok() {
                let pre_orphan_block = self
                    .orphan_block_pool
                    .remove_blocks_by_parent(&block.hash());
                for block in pre_orphan_block {
                    if self.chain.output_root(&block.header.parent_hash).is_some() {
                        let ret = self.chain.process_block(&block);
                        if ret.is_err() {
                            info!(
                                target: "sync", "[Synchronizer] insert_new_block {:#?} error {:?}",
                                block,
                                ret.unwrap_err()
                            );
                        }
                    } else {
                        self.orphan_block_pool.insert(block);
                    }
                }
            } else {
                info!(
                    target: "sync", "[Synchronizer] insert_new_block {:#?} error {:?}",
                    block,
                    process_ret.unwrap_err()
                )
            }
        } else {
            self.orphan_block_pool.insert(block);
        }
    }

    pub fn get_blocks_to_fetch(&self, peer: PeerId) -> Option<Vec<H256>> {
        debug!(target: "sync", "[block downloader] process");
        BlockFetcher::new(&self, peer).fetch()
    }
}

pub struct BlockFetcher<C> {
    synchronizer: Synchronizer<C>,
    peer: PeerId,
}

impl<C> BlockFetcher<C>
where
    C: ChainProvider,
{
    pub fn new(synchronizer: &Synchronizer<C>, peer: PeerId) -> Self {
        BlockFetcher {
            peer,
            synchronizer: synchronizer.clone(),
        }
    }
    pub fn inflight_limit_reach(&self) -> bool {
        let mut blocks_inflight = self.synchronizer.peers.blocks_inflight.write();
        let inflight_count = blocks_inflight
            .entry(self.peer)
            .or_insert_with(Default::default)
            .len();

        // current peer block blocks_inflight reach limit
        if MAX_BLOCKS_IN_TRANSIT_PER_PEER.saturating_sub(inflight_count) == 0 {
            debug!(target: "sync", "[block downloader] inflight count reach limit");
            true
        } else {
            false
        }
    }

    pub fn peer_best_known_header(&self) -> Option<HeaderView> {
        self.synchronizer
            .peers
            .best_known_headers
            .read()
            .get(&self.peer)
            .cloned()
    }

    pub fn latest_common_height(&self, header: &HeaderView) -> Option<BlockNumber> {
        let chain_tip = self.synchronizer.chain.tip_header().read();
        //if difficulty of this peer less than our
        if header.total_difficulty < chain_tip.total_difficulty {
            debug!(
                target: "sync",
                "[block downloader] best_known_header {} chain {}",
                header.total_difficulty,
                chain_tip.total_difficulty
            );
            None
        } else {
            Some(cmp::min(header.header.number, chain_tip.header.number))
        }
    }

    // this peer's tip is wherethe the ancestor of global_best_known_header
    pub fn is_known_best(&self, header: &HeaderView) -> bool {
        let global_best_known_header = { self.synchronizer.best_known_header.read().clone() };
        if let Some(ancestor) = self.synchronizer.get_ancestor(
            &global_best_known_header.header.hash(),
            header.header.number,
        ) {
            if ancestor != header.header {
                debug!(
                    target: "sync",
                    "[block downloader] peer best_known_header is not ancestor of global_best_known_header"
                );
                return false;
            }
        } else {
            return false;
        }
        true
    }

    fn fetch(self) -> Option<Vec<H256>> {
        debug!(target: "sync", "[block downloader] BlockFetcher process");

        if self.inflight_limit_reach() {
            debug!(target: "sync", "[block downloader] inflight count reach limit");
            return None;
        }

        let best_known_header = match self.peer_best_known_header() {
            Some(best_known_header) => best_known_header,
            _ => {
                debug!(target: "sync", "[block downloader] peer_best_known_header not found peer={}", self.peer);
                return None;
            }
        };

        let latest_common_height = try_option!(self.latest_common_height(&best_known_header));

        if !self.is_known_best(&best_known_header) {
            return None;
        }

        let latest_common_hash =
            try_option!(self.synchronizer.chain.block_hash(latest_common_height));
        let latest_common_header =
            try_option!(self.synchronizer.chain.block_header(&latest_common_hash));

        // If the peer reorganized, our previous last_common_header may not be an ancestor
        // of its current best_known_header. Go back enough to fix that.
        let fixed_latest_common_header = try_option!(
            self.synchronizer
                .last_common_ancestor(&latest_common_header, &best_known_header.header)
        );

        if fixed_latest_common_header == best_known_header.header {
            debug!(target: "sync", "[block downloader] fixed_latest_common_header == best_known_header");
            return None;
        }

        debug!(target: "sync", "[block downloader] fixed_latest_common_header = {}", fixed_latest_common_header.number);

        debug_assert!(best_known_header.header.number > fixed_latest_common_header.number);

        let window_end = fixed_latest_common_header.number + BLOCK_DOWNLOAD_WINDOW;
        let max_height = cmp::min(window_end + 1, best_known_header.header.number);

        let mut n_height = fixed_latest_common_header.number;
        let mut v_fetch = Vec::with_capacity(PER_FETCH_BLOCK_LIMIT);

        {
            let mut guard = self.synchronizer.peers.blocks_inflight.write();
            let inflight = guard.get_mut(&self.peer).expect("inflight already init");

            while n_height < max_height && v_fetch.len() < PER_FETCH_BLOCK_LIMIT {
                n_height += 1;
                let to_fetch = try_option!(
                    self.synchronizer
                        .get_ancestor(&best_known_header.header.hash(), n_height)
                );
                let to_fetch_hash = to_fetch.header.hash();

                if inflight.insert(to_fetch_hash) {
                    v_fetch.push(to_fetch_hash);
                }
            }
        }
        Some(v_fetch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bigint::U256;
    use ckb_chain::chain::Chain;
    use ckb_chain::consensus::Consensus;
    use ckb_chain::index::ChainIndex;
    use ckb_chain::store::ChainKVStore;
    use ckb_chain::COLUMNS;
    use ckb_notify::Notify;
    use ckb_protocol::{self, Payload};
    use ckb_time::now_ms;
    use core::difficulty::cal_difficulty;
    use core::header::{Header, RawHeader, Seal};
    use db::memorydb::MemoryKeyValueDB;
    use headers_process::HeadersProcess;
    use network::NetworkContextExt;
    use network::{
        Error as NetworkError, NetworkContext, PacketId, PeerId, ProtocolId, SessionInfo, Severity,
        TimerToken,
    };
    use protobuf::RepeatedField;
    use std::time::Duration;

    #[test]
    fn test_block_status() {
        let status1 = BlockStatus::FAILED_VALID;
        let status2 = BlockStatus::FAILED_CHILD;
        assert!((status1 & BlockStatus::FAILED_MASK) == status1);
        assert!((status2 & BlockStatus::FAILED_MASK) == status2);
    }

    fn gen_chain(consensus: &Consensus) -> Chain<ChainKVStore<MemoryKeyValueDB>> {
        let db = MemoryKeyValueDB::open(COLUMNS as usize);
        let store = ChainKVStore { db };
        let chain = Chain::init(store, consensus.clone(), Notify::default()).unwrap();
        chain
    }

    fn gen_block(parent_header: IndexedHeader, difficulty: U256) -> IndexedBlock {
        let time = now_ms();
        let nonce = parent_header.seal.nonce + 1;
        let header = Header {
            raw: RawHeader {
                number: parent_header.number + 1,
                version: 0,
                parent_hash: parent_header.hash(),
                timestamp: time,
                txs_commit: H256::from(0),
                difficulty: difficulty,
            },
            seal: Seal {
                nonce,
                mix_hash: H256::from(nonce),
            },
        };

        IndexedBlock {
            header: header.into(),
            transactions: vec![],
        }
    }

    fn insert_block<CS: ChainIndex>(chain: &Chain<CS>, nonce: u64, number: BlockNumber) {
        let parent = chain
            .block_header(&chain.block_hash(number - 1).unwrap())
            .unwrap();
        let now = 1 + parent.timestamp;
        let difficulty = cal_difficulty(&parent, now);
        let header = Header {
            raw: RawHeader {
                number,
                version: 0,
                parent_hash: parent.hash(),
                timestamp: now,
                txs_commit: H256::from(0),
                difficulty: difficulty,
            },
            seal: Seal {
                nonce,
                mix_hash: H256::from(nonce),
            },
        };

        let block = IndexedBlock {
            header: header.into(),
            transactions: vec![],
        };
        chain.process_block(&block).expect("process block ok");
    }

    #[test]
    fn test_locator() {
        let config = Consensus::default();
        let chain = Arc::new(gen_chain(&config));

        let num = 200;
        let index = [
            199, 198, 197, 196, 195, 194, 193, 192, 191, 190, 188, 184, 176, 160, 128, 64,
        ];

        for i in 1..num {
            insert_block(&chain, i, i);
        }

        let synchronizer = Synchronizer::new(&chain, Notify::default(), None, Config::default());

        let locator = synchronizer.get_locator(&chain.tip_header().read().header);

        let mut expect = Vec::new();

        for i in index.iter() {
            expect.push(chain.block_hash(*i).unwrap());
        }
        //genesis_hash must be the last one
        expect.push(chain.genesis_hash());

        assert_eq!(expect, locator);
    }

    #[test]
    fn test_locate_latest_common_block() {
        let config = Consensus::default();
        let chain1 = Arc::new(gen_chain(&config));
        let chain2 = Arc::new(gen_chain(&config));
        let num = 200;

        for i in 1..num {
            insert_block(&chain1, i, i);
        }

        for i in 1..num {
            insert_block(&chain2, i + 1, i);
        }

        let synchronizer1 = Synchronizer::new(&chain1, Notify::default(), None, Config::default());

        let synchronizer2 = Synchronizer::new(&chain2, Notify::default(), None, Config::default());

        let locator1 = synchronizer1.get_locator(&chain1.tip_header().read().header);

        let latest_common = synchronizer2.locate_latest_common_block(&H256::zero(), &locator1[..]);

        assert_eq!(latest_common, Some(0));

        let chain3 = Arc::new(gen_chain(&config));

        for i in 1..num {
            let j = if i > 192 { i + 1 } else { i };
            insert_block(&chain3, j, i);
        }

        let synchronizer3 = Synchronizer::new(&chain3, Notify::default(), None, Config::default());

        let latest_common3 = synchronizer3.locate_latest_common_block(&H256::zero(), &locator1[..]);
        assert_eq!(latest_common3, Some(192));
    }

    #[test]
    fn test_locate_latest_common_block2() {
        let config = Consensus::default();
        let chain1 = Arc::new(gen_chain(&config));
        let chain2 = Arc::new(gen_chain(&config));
        let block_number = 200;

        let mut blocks: Vec<IndexedBlock> = Vec::new();
        let mut parent = config.genesis_block().header.clone();
        for _ in 1..block_number {
            let difficulty = parent.header.difficulty;
            let new_block = gen_block(parent, difficulty + U256::from(100));
            blocks.push(new_block.clone());
            parent = new_block.header;
        }

        for block in &blocks {
            chain1.process_block(&block).expect("process block ok");
        }

        for block in &blocks {
            chain2.process_block(&block).expect("process block ok");
        }

        parent = blocks[150].header.clone();
        let fork = parent.number;
        for _ in 1..block_number + 1 {
            let difficulty = parent.header.difficulty;
            let new_block = gen_block(parent, difficulty + U256::from(200));
            chain2.process_block(&new_block).expect("process block ok");
            parent = new_block.header;
        }

        let synchronizer1 = Synchronizer::new(&chain1, Notify::default(), None, Config::default());

        let synchronizer2 = Synchronizer::new(&chain2, Notify::default(), None, Config::default());

        let locator1 = synchronizer1.get_locator(&chain1.tip_header().read().header);

        let latest_common = synchronizer2
            .locate_latest_common_block(&H256::zero(), &locator1[..])
            .unwrap();

        assert_eq!(
            chain1.block_hash(fork).unwrap(),
            chain2.block_hash(fork).unwrap()
        );
        assert!(chain1.block_hash(fork + 1).unwrap() != chain2.block_hash(fork + 1).unwrap());
        assert_eq!(
            chain1.block_hash(latest_common).unwrap(),
            chain1.block_hash(fork).unwrap()
        );
    }

    #[test]
    fn test_get_ancestor() {
        let config = Consensus::default();
        let chain = Arc::new(gen_chain(&config));
        let num = 200;

        for i in 1..num {
            insert_block(&chain, i, i);
        }

        let synchronizer = Synchronizer::new(&chain, Notify::default(), None, Config::default());

        let header = synchronizer.get_ancestor(&chain.tip_header().read().header.hash(), 100);
        let tip = synchronizer.get_ancestor(&chain.tip_header().read().header.hash(), 199);
        let noop = synchronizer.get_ancestor(&chain.tip_header().read().header.hash(), 200);
        assert!(tip.is_some());
        assert!(header.is_some());
        assert!(noop.is_none());
        assert_eq!(tip.unwrap(), chain.tip_header().read().header.clone());
        assert_eq!(
            header.unwrap(),
            chain.block_header(&chain.block_hash(100).unwrap()).unwrap()
        );
    }

    #[test]
    fn test_process_new_block() {
        let config = Consensus::default();
        let chain = Arc::new(gen_chain(&config));
        let block_number = 2000;

        let mut blocks: Vec<IndexedBlock> = Vec::new();
        let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for _ in 1..block_number {
            let difficulty = parent.difficulty;
            let new_block = gen_block(parent, difficulty + U256::from(100));
            blocks.push(new_block.clone());
            parent = new_block.header;
        }

        let synchronizer = Synchronizer::new(&chain, Notify::default(), None, Config::default());

        blocks.clone().into_iter().for_each(|block| {
            synchronizer.insert_new_block(block);
        });

        assert_eq!(
            blocks.last().unwrap().header,
            chain.tip_header().read().header
        );
    }

    #[test]
    fn test_get_locator_response() {
        let config = Consensus::default();
        let chain = Arc::new(gen_chain(&config));
        let block_number = 200;

        let mut blocks: Vec<IndexedBlock> = Vec::new();
        let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for _ in 1..block_number + 1 {
            let difficulty = parent.difficulty;
            let new_block = gen_block(parent, difficulty + U256::from(100));
            blocks.push(new_block.clone());
            parent = new_block.header;
        }

        for block in &blocks {
            chain.process_block(&block).expect("process block ok");
        }

        let synchronizer = Synchronizer::new(&chain, Notify::default(), None, Config::default());

        let headers = synchronizer.get_locator_response(180, &H256::zero());

        assert_eq!(headers.first().unwrap(), &blocks[180].header);
        assert_eq!(headers.last().unwrap(), &blocks[199].header);

        for window in headers.windows(2) {
            if let [parent, header] = &window {
                assert_eq!(header.parent_hash, parent.hash());
            }
        }
    }

    #[derive(Clone)]
    struct DummyNetworkContext {}

    impl NetworkContext for DummyNetworkContext {
        /// Send a packet over the network to another peer.
        fn send(&self, peer: PeerId, packet_id: PacketId, data: Vec<u8>) {}

        /// Send a packet over the network to another peer using specified protocol.
        fn send_protocol(
            &self,
            protocol: ProtocolId,
            peer: PeerId,
            packet_id: PacketId,
            data: Vec<u8>,
        ) {
        }

        /// Respond to a current network message. Panics if no there is no packet in the context. If the session is expired returns nothing.
        fn respond(&self, packet_id: PacketId, data: Vec<u8>) {}

        /// Report peer. Depending on the report, peer may be disconnected and possibly banned.
        fn report_peer(&self, peer: PeerId, reason: Severity) {}

        /// Check if the session is still active.
        fn is_expired(&self) -> bool {
            false
        }

        /// Register a new IO timer. 'IoHandler::timeout' will be called with the token.
        fn register_timer(&self, token: TimerToken, delay: Duration) -> Result<(), NetworkError> {
            Ok(())
        }

        /// Returns peer identification string
        fn peer_client_version(&self, peer: PeerId) -> String {
            "unknown".to_string()
        }

        /// Returns information on p2p session
        fn session_info(&self, peer: PeerId) -> Option<SessionInfo> {
            None
        }

        /// Returns max version for a given protocol.
        fn protocol_version(&self, protocol: ProtocolId, peer: PeerId) -> Option<u8> {
            None
        }

        /// Returns this object's subprotocol name.
        fn subprotocol_name(&self) -> ProtocolId {
            [1, 1, 1]
        }
    }

    #[test]
    fn test_sync_process() {
        let config = Consensus::default();
        let chain1 = Arc::new(gen_chain(&config));
        let chain2 = Arc::new(gen_chain(&config));
        let num = 200;

        for i in 1..num {
            insert_block(&chain1, i, i);
        }
        let synchronizer1 = Synchronizer::new(&chain1, Notify::default(), None, Config::default());

        let locator1 = synchronizer1.get_locator(&chain1.tip_header().read().header);
        let chain2 = Arc::new(gen_chain(&config));

        for i in 1..num + 1 {
            let j = if i > 192 { i + 1 } else { i };
            insert_block(&chain2, j, i);
        }

        let synchronizer2 = Synchronizer::new(&chain2, Notify::default(), None, Config::default());
        let latest_common = synchronizer2.locate_latest_common_block(&H256::zero(), &locator1[..]);
        assert_eq!(latest_common, Some(192));

        let headers = synchronizer2.get_locator_response(192, &H256::zero());

        assert_eq!(
            headers.first().unwrap().hash(),
            chain2.block_hash(193).unwrap()
        );
        assert_eq!(
            headers.last().unwrap().hash(),
            chain2.block_hash(200).unwrap()
        );

        let mut headers_proto = ckb_protocol::Headers::new();
        headers_proto.set_headers(RepeatedField::from_vec(
            headers.iter().map(|h| &h.header).map(Into::into).collect(),
        ));

        let peer = 1usize;
        HeadersProcess::new(
            &headers_proto,
            &synchronizer1,
            &peer,
            &DummyNetworkContext {},
        ).execute();

        let best_known_header = synchronizer1.peers.best_known_header(&peer);

        assert_eq!(
            best_known_header.clone().map(|h| h.header),
            headers.last().cloned()
        );

        let blocks_to_fetch = synchronizer1.get_blocks_to_fetch(peer).unwrap();

        assert_eq!(
            blocks_to_fetch.first().unwrap(),
            &chain2.block_hash(193).unwrap()
        );
        assert_eq!(
            blocks_to_fetch.last().unwrap(),
            &chain2.block_hash(200).unwrap()
        );
    }
}
