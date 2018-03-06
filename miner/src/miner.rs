use bigint::H256;
use chain::chain::Chain;
use core::block::Block;
use core::global::{MAX_TX, TIME_STEP};
use core::proof::Proof;
use pool::TransactionPool;
use std::sync::Arc;
use std::thread;
use time::{now_ms, Duration};

const FREQUENCY: usize = 50;

pub struct Miner {
    pub chain: Arc<Chain>,
    pub tx_pool: Arc<TransactionPool>,
    pub miner_key: Vec<u8>,
    pub signer_key: H256,
}

impl Miner {
    pub fn run_loop(&self) {
        let mut pre_header = self.chain.head_header();
        let mut challenge = self.chain.challenge(&pre_header).unwrap();
        let mut difficulty = self.chain.cal_difficulty(&pre_header);
        let mut pre_time = now_ms();
        let mut num: usize = 0;

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
                difficulty = self.chain.cal_difficulty(&head_header);
                pre_header = head_header;
            }

            let proof = Proof::new(&self.miner_key, time, pre_header.height + 1, &challenge);

            if proof.difficulty() > difficulty {
                let txs = self.tx_pool.get_transactions(MAX_TX);
                let mut block = Block::new(&pre_header, time, difficulty, challenge, proof, txs);
                block.sign(self.signer_key);

                info!(target: "miner", "new block mined: {}", block.hash());
                self.chain.process_block(&block).unwrap();
            }
        }
    }
}
