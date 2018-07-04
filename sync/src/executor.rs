use bigint::H256;
use core::block::Block;
use core::header::Header;
use nervos_protocol;
use network::{NetworkContext, PeerId};
use protobuf::RepeatedField;

// TODO refactor these code and protocol.rs
#[allow(dead_code)]
#[derive(Debug, PartialEq)]
pub enum Task {
    GetHeaders(PeerId, Vec<H256>),
    GetData(PeerId, nervos_protocol::GetData),
    Headers(PeerId, Vec<Header>),
    Block(PeerId, Box<Block>), //boxing the large fields to reduce the total size of the enum
}

pub struct Executor<'a> {
    pub nc: &'a NetworkContext,
}

impl<'a> Executor<'a> {
    pub fn execute(&self, task: Task) {
        match task {
            Task::GetHeaders(peer, locator_hash) => {
                self.execute_getheaders(peer, &locator_hash[..])
            }
            Task::Headers(peer, headers) => self.execute_headers(peer, &headers[..]),
            Task::GetData(peer, message) => self.execute_getdata(peer, message),
            Task::Block(peer, block) => self.execute_block(peer, &block),
        }
    }

    fn execute_headers(&self, peer: PeerId, headers: &[Header]) {
        info!(target: "sync", "sync executor execute_headers to {:?}", peer);
        let mut payload = nervos_protocol::Payload::new();
        let mut headers_proto = nervos_protocol::Headers::new();
        let headers = headers.iter().map(Into::into).collect();
        headers_proto.set_headers(RepeatedField::from_vec(headers));
        payload.set_headers(headers_proto);
        let _ = self.nc.send(peer, payload);
    }

    fn execute_getdata(&self, peer: PeerId, getdata: nervos_protocol::GetData) {
        info!(target: "sync", "sync executor execute_getdata to {:?}", peer);
        let mut payload = nervos_protocol::Payload::new();
        payload.set_getdata(getdata);
        let _ = self.nc.send(peer, payload);
    }

    fn execute_getheaders(&self, peer: PeerId, locator_hash: &[H256]) {
        info!(target: "sync", "sync executor execute_getheaders to {:?}", peer);
        let mut payload = nervos_protocol::Payload::new();
        let mut getheaders = nervos_protocol::GetHeaders::new();
        let locator_hash = locator_hash.iter().map(|hash| hash.to_vec()).collect();
        getheaders.set_version(0);
        getheaders.set_block_locator_hashes(RepeatedField::from_vec(locator_hash));
        getheaders.set_hash_stop(H256::default().to_vec());
        payload.set_getheaders(getheaders);
        let _ = self.nc.send(peer, payload);
    }

    fn execute_block(&self, peer: PeerId, block: &Block) {
        info!(target: "sync", "sync executor execute_block to {:?}", peer);
        let mut payload = nervos_protocol::Payload::new();
        payload.set_block(block.into());
        let _ = self.nc.send(peer, payload);
    }
}
