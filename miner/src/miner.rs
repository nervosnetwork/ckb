use super::sealer::{Sealer, Signal};
use super::Config;
use chain::chain::{ChainProvider, Error};
use core::block::IndexedBlock;
use core::header::{Header, IndexedHeader};
use core::script::Script;
use core::transaction::{CellInput, CellOutput, OutPoint, Transaction, VERSION};
use crossbeam_channel;
use ethash::{get_epoch, Ethash};
use nervos_notify::{Event, Notify};
use nervos_protocol::Payload;
use network::NetworkService;
use pool::TransactionPool;
use std::collections::HashSet;
use std::sync::Arc;
use std::thread;
use sync::compact_block::CompactBlockBuilder;
use sync::RELAY_PROTOCOL_ID;
use time::now_ms;

pub struct Miner<C> {
    pub config: Config,
    pub chain: Arc<C>,
    pub network: Arc<NetworkService>,
    pub tx_pool: Arc<TransactionPool<C>>,
    pub sealer: Sealer,
    pub signal: Signal,
}

pub struct Work {
    pub time: u64,
    pub tip: IndexedHeader,
    pub transactions: Vec<Transaction>,
    pub signal: Signal,
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
            while let Some(_event) = sub.recv() {
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
        let tip = { self.chain.tip_header().read().header.clone() };
        let mut transactions = self
            .tx_pool
            .prepare_mineable_transactions(self.config.max_tx);
        match self.create_cellbase_transaction(&tip.header, &transactions) {
            Ok(cellbase_transaction) => {
                transactions.insert(0, cellbase_transaction);
                let signal = self.signal.clone();
                let work = Work {
                    time,
                    tip,
                    transactions,
                    signal,
                };
                if let Some(block) = self.sealer.seal(work) {
                    let block: IndexedBlock = block.into();
                    info!(target: "miner", "new block mined: {} -> (number: {}, difficulty: {}, timestamp: {})",
                          block.hash(), block.header.number, block.header.difficulty, block.header.timestamp);
                    if self.chain.process_block(&block).is_ok() {
                        self.announce_new_block(&block);
                    }
                }
            }
            Err(err) => {
                error!(target: "miner", "error generating cellbase transaction: {:?}", err);
            }
        }
    }

    fn create_cellbase_transaction(
        &self,
        head: &Header,
        transactions: &[Transaction],
    ) -> Result<Transaction, Error> {
        // NOTE: To generate different cellbase txid, we put header number in the input script
        let inputs = vec![CellInput::new(
            OutPoint::null(),
            Script::new(0, Vec::new(), head.raw.number.to_le().to_bytes().to_vec()),
        )];
        // NOTE: We could've just used byteorder to serialize u64 and hex string into bytes,
        // but the truth is we will modify this after we designed lock script anyway, so let's
        // stick to the simpler way and just convert everything to a single string, then to UTF8
        // bytes, they really serve the same purpose at the moment
        let reward = self.cellbase_reward(head, transactions)?;
        let outputs = vec![CellOutput::new(
            0,
            reward,
            Vec::new(),
            self.config.redeem_script_hash,
        )];
        Ok(Transaction::new(VERSION, Vec::new(), inputs, outputs))
    }

    fn cellbase_reward(&self, head: &Header, transactions: &[Transaction]) -> Result<u32, Error> {
        let block_reward = self.chain.block_reward(head.raw.number);
        let mut fee = 0;
        for transaction in transactions {
            fee += self.chain.calculate_transaction_fee(transaction)?;
        }
        Ok(block_reward + fee)
    }

    fn announce_new_block(&self, block: &IndexedBlock) {
        self.network.with_context_eval(RELAY_PROTOCOL_ID, |nc| {
            for (peer_id, _session) in nc.sessions() {
                debug!(target: "miner", "announce new block to peer#{:?}, {} => {}",
                       peer_id, block.header().number, block.hash());
                let mut payload = Payload::new();
                let compact_block = CompactBlockBuilder::new(block, &HashSet::new()).build();
                payload.set_compact_block(compact_block.into());
                nc.send(peer_id, payload).ok();
            }
        });
    }
}
