use bigint::H256;
use chain::chain::Chain;
use core::block::Block;
use core::global::TIME_STEP;
use core::proof::Proof;
use pool::TransactionPool;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use time::now_ms;

// Max number of transactions this miner will assemble in a block
// TODO move it to config
const MAX_TX: usize = 1024;

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

        loop {
            thread::sleep(Duration::from_millis(TIME_STEP / 10));
            let time = now_ms();
            if time / TIME_STEP <= pre_time / TIME_STEP {
                continue;
            }
            pre_time = time;

            info!(target: "miner", "minning loop ...");

            let head_header = self.chain.head_header();
            if head_header != pre_header {
                challenge = self.chain.challenge(&head_header).unwrap();
                difficulty = self.chain.cal_difficulty(&head_header);
                pre_header = head_header;
            }

            let proof = Proof::new(&self.miner_key, time, pre_header.height + 1, &challenge);

            if proof.difficulty() < difficulty {
                let txs = self.tx_pool.get_transactions(MAX_TX);
                let mut block = Block::new(
                    pre_header.hash(),
                    time,
                    pre_header.height + 1,
                    difficulty,
                    challenge,
                    proof,
                    txs,
                );
                block.sign(self.signer_key);
                self.chain.process_block(&block);
            }
        }
    }
}
