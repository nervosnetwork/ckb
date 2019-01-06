use crate::types::BlockTemplate;
use channel::{self, select, Receiver, Sender};
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::header::{Header, HeaderBuilder};
use ckb_core::service::{Request, DEFAULT_CHANNEL_SIZE};
use ckb_core::transaction::{CellInput, CellOutput, Transaction, TransactionBuilder};
use ckb_core::uncle::UncleBlock;
use ckb_notify::NotifyController;
use ckb_pool::txs_pool::TransactionPoolController;
use ckb_shared::error::SharedError;
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::{ChainProvider, Shared};
use faketime::unix_time_as_millis;
use fnv::{FnvHashMap, FnvHashSet};
use log::error;
use numext_fixed_hash::H256;
use std::cmp;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

const MINER_AGENT_SUBSCRIBER: &str = "miner_agent";

pub struct Agent<CI> {
    shared: Shared<CI>,
    tx_pool: TransactionPoolController,
    candidate_uncles: FnvHashMap<H256, Arc<Block>>,
}

type BlockTemplateArgs = (H256, usize, usize);
type BlockTemplateReturn = Result<BlockTemplate, SharedError>;

#[derive(Clone)]
pub struct AgentController {
    get_block_template_sender: Sender<Request<BlockTemplateArgs, BlockTemplateReturn>>,
}

pub struct AgentReceivers {
    get_block_template_receiver: Receiver<Request<BlockTemplateArgs, BlockTemplateReturn>>,
}

impl AgentController {
    pub fn build() -> (AgentController, AgentReceivers) {
        let (get_block_template_sender, get_block_template_receiver) =
            channel::bounded(DEFAULT_CHANNEL_SIZE);
        (
            AgentController {
                get_block_template_sender,
            },
            AgentReceivers {
                get_block_template_receiver,
            },
        )
    }

    pub fn get_block_template(
        &self,
        type_hash: H256,
        max_tx: usize,
        max_prop: usize,
    ) -> BlockTemplateReturn {
        Request::call(
            &self.get_block_template_sender,
            (type_hash, max_tx, max_prop),
        )
        .expect("get_block_template() failed")
    }
}

impl<CI: ChainIndex + 'static> Agent<CI> {
    pub fn new(shared: Shared<CI>, tx_pool: TransactionPoolController) -> Self {
        Self {
            shared,
            tx_pool,
            candidate_uncles: FnvHashMap::default(),
        }
    }

    pub fn start<S: ToString>(
        mut self,
        thread_name: Option<S>,
        receivers: AgentReceivers,
        notify: &NotifyController,
    ) -> JoinHandle<()> {
        let mut thread_builder = thread::Builder::new();
        // Mainly for test: give a empty thread_name
        if let Some(name) = thread_name {
            thread_builder = thread_builder.name(name.to_string());
        }

        let new_uncle_receiver = notify.subscribe_new_uncle(MINER_AGENT_SUBSCRIBER);
        thread_builder
            .spawn(move || loop {
                select! {
                    recv(new_uncle_receiver) -> msg => match msg {
                        Ok(uncle_block) => {
                            let hash = uncle_block.header().hash().clone();
                            self.candidate_uncles.insert(hash, uncle_block);
                        }
                        _ => {
                            error!(target: "miner", "new_uncle_receiver closed");
                            break;
                        }
                    },
                    recv(receivers.get_block_template_receiver) -> msg => match msg {
                        Ok(Request { responder, arguments: (type_hash, max_tx, max_prop) }) => {
                            let _ = responder.send(self.get_block_template(type_hash, max_tx, max_prop));
                        },
                        _ => {
                            error!(target: "miner", "get_block_template_receiver closed");
                            break;
                        },
                    }
                }
            }).expect("Start MinerAgent failed")
    }

    fn get_block_template(
        &mut self,
        type_hash: H256,
        max_tx: usize,
        max_prop: usize,
    ) -> Result<BlockTemplate, SharedError> {
        let (cellbase, commit_transactions, proposal_transactions, header_builder) = {
            let chain_state = self.shared.chain_state().read();
            let header = chain_state.tip_header();
            let now = cmp::max(unix_time_as_millis(), header.timestamp() + 1);
            let difficulty = self
                .shared
                .calculate_difficulty(header)
                .expect("get difficulty");

            let (proposal_transactions, commit_transactions) = self
                .tx_pool
                .get_proposal_commit_transactions(max_prop, max_tx);

            let cellbase =
                self.create_cellbase_transaction(header, &commit_transactions, type_hash)?;

            let header_builder = HeaderBuilder::default()
                .parent_hash(header.hash().clone())
                .timestamp(now)
                .number(header.number() + 1)
                .difficulty(difficulty)
                .cellbase_id(cellbase.hash().clone());
            (
                cellbase,
                commit_transactions,
                proposal_transactions,
                header_builder,
            )
        };

        let block = BlockBuilder::default()
            .commit_transaction(cellbase)
            .commit_transactions(commit_transactions)
            .proposal_transactions(proposal_transactions)
            .uncles(self.get_tip_uncles())
            .with_header_builder(header_builder);

        Ok(BlockTemplate {
            raw_header: block.header().clone().into_raw(),
            uncles: block.uncles().to_vec(),
            commit_transactions: block.commit_transactions().to_vec(),
            proposal_transactions: block.proposal_transactions().to_vec(),
        })
    }

    fn create_cellbase_transaction(
        &self,
        header: &Header,
        transactions: &[Transaction],
        type_hash: H256,
    ) -> Result<Transaction, SharedError> {
        // NOTE: To generate different cellbase txid, we put header number in the input script
        let input = CellInput::new_cellbase_input(header.number() + 1);
        // NOTE: We could've just used byteorder to serialize u64 and hex string into bytes,
        // but the truth is we will modify this after we designed lock script anyway, so let's
        // stick to the simpler way and just convert everything to a single string, then to UTF8
        // bytes, they really serve the same purpose at the moment
        let block_reward = self.shared.block_reward(header.number() + 1);
        let mut fee = 0;
        for transaction in transactions {
            fee += self.shared.calculate_transaction_fee(transaction)?;
        }

        let output = CellOutput::new(block_reward + fee, Vec::new(), type_hash, None);

        Ok(TransactionBuilder::default()
            .input(input)
            .output(output)
            .build())
    }

    fn get_tip_uncles(&mut self) -> Vec<UncleBlock> {
        let max_uncles_age = self.shared.consensus().max_uncles_age();
        let chain_state = self.shared.chain_state().read();
        let header = chain_state.tip_header();
        let mut excluded = FnvHashSet::default();

        // cB
        // tip      1 depth, valid uncle
        // tip.p^0  ---/  2
        // tip.p^1  -----/  3
        // tip.p^2  -------/  4
        // tip.p^3  ---------/  5
        // tip.p^4  -----------/  6
        // tip.p^5  -------------/
        // tip.p^6
        let mut block_hash = header.hash().clone();
        excluded.insert(block_hash.clone());
        for _depth in 0..max_uncles_age {
            if let Some(block) = self.shared.block(&block_hash) {
                excluded.insert(block.header().parent_hash().clone());
                for uncle in block.uncles() {
                    excluded.insert(uncle.header.hash().clone());
                }

                block_hash = block.header().parent_hash().clone();
            } else {
                break;
            }
        }

        let tip_difficulty_epoch =
            header.number() / self.shared.consensus().difficulty_adjustment_interval();

        let max_uncles_len = self.shared.consensus().max_uncles_len();
        let mut included = FnvHashSet::default();
        let mut uncles = Vec::with_capacity(max_uncles_len);
        let mut bad_uncles = Vec::new();
        let current_number = header.number() + 1;
        for (hash, block) in &self.candidate_uncles {
            if uncles.len() == max_uncles_len {
                break;
            }

            let block_difficulty_epoch =
                block.header().number() / self.shared.consensus().difficulty_adjustment_interval();

            // uncle must be same difficulty epoch with tip
            if block.header().difficulty() != header.difficulty()
                || block_difficulty_epoch != tip_difficulty_epoch
            {
                bad_uncles.push(hash.clone());
                continue;
            }

            let depth = current_number.saturating_sub(block.header().number());
            if depth > max_uncles_age as u64
                || depth < 1
                || included.contains(hash)
                || excluded.contains(hash)
            {
                bad_uncles.push(hash.clone());
            } else if let Some(cellbase) = block.commit_transactions().first() {
                let uncle = UncleBlock {
                    header: block.header().clone(),
                    cellbase: cellbase.clone(),
                    proposal_transactions: block.proposal_transactions().to_vec(),
                };
                uncles.push(uncle);
                included.insert(hash.clone());
            } else {
                bad_uncles.push(hash.clone());
            }
        }

        if !bad_uncles.is_empty() {
            for bad in bad_uncles {
                self.candidate_uncles.remove(&bad);
            }
        }

        uncles
    }
}
