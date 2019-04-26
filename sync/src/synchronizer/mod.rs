mod block_fetcher;
mod block_pool;
mod block_process;
mod get_blocks_process;
mod get_headers_process;
mod headers_process;

use self::block_fetcher::BlockFetcher;
use self::block_pool::OrphanBlockPool;
use self::block_process::BlockProcess;
use self::get_blocks_process::GetBlocksProcess;
use self::get_headers_process::GetHeadersProcess;
use self::headers_process::HeadersProcess;
use crate::config::Config;
use crate::types::{HeaderView, Peers};
use crate::{
    BAD_MESSAGE_BAN_TIME, CHAIN_SYNC_TIMEOUT, EVICTION_HEADERS_RESPONSE_TIME,
    HEADERS_DOWNLOAD_TIMEOUT_BASE, HEADERS_DOWNLOAD_TIMEOUT_PER_HEADER, MAX_HEADERS_LEN,
    MAX_OUTBOUND_PEERS_TO_PROTECT_FROM_DISCONNECT, MAX_TIP_AGE, POW_SPACE,
};
use bitflags::bitflags;
use ckb_chain::chain::ChainController;
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::header::{BlockNumber, Header};
use ckb_network::{CKBProtocolContext, CKBProtocolHandler, PeerIndex};
use ckb_protocol::{cast, get_root, SyncMessage, SyncPayload};
use ckb_shared::shared::Shared;
use ckb_shared::store::ChainStore;
use ckb_traits::ChainProvider;
use ckb_util::{try_option, Mutex, RwLock};
use failure::Error as FailureError;
use faketime::unix_time_as_millis;
use flatbuffers::FlatBufferBuilder;
use log::{debug, info, trace};
use numext_fixed_hash::H256;
use std::cmp;
use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

pub const SEND_GET_HEADERS_TOKEN: u64 = 0;
pub const BLOCK_FETCH_TOKEN: u64 = 1;
pub const TIMEOUT_EVICTION_TOKEN: u64 = 2;

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

pub type BlockStatusMap = Arc<Mutex<HashMap<H256, BlockStatus>>>;
pub type BlockHeaderMap = Arc<RwLock<HashMap<H256, HeaderView>>>;

pub struct Synchronizer<CS: ChainStore> {
    chain: ChainController,
    shared: Shared<CS>,
    pub status_map: BlockStatusMap,
    pub header_map: BlockHeaderMap,
    pub best_known_header: Arc<RwLock<HeaderView>>,
    pub n_sync: Arc<AtomicUsize>,
    pub peers: Arc<Peers>,
    pub config: Arc<Config>,
    pub orphan_block_pool: Arc<OrphanBlockPool>,
    pub outbound_peers_with_protect: Arc<AtomicUsize>,
}

// https://github.com/rust-lang/rust/issues/40754
impl<CS: ChainStore> ::std::clone::Clone for Synchronizer<CS> {
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

impl<CS: ChainStore> Synchronizer<CS> {
    pub fn new(chain: ChainController, shared: Shared<CS>, config: Config) -> Synchronizer<CS> {
        let (total_difficulty, header, total_uncles_count) = {
            let chain_state = shared.chain_state().lock();
            let block_ext = shared
                .block_ext(&chain_state.tip_hash())
                .expect("tip block_ext must exist");
            (
                chain_state.total_difficulty().clone(),
                chain_state.tip_header().clone(),
                block_ext.total_uncles_count,
            )
        };
        let best_known_header = HeaderView::new(header, total_difficulty, total_uncles_count);
        let orphan_block_limit = config.orphan_block_limit;

        Synchronizer {
            config: Arc::new(config),
            chain,
            shared,
            peers: Arc::new(Peers::default()),
            orphan_block_pool: Arc::new(OrphanBlockPool::with_capacity(orphan_block_limit)),
            best_known_header: Arc::new(RwLock::new(best_known_header)),
            status_map: Arc::new(Mutex::new(HashMap::new())),
            header_map: Arc::new(RwLock::new(HashMap::new())),
            n_sync: Arc::new(AtomicUsize::new(0)),
            outbound_peers_with_protect: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn try_process(
        &self,
        nc: &CKBProtocolContext,
        peer: PeerIndex,
        message: SyncMessage,
    ) -> Result<(), FailureError> {
        match message.payload_type() {
            SyncPayload::GetHeaders => {
                GetHeadersProcess::new(&cast!(message.payload_as_get_headers())?, self, peer, nc)
                    .execute()?;
            }
            SyncPayload::Headers => {
                HeadersProcess::new(&cast!(message.payload_as_headers())?, self, peer, nc)
                    .execute()?;
            }
            SyncPayload::GetBlocks => {
                GetBlocksProcess::new(&cast!(message.payload_as_get_blocks())?, self, peer, nc)
                    .execute()?;
            }
            SyncPayload::Block => {
                BlockProcess::new(&cast!(message.payload_as_block())?, self, peer, nc).execute()?;
            }
            SyncPayload::NONE => {
                cast!(None)?;
            }
            _ => {
                cast!(None)?;
            }
        }
        Ok(())
    }

    fn process(&self, nc: &CKBProtocolContext, peer: PeerIndex, message: SyncMessage) {
        if self.try_process(nc, peer, message).is_err() {
            nc.ban_peer(peer, BAD_MESSAGE_BAN_TIME);
        }
    }

    pub fn get_block_status(&self, hash: &H256) -> BlockStatus {
        let mut guard = self.status_map.lock();
        match guard.get(hash).cloned() {
            Some(s) => s,
            None => {
                if self.shared.block_header(hash).is_some() {
                    guard.insert(hash.clone(), BlockStatus::BLOCK_HAVE_MASK);
                    BlockStatus::BLOCK_HAVE_MASK
                } else {
                    BlockStatus::UNKNOWN
                }
            }
        }
    }

    pub fn peers(&self) -> Arc<Peers> {
        Arc::clone(&self.peers)
    }

    pub fn insert_block_status(&self, hash: H256, status: BlockStatus) {
        self.status_map.lock().insert(hash, status);
    }

    pub fn best_known_header(&self) -> HeaderView {
        self.best_known_header.read().clone()
    }

    pub fn is_initial_block_download(&self) -> bool {
        unix_time_as_millis()
            .saturating_sub(self.shared.chain_state().lock().tip_header().timestamp())
            > MAX_TIP_AGE
    }

    pub fn predict_headers_sync_time(&self, header: &Header) -> u64 {
        let now = unix_time_as_millis();
        now + HEADERS_DOWNLOAD_TIMEOUT_BASE
            + HEADERS_DOWNLOAD_TIMEOUT_PER_HEADER
                * (now.saturating_sub(header.timestamp()) / POW_SPACE)
    }

    pub fn mark_block_stored(&self, hash: H256) {
        self.status_map
            .lock()
            .entry(hash)
            .and_modify(|status| *status = BlockStatus::BLOCK_HAVE_MASK)
            .or_insert_with(|| BlockStatus::BLOCK_HAVE_MASK);
    }

    pub fn tip_header(&self) -> Header {
        self.shared.chain_state().lock().tip_header().clone()
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
            locator.push(header.hash().clone());

            if locator.len() >= 10 {
                step <<= 1;
            }

            if index < step {
                // always include genesis hash
                if index != 0 {
                    locator.push(self.shared.genesis_hash().clone());
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
            let mut block_hash = header.parent_hash().clone();
            loop {
                let block_header = match self.shared.block_header(&block_hash) {
                    None => break latest_common,
                    Some(block_header) => block_header,
                };

                if let Some(block_number) = self.shared.block_number(&block_hash) {
                    return Some(block_number);
                }

                block_hash = block_header.parent_hash().clone();
            }
        } else {
            latest_common
        }
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

    pub fn consensus(&self) -> &Consensus {
        self.shared.consensus()
    }

    pub fn get_header(&self, hash: &H256) -> Option<Header> {
        self.header_map
            .read()
            .get(hash)
            .map(HeaderView::inner)
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
            let total_uncles_count =
                parent_view.total_uncles_count() + u64::from(header.uncles_count());
            let header_view = {
                let mut best_known_header = self.best_known_header.write();
                let header_view =
                    HeaderView::new(header.clone(), total_difficulty.clone(), total_uncles_count);

                if total_difficulty.gt(best_known_header.total_difficulty())
                    || (&total_difficulty == best_known_header.total_difficulty()
                        && header.hash() < best_known_header.hash())
                {
                    *best_known_header = header_view.clone();
                }
                header_view
            };

            self.peers.new_header_received(peer, &header_view);

            let mut header_map = self.header_map.write();
            header_map.insert(header.hash().clone(), header_view);
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
    pub fn process_new_block(&self, peer: PeerIndex, block: Block) {
        if self.orphan_block_pool.contains(&block) {
            debug!(target: "sync", "block {} already in orphan pool", block.header().hash());
            return;
        }

        match self.get_block_status(&block.header().hash()) {
            BlockStatus::VALID_MASK => {
                self.insert_new_block(peer, block);
            }
            status => {
                debug!(target: "sync", "[Synchronizer] process_new_block unexpect status {:?}", status);
            }
        }
    }

    fn accept_block(&self, peer: PeerIndex, block: &Arc<Block>) -> Result<(), FailureError> {
        self.chain.process_block(Arc::clone(&block))?;
        self.mark_block_stored(block.header().hash().clone());
        self.peers.set_last_common_header(peer, &block.header());
        Ok(())
    }

    //FIXME: guarantee concurrent block process
    fn insert_new_block(&self, peer: PeerIndex, block: Block) {
        let block = Arc::new(block);
        if self
            .shared
            .block_header(&block.header().parent_hash())
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
                        .block_header(&block.header().parent_hash())
                        .is_some()
                    {
                        if let Err(e) = self.accept_block(peer, &block) {
                            debug!(
                                target: "sync", "[Synchronizer] accept_block {:?} error {:?}",
                                block,
                                e
                            );
                        }
                    } else {
                        debug!(
                            target: "sync", "[Synchronizer] insert_orphan_block {:?}------------{:x}",
                            block.header().number(),
                            block.header().hash()
                        );
                        self.orphan_block_pool.insert(Block::clone(&block));
                    }
                }
            } else {
                debug!(
                    target: "sync", "[Synchronizer] accept_block {:?} error {}",
                    block,
                    accept_ret.unwrap_err()
                )
            }
        } else {
            debug!(
                target: "sync", "[Synchronizer] insert_orphan_block {:?}------------{:x}",
                block.header().number(),
                block.header().hash()
            );
            self.orphan_block_pool.insert(Block::clone(&block));
        }

        debug!(target: "sync", "[Synchronizer] insert_new_block finish");
    }

    pub fn get_blocks_to_fetch(&self, peer: PeerIndex) -> Option<Vec<H256>> {
        BlockFetcher::new(self.clone(), peer).fetch()
    }

    fn on_connected(&self, nc: &CKBProtocolContext, peer: PeerIndex) {
        let tip = self.tip_header();
        let predicted_headers_sync_time = self.predict_headers_sync_time(&tip);

        let is_outbound = nc
            .get_peer(peer)
            .map(|peer| peer.is_outbound())
            .unwrap_or(false);
        let protect_outbound = is_outbound
            && self.outbound_peers_with_protect.load(Ordering::Acquire)
                < MAX_OUTBOUND_PEERS_TO_PROTECT_FROM_DISCONNECT;

        if protect_outbound {
            self.outbound_peers_with_protect
                .fetch_add(1, Ordering::Release);
        }

        self.peers
            .on_connected(peer, predicted_headers_sync_time, protect_outbound);
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
        nc.send_message_to(peer, fbb.finished_data().to_vec());
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
            let now = unix_time_as_millis();
            // headers_sync_timeout
            if let Some(timeout) = state.headers_sync_timeout {
                if now > timeout && is_initial_block_download && !state.disconnect {
                    eviction.push(*peer);
                    state.disconnect = true;
                    continue;
                }
            }

            if let Some(is_outbound) = nc.get_peer(*peer).map(|peer| peer.is_outbound()) {
                if !state.chain_sync.protect && is_outbound {
                    let best_known_header = best_known_headers.get(peer);

                    let (tip_header, local_total_difficulty) = {
                        let chain_state = self.shared.chain_state().lock();
                        (
                            chain_state.tip_header().clone(),
                            chain_state.total_difficulty().clone(),
                        )
                    };
                    if best_known_header.map(HeaderView::total_difficulty)
                        >= Some(&local_total_difficulty)
                    {
                        if state.chain_sync.timeout != 0 {
                            state.chain_sync.timeout = 0;
                            state.chain_sync.work_header = None;
                            state.chain_sync.total_difficulty = None;
                            state.chain_sync.sent_getheaders = false;
                        }
                    } else if state.chain_sync.timeout == 0
                        || (best_known_header.is_some()
                            && best_known_header.map(HeaderView::total_difficulty)
                                >= state.chain_sync.total_difficulty.as_ref())
                    {
                        // Our best block known by this peer is behind our tip, and we're either noticing
                        // that for the first time, OR this peer was able to catch up to some earlier point
                        // where we checked against our tip.
                        // Either way, set a new timeout based on current tip.
                        state.chain_sync.timeout = now + CHAIN_SYNC_TIMEOUT;
                        state.chain_sync.work_header = Some(tip_header);
                        state.chain_sync.total_difficulty = Some(local_total_difficulty);
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
                                &state
                                    .chain_sync
                                    .work_header
                                    .clone()
                                    .expect("work_header be assigned"),
                            );
                        }
                    }
                }
            }
        }
        for peer in eviction {
            info!(target: "sync", "timeout eviction peer={}", peer);
            nc.disconnect(peer);
        }
    }

    fn start_sync_headers(&self, nc: &CKBProtocolContext) {
        let peers: Vec<PeerIndex> = self
            .peers
            .state
            .read()
            .iter()
            .filter(|(_, state)| !state.sync_started)
            .map(|(peer_id, _)| peer_id)
            .cloned()
            .collect();

        let tip = {
            let (header, total_difficulty) = {
                let chain_state = self.shared.chain_state().lock();
                (
                    chain_state.tip_header().clone(),
                    chain_state.total_difficulty().clone(),
                )
            };
            let best_known = self.best_known_header();
            if total_difficulty > *best_known.total_difficulty()
                || (&total_difficulty == best_known.total_difficulty()
                    && header.hash() < best_known.hash())
            {
                header
            } else {
                best_known.into_inner()
            }
        };
        for peer in peers {
            // Only sync with 1 peer if we're in IBD
            if self.is_initial_block_download() && self.n_sync.load(Ordering::Acquire) != 0 {
                break;
            }
            {
                let mut state = self.peers.state.write();
                if let Some(mut peer_state) = state.get_mut(&peer) {
                    if !peer_state.sync_started {
                        peer_state.sync_started = true;
                        self.n_sync.fetch_add(1, Ordering::Release);
                    }
                }
            }

            debug!(target: "sync", "start sync peer={}", peer);
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

        trace!(target: "sync", "poll find_blocks_to_fetch select peers");
        for peer in peers {
            if let Some(v_fetch) = self.get_blocks_to_fetch(peer) {
                if !v_fetch.is_empty() {
                    self.send_getblocks(&v_fetch, nc, peer);
                }
            }
        }
    }

    fn send_getblocks(&self, v_fetch: &[H256], nc: &CKBProtocolContext, peer: PeerIndex) {
        let fbb = &mut FlatBufferBuilder::new();
        let message = SyncMessage::build_get_blocks(fbb, v_fetch);
        fbb.finish(message, None);
        nc.send_message_to(peer, fbb.finished_data().to_vec());
        trace!(target: "sync", "send_getblocks len={:?} to peer={}", v_fetch.len() , peer);
    }
}

impl<CS: ChainStore> CKBProtocolHandler for Synchronizer<CS> {
    fn init(&mut self, nc: Box<dyn CKBProtocolContext>) {
        // NOTE: 100ms is what bitcoin use.
        nc.set_notify(Duration::from_millis(1000), SEND_GET_HEADERS_TOKEN);
        nc.set_notify(Duration::from_millis(1000), BLOCK_FETCH_TOKEN);
        nc.set_notify(Duration::from_millis(1000), TIMEOUT_EVICTION_TOKEN);
    }

    fn received(
        &mut self,
        nc: Box<dyn CKBProtocolContext>,
        peer_index: PeerIndex,
        data: bytes::Bytes,
    ) {
        let msg = match get_root::<SyncMessage>(&data) {
            Ok(msg) => msg,
            _ => {
                info!(target: "sync", "Peer {} sends us a malformed message", peer_index);
                nc.ban_peer(peer_index, BAD_MESSAGE_BAN_TIME);
                return;
            }
        };

        debug!(target: "sync", "msg {:?}", msg.payload_type());
        self.process(nc.as_ref(), peer_index, msg);
    }

    fn connected(&mut self, nc: Box<CKBProtocolContext>, peer_index: PeerIndex, _version: &str) {
        info!(target: "sync", "SyncProtocol.connected peer={}", peer_index);
        self.on_connected(nc.as_ref(), peer_index);
    }

    fn disconnected(&mut self, _nc: Box<CKBProtocolContext>, peer_index: PeerIndex) {
        info!(target: "sync", "SyncProtocol.disconnected peer={}", peer_index);
        let mut state = self.peers.state.write();
        if let Some(peer_state) = state.get(&peer_index) {
            // It shouldn't happen
            // fetch_sub wraps around on overflow, we still check manually
            // panic here to prevent some bug be hidden silently.
            if peer_state.sync_started && self.n_sync.fetch_sub(1, Ordering::Release) == 0 {
                panic!("Synchronizer n_sync overflow");
            }
        }
        state.remove(&peer_index);
        self.peers.disconnected(peer_index);
    }

    fn notify(&mut self, nc: Box<dyn CKBProtocolContext>, token: u64) {
        if !self.peers.state.read().is_empty() {
            match token {
                SEND_GET_HEADERS_TOKEN => {
                    self.start_sync_headers(nc.as_ref());
                }
                BLOCK_FETCH_TOKEN => {
                    self.find_blocks_to_fetch(nc.as_ref());
                }
                TIMEOUT_EVICTION_TOKEN => {
                    self.eviction(nc.as_ref());
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
    use self::block_process::BlockProcess;
    use self::headers_process::HeadersProcess;
    use super::*;
    use ckb_chain::chain::ChainBuilder;
    use ckb_chain_spec::consensus::Consensus;
    use ckb_core::block::BlockBuilder;
    use ckb_core::header::{Header, HeaderBuilder};
    use ckb_core::script::Script;
    use ckb_core::transaction::{CellInput, CellOutput, Transaction, TransactionBuilder};
    use ckb_core::Capacity;
    use ckb_db::memorydb::MemoryKeyValueDB;
    use ckb_network::{
        multiaddr::ToMultiaddr, Behaviour, CKBProtocolContext, Peer, PeerId, PeerIndex, ProtocolId,
        SessionType, TargetSession,
    };
    use ckb_notify::{NotifyController, NotifyService};
    use ckb_protocol::{Block as FbsBlock, Headers as FbsHeaders};
    use ckb_shared::shared::SharedBuilder;
    use ckb_shared::store::{ChainKVStore, ChainStore};
    use ckb_util::Mutex;
    #[cfg(not(disable_faketime))]
    use faketime;
    use flatbuffers::{get_root, FlatBufferBuilder};
    use fnv::{FnvHashMap, FnvHashSet};
    use numext_fixed_uint::U256;
    use std::ops::Deref;
    use std::time::Duration;

    fn start_chain(
        consensus: Option<Consensus>,
        notify: Option<NotifyController>,
    ) -> (
        ChainController,
        Shared<ChainKVStore<MemoryKeyValueDB>>,
        NotifyController,
    ) {
        let mut builder = SharedBuilder::<MemoryKeyValueDB>::new();
        if let Some(consensus) = consensus {
            builder = builder.consensus(consensus);
        }
        let shared = builder.build().unwrap();

        let notify = notify.unwrap_or_else(|| NotifyService::default().start::<&str>(None));
        let chain_service = ChainBuilder::new(shared.clone(), notify.clone())
            .verification(false)
            .build();
        let chain_controller = chain_service.start::<&str>(None);

        (chain_controller, shared, notify)
    }

    fn gen_synchronizer<CS: ChainStore>(
        chain_controller: ChainController,
        shared: Shared<CS>,
    ) -> Synchronizer<CS> {
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
            .output(CellOutput::new(
                Capacity::zero(),
                vec![],
                Script::default(),
                None,
            ))
            .build()
    }

    fn gen_block(parent_header: &Header, difficulty: U256, nonce: u64) -> Block {
        let now = 1 + parent_header.timestamp();
        let number = parent_header.number() + 1;
        let cellbase = create_cellbase(number);
        let header_builder = HeaderBuilder::default()
            .parent_hash(parent_header.hash().clone())
            .timestamp(now)
            .number(number)
            .difficulty(difficulty)
            .nonce(nonce);

        BlockBuilder::default()
            .transaction(cellbase)
            .with_header_builder(header_builder)
    }

    fn insert_block<CS: ChainStore>(
        chain_controller: &ChainController,
        shared: &Shared<CS>,
        nonce: u64,
        number: BlockNumber,
    ) {
        let parent = shared
            .block_header(&shared.block_hash(number - 1).unwrap())
            .unwrap();
        let difficulty = shared.calculate_difficulty(&parent).unwrap();
        let block = gen_block(&parent, difficulty, nonce);

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

        let locator = synchronizer.get_locator(shared.chain_state().lock().tip_header());

        let mut expect = Vec::new();

        for i in index.iter() {
            expect.push(shared.block_hash(*i).unwrap());
        }
        //genesis_hash must be the last one
        expect.push(shared.genesis_hash().clone());

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

        let locator1 = synchronizer1.get_locator(shared1.chain_state().lock().tip_header());

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
            let new_block = gen_block(&parent, difficulty, i);
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
        for i in 1..=block_number {
            let difficulty = shared2.calculate_difficulty(&parent).unwrap();
            let new_block = gen_block(&parent, difficulty, i + 100);

            chain_controller2
                .process_block(Arc::new(new_block.clone()))
                .expect("process block ok");
            parent = new_block.header().clone();
        }

        let synchronizer1 = gen_synchronizer(chain_controller1.clone(), shared1.clone());
        let synchronizer2 = gen_synchronizer(chain_controller2.clone(), shared2.clone());
        let locator1 = synchronizer1.get_locator(shared1.chain_state().lock().tip_header());

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

        let header = synchronizer.get_ancestor(&shared.chain_state().lock().tip_hash(), 100);
        let tip = synchronizer.get_ancestor(&shared.chain_state().lock().tip_hash(), 199);
        let noop = synchronizer.get_ancestor(&shared.chain_state().lock().tip_hash(), 200);
        assert!(tip.is_some());
        assert!(header.is_some());
        assert!(noop.is_none());
        assert_eq!(
            tip.unwrap(),
            shared.chain_state().lock().tip_header().clone()
        );
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
        let peer: PeerIndex = 0.into();

        let mut blocks: Vec<Block> = Vec::new();
        let mut parent = shared1
            .block_header(&shared1.block_hash(0).unwrap())
            .unwrap();
        for i in 1..block_number {
            let difficulty = shared1.calculate_difficulty(&parent).unwrap();
            let new_block = gen_block(&parent, difficulty, i + 100);

            chain_controller1
                .process_block(Arc::new(new_block.clone()))
                .expect("process block ok");
            parent = new_block.header().clone();
            blocks.push(new_block);
        }
        let synchronizer = gen_synchronizer(chain_controller2.clone(), shared2.clone());
        let chain1_last_block = blocks.last().cloned().unwrap();
        blocks.into_iter().for_each(|block| {
            synchronizer.insert_new_block(peer, block);
        });
        assert_eq!(
            chain1_last_block.header(),
            shared2.chain_state().lock().tip_header()
        );
    }

    #[test]
    fn test_get_locator_response() {
        let consensus = Consensus::default();
        let (chain_controller, shared, _notify) = start_chain(Some(consensus), None);
        let block_number = 200;

        let mut blocks: Vec<Block> = Vec::new();
        let mut parent = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
        for i in 1..=block_number {
            let difficulty = shared.calculate_difficulty(&parent).unwrap();
            let new_block = gen_block(&parent, difficulty, i + 100);
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
                assert_eq!(header.parent_hash(), &parent.hash());
            }
        }
    }

    #[derive(Clone)]
    struct DummyNetworkContext {
        pub peers: FnvHashMap<PeerIndex, Peer>,
        pub disconnected: Arc<Mutex<FnvHashSet<PeerIndex>>>,
    }

    fn mock_peer_info() -> Peer {
        Peer::new(
            0.into(),
            SessionType::Outbound,
            PeerId::random(),
            "/ip4/127.0.0.1".to_multiaddr().expect("parse multiaddr"),
            false,
        )
    }

    fn mock_header_view(total_difficulty: u64) -> HeaderView {
        HeaderView::new(
            HeaderBuilder::default().build(),
            U256::from(total_difficulty),
            0,
        )
    }

    impl CKBProtocolContext for DummyNetworkContext {
        // Interact with underlying p2p service
        fn set_notify(&self, _interval: Duration, _token: u64) {
            unimplemented!();
        }
        fn send_message_to(&self, _peer_index: PeerIndex, _data: Vec<u8>) {}
        fn filter_broadcast(&self, _target: TargetSession, _data: Vec<u8>) {
            unimplemented!();
        }
        fn disconnect(&self, peer_index: PeerIndex) {
            self.disconnected.lock().insert(peer_index);
        }
        // Interact with NetworkState
        fn get_peer(&self, peer_index: PeerIndex) -> Option<Peer> {
            self.peers.get(&peer_index).cloned()
        }
        fn connected_peers(&self) -> Vec<PeerIndex> {
            unimplemented!();
        }
        fn report_peer(&self, _peer_index: PeerIndex, _behaviour: Behaviour) {}
        fn ban_peer(&self, _peer_index: PeerIndex, _timeout: Duration) {}
        // Other methods
        fn protocol_id(&self) -> ProtocolId {
            unimplemented!();
        }
    }

    fn mock_network_context(peer_num: usize) -> DummyNetworkContext {
        let mut peers = FnvHashMap::default();
        for peer in 0..peer_num {
            peers.insert(peer.into(), mock_peer_info());
        }
        DummyNetworkContext {
            peers,
            disconnected: Arc::new(Mutex::new(FnvHashSet::default())),
        }
    }

    #[test]
    fn test_sync_process() {
        let _ = env_logger::try_init();
        let consensus = Consensus::default();
        let notify = NotifyService::default().start::<&str>(None);
        let (chain_controller1, shared1, _) =
            start_chain(Some(consensus.clone()), Some(notify.clone()));
        let (chain_controller2, shared2, _) =
            start_chain(Some(consensus.clone()), Some(notify.clone()));
        let num = 200;

        for i in 1..num {
            insert_block(&chain_controller1, &shared1, i, i);
        }

        let synchronizer1 = gen_synchronizer(chain_controller1.clone(), shared1.clone());

        let locator1 = synchronizer1.get_locator(&shared1.chain_state().lock().tip_header());

        for i in 1..=num {
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

        println!(
            "headers\n {:#?}",
            headers
                .iter()
                .map(|h| format!(
                    "{} hash({}) timestamp({}) parent({})",
                    h.number(),
                    h.hash(),
                    h.timestamp(),
                    h.parent_hash(),
                ))
                .collect::<Vec<_>>()
        );

        let fbb = &mut FlatBufferBuilder::new();
        let fbs_headers = FbsHeaders::build(fbb, &headers);
        fbb.finish(fbs_headers, None);
        let fbs_headers = get_root::<FbsHeaders>(fbb.finished_data());

        let peer: PeerIndex = 1.into();
        HeadersProcess::new(&fbs_headers, &synchronizer1, peer, &mock_network_context(0))
            .execute()
            .unwrap();

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

        for block in &fetched_blocks {
            let fbb = &mut FlatBufferBuilder::new();
            let fbs_block = FbsBlock::build(fbb, block);
            fbb.finish(fbs_block, None);
            let fbs_block = get_root::<FbsBlock>(fbb.finished_data());

            BlockProcess::new(&fbs_block, &synchronizer1, peer, &mock_network_context(0))
                .execute()
                .unwrap();
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
    }

    #[cfg(not(disable_faketime))]
    #[test]
    fn test_header_sync_timeout() {
        use std::iter::FromIterator;
        let faketime_file = faketime::millis_tempfile(0).expect("create faketime file");
        faketime::enable(&faketime_file);

        let (chain_controller, shared, _notify) = start_chain(None, None);

        let synchronizer = gen_synchronizer(chain_controller.clone(), shared.clone());

        let network_context = mock_network_context(5);
        faketime::write_millis(&faketime_file, MAX_TIP_AGE * 2).expect("write millis");
        assert!(synchronizer.is_initial_block_download());
        let peers = synchronizer.peers();
        // protect should not effect headers_timeout
        peers.on_connected(0.into(), 0, true);
        peers.on_connected(1.into(), 0, false);
        peers.on_connected(2.into(), MAX_TIP_AGE * 2, false);
        synchronizer.eviction(&network_context);
        let disconnected = network_context.disconnected.lock();
        assert_eq!(
            disconnected.deref(),
            &FnvHashSet::from_iter(vec![0, 1].into_iter().map(Into::into))
        )
    }

    #[cfg(not(disable_faketime))]
    #[test]
    fn test_chain_sync_timeout() {
        use std::iter::FromIterator;
        let faketime_file = faketime::millis_tempfile(0).expect("create faketime file");
        faketime::enable(&faketime_file);

        let consensus = Consensus::default();
        let header = HeaderBuilder::default()
            .difficulty(U256::from(2u64))
            .build();
        let block = BlockBuilder::default().header(header).build();
        let consensus = consensus.set_genesis_block(block);

        let (chain_controller, shared, _notify) = start_chain(Some(consensus), None);

        assert_eq!(
            shared.chain_state().lock().total_difficulty(),
            &U256::from(2u64)
        );

        let synchronizer = gen_synchronizer(chain_controller.clone(), shared.clone());

        let network_context = mock_network_context(6);
        let peers = synchronizer.peers();
        //6 peers do not trigger header sync timeout
        peers.on_connected(0.into(), MAX_TIP_AGE * 2, true);
        peers.on_connected(1.into(), MAX_TIP_AGE * 2, true);
        peers.on_connected(2.into(), MAX_TIP_AGE * 2, true);
        peers.on_connected(3.into(), MAX_TIP_AGE * 2, false);
        peers.on_connected(4.into(), MAX_TIP_AGE * 2, false);
        peers.on_connected(5.into(), MAX_TIP_AGE * 2, false);
        peers.new_header_received(0.into(), &mock_header_view(1));
        peers.new_header_received(2.into(), &mock_header_view(3));
        peers.new_header_received(3.into(), &mock_header_view(1));
        peers.new_header_received(5.into(), &mock_header_view(3));
        synchronizer.eviction(&network_context);
        {
            assert!({ network_context.disconnected.lock().is_empty() });
            let peer_state = peers.state.read();
            assert_eq!(peer_state.get(&0.into()).unwrap().chain_sync.protect, true);
            assert_eq!(peer_state.get(&1.into()).unwrap().chain_sync.protect, true);
            assert_eq!(peer_state.get(&2.into()).unwrap().chain_sync.protect, true);
            //protect peer is protected from disconnection
            assert!(peer_state
                .get(&2.into())
                .unwrap()
                .chain_sync
                .work_header
                .is_none());
            assert_eq!(peer_state.get(&3.into()).unwrap().chain_sync.protect, false);
            assert_eq!(peer_state.get(&4.into()).unwrap().chain_sync.protect, false);
            assert_eq!(peer_state.get(&5.into()).unwrap().chain_sync.protect, false);
            // Our best block known by this peer is behind our tip, and we're either noticing
            // that for the first time, OR this peer was able to catch up to some earlier point
            // where we checked against our tip.
            // Either way, set a new timeout based on current tip.
            let (tip, total_difficulty) = {
                let chain_state = shared.chain_state().lock();
                let header = chain_state.tip_header().clone();
                let total_difficulty = chain_state.total_difficulty().clone();
                (header, total_difficulty)
            };
            assert_eq!(
                peer_state.get(&3.into()).unwrap().chain_sync.work_header,
                Some(tip.clone())
            );
            assert_eq!(
                peer_state
                    .get(&3.into())
                    .unwrap()
                    .chain_sync
                    .total_difficulty,
                Some(total_difficulty.clone())
            );
            assert_eq!(
                peer_state.get(&4.into()).unwrap().chain_sync.work_header,
                Some(tip)
            );
            assert_eq!(
                peer_state
                    .get(&4.into())
                    .unwrap()
                    .chain_sync
                    .total_difficulty,
                Some(total_difficulty)
            );
            assert_eq!(
                peer_state.get(&3.into()).unwrap().chain_sync.timeout,
                CHAIN_SYNC_TIMEOUT
            );
            assert_eq!(
                peer_state.get(&4.into()).unwrap().chain_sync.timeout,
                CHAIN_SYNC_TIMEOUT
            );
        }
        faketime::write_millis(&faketime_file, CHAIN_SYNC_TIMEOUT + 1).expect("write millis");
        synchronizer.eviction(&network_context);
        {
            let peer_state = peers.state.read();
            // No evidence yet that our peer has synced to a chain with work equal to that
            // of our tip, when we first detected it was behind. Send a single getheaders
            // message to give the peer a chance to update us.
            assert!({ network_context.disconnected.lock().is_empty() });
            assert_eq!(
                peer_state.get(&3.into()).unwrap().chain_sync.timeout,
                unix_time_as_millis() + EVICTION_HEADERS_RESPONSE_TIME
            );
            assert_eq!(
                peer_state.get(&4.into()).unwrap().chain_sync.timeout,
                unix_time_as_millis() + EVICTION_HEADERS_RESPONSE_TIME
            );
        }
        faketime::write_millis(
            &faketime_file,
            unix_time_as_millis() + EVICTION_HEADERS_RESPONSE_TIME + 1,
        )
        .expect("write millis");
        synchronizer.eviction(&network_context);
        {
            // Peer(3,4) run out of time to catch up!
            let disconnected = network_context.disconnected.lock();
            assert_eq!(
                disconnected.deref(),
                &FnvHashSet::from_iter(vec![3, 4].into_iter().map(Into::into))
            )
        }
    }
}
