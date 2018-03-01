use bigint::{H256, H512, U256};
use chain::chain::Chain;
use core::block::{Block, Header};
use core::proof::Proof;
use pool::TransactionPool;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

// Max number of transactions this miner will assemble in a block
// TODO move it to config
const MAX_TX: usize = 1024;

pub struct Miner {
    pub chain: Arc<Chain>,
    pub tx_pool: Arc<TransactionPool>,
    pub private_key: Vec<u8>,
}

impl Miner {
    pub fn run_loop(&self) {
        loop {
            info!(target: "miner", "minning loop ...");
            let mut block = self.build_block(&self.chain.head_header());
            if let Some(proof) = self.inner_loop(&block) {
                block.header.proof = proof;
                self.chain.process_block(&block);
            }
        }
    }

    fn build_block(&self, h: &Header) -> Block {
        let txs = self.tx_pool.get_transactions(MAX_TX);
        // TODO setup timestamp, difficult, etc
        Block {
            header: Header {
                parent_hash: h.hash(),
                timestamp: 0,
                transactions_root: H256::from(0),
                difficulty: U256::from(0),
                challenge: H256::from(0),
                proof: Proof::new(&[0], 0, 0, H256::from(0)),
                height: 0,
                signature: H512::from(0),
            },
            transactions: txs,
        }
    }

    fn inner_loop(&self, b: &Block) -> Option<Proof> {
        loop {
            info!(target: "miner", "minning inner loop ...");
            let proof = Proof::new(
                &self.private_key,
                b.header.timestamp,
                b.header.height,
                b.header.challenge,
            );
            if proof.difficulty() > b.header.difficulty {
                return Some(proof);
            } else {
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}
