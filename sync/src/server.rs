use super::executor::{ExecutorAddr, Task};
use super::{BlockHeight, MAX_HEADERS_LEN};
use actix::prelude::*;
use bigint::H256;
use nervos_chain::chain::ChainClient;
use nervos_protocol;
use network::protocol::Peer;
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::thread;

pub type ServerAddr = Addr<Syn, Server>;

#[derive(Debug, PartialEq)]
pub enum Request {
    /// Serve 'getdata' request
    GetData(Peer, nervos_protocol::GetData),
    /// Serve 'getheaders' request
    GetHeaders(Peer, nervos_protocol::GetHeaders),
}

impl Message for Request {
    type Result = ();
}

pub struct Server {
    pub executor: Arc<ExecutorAddr>,
    pub chain: Arc<ChainClient>,
}

impl Actor for Server {
    type Context = Context<Self>;
}

impl Server {
    pub fn new(chain: &Arc<ChainClient>, executor: &Arc<ExecutorAddr>) -> ServerAddr {
        let executor_clone = Arc::clone(executor);
        let chain_clone = Arc::clone(chain);
        let (sender, receiver) = channel();
        let _ = thread::Builder::new()
            .name("sync_server".to_string())
            .spawn(move || {
                let sys = System::new("sync_server_system");
                let server = Server {
                    executor: executor_clone,
                    chain: chain_clone,
                };
                let addr: Addr<Syn, _> = server.start();
                sender.send(addr).unwrap();
                sys.run();
            });
        receiver.recv().unwrap()
    }

    fn handle_getdata(&self, peer: Peer, mut message: nervos_protocol::GetData) {
        info!(target: "sync", "sync server handle_getdata {:?}", message);
        let inventory_vec = message.take_inventory();
        for inventory in inventory_vec.iter() {
            self.process_inventory(peer, inventory);
        }
    }

    fn process_inventory(&self, peer: Peer, inventory: &nervos_protocol::Inventory) {
        let inv_type = inventory.get_inv_type();
        match inv_type {
            nervos_protocol::InventoryType::MSG_BLOCK => {
                if let Some(block) = self.chain.block(&H256::from(inventory.get_hash())) {
                    trace!(target: "sync", "'getdata' response to peer#{}", peer);
                    self.executor.do_send(Task::Block(peer, Box::new(block)));
                } else {
                    //Reponse notfound
                }
            }
            nervos_protocol::InventoryType::ERROR => {}
        }
    }

    fn handle_getheaders(&self, peer: Peer, mut message: nervos_protocol::GetHeaders) {
        info!(target: "sync", "sync server handle_getheaders");
        let hash_stop = H256::from_slice(message.get_hash_stop());
        let block_locator_hashes: Vec<H256> = message
            .take_block_locator_hashes()
            .iter()
            .map(|hash| H256::from_slice(&hash[..]))
            .collect();
        if let Some(block_height) =
            self.locate_best_common_block(&hash_stop, &block_locator_hashes[..])
        {
            let headers: Vec<_> = (block_height + 1..block_height + 1 + MAX_HEADERS_LEN as u64)
                .filter_map(|block_height| self.chain.block_hash(block_height))
                .take_while(|block_hash| block_hash != &hash_stop)
                .filter_map(|block_hash| self.chain.block_header(&block_hash))
                .collect();
            // response headers
            self.executor.do_send(Task::Headers(peer, headers));
        } else {
            // Got 'headers' message without known blocks
            // ban or close peers
        }
    }

    fn locate_best_common_block(&self, hash_stop: &H256, locator: &[H256]) -> Option<BlockHeight> {
        for block_hash in locator.iter().chain(&[*hash_stop]) {
            if let Some(block_height) = self.chain.block_height(block_hash) {
                return Some(block_height);
            }

            // block with this hash is definitely not in the main chain (block_height has returned None)
            // but maybe it is in some fork? if so => we should find intersection with main chain
            // and this would be our best common block
            let mut block_hash = *block_hash;
            loop {
                let block_header = match self.chain.block_header(&block_hash) {
                    None => break,
                    Some(block_header) => block_header,
                };

                if let Some(block_height) = self.chain.block_height(&block_header.parent_hash) {
                    return Some(block_height);
                }

                block_hash = block_header.parent_hash;
            }
        }

        None
    }
}

impl Handler<Request> for Server {
    type Result = ();

    fn handle(&mut self, req: Request, _ctx: &mut Self::Context) -> Self::Result {
        match req {
            Request::GetData(peer, message) => self.handle_getdata(peer, message),
            Request::GetHeaders(peer, message) => self.handle_getheaders(peer, message),
        }
    }
}
