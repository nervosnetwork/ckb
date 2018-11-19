use super::Config;
use channel::Receiver;
use ckb_chain::chain::ChainController;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::header::{RawHeader, Seal};
use ckb_core::BlockNumber;
use ckb_network::NetworkService;
use ckb_notify::{MsgNewTip, MsgNewTransaction, NotifyController, MINER_SUBSCRIBER};
use ckb_pow::PowEngine;
use ckb_protocol::RelayMessage;
use ckb_rpc::{BlockTemplate, RpcController};
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::Shared;
use ckb_sync::RELAY_PROTOCOL_ID;
use flatbuffers::FlatBufferBuilder;
use rand::{thread_rng, Rng};
use std::collections::HashSet;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

pub struct MinerService {
    config: Config,
    pow: Arc<dyn PowEngine>,
    chain: ChainController,
    rpc: RpcController,
    network: Arc<NetworkService>,
    new_tx_receiver: Receiver<MsgNewTransaction>,
    new_tip_receiver: Receiver<MsgNewTip>,
    mining_number: BlockNumber,
}

impl MinerService {
    pub fn new<CI: ChainIndex>(
        config: Config,
        pow: Arc<dyn PowEngine>,
        shared: &Shared<CI>,
        chain: ChainController,
        rpc: RpcController,
        network: Arc<NetworkService>,
        notify: &NotifyController,
    ) -> Self {
        let new_tx_receiver = notify.subscribe_new_transaction(MINER_SUBSCRIBER);
        let new_tip_receiver = notify.subscribe_new_tip(MINER_SUBSCRIBER);

        let mining_number = shared.tip_header().read().number();

        MinerService {
            config,
            pow,
            chain,
            rpc,
            new_tx_receiver,
            new_tip_receiver,
            network,
            mining_number,
        }
    }

    pub fn start<S: ToString>(mut self, thread_name: Option<S>) -> JoinHandle<()> {
        let mut thread_builder = thread::Builder::new();
        // Mainly for test: give a empty thread_name
        if let Some(name) = thread_name {
            thread_builder = thread_builder.name(name.to_string());
        }

        self.pow.init(self.mining_number);

        thread_builder
            .spawn(move || loop {
                self.commit_new_block();
            }).expect("Start MinerService failed!")
    }

    fn commit_new_block(&mut self) {
        match self.rpc.get_block_template(
            self.config.type_hash,
            self.config.max_tx,
            self.config.max_prop,
        ) {
            Ok(block_template) => {
                self.mining_number = block_template.raw_header.number();
                if let Some(block) = self.mine(block_template) {
                    let block = Arc::new(block);
                    debug!(target: "miner", "new block mined: {} -> (number: {}, difficulty: {}, timestamp: {})",
                          block.header().hash(), block.header().number(), block.header().difficulty(), block.header().timestamp());
                    if self.chain.process_block(Arc::clone(&block)).is_ok() {
                        self.announce_new_block(&block);
                    }
                }
            }
            Err(err) => {
                error!(target: "miner", "build_block_template: {:?}", err);
            }
        }
    }

    fn mine(&self, block_template: BlockTemplate) -> Option<Block> {
        let BlockTemplate {
            raw_header,
            uncles,
            commit_transactions,
            proposal_transactions,
        } = block_template;

        self.mine_loop(&raw_header).map(|seal| {
            BlockBuilder::default()
                .header(raw_header.with_seal(seal))
                .uncles(uncles)
                .commit_transactions(commit_transactions)
                .proposal_transactions(proposal_transactions)
                .build()
        })
    }

    fn mine_loop(&self, header: &RawHeader) -> Option<Seal> {
        let new_transactions_threshold = self.config.new_transactions_threshold;
        let mut new_transactions_counter = 0;
        let mut nonce: u64 = thread_rng().gen();
        loop {
            loop {
                select! {
                    recv(self.new_tx_receiver, msg) => match msg {
                        Some(()) => {
                            if new_transactions_counter >= new_transactions_threshold {
                                return None;
                            } else {
                                new_transactions_counter += 1;
                            }
                        }
                        None => {
                            error!(target: "miner", "channel new_tx_receiver closed");
                            return None;
                        }
                    }
                    recv(self.new_tip_receiver, msg) => match msg {
                        Some(block) => {
                            if block.header().number() >= self.mining_number {
                                return None;
                            }
                        }
                        None => {
                            error!(target: "miner", "channel new_tip_receiver closed");
                            return None;
                        }
                    }
                    default => break,
                }
            }
            if let Some(seal) = self.pow.solve_header(header, nonce) {
                debug!(target: "miner", "found seal: {:?}", seal);
                break Some(seal);
            }
            nonce = nonce.wrapping_add(1);
        }
    }

    fn announce_new_block(&self, block: &Arc<Block>) {
        self.network.with_protocol_context(RELAY_PROTOCOL_ID, |nc| {
            let fbb = &mut FlatBufferBuilder::new();
            let message = RelayMessage::build_compact_block(fbb, &block, &HashSet::new());
            fbb.finish(message, None);
            for peer in nc.connected_peers() {
                debug!(target: "miner", "announce new block to peer#{}, {} => {}",
                       peer, block.header().number(), block.header().hash());
                let _ = nc.send(peer, fbb.finished_data().to_vec());
            }
        });
    }
}
