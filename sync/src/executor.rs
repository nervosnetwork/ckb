use actix::prelude::*;
use bigint::H256;
use core::block::{Block, Header};
use nervos_protocol;
use network::Network;
use network::protocol::Peer;
use protobuf::RepeatedField;
use std::sync::Arc;
use std::sync::mpsc::channel;
use std::thread;

pub type ExecutorAddr = Addr<Syn, Executor>;

#[derive(Debug, PartialEq)]
pub enum Task {
    GetHeaders(Peer, Vec<H256>),
    GetData(Peer, nervos_protocol::GetData),
    Headers(Peer, Vec<Header>),
    Block(Peer, Box<Block>), //boxing the large fields to reduce the total size of the enum
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

    fn execute_headers(&self, peer: Peer, headers: &[Header]) {
        info!(target: "sync", "sync executor execute_headers to {:?}", peer);
        let mut payload = nervos_protocol::Payload::new();
        let mut headers_proto = nervos_protocol::Headers::new();
        let headers = headers.iter().map(Into::into).collect();
        headers_proto.set_headers(RepeatedField::from_vec(headers));
        payload.set_headers(headers_proto);
        self.network.unicast(peer, payload);
    }

    fn execute_getdata(&self, peer: Peer, getdata: nervos_protocol::GetData) {
        info!(target: "sync", "sync executor execute_getdata to {:?}", peer);
        let mut payload = nervos_protocol::Payload::new();
        payload.set_getdata(getdata);
        self.network.unicast(peer, payload);
    }

    fn execute_getheaders(&self, peer: Peer, locator_hash: &[H256]) {
        info!(target: "sync", "sync executor execute_getheaders to {:?}", peer);
        let mut payload = nervos_protocol::Payload::new();
        let mut getheaders = nervos_protocol::GetHeaders::new();
        let locator_hash = locator_hash.iter().map(|hash| hash.to_vec()).collect();
        getheaders.set_version(0);
        getheaders.set_block_locator_hashes(RepeatedField::from_vec(locator_hash));
        getheaders.set_hash_stop(H256::default().to_vec());
        payload.set_getheaders(getheaders);
        self.network.unicast(peer, payload);
    }

    fn execute_block(&self, peer: Peer, block: &Block) {
        info!(target: "sync", "sync executor execute_block to {:?}", peer);
        let mut payload = nervos_protocol::Payload::new();
        payload.set_block(block.into());
        self.network.unicast(peer, payload);
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
