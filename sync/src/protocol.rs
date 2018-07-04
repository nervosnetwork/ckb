use super::chain::{BlockState, Chain};
use super::compact_block::{short_transaction_id, short_transaction_id_keys, CompactBlock};
use super::executor::{Executor, Task};
use super::peers::Peers;
use super::{BlockHeight, MAX_HEADERS_LEN, MAX_SCHEDULED_LEN};
use bigint::H256;
use core::block::Block;
use core::header::Header;
use core::transaction::Transaction;
use fnv::{FnvHashMap, FnvHashSet};
use nervos_chain::chain::ChainClient;
use nervos_protocol;
use network::{NetworkContext, NetworkProtocolHandler, PeerId, ProtocolId};
use pool::txs_pool::TransactionPool;
use protobuf::RepeatedField;
use std::cmp::min;
use std::collections::VecDeque;
use std::sync::Arc;
use util::{Mutex, RwLock};

pub const SYNC_PROTOCOL_ID: ProtocolId = *b"syn";

pub struct SyncProtocol<C> {
    pub chain: Arc<Chain<C>>,
    pub peers: Arc<RwLock<Peers>>,
}

impl<C: ChainClient + 'static> SyncProtocol<C> {
    pub fn new(chain: &Arc<Chain<C>>) -> Self {
        SyncProtocol {
            chain: Arc::clone(chain),
            peers: Arc::new(RwLock::new(Peers::default())),
        }
    }

    pub fn handle_getheaders(
        &self,
        nc: &NetworkContext,
        peer: PeerId,
        message: &nervos_protocol::GetHeaders,
    ) {
        info!(target: "sync", "handle_getheaders from peer {}", peer);
        let hash_stop = H256::from_slice(message.get_hash_stop());
        let block_locator_hashes: Vec<H256> = message
            .get_block_locator_hashes()
            .iter()
            .map(|hash| H256::from_slice(&hash[..]))
            .collect();
        if let Some(block_height) =
            self.locate_best_common_block(&hash_stop, &block_locator_hashes[..])
        {
            let headers: Vec<_> = (block_height + 1..block_height + 1 + MAX_HEADERS_LEN as u64)
                .filter_map(|block_height| self.chain.provider().block_hash(block_height))
                .take_while(|block_hash| block_hash != &hash_stop)
                .filter_map(|block_hash| self.chain.provider().block_header(&block_hash))
                .collect();
            // response headers
            let mut payload = nervos_protocol::Payload::new();
            let mut headers_proto = nervos_protocol::Headers::new();
            headers_proto.set_headers(RepeatedField::from_vec(
                headers.iter().map(Into::into).collect(),
            ));
            payload.set_headers(headers_proto);
            let _ = nc.respond(payload);
        } else {
            info!(target: "sync", "unknown block headers from peer {}", peer);
            // Got 'headers' message without known blocks
            // ban or close peers
        }
    }

    pub fn handle_headers(
        &self,
        nc: &NetworkContext,
        peer: PeerId,
        message: &nervos_protocol::Headers,
    ) {
        info!(target: "sync", "handle_headers from peer {}", peer);

        let mut headers: Vec<Header> = message.headers.iter().map(From::from).collect();
        if headers.is_empty() {
            return;
        }

        if headers.len() > MAX_HEADERS_LEN {
            //TODO: ban peer, possible DOS
            // nc.disable_peer(peer)
            return;
        }
        {
            self.peers.write().on_headers_received(peer);
        }

        let header0 = headers[0].clone();
        //check first header parent
        if self.chain.block_state(&header0.parent_hash) == BlockState::Unknown {
            info!(
                target: "sync",
                "Previous header of the first header from peer#{} `headers` message is unknown. First: {}. Previous: {}", 
                peer, header0.hash(), &header0.parent_hash
            );
            return;
        }

        let num_headers = headers.len();
        let first_unknown_index = match self.chain.block_state(&header0.hash()) {
            BlockState::Unknown => 0,
            _ => {
                // optimization: if last header is known, then all headers are also known
                let header_last = &headers[num_headers - 1];
                match self.chain.block_state(&header_last.hash()) {
                    BlockState::Unknown => {
                        if let Some(index) = headers.iter().skip(1).position(|header| {
                            self.chain.block_state(&header.hash()) == BlockState::Unknown
                        }) {
                            1 + index
                        } else {
                            info!(
                                target: "sync",
                                "`headers` message out of order from peer#{}", peer
                            );
                            return;
                        }
                    }
                    // else all headers are known
                    _ => {
                        info!(target: "sync", "Ignoring {} known headers from peer#{}", headers.len(), peer);
                        // but this peer is still useful for synchronization
                        {
                            self.peers.write().as_useful_peer(peer);
                        }
                        return;
                    }
                }
            }
        };
        let _last_known_hash = if first_unknown_index > 0 {
            headers[first_unknown_index - 1].hash()
        } else {
            header0.parent_hash
        };
        //TODO: check dead-end

        //TODO: verify_header
        // self.verify_headers();

        let new_headers = headers.split_off(first_unknown_index);

        info!(target: "sync", "on_headers new_headers");
        self.chain.schedule_blocks_headers(new_headers);
        {
            self.peers.write().as_useful_peer(peer);
        }
        self.execute_tasks(nc);
    }

    fn handle_getdata(
        &self,
        nc: &NetworkContext,
        peer: PeerId,
        message: &nervos_protocol::GetData,
    ) {
        info!(target: "sync", "handle_getdata from peer {}", peer);
        let inventory_vec = message.get_inventory();
        for inventory in inventory_vec.iter() {
            self.process_inventory(nc, peer, inventory);
        }
    }

    fn process_inventory(
        &self,
        nc: &NetworkContext,
        _peer: PeerId,
        inventory: &nervos_protocol::Inventory,
    ) {
        let inv_type = inventory.get_inv_type();
        match inv_type {
            nervos_protocol::InventoryType::MSG_BLOCK => {
                if let Some(ref block) = self
                    .chain
                    .provider()
                    .block(&H256::from(inventory.get_hash()))
                {
                    let mut payload = nervos_protocol::Payload::new();
                    payload.set_block(block.into());
                    let _ = nc.respond(payload);
                } else {
                    //Reponse notfound
                }
            }
            nervos_protocol::InventoryType::ERROR => {}
        }
    }

    fn handle_block(&self, nc: &NetworkContext, peer: PeerId, message: &nervos_protocol::Block) {
        info!(target: "sync", "handle_block from peer {}", peer);

        let block: Block = message.into();
        let block_hash = block.hash();
        {
            self.peers.write().on_block_received(peer, &block_hash);
        }
        let block_state = self.chain.block_state(&block_hash);

        match block_state {
            BlockState::DeadEnd => {
                //ban peer
            }
            BlockState::Verifying | BlockState::Stored => {
                self.peers.write().as_useful_peer(peer);
            }
            BlockState::Unknown | BlockState::Scheduled | BlockState::Requested => {
                let parent_state = self.chain.block_state(&block.header.parent_hash);
                match parent_state {
                    BlockState::DeadEnd => {
                        //ban peer
                    }
                    BlockState::Unknown => {
                        //if synchronizing forget block which parent is unknown
                        self.chain.forget_block(&block.hash());
                        // else self.unknown_blocks_pool.insert(block);
                    }
                    BlockState::Verifying | BlockState::Stored => {
                        {
                            self.peers.write().as_useful_peer(peer);
                        }
                        let mut blocks_to_verify: VecDeque<Block> = VecDeque::new();
                        let blocks_to_forget: Vec<_> =
                            blocks_to_verify.iter().map(|b| b.hash()).collect();
                        self.chain.forget_blocks_leave_header(&blocks_to_forget);
                        // TODO: impl switch fork
                        // blocks_to_verify.extend(
                        //     self.orphaned_blocks_pool
                        //         .remove_blocks_by_parent(&block.hash()),
                        // );
                        blocks_to_verify.push_front(block);

                        //TODO: Async?
                        while let Some(block) = blocks_to_verify.pop_front() {
                            // self.verifier.verify_block(block);
                            self.chain.insert_block(&block);
                        }
                        self.execute_tasks(nc);
                    }
                    BlockState::Requested | BlockState::Scheduled => {
                        {
                            self.peers.write().as_useful_peer(peer);
                        }
                        // TODO
                        // remember as orphan block
                        // self.orphaned_blocks_pool.insert(block);
                    }
                }
            }
        }
    }

    fn locate_best_common_block(&self, hash_stop: &H256, locator: &[H256]) -> Option<BlockHeight> {
        for block_hash in locator.iter().chain(&[*hash_stop]) {
            if let Some(block_height) = self.chain.provider().block_height(block_hash) {
                return Some(block_height);
            }

            // block with this hash is definitely not in the main chain (block_height has returned None)
            // but maybe it is in some fork? if so => we should find intersection with main chain
            // and this would be our best common block
            let mut block_hash = *block_hash;
            loop {
                let block_header = match self.chain.provider().block_header(&block_hash) {
                    None => break,
                    Some(block_header) => block_header,
                };

                if let Some(block_height) = self
                    .chain
                    .provider()
                    .block_height(&block_header.parent_hash)
                {
                    return Some(block_height);
                }

                block_hash = block_header.parent_hash;
            }
        }

        None
    }

    fn execute_tasks(&self, nc: &NetworkContext) {
        let mut tasks: Vec<Task> = Vec::new();

        let scheduled_len = self.chain.scheduled_len();

        let headers_idle_peers: Vec<_> = {
            self.peers
                .read()
                .idle_peers_for_headers()
                .iter()
                .cloned()
                .collect()
        };
        if !headers_idle_peers.is_empty() {
            if scheduled_len < MAX_SCHEDULED_LEN {
                for peer in &headers_idle_peers {
                    self.peers.write().on_headers_requested(*peer);
                }

                let block_locator_hashes = self.chain.get_locator();
                let headers_tasks = headers_idle_peers
                    .into_iter()
                    .map(|peer| Task::GetHeaders(peer, block_locator_hashes.clone()));
                tasks.extend(headers_tasks);
            } else {
                //ban peer
            }
        }

        let blocks_requests = self.chain.request_blocks_hashes(scheduled_len as u32);
        let blocks_idle_peers: Vec<_> = {
            self.peers
                .read()
                .idle_peers_for_blocks()
                .iter()
                .cloned()
                .collect()
        };
        info!(target: "sync", "execute_tasks blocks_idle_peers {:?}", blocks_idle_peers);
        tasks.extend(self.prepare_blocks_requests_tasks(blocks_idle_peers, blocks_requests));
        let executor = Executor { nc };
        for task in tasks {
            executor.execute(task);
        }
    }

    fn prepare_blocks_requests_tasks(&self, peers: Vec<usize>, mut hashes: Vec<H256>) -> Vec<Task> {
        use std::mem::swap;

        let mut tasks: Vec<Task> = Vec::new();
        if peers.is_empty() {
            return tasks;
        }

        let hashes_count = hashes.len();
        let peers_count = peers.len();

        // chunk requests by peers_count
        // TODO: we may need to duplicate pending blocks requests to peers
        let chunk_size = if peers_count > 1 {
            hashes_count / (peers_count - 1)
        } else {
            hashes_count
        };

        for peer in peers {
            let index = min(hashes.len(), chunk_size as usize);
            let mut chunk_hashes = hashes.split_off(index);
            swap(&mut chunk_hashes, &mut hashes);
            {
                self.peers.write().on_blocks_requested(peer, &chunk_hashes);
            }

            let mut getdata = nervos_protocol::GetData::new();
            let inventory = chunk_hashes
                .into_iter()
                .map(|h| {
                    let mut inventory = nervos_protocol::Inventory::new();
                    inventory.set_inv_type(nervos_protocol::InventoryType::MSG_BLOCK);
                    inventory.set_hash(h.to_vec());
                    inventory
                })
                .collect();
            getdata.set_inventory(RepeatedField::from_vec(inventory));

            tasks.push(Task::GetData(peer, getdata));
        }
        tasks
    }
}

impl<C: ChainClient + 'static> NetworkProtocolHandler for SyncProtocol<C> {
    fn process(&self, nc: &NetworkContext, peer: PeerId, payload: nervos_protocol::Payload) {
        if payload.has_getheaders() {
            self.handle_getheaders(nc, peer, payload.get_getheaders());
        } else if payload.has_headers() {
            self.handle_headers(nc, peer, payload.get_headers());
        } else if payload.has_getdata() {
            self.handle_getdata(nc, peer, payload.get_getdata());
        } else if payload.has_block() {
            self.handle_block(nc, peer, payload.get_block());
        }
    }

    fn connected(&self, nc: &NetworkContext, peer: PeerId) {
        let locator_hash = self.chain.get_locator();
        let mut payload = nervos_protocol::Payload::new();
        let mut getheaders = nervos_protocol::GetHeaders::new();
        let locator_hash = locator_hash.into_iter().map(|hash| hash.to_vec()).collect();
        getheaders.set_version(0);
        getheaders.set_block_locator_hashes(RepeatedField::from_vec(locator_hash));
        getheaders.set_hash_stop(H256::default().to_vec());
        payload.set_getheaders(getheaders);
        let _ = nc.send(peer, payload);
    }

    fn disconnected(&self, _nc: &NetworkContext, _peer: PeerId) {
        // TODO
    }
}

pub const RELAY_PROTOCOL_ID: ProtocolId = *b"rel";

pub struct RelayProtocol<C> {
    pub chain: Arc<Chain<C>>,
    pub tx_pool: Arc<TransactionPool<C>>,
    // TODO add size limit or use bloom filter
    pub received_blocks: Mutex<FnvHashSet<H256>>,
    pub received_transactions: Mutex<FnvHashSet<H256>>,
    pub pending_compact_blocks: Mutex<FnvHashMap<H256, CompactBlock>>,
}

impl<C: ChainClient + 'static> RelayProtocol<C> {
    pub fn new(chain: &Arc<Chain<C>>, tx_pool: &Arc<TransactionPool<C>>) -> Self {
        RelayProtocol {
            chain: Arc::clone(chain),
            tx_pool: Arc::clone(tx_pool),
            received_blocks: Mutex::new(FnvHashSet::default()),
            received_transactions: Mutex::new(FnvHashSet::default()),
            pending_compact_blocks: Mutex::new(FnvHashMap::default()),
        }
    }

    #[allow(unused_variables)]
    pub fn relay(&self, nc: &NetworkContext, source: PeerId, payload: &nervos_protocol::Payload) {
        unimplemented!()
        // for peer in nc.peers() {
        //     if peer != source {
        //         let _ = nc.send(peer, payload.clone());
        //     }
        // }
    }

    fn reconstruct_block(
        &self,
        compact_block: &CompactBlock,
        transactions: Vec<Transaction>,
    ) -> (Option<Block>, Option<Vec<usize>>) {
        let (key0, key1) = short_transaction_id_keys(compact_block.nonce, &compact_block.header);

        let mut txs = transactions;
        txs.extend(self.tx_pool.pool.read().pool.get_vertices());
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
            let block = Block::new(compact_block.header.clone(), block_transactions);

            (Some(block), None)
        } else {
            (None, Some(missing_indexes))
        }
    }
}

impl<C: ChainClient + 'static> NetworkProtocolHandler for RelayProtocol<C> {
    fn process(&self, nc: &NetworkContext, peer: PeerId, payload: nervos_protocol::Payload) {
        if payload.has_transaction() {
            let tx: Transaction = payload.get_transaction().into();
            if !self.received_transactions.lock().insert(tx.hash()) {
                let _ = self.tx_pool.add_to_memory_pool(tx);
                self.relay(nc, peer, &payload);
            }
        } else if payload.has_block() {
            let block: Block = payload.get_block().into();
            if !self.received_blocks.lock().insert(block.hash()) {
                self.chain.insert_block(&block);
                self.relay(nc, peer, &payload);
            }
        } else if payload.has_compact_block() {
            let compact_block: CompactBlock = payload.get_compact_block().into();
            if !self
                .received_blocks
                .lock()
                .insert(compact_block.header.hash())
            {
                match self.reconstruct_block(&compact_block, Vec::new()) {
                    (Some(block), _) => {
                        self.chain.insert_block(&block);
                        self.relay(nc, peer, &payload);
                    }
                    (_, Some(missing_indexes)) => {
                        let mut payload = nervos_protocol::Payload::new();
                        let mut cbr = nervos_protocol::BlockTransactionsRequest::new();
                        cbr.set_hash(compact_block.header.hash().to_vec());
                        cbr.set_indexes(missing_indexes.into_iter().map(|i| i as u32).collect());
                        payload.set_block_transactions_request(cbr);
                        self.pending_compact_blocks
                            .lock()
                            .insert(compact_block.header.hash(), compact_block);
                        let _ = nc.respond(payload);
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
            if let Some(block) = self.chain.provider().block(&hash) {
                let mut payload = nervos_protocol::Payload::new();
                let mut bt = nervos_protocol::BlockTransactions::new();
                bt.set_hash(hash.to_vec());
                bt.set_transactions(RepeatedField::from_vec(
                    indexes
                        .iter()
                        .filter_map(|i| block.transactions.get(*i as usize))
                        .map(Into::into)
                        .collect(),
                ));
                let _ = nc.respond(payload);
            }
        } else if payload.has_block_transactions() {
            let bt = payload.get_block_transactions();
            let hash = H256::from_slice(bt.get_hash());
            if let Some(compact_block) = self.pending_compact_blocks.lock().remove(&hash) {
                let transactions: Vec<Transaction> =
                    bt.get_transactions().iter().map(Into::into).collect();
                if let (Some(block), _) = self.reconstruct_block(&compact_block, transactions) {
                    self.chain.insert_block(&block);
                }
            }
        }
    }

    fn connected(&self, _nc: &NetworkContext, _peer: PeerId) {
        // do nothing
    }

    fn disconnected(&self, _nc: &NetworkContext, _peer: PeerId) {
        // TODO
    }
}
