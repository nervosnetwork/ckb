use super::BlockTemplate;
use bigint::{H256, U256};
use core::block::IndexedBlock;
use core::difficulty::difficulty_to_boundary;
use core::header::{BlockNumber, RawHeader};
use core::transaction::ProposalTransaction;
use core::uncle::uncles_hash;
use ethash::Ethash;
use fnv::FnvHashSet;
use miner::Work;
use rand::{thread_rng, Rng};
use std::sync::mpsc;
use std::sync::Arc;
use std::{thread, time as std_time};

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
    pub ethash: Option<Arc<Ethash>>,
    pub signal: mpsc::Receiver<Message>,
}

impl Sealer {
    pub fn new(ethash: Option<Arc<Ethash>>) -> (Self, Signal) {
        let (signal_tx, signal_rx) = mpsc::channel();
        (
            Sealer {
                ethash,
                signal: signal_rx,
            },
            self::Signal::new(signal_tx),
        )
    }

    pub fn seal(&self, work: Work) -> Option<(IndexedBlock, FnvHashSet<ProposalTransaction>)> {
        let Work {
            time,
            tip,
            cellbase,
            difficulty,
            mut commit,
            propasal,
            signal,
            uncles,
        } = work;

        let uncles_hash = uncles_hash(&uncles);
        let cellbase_id = cellbase.hash();
        commit.insert(0, cellbase);

        let raw_header = RawHeader::new(
            &tip,
            commit.iter(),
            propasal.iter(),
            time,
            difficulty,
            cellbase_id,
            uncles_hash,
        );
        let pow_hash = raw_header.pow_hash();
        let number = raw_header.number;

        let nonce: u64 = thread_rng().gen();
        match self.mine(
            raw_header.pow_hash(),
            raw_header.number,
            nonce,
            raw_header.difficulty,
            &signal,
        ) {
            self::Message::Found(solution) => {
                let Solution { nonce, mix_hash } = solution;
                let header = raw_header.with_seal(nonce, mix_hash);
                Some((
                    IndexedBlock {
                        header: header.into(),
                        uncles,
                        commit_transactions: commit,
                        proposal_transactions: propasal
                            .iter()
                            .map(|p| p.proposal_short_id())
                            .collect(),
                    },
                    propasal,
                ))
            }
            self::Message::Abort => None,
        }
    }

    fn mine(
        &self,
        pow_hash: H256,
        number: BlockNumber,
        mut nonce: u64,
        difficulty: U256,
        signal: &Signal,
    ) -> Message {
        let boundary = difficulty_to_boundary(&difficulty);
        loop {
            if let Ok(message) = self.signal.try_recv() {
                break message;
            }
            match self.ethash {
                Some(ref ethash) => {
                    let signal = signal.clone();
                    let ethash = Arc::clone(&ethash);
                    let pow = ethash.compute(number, pow_hash, nonce);
                    if pow.value < boundary {
                        signal.send_found(Solution {
                            nonce,
                            mix_hash: pow.mix,
                        });
                    }
                    nonce = nonce.wrapping_add(1);
                }
                None => {
                    thread::sleep(std_time::Duration::from_secs(thread_rng().gen_range(2, 5)));
                    signal.send_found(Solution {
                        nonce: 0,
                        mix_hash: H256::from(0),
                    });
                }
            }
        }
    }
}
