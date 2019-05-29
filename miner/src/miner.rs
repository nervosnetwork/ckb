use crate::client::Client;
use crate::Work;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::difficulty::difficulty_to_target;
use ckb_core::header::{HeaderBuilder, RawHeader, Seal};
use ckb_pow::{PowEngine, Work as PowWork};
use crossbeam_channel::Receiver;
use failure::Error;
use jsonrpc_types::{BlockTemplate, CellbaseTemplate};
use log::{error, info, trace};
use rand::{thread_rng, Rng};
use std::convert::TryInto;
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
            match self.mine() {
                Ok(result) => {
                    if let Some((work_id, block)) = result {
                        self.client.submit_block(&work_id, &block);
                        self.client.try_update_block_template();
                    }
                }
                Err(e) => error!(target: "miner", "mining error encountered: {:?}", e),
            };
        }
    }

    fn mine(&self) -> Result<Option<(String, Block)>, Error> {
        let current_work = { self.current_work.lock().to_owned() };
        if let Some(template) = current_work {
            let BlockTemplate {
                version,
                difficulty,
                current_time,
                number,
                epoch,
                parent_hash,
                uncles, // Vec<UncleTemplate>
                transactions, // Vec<TransactionTemplate>
                proposals, // Vec<ProposalShortId>
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
                .version(version.0)
                .number(number.0)
                .epoch(epoch.0)
                .difficulty(difficulty)
                .timestamp(current_time.0)
                .parent_hash(parent_hash);

            let block = BlockBuilder::from_header_builder(header_builder)
                .uncles(
                    uncles
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<_, _>>()?,
                )
                .transaction(cellbase.try_into()?)
                .transactions(
                    transactions
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<_, _>>()?,
                )
                .proposals(
                    proposals
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<_, _>>()?,
                )
                .build();

            let raw_header = block.header().raw().to_owned();

            Ok(self
                .mine_loop(&raw_header)
                .map(|seal| {
                    BlockBuilder::from_block(block)
                        .header(raw_header.with_seal(seal))
                        .build()
                })
                .map(|block| (work_id.0.to_string(), block)))
        } else {
            Ok(None)
        }
    }

    fn mine_loop(&self, header: &RawHeader) -> Option<Seal> {
        let mut work = PowWork {
            block_number: header.number(),
            pow_hash: header.pow_hash(),
            target_difficulty: difficulty_to_target(&header.difficulty()),
            nonce: thread_rng().gen(),
        };

        loop {
            if self.new_work_rx.try_recv().is_ok() {
                break None;
            }
            trace!(target: "miner", "mining block #{} with nonce {}", work.block_number, work.nonce);
            if let Some(seal) = self.pow.find_seal(&work) {
                info!(target: "miner", "found seal {:?} of block #{}", seal, work.block_number);
                break Some(seal);
            }
            work.nonce = work.nonce.wrapping_add(1);
        }
    }
}
