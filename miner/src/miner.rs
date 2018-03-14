use bigint::{H160, H256};
use chain::chain::Chain;
use core::adapter::ChainAdapter;
use core::block::Block;
use core::global::{MAX_TX, TIME_STEP};
use core::proof::Proof;
use db::store::ChainStore;
use pool::TransactionPool;
use std::sync::Arc;
use std::thread;
use time::{now_ms, Duration};

const FREQUENCY: usize = 50;

pub struct Miner<CA, CS> {
    pub chain: Arc<Chain<CA, CS>>,
    pub tx_pool: Arc<TransactionPool>,
    pub miner_key: H160,
    pub signer_key: H256,
}

impl<CA: ChainAdapter, CS: ChainStore> Miner<CA, CS> {
    pub fn run_loop(&self) {
        let mut pre_header = self.chain.head_header();
        let mut challenge = self.chain.challenge(&pre_header).unwrap();
        let mut pre_time = now_ms();
        let mut num: usize = 0;
        let mut mined: bool = false;

        loop {
            thread::sleep(Duration::from_millis(TIME_STEP / 10));
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

            let head_header = self.chain.head_header();

            if time / TIME_STEP <= head_header.timestamp / TIME_STEP {
                pre_time = head_header.timestamp;
                continue;
            }

            if head_header != pre_header {
                challenge = self.chain.challenge(&head_header).unwrap();
                pre_header = head_header;
                mined = false;
            }

            if mined {
                continue;
            }

            let difficulty = self.chain.cal_difficulty(&pre_header, time);
            let proof = Proof::new(&self.miner_key, time, pre_header.height + 1, &challenge);

            if proof.difficulty() > difficulty {
                let txs = self.tx_pool.get_transactions(MAX_TX);
                let mut block = Block::new(&pre_header, time, difficulty, challenge, proof, txs);
                block.sign(self.signer_key);

                info!(target: "miner", "new block mined: {} -> ({}, {})", block.hash(), block.header.timestamp, block.header.difficulty);
                self.chain.process_block(&block).unwrap();

                mined = true;
            }
        }
    }
}
