extern crate bigint;
extern crate env_logger;
extern crate ethash;
extern crate futures;
extern crate nervos_chain as chain;
extern crate nervos_core as core;
extern crate nervos_db as db;
extern crate nervos_network as network;
extern crate nervos_notify as notify;
extern crate nervos_protocol;
extern crate nervos_sync as sync;
extern crate nervos_time as time;
extern crate nervos_verification as verification;
extern crate tempdir;

use bigint::H256;
use chain::chain::{Chain, ChainClient};
use chain::store::ChainKVStore;
use chain::Config;
use chain::COLUMNS;
use core::block::Block;
use core::difficulty::cal_difficulty;
use core::header::{Header, RawHeader, Seal};
use db::memorydb::MemoryKeyValueDB;
use ethash::Ethash;
use nervos_protocol::Payload;
use network::protocol::{NetworkContext, NetworkProtocolHandler};
use network::protocol::{PeerIndex, ProtocolId};
use notify::Notify;
use std::collections::HashMap;
use std::io::Error;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::{thread, time as std_time};
use sync::chain::Chain as SyncChain;
use sync::protocol::{SyncProtocol, SYNC_PROTOCOL_ID};
use tempdir::TempDir;
use time::now_ms;

#[derive(Default)]
struct TestNode {
    pub peers: Vec<PeerIndex>,
    pub senders: HashMap<(ProtocolId, PeerIndex), Sender<Payload>>,
    pub receivers: HashMap<(ProtocolId, PeerIndex), Receiver<Payload>>,
    pub protocols: HashMap<ProtocolId, Arc<NetworkProtocolHandler + Send + Sync>>,
}

impl TestNode {
    pub fn connect(&mut self, remote: &mut TestNode, protocol: ProtocolId) {
        let (local_sender, local_receiver) = channel();
        let local_index = self.peers.len();
        self.peers.insert(local_index, local_index);
        self.senders.insert((protocol, local_index), local_sender);

        let (remote_sender, remote_receiver) = channel();
        let remote_index = remote.peers.len();
        remote.peers.insert(remote_index, remote_index);
        remote
            .senders
            .insert((protocol, remote_index), remote_sender);

        self.receivers
            .insert((protocol, remote_index), remote_receiver);
        remote
            .receivers
            .insert((protocol, local_index), local_receiver);

        if let Some(handler) = self.protocols.get(&protocol) {
            handler.connected(
                &TestNetworkContext {
                    protocol,
                    current_peer: Some(local_index),
                    senders: self.senders.clone(),
                },
                local_index,
            )
        }
    }

    pub fn start(&self) {
        loop {
            for ((protocol, peer), receiver) in &self.receivers {
                let payload = receiver.recv().unwrap();
                if let Some(handler) = self.protocols.get(protocol) {
                    handler.process(
                        &TestNetworkContext {
                            protocol: *protocol,
                            current_peer: Some(*peer),
                            senders: self.senders.clone(),
                        },
                        *peer,
                        payload,
                    )
                }
            }
        }
    }
}

struct TestNetworkContext {
    protocol: ProtocolId,
    current_peer: Option<PeerIndex>,
    senders: HashMap<(ProtocolId, PeerIndex), Sender<Payload>>,
}

impl NetworkContext for TestNetworkContext {
    fn send_all(&self, payload: Payload) {
        for peer in self.peers() {
            let _ = self.send_protocol(self.protocol, peer, payload.clone());
        }
    }

    fn send_protocol(
        &self,
        protocol: ProtocolId,
        peer: PeerIndex,
        payload: Payload,
    ) -> Result<(), Error> {
        if let Some(sender) = self.senders.get(&(protocol, peer)) {
            sender.send(payload).unwrap();
        }
        Ok(())
    }

    fn send(&self, peer: PeerIndex, payload: Payload) -> Result<(), Error> {
        self.send_protocol(self.protocol, peer, payload)
    }

    fn response(&self, payload: Payload) -> Result<(), Error> {
        if let Some(peer) = self.current_peer {
            self.send(peer, payload)
        } else {
            panic!() // TODO log error or return?
        }
    }

    fn disable_peer(&self, _peer: PeerIndex) {
        unimplemented!()
    }

    fn disconnect_peer(&self, _peer: PeerIndex) {
        unimplemented!()
    }

    fn peers(&self) -> Vec<PeerIndex> {
        unimplemented!()
    }
}

#[test]
fn basic_sync() {
    let _ = env_logger::init();

    let (mut node1, chain1) = setup_node(0);
    let (mut node2, chain2) = setup_node(1);

    node1.connect(&mut node2, SYNC_PROTOCOL_ID);

    thread::spawn(move || {
        node1.start();
    });

    thread::spawn(move || {
        node2.start();
    });

    // TODO use join
    thread::sleep(std_time::Duration::from_secs(5));

    assert_eq!(chain1.tip_header().number, chain2.tip_header().number);
}

fn setup_node(height: u64) -> (TestNode, Arc<Chain<ChainKVStore<MemoryKeyValueDB>>>) {
    let db = MemoryKeyValueDB::open(COLUMNS as usize);
    let store = ChainKVStore { db };
    let mut spec = Config::default();
    spec.verifier_type = "Noop".to_string();

    let ethash = Arc::new(Ethash::new(TempDir::new("").unwrap().path()));
    let notify = Notify::new();
    let chain = Arc::new(Chain::init(store, spec.clone(), &ethash, notify.clone()).unwrap());
    let block = spec.genesis_block();
    for i in 0..height {
        let time = now_ms();
        let header = Header {
            raw: RawHeader {
                version: 0,
                parent_hash: block.header.hash(),
                timestamp: time,
                txs_commit: H256::from(0),
                difficulty: cal_difficulty(&block.header, time),
                number: i + 1,
            },
            seal: Seal {
                nonce: 0,
                mix_hash: H256::from(0),
            },
            hash: None,
        };

        let block = Block {
            header,
            transactions: vec![],
        };
        chain.process_block(&block).unwrap();
    }

    let sync_chain = Arc::new(SyncChain::new(&chain, notify.clone()));
    let sync_protocol = Arc::new(SyncProtocol::new(&sync_chain));

    let mut node = TestNode::default();
    node.protocols.insert(SYNC_PROTOCOL_ID, sync_protocol);
    (node, chain)
}
