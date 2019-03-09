use ckb_core::block::Block;
use ckb_core::header::Header;
use ckb_core::service::{Request, DEFAULT_CHANNEL_SIZE, SIGNAL_CHANNEL_SIZE};
use ckb_core::transaction::{CellInput, CellOutput, Transaction, TransactionBuilder};
use ckb_core::uncle::UncleBlock;
use ckb_core::BlockNumber;
use ckb_core::{Cycle, Version};
use ckb_notify::NotifyController;
use ckb_shared::{index::ChainIndex, shared::Shared, tx_pool::PoolEntry};
use ckb_traits::ChainProvider;
use ckb_util::Mutex;
use crossbeam_channel::{self, select, Receiver, Sender};
use failure::Error as FailureError;
use faketime::unix_time_as_millis;
use fnv::FnvHashSet;
use jsonrpc_types::{BlockTemplate, CellbaseTemplate, TransactionTemplate, UncleTemplate};
use log::error;
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::cmp;
use std::sync::{atomic::AtomicUsize, atomic::Ordering, Arc};
use std::thread;
use stop_handler::{SignalSender, StopHandler};

const MAX_CANDIDATE_UNCLES: usize = 42;
type BlockTemplateParams = (Option<Cycle>, Option<u64>, Option<Version>);
type BlockTemplateResult = Result<BlockTemplate, FailureError>;
const BLOCK_ASSEMBLER_SUBSCRIBER: &str = "block_assembler";
const BLOCK_TEMPLATE_TIMEOUT: u64 = 3000;
const TEMPLATE_CACHE_SIZE: usize = 10;

struct TemplateCache {
    pub time: u64,
    pub uncles_updated_at: u64,
    pub txs_updated_at: u64,
    pub template: BlockTemplate,
}

impl TemplateCache {
    fn is_outdate(
        &self,
        last_uncles_updated_at: u64,
        last_txs_updated_at: u64,
        current_time: u64,
        number: BlockNumber,
    ) -> bool {
        last_uncles_updated_at != self.uncles_updated_at
            || last_txs_updated_at != self.txs_updated_at
            || number != self.template.number
            || current_time.saturating_sub(self.time) > BLOCK_TEMPLATE_TIMEOUT
    }
}

#[derive(Clone)]
pub struct BlockAssemblerController {
    get_block_template_sender: Sender<Request<BlockTemplateParams, BlockTemplateResult>>,
    stop: StopHandler<()>,
}

impl Drop for BlockAssemblerController {
    fn drop(&mut self) {
        self.stop.try_send();
    }
}

struct BlockAssemblerReceivers {
    get_block_template_receiver: Receiver<Request<BlockTemplateParams, BlockTemplateResult>>,
}

impl BlockAssemblerController {
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

pub struct BlockAssembler<CI> {
    shared: Shared<CI>,
    candidate_uncles: LruCache<H256, Arc<Block>>,
    type_hash: H256,
    work_id: AtomicUsize,
    last_uncles_updated_at: AtomicUsize,
    template_caches: Mutex<LruCache<(Cycle, u64, Version), TemplateCache>>,
}

impl<CI: ChainIndex + 'static> BlockAssembler<CI> {
    pub fn new(shared: Shared<CI>, type_hash: H256) -> Self {
        Self {
            shared,
            type_hash,
            candidate_uncles: LruCache::new(MAX_CANDIDATE_UNCLES),
            work_id: AtomicUsize::new(0),
            last_uncles_updated_at: AtomicUsize::new(0),
            template_caches: Mutex::new(LruCache::new(TEMPLATE_CACHE_SIZE)),
        }
    }

    pub fn start<S: ToString>(
        mut self,
        thread_name: Option<S>,
        notify: &NotifyController,
    ) -> BlockAssemblerController {
        let (signal_sender, signal_receiver) =
            crossbeam_channel::bounded::<()>(SIGNAL_CHANNEL_SIZE);
        let (get_block_template_sender, get_block_template_receiver) =
            crossbeam_channel::bounded(DEFAULT_CHANNEL_SIZE);

        let mut thread_builder = thread::Builder::new();
        // Mainly for test: give a empty thread_name
        if let Some(name) = thread_name {
            thread_builder = thread_builder.name(name.to_string());
        }

        let receivers = BlockAssemblerReceivers {
            get_block_template_receiver,
        };

        let new_uncle_receiver = notify.subscribe_new_uncle(BLOCK_ASSEMBLER_SUBSCRIBER);
        let thread = thread_builder
            .spawn(move || loop {
                select! {
                    recv(signal_receiver) -> _ => {
                        break;
                    }
                    recv(new_uncle_receiver) -> msg => match msg {
                        Ok(uncle_block) => {
                            let hash = uncle_block.header().hash().clone();
                            self.candidate_uncles.insert(hash, uncle_block);
                            self.last_uncles_updated_at
                                .store(unix_time_as_millis() as usize, Ordering::SeqCst);
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
            }).expect("Start MinerAgent failed");
        let stop = StopHandler::new(SignalSender::Crossbeam(signal_sender), thread);

        BlockAssemblerController {
            get_block_template_sender,
            stop,
        }
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
        tx: &PoolEntry,
        required: bool,
        depends: Option<Vec<u32>>,
    ) -> TransactionTemplate {
        TransactionTemplate {
            hash: tx.transaction.hash(),
            required,
            cycles: tx.cycles,
            depends,
            data: (&tx.transaction).into(),
        }
    }

    fn get_block_template(
        &mut self,
        cycles_limit: Option<Cycle>,
        bytes_limit: Option<u64>,
        max_version: Option<Version>,
    ) -> Result<BlockTemplate, FailureError> {
        let (cycles_limit, bytes_limit, version) =
            self.transform_params(cycles_limit, bytes_limit, max_version);
        let uncles_count_limit = self.shared.consensus().max_uncles_num() as u32;

        let last_uncles_updated_at = self.last_uncles_updated_at.load(Ordering::SeqCst) as u64;
        let chain_state = self.shared.chain_state().lock();
        let last_txs_updated_at = chain_state.get_last_txs_updated_at();

        let header = chain_state.tip_header();
        let number = chain_state.tip_number() + 1;
        let current_time = cmp::max(unix_time_as_millis(), header.timestamp() + 1);

        let mut template_caches = self.template_caches.lock();

        if let Some(template_cache) = template_caches.get(&(cycles_limit, bytes_limit, version)) {
            if !template_cache.is_outdate(
                last_uncles_updated_at,
                last_txs_updated_at,
                current_time,
                number,
            ) {
                return Ok(template_cache.template.clone());
            }
        }

        let difficulty = self
            .shared
            .calculate_difficulty(header)
            .expect("get difficulty");

        let (proposal_transactions, commit_transactions) =
            chain_state.get_proposal_and_staging_txs(10000, 10000);

        let (uncles, bad_uncles) = self.prepare_uncles(&header, &difficulty);
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
            parent_hash: header.hash(),
            cycles_limit,
            bytes_limit,
            uncles_count_limit,
            uncles: uncles.into_iter().map(Self::transform_uncle).collect(),
            commit_transactions: commit_transactions
                .iter()
                .map(|tx| Self::transform_tx(tx, false, None))
                .collect(),
            proposal_transactions: proposal_transactions.into_iter().map(Into::into).collect(),
            cellbase: Self::transform_cellbase(&cellbase, None),
            work_id: format!("{}", self.work_id.fetch_add(1, Ordering::SeqCst)),
        };

        template_caches.insert(
            (cycles_limit, bytes_limit, version),
            TemplateCache {
                time: current_time,
                uncles_updated_at: last_uncles_updated_at,
                txs_updated_at: last_txs_updated_at,
                template: template.clone(),
            },
        );

        Ok(template)
    }

    fn create_cellbase_transaction(
        &self,
        header: &Header,
        pes: &[PoolEntry],
        type_hash: H256,
    ) -> Result<Transaction, FailureError> {
        // NOTE: To generate different cellbase txid, we put header number in the input script
        let input = CellInput::new_cellbase_input(header.number() + 1);
        // NOTE: We could've just used byteorder to serialize u64 and hex string into bytes,
        // but the truth is we will modify this after we designed lock script anyway, so let's
        // stick to the simpler way and just convert everything to a single string, then to UTF8
        // bytes, they really serve the same purpose at the moment
        let block_reward = self.shared.block_reward(header.number() + 1);
        let mut fee = 0;
        for pe in pes {
            fee += self.shared.calculate_transaction_fee(&pe.transaction)?;
        }

        let output = CellOutput::new(block_reward + fee, Vec::new(), type_hash, None);

        Ok(TransactionBuilder::default()
            .input(input)
            .output(output)
            .build())
    }

    fn prepare_uncles(
        &self,
        tip: &Header,
        current_difficulty: &U256,
    ) -> (Vec<UncleBlock>, Vec<H256>) {
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

        let current_number = tip.number() + 1;
        let current_difficulty_epoch =
            current_number / self.shared.consensus().difficulty_adjustment_interval();

        let max_uncles_num = self.shared.consensus().max_uncles_num();
        let mut included = FnvHashSet::default();
        let mut uncles = Vec::with_capacity(max_uncles_num);
        let mut bad_uncles = Vec::new();

        for (hash, block) in self.candidate_uncles.iter() {
            if uncles.len() == max_uncles_num {
                break;
            }

            let block_difficulty_epoch =
                block.header().number() / self.shared.consensus().difficulty_adjustment_interval();

            // uncle must be same difficulty epoch with candidate
            if block.header().difficulty() != current_difficulty
                || block_difficulty_epoch != current_difficulty_epoch
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

#[cfg(test)]
mod tests {
    use crate::block_assembler::BlockAssembler;
    use ckb_chain::chain::ChainBuilder;
    use ckb_chain::chain::ChainController;
    use ckb_chain_spec::consensus::Consensus;
    use ckb_core::block::Block;
    use ckb_core::block::BlockBuilder;
    use ckb_core::header::{Header, HeaderBuilder};
    use ckb_core::transaction::{
        CellInput, CellOutput, ProposalShortId, Transaction, TransactionBuilder,
    };
    use ckb_core::BlockNumber;
    use ckb_db::memorydb::MemoryKeyValueDB;
    use ckb_notify::{NotifyController, NotifyService};
    use ckb_pow::Pow;
    use ckb_shared::index::ChainIndex;
    use ckb_shared::shared::Shared;
    use ckb_shared::shared::SharedBuilder;
    use ckb_shared::store::ChainKVStore;
    use ckb_traits::ChainProvider;
    use ckb_verification::{BlockVerifier, HeaderResolverWrapper, HeaderVerifier, Verifier};
    use jsonrpc_types::{BlockTemplate, CellbaseTemplate};
    use numext_fixed_hash::H256;
    use numext_fixed_uint::U256;
    use std::sync::Arc;

    fn start_chain(
        consensus: Option<Consensus>,
        notify: Option<NotifyController>,
    ) -> (
        ChainController,
        Shared<ChainKVStore<MemoryKeyValueDB>>,
        NotifyController,
    ) {
        let mut builder = SharedBuilder::<MemoryKeyValueDB>::new();
        if let Some(consensus) = consensus {
            builder = builder.consensus(consensus);
        }
        let shared = builder.build();

        let notify = notify.unwrap_or_else(|| NotifyService::default().start::<&str>(None));
        let chain_service = ChainBuilder::new(shared.clone(), notify.clone())
            .verification(false)
            .build();
        let chain_controller = chain_service.start::<&str>(None);
        (chain_controller, shared, notify)
    }

    fn setup_block_assembler<CI: ChainIndex + 'static>(
        shared: Shared<CI>,
        type_hash: H256,
    ) -> BlockAssembler<CI> {
        BlockAssembler::new(shared, type_hash)
    }

    #[test]
    fn test_get_block_template() {
        let (_chain_controller, shared, _notify) = start_chain(None, None);
        let mut block_assembler = setup_block_assembler(shared.clone(), H256::zero());

        let block_template = block_assembler
            .get_block_template(None, None, None)
            .unwrap();

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
            ..
            // cycles_limit,
            // bytes_limit,
            // uncles_count_limit,
        } = block_template;

        let (cellbase_id, cellbase) = {
            let CellbaseTemplate { hash, data, .. } = cellbase;
            (hash, data)
        };

        let header_builder = HeaderBuilder::default()
            .version(version)
            .number(number)
            .difficulty(difficulty)
            .timestamp(current_time)
            .parent_hash(parent_hash)
            .cellbase_id(cellbase_id);

        let block = BlockBuilder::default()
            .uncles(uncles.into_iter().map(Into::into).collect())
            .commit_transaction(cellbase.into())
            .commit_transactions(commit_transactions.into_iter().map(Into::into).collect())
            .proposal_transactions(proposal_transactions.into_iter().map(Into::into).collect())
            .with_header_builder(header_builder);

        let resolver = HeaderResolverWrapper::new(block.header(), shared.clone());
        let header_verifier = HeaderVerifier::new(shared.clone(), Pow::Dummy.engine());
        assert!(header_verifier.verify(&resolver).is_ok());

        let block_verify = BlockVerifier::new(shared.clone());
        assert!(block_verify.verify(&block).is_ok());
    }

    fn gen_block(parent_header: &Header, nonce: u64, difficulty: U256) -> Block {
        let number = parent_header.number() + 1;
        let cellbase = create_cellbase(number);
        let header = HeaderBuilder::default()
            .parent_hash(parent_header.hash().clone())
            .timestamp(parent_header.timestamp() + 10)
            .number(number)
            .difficulty(difficulty)
            .nonce(nonce)
            .build();

        BlockBuilder::default()
            .header(header)
            .commit_transaction(cellbase)
            .proposal_transaction(ProposalShortId::from_slice(&[1; 10]).unwrap())
            .build()
    }

    fn create_cellbase(number: BlockNumber) -> Transaction {
        TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(number))
            .output(CellOutput::new(0, vec![], H256::zero(), None))
            .build()
    }

    #[test]
    fn test_prepare_uncles() {
        let mut consensus = Consensus::default();
        consensus.pow_time_span = 4;
        consensus.pow_spacing = 1;

        let (chain_controller, shared, notify) = start_chain(Some(consensus), None);
        let block_assembler = setup_block_assembler(shared.clone(), H256::zero());
        let new_uncle_receiver = notify.subscribe_new_uncle("test_prepare_uncles");
        let block_assembler_controller = block_assembler.start(Some("test"), &notify.clone());

        let genesis = shared.block_header(&shared.block_hash(0).unwrap()).unwrap();
        let block0_0 = gen_block(&genesis, 10, genesis.difficulty().clone());
        let block0_1 = gen_block(&genesis, 11, genesis.difficulty().clone());
        let block1_1 = gen_block(
            block0_1.header(),
            10,
            block0_1.header().difficulty().clone(),
        );

        chain_controller
            .process_block(Arc::new(block0_1.clone()))
            .unwrap();
        chain_controller
            .process_block(Arc::new(block0_0.clone()))
            .unwrap();
        chain_controller
            .process_block(Arc::new(block1_1.clone()))
            .unwrap();

        // block number 3, epoch 0
        let _ = new_uncle_receiver.recv();
        let block_template = block_assembler_controller
            .get_block_template(None, None, None)
            .unwrap();
        assert_eq!(block_template.uncles[0].hash, block0_0.header().hash());

        let block2_1 = gen_block(
            block1_1.header(),
            10,
            block1_1.header().difficulty().clone(),
        );
        chain_controller
            .process_block(Arc::new(block2_1.clone()))
            .unwrap();

        let block_template = block_assembler_controller
            .get_block_template(None, None, None)
            .unwrap();
        // block number 4, epoch 1, block_template should not include last epoch uncles
        assert!(block_template.uncles.is_empty());
    }
}
