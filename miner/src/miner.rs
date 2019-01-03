use crate::client::Client;
use crate::types::{BlockTemplate, Shared};
use channel::Receiver;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::header::{RawHeader, Seal};
use ckb_pow::PowEngine;
use log::{debug, info};
use rand::{thread_rng, Rng};
use std::sync::Arc;

pub struct Miner {
    pub pow: Arc<dyn PowEngine>,
    pub new_job_rx: Receiver<()>,
    pub shared: Shared,
    pub client: Client,
}

impl Miner {
    pub fn run(&self) {
        loop {
            if let Some(block) = self.mine() {
                self.client.submit_block(&block);
            }
        }
    }

    fn mine(&self) -> Option<Block> {
        if let Some(BlockTemplate {
            raw_header,
            uncles,
            commit_transactions,
            proposal_transactions,
        }) = self.shared.inner.read().clone()
        {
            self.mine_loop(&raw_header).map(|seal| {
                BlockBuilder::default()
                    .header(raw_header.with_seal(seal))
                    .uncles(uncles)
                    .commit_transactions(commit_transactions)
                    .proposal_transactions(proposal_transactions)
                    .build()
            })
        } else {
            None
        }
    }

    fn mine_loop(&self, header: &RawHeader) -> Option<Seal> {
        let mut nonce: u64 = thread_rng().gen();
        loop {
            if self.new_job_rx.try_recv().is_ok() {
                break None;
            }
            debug!(target: "miner", "mining header #{} with nonce {}", header.number(), nonce);
            if let Some(seal) = self.pow.solve_header(header, nonce) {
                info!(target: "miner", "found seal: {:?}", seal);
                break Some(seal);
            }
            nonce = nonce.wrapping_add(1);
        }
    }
}
