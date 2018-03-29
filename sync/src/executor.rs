use super::message;
use actix::prelude::*;
use bigint::H256;
use core::block::{Block, Header};
use multiaddr::Multiaddr;
use nervos_protocol;
use network::Network;
use protobuf::{Message as ProtobufMessage, RepeatedField};
use std::sync::Arc;
use std::sync::mpsc::channel;
use std::thread;

pub type ExecutorAddr = Addr<Syn, Executor>;

#[derive(Debug, PartialEq)]
pub enum Task {
    GetHeaders(Multiaddr, Vec<H256>),
    GetData(Multiaddr, nervos_protocol::GetData),
    Headers(Multiaddr, Vec<Header>),
    Block(Multiaddr, Box<Block>), //boxing the large fields to reduce the total size of the enum
}

impl Message for Task {
    type Result = ();
}

pub struct Executor {
    pub network: Arc<Network>,
}

impl Actor for Executor {
    type Context = Context<Self>;
}

impl Executor {
    pub fn new(network: &Arc<Network>) -> ExecutorAddr {
        let (sender, receiver) = channel();
        let network_clone = Arc::clone(network);
        let _ = thread::Builder::new()
            .name("sync_executor".to_string())
            .spawn(move || {
                let sys = System::new("sync_executor_system");
                let executor = Executor {
                    network: network_clone,
                };
                let addr: Addr<Syn, _> = executor.start();
                sender.send(addr).unwrap();
                sys.run();
            });
        receiver.recv().unwrap()
    }

    fn execute_headers(&self, addr: Multiaddr, headers: &[Header]) {
        info!(target: "sync", "sync executor execute_headers to {:?}", addr);
        let message = message::new_headers_payload(headers);
        self.network.unicast(addr, message);
    }

    fn execute_getdata(&self, addr: Multiaddr, getdata: nervos_protocol::GetData) {
        info!(target: "sync", "sync executor execute_getdata to {:?}", addr);
        let mut payload = nervos_protocol::Payload::new();
        payload.set_getdata(getdata);
        let message = payload.write_to_bytes().unwrap();
        self.network.unicast(addr, message);
    }

    fn execute_getheaders(&self, addr: Multiaddr, locator_hash: &[H256]) {
        info!(target: "sync", "sync executor execute_getheaders to {:?}", addr);
        let mut payload = nervos_protocol::Payload::new();
        let mut getheaders = nervos_protocol::GetHeaders::new();
        let locator_hash = locator_hash.iter().map(|hash| hash.to_vec()).collect();
        getheaders.set_version(0);
        getheaders.set_block_locator_hashes(RepeatedField::from_vec(locator_hash));
        getheaders.set_hash_stop(H256::default().to_vec());
        payload.set_getheaders(getheaders);
        self.network
            .unicast(addr, payload.write_to_bytes().unwrap());
    }

    fn execute_block(&self, addr: Multiaddr, block: &Block) {
        info!(target: "sync", "sync executor execute_block to {:?}", addr);
        let mut payload = nervos_protocol::Payload::new();
        payload.set_block(block.into());
        self.network
            .unicast(addr, payload.write_to_bytes().unwrap());
    }
}

impl Handler<Task> for Executor {
    type Result = ();

    fn handle(&mut self, task: Task, _ctx: &mut Self::Context) -> Self::Result {
        match task {
            Task::GetHeaders(addr, locator_hash) => {
                self.execute_getheaders(addr, &locator_hash[..])
            }
            Task::Headers(addr, headers) => self.execute_headers(addr, &headers[..]),
            Task::GetData(addr, message) => self.execute_getdata(addr, message),
            Task::Block(addr, block) => self.execute_block(addr, &block),
        }
    }
}
