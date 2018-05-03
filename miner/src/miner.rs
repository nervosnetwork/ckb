use bigint::{H160, H256};
use chain::chain::ChainClient;
use core::block::Block;
use core::difficulty::cal_difficulty;
use core::global::{MAX_TX, TIME_STEP};
use core::proof::Proof;
use network::{Broadcastable, Network};
use pool::TransactionPool;
use std::sync::Arc;
use std::thread;
use time::{now_ms, Duration};
use util::Mutex;

const FREQUENCY: usize = 50;

pub struct Miner<C> {
    pub chain: Arc<C>,
    pub tx_pool: Arc<TransactionPool<C>>,
    pub miner_key: H160,
    pub signer_key: H256,
    pub network: Arc<Network>,
    pub lock: Arc<Mutex<()>>,
}

impl<C: ChainClient> Miner<C> {
    pub fn new(
        chain: Arc<C>,
        tx_pool: &Arc<TransactionPool<C>>,
        miner_key: H160,
        signer_key: H256,
        network: &Arc<Network>,
        lock: &Arc<Mutex<()>>,
    ) -> Self {
        Miner {
            chain,
            miner_key,
            signer_key,
            tx_pool: Arc::clone(tx_pool),
            network: Arc::clone(network),
            lock: Arc::clone(lock),
        }
    }

    pub fn run_loop(&self) {
        let mut pre_header = { self.chain.head_header().clone() };
        let mut challenge = self.chain.challenge(&pre_header).unwrap();
        let mut pre_time = now_ms();
        let mut num: usize = 0;
        let mut mined: bool = false;

        loop {
            thread::sleep(Duration::from_millis(TIME_STEP / 10));
            let _guard = self.lock.lock();
            let time = now_ms();
            if time / TIME_STEP <= pre_time / TIME_STEP {
                continue;
            }
            pre_time = time;

            num += 1;
            if num == FREQUENCY {
                info!(target: "miner", "{} times is tried", FREQUENCY);
                num = 0;
            }
            {
                let head_guard = self.chain.head_header();

                if time / TIME_STEP <= head_guard.timestamp / TIME_STEP {
                    pre_time = head_guard.timestamp;
                    continue;
                }

                if *head_guard != pre_header {
                    challenge = self.chain.challenge(&head_guard).unwrap();
                    pre_header = head_guard.clone();
                    mined = false;
                }
            }

            if mined {
                continue;
            }

            let difficulty = cal_difficulty(&pre_header, time);
            let proof = Proof::new(&self.miner_key, time, pre_header.height + 1, &challenge);

            if proof.difficulty() > difficulty {
                let txs = self.tx_pool.prepare_mineable_transactions(MAX_TX);
                let mut block = Block::new(&pre_header, time, difficulty, challenge, proof, txs);
                block.sign(self.signer_key);

                info!(target: "miner", "new block mined: {} -> ({}, {})", block.hash(), block.header.timestamp, block.header.difficulty);
                if self.chain.process_block(&block).is_ok() {
                    self.announce_new_block(&block);
                }

                mined = true;
            }
        }
    }

    fn announce_new_block(&self, block: &Block) {
        self.network.broadcast(Broadcastable::Block(block.into()));
    }
}
