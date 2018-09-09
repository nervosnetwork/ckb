use super::build_block_template;
use super::Config;
use block_template::BlockTemplate;
use chain::chain::ChainProvider;
use chain::PowEngine;
use ckb_notify::{Event, Notify, MINER_SUBSCRIBER};
use ckb_protocol::Payload;
use core::block::IndexedBlock;
use core::header::{RawHeader, Seal};
use core::transaction::ProposalTransaction;
use core::BlockNumber;
use crossbeam_channel;
use fnv::FnvHashSet;
use network::NetworkContextExt;
use network::NetworkService;
use pool::TransactionPool;
use rand::{thread_rng, Rng};
use std::collections::HashSet;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use sync::compact_block::CompactBlockBuilder;
use sync::RELAY_PROTOCOL_ID;
use util::RwLock;

enum Message {
    Abort,
    Found(Seal),
}

pub struct Miner<C, P> {
    config: Config,
    chain: Arc<C>,
    pow: Arc<P>,
    network: Arc<NetworkService>,
    tx_pool: Arc<TransactionPool<C>>,
    signal_tx: mpsc::Sender<Message>,
    signal_rx: mpsc::Receiver<Message>,
    mining_number: Arc<RwLock<BlockNumber>>,
}

impl<C, P> Miner<C, P>
where
    C: ChainProvider + 'static,
    P: PowEngine + 'static,
{
    pub fn new(
        config: Config,
        chain: Arc<C>,
        pow: Arc<P>,
        tx_pool: &Arc<TransactionPool<C>>,
        network: &Arc<NetworkService>,
        notify: &Notify,
    ) -> Self {
        let (signal_tx, signal_rx) = mpsc::channel();
        let number = chain.tip_header().read().header.number;

        let miner = Miner {
            config,
            chain,
            pow,
            signal_tx,
            signal_rx,
            tx_pool: Arc::clone(tx_pool),
            network: Arc::clone(network),
            mining_number: Arc::new(RwLock::new(number)),
        };

        let (tx, rx) = crossbeam_channel::unbounded();
        notify.register_transaction_subscriber(MINER_SUBSCRIBER, tx.clone());
        notify.register_tip_subscriber(MINER_SUBSCRIBER, tx.clone());
        miner.subscribe_update(rx);
        miner
    }

    pub fn subscribe_update(&self, sub: crossbeam_channel::Receiver<Event>) {
        let signal_tx = self.signal_tx.clone();
        let mining_number = Arc::clone(&self.mining_number);
        let new_transactions_threshold = self.config.new_transactions_threshold;
        let mut new_transactions_counter = 0;
        thread::spawn(move || loop {
            match sub.recv() {
                Some(Event::NewTip(block)) => {
                    if block.header.number >= *mining_number.read() {
                        let _ = signal_tx.send(Message::Abort);
                    }
                }
                Some(Event::NewTransaction) => {
                    if new_transactions_counter >= new_transactions_threshold {
                        let _ = signal_tx.send(Message::Abort);
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

    pub fn start(&mut self) {
        self.pow.init(*self.mining_number.read());

        loop {
            self.commit_new_block();
        }
    }

    fn commit_new_block(&mut self) {
        match build_block_template(&self.chain, &self.tx_pool) {
            Ok(block_template) => {
                let mut mining_number = self.mining_number.write();
                *mining_number = block_template.raw_header.number;
                if let Some((block, propasal)) = self.mine(block_template) {
                    debug!(target: "miner", "new block mined: {} -> (number: {}, difficulty: {}, timestamp: {})",
                          block.hash(), block.header.number, block.header.difficulty, block.header.timestamp);
                    if self.chain.process_block(&block, true).is_ok() {
                        self.tx_pool.proposal_n(block.number(), propasal);
                        self.announce_new_block(&block);
                    }
                }
            }
            Err(err) => {
                error!(target: "miner", "build_block_template: {:?}", err);
            }
        }
    }

    fn mine(
        &self,
        block_template: BlockTemplate,
    ) -> Option<(IndexedBlock, FnvHashSet<ProposalTransaction>)> {
        let BlockTemplate {
            raw_header,
            uncles,
            commit_transactions,
            proposal_transactions,
        } = block_template;

        match self.mine_loop(&raw_header) {
            Message::Found(seal) => Some((
                IndexedBlock {
                    header: raw_header.with_seal(seal).into(),
                    uncles,
                    commit_transactions,
                    proposal_transactions: proposal_transactions
                        .iter()
                        .map(|p| p.proposal_short_id())
                        .collect(),
                },
                proposal_transactions,
            )),
            Message::Abort => None,
        }
    }

    fn mine_loop(&self, header: &RawHeader) -> Message {
        let mut nonce: u64 = thread_rng().gen();
        loop {
            debug!(target: "miner", "mining {}", nonce);
            if let Ok(message) = self.signal_rx.try_recv() {
                break message;
            }
            if let Some(seal) = self.pow.solve_header(header, nonce) {
                break Message::Found(seal);
            }
            nonce = nonce.wrapping_add(1);
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
