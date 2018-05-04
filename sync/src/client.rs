use super::chain::{BlockState, Chain};
use super::executor::{ExecutorAddr, Task};
use super::peers::Peers;
use super::{MAX_HEADERS_LEN, MAX_SCHEDULED_LEN};
use actix::prelude::*;
use bigint::H256;
use core::block::Block;
use core::header::Header;
use multiaddr::Multiaddr;
use nervos_chain::chain::ChainClient;
use nervos_notify::Notify;
use nervos_protocol;
use pool::{OrphanBlockPool, TransactionPool};
use protobuf::RepeatedField;
use std::cmp::min;
use std::collections::VecDeque;
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::thread;
use util::RwLock;

pub type PeersRef = Arc<RwLock<Peers>>;

#[derive(Debug, PartialEq)]
pub enum Command {
    OnHeaders(Multiaddr, nervos_protocol::Headers),
    OnTransaction(Multiaddr, nervos_protocol::Transaction),
    OnBlock(Multiaddr, nervos_protocol::Block),
}

impl Message for Command {
    type Result = ();
}

pub struct Client<C> {
    pub chain: Chain<C>,
    pub executor: Arc<ExecutorAddr>,
    pub peers: PeersRef,
    pub tx_pool: Arc<TransactionPool<C>>,
    pub orphaned_blocks_pool: Arc<OrphanBlockPool>,
    pub notify: Notify,
}

impl<C: ChainClient + 'static> Actor for Client<C> {
    type Context = Context<Self>;
}

impl<C: ChainClient + 'static> Client<C> {
    pub fn new(
        chain: &Arc<C>,
        executor: &Arc<ExecutorAddr>,
        peers: &PeersRef,
        tx_pool: &Arc<TransactionPool<C>>,
        notify: Notify,
    ) -> Addr<Syn, Client<C>> {
        let executor_clone = Arc::clone(executor);
        let (sender, receiver) = channel();
        let client = Client {
            notify,
            executor: executor_clone,
            chain: Chain::new(chain),
            peers: Arc::clone(peers),
            tx_pool: Arc::clone(tx_pool),
            orphaned_blocks_pool: Arc::new(OrphanBlockPool::default()),
        };

        let _ = thread::Builder::new()
            .name("sync_client".to_string())
            .spawn(move || {
                let sys = System::new("sync_client_system");
                let addr: Addr<Syn, _> = client.start();
                sender.send(addr).expect("channel alive");
                sys.run();
            });
        receiver.recv().unwrap()
    }

    //TODO: broadcast Transaction
    fn on_transaction(&self, _addr: &Multiaddr, tx: &nervos_protocol::Transaction) {
        if let Err(e) = self.tx_pool.add_to_memory_pool(tx.into()) {
            debug!("Transaction rejected: {:?}", e);
        }
    }

    fn on_block(&self, addr: &Multiaddr, block: Block) {
        info!(target: "sync", "client on_block peer#{}", addr);
        let block_hash = block.hash();
        {
            self.peers.write().on_block_received(addr, &block_hash);
        }
        let block_state = self.chain.block_state(&block_hash);

        match block_state {
            BlockState::DeadEnd => {
                //ban peer
            }
            BlockState::Verifying | BlockState::Stored => {
                self.peers.write().as_useful_peer(addr);
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
                            self.peers.write().as_useful_peer(addr);
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
                            self.notify.notify_sync_head();
                        }
                        self.execute_tasks();
                    }
                    BlockState::Requested | BlockState::Scheduled => {
                        {
                            self.peers.write().as_useful_peer(addr);
                        }
                        // remember as orphan block
                        self.orphaned_blocks_pool.insert(block);
                    }
                }
            }
        }
    }

    fn on_headers(&self, addr: &Multiaddr, message: &nervos_protocol::Headers) {
        info!(target: "sync", "sync client on_headers");

        let mut headers: Vec<Header> = message.headers.iter().map(From::from).collect();
        if headers.is_empty() {
            return;
        }

        if headers.len() > MAX_HEADERS_LEN {
            //TODO: ban peer, possible DOS
            return;
        }
        {
            self.peers.write().on_headers_received(addr);
        }

        let header0 = headers[0].clone();
        //check first header parent
        if self.chain.block_state(&header0.parent_hash) == BlockState::Unknown {
            info!(
                target: "sync",
                "Previous header of the first header from peer#{} `headers` message is unknown. First: {}. Previous: {}", 
                addr, header0.hash(), &header0.parent_hash
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
                                "`headers` message out of order from peer#{}", addr
                            );
                            return;
                        }
                    }
                    // else all headers are known
                    _ => {
                        info!(target: "sync", "Ignoring {} known headers from peer#{}", headers.len(), addr);
                        // but this peer is still useful for synchronization
                        {
                            self.peers.write().as_useful_peer(addr);
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
            self.peers.write().as_useful_peer(addr);
        }
        self.execute_tasks();
    }

    // fn verify_headers(&self, _addr: &Multiaddr, _last_known_hash: H256, _headers: &[Header]) {}

    fn execute_tasks(&self) {
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
                for addr in &headers_idle_peers {
                    self.peers.write().on_headers_requested(addr);
                }

                let block_locator_hashes = self.chain.get_locator();
                let headers_tasks = headers_idle_peers
                    .into_iter()
                    .map(move |addr| Task::GetHeaders(addr, block_locator_hashes.clone()));
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
        for task in tasks {
            self.executor.do_send(task);
        }
    }

    fn prepare_blocks_requests_tasks(
        &self,
        peers: Vec<Multiaddr>,
        mut hashes: Vec<H256>,
    ) -> Vec<Task> {
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

        for addr in peers {
            let index = min(hashes.len(), chunk_size as usize);
            let mut chunk_hashes = hashes.split_off(index);
            swap(&mut chunk_hashes, &mut hashes);
            {
                self.peers.write().on_blocks_requested(&addr, &chunk_hashes);
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

            tasks.push(Task::GetData(addr, getdata));
        }
        tasks
    }
}

impl<C: ChainClient + 'static> Handler<Command> for Client<C> {
    type Result = ();

    fn handle(&mut self, cmd: Command, _ctx: &mut Self::Context) -> Self::Result {
        match cmd {
            Command::OnHeaders(addr, message) => self.on_headers(&addr, &message),
            Command::OnTransaction(addr, transaction) => self.on_transaction(&addr, &transaction),
            Command::OnBlock(addr, block) => self.on_block(&addr, (&block).into()),
        }
    }
}
