use crate::config::BlockAssemblerConfig;
use crate::error::Error;
use ckb_core::block::Block;
use ckb_core::cell::{resolve_transaction, OverlayCellProvider, TransactionsProvider};
use ckb_core::extras::EpochExt;
use ckb_core::header::Header;
use ckb_core::script::Script;
use ckb_core::service::{Request, DEFAULT_CHANNEL_SIZE, SIGNAL_CHANNEL_SIZE};
use ckb_core::transaction::{
    CellInput, CellOutput, ProposalShortId, Transaction, TransactionBuilder,
};
use ckb_core::uncle::UncleBlock;
use ckb_core::{BlockNumber, Bytes, Capacity, Cycle, Version};
use ckb_dao::DaoCalculator;
use ckb_jsonrpc_types::{
    BlockNumber as JsonBlockNumber, BlockTemplate, CellbaseTemplate, Cycle as JsonCycle,
    EpochNumber as JsonEpochNumber, JsonBytes, Timestamp as JsonTimestamp, TransactionTemplate,
    UncleTemplate, Unsigned, Version as JsonVersion,
};
use ckb_logger::{error, info};
use ckb_notify::NotifyController;
use ckb_shared::{shared::Shared, tx_pool::ProposedEntry};
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_store::ChainStore;
use ckb_traits::ChainProvider;
use ckb_verification::TransactionError;
use crossbeam_channel::{self, select, Receiver, Sender};
use failure::Error as FailureError;
use faketime::unix_time_as_millis;
use fnv::FnvHashSet;
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use std::cmp;
use std::sync::{atomic::AtomicU64, atomic::AtomicUsize, atomic::Ordering, Arc};
use std::thread;

const MAX_CANDIDATE_UNCLES: usize = 42;
type BlockTemplateParams = (Option<u64>, Option<u64>, Option<Version>);
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
    fn is_outdate(&self, current_time: u64) -> bool {
        current_time.saturating_sub(self.time) > BLOCK_TEMPLATE_TIMEOUT
    }

    fn is_modified(&self, last_uncles_updated_at: u64, last_txs_updated_at: u64) -> bool {
        last_uncles_updated_at != self.uncles_updated_at
            || last_txs_updated_at != self.txs_updated_at
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
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<Version>,
    ) -> BlockTemplateResult {
        Request::call(
            &self.get_block_template_sender,
            (bytes_limit, proposals_limit, max_version),
        )
        .expect("get_block_template() failed")
    }
}

pub struct BlockAssembler<CS> {
    shared: Shared<CS>,
    config: BlockAssemblerConfig,
    work_id: AtomicUsize,
    last_uncles_updated_at: AtomicU64,
    template_caches: LruCache<(H256, Cycle, u64, Version), TemplateCache>,
    proof_size: usize,
}

impl<CS: ChainStore + 'static> BlockAssembler<CS> {
    pub fn new(shared: Shared<CS>, config: BlockAssemblerConfig) -> Self {
        Self {
            proof_size: shared.consensus().pow_engine().proof_size(),
            shared,
            config,
            work_id: AtomicUsize::new(0),
            last_uncles_updated_at: AtomicU64::new(0),
            template_caches: LruCache::new(TEMPLATE_CACHE_SIZE),
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
            .spawn(move || {
                let mut candidate_uncles = LruCache::new(MAX_CANDIDATE_UNCLES);
                loop {
                    select! {
                        recv(signal_receiver) -> _ => {
                            break;
                        }
                        recv(new_uncle_receiver) -> msg => match msg {
                            Ok(uncle_block) => {
                                let hash = uncle_block.header().hash();
                                candidate_uncles.insert(hash.to_owned(), uncle_block);
                                self.last_uncles_updated_at
                                    .store(unix_time_as_millis(), Ordering::SeqCst);
                            }
                            _ => {
                                error!("new_uncle_receiver closed");
                                break;
                            }
                        },
                        recv(receivers.get_block_template_receiver) -> msg => match msg {
                            Ok(Request { responder, arguments: (bytes_limit, proposals_limit,  max_version) }) => {
                                let _ = responder.send(
                                    self.get_block_template(
                                        bytes_limit,
                                        proposals_limit,
                                        max_version,
                                        &mut candidate_uncles
                                    )
                                );
                            },
                            _ => {
                                error!("get_block_template_receiver closed");
                                break;
                            },
                        }
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
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<Version>,
    ) -> (u64, u64, Version) {
        let consensus = self.shared.consensus();
        let bytes_limit = bytes_limit
            .min(Some(consensus.max_block_bytes()))
            .unwrap_or_else(|| consensus.max_block_bytes());
        let proposals_limit = proposals_limit
            .min(Some(consensus.max_block_proposals_limit()))
            .unwrap_or_else(|| consensus.max_block_proposals_limit());
        let version = max_version
            .min(Some(consensus.block_version()))
            .unwrap_or_else(|| consensus.block_version());

        (bytes_limit, proposals_limit, version)
    }

    fn transform_uncle(uncle: UncleBlock) -> UncleTemplate {
        let UncleBlock { header, proposals } = uncle;

        UncleTemplate {
            hash: header.hash().to_owned(),
            required: false,
            proposals: proposals.into_iter().map(Into::into).collect(),
            header: (&header).into(),
        }
    }

    fn transform_cellbase(tx: &Transaction, cycles: Option<Cycle>) -> CellbaseTemplate {
        CellbaseTemplate {
            hash: tx.hash().to_owned(),
            cycles: cycles.map(JsonCycle),
            data: tx.into(),
        }
    }

    fn transform_tx(
        tx: &ProposedEntry,
        required: bool,
        depends: Option<Vec<u32>>,
    ) -> TransactionTemplate {
        TransactionTemplate {
            hash: tx.transaction.hash().to_owned(),
            required,
            cycles: Some(JsonCycle(tx.cycles)),
            depends: depends.map(|deps| deps.into_iter().map(|x| Unsigned(u64::from(x))).collect()),
            data: (&tx.transaction).into(),
        }
    }

    fn calculate_txs_size_limit(
        &self,
        cellbase_size: usize,
        bytes_limit: u64,
        uncles: &FnvHashSet<UncleBlock>,
        proposals: &FnvHashSet<ProposalShortId>,
    ) -> Result<usize, FailureError> {
        let occupied = Header::serialized_size(self.proof_size)
            + uncles
                .iter()
                .map(|u| u.serialized_size(self.proof_size))
                .sum::<usize>()
            + proposals.len() * ProposalShortId::serialized_size()
            + cellbase_size;
        let bytes_limit = bytes_limit as usize;

        bytes_limit
            .checked_sub(occupied)
            .ok_or_else(|| Error::InvalidParams(format!("bytes_limit {}", bytes_limit)).into())
    }

    fn get_block_template(
        &mut self,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<Version>,
        candidate_uncles: &mut LruCache<H256, Arc<Block>>,
    ) -> Result<BlockTemplate, FailureError> {
        let cycles_limit = self.shared.consensus().max_block_cycles();
        let (bytes_limit, proposals_limit, version) =
            self.transform_params(bytes_limit, proposals_limit, max_version);
        let uncles_count_limit = self.shared.consensus().max_uncles_num() as u32;

        let last_uncles_updated_at = self.last_uncles_updated_at.load(Ordering::SeqCst);

        // try get cache
        // this attempt will not touch chain_state lock which mean it should be fast
        let store = self.shared.store();
        let tip_header = store.get_tip_header().to_owned().expect("get tip header");
        let current_time = cmp::max(unix_time_as_millis(), tip_header.timestamp() + 1);
        if let Some(template_cache) = self.template_caches.get(&(
            tip_header.hash().to_owned(),
            cycles_limit,
            bytes_limit,
            version,
        )) {
            // check template cache outdate time
            if !template_cache.is_outdate(current_time) {
                return Ok(template_cache.template.clone());
            }
        }

        // lock chain_store to make sure data consistency
        let chain_state = self.shared.lock_chain_state();
        // refetch tip header, tip may changed after we get the lock
        let tip_header = chain_state.tip_header().to_owned();
        let tip_hash = tip_header.hash();
        let candidate_number = tip_header.number() + 1;
        // check cache again, return cache if we have no modify
        if let Some(template_cache) =
            self.template_caches
                .get(&(tip_hash.to_owned(), cycles_limit, bytes_limit, version))
        {
            let last_txs_updated_at = chain_state.get_last_txs_updated_at();
            // check our tx_pool wether is modified
            // we can reuse cache if it is not modidied
            if !template_cache.is_modified(last_uncles_updated_at, last_txs_updated_at) {
                return Ok(template_cache.template.clone());
            }
        }

        let last_epoch = store.get_current_epoch_ext().expect("current epoch ext");
        let next_epoch_ext = self.shared.next_epoch_ext(&last_epoch, &tip_header);
        let current_epoch = next_epoch_ext.unwrap_or(last_epoch);
        let uncles = self.prepare_uncles(candidate_number, &current_epoch, candidate_uncles);

        let cellbase_lock_args = self
            .config
            .args
            .iter()
            .cloned()
            .map(JsonBytes::into_bytes)
            .collect();

        let cellbase_lock = Script::new(cellbase_lock_args, self.config.code_hash.clone());

        let (cellbase, cellbase_size) = self.build_cellbase(&tip_header, cellbase_lock)?;

        let last_txs_updated_at = chain_state.get_last_txs_updated_at();
        let proposals = chain_state.get_proposals(proposals_limit as usize);
        let txs_size_limit =
            self.calculate_txs_size_limit(cellbase_size, bytes_limit, &uncles, &proposals)?;

        let (entries, size, cycles) = chain_state.get_proposed_txs(txs_size_limit, cycles_limit);
        if !entries.is_empty() {
            info!(
                "[get_block_template] candidate txs count: {}, size: {}/{}, cycles:{}/{}",
                entries.len(),
                size,
                txs_size_limit,
                cycles,
                cycles_limit
            );
        }

        // Generate DAO fields here
        let mut txs = vec![cellbase.to_owned()];
        for entry in &entries {
            txs.push(entry.transaction.to_owned());
        }
        let mut seen_inputs = FnvHashSet::default();
        let transactions_provider = TransactionsProvider::new(&txs);
        let overlay_cell_provider = OverlayCellProvider::new(&transactions_provider, &*chain_state);

        let rtxs = txs
            .iter()
            .try_fold(vec![], |mut rtxs, tx| {
                match resolve_transaction(
                    &tx,
                    &mut seen_inputs,
                    &overlay_cell_provider,
                    &*chain_state,
                ) {
                    Ok(rtx) => {
                        rtxs.push(rtx);
                        Ok(rtxs)
                    }
                    Err(e) => Err(e),
                }
            })
            .map_err(|_| Error::InvalidInput)?;
        let dao = DaoCalculator::new(&chain_state.consensus(), Arc::clone(chain_state.store()))
            .dao_field(&rtxs, &tip_header)?;

        // Release the lock as soon as possible, let other services do their work
        drop(chain_state);

        // Should recalculate current time after create cellbase (create cellbase may spend a lot of time)
        let current_time = cmp::max(unix_time_as_millis(), tip_header.timestamp() + 1);
        let template = BlockTemplate {
            version: JsonVersion(version),
            difficulty: current_epoch.difficulty().clone(),
            current_time: JsonTimestamp(current_time),
            number: JsonBlockNumber(candidate_number),
            epoch: JsonEpochNumber(current_epoch.number()),
            parent_hash: tip_hash.to_owned(),
            cycles_limit: JsonCycle(cycles_limit),
            bytes_limit: Unsigned(bytes_limit),
            uncles_count_limit: Unsigned(uncles_count_limit.into()),
            uncles: uncles.into_iter().map(Self::transform_uncle).collect(),
            transactions: entries
                .iter()
                .map(|entry| Self::transform_tx(entry, false, None))
                .collect(),
            proposals: proposals.into_iter().map(Into::into).collect(),
            cellbase: Self::transform_cellbase(&cellbase, None),
            work_id: Unsigned(self.work_id.fetch_add(1, Ordering::SeqCst) as u64),
            dao: JsonBytes::from_bytes(dao),
        };

        self.template_caches.insert(
            (tip_hash.to_owned(), cycles_limit, bytes_limit, version),
            TemplateCache {
                time: current_time,
                uncles_updated_at: last_uncles_updated_at,
                txs_updated_at: last_txs_updated_at,
                template: template.clone(),
            },
        );

        Ok(template)
    }

    /// Miner mined block H(c), the block reward will be finalized at H(c + w_far + 1).
    /// Miner specify own lock in cellbase witness.
    /// The cellbase have only one output,
    /// miner should collect the block reward for finalize target H(max(0, c - w_far - 1))
    fn build_cellbase(
        &self,
        tip: &Header,
        lock: Script,
    ) -> Result<(Transaction, usize), FailureError> {
        let candidate_number = tip.number() + 1;

        let tx = {
            let (target_lock, block_reward) = self.shared.finalize_block_reward(tip)?;
            let witness = lock.into_witness();
            let input = CellInput::new_cellbase_input(candidate_number);
            let raw_output = CellOutput::new(block_reward, Bytes::default(), target_lock, None);
            let output = self.custom_output(block_reward, raw_output)?;

            TransactionBuilder::default()
                .input(input)
                .output(output)
                .witness(witness)
                .build()
        };
        let serialized_size = tx.serialized_size();

        Ok((tx, serialized_size))
    }

    fn custom_output(
        &self,
        reward: Capacity,
        mut output: CellOutput,
    ) -> Result<CellOutput, FailureError> {
        let occupied_capacity = output.occupied_capacity()?;

        if reward < occupied_capacity {
            return Err(TransactionError::InsufficientCellCapacity.into());
        }

        let mut data = self.config.data.clone().into_bytes();

        if !data.is_empty() {
            let data_max_len = (reward.as_u64() - occupied_capacity.as_u64()) as usize;

            // User-defined data has a risk of exceeding capacity
            // just truncate it
            if data.len() > data_max_len {
                data.truncate(data_max_len);
            }

            output.data = data;
        }

        Ok(output)
    }

    /// A block B1 is considered to be the uncle of
    /// another block B2 if all of the following conditions are met:
    /// (1) they are in the same epoch, sharing the same difficulty;
    /// (2) height(B2) > height(B1);
    /// (3) B2 is the first block in its chain to refer to B1
    fn prepare_uncles(
        &self,
        candidate_number: BlockNumber,
        current_epoch_ext: &EpochExt,
        candidate_uncles: &mut LruCache<H256, Arc<Block>>,
    ) -> FnvHashSet<UncleBlock> {
        let store = self.shared.store();
        let epoch_number = current_epoch_ext.number();
        let max_uncles_num = self.shared.consensus().max_uncles_num();
        let mut uncles = FnvHashSet::with_capacity_and_hasher(max_uncles_num, Default::default());

        for entry in candidate_uncles.entries() {
            if uncles.len() == max_uncles_num {
                break;
            }
            let block = entry.get();
            let hash = entry.key();
            if block.header().difficulty() != current_epoch_ext.difficulty()
                || block.header().epoch() != epoch_number
                || store.get_block_number(hash).is_some()
                || store.is_uncle(hash)
                || block.header().number() >= candidate_number
            {
                entry.remove();
            } else {
                let uncle = UncleBlock {
                    header: block.header().to_owned(),
                    proposals: block.proposals().to_vec(),
                };
                uncles.insert(uncle);
            }
        }
        uncles
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{block_assembler::BlockAssembler, config::BlockAssemblerConfig};
    use ckb_chain::chain::ChainController;
    use ckb_chain::chain::ChainService;
    use ckb_chain_spec::consensus::Consensus;
    use ckb_core::block::Block;
    use ckb_core::block::BlockBuilder;
    use ckb_core::extras::EpochExt;
    use ckb_core::header::{Header, HeaderBuilder};
    use ckb_core::script::Script;
    use ckb_core::transaction::{
        CellInput, CellOutput, ProposalShortId, Transaction, TransactionBuilder,
    };
    use ckb_core::{BlockNumber, Bytes};
    use ckb_dao_utils::genesis_dao_data;
    use ckb_db::memorydb::MemoryKeyValueDB;
    use ckb_notify::{NotifyController, NotifyService};
    use ckb_pow::Pow;
    use ckb_shared::shared::Shared;
    use ckb_shared::shared::SharedBuilder;
    use ckb_store::{ChainKVStore, ChainStore};
    use ckb_traits::ChainProvider;
    use ckb_verification::{BlockVerifier, HeaderResolverWrapper, HeaderVerifier, Verifier};
    use numext_fixed_hash::H256;
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
        let shared = builder.build().unwrap();

        let notify = notify.unwrap_or_else(|| NotifyService::default().start::<&str>(None));
        let chain_service = ChainService::new(shared.clone(), notify.clone());
        let chain_controller = chain_service.start::<&str>(None);
        (chain_controller, shared, notify)
    }

    fn setup_block_assembler<CS: ChainStore + 'static>(
        shared: Shared<CS>,
        config: BlockAssemblerConfig,
    ) -> BlockAssembler<CS> {
        BlockAssembler::new(shared, config)
    }

    #[test]
    fn test_get_block_template() {
        let (_chain_controller, shared, _notify) = start_chain(None, None);
        let config = BlockAssemblerConfig {
            code_hash: H256::zero(),
            args: vec![],
            data: JsonBytes::default(),
        };
        let mut block_assembler = setup_block_assembler(shared.clone(), config);
        let mut candidate_uncles = LruCache::new(MAX_CANDIDATE_UNCLES);

        let block_template = block_assembler
            .get_block_template(None, None, None, &mut candidate_uncles)
            .unwrap();

        let block: BlockBuilder = block_template.into();
        let block = block.build();

        let resolver = HeaderResolverWrapper::new(block.header(), shared.clone());
        let header_verify_result = {
            let chain_state = shared.lock_chain_state();
            let header_verifier = HeaderVerifier::new(&*chain_state, Pow::Dummy.engine());
            header_verifier.verify(&resolver)
        };
        assert!(header_verify_result.is_ok());

        let block_verify = BlockVerifier::new(shared.clone());
        assert!(block_verify.verify(&block).is_ok());
    }

    fn gen_block(parent_header: &Header, nonce: u64, epoch: &EpochExt) -> Block {
        let number = parent_header.number() + 1;
        let cellbase = create_cellbase(number, epoch);
        // This just make sure we can generate a valid block template,
        // the actual DAO validation logic will be ensured in other
        // tests
        let dao = genesis_dao_data(&cellbase).unwrap();
        let header = HeaderBuilder::default()
            .parent_hash(parent_header.hash().to_owned())
            .timestamp(parent_header.timestamp() + 10)
            .number(number)
            .epoch(epoch.number())
            .difficulty(epoch.difficulty().clone())
            .nonce(nonce)
            .dao(dao)
            .build();

        unsafe {
            BlockBuilder::default()
                .header(header)
                .transaction(cellbase)
                .proposal(ProposalShortId::from_slice(&[1; 10]).unwrap())
                .build_unchecked()
        }
    }

    fn create_cellbase(number: BlockNumber, epoch: &EpochExt) -> Transaction {
        TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(number))
            .output(CellOutput::new(
                epoch.block_reward(number).unwrap(),
                Bytes::new(),
                Script::default(),
                None,
            ))
            .build()
    }

    #[test]
    fn test_prepare_uncles() {
        let mut consensus = Consensus::default();
        consensus.genesis_epoch_ext.set_length(5);
        let epoch = consensus.genesis_epoch_ext().clone();

        let (chain_controller, shared, notify) = start_chain(Some(consensus), None);
        let config = BlockAssemblerConfig {
            code_hash: H256::zero(),
            args: vec![],
            data: JsonBytes::default(),
        };
        let block_assembler = setup_block_assembler(shared.clone(), config);
        let new_uncle_receiver = notify.subscribe_new_uncle("test_prepare_uncles");
        let block_assembler_controller = block_assembler.start(Some("test"), &notify.clone());

        let genesis = shared
            .store()
            .get_block_header(&shared.store().get_block_hash(0).unwrap())
            .unwrap();

        let block0_0 = gen_block(&genesis, 11, &epoch);
        let block0_1 = gen_block(&genesis, 10, &epoch);
        let (block0_0, block0_1) = if block0_0.header().hash() < block0_1.header().hash() {
            (block0_1, block0_0)
        } else {
            (block0_0, block0_1)
        };

        let last_epoch = epoch.clone();
        let epoch = shared
            .next_epoch_ext(&last_epoch, block0_1.header())
            .unwrap_or(last_epoch);

        let block1_1 = gen_block(block0_1.header(), 10, &epoch);

        chain_controller
            .process_block(Arc::new(block0_1.clone()), false)
            .unwrap();
        chain_controller
            .process_block(Arc::new(block0_0.clone()), false)
            .unwrap();
        chain_controller
            .process_block(Arc::new(block1_1.clone()), false)
            .unwrap();

        // block number 3, epoch 0
        let _ = new_uncle_receiver.recv();
        let block_template = block_assembler_controller
            .get_block_template(None, None, None)
            .unwrap();
        assert_eq!(&block_template.uncles[0].hash, block0_0.header().hash());

        let last_epoch = epoch.clone();
        let epoch = shared
            .next_epoch_ext(&last_epoch, block1_1.header())
            .unwrap_or(last_epoch);

        let block2_1 = gen_block(block1_1.header(), 10, &epoch);
        chain_controller
            .process_block(Arc::new(block2_1.clone()), false)
            .unwrap();

        let block_template = block_assembler_controller
            .get_block_template(None, None, None)
            .unwrap();
        // block number 4, epoch 0, uncles should retained
        assert_eq!(&block_template.uncles[0].hash, block0_0.header().hash());

        let last_epoch = epoch.clone();
        let epoch = shared
            .next_epoch_ext(&last_epoch, block2_1.header())
            .unwrap_or(last_epoch);

        let block3_1 = gen_block(block2_1.header(), 10, &epoch);
        chain_controller
            .process_block(Arc::new(block3_1.clone()), false)
            .unwrap();

        let block_template = block_assembler_controller
            .get_block_template(None, None, None)
            .unwrap();
        // block number 5, epoch 1, block_template should not include last epoch uncles
        assert!(block_template.uncles.is_empty());
    }
}
