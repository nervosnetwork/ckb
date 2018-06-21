use super::sealer::{Sealer, Signal};
use chain::chain::ChainClient;
use core::block::Block;
use core::global::MAX_TX;
use core::header::Header;
use core::transaction::Transaction;
use crossbeam_channel;
use ethash::{get_epoch, Ethash};
use nervos_notify::{Event, Notify};
use nervos_protocol::Payload;
use network::Network;
use network::protocol::NetworkContext;
use pool::TransactionPool;
use std::collections::HashSet;
use std::sync::Arc;
use std::thread;
use sync::compact_block::build_compact_block;
use sync::protocol::SYNC_PROTOCOL_ID;
use time::now_ms;

pub struct Miner<C> {
    pub chain: Arc<C>,
    pub network: Arc<Network>,
    pub tx_pool: Arc<TransactionPool<C>>,
    pub sealer: Sealer,
    pub signal: Signal,
}

pub struct Work {
    pub time: u64,
    pub head: Header,
    pub transactions: Vec<Transaction>,
    pub signal: Signal,
}

impl<C: ChainClient> Miner<C> {
    pub fn new(
        chain: Arc<C>,
        tx_pool: &Arc<TransactionPool<C>>,
        network: &Arc<Network>,
        ethash: &Arc<Ethash>,
        notify: &Notify,
    ) -> Self {
        let number = { chain.tip_header().number };
        let _dataset = ethash.gen_dataset(get_epoch(number));
        let (sealer, signal) = Sealer::new(ethash);

        let miner = Miner {
            chain,
            sealer,
            signal,
            tx_pool: Arc::clone(tx_pool),
            network: Arc::clone(network),
        };

        let (tx, rx) = crossbeam_channel::unbounded();
        notify.register_transaction_subscriber("miner", tx.clone());
        notify.register_sync_subscribers("miner", tx);
        miner.subscribe_update(rx);
        miner
    }

    pub fn subscribe_update(&self, sub: crossbeam_channel::Receiver<Event>) {
        let signal = self.signal.clone();
        thread::spawn(move || {
            // some work here
            while let Ok(_event) = sub.recv() {
                signal.send_abort();
            }
        });
    }

    pub fn run_loop(&self) {
        loop {
            self.commit_new_work();
        }
    }

    fn commit_new_work(&self) {
        let time = now_ms();
        let head = { self.chain.tip_header().clone() };
        let transactions = self.tx_pool.prepare_mineable_transactions(MAX_TX);
        let signal = self.signal.clone();
        let work = Work {
            time,
            head,
            transactions,
            signal,
        };
        if let Some(block) = self.sealer.seal(work) {
            info!(target: "miner", "new block mined: {} -> ({}, {})", block.hash(), block.header.number, block.header.difficulty);
            if self.chain.process_block(&block).is_ok() {
                self.announce_new_block(&block);
            }
        }
    }

    fn announce_new_block(&self, block: &Block) {
        let nc = self.network.build_network_context(SYNC_PROTOCOL_ID);
        let mut payload = Payload::new();
        let compact_block = build_compact_block(block, &HashSet::new());
        payload.set_compact_block(compact_block.into());
        nc.send_all(payload)
    }
}
