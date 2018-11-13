mod block_fetcher;
mod block_pool;
mod block_process;
mod get_blocks_process;
mod get_headers_process;
mod header_view;
mod headers_process;
mod peers;

use self::block_fetcher::BlockFetcher;
use self::block_pool::OrphanBlockPool;
use self::block_process::BlockProcess;
use self::get_blocks_process::GetBlocksProcess;
use self::get_headers_process::GetHeadersProcess;
use self::header_view::HeaderView;
use self::headers_process::HeadersProcess;
use self::peers::Peers;
use bigint::H256;
use ckb_chain::chain::ChainController;
use ckb_chain::error::ProcessBlockError;
use ckb_protocol::{SyncMessage, SyncPayload};
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::{ChainProvider, Shared};
use ckb_time::now_ms;
use config::Config;
use core::block::Block;
use core::header::{BlockNumber, Header};
use flatbuffers::{get_root, FlatBufferBuilder};
use network::PeerIndex;
use std::cmp;
use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use util::{RwLock, RwLockUpgradableReadGuard};
use {
    CHAIN_SYNC_TIMEOUT, EVICTION_HEADERS_RESPONSE_TIME, HEADERS_DOWNLOAD_TIMEOUT_BASE,
    HEADERS_DOWNLOAD_TIMEOUT_PER_HEADER, MAX_HEADERS_LEN,
    MAX_OUTBOUND_PEERS_TO_PROTECT_FROM_DISCONNECT, MAX_TIP_AGE, POW_SPACE,
};

pub const SEND_GET_HEADERS_TOKEN: TimerToken = 0;
pub const BLOCK_FETCH_TOKEN: TimerToken = 1;
pub const TIMEOUT_EVICTION_TOKEN: TimerToken = 2;

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

pub struct Synchronizer<CI: ChainIndex> {
    chain: ChainController,
    shared: Shared<CI>,
    pub status_map: BlockStatusMap,
    pub header_map: BlockHeaderMap,
    pub best_known_header: Arc<RwLock<HeaderView>>,
    pub n_sync: Arc<AtomicUsize>,
    pub peers: Arc<Peers>,
    pub config: Arc<Config>,
    pub orphan_block_pool: Arc<OrphanBlockPool>,
    pub outbound_peers_with_protect: Arc<AtomicUsize>,
}

impl<CI: ChainIndex> ::std::clone::Clone for Synchronizer<CI> {
    fn clone(&self) -> Self {
        Synchronizer {
            chain: self.chain.clone(),
            shared: self.shared.clone(),
            status_map: Arc::clone(&self.status_map),
            header_map: Arc::clone(&self.header_map),
            best_known_header: Arc::clone(&self.best_known_header),
            n_sync: Arc::clone(&self.n_sync),
            peers: Arc::clone(&self.peers),
            config: Arc::clone(&self.config),
            orphan_block_pool: Arc::clone(&self.orphan_block_pool),
            outbound_peers_with_protect: Arc::clone(&self.outbound_peers_with_protect),
        }
    }
}

fn is_outbound(nc: &CKBProtocolContext, peer: PeerIndex) -> Option<bool> {
    nc.session_info(peer)
        .map(|session_info| session_info.peer.is_outgoing())
}

impl<CI: ChainIndex> Synchronizer<CI> {
    pub fn new(chain: ChainController, shared: Shared<CI>, config: Config) -> Synchronizer<CI> {
        let (total_difficulty, header) = {
            let tip_header = shared.tip_header().read();
            (tip_header.total_difficulty(), tip_header.inner().clone())
        };
        let best_known_header = HeaderView::new(header, total_difficulty);
        let orphan_block_limit = config.orphan_block_limit;

        Synchronizer {
            config: Arc::new(config),
            chain,
            shared,
            peers: Arc::new(Peers::default()),
            orphan_block_pool: Arc::new(OrphanBlockPool::with_capacity(orphan_block_limit)),
            best_known_header: Arc::new(RwLock::new(best_known_header)),
            status_map: Arc::new(RwLock::new(HashMap::new())),
            header_map: Arc::new(RwLock::new(HashMap::new())),
            n_sync: Arc::new(AtomicUsize::new(0)),
            outbound_peers_with_protect: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn process(&self, nc: &CKBProtocolContext, peer: PeerIndex, message: SyncMessage) {
        match message.payload_type() {
            SyncPayload::GetHeaders => {
                GetHeadersProcess::new(&message.payload_as_get_headers().unwrap(), self, peer, nc)
                    .execute()
            }
            SyncPayload::Headers => {
                HeadersProcess::new(&message.payload_as_headers().unwrap(), self, peer, nc)
                    .execute()
            }
            SyncPayload::GetBlocks => {
                GetBlocksProcess::new(&message.payload_as_get_blocks().unwrap(), self, peer, nc)
                    .execute()
            }
            SyncPayload::Block => {
                BlockProcess::new(&message.payload_as_block().unwrap(), self, peer, nc).execute()
            }
            SyncPayload::NONE => {}
        }
    }

    pub fn get_block_status(&self, hash: &H256) -> BlockStatus {
        let guard = self.status_map.upgradable_read();
        match guard.get(hash).cloned() {
            Some(s) => s,
            None => if self.shared.block_header(hash).is_some() {
                let mut write_guard = RwLockUpgradableReadGuard::upgrade(guard);
                write_guard.insert(*hash, BlockStatus::BLOCK_HAVE_MASK);
                BlockStatus::BLOCK_HAVE_MASK
            } else {
                BlockStatus::UNKNOWN
            },
        }
    }

    pub fn peers(&self) -> Arc<Peers> {
        Arc::clone(&self.peers)
    }

    pub fn insert_block_status(&self, hash: H256, status: BlockStatus) {
        self.status_map.write().insert(hash, status);
    }

    pub fn best_known_header(&self) -> HeaderView {
        self.best_known_header.read().clone()
    }

    pub fn is_initial_block_download(&self) -> bool {
        now_ms().saturating_sub(self.shared.tip_header().read().inner().timestamp()) > MAX_TIP_AGE
    }

    pub fn get_headers_sync_timeout(&self, header: &Header) -> u64 {
        HEADERS_DOWNLOAD_TIMEOUT_BASE
            + HEADERS_DOWNLOAD_TIMEOUT_PER_HEADER
                * (now_ms().saturating_sub(header.timestamp()) / POW_SPACE)
    }

    pub fn mark_block_stored(&self, hash: H256) {
        self.status_map
            .write()
            .entry(hash)
            .and_modify(|status| *status = BlockStatus::BLOCK_HAVE_MASK)
            .or_insert_with(|| BlockStatus::BLOCK_HAVE_MASK);
    }

    pub fn tip_header(&self) -> Header {
        self.shared.tip_header().read().inner().clone()
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
                    locator.push(self.shared.genesis_hash());
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

        if locator.last().expect("empty checked") != &self.shared.genesis_hash() {
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
            let mut block_hash = header.parent_hash();
            loop {
                let block_header = match self.shared.block_header(&block_hash) {
                    None => break latest_common,
                    Some(block_header) => block_header,
                };

                if let Some(block_number) = self.shared.block_number(&block_hash) {
                    return Some(block_number);
                }

                block_hash = block_header.parent_hash();
            }
        } else {
            latest_common
        }
    }

    pub fn get_header_view(&self, hash: &H256) -> Option<HeaderView> {
        self.header_map.read().get(hash).cloned().or_else(|| {
            self.shared.block_header(hash).and_then(|header| {
                self.shared
                    .block_ext(&hash)
                    .map(|block_ext| HeaderView::new(header, block_ext.total_difficulty))
            })
        })
    }

    pub fn get_header(&self, hash: &H256) -> Option<Header> {
        self.header_map
            .read()
            .get(hash)
            .map(|view| view.inner())
            .cloned()
            .or_else(|| self.shared.block_header(hash))
    }

    pub fn get_block(&self, hash: &H256) -> Option<Block> {
        self.shared.block(hash)
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

    pub fn insert_header_view(&self, header: &Header, peer: PeerIndex) {
        if let Some(parent_view) = self.get_header_view(&header.parent_hash()) {
            let total_difficulty = parent_view.total_difficulty() + header.difficulty();
            let header_view = {
                let best_known_header = self.best_known_header.upgradable_read();
                let header_view = HeaderView::new(header.clone(), total_difficulty);

                if total_difficulty > best_known_header.total_difficulty()
                    || (total_difficulty == best_known_header.total_difficulty()
                        && header.hash() < best_known_header.hash())
                {
                    let mut best_known_header =
                        RwLockUpgradableReadGuard::upgrade(best_known_header);
                    *best_known_header = header_view.clone();
                }
                header_view
            };

            self.peers.new_header_received(peer, &header_view);

            let mut header_map = self.header_map.write();
            header_map.insert(header.hash(), header_view);
        }
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
            try_option!(self.get_ancestor(&best_known_header.hash(), last_common_header.number()));

        if &m_right == last_common_header {
            return Some(m_right);
        }

        let mut m_left = try_option!(self.get_header(&last_common_header.hash()));
        debug_assert!(m_right.number() == m_left.number());

        while m_left != m_right {
            m_left = try_option!(self.get_ancestor(&m_left.hash(), m_left.number() - 1));
            m_right = try_option!(self.get_ancestor(&m_right.hash(), m_right.number() - 1));
        }
        Some(m_left)
    }

    //TODO: process block which we don't request
    #[cfg_attr(feature = "cargo-clippy", allow(single_match))]
    pub fn process_new_block(&self, peer: PeerIndex, block: Block) {
        match self.get_block_status(&block.header().hash()) {
            BlockStatus::VALID_MASK => {
                self.insert_new_block(peer, block);
            }
            status => {
                debug!(target: "sync", "[Synchronizer] process_new_block unexpect status {:?}", status);
            }
        }
    }

    fn accept_block(&self, peer: PeerIndex, block: &Arc<Block>) -> Result<(), ProcessBlockError> {
        self.chain.process_block(Arc::clone(&block))?;
        self.mark_block_stored(block.header().hash());
        self.peers.set_last_common_header(peer, &block.header());
        Ok(())
    }

    //FIXME: guarantee concurrent block process
    fn insert_new_block(&self, peer: PeerIndex, block: Block) {
        let block = Arc::new(block);
        if self
            .shared
            .output_root(&block.header().parent_hash())
            .is_some()
        {
            let accept_ret = self.accept_block(peer, &block);
            if accept_ret.is_ok() {
                let pre_orphan_block = self
                    .orphan_block_pool
                    .remove_blocks_by_parent(&block.header().hash());
                for block in pre_orphan_block {
                    let block = Arc::new(block);
                    if self
                        .shared
                        .output_root(&block.header().parent_hash())
                        .is_some()
                    {
                        let ret = self.accept_block(peer, &block);
                        if ret.is_err() {
                            debug!(
                                target: "sync", "[Synchronizer] accept_block {:#?} error {:?}",
                                block,
                                ret.unwrap_err()
                            );
                        }
                    } else {
                        debug!(
                            target: "sync", "[Synchronizer] insert_orphan_block {:#?}------------{:?}",
                            block.header().number(),
                            block.header().hash()
                        );
                        self.orphan_block_pool.insert(Block::clone(&block));
                    }
                }
            } else {
                debug!(
                    target: "sync", "[Synchronizer] accept_block {:#?} error {:?}",
                    block,
                    accept_ret.unwrap_err()
                )
            }
        } else {
            debug!(
                target: "sync", "[Synchronizer] insert_orphan_block {:#?}------------{:?}",
                block.header().number(),
                block.header().hash()
            );
            self.orphan_block_pool.insert(Block::clone(&block));
        }

        debug!(target: "sync", "[Synchronizer] insert_new_block finish");
    }

    pub fn get_blocks_to_fetch(&self, peer: PeerIndex) -> Option<Vec<H256>> {
        BlockFetcher::new(&self, peer).fetch()
    }

    fn on_connected(&self, nc: &CKBProtocolContext, peer: PeerIndex) {
        let tip = self.tip_header();
        let timeout = self.get_headers_sync_timeout(&tip);

        let protect_outbound = is_outbound(nc, peer).unwrap_or_else(|| false)
            && self.outbound_peers_with_protect.load(Ordering::Acquire)
                < MAX_OUTBOUND_PEERS_TO_PROTECT_FROM_DISCONNECT;

        if protect_outbound {
            self.outbound_peers_with_protect
                .fetch_add(1, Ordering::Release);
        }

        self.peers.on_connected(peer, timeout, protect_outbound);
        self.n_sync.fetch_add(1, Ordering::Release);
        self.send_getheaders_to_peer(nc, peer, &tip);
    }

    pub fn send_getheaders_to_peer(
        &self,
        nc: &CKBProtocolContext,
        peer: PeerIndex,
        header: &Header,
    ) {
        let locator_hash = self.get_locator(header);
        let fbb = &mut FlatBufferBuilder::new();
        let message = SyncMessage::build_get_headers(fbb, &locator_hash);
        fbb.finish(message, None);
        let _ = nc.send(peer, fbb.finished_data().to_vec());
    }

    //   - If at timeout their best known block now has more work than our tip
    //     when the timeout was set, then either reset the timeout or clear it
    //     (after comparing against our current tip's work)
    //   - If at timeout their best known block still has less work than our
    //     tip did when the timeout was set, then send a getheaders message,
    //     and set a shorter timeout, HEADERS_RESPONSE_TIME seconds in future.
    //     If their best known block is still behind when that new timeout is
    //     reached, disconnect.
    pub fn eviction(&self, nc: &CKBProtocolContext) {
        let mut peer_state = self.peers.state.write();
        let best_known_headers = self.peers.best_known_headers.read();
        let is_initial_block_download = self.is_initial_block_download();
        let mut eviction = Vec::new();
        for (peer, state) in peer_state.iter_mut() {
            let now = now_ms();
            // headers_sync_timeout
            if let Some(timeout) = state.headers_sync_timeout {
                if now > timeout && is_initial_block_download && !state.disconnect {
                    eviction.push(*peer);
                    state.disconnect = true;
                    continue;
                }
            }
            if let Some(is_outbound) = is_outbound(nc, *peer) {
                if !state.chain_sync.protect && is_outbound {
                    let best_known_header = best_known_headers.get(peer);

                    let chain_tip = self.shared.tip_header().read();
                    if best_known_header.map(|h| h.total_difficulty())
                        >= Some(chain_tip.total_difficulty())
                    {
                        if state.chain_sync.timeout != 0 {
                            state.chain_sync.timeout = 0;
                            state.chain_sync.work_header = None;
                            state.chain_sync.sent_getheaders = false;
                        }
                    } else if state.chain_sync.timeout == 0
                        || (best_known_header.is_some()
                            && best_known_header.map(|h| h.total_difficulty()) >= state
                                .chain_sync
                                .work_header
                                .as_ref()
                                .map(|h| h.total_difficulty()))
                    {
                        // Our best block known by this peer is behind our tip, and we're either noticing
                        // that for the first time, OR this peer was able to catch up to some earlier point
                        // where we checked against our tip.
                        // Either way, set a new timeout based on current tip.
                        state.chain_sync.timeout = now + CHAIN_SYNC_TIMEOUT;
                        state.chain_sync.work_header = Some(chain_tip.clone());
                        state.chain_sync.sent_getheaders = false;
                    } else if state.chain_sync.timeout > 0 && now > state.chain_sync.timeout {
                        // No evidence yet that our peer has synced to a chain with work equal to that
                        // of our tip, when we first detected it was behind. Send a single getheaders
                        // message to give the peer a chance to update us.
                        if state.chain_sync.sent_getheaders {
                            eviction.push(*peer);
                            state.disconnect = true;
                        } else {
                            state.chain_sync.sent_getheaders = true;
                            state.chain_sync.timeout = now + EVICTION_HEADERS_RESPONSE_TIME;
                            self.send_getheaders_to_peer(
                                nc,
                                *peer,
                                state.chain_sync.work_header.clone().unwrap().inner(),
                            );
                        }
                    }
                }
            }
        }
        for peer in eviction {
            nc.report_peer(peer, Severity::Timeout);
        }
    }

    fn send_getheaders_to_all(&self, nc: &CKBProtocolContext) {
        let peers: Vec<PeerIndex> = self
            .peers
            .state
            .read()
            .iter()
            .filter(|(_, state)| state.sync_started)
            .map(|(peer_id, _)| peer_id)
            .cloned()
            .collect();
        debug!(target: "sync", "send_getheaders to peers= {:?}", &peers);
        let tip = self.tip_header();
        for peer in peers {
            self.send_getheaders_to_peer(nc, peer, &tip);
        }
    }

    fn find_blocks_to_fetch(&self, nc: &CKBProtocolContext) {
        let peers: Vec<PeerIndex> = self
            .peers
            .state
            .read()
            .iter()
            .filter(|(_, state)| state.sync_started)
            .map(|(peer_id, _)| peer_id)
            .cloned()
            .collect();

        debug!(target: "sync", "poll find_blocks_to_fetch select peers");
        for peer in peers {
            if let Some(v_fetch) = self.get_blocks_to_fetch(peer) {
                self.send_getblocks(&v_fetch, nc, peer);
            }
        }
    }

    fn send_getblocks(&self, v_fetch: &[H256], nc: &CKBProtocolContext, peer: PeerIndex) {
        let fbb = &mut FlatBufferBuilder::new();
        let message = SyncMessage::build_get_blocks(fbb, v_fetch);
        fbb.finish(message, None);
        let _ = nc.send(peer, fbb.finished_data().to_vec());
        debug!(target: "sync", "send_getblocks len={:?} to peer={}", v_fetch.len() , peer);
    }
}

impl<CI> CKBProtocolHandler for Synchronizer<CI>
where
    CI: ChainIndex + 'static,
{
    fn initialize(&self, nc: Box<CKBProtocolContext>) {
        // NOTE: 100ms is what bitcoin use.
        let _ = nc.register_timer(SEND_GET_HEADERS_TOKEN, Duration::from_millis(100));
        let _ = nc.register_timer(BLOCK_FETCH_TOKEN, Duration::from_millis(100));
        let _ = nc.register_timer(TIMEOUT_EVICTION_TOKEN, Duration::from_millis(100));
    }

    fn received(&self, nc: Box<CKBProtocolContext>, peer: PeerIndex, data: &[u8]) {
        // TODO use flatbuffers verifier
        let msg = get_root::<SyncMessage>(&data);
        debug!(target: "sync", "msg {:?}", msg.payload_type());
        self.process(nc.as_ref(), peer, msg);
    }

    fn connected(&self, nc: Box<CKBProtocolContext>, peer: PeerIndex) {
        if self.n_sync.load(Ordering::Acquire) == 0 || !self.is_initial_block_download() {
            debug!(target: "sync", "init_getheaders peer={:?} connected", peer);
            self.on_connected(nc.as_ref(), peer);
        }
    }

    fn disconnected(&self, _nc: Box<CKBProtocolContext>, peer: PeerIndex) {
        info!(target: "sync", "peer={} SyncProtocol.disconnected", peer);
        self.peers.disconnected(peer);
    }

    fn timer_triggered(&self, nc: Box<CKBProtocolContext>, token: TimerToken) {
        if !self.peers.state.read().is_empty() {
            match token as usize {
                SEND_GET_HEADERS_TOKEN => {
                    self.send_getheaders_to_all(nc.as_ref());
                }
                BLOCK_FETCH_TOKEN => {
                    self.find_blocks_to_fetch(nc.as_ref());
                }
                _ => unreachable!(),
            }
        } else {
            debug!(target: "sync", "no peers connected");
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate env_logger;

    use self::block_process::BlockProcess;
    use self::headers_process::HeadersProcess;
    use super::*;
    use bigint::U256;
    use chain_spec::consensus::Consensus;
    use ckb_chain::chain::ChainBuilder;
    use ckb_notify::{NotifyController, NotifyService, MINER_SUBSCRIBER};
    use ckb_protocol::{Block as FbsBlock, Headers as FbsHeaders};
    use ckb_shared::index::ChainIndex;
    use ckb_shared::shared::SharedBuilder;
    use ckb_shared::store::ChainKVStore;
    use ckb_time::set_mock_timer;
    use core::block::BlockBuilder;
    use core::header::{Header, HeaderBuilder};
    use core::transaction::{CellInput, CellOutput, Transaction, TransactionBuilder};
    use db::memorydb::MemoryKeyValueDB;
    use flatbuffers::FlatBufferBuilder;
    use fnv::{FnvHashMap, FnvHashSet};
    use network::{
        random_peer_id, CKBProtocolContext, Endpoint, Error as NetworkError, PeerIndex, PeerInfo,
        ProtocolId, SessionInfo, Severity, TimerToken,
    };
    use std::ops::Deref;
    use std::time::Duration;
    use util::Mutex;

    fn start_chain(
        consensus: Option<Consensus>,
        notify: Option<NotifyController>,
    ) -> (
        ChainController,
        Shared<ChainKVStore<MemoryKeyValueDB>>,
        NotifyController,
    ) {
        let mut builder = SharedBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory();
        if let Some(consensus) = consensus {
            builder = builder.consensus(consensus);
        }
        let shared = builder.build();

        let notify = notify.unwrap_or_else(|| NotifyService::default().start::<&str>(None).1);
        let (chain_controller, chain_receivers) = ChainController::new();
        let chain_service = ChainBuilder::new(shared.clone())
            .notify(notify.clone())
            .build();
        let _handle = chain_service.start::<&str>(None, chain_receivers);
        (chain_controller, shared, notify)
    }

    fn gen_synchronizer<CI: ChainIndex + 'static>(
        chain_controller: ChainController,
        shared: Shared<CI>,
    ) -> Synchronizer<CI> {
        Synchronizer::new(chain_controller, shared, Config::default())
    }

    #[test]
    fn test_block_status() {
        let status1 = BlockStatus::FAILED_VALID;
        let status2 = BlockStatus::FAILED_CHILD;
        assert!((status1 & BlockStatus::FAILED_MASK) == status1);
        assert!((status2 & BlockStatus::FAILED_MASK) == status2);
    }

    fn create_cellbase(number: BlockNumber) -> Transaction {
        TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(number))
            .output(CellOutput::new(0, vec![], H256::from(0), None))
            .build()
    }

    fn gen_block(parent_header: Header, difficulty: U256, nonce: u64) -> Block {
        let now = 1 + parent_header.timestamp();
        let number = parent_header.number() + 1;
        let cellbase = create_cellbase(number);
        let header_builder = HeaderBuilder::default()
            .parent_hash(&parent_header.hash())
            .timestamp(now)
            .number(number)
            .difficulty(&difficulty)
            .cellbase_id(&cellbase.hash())
            .nonce(nonce);

        BlockBuilder::default()
            .commit_transaction(cellbase)
            .with_header_builder(header_builder)
    }

    fn insert_block<CI: ChainIndex>(
        chain_controller: &ChainController,
        shared: &Shared<CI>,
        nonce: u64,
        number: BlockNumber,
    ) {
        let parent = shared
            .block_header(&shared.block_hash(number - 1).unwrap())
            .unwrap();
        let difficulty = shared.calculate_difficulty(&parent).unwrap();
        let block = gen_block(parent, difficulty, nonce);

        chain_controller
            .process_block(Arc::new(block))
            .expect("process block ok");
    }

    #[test]
    fn test_locator() {
        let (chain_controller, shared, _notify) = start_chain(None, None);

        let num = 200;
        let index = [
            199, 198, 197, 196, 195, 194, 193, 192, 191, 190, 188, 184, 176, 160, 128, 64,
        ];

        for i in 1..num {
            insert_block(&chain_controller, &shared, i, i);
        }

        let synchronizer = gen_synchronizer(chain_controller.clone(), shared.clone());

        let locator = synchronizer.get_locator(&shared.tip_header().read().inner());

        let mut expect = Vec::new();

        for i in index.iter() {
            expect.push(shared.block_hash(*i).unwrap());
        }
        //genesis_hash must be the last one
        expect.push(shared.genesis_hash());

        assert_eq!(expect, locator);
    }

    #[test]
    fn test_locate_latest_common_block() {
        let consensus = Consensus::default();
        let (chain_controller1, shared1, _notify1) = start_chain(Some(consensus.clone()), None);
        let (chain_controller2, shared2, _notify2) = start_chain(Some(consensus.clone()), None);
        let num = 200;

        for i in 1..num {
            insert_block(&chain_controller1, &shared1, i, i);
        }

        for i in 1..num {
            insert_block(&chain_controller2, &shared2, i + 1, i);
        }

        let synchronizer1 = gen_synchronizer(chain_controller1.clone(), shared1.clone());

        let synchronizer2 = gen_synchronizer(chain_controller2.clone(), shared2.clone());

        let locator1 = synchronizer1.get_locator(&shared1.tip_header().read().inner());

        let latest_common = synchronizer2.locate_latest_common_block(&H256::zero(), &locator1[..]);

        assert_eq!(latest_common, Some(0));

        let (chain_controller3, shared3, _notify3) = start_chain(Some(consensus), None);

        for i in 1..num {
            let j = if i > 192 { i + 1 } else { i };
            insert_block(&chain_controller3, &shared3, j, i);
        }

        let synchronizer3 = gen_synchronizer(chain_controller3.clone(), shared3.clone());

        let latest_common3 = synchronizer3.locate_latest_common_block(&H256::zero(), &locator1[..]);
        assert_eq!(latest_common3, Some(192));
    }

    #[test]
    fn test_locate_latest_common_block2() {
        let consensus = Consensus::default();
        let (chain_controller1, shared1, _notify1) = start_chain(Some(consensus.clone()), None);
        let (chain_controller2, shared2, _notify2) = start_chain(Some(consensus.clone()), None);
        let block_number = 200;

        let mut blocks: Vec<Block> = Vec::new();
        let mut parent = consensus.genesis_block().header().clone();
        for i in 1..block_number {
            let difficulty = shared1.calculate_difficulty(&parent).unwrap();
            let new_block = gen_block(parent, difficulty, i);
            blocks.push(new_block.clone());

            chain_controller1
                .process_block(Arc::new(new_block.clone()))
                .expect("process block ok");
            chain_controller2
                .process_block(Arc::new(new_block.clone()))
                .expect("process block ok");
            parent = new_block.header().clone();
        }

        parent = blocks[150].header().clone();
        let fork = parent.number();
        for i in 1..block_number + 1 {
            let difficulty = shared2.calculate_difficulty(&parent).unwrap();
            let new_block = gen_block(parent, difficulty, i + 100);
            chain_controller2
                .process_block(Arc::new(new_block.clone()))
                .expect("process block ok");
            parent = new_block.header().clone();
        }

        let synchronizer1 = gen_synchronizer(chain_controller1.clone(), shared1.clone());
        let synchronizer2 = gen_synchronizer(chain_controller2.clone(), shared2.clone());
        let locator1 = synchronizer1.get_locator(&shared1.tip_header().read().inner());

        let latest_common = synchronizer2
            .locate_latest_common_block(&H256::zero(), &locator1[..])
            .unwrap();

        assert_eq!(
            shared1.block_hash(fork).unwrap(),
            shared2.block_hash(fork).unwrap()
        );
        assert!(shared1.block_hash(fork + 1).unwrap() != shared2.block_hash(fork + 1).unwrap());
        assert_eq!(
            shared1.block_hash(latest_common).unwrap(),
            shared1.block_hash(fork).unwrap()
        );
    }

    #[test]
    fn test_get_ancestor() {
        let consensus = Consensus::default();
        let (chain_controller, shared, _notify) = start_chain(Some(consensus), None);
        let num = 200;

        for i in 1..num {
            insert_block(&chain_controller, &shared, i, i);
        }

        let synchronizer = gen_synchronizer(chain_controller.clone(), shared.clone());

        let header = synchronizer.get_ancestor(&shared.tip_header().read().hash(), 100);
        let tip = synchronizer.get_ancestor(&shared.tip_header().read().hash(), 199);
        let noop = synchronizer.get_ancestor(&shared.tip_header().read().hash(), 200);
        assert!(tip.is_some());
        assert!(header.is_some());
        assert!(noop.is_none());
        assert_eq!(tip.unwrap(), shared.tip_header().read().inner().clone());
        assert_eq!(
            header.unwrap(),
            shared
                .block_header(&shared.block_hash(100).unwrap())
                .unwrap()
        );
    }

    #[test]
    fn test_process_new_block() {
        let consensus = Consensus::default();
        let (chain_controller1, shared1, _notify1) = start_chain(Some(consensus.clone()), None);
        let (chain_controller2, shared2, _notify2) = start_chain(Some(consensus.clone()), None);
        let block_number = 2000;
        let peer = 0;

        let mut blocks: Vec<Block> = Vec::new();
        let mut parent = shared1
            .block_header(&shared1.block_hash(0).unwrap())
            .unwrap();
        for i in 1..block_number {
            let difficulty = shared1.calculate_difficulty(&parent).unwrap();
            let new_block = gen_block(parent, difficulty, i + 100);
            chain_controller1
                .process_block(Arc::new(new_block.clone()))
                .expect("process block ok");
            blocks.push(new_block.clone());
            parent = new_block.header().clone();
        }

        let synchronizer = gen_synchronizer(chain_controller2.clone(), shared2.clone());

        blocks.clone().into_iter().for_each(|block| {
            synchronizer.insert_new_block(peer, block);
        });

        assert_eq!(
            blocks.last().unwrap().header(),
            shared2.tip_header().read().inner()
        );
    }

    #[test]
    fn test_get_locator_response() {
        let consensus = Consensus::default();
        let (chain_controller, shared, _notify) = start_chain(Some(consensus), None);
        let block_number = 200;

        let mut blocks: Vec<Block> = Vec::new();
        let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
        for i in 1..block_number + 1 {
            let difficulty = shared.calculate_difficulty(&parent).unwrap();
            let new_block = gen_block(parent, difficulty, i + 100);
            blocks.push(new_block.clone());
            chain_controller
                .process_block(Arc::new(new_block.clone()))
                .expect("process block ok");
            parent = new_block.header().clone();
        }

        let synchronizer = gen_synchronizer(chain_controller.clone(), shared.clone());

        let headers = synchronizer.get_locator_response(180, &H256::zero());

        assert_eq!(headers.first().unwrap(), blocks[180].header());
        assert_eq!(headers.last().unwrap(), blocks[199].header());

        for window in headers.windows(2) {
            if let [parent, header] = &window {
                assert_eq!(header.parent_hash(), parent.hash());
            }
        }
    }

    #[derive(Clone)]
    struct DummyNetworkContext {
        pub sessions: FnvHashMap<PeerIndex, SessionInfo>,
        pub disconnected: Arc<Mutex<FnvHashSet<PeerIndex>>>,
    }

    fn mock_session_info() -> SessionInfo {
        SessionInfo {
            peer: PeerInfo {
                peer_id: random_peer_id().unwrap(),
                endpoint_role: Endpoint::Dialer,
                last_ping_time: None,
                remote_addresses: vec![],
                identify_info: None,
            },
            protocol_version: None,
        }
    }

    fn mock_header_view(total_difficulty: u64) -> HeaderView {
        HeaderView::new(
            HeaderBuilder::default().build(),
            U256::from(total_difficulty),
        )
    }

    impl CKBProtocolContext for DummyNetworkContext {
        /// Send a packet over the network to another peer.
        fn send(&self, _peer: PeerIndex, _data: Vec<u8>) -> Result<(), NetworkError> {
            Ok(())
        }

        /// Send a packet over the network to another peer using specified protocol.
        fn send_protocol(
            &self,
            _peer: PeerIndex,
            _protocol: ProtocolId,
            _data: Vec<u8>,
        ) -> Result<(), NetworkError> {
            Ok(())
        }
        /// Report peer. Depending on the report, peer may be disconnected and possibly banned.
        fn report_peer(&self, peer: PeerIndex, _reason: Severity) {
            self.disconnected.lock().insert(peer);
        }

        fn ban_peer(&self, _peer: PeerIndex, _duration: Duration) {}

        /// Register a new IO timer. 'IoHandler::timeout' will be called with the token.
        fn register_timer(&self, _token: TimerToken, _delay: Duration) -> Result<(), NetworkError> {
            unimplemented!();
        }

        /// Returns information on p2p session
        fn session_info(&self, peer: PeerIndex) -> Option<SessionInfo> {
            self.sessions.get(&peer).cloned()
        }
        /// Returns max version for a given protocol.
        fn protocol_version(&self, _peer: PeerIndex, _protocol: ProtocolId) -> Option<u8> {
            unimplemented!();
        }

        fn disconnect(&self, _peer: PeerIndex) {}
        fn protocol_id(&self) -> ProtocolId {
            unimplemented!();
        }

        fn connected_peers(&self) -> Vec<PeerIndex> {
            unimplemented!();
        }
    }

    fn mock_network_context(peer_num: usize) -> DummyNetworkContext {
        let mut sessions = FnvHashMap::default();
        for peer in 0..peer_num {
            sessions.insert(peer, mock_session_info());
        }
        DummyNetworkContext {
            sessions,
            disconnected: Arc::new(Mutex::new(FnvHashSet::default())),
        }
    }

    #[test]
    fn test_sync_process() {
        let _ = env_logger::try_init();
        let consensus = Consensus::default();
        let (_handle, notify) = NotifyService::default().start::<&str>(None);
        let (chain_controller1, shared1, _) =
            start_chain(Some(consensus.clone()), Some(notify.clone()));
        let (chain_controller2, shared2, _) =
            start_chain(Some(consensus.clone()), Some(notify.clone()));
        let num = 200;

        for i in 1..num {
            insert_block(&chain_controller1, &shared1, i, i);
        }

        let synchronizer1 = gen_synchronizer(chain_controller1.clone(), shared1.clone());

        let locator1 = synchronizer1.get_locator(&shared1.tip_header().read().inner());

        for i in 1..num + 1 {
            let j = if i > 192 { i + 1 } else { i };
            insert_block(&chain_controller2, &shared2, j, i);
        }

        let synchronizer2 = gen_synchronizer(chain_controller2.clone(), shared2.clone());
        let latest_common = synchronizer2.locate_latest_common_block(&H256::zero(), &locator1[..]);
        assert_eq!(latest_common, Some(192));

        let headers = synchronizer2.get_locator_response(192, &H256::zero());

        assert_eq!(
            headers.first().unwrap().hash(),
            shared2.block_hash(193).unwrap()
        );
        assert_eq!(
            headers.last().unwrap().hash(),
            shared2.block_hash(200).unwrap()
        );

        let fbb = &mut FlatBufferBuilder::new();
        let fbs_headers = FbsHeaders::build(fbb, &headers);
        fbb.finish(fbs_headers, None);
        let fbs_headers = get_root::<FbsHeaders>(fbb.finished_data());

        let peer = 1usize;
        HeadersProcess::new(&fbs_headers, &synchronizer1, peer, &mock_network_context(0)).execute();

        let best_known_header = synchronizer1.peers.best_known_header(peer);

        assert_eq!(best_known_header.unwrap().inner(), headers.last().unwrap());

        let blocks_to_fetch = synchronizer1.get_blocks_to_fetch(peer).unwrap();

        assert_eq!(
            blocks_to_fetch.first().unwrap(),
            &shared2.block_hash(193).unwrap()
        );
        assert_eq!(
            blocks_to_fetch.last().unwrap(),
            &shared2.block_hash(200).unwrap()
        );

        let mut fetched_blocks = Vec::new();
        for block_hash in &blocks_to_fetch {
            fetched_blocks.push(shared2.block(block_hash).unwrap());
        }

        let new_tip_receiver = notify.subscribe_new_tip(MINER_SUBSCRIBER);

        for block in &fetched_blocks {
            let fbb = &mut FlatBufferBuilder::new();
            let fbs_block = FbsBlock::build(fbb, block);
            fbb.finish(fbs_block, None);
            let fbs_block = get_root::<FbsBlock>(fbb.finished_data());

            BlockProcess::new(&fbs_block, &synchronizer1, peer, &mock_network_context(0)).execute();
        }

        assert_eq!(
            &synchronizer1
                .peers
                .last_common_headers
                .read()
                .get(&peer)
                .unwrap()
                .hash(),
            blocks_to_fetch.last().unwrap()
        );

        assert_eq!(
            new_tip_receiver.recv(),
            Some(Arc::new(fetched_blocks[6].clone()))
        );
    }

    #[test]
    fn test_header_sync_timeout() {
        use std::iter::FromIterator;

        let (chain_controller, shared, _notify) = start_chain(None, None);

        let synchronizer = gen_synchronizer(chain_controller.clone(), shared.clone());

        let network_context = mock_network_context(5);
        set_mock_timer(MAX_TIP_AGE * 2);
        assert!(synchronizer.is_initial_block_download());
        let peers = synchronizer.peers();
        // protect should not effect headers_timeout
        peers.on_connected(0, 0, true);
        peers.on_connected(1, 0, false);
        peers.on_connected(2, MAX_TIP_AGE * 2, false);
        synchronizer.eviction(&network_context);
        let disconnected = network_context.disconnected.lock();
        assert_eq!(
            disconnected.deref(),
            &FnvHashSet::from_iter(vec![0, 1].into_iter())
        )
    }
    #[test]
    fn test_chain_sync_timeout() {
        use std::iter::FromIterator;

        let consensus = Consensus::default();
        let header = HeaderBuilder::default().difficulty(&U256::from(2)).build();
        let block = BlockBuilder::default().header(header).build();
        let consensus = consensus.set_genesis_block(block);

        let (chain_controller, shared, _notify) = start_chain(Some(consensus), None);

        assert_eq!(shared.tip_header().read().total_difficulty(), U256::from(2));

        let synchronizer = gen_synchronizer(chain_controller.clone(), shared.clone());

        let network_context = mock_network_context(6);
        let peers = synchronizer.peers();
        //6 peers do not trigger header sync timeout
        peers.on_connected(0, MAX_TIP_AGE * 2, true);
        peers.on_connected(1, MAX_TIP_AGE * 2, true);
        peers.on_connected(2, MAX_TIP_AGE * 2, true);
        peers.on_connected(3, MAX_TIP_AGE * 2, false);
        peers.on_connected(4, MAX_TIP_AGE * 2, false);
        peers.on_connected(5, MAX_TIP_AGE * 2, false);
        peers.new_header_received(0, &mock_header_view(1));
        peers.new_header_received(2, &mock_header_view(3));
        peers.new_header_received(3, &mock_header_view(1));
        peers.new_header_received(5, &mock_header_view(3));
        synchronizer.eviction(&network_context);
        {
            assert!({ network_context.disconnected.lock().is_empty() });
            let peer_state = peers.state.read();
            assert_eq!(peer_state.get(&0).unwrap().chain_sync.protect, true);
            assert_eq!(peer_state.get(&1).unwrap().chain_sync.protect, true);
            assert_eq!(peer_state.get(&2).unwrap().chain_sync.protect, true);
            //protect peer is protected from disconnection
            assert!(peer_state.get(&2).unwrap().chain_sync.work_header.is_none());
            assert_eq!(peer_state.get(&3).unwrap().chain_sync.protect, false);
            assert_eq!(peer_state.get(&4).unwrap().chain_sync.protect, false);
            assert_eq!(peer_state.get(&5).unwrap().chain_sync.protect, false);
            // Our best block known by this peer is behind our tip, and we're either noticing
            // that for the first time, OR this peer was able to catch up to some earlier point
            // where we checked against our tip.
            // Either way, set a new timeout based on current tip.
            let tip = { shared.tip_header().read().clone() };
            assert_eq!(
                peer_state.get(&3).unwrap().chain_sync.work_header,
                Some(tip.clone())
            );
            assert_eq!(
                peer_state.get(&4).unwrap().chain_sync.work_header,
                Some(tip)
            );
            assert_eq!(
                peer_state.get(&3).unwrap().chain_sync.timeout,
                CHAIN_SYNC_TIMEOUT
            );
            assert_eq!(
                peer_state.get(&4).unwrap().chain_sync.timeout,
                CHAIN_SYNC_TIMEOUT
            );
        }
        set_mock_timer(CHAIN_SYNC_TIMEOUT + 1);
        synchronizer.eviction(&network_context);
        {
            let peer_state = peers.state.read();
            // No evidence yet that our peer has synced to a chain with work equal to that
            // of our tip, when we first detected it was behind. Send a single getheaders
            // message to give the peer a chance to update us.
            assert!({ network_context.disconnected.lock().is_empty() });
            assert_eq!(
                peer_state.get(&3).unwrap().chain_sync.timeout,
                now_ms() + EVICTION_HEADERS_RESPONSE_TIME
            );
            assert_eq!(
                peer_state.get(&4).unwrap().chain_sync.timeout,
                now_ms() + EVICTION_HEADERS_RESPONSE_TIME
            );
        }
        set_mock_timer(now_ms() + EVICTION_HEADERS_RESPONSE_TIME + 1);
        synchronizer.eviction(&network_context);
        {
            // Peer(3,4) run out of time to catch up!
            let disconnected = network_context.disconnected.lock();
            assert_eq!(
                disconnected.deref(),
                &FnvHashSet::from_iter(vec![3, 4].into_iter())
            )
        }
    }
}
