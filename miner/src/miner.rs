use crate::client::Client;
use crate::Work;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::header::{HeaderBuilder, RawHeader, Seal};
use ckb_pow::PowEngine;
use ckb_util::TryInto;
use crossbeam_channel::Receiver;
use jsonrpc_types::{BlockTemplate, CellbaseTemplate};
use log::{debug, error, info};
use rand::{thread_rng, Rng};
use std::sync::Arc;

pub struct Miner {
    pub pow: Arc<dyn PowEngine>,
    pub new_work_rx: Receiver<()>,
    pub current_work: Work,
    pub client: Client,
}

impl Miner {
    pub fn new(
        current_work: Work,
        pow: Arc<dyn PowEngine>,
        new_work_rx: Receiver<()>,
        client: Client,
    ) -> Miner {
        Miner {
            pow,
            new_work_rx,
            current_work,
            client,
        }
    }
    pub fn run(&self) {
        loop {
            self.client.try_update_block_template();
            if let Some((work_id, block)) = self.mine() {
                self.client.submit_block(&work_id, &block);
            }
        }
    }

    fn mine(&self) -> Option<(String, Block)> {
        if let Some(template) = { self.current_work.lock().clone() } {
            let BlockTemplate {
                version,
                difficulty,
                current_time,
                number,
                parent_hash,
                uncles, // Vec<UncleTemplate>
                commit_transactions, // Vec<TransactionTemplate>
                proposal_transactions, // Vec<ProposalShortId>
                cellbase, // CellbaseTemplate
                work_id,
                ..
                // cycles_limit,
                // bytes_limit,
                // uncles_count_limit,
            } = template;

            let cellbase = {
                let CellbaseTemplate { data, .. } = cellbase;
                data
            };

            let header_builder = HeaderBuilder::default()
                .version(version)
                .number(number)
                .difficulty(difficulty)
                .timestamp(current_time)
                .parent_hash(parent_hash);

            let uncles = uncles.into_iter().map(TryInto::try_into).collect();
            if let Err(ref e) = uncles {
                error!(target: "miner", "error parsing uncles: {:?}", e);
            }
            let cellbase = cellbase.try_into();
            if let Err(ref e) = cellbase {
                error!(target: "miner", "error parsing cellbase: {:?}", e);
            }
            let commit_transactions = commit_transactions
                .into_iter()
                .map(TryInto::try_into)
                .collect();
            if let Err(ref e) = commit_transactions {
                error!(target: "miner", "error parsing commit transactions: {:?}", e);
            }
            let proposal_transactions = proposal_transactions
                .into_iter()
                .map(TryInto::try_into)
                .collect();
            if let Err(ref e) = proposal_transactions {
                error!(target: "miner", "error parsing proposal transactions: {:?}", e);
            }

            let block = BlockBuilder::default()
                .uncles(uncles.unwrap())
                .commit_transaction(cellbase.unwrap())
                .commit_transactions(commit_transactions.unwrap())
                .proposal_transactions(proposal_transactions.unwrap())
                .with_header_builder(header_builder);

            let raw_header = block.header().raw().clone();

            self.mine_loop(&raw_header)
                .map(|seal| {
                    BlockBuilder::default()
                        .block(block)
                        .header(raw_header.with_seal(seal))
                        .build()
                })
                .map(|block| (work_id, block))
        } else {
            None
        }
    }

    fn mine_loop(&self, header: &RawHeader) -> Option<Seal> {
        let mut nonce: u64 = thread_rng().gen();
        loop {
            if self.new_work_rx.try_recv().is_ok() {
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
