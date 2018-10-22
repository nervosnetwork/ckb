use super::Config;
use chain::chain::ChainController;
use channel::Receiver;
use ckb_notify::{MsgNewTip, MsgNewTransaction, NotifyController, MINER_SUBSCRIBER};
use ckb_pow::PowEngine;
use ckb_protocol::RelayMessage;
use core::block::{Block, BlockBuilder};
use core::header::{RawHeader, Seal};
use core::BlockNumber;
use flatbuffers::FlatBufferBuilder;
use network::NetworkService;
use rand::{thread_rng, Rng};
use rpc::{BlockTemplate, RpcController};
use shared::index::ChainIndex;
use shared::shared::Shared;
use std::collections::HashSet;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use sync::RELAY_PROTOCOL_ID;

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
            debug!(target: "miner", "mining {}", nonce);
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
            for peer in self.network.connected_peers_indexes() {
                debug!(target: "miner", "announce new block to peer#{}, {} => {}",
                       peer, block.header().number(), block.header().hash());
                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_compact_block(fbb, &block, &HashSet::new());
                fbb.finish(message, None);
                let _ = nc.send(peer, fbb.finished_data().to_vec());
            }
        });
    }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use bigint::H256;
    use ckb_notify::NotifyService;
    use ckb_pow::{DummyPowEngine, PowEngine};
    use core::block::BlockBuilder;
    use db::memorydb::MemoryKeyValueDB;
    use pool::txs_pool::{PoolConfig, TransactionPoolController, TransactionPoolService};
    use rpc::RpcService;
    use shared::shared::SharedBuilder;
    use shared::store::ChainKVStore;
    use verification::{BlockVerifier, HeaderResolverWrapper, HeaderVerifier, Verifier};

    #[test]
    fn test_block_template() {
        let (_handle, notify) = NotifyService::default().start::<&str>(None);
        let (tx_pool_controller, tx_pool_receivers) = TransactionPoolController::new();
        let (rpc_controller, rpc_receivers) = RpcController::new();

        let shared = SharedBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory().build();
        let tx_pool_service =
            TransactionPoolService::new(PoolConfig::default(), shared.clone(), notify.clone());
        let _handle = tx_pool_service.start::<&str>(None, tx_pool_receivers);

        let rpc_service = RpcService::new(shared.clone(), tx_pool_controller.clone());
        let _handle = rpc_service.start(Some("RpcService"), rpc_receivers, &notify);

        let block_template = rpc_controller
            .get_block_template(H256::from(0), 1000, 1000)
            .unwrap();

        let BlockTemplate {
            raw_header,
            uncles,
            commit_transactions,
            proposal_transactions,
        } = block_template;

        //do not verfiy pow here
        let header = raw_header.with_seal(Default::default());

        let block = BlockBuilder::default()
            .header(header)
            .uncles(uncles)
            .commit_transactions(commit_transactions)
            .proposal_transactions(proposal_transactions)
            .build();

        fn dummy_pow_engine() -> Arc<dyn PowEngine> {
            Arc::new(DummyPowEngine::new())
        }
        let pow_engine = dummy_pow_engine();
        let resolver = HeaderResolverWrapper::new(block.header(), shared.clone());
        let header_verifier = HeaderVerifier::new(Arc::clone(&pow_engine));

        assert!(header_verifier.verify(&resolver).is_ok());

        let block_verfier = BlockVerifier::new(
            shared.clone(),
            shared.consensus().clone(),
            Arc::clone(&pow_engine),
        );
        assert!(block_verfier.verify(&block).is_ok());
    }
}
