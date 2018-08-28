extern crate bigint;
extern crate ckb_chain as chain;
extern crate ckb_core as core;
extern crate ckb_db as db;
extern crate ckb_network as network;
extern crate ckb_notify as notify;
extern crate ckb_protocol;
extern crate ckb_sync as sync;
extern crate ckb_time as time;
extern crate ckb_verification as verification;
extern crate env_logger;
#[cfg(test)]
extern crate futures;
extern crate tempdir;

use bigint::H256;
use chain::chain::{Chain, ChainBuilder, ChainProvider};
use chain::store::ChainKVStore;
use chain::Config;
use chain::COLUMNS;
use ckb_protocol::Payload;
use core::block::IndexedBlock;
use core::difficulty::cal_difficulty;
use core::header::{Header, RawHeader, Seal};
use core::transaction::{CellInput, CellOutput, OutPoint, Transaction};
use db::memorydb::MemoryKeyValueDB;
use network::*;
use notify::Notify;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use std::{thread, time as std_time};
use sync::protocol::SyncProtocol;
use sync::synchronizer::Synchronizer;
use sync::{Config as SyncConfig, SYNC_PROTOCOL_ID};
use tempdir::TempDir;
use time::now_ms;

#[derive(Default)]
struct TestNode {
    pub peers: Vec<PeerId>,
    pub senders: HashMap<(ProtocolId, PeerId), Sender<Payload>>,
    pub receivers: HashMap<(ProtocolId, PeerId), Receiver<Payload>>,
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
                Box::new(TestNetworkContext {
                    protocol,
                    current_peer: Some(local_index),
                    senders: self.senders.clone(),
                }),
                local_index,
            )
        }
    }

    pub fn start<F: Fn(u32) -> bool>(&self, signal: Sender<()>, pred: F) {
        let mut block_count = 0u32;
        loop {
            for ((protocol, peer), receiver) in &self.receivers {
                let payload = receiver.recv().unwrap();
                if payload.has_block() {
                    block_count += 1;
                }
                if let Some(handler) = self.protocols.get(protocol) {
                    handler.process(
                        Box::new(TestNetworkContext {
                            protocol: *protocol,
                            current_peer: Some(*peer),
                            senders: self.senders.clone(),
                        }),
                        *peer,
                        payload,
                    )
                }
                if pred(block_count) {
                    signal.send(()).unwrap();
                }
            }
        }
    }
}

struct TestNetworkContext {
    protocol: ProtocolId,
    current_peer: Option<PeerId>,
    senders: HashMap<(ProtocolId, PeerId), Sender<Payload>>,
}

impl NetworkContext for TestNetworkContext {
    fn send_protocol(
        &self,
        protocol: ProtocolId,
        peer: PeerId,
        payload: Payload,
    ) -> Result<(), Error> {
        if let Some(sender) = self.senders.get(&(protocol, peer)) {
            sender.send(payload).unwrap();
        }
        Ok(())
    }

    fn send(&self, peer: PeerId, payload: Payload) -> Result<(), Error> {
        self.send_protocol(self.protocol, peer, payload)
    }

    fn respond(&self, payload: Payload) -> Result<(), Error> {
        if let Some(peer) = self.current_peer {
            self.send(peer, payload)
        } else {
            panic!() // TODO log error or return?
        }
    }

    fn sessions(&self) -> Vec<(PeerId, SessionInfo)> {
        unimplemented!()
    }

    fn disable_peer(&self, _peer: PeerId) {
        unimplemented!()
    }

    fn disconnect_peer(&self, _peer: PeerId) {
        unimplemented!()
    }

    /// Check if the session is still active.
    fn is_expired(&self) -> bool {
        unimplemented!()
    }

    /// Register a new IO timer. 'IoHandler::timeout' will be called with the token.
    fn register_timer(&self, _token: TimerToken, _delay: Duration) -> Result<(), Error> {
        unimplemented!()
    }

    /// Returns peer identification string
    fn peer_client_version(&self, _peer: PeerId) -> String {
        unimplemented!()
    }

    /// Returns information on p2p session
    fn session_info(&self, _peer: PeerId) -> Option<SessionInfo> {
        None
    }

    /// Returns max version for a given protocol.
    fn protocol_version(&self, _protocol: ProtocolId, _peer: PeerId) -> Option<u8> {
        unimplemented!()
    }

    /// Returns this object's subprotocol name.
    fn subprotocol_name(&self) -> ProtocolId {
        unimplemented!()
    }
}

#[test]
fn basic_sync() {
    let _ = env_logger::init();

    let (mut node1, chain1) = setup_node(0);
    let (mut node2, chain2) = setup_node(1);

    node1.connect(&mut node2, SYNC_PROTOCOL_ID);

    let (signal_tx1, signal_rx1) = channel();
    thread::spawn(move || {
        node1.start(signal_tx1, |block_count| block_count >= 1);
    });

    let (signal_tx2, _) = channel();
    thread::spawn(move || {
        node2.start(signal_tx2, |_| false);
    });

    // Wait node1 receive block from node2
    let _ = signal_rx1.recv();

    assert_eq!(chain1.tip_header().number, 1);
    assert_eq!(chain1.tip_header().number, chain2.tip_header().number);
}

fn setup_node(height: u64) -> (TestNode, Arc<Chain<ChainKVStore<MemoryKeyValueDB>>>) {
    let notify = Notify::new();
    let builder =
        ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory().notify(notify.clone());
    let mut block = builder.get_config().genesis_block();
    let chain = Arc::new(builder.build().unwrap());

    for _i in 0..height {
        let time = now_ms();
        let transactions = vec![Transaction::new(
            0,
            Vec::new(),
            vec![CellInput::new(OutPoint::null(), Default::default())],
            vec![CellOutput::new(0, 50, Vec::new(), H256::default())],
        )];

        let header = Header {
            raw: RawHeader::new(
                &block.header,
                transactions.iter(),
                time,
                cal_difficulty(&block.header, time),
            ),
            seal: Seal {
                nonce: 0,
                mix_hash: H256::from(0),
            },
        };

        block = IndexedBlock {
            header: header.into(),
            transactions,
        };
        chain.process_block(&block).unwrap();
    }

    let synchronizer = Synchronizer::new(&chain, notify.clone(), None, SyncConfig::default());
    let sync_protocol = Arc::new(SyncProtocol::new(synchronizer));
    let sync_protocol_clone = Arc::clone(&sync_protocol);
    let _ = thread::Builder::new().spawn(move || {
        sync_protocol_clone.start();
    });

    let mut node = TestNode::default();
    node.protocols.insert(SYNC_PROTOCOL_ID, sync_protocol);
    (node, chain)
}
