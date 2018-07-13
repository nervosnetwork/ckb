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
use chain::chain::{Chain, ChainBuilder, ChainClient};
use chain::store::ChainKVStore;
use chain::Config;
use chain::COLUMNS;
use core::block::Block;
use core::difficulty::cal_difficulty;
use core::header::{Header, RawHeader, Seal};
use core::transaction::{CellInput, CellOutput, OutPoint, Transaction};
use db::memorydb::MemoryKeyValueDB;
use ethash::Ethash;
use nervos_protocol::Payload;
use network::*;
use notify::Notify;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;
use std::{thread, time as std_time};
use sync::chain::Chain as SyncChain;
use sync::protocol::{SyncProtocol, SYNC_PROTOCOL_ID};
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
                &TestNetworkContext {
                    protocol,
                    current_peer: Some(local_index),
                    senders: self.senders.clone(),
                },
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
                        &TestNetworkContext {
                            protocol: *protocol,
                            current_peer: Some(*peer),
                            senders: self.senders.clone(),
                        },
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
    fn register_timer(&self, token: TimerToken, delay: Duration) -> Result<(), Error> {
        unimplemented!()
    }

    /// Returns peer identification string
    fn peer_client_version(&self, peer: PeerId) -> String {
        unimplemented!()
    }

    /// Returns information on p2p session
    fn session_info(&self, peer: PeerId) -> Option<SessionInfo> {
        None
    }

    /// Returns max version for a given protocol.
    fn protocol_version(&self, protocol: ProtocolId, peer: PeerId) -> Option<u8> {
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
    let builder = ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
        .verification_level("NoVerification")
        .notify(notify.clone());
    let mut block = builder.get_config().genesis_block();
    let chain = Arc::new(builder.build().unwrap());

    for i in 0..height {
        let time = now_ms();
        let transactions = vec![Transaction::new(
            0,
            Vec::new(),
            vec![CellInput::new(OutPoint::null(), Vec::new())],
            vec![CellOutput::new(0, 50, Vec::new(), Vec::new())],
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
            hash: None,
        };

        block = Block {
            header,
            transactions,
        };
        chain.process_block(&block).unwrap();
    }

    let sync_chain = Arc::new(SyncChain::new(&chain, notify.clone()));
    let sync_protocol = Arc::new(SyncProtocol::new(&sync_chain));

    let mut node = TestNode::default();
    node.protocols.insert(SYNC_PROTOCOL_ID, sync_protocol);
    (node, chain)
}
