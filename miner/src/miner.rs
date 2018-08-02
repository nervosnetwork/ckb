use super::sealer::{Sealer, Signal};
use super::Config;
use bigint::H256;
use chain::chain::{ChainProvider, Error};
use ckb_notify::{Event, Notify, MINER_SUBSCRIBER};
use ckb_protocol::Payload;
use core::block::IndexedBlock;
use core::header::{Header, IndexedHeader};
use core::transaction::{Capacity, CellInput, CellOutput, Transaction, VERSION};
use core::uncle::UncleBlock;
use crossbeam_channel;
use ethash::{get_epoch, Ethash};
use fnv::{FnvHashMap, FnvHashSet};
use network::NetworkContextExt;
use network::NetworkService;
use pool::TransactionPool;
use std::collections::HashSet;
use std::sync::Arc;
use std::thread;
use sync::compact_block::CompactBlockBuilder;
use sync::RELAY_PROTOCOL_ID;
use time::now_ms;
use util::{RwLock, RwLockUpgradableReadGuard};

pub struct Miner<C> {
    pub config: Config,
    pub chain: Arc<C>,
    pub network: Arc<NetworkService>,
    pub tx_pool: Arc<TransactionPool<C>>,
    pub candidate_uncles: Arc<RwLock<FnvHashMap<H256, Arc<IndexedBlock>>>>,
    pub sealer: Sealer,
    pub signal: Signal,
}

pub struct Work {
    pub time: u64,
    pub tip: IndexedHeader,
    pub cellbase: Transaction,
    pub transactions: Vec<Transaction>,
    pub signal: Signal,
    pub uncles: Vec<UncleBlock>,
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
            candidate_uncles: Arc::new(RwLock::new(FnvHashMap::default())),
        };

        let (tx, rx) = crossbeam_channel::unbounded();
        notify.register_transaction_subscriber(MINER_SUBSCRIBER, tx.clone());
        notify.register_tip_subscriber(MINER_SUBSCRIBER, tx.clone());
        notify.register_side_chain_subscriber(MINER_SUBSCRIBER, tx);
        miner.subscribe_update(rx);
        miner
    }

    pub fn subscribe_update(&self, sub: crossbeam_channel::Receiver<Event>) {
        let signal = self.signal.clone();
        let candidate_uncles = Arc::clone(&self.candidate_uncles);
        thread::spawn(move || {
            // some work here
            loop {
                match sub.recv() {
                    Some(Event::NewTransaction) | Some(Event::NewTip(_)) => {
                        signal.send_abort();
                    }
                    Some(Event::SideChainBlock(block)) => {
                        candidate_uncles.write().insert(block.hash(), block);
                    }
                    None => {
                        info!(target: "miner", "sub channel closed");
                        break;
                    }
                    event => {
                        warn!(target: "miner", "Unexpected sub message {:?}", event);
                    }
                }
            }
        });
    }

    pub fn run_loop(&self) {
        loop {
            self.commit_new_work();
        }
    }

    // Make sure every uncle is rewarded only once
    fn select_uncles(
        &self,
        ancestors: &FnvHashSet<H256>,
        excluded: &FnvHashSet<H256>,
    ) -> Vec<UncleBlock> {
        let max_uncles_len = self.chain.consensus().max_uncles_len();
        let mut included = FnvHashSet::default();
        let mut uncles = Vec::with_capacity(max_uncles_len);
        let mut bad_uncles = Vec::new();
        let r_candidate_uncle = self.candidate_uncles.upgradable_read();
        for (hash, block) in r_candidate_uncle.iter() {
            if uncles.len() == max_uncles_len {
                break;
            }

            if !included.contains(hash)
                && ancestors.contains(&block.header.parent_hash)
                && !excluded.contains(hash)
            {
                if let Some(cellbase) = block.transactions.first() {
                    let uncle = UncleBlock {
                        header: block.header.header.clone(),
                        cellbase: cellbase.clone(),
                    };
                    uncles.push(uncle);
                    included.insert(*hash);
                } else {
                    bad_uncles.push(*hash);
                }
            } else {
                bad_uncles.push(*hash);
            }
        }

        if !bad_uncles.is_empty() {
            let mut w_candidate_uncles = RwLockUpgradableReadGuard::upgrade(r_candidate_uncle);
            for bad in bad_uncles {
                w_candidate_uncles.remove(&bad);
            }
        }

        uncles
    }

    fn make_current_work(&self) -> Result<Work, Error> {
        let tip = self.chain.tip_header().read();
        let now = now_ms();
        let transactions = self
            .tx_pool
            .prepare_mineable_transactions(self.config.max_tx);
        let cellbase = self.create_cellbase_transaction(&tip.header, &transactions)?;

        let mut ancestors = FnvHashSet::default();
        let mut excluded = FnvHashSet::default();

        // cB
        // tip      1 depth, valid uncle
        // tip.p^0  ---/  2
        // tip.p^1  -----/  3
        // tip.p^2  -------/  4
        // tip.p^3  ---------/  5
        // tip.p^4  -----------/  6
        // tip.p^5  -------------/
        // tip.p^6
        let mut block_hash = tip.header.hash();
        excluded.insert(block_hash);
        for _depth in 0..self.chain.consensus().max_uncles_age() {
            if let Some(block) = self.chain.block(&block_hash) {
                ancestors.insert(block.header.parent_hash);
                excluded.insert(block.header.parent_hash);
                for uncle in block.uncles() {
                    excluded.insert(uncle.header.hash());
                }

                block_hash = block.header.parent_hash;
            } else {
                break;
            }
        }

        let uncles = self.select_uncles(&ancestors, &excluded);

        let work = Work {
            cellbase,
            transactions,
            uncles,
            signal: self.signal.clone(),
            tip: tip.header.clone(),
            time: now,
        };

        Ok(work)
    }

    fn commit_new_work(&self) {
        match self.make_current_work() {
            Ok(work) => {
                if let Some(block) = self.sealer.seal(work) {
                    let block: IndexedBlock = block.into();
                    info!(target: "miner", "new block mined: {} -> (number: {}, difficulty: {}, timestamp: {})",
                          block.hash(), block.header.number, block.header.difficulty, block.header.timestamp);
                    if self.chain.process_block(&block, true).is_ok() {
                        self.announce_new_block(&block);
                    }
                }
            }
            Err(err) => {
                error!(target: "miner", "make current work: {:?}", err);
            }
        }
    }

    fn create_cellbase_transaction(
        &self,
        tip: &Header,
        transactions: &[Transaction],
    ) -> Result<Transaction, Error> {
        // NOTE: To generate different cellbase txid, we put header number in the input script
        let inputs = vec![CellInput::new_cellbase_input(tip.raw.number + 1)];
        // NOTE: We could've just used byteorder to serialize u64 and hex string into bytes,
        // but the truth is we will modify this after we designed lock script anyway, so let's
        // stick to the simpler way and just convert everything to a single string, then to UTF8
        // bytes, they really serve the same purpose at the moment
        let reward = self.cellbase_reward(tip, transactions)?;
        let outputs = vec![CellOutput::new(
            reward,
            Vec::new(),
            self.config.redeem_script_hash,
        )];
        Ok(Transaction::new(VERSION, Vec::new(), inputs, outputs))
    }

    fn cellbase_reward(
        &self,
        tip: &Header,
        transactions: &[Transaction],
    ) -> Result<Capacity, Error> {
        let block_reward = self.chain.block_reward(tip.raw.number + 1);
        let mut fee = 0;
        for transaction in transactions {
            fee += self.chain.calculate_transaction_fee(transaction)?;
        }
        Ok(block_reward + fee)
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
