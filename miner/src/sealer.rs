use bigint::{H256, U256};
use core::block::Block;
use core::difficulty::cal_difficulty;
use core::difficulty::difficulty_to_boundary;
use core::header::RawHeader;
use ethash::Ethash;
use miner::Work;
use rand::{thread_rng, Rng};
use std::sync::mpsc;
use std::sync::Arc;

pub struct Solution {
    pub nonce: u64,
    pub mix_hash: H256,
}

pub enum Message {
    Abort,
    Found(Solution),
}

#[derive(Clone)]
pub struct Signal {
    inner: mpsc::Sender<Message>,
}

impl Signal {
    pub fn new(inner: mpsc::Sender<Message>) -> Self {
        Signal { inner }
    }

    pub fn send_abort(&self) {
        let _ = self.inner.send(Message::Abort);
    }

    pub fn send_found(&self, solution: Solution) {
        let _ = self.inner.send(Message::Found(solution));
    }
}

pub struct Sealer {
    pub ethash: Arc<Ethash>,
    pub signal: mpsc::Receiver<Message>,
}

impl Sealer {
    pub fn new(ethash: &Arc<Ethash>) -> (Sealer, Signal) {
        let (signal_tx, signal_rx) = mpsc::channel();
        (
            Sealer {
                ethash: Arc::clone(ethash),
                signal: signal_rx,
            },
            self::Signal::new(signal_tx),
        )
    }

    pub fn seal(&self, work: Work) -> Option<Block> {
        let Work {
            time,
            head,
            transactions,
            signal,
        } = work;
        let difficulty = cal_difficulty(&head, time);

        let raw_header = RawHeader::new(&head, transactions.iter(), time, difficulty);
        let pow_hash = raw_header.pow_hash();
        let height = raw_header.height;

        let nonce: u64 = thread_rng().gen();
        match self.mine(pow_hash, height, nonce, difficulty, &signal) {
            self::Message::Found(solution) => {
                let Solution { nonce, mix_hash } = solution;
                let header = raw_header.with_seal(nonce, mix_hash);
                Some(Block {
                    header,
                    transactions,
                })
            }
            self::Message::Abort => None,
        }
    }

    fn mine(
        &self,
        pow_hash: H256,
        height: u64,
        mut nonce: u64,
        difficulty: U256,
        signal: &Signal,
    ) -> Message {
        let boundary = difficulty_to_boundary(&difficulty);
        loop {
            if let Ok(message) = self.signal.try_recv() {
                break message;
            }
            let signal = signal.clone();
            let ethash = Arc::clone(&self.ethash);
            let pow = ethash.compute(height, pow_hash, nonce);
            if pow.value < boundary {
                signal.send_found(Solution {
                    nonce,
                    mix_hash: pow.mix,
                });
            }
            nonce = nonce.wrapping_add(1);
        }
    }
}
