#![cfg_attr(feature = "cargo-clippy", allow(needless_pass_by_value))]

use super::compact_block::{short_transaction_id, short_transaction_id_keys, CompactBlock};
use bigint::H256;
use block_process::BlockProcess;
use ckb_chain::chain::ChainProvider;
use ckb_protocol;
use ckb_time::now_ms;
use core::block::IndexedBlock;
use core::header::IndexedHeader;
use core::transaction::{IndexedTransaction, ProposalShortId, ProposalTransaction};
use core::BlockNumber;
use fnv::{FnvHashMap, FnvHashSet};
use futures::future;
use futures::future::lazy;
use getdata_process::GetDataProcess;
use getheaders_process::GetHeadersProcess;
use headers_process::HeadersProcess;
use network::NetworkContextExt;
use network::{NetworkContext, NetworkProtocolHandler, PeerId, Severity, TimerToken};
use pool::txs_pool::TransactionPool;
use protobuf;
use std::collections::hash_map;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use synchronizer::Synchronizer;
use tokio;
use util::Mutex;

use {
    CHAIN_SYNC_TIMEOUT, EVICTION_TEST_RESPONSE_TIME, MAX_OUTBOUND_PEERS_TO_PROTECT_FROM_DISCONNECT,
};

pub const SEND_GET_HEADERS_TOKEN: TimerToken = 1;
pub const BLOCK_FETCH_TOKEN: TimerToken = 2;
pub const TX_PROPOSAL_TOKEN: TimerToken = 3;

fn is_outbound(nc: &NetworkContext, peer: PeerId) -> Option<bool> {
    nc.session_info(peer)
        .map(|session_info| session_info.originated)
}

pub struct SyncProtocol<C> {
    pub synchronizer: Synchronizer<C>,
}

impl<C: ChainProvider + 'static> SyncProtocol<C> {
    pub fn new(synchronizer: Synchronizer<C>) -> Self {
        SyncProtocol { synchronizer }
    }

    pub fn handle_getheaders(
        synchronizer: Synchronizer<C>,
        nc: Box<NetworkContext>,
        peer: PeerId,
        message: &ckb_protocol::GetHeaders,
    ) {
        GetHeadersProcess::new(message, &synchronizer, &peer, nc.as_ref()).execute()
    }

    pub fn handle_headers(
        synchronizer: Synchronizer<C>,
        nc: Box<NetworkContext>,
        peer: PeerId,
        message: &ckb_protocol::Headers,
    ) {
        HeadersProcess::new(message, &synchronizer, &peer, nc.as_ref()).execute()
    }

    fn handle_getdata(
        synchronizer: Synchronizer<C>,
        nc: Box<NetworkContext>,
        peer: PeerId,
        message: &ckb_protocol::GetData,
    ) {
        GetDataProcess::new(message, &synchronizer, &peer, nc.as_ref()).execute()
    }

    // fn handle_cmpt_block(
    //     synchronizer: Synchronizer<C>,
    //     nc: Box<NetworkContext>,
    //     peer: PeerId,
    //     message: &ckb_protocol::CompactBlock,
    // ) {
    //     CompactBlockProcess::new(message, &synchronizer, &peer, nc.as_ref()).execute()
    // }

    fn handle_block(
        synchronizer: Synchronizer<C>,
        nc: Box<NetworkContext>,
        peer: PeerId,
        message: &ckb_protocol::Block,
    ) {
        BlockProcess::new(message, &synchronizer, &peer, nc.as_ref()).execute()
    }

    pub fn find_blocks_to_fetch(synchronizer: Synchronizer<C>, nc: Box<NetworkContext>) {
        let peers: Vec<PeerId> = {
            synchronizer
                .peers
                .state
                .read()
                .iter()
                .filter(|(_, state)| state.sync_started)
                .map(|(peer_id, _)| peer_id)
                .cloned()
                .collect()
        };
        debug!(target: "sync", "poll find_blocks_to_fetch select peers");
        for peer in peers {
            let ret = synchronizer.get_blocks_to_fetch(peer);
            if let Some(v_fetch) = ret {
                Self::send_block_getdata(&v_fetch, nc.as_ref(), peer);
            }
        }
    }

    fn send_block_getdata(v_fetch: &[H256], nc: &NetworkContext, peer: PeerId) {
        let mut payload = ckb_protocol::Payload::new();
        let mut getdata = ckb_protocol::GetData::new();
        let inventory = v_fetch
            .iter()
            .map(|h| {
                let mut inventory = ckb_protocol::Inventory::new();
                inventory.set_inv_type(ckb_protocol::InventoryType::MSG_BLOCK);
                inventory.set_hash(h.to_vec());
                inventory
            })
            .collect();
        getdata.set_inventory(inventory);
        payload.set_getdata(getdata);

        let _ = nc.send_payload(peer, payload);
        debug!(target: "sync", "send_block_getdata len={:?} to peer={:?}", v_fetch.len() , peer);
    }

    fn on_connected(synchronizer: Synchronizer<C>, nc: &NetworkContext, peer: PeerId) {
        let tip = synchronizer.tip_header();
        let timeout = synchronizer.get_headers_sync_timeout(&tip);

        let protect_outbound = is_outbound(nc, peer).unwrap_or_else(|| false)
            && synchronizer
                .outbound_peers_with_protect
                .load(Ordering::Acquire)
                < MAX_OUTBOUND_PEERS_TO_PROTECT_FROM_DISCONNECT;

        if protect_outbound {
            synchronizer
                .outbound_peers_with_protect
                .fetch_add(1, Ordering::Release);
        }

        synchronizer
            .peers
            .on_connected(&peer, timeout, protect_outbound);
        synchronizer.n_sync.fetch_add(1, Ordering::Release);
        Self::send_getheaders_to_peer(synchronizer, nc, peer, &tip);
    }

    pub fn eviction(synchronizer: Synchronizer<C>, nc: &NetworkContext) {
        let mut peer_state = synchronizer.peers.state.write();
        let best_known_headers = synchronizer.peers.best_known_headers.read();
        let is_initial_block_download = synchronizer.is_initial_block_download();
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
                    let chain_tip = { synchronizer.chain.tip_header().read().clone() };

                    if best_known_header.is_some()
                        && best_known_header.unwrap().total_difficulty >= chain_tip.total_difficulty
                    {
                        if state.chain_sync.timeout != 0 {
                            state.chain_sync.timeout = 0;
                            state.chain_sync.work_header = None;
                            state.chain_sync.sent_getheaders = false;
                        }
                    } else if state.chain_sync.timeout == 0
                        || (best_known_header.is_some() && state.chain_sync.work_header.is_some()
                            && best_known_header.unwrap().total_difficulty
                                >= state
                                    .chain_sync
                                    .work_header
                                    .clone()
                                    .unwrap()
                                    .total_difficulty)
                    {
                        state.chain_sync.timeout = now + CHAIN_SYNC_TIMEOUT;
                        state.chain_sync.work_header = Some(chain_tip);
                        state.chain_sync.sent_getheaders = false;
                    } else if state.chain_sync.timeout > 0 && now > state.chain_sync.timeout {
                        if state.chain_sync.sent_getheaders {
                            eviction.push(*peer);
                            state.disconnect = true;
                        } else {
                            state.chain_sync.sent_getheaders = true;
                            state.chain_sync.timeout = now + EVICTION_TEST_RESPONSE_TIME;
                            Self::send_getheaders_to_peer(
                                synchronizer.clone(),
                                nc,
                                *peer,
                                &state.chain_sync.work_header.clone().unwrap().header,
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

    fn send_getheaders_to_all(synchronizer: Synchronizer<C>, nc: Box<NetworkContext>) {
        let peers: Vec<PeerId> = {
            synchronizer
                .peers
                .state
                .read()
                .iter()
                .filter(|(_, state)| state.sync_started)
                .map(|(peer_id, _)| peer_id)
                .cloned()
                .collect()
        };
        debug!(target: "sync", "send_getheaders to peers= {:?}", &peers);
        let tip = synchronizer.tip_header();
        for peer in peers {
            Self::send_getheaders_to_peer(synchronizer.clone(), nc.as_ref(), peer, &tip);
        }
    }

    fn send_getheaders_to_peer(
        synchronizer: Synchronizer<C>,
        nc: &NetworkContext,
        peer: PeerId,
        tip: &IndexedHeader,
    ) {
        let locator_hash = synchronizer.get_locator(tip);
        let mut payload = ckb_protocol::Payload::new();
        let mut getheaders = ckb_protocol::GetHeaders::new();
        let locator_hash = locator_hash.into_iter().map(|hash| hash.to_vec()).collect();
        getheaders.set_version(0);
        getheaders.set_block_locator_hashes(locator_hash);
        getheaders.set_hash_stop(H256::zero().to_vec());
        payload.set_getheaders(getheaders);
        let _ = nc.send_payload(peer, payload);
        debug!(target: "sync", "send_getheaders_to_peer getheaders {:?} to peer={:?}", tip.number ,peer);
    }

    fn process(
        synchronizer: Synchronizer<C>,
        nc: Box<NetworkContext>,
        peer: PeerId,
        payload: ckb_protocol::Payload,
    ) {
        if payload.has_getheaders() {
            Self::handle_getheaders(synchronizer, nc, peer, payload.get_getheaders());
        } else if payload.has_headers() {
            Self::handle_headers(synchronizer, nc, peer, payload.get_headers());
        } else if payload.has_getdata() {
            Self::handle_getdata(synchronizer, nc, peer, payload.get_getdata());
        } else if payload.has_block() {
            Self::handle_block(synchronizer, nc, peer, payload.get_block());
        }
    }
}

impl<C: ChainProvider + 'static> NetworkProtocolHandler for SyncProtocol<C> {
    fn initialize(&self, nc: Box<NetworkContext>) {
        // NOTE: 100ms is what bitcoin use.
        let _ = nc.register_timer(SEND_GET_HEADERS_TOKEN, Duration::from_millis(100));
        let _ = nc.register_timer(BLOCK_FETCH_TOKEN, Duration::from_millis(100));
    }

    /// Called when new network packet received.
    fn read(&self, nc: Box<NetworkContext>, peer: &PeerId, _packet_id: u8, data: &[u8]) {
        match protobuf::parse_from_bytes::<ckb_protocol::Payload>(data) {
            Ok(payload) => {
                let synchronizer = self.synchronizer.clone();
                let peer = *peer;
                tokio::spawn(lazy(move || {
                    Self::process(synchronizer, nc, peer, payload);
                    future::ok(())
                }));
            }
            Err(err) => warn!(target: "sync", "Failed to parse protobuf, error={:?}", err),
        };
    }

    fn connected(&self, nc: Box<NetworkContext>, peer: &PeerId) {
        let synchronizer = self.synchronizer.clone();
        let peer = *peer;
        tokio::spawn(lazy(move || {
            if synchronizer.n_sync.load(Ordering::Acquire) == 0
                || !synchronizer.is_initial_block_download()
            {
                debug!(target: "sync", "init_getheaders peer={:?} connected", peer);

                Self::on_connected(synchronizer, nc.as_ref(), peer);
            }
            future::ok(())
        }));
    }

    fn disconnected(&self, _nc: Box<NetworkContext>, peer: &PeerId) {
        let synchronizer = self.synchronizer.clone();
        let peer = *peer;
        tokio::spawn(lazy(move || {
            info!(target: "sync", "peer={} SyncProtocol.disconnected", peer);
            synchronizer.peers.disconnected(&peer);
            future::ok(())
        }));
    }

    fn timeout(&self, nc: Box<NetworkContext>, token: TimerToken) {
        let synchronizer = self.synchronizer.clone();
        tokio::spawn(lazy(move || {
            if !synchronizer.peers.state.read().is_empty() {
                match token as usize {
                    SEND_GET_HEADERS_TOKEN => {
                        Self::send_getheaders_to_all(synchronizer, nc);
                    }
                    BLOCK_FETCH_TOKEN => {
                        Self::find_blocks_to_fetch(synchronizer, nc);
                    }
                    _ => unreachable!(),
                }
            } else {
                debug!(target: "sync", "no peers connected");
            }
            future::ok(())
        }));
    }
}

pub type TxProposalIdTable = FnvHashMap<BlockNumber, FnvHashSet<ProposalShortId>>;

#[derive(Default)]
struct RelayState {
    // TODO add size limit or use bloom filter
    pub received_blocks: Mutex<FnvHashSet<H256>>,
    pub received_transactions: Mutex<FnvHashSet<H256>>,
    pub pending_compact_blocks: Mutex<FnvHashMap<H256, CompactBlock>>,
    pub inflight_proposals: Mutex<TxProposalIdTable>,
    pub pending_proposals_request: Mutex<FnvHashMap<PeerId, TxProposalIdTable>>,
}

pub struct RelayProtocol<C> {
    synchronizer: Synchronizer<C>,
    tx_pool: Arc<TransactionPool<C>>,
    state: Arc<RelayState>,
}

impl<C> Clone for RelayProtocol<C>
where
    C: ChainProvider,
{
    fn clone(&self) -> RelayProtocol<C> {
        RelayProtocol {
            synchronizer: self.synchronizer.clone(),
            tx_pool: Arc::clone(&self.tx_pool),
            state: Arc::clone(&self.state),
        }
    }
}

impl<C: ChainProvider + 'static> RelayProtocol<C> {
    pub fn new(synchronizer: Synchronizer<C>, tx_pool: &Arc<TransactionPool<C>>) -> Self {
        RelayProtocol {
            synchronizer,
            tx_pool: Arc::clone(tx_pool),
            state: Arc::new(RelayState::default()),
        }
    }

    pub fn relay(&self, nc: &NetworkContext, source: PeerId, payload: &ckb_protocol::Payload) {
        let peer_ids = self
            .synchronizer
            .peers
            .state
            .read()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for (peer_id, _session) in nc.sessions(&peer_ids) {
            if peer_id != source {
                let _ = nc.send_payload(peer_id, payload.clone());
            }
        }
    }

    pub fn request_proposal_txs<'a>(
        &self,
        nc: &NetworkContext,
        block_number: BlockNumber,
        proposal_ids: impl Iterator<Item = &'a ProposalShortId>,
    ) {
        let proposal_ids: Vec<Vec<u8>> =
            if let Some(known_proposal_ids) = self.tx_pool.query_proposal_ids(&block_number) {
                let proposal_ids: FnvHashSet<ProposalShortId> = proposal_ids.cloned().collect();
                let request_proposal = proposal_ids.difference(&known_proposal_ids);

                match self.state.inflight_proposals.lock().entry(block_number) {
                    hash_map::Entry::Vacant(v) => {
                        v.insert(request_proposal.clone().cloned().collect());
                    }
                    hash_map::Entry::Occupied(mut o) => {
                        o.get_mut().extend(request_proposal.clone().cloned());
                    }
                }

                request_proposal.map(|id| id.to_vec()).collect()
            } else {
                proposal_ids.map(|id| id.to_vec()).collect()
            };

        let mut payload = ckb_protocol::Payload::new();
        let mut proposal_request = ckb_protocol::BlockProposalRequest::new();
        proposal_request.set_block_number(block_number);
        proposal_request.set_proposal_ids(proposal_ids.into());
        payload.set_block_proposal_request(proposal_request);
        let _ = nc.respond_payload(payload);
    }

    fn reconstruct_block(
        &self,
        compact_block: &CompactBlock,
        transactions: Vec<IndexedTransaction>,
    ) -> (Option<IndexedBlock>, Option<Vec<usize>>) {
        let (key0, key1) = short_transaction_id_keys(compact_block.nonce, &compact_block.header);

        let mut txs = transactions;
        txs.extend(self.tx_pool.commit.read().pool.get_vertices());
        txs.extend(self.tx_pool.orphan.read().pool.get_vertices());

        let mut txs_map = FnvHashMap::default();
        for tx in txs {
            let short_id = short_transaction_id(key0, key1, &tx.hash());
            txs_map.insert(short_id, tx);
        }

        let mut block_transactions = Vec::with_capacity(compact_block.short_ids.len());
        let mut missing_indexes = Vec::new();
        for (index, short_id) in compact_block.short_ids.iter().enumerate() {
            match txs_map.remove(short_id) {
                Some(tx) => block_transactions.insert(index, tx),
                None => missing_indexes.push(index),
            }
        }

        if missing_indexes.is_empty() {
            let block = IndexedBlock::new(
                compact_block.header.clone().into(),
                compact_block.uncles.clone(),
                block_transactions,
                compact_block.proposal_transactions.clone(),
            );

            (Some(block), None)
        } else {
            (None, Some(missing_indexes))
        }
    }

    fn process(&self, nc: Box<NetworkContext>, peer: &PeerId, mut payload: ckb_protocol::Payload) {
        if payload.has_transaction() {
            let tx: IndexedTransaction = payload.get_transaction().into();
            if !self.state.received_transactions.lock().insert(tx.hash()) {
                self.tx_pool.insert_candidate(tx);
                self.relay(nc.as_ref(), *peer, &payload);
            }
        } else if payload.has_block() {
            let block: IndexedBlock = payload.get_block().into();
            if !self.state.received_blocks.lock().insert(block.hash()) {
                self.request_proposal_txs(
                    nc.as_ref(),
                    block.number(),
                    block.proposal_transactions.iter().chain(
                        block
                            .uncles()
                            .iter()
                            .flat_map(|uncle| uncle.proposal_transactions()),
                    ),
                );

                self.synchronizer.process_new_block(*peer, block);
                self.relay(nc.as_ref(), *peer, &payload);
            }
        } else if payload.has_compact_block() {
            let compact_block: CompactBlock = payload.get_compact_block().into();
            debug!(target: "sync", "receive compact block from peer#{}, {} => {}",
                   peer,
                   compact_block.header().number,
                   compact_block.header().hash(),
            );
            if !self
                .state
                .received_blocks
                .lock()
                .insert(compact_block.header.hash())
            {
                self.request_proposal_txs(
                    nc.as_ref(),
                    compact_block.header.number,
                    compact_block.proposal_transactions.iter().chain(
                        compact_block
                            .uncles
                            .iter()
                            .flat_map(|uncle| uncle.proposal_transactions()),
                    ),
                );
                match self.reconstruct_block(&compact_block, Vec::new()) {
                    (Some(block), _) => {
                        self.synchronizer.process_new_block(*peer, block);
                        self.relay(nc.as_ref(), *peer, &payload);
                    }
                    (_, Some(missing_indexes)) => {
                        let mut payload = ckb_protocol::Payload::new();
                        let mut cbr = ckb_protocol::BlockTransactionsRequest::new();
                        cbr.set_hash(compact_block.header.hash().to_vec());
                        cbr.set_indexes(missing_indexes.into_iter().map(|i| i as u32).collect());
                        payload.set_block_transactions_request(cbr);
                        self.state
                            .pending_compact_blocks
                            .lock()
                            .insert(compact_block.header.hash(), compact_block);
                        let _ = nc.respond_payload(payload);
                    }
                    (None, None) => {
                        // TODO fail to reconstruct block, downgrade to header first?
                    }
                }
            }
        } else if payload.has_block_transactions_request() {
            let btr = payload.get_block_transactions_request();
            let hash = H256::from_slice(btr.get_hash());
            let indexes = btr.get_indexes();
            if let Some(block) = self.synchronizer.get_block(&hash) {
                let mut payload = ckb_protocol::Payload::new();
                let mut bt = ckb_protocol::BlockTransactions::new();
                bt.set_hash(hash.to_vec());
                bt.set_transactions(
                    indexes
                        .iter()
                        .filter_map(|i| block.commit_transactions.get(*i as usize))
                        .map(Into::into)
                        .collect(),
                );
                let _ = nc.respond_payload(payload);
            }
        } else if payload.has_block_transactions() {
            let bt = payload.get_block_transactions();
            let hash = H256::from_slice(bt.get_hash());
            if let Some(compact_block) = self.state.pending_compact_blocks.lock().remove(&hash) {
                let transactions: Vec<IndexedTransaction> =
                    bt.get_transactions().iter().map(Into::into).collect();
                if let (Some(block), _) = self.reconstruct_block(&compact_block, transactions) {
                    self.synchronizer.process_new_block(*peer, block);
                }
            }
        } else if payload.has_block_proposal_request() {
            let request = payload.get_block_proposal_request();
            let number = request.get_block_number();
            let proposal_ids = request
                .get_proposal_ids()
                .iter()
                .filter_map(|bytes| ProposalShortId::from_slice(&bytes));
            if let Some((txs, notfound)) = self.tx_pool.query_proposal(&number, proposal_ids) {
                if !txs.is_empty() {
                    let mut payload = ckb_protocol::Payload::new();
                    let mut response = ckb_protocol::BlockProposalResponse::new();
                    response.set_block_number(number);
                    response.set_transactions(txs.iter().map(Into::into).collect());
                    payload.set_block_proposal_response(response);
                    let _ = nc.respond_payload(payload);
                }
                if !notfound.is_empty() {
                    let mut pending_proposals_request = self.state.pending_proposals_request.lock();
                    let txs_table = pending_proposals_request
                        .entry(*peer)
                        .or_insert_with(FnvHashMap::default);
                    let mut txs = txs_table.entry(number).or_insert_with(FnvHashSet::default);
                    txs.extend(notfound.into_iter());
                }
            }
        } else if payload.has_block_proposal_response() {
            let mut response = payload.take_block_proposal_response();
            let block_number = response.get_block_number();
            let txs: FnvHashSet<ProposalTransaction> = response
                .take_transactions()
                .iter()
                .map(Into::into)
                .collect();

            let txs_ids: FnvHashSet<ProposalShortId> =
                txs.iter().map(|tx| tx.proposal_short_id()).collect();

            if let Some(proposals) = self.state.inflight_proposals.lock().get_mut(&block_number) {
                proposals.retain(|id| !txs_ids.contains(id));
            }
            self.tx_pool.proposal_n(block_number, txs);
        }
    }

    fn prune_tx_proposal_request(&self, nc: Box<NetworkContext>) {
        let mut pending_proposals_request = self.state.pending_proposals_request.lock();
        for (peer, mut txs_table) in pending_proposals_request.iter_mut() {
            for (number, mut proposal_ids) in txs_table.iter_mut() {
                let result = { self.tx_pool.query_proposal(number, proposal_ids.drain()) };
                if let Some((txs, notfound)) = result {
                    if !txs.is_empty() {
                        let mut payload = ckb_protocol::Payload::new();
                        let mut response = ckb_protocol::BlockProposalResponse::new();
                        response.set_block_number(*number);
                        response.set_transactions(txs.iter().map(Into::into).collect());
                        payload.set_block_proposal_response(response);
                        let _ = nc.send_payload(*peer, payload);
                    }
                    if !notfound.is_empty() {
                        *proposal_ids = notfound.into_iter().collect();
                    }
                }
            }
            txs_table.retain(|_, txs| !txs.is_empty());
        }
        pending_proposals_request.retain(|_, table| !table.is_empty());
    }
}

impl<C: ChainProvider + 'static> NetworkProtocolHandler for RelayProtocol<C> {
    fn initialize(&self, nc: Box<NetworkContext>) {
        let _ = nc.register_timer(TX_PROPOSAL_TOKEN, Duration::from_millis(100));
    }
    /// Called when new network packet received.
    fn read(&self, nc: Box<NetworkContext>, peer: &PeerId, _packet_id: u8, data: &[u8]) {
        let protocol = self.clone();
        let peer = *peer;
        match protobuf::parse_from_bytes::<ckb_protocol::Payload>(data) {
            Ok(payload) => {
                tokio::spawn(lazy(move || {
                    protocol.process(nc, &peer, payload);
                    future::ok(())
                }));
            }
            Err(err) => warn!(target: "sync", "Failed to parse protobuf, error={:?}", err),
        };
    }

    fn connected(&self, _nc: Box<NetworkContext>, peer: &PeerId) {
        info!(target: "sync", "peer={} RelayProtocol.connected", peer);
        // do nothing
    }

    fn disconnected(&self, _nc: Box<NetworkContext>, peer: &PeerId) {
        info!(target: "sync", "peer={} RelayProtocol.disconnected", peer);
        // TODO
    }

    fn timeout(&self, nc: Box<NetworkContext>, token: TimerToken) {
        let protocol = self.clone();
        tokio::spawn(lazy(move || {
            if !protocol.synchronizer.peers.state.read().is_empty() {
                match token as usize {
                    TX_PROPOSAL_TOKEN => protocol.prune_tx_proposal_request(nc),
                    _ => unreachable!(),
                }
            } else {
                debug!(target: "sync", "no peers connected");
            }
            future::ok(())
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bigint::U256;
    use ckb_chain::chain::Chain;
    use ckb_chain::consensus::Consensus;
    use ckb_chain::store::ChainKVStore;
    use ckb_chain::COLUMNS;
    use ckb_notify::Notify;
    use ckb_time::{now_ms, set_mock_timer};
    use config::Config;
    use db::memorydb::MemoryKeyValueDB;
    use header_view::HeaderView;
    use network::{
        Error as NetworkError, NetworkContext, PacketId, PeerId, ProtocolId, SessionInfo, Severity,
        TimerToken,
    };
    use std::iter::FromIterator;
    use std::ops::Deref;
    use std::time::Duration;
    use MAX_TIP_AGE;

    fn mock_session_info() -> SessionInfo {
        SessionInfo {
            id: None,
            client_version: "mock".to_string(),
            protocol_version: 0,
            capabilities: vec![],
            peer_capabilities: vec![],
            ping: None,
            originated: true,
            remote_address: "mock".to_string(),
            local_address: "mock".to_string(),
        }
    }

    fn mock_header_view(total_difficulty: u64) -> HeaderView {
        HeaderView {
            total_difficulty: U256::from(total_difficulty),
            header: IndexedHeader::default(),
        }
    }

    fn gen_chain(consensus: &Consensus) -> Chain<ChainKVStore<MemoryKeyValueDB>> {
        let db = MemoryKeyValueDB::open(COLUMNS as usize);
        let store = ChainKVStore { db };
        let chain = Chain::init(store, consensus.clone(), Notify::default()).unwrap();
        chain
    }

    #[derive(Clone)]
    struct DummyNetworkContext {
        pub sessions: FnvHashMap<PeerId, SessionInfo>,
        pub disconnected: Arc<Mutex<FnvHashSet<PeerId>>>,
    }

    impl NetworkContext for DummyNetworkContext {
        /// Send a packet over the network to another peer.
        fn send(&self, _peer: PeerId, _packet_id: PacketId, _data: Vec<u8>) {}

        /// Send a packet over the network to another peer using specified protocol.
        fn send_protocol(
            &self,
            _protocol: ProtocolId,
            _peer: PeerId,
            _packet_id: PacketId,
            _data: Vec<u8>,
        ) {
        }

        /// Respond to a current network message. Panics if no there is no packet in the context. If the session is expired returns nothing.
        fn respond(&self, _packet_id: PacketId, _data: Vec<u8>) {
            unimplemented!();
        }

        /// Report peer. Depending on the report, peer may be disconnected and possibly banned.
        fn report_peer(&self, peer: PeerId, _reason: Severity) {
            self.disconnected.lock().insert(peer);
        }

        /// Check if the session is still active.
        fn is_expired(&self) -> bool {
            false
        }

        /// Register a new IO timer. 'IoHandler::timeout' will be called with the token.
        fn register_timer(&self, _token: TimerToken, _delay: Duration) -> Result<(), NetworkError> {
            unimplemented!();
        }

        /// Returns peer identification string
        fn peer_client_version(&self, _peer: PeerId) -> String {
            unimplemented!();
        }

        /// Returns information on p2p session
        fn session_info(&self, peer: PeerId) -> Option<SessionInfo> {
            self.sessions.get(&peer).cloned()
        }

        /// Returns max version for a given protocol.
        fn protocol_version(&self, _protocol: ProtocolId, _peer: PeerId) -> Option<u8> {
            unimplemented!();
        }

        /// Returns this object's subprotocol name.
        fn subprotocol_name(&self) -> ProtocolId {
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
    fn test_header_sync_timeout() {
        let config = Consensus::default();
        let chain = Arc::new(gen_chain(&config));

        let synchronizer = Synchronizer::new(&chain, None, Config::default());

        let network_context = mock_network_context(5);

        set_mock_timer(MAX_TIP_AGE * 2);

        assert!(synchronizer.is_initial_block_download());

        let peers = synchronizer.peers();
        // protect should not effect headers_timeout
        peers.on_connected(&0, 0, true);
        peers.on_connected(&1, 0, false);
        peers.on_connected(&2, MAX_TIP_AGE * 2, false);

        SyncProtocol::eviction(synchronizer, &network_context);

        let disconnected = network_context.disconnected.lock();

        assert_eq!(
            disconnected.deref(),
            &FnvHashSet::from_iter(vec![0, 1].into_iter())
        )
    }

    #[test]
    fn test_chain_sync_timeout() {
        let mut consensus = Consensus::default();
        consensus.genesis_block.header.raw.difficulty = U256::from(2);
        let chain = Arc::new(gen_chain(&consensus));

        assert_eq!(chain.tip_header().read().total_difficulty, U256::from(2));

        let synchronizer = Synchronizer::new(&chain, None, Config::default());

        let network_context = mock_network_context(6);

        let peers = synchronizer.peers();

        //6 peers do not trigger header sync timeout
        peers.on_connected(&0, MAX_TIP_AGE * 2, true);
        peers.on_connected(&1, MAX_TIP_AGE * 2, true);
        peers.on_connected(&2, MAX_TIP_AGE * 2, true);
        peers.on_connected(&3, MAX_TIP_AGE * 2, false);
        peers.on_connected(&4, MAX_TIP_AGE * 2, false);
        peers.on_connected(&5, MAX_TIP_AGE * 2, false);

        peers.new_header_received(&0, &mock_header_view(1));
        peers.new_header_received(&2, &mock_header_view(3));
        peers.new_header_received(&3, &mock_header_view(1));
        peers.new_header_received(&5, &mock_header_view(3));

        SyncProtocol::eviction(synchronizer.clone(), &network_context);

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
            let tip = { chain.tip_header().read().clone() };
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
        SyncProtocol::eviction(synchronizer.clone(), &network_context);
        {
            let peer_state = peers.state.read();
            // No evidence yet that our peer has synced to a chain with work equal to that
            // of our tip, when we first detected it was behind. Send a single getheaders
            // message to give the peer a chance to update us.
            assert!({ network_context.disconnected.lock().is_empty() });

            assert_eq!(
                peer_state.get(&3).unwrap().chain_sync.timeout,
                now_ms() + EVICTION_TEST_RESPONSE_TIME
            );
            assert_eq!(
                peer_state.get(&4).unwrap().chain_sync.timeout,
                now_ms() + EVICTION_TEST_RESPONSE_TIME
            );
        }

        set_mock_timer(now_ms() + EVICTION_TEST_RESPONSE_TIME + 1);
        SyncProtocol::eviction(synchronizer, &network_context);

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
