use super::build_block_template;
use super::sealer::{Sealer, Signal};
use super::Config;
use chain::chain::ChainProvider;
use ckb_notify::{Event, Notify, MINER_SUBSCRIBER};
use ckb_protocol::Payload;
use core::block::IndexedBlock;
use core::header::BlockNumber;
use crossbeam_channel;
use ethash::{get_epoch, Ethash};
use network::NetworkContextExt;
use network::NetworkService;
use pool::TransactionPool;
use std::collections::HashSet;
use std::sync::Arc;
use std::thread;
use sync::compact_block::CompactBlockBuilder;
use sync::RELAY_PROTOCOL_ID;
use util::RwLock;

pub struct Miner<C> {
    config: Config,
    chain: Arc<C>,
    network: Arc<NetworkService>,
    tx_pool: Arc<TransactionPool<C>>,
    sealer: Sealer,
    signal: Signal,
    mining_number: Arc<RwLock<BlockNumber>>,
}

impl<C: ChainProvider + 'static> Miner<C> {
    pub fn new(
        config: Config,
        chain: Arc<C>,
        tx_pool: &Arc<TransactionPool<C>>,
        network: &Arc<NetworkService>,
        ethash: Option<Arc<Ethash>>,
        notify: &Notify,
    ) -> Self {
        if let Some(ref ethash) = ethash {
            let number = { chain.tip_header().read().header.number };
            let _ = ethash.gen_dataset(get_epoch(number));
        }
        let (sealer, signal) = Sealer::new(ethash);

        let miner = Miner {
            config,
            chain,
            sealer,
            signal,
            tx_pool: Arc::clone(tx_pool),
            network: Arc::clone(network),
            mining_number: Default::default(),
        };

        let (tx, rx) = crossbeam_channel::unbounded();
        notify.register_transaction_subscriber(MINER_SUBSCRIBER, tx.clone());
        notify.register_tip_subscriber(MINER_SUBSCRIBER, tx.clone());
        miner.subscribe_update(rx);
        miner
    }

    pub fn subscribe_update(&self, sub: crossbeam_channel::Receiver<Event>) {
        let signal = self.signal.clone();
        let mining_number = Arc::clone(&self.mining_number);
        let new_transactions_threshold = self.config.new_transactions_threshold;
        let mut new_transactions_counter = 0;
        thread::spawn(move || loop {
            match sub.recv() {
                Some(Event::NewTip(block)) => {
                    if block.header.number >= *mining_number.read() {
                        signal.send_abort();
                    }
                }
                Some(Event::NewTransaction) => {
                    if new_transactions_counter >= new_transactions_threshold {
                        signal.send_abort();
                        new_transactions_counter = 0;
                    } else {
                        new_transactions_counter += 1;
                    }
                }
                None => {
                    info!(target: "miner", "sub channel closed");
                    break;
                }
                event => {
                    warn!(target: "miner", "Unexpected sub message {:?}", event);
                }
            }
        });
    }

    pub fn run_loop(&mut self) {
        loop {
            self.commit_new_block();
        }
    }

    fn commit_new_block(&mut self) {
        match build_block_template(&self.chain, &self.tx_pool) {
            Ok(block_template) => {
                let mut mining_number = self.mining_number.write();
                *mining_number = block_template.raw_header.number;
                if let Some(block) = self.sealer.seal(block_template, &self.signal) {
                    let block: IndexedBlock = block.into();
                    debug!(target: "miner", "new block mined: {} -> (number: {}, difficulty: {}, timestamp: {})",
                          block.hash(), block.header.number, block.header.difficulty, block.header.timestamp);
                    if self.chain.process_block(&block).is_ok() {
                        self.announce_new_block(&block);
                    }
                }
            }
            Err(err) => {
                error!(target: "miner", "build_block_template: {:?}", err);
            }
        }
    }

    fn announce_new_block(&self, block: &IndexedBlock) {
        self.network.with_context_eval(RELAY_PROTOCOL_ID, |nc| {
            for (peer_id, _session) in nc.sessions(&self.network.connected_peers()) {
                debug!(target: "miner", "announce new block to peer#{:?}, {} => {}",
                       peer_id, block.header().number, block.hash());
                let mut payload = Payload::new();
                let compact_block = CompactBlockBuilder::new(block, &HashSet::new()).build();
                payload.set_compact_block(compact_block.into());
                let _ = nc.send_payload(peer_id, payload);
            }
        });
    }
}
