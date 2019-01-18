use ckb_core::block::Block;
use ckb_core::header::Header;
use ckb_core::service::{Request, DEFAULT_CHANNEL_SIZE};
use ckb_core::transaction::{CellInput, CellOutput, Transaction, TransactionBuilder};
use ckb_core::uncle::UncleBlock;
use ckb_core::{Cycle, Version};
use ckb_notify::NotifyController;
use ckb_pool::txs_pool::TransactionPoolController;
use ckb_shared::error::SharedError;
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::{ChainProvider, Shared};
use crossbeam_channel::{self, select, Receiver, Sender};
use faketime::unix_time_as_millis;
use fnv::FnvHashSet;
use jsonrpc_types::{BlockTemplate, CellbaseTemplate, TransactionTemplate, UncleTemplate};
use log::error;
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use std::cmp;
use std::sync::{atomic::AtomicUsize, atomic::Ordering, Arc};
use std::thread::{self, JoinHandle};

const MAX_CANDIDATE_UNCLES: usize = 42;
type BlockTemplateParams = (Option<Cycle>, Option<u64>, Option<Version>);
type BlockTemplateResult = Result<BlockTemplate, SharedError>;

const BLOCK_ASSEMBLER_SUBSCRIBER: &str = "block_assembler";

pub struct BlockAssembler<CI> {
    shared: Shared<CI>,
    tx_pool: TransactionPoolController,
    candidate_uncles: LruCache<H256, Arc<Block>>,
    type_hash: H256,
    work_id: AtomicUsize,
}

#[derive(Clone)]
pub struct BlockAssemblerController {
    get_block_template_sender: Sender<Request<BlockTemplateParams, BlockTemplateResult>>,
}

pub struct BlockAssemblerReceivers {
    get_block_template_receiver: Receiver<Request<BlockTemplateParams, BlockTemplateResult>>,
}

impl BlockAssemblerController {
    pub fn build() -> (BlockAssemblerController, BlockAssemblerReceivers) {
        let (get_block_template_sender, get_block_template_receiver) =
            crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);
        (
            BlockAssemblerController {
                get_block_template_sender,
            },
            BlockAssemblerReceivers {
                get_block_template_receiver,
            },
        )
    }

    pub fn get_block_template(
        &self,
        cycles_limit: Option<Cycle>,
        bytes_limit: Option<u64>,
        max_version: Option<Version>,
    ) -> BlockTemplateResult {
        Request::call(
            &self.get_block_template_sender,
            (cycles_limit, bytes_limit, max_version),
        )
        .expect("get_block_template() failed")
    }
}

impl<CI: ChainIndex + 'static> BlockAssembler<CI> {
    pub fn new(shared: Shared<CI>, tx_pool: TransactionPoolController, type_hash: H256) -> Self {
        Self {
            shared,
            tx_pool,
            type_hash,
            candidate_uncles: LruCache::new(MAX_CANDIDATE_UNCLES),
            work_id: AtomicUsize::new(0),
        }
    }

    pub fn start<S: ToString>(
        mut self,
        thread_name: Option<S>,
        receivers: BlockAssemblerReceivers,
        notify: &NotifyController,
    ) -> JoinHandle<()> {
        let mut thread_builder = thread::Builder::new();
        // Mainly for test: give a empty thread_name
        if let Some(name) = thread_name {
            thread_builder = thread_builder.name(name.to_string());
        }

        let new_uncle_receiver = notify.subscribe_new_uncle(BLOCK_ASSEMBLER_SUBSCRIBER);
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
                        Ok(Request { responder, arguments: (cycles_limit, bytes_limit, max_version) }) => {
                            let _ = responder.send(self.get_block_template(cycles_limit, bytes_limit, max_version));
                        },
                        _ => {
                            error!(target: "miner", "get_block_template_receiver closed");
                            break;
                        },
                    }
                }
            }).expect("Start MinerAgent failed")
    }

    fn transform_params(
        &self,
        cycles_limit: Option<Cycle>,
        bytes_limit: Option<u64>,
        max_version: Option<Version>,
    ) -> (Cycle, u64, Version) {
        let consensus = self.shared.consensus();
        let cycles_limit = cycles_limit
            .min(Some(consensus.max_block_cycles()))
            .unwrap_or_else(|| consensus.max_block_cycles());
        let bytes_limit = bytes_limit
            .min(Some(consensus.max_block_bytes()))
            .unwrap_or_else(|| consensus.max_block_bytes());
        let version = max_version
            .min(Some(consensus.block_version()))
            .unwrap_or_else(|| consensus.block_version());

        (cycles_limit, bytes_limit, version)
    }

    fn transform_uncle(uncle: UncleBlock) -> UncleTemplate {
        let UncleBlock {
            header,
            cellbase,
            proposal_transactions,
        } = uncle;

        UncleTemplate {
            hash: header.hash(),
            required: false, //
            cellbase: Self::transform_cellbase(&cellbase, None),
            proposal_transactions: proposal_transactions.into_iter().map(Into::into).collect(),
            header: (&header).into(),
        }
    }

    fn transform_cellbase(tx: &Transaction, cycles: Option<Cycle>) -> CellbaseTemplate {
        CellbaseTemplate {
            hash: tx.hash(),
            cycles,
            data: tx.into(),
        }
    }

    fn transform_tx(
        tx: &Transaction,
        required: bool,
        cycles: Option<Cycle>,
        depends: Option<Vec<u32>>,
    ) -> TransactionTemplate {
        TransactionTemplate {
            hash: tx.hash(),
            required,
            cycles,
            depends,
            data: tx.into(),
        }
    }

    fn get_block_template(
        &mut self,
        cycles_limit: Option<Cycle>,
        bytes_limit: Option<u64>,
        max_version: Option<Version>,
    ) -> Result<BlockTemplate, SharedError> {
        let (cycles_limit, bytes_limit, version) =
            self.transform_params(cycles_limit, bytes_limit, max_version);
        let uncles_count_limit = self.shared.consensus().max_uncles_len() as u32;

        let tip_header = self.shared.tip_header().read();
        let header = tip_header.inner();
        let number = tip_header.number() + 1;
        let current_time = cmp::max(unix_time_as_millis(), header.timestamp() + 1);
        let difficulty = self
            .shared
            .calculate_difficulty(header)
            .expect("get difficulty");

        let (proposal_transactions, commit_transactions) =
            self.tx_pool.get_proposal_commit_transactions(10000, 10000);

        let (uncles, bad_uncles) = self.prepare_uncles(&header);
        if !bad_uncles.is_empty() {
            for bad in bad_uncles {
                self.candidate_uncles.remove(&bad);
            }
        }

        // dummy cellbase
        let cellbase =
            self.create_cellbase_transaction(header, &commit_transactions, self.type_hash.clone())?;

        let template = BlockTemplate {
            version,
            difficulty,
            current_time,
            number,
            parent_hash: tip_header.hash(),
            cycles_limit,
            bytes_limit,
            uncles_count_limit,
            uncles: uncles.into_iter().map(Self::transform_uncle).collect(),
            commit_transactions: commit_transactions
                .iter()
                .map(|tx| Self::transform_tx(tx, false, None, None))
                .collect(),
            proposal_transactions: proposal_transactions.into_iter().map(Into::into).collect(),
            cellbase: Self::transform_cellbase(&cellbase, None),
            work_id: format!("{}", self.work_id.fetch_add(1, Ordering::SeqCst)),
        };

        Ok(template)
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

    fn prepare_uncles(&self, tip: &Header) -> (Vec<UncleBlock>, Vec<H256>) {
        let max_uncles_age = self.shared.consensus().max_uncles_age();
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
        let mut block_hash = tip.hash().clone();
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
            tip.number() / self.shared.consensus().difficulty_adjustment_interval();

        let max_uncles_len = self.shared.consensus().max_uncles_len();
        let mut included = FnvHashSet::default();
        let mut uncles = Vec::with_capacity(max_uncles_len);
        let mut bad_uncles = Vec::new();
        let current_number = tip.number() + 1;
        for (hash, block) in self.candidate_uncles.iter() {
            if uncles.len() == max_uncles_len {
                break;
            }

            let block_difficulty_epoch =
                block.header().number() / self.shared.consensus().difficulty_adjustment_interval();

            // uncle must be same difficulty epoch with tip
            if block.header().difficulty() != tip.difficulty()
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
        (uncles, bad_uncles)
    }
}
