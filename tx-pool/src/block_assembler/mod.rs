mod candidate_uncles;

use crate::block_assembler::candidate_uncles::CandidateUncles;
use crate::component::entry::TxEntry;
use crate::config::BlockAssemblerConfig;
use crate::error::BlockAssemblerError as Error;
use ckb_chain_spec::consensus::Consensus;
use ckb_jsonrpc_types::{
    BlockTemplate, CellbaseTemplate, Cycle as JsonCycle, TransactionTemplate, UncleTemplate,
    Unsigned,
};
use ckb_reward_calculator::RewardCalculator;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::{
    bytes::Bytes,
    core::{
        BlockNumber, Capacity, Cycle, EpochExt, HeaderView, TransactionBuilder, TransactionView,
        Version,
    },
    packed::{self, CellInput, CellOutput, ProposalShortId, Script, Transaction, UncleBlock},
    prelude::*,
    H256,
};
use ckb_verification::TransactionError;
use failure::Error as FailureError;
use lru_cache::LruCache;
use std::collections::HashSet;
use std::sync::{atomic::AtomicU64, atomic::AtomicUsize, atomic::Ordering, Arc};

const BLOCK_TEMPLATE_TIMEOUT: u64 = 3000;
const TEMPLATE_CACHE_SIZE: usize = 10;

pub(crate) struct TemplateCache {
    pub time: u64,
    pub uncles_updated_at: u64,
    pub txs_updated_at: u64,
    pub template: BlockTemplate,
}

impl TemplateCache {
    pub(crate) fn is_outdate(&self, current_time: u64) -> bool {
        current_time.saturating_sub(self.time) > BLOCK_TEMPLATE_TIMEOUT
    }

    pub(crate) fn is_modified(
        &self,
        last_uncles_updated_at: u64,
        last_txs_updated_at: u64,
    ) -> bool {
        last_uncles_updated_at != self.uncles_updated_at
            || last_txs_updated_at != self.txs_updated_at
    }
}

pub struct BlockAssembler {
    pub(crate) config: BlockAssemblerConfig,
    pub(crate) work_id: AtomicUsize,
    pub(crate) last_uncles_updated_at: AtomicU64,
    pub(crate) template_caches: LruCache<(H256, Cycle, u64, Version), TemplateCache>,
    pub(crate) candidate_uncles: CandidateUncles,
}

impl BlockAssembler {
    pub fn new(config: BlockAssemblerConfig) -> Self {
        Self {
            config,
            work_id: AtomicUsize::new(0),
            last_uncles_updated_at: AtomicU64::new(0),
            template_caches: LruCache::new(TEMPLATE_CACHE_SIZE),
            candidate_uncles: CandidateUncles::new(),
        }
    }

    pub(crate) fn transform_params(
        &self,
        consensus: &Consensus,
        bytes_limit: Option<u64>,
        proposals_limit: Option<u64>,
        max_version: Option<Version>,
    ) -> (u64, u64, Version) {
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

    pub(crate) fn load_last_uncles_updated_at(&self) -> u64 {
        self.last_uncles_updated_at.load(Ordering::SeqCst)
    }

    pub(crate) fn transform_uncle(uncle: UncleBlock) -> UncleTemplate {
        UncleTemplate {
            hash: uncle.calc_header_hash().unpack(),
            required: false,
            proposals: uncle.proposals().into_iter().map(Into::into).collect(),
            header: uncle.header().into(),
        }
    }

    pub(crate) fn transform_cellbase(
        tx: &TransactionView,
        cycles: Option<Cycle>,
    ) -> CellbaseTemplate {
        CellbaseTemplate {
            hash: tx.hash().unpack(),
            cycles: cycles.map(JsonCycle),
            data: tx.data().into(),
        }
    }

    pub(crate) fn transform_tx(
        tx: &TxEntry,
        required: bool,
        depends: Option<Vec<u32>>,
    ) -> TransactionTemplate {
        TransactionTemplate {
            hash: tx.transaction.hash().unpack(),
            required,
            cycles: Some(JsonCycle(tx.cycles)),
            depends: depends.map(|deps| deps.into_iter().map(|x| Unsigned(u64::from(x))).collect()),
            data: tx.transaction.data().into(),
        }
    }

    pub(crate) fn calculate_txs_size_limit(
        &self,
        bytes_limit: u64,
        cellbase: Transaction,
        uncles: &[UncleBlock],
        proposals: &HashSet<ProposalShortId>,
    ) -> Result<usize, FailureError> {
        let empty_dao = packed::Byte32::default();
        let raw_header = packed::RawHeader::new_builder().dao(empty_dao).build();
        let header = packed::Header::new_builder().raw(raw_header).build();
        let block = packed::Block::new_builder()
            .header(header)
            .transactions(vec![cellbase].pack())
            .uncles(uncles.to_owned().pack())
            .proposals(
                proposals
                    .iter()
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
                    .pack(),
            )
            .build();
        let occupied = block.as_slice().len();
        let bytes_limit = bytes_limit as usize;
        bytes_limit
            .checked_sub(occupied)
            .ok_or_else(|| Error::InvalidParams(format!("bytes_limit {}", bytes_limit)).into())
    }

    /// Miner mined block H(c), the block reward will be finalized at H(c + w_far + 1).
    /// Miner specify own lock in cellbase witness.
    /// The cellbase have only one output,
    /// miner should collect the block reward for finalize target H(max(0, c - w_far - 1))
    pub(crate) fn build_cellbase(
        &self,
        snapshot: &Snapshot,
        tip: &HeaderView,
        lock: Script,
    ) -> Result<TransactionView, FailureError> {
        let candidate_number = tip.number() + 1;

        let tx = {
            let (target_lock, block_reward) =
                RewardCalculator::new(snapshot.consensus(), snapshot).block_reward(tip)?;
            let witness = lock.into_witness();
            let input = CellInput::new_cellbase_input(candidate_number);
            let output = CellOutput::new_builder()
                .capacity(block_reward.total.pack())
                .lock(target_lock)
                .build();
            let output_data = self.build_output_data(block_reward.total, &output)?;

            TransactionBuilder::default()
                .input(input)
                .output(output)
                .output_data(output_data.pack())
                .witness(witness)
                .build()
        };

        Ok(tx)
    }

    pub(crate) fn build_output_data(
        &self,
        reward: Capacity,
        output: &CellOutput,
    ) -> Result<Bytes, FailureError> {
        let mut data = self.config.data.clone().into_bytes();
        let occupied_capacity = output.occupied_capacity(Capacity::bytes(data.len())?)?;

        if reward < occupied_capacity {
            return Err(TransactionError::InsufficientCellCapacity.into());
        }

        if !data.is_empty() {
            let data_max_len = (reward.as_u64() - occupied_capacity.as_u64()) as usize;

            // User-defined data has a risk of exceeding capacity
            // just truncate it
            if data.len() > data_max_len {
                data.truncate(data_max_len);
            }

            Ok(data)
        } else {
            Ok(data)
        }
    }

    // A block B1 is considered to be the uncle of another block B2 if all of the following conditions are met:
    // (1) they are in the same epoch, sharing the same difficulty;
    // (2) height(B2) > height(B1);
    // (3) B1's parent is either B2's ancestor or embedded in B2 or its ancestors as an uncle;
    // and (4) B2 is the first block in its chain to refer to B1.
    pub(crate) fn prepare_uncles(
        &mut self,
        snapshot: &Snapshot,
        candidate_number: BlockNumber,
        current_epoch_ext: &EpochExt,
    ) -> Vec<UncleBlock> {
        let epoch_number = current_epoch_ext.number();
        let max_uncles_num = snapshot.consensus().max_uncles_num();
        let mut uncles: Vec<UncleBlock> = Vec::with_capacity(max_uncles_num);
        let mut removed = Vec::new();

        for uncle in self.candidate_uncles.values() {
            if uncles.len() == max_uncles_num {
                break;
            }
            let uncle = uncle.as_ref().clone().into_view();
            let parent_hash = uncle.data().header().raw().parent_hash();
            if &uncle.difficulty() != current_epoch_ext.difficulty()
                || uncle.epoch() != epoch_number
                || snapshot.get_block_number(&uncle.hash()).is_some()
                || snapshot.is_uncle(&uncle.hash())
                || !(uncles.iter().any(|u| u.calc_header_hash() == parent_hash)
                    || snapshot.get_block_number(&parent_hash).is_some()
                    || snapshot.is_uncle(&parent_hash))
                || uncle.number() >= candidate_number
            {
                removed.push(Arc::new(uncle.data()));
            } else {
                uncles.push(uncle.data());
            }
        }

        for r in removed {
            self.candidate_uncles.remove(&r);
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
    use ckb_dao_utils::genesis_dao_data;
    use ckb_jsonrpc_types::{JsonBytes, ScriptHashType};
    use ckb_notify::{NotifyController, NotifyService};
    use ckb_pow::Pow;
    use ckb_shared::shared::Shared;
    use ckb_shared::shared::SharedBuilder;
    use ckb_store::ChainStore;
    use ckb_traits::ChainProvider;
    use ckb_types::{
        core::{
            BlockBuilder, BlockNumber, BlockView, EpochExt, HeaderBuilder, HeaderView,
            TransactionBuilder, TransactionView,
        },
        h256,
        packed::{Block, CellInput, CellOutput, CellOutputBuilder, OutPoint},
        H256,
    };
    use ckb_verification::{BlockVerifier, HeaderResolverWrapper, HeaderVerifier, Verifier};
    use std::sync::Arc;

    const BASIC_BLOCK_SIZE: u64 = 706;

    fn start_chain(
        consensus: Option<Consensus>,
        notify: Option<NotifyController>,
    ) -> (ChainController, Shared, NotifyController) {
        let mut builder = SharedBuilder::default();
        if let Some(consensus) = consensus {
            builder = builder.consensus(consensus);
        }
        let (shared, table) = builder.build().unwrap();

        let notify = notify.unwrap_or_else(|| NotifyService::default().start::<&str>(None));
        let chain_service = ChainService::new(shared.clone(), table, notify.clone());
        let chain_controller = chain_service.start::<&str>(None);
        (chain_controller, shared, notify)
    }

    fn setup_block_assembler(shared: Shared, config: BlockAssemblerConfig) -> BlockAssembler {
        BlockAssembler::new(shared, config)
    }

    #[test]
    fn test_get_block_template() {
        let (_chain_controller, shared, _notify) = start_chain(None, None);
        let config = BlockAssemblerConfig {
            code_hash: h256!("0x0"),
            args: vec![],
            data: JsonBytes::default(),
            hash_type: ScriptHashType::Data,
        };
        let mut block_assembler = setup_block_assembler(shared.clone(), config);
        let mut candidate_uncles = CandidateUncles::new();

        let block_template = block_assembler
            .get_block_template(None, None, None, &mut candidate_uncles)
            .unwrap();

        let block: Block = block_template.into();
        let block = block.as_advanced_builder().build();
        let header = block.header();

        let resolver = HeaderResolverWrapper::new(&header, shared.store(), shared.consensus());
        let header_verify_result = {
            let snapshot: &Snapshot = &shared.snapshot();
            let header_verifier = HeaderVerifier::new(snapshot, Pow::Dummy.engine());
            header_verifier.verify(&resolver)
        };
        assert!(header_verify_result.is_ok());

        let block_verify = BlockVerifier::new(shared.consensus());
        assert!(block_verify.verify(&block).is_ok());
    }

    fn gen_block(parent_header: &HeaderView, nonce: u64, epoch: &EpochExt) -> BlockView {
        let number = parent_header.number() + 1;
        let cellbase = create_cellbase(number, epoch);
        // This just make sure we can generate a valid block template,
        // the actual DAO validation logic will be ensured in other
        // tests
        let dao = genesis_dao_data(&cellbase).unwrap();
        let header = HeaderBuilder::default()
            .parent_hash(parent_header.hash())
            .timestamp((parent_header.timestamp() + 10).pack())
            .number(number.pack())
            .epoch(epoch.number().pack())
            .difficulty(epoch.difficulty().clone().pack())
            .nonce(nonce.pack())
            .dao(dao)
            .build();

        BlockBuilder::default()
            .header(header)
            .transaction(cellbase)
            .proposal([1; 10].pack())
            .build_unchecked()
    }

    fn create_cellbase(number: BlockNumber, epoch: &EpochExt) -> TransactionView {
        TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(number))
            .output(
                CellOutput::new_builder()
                    .capacity(epoch.block_reward(number).unwrap().pack())
                    .build(),
            )
            .output_data(Default::default())
            .build()
    }

    #[test]
    fn test_prepare_uncles() {
        let mut consensus = Consensus::default();
        consensus.genesis_epoch_ext.set_length(5);
        let epoch = consensus.genesis_epoch_ext().clone();

        let (chain_controller, shared, notify) = start_chain(Some(consensus), None);
        let config = BlockAssemblerConfig {
            code_hash: h256!("0x0"),
            args: vec![],
            data: JsonBytes::default(),
            hash_type: ScriptHashType::Data,
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
        let hash0_0: H256 = block0_0.hash().unpack();
        let hash0_1: H256 = block0_1.hash().unpack();
        let (block0_0, block0_1) = if hash0_0 < hash0_1 {
            (block0_1, block0_0)
        } else {
            (block0_0, block0_1)
        };

        let last_epoch = epoch.clone();
        let epoch = shared
            .next_epoch_ext(&last_epoch, &block0_1.header())
            .unwrap_or(last_epoch);

        let block1_1 = gen_block(&block0_1.header(), 10, &epoch);

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
        assert_eq!(block_template.uncles[0].hash, block0_0.hash().unpack());

        let last_epoch = epoch.clone();
        let epoch = shared
            .next_epoch_ext(&last_epoch, &block1_1.header())
            .unwrap_or(last_epoch);

        let block2_1 = gen_block(&block1_1.header(), 10, &epoch);
        chain_controller
            .process_block(Arc::new(block2_1.clone()), false)
            .unwrap();

        let block_template = block_assembler_controller
            .get_block_template(None, None, None)
            .unwrap();
        // block number 4, epoch 0, uncles should retained
        assert_eq!(block_template.uncles[0].hash, block0_0.hash().unpack());

        let last_epoch = epoch.clone();
        let epoch = shared
            .next_epoch_ext(&last_epoch, &block2_1.header())
            .unwrap_or(last_epoch);

        let block3_1 = gen_block(&block2_1.header(), 10, &epoch);
        chain_controller
            .process_block(Arc::new(block3_1.clone()), false)
            .unwrap();

        let block_template = block_assembler_controller
            .get_block_template(None, None, None)
            .unwrap();
        // block number 5, epoch 1, block_template should not include last epoch uncles
        assert!(block_template.uncles.is_empty());
    }

    fn build_tx(
        parent_tx: &TransactionView,
        inputs: &[u32],
        outputs_len: usize,
    ) -> TransactionView {
        let per_output_capacity =
            Capacity::shannons(parent_tx.outputs_capacity().unwrap().as_u64() / outputs_len as u64);
        TransactionBuilder::default()
            .inputs(
                inputs.iter().map(|index| {
                    CellInput::new(OutPoint::new(parent_tx.hash().to_owned(), *index), 0)
                }),
            )
            .outputs(
                (0..outputs_len)
                    .map(|_| {
                        CellOutputBuilder::default()
                            .capacity(per_output_capacity.pack())
                            .build()
                    })
                    .collect::<Vec<CellOutput>>(),
            )
            .outputs_data((0..outputs_len).map(|_| Bytes::new().pack()))
            .build()
    }

    #[test]
    fn test_package_basic() {
        let mut consensus = Consensus::default();
        consensus.genesis_epoch_ext.set_length(5);
        let epoch = consensus.genesis_epoch_ext().clone();

        let (chain_controller, shared, notify) = start_chain(Some(consensus), None);
        let config = BlockAssemblerConfig {
            code_hash: h256!("0x0"),
            args: vec![],
            data: JsonBytes::default(),
            hash_type: ScriptHashType::Data,
        };
        let block_assembler = setup_block_assembler(shared.clone(), config);
        let block_assembler_controller = block_assembler.start(Some("test"), &notify.clone());

        let genesis = shared
            .store()
            .get_block_header(&shared.store().get_block_hash(0).unwrap())
            .unwrap();
        let mut parent_header = genesis.to_owned();
        let mut blocks = vec![];
        for _i in 0..4 {
            let block = gen_block(&parent_header, 11, &epoch);
            chain_controller
                .process_block(Arc::new(block.clone()), false)
                .expect("process block");
            parent_header = block.header().to_owned();
            blocks.push(block);
        }

        let tx0 = &blocks[0].transactions()[0];
        let tx1 = build_tx(tx0, &[0], 2);
        let tx2 = build_tx(&tx1, &[0], 2);
        let tx3 = build_tx(&tx2, &[0], 2);
        let tx4 = build_tx(&tx3, &[0], 2);

        let tx2_0 = &blocks[1].transactions()[0];
        let tx2_1 = build_tx(tx2_0, &[0], 2);
        let tx2_2 = build_tx(&tx2_1, &[0], 2);
        let tx2_3 = build_tx(&tx2_2, &[0], 2);

        {
            let mut tx_pool = shared.try_lock_tx_pool();
            for (tx, fee, cycles, size) in &[
                (&tx1, 100, 0, 100),
                (&tx2, 100, 0, 100),
                (&tx3, 100, 0, 100),
                (&tx4, 1500, 0, 500),
                (&tx2_1, 150, 0, 100),
                (&tx2_2, 150, 0, 100),
                (&tx2_3, 150, 0, 100),
            ] {
                tx_pool.add_proposed(
                    *cycles,
                    Capacity::shannons(*fee),
                    *size,
                    (*tx).to_owned(),
                    vec![],
                );
            }
        }

        let check_txs = |block_template: &BlockTemplate, expect_txs: Vec<&TransactionView>| {
            assert_eq!(
                block_template
                    .transactions
                    .iter()
                    .map(|tx| format!("{}", tx.hash))
                    .collect::<Vec<_>>(),
                expect_txs
                    .iter()
                    .map(|tx| format!("{}", Unpack::<H256>::unpack(&tx.hash())))
                    .collect::<Vec<_>>()
            );
        };

        // 300 size best scored txs
        let block_template = block_assembler_controller
            .get_block_template(Some(300 + BASIC_BLOCK_SIZE), None, None)
            .unwrap();
        check_txs(&block_template, vec![&tx2_1, &tx2_2, &tx2_3]);

        // 400 size best scored txs
        let block_template = block_assembler_controller
            .get_block_template(Some(400 + BASIC_BLOCK_SIZE), None, None)
            .unwrap();
        check_txs(&block_template, vec![&tx2_1, &tx2_2, &tx2_3, &tx1]);

        // 500 size best scored txs
        let block_template = block_assembler_controller
            .get_block_template(Some(500 + BASIC_BLOCK_SIZE), None, None)
            .unwrap();
        check_txs(&block_template, vec![&tx2_1, &tx2_2, &tx2_3, &tx1, &tx2]);

        // 600 size best scored txs
        let block_template = block_assembler_controller
            .get_block_template(Some(600 + BASIC_BLOCK_SIZE), None, None)
            .unwrap();
        check_txs(
            &block_template,
            vec![&tx2_1, &tx2_2, &tx2_3, &tx1, &tx2, &tx3],
        );

        // 700 size best scored txs
        let block_template = block_assembler_controller
            .get_block_template(Some(700 + BASIC_BLOCK_SIZE), None, None)
            .unwrap();
        check_txs(
            &block_template,
            vec![&tx2_1, &tx2_2, &tx2_3, &tx1, &tx2, &tx3],
        );

        // 800 size best scored txs
        let block_template = block_assembler_controller
            .get_block_template(Some(800 + BASIC_BLOCK_SIZE), None, None)
            .unwrap();
        check_txs(&block_template, vec![&tx1, &tx2, &tx3, &tx4]);

        // none package txs
        let block_template = block_assembler_controller
            .get_block_template(Some(30 + BASIC_BLOCK_SIZE), None, None)
            .unwrap();
        check_txs(&block_template, vec![]);

        // best scored txs
        let block_template = block_assembler_controller
            .get_block_template(None, None, None)
            .unwrap();
        check_txs(
            &block_template,
            vec![&tx1, &tx2, &tx3, &tx4, &tx2_1, &tx2_2, &tx2_3],
        );
    }

    #[test]
    fn test_package_multi_best_scores() {
        let mut consensus = Consensus::default();
        consensus.genesis_epoch_ext.set_length(5);
        let epoch = consensus.genesis_epoch_ext().clone();

        let (chain_controller, shared, notify) = start_chain(Some(consensus), None);
        let config = BlockAssemblerConfig {
            code_hash: h256!("0x0"),
            args: vec![],
            data: JsonBytes::default(),
            hash_type: ScriptHashType::Data,
        };
        let block_assembler = setup_block_assembler(shared.clone(), config);
        let block_assembler_controller = block_assembler.start(Some("test"), &notify.clone());

        let genesis = shared
            .store()
            .get_block_header(&shared.store().get_block_hash(0).unwrap())
            .unwrap();
        let mut parent_header = genesis.to_owned();
        let mut blocks = vec![];
        for _i in 0..4 {
            let block = gen_block(&parent_header, 11, &epoch);
            chain_controller
                .process_block(Arc::new(block.clone()), false)
                .expect("process block");
            parent_header = block.header().to_owned();
            blocks.push(block);
        }

        let tx0 = &blocks[0].transactions()[0];
        let tx1 = build_tx(tx0, &[0], 2);
        let tx2 = build_tx(&tx1, &[0], 2);
        let tx3 = build_tx(&tx2, &[0], 2);
        let tx4 = build_tx(&tx3, &[0], 2);

        let tx2_0 = &blocks[1].transactions()[0];
        let tx2_1 = build_tx(tx2_0, &[0], 2);
        let tx2_2 = build_tx(&tx2_1, &[0], 2);
        let tx2_3 = build_tx(&tx2_2, &[0], 2);
        let tx2_4 = build_tx(&tx2_3, &[0], 2);

        let tx3_0 = &blocks[2].transactions()[0];
        let tx3_1 = build_tx(tx3_0, &[0], 1);

        let tx4_0 = &blocks[3].transactions()[0];
        let tx4_1 = build_tx(tx4_0, &[0], 1);

        {
            let mut tx_pool = shared.try_lock_tx_pool();
            for (tx, fee, cycles, size) in &[
                (&tx1, 200, 0, 100),
                (&tx2, 200, 0, 100),
                (&tx3, 50, 0, 50),
                (&tx4, 1500, 0, 500),
                (&tx2_1, 150, 0, 100),
                (&tx2_2, 150, 0, 100),
                (&tx2_3, 150, 0, 100),
                (&tx2_4, 150, 0, 100),
                (&tx3_1, 1000, 0, 1000),
                (&tx4_1, 300, 0, 250),
            ] {
                tx_pool.add_proposed(
                    *cycles,
                    Capacity::shannons(*fee),
                    *size,
                    (*tx).to_owned(),
                    vec![],
                );
            }
        }

        let check_txs = |block_template: &BlockTemplate, expect_txs: Vec<&TransactionView>| {
            assert_eq!(
                block_template
                    .transactions
                    .iter()
                    .map(|tx| format!("{}", tx.hash))
                    .collect::<Vec<_>>(),
                expect_txs
                    .iter()
                    .map(|tx| format!("{}", Unpack::<H256>::unpack(&tx.hash())))
                    .collect::<Vec<_>>()
            );
        };

        // 250 size best scored txs
        let block_template = block_assembler_controller
            .get_block_template(Some(250 + BASIC_BLOCK_SIZE), None, None)
            .unwrap();
        check_txs(&block_template, vec![&tx1, &tx2, &tx3]);

        // 400 size best scored txs
        let block_template = block_assembler_controller
            .get_block_template(Some(400 + BASIC_BLOCK_SIZE), None, None)
            .unwrap();
        check_txs(&block_template, vec![&tx1, &tx2, &tx2_1, &tx2_2]);

        // 500 size best scored txs
        let block_template = block_assembler_controller
            .get_block_template(Some(500 + BASIC_BLOCK_SIZE), None, None)
            .unwrap();
        check_txs(&block_template, vec![&tx1, &tx2, &tx2_1, &tx2_2, &tx2_3]);

        // 900 size best scored txs
        let block_template = block_assembler_controller
            .get_block_template(Some(900 + BASIC_BLOCK_SIZE), None, None)
            .unwrap();
        check_txs(&block_template, vec![&tx1, &tx2, &tx3, &tx4, &tx2_1]);

        // none package txs
        let block_template = block_assembler_controller
            .get_block_template(Some(30 + BASIC_BLOCK_SIZE), None, None)
            .unwrap();
        check_txs(&block_template, vec![]);

        // best scored txs
        let block_template = block_assembler_controller
            .get_block_template(None, None, None)
            .unwrap();
        check_txs(
            &block_template,
            vec![
                &tx1, &tx2, &tx3, &tx4, &tx2_1, &tx2_2, &tx2_3, &tx2_4, &tx4_1, &tx3_1,
            ],
        );
    }

    #[test]
    fn test_package_zero_fee_txs() {
        let mut consensus = Consensus::default();
        consensus.genesis_epoch_ext.set_length(5);
        let epoch = consensus.genesis_epoch_ext().clone();

        let (chain_controller, shared, notify) = start_chain(Some(consensus), None);
        let config = BlockAssemblerConfig {
            code_hash: h256!("0x0"),
            args: vec![],
            data: JsonBytes::default(),
            hash_type: ScriptHashType::Data,
        };
        let block_assembler = setup_block_assembler(shared.clone(), config);
        let block_assembler_controller = block_assembler.start(Some("test"), &notify.clone());

        let genesis = shared
            .store()
            .get_block_header(&shared.store().get_block_hash(0).unwrap())
            .unwrap();
        let mut parent_header = genesis.to_owned();
        let mut blocks = vec![];
        for _i in 0..4 {
            let block = gen_block(&parent_header, 11, &epoch);
            chain_controller
                .process_block(Arc::new(block.clone()), false)
                .expect("process block");
            parent_header = block.header().to_owned();
            blocks.push(block);
        }

        let tx0 = &blocks[0].transactions()[0];
        let tx1 = build_tx(tx0, &[0], 2);
        let tx2 = build_tx(&tx1, &[0], 2);
        let tx3 = build_tx(&tx2, &[0], 2);
        let tx4 = build_tx(&tx3, &[0], 2);
        let tx5 = build_tx(&tx4, &[0], 2);

        {
            let mut tx_pool = shared.try_lock_tx_pool();
            for (tx, fee, cycles, size) in &[
                (&tx1, 1000, 0, 100),
                (&tx2, 0, 0, 100),
                (&tx3, 0, 0, 100),
                (&tx4, 0, 0, 100),
                (&tx5, 0, 0, 100),
            ] {
                tx_pool.add_proposed(
                    *cycles,
                    Capacity::shannons(*fee),
                    *size,
                    (*tx).to_owned(),
                    vec![],
                );
            }
        }

        let check_txs = |block_template: &BlockTemplate, expect_txs: Vec<&TransactionView>| {
            assert_eq!(
                block_template
                    .transactions
                    .iter()
                    .map(|tx| format!("{}", tx.hash))
                    .collect::<Vec<_>>(),
                expect_txs
                    .iter()
                    .map(|&tx| format!("{}", Unpack::<H256>::unpack(&tx.hash())))
                    .collect::<Vec<_>>()
            );
        };
        // best scored txs
        let block_template = block_assembler_controller
            .get_block_template(None, None, None)
            .unwrap();
        check_txs(&block_template, vec![&tx1, &tx2, &tx3, &tx4, &tx5]);
    }
}
