use super::build_block_template;
use super::sealer::{Sealer, Signal};
use super::Config;
use bigint::{H256, U256};
use chain::chain::ChainProvider;
use chain::error::Error;
use ckb_notify::{Event, Notify, MINER_SUBSCRIBER};
use ckb_protocol::Payload;
use core::block::IndexedBlock;
use core::header::{Header, IndexedHeader};
use core::transaction::{
    Capacity, CellInput, CellOutput, IndexedTransaction, ProposalShortId, ProposalTransaction,
    Transaction, VERSION,
};
use core::uncle::UncleBlock;
use core::BlockNumber;
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
    pub cellbase: IndexedTransaction,
    pub difficulty: U256,
    pub propasal: FnvHashSet<ProposalTransaction>,
    pub commit: Vec<IndexedTransaction>,
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

    // Make sure every uncle is rewarded only once
    fn select_uncles(
        &self,
        current_number: BlockNumber,
        excluded: &FnvHashSet<H256>,
    ) -> Vec<UncleBlock> {
        let max_uncles_len = self.chain.consensus().max_uncles_len();
        let max_uncles_age = self.chain.consensus().max_uncles_age();
        let mut included = FnvHashSet::default();
        let mut uncles = Vec::with_capacity(max_uncles_len);
        let mut bad_uncles = Vec::new();
        let r_candidate_uncle = self.candidate_uncles.upgradable_read();
        for (hash, block) in r_candidate_uncle.iter() {
            if uncles.len() == max_uncles_len {
                break;
            }

            let depth = current_number.saturating_sub(block.number());
            if depth > max_uncles_age as u64
                || depth < 1
                || included.contains(hash)
                || excluded.contains(hash)
            {
                bad_uncles.push(*hash);
            } else if let Some(cellbase) = block.commit_transactions.first() {
                let uncle = UncleBlock {
                    header: block.header.header.clone(),
                    cellbase: cellbase.clone().into(),
                    proposal_transactions: block.proposal_transactions.clone(),
                };
                uncles.push(uncle);
                included.insert(*hash);
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

    fn select_commit_ids(&self, tip: &IndexedHeader) -> FnvHashSet<ProposalShortId> {
        let mut proposal_txs_ids = FnvHashSet::default();
        if tip.is_genesis() {
            return proposal_txs_ids;
        }
        let mut walk = self.chain.consensus().transaction_propagation_timeout;
        let mut block_hash = tip.hash();

        while walk > 0 {
            let block = self
                .chain
                .block(&block_hash)
                .expect("main chain should be stored");
            if block.is_genesis() {
                break;
            }
            proposal_txs_ids.extend(
                block.proposal_transactions().iter().chain(
                    block
                        .uncles()
                        .iter()
                        .flat_map(|uncle| uncle.proposal_transactions()),
                ),
            );
            block_hash = block.header.parent_hash;
            walk -= 1;
        }

        proposal_txs_ids
    }

    fn make_current_work(&self) -> Result<Work, Error> {
        let tip = self.chain.tip_header().read();
        let now = cmp::max(now_ms(), tip.header.timestamp + 1);

        let difficulty = self
            .chain
            .calculate_difficulty(&tip.header)
            .expect("get difficulty");

        let propasal = self.tx_pool.prepare_proposal(self.config.max_tx);

        let include_ids = self.select_commit_ids(&tip.header);
        let commit =
            self.tx_pool
                .prepare_commit(tip.header.number + 1, &include_ids, self.config.max_tx);

        let cellbase = self.create_cellbase_transaction(&tip.header, &commit)?;

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
                excluded.insert(block.header.parent_hash);
                for uncle in block.uncles() {
                    excluded.insert(uncle.header.hash());
                }

                block_hash = block.header.parent_hash;
            } else {
                break;
            }
        }

        let current_number = tip.header.number + 1;
        let uncles = self.select_uncles(current_number, &excluded);

        let work = Work {
            cellbase,
            propasal,
            commit,
            difficulty,
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
                if let Some((block, propasal)) = self.sealer.seal(work) {
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

    fn create_cellbase_transaction(
        &self,
        tip: &Header,
        transactions: &[IndexedTransaction],
    ) -> Result<IndexedTransaction, Error> {
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
        Ok(Transaction::new(VERSION, Vec::new(), inputs, outputs).into())
    }

    fn cellbase_reward(
        &self,
        tip: &Header,
        transactions: &[IndexedTransaction],
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
