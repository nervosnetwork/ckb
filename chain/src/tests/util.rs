use crate::chain::{ChainController, ChainService};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::block::BlockBuilder;
use ckb_core::header::{Header, HeaderBuilder};
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, Transaction, TransactionBuilder};
use ckb_core::{capacity_bytes, BlockNumber, Bytes, Capacity};
use ckb_db::memorydb::MemoryKeyValueDB;
use ckb_notify::NotifyService;
use ckb_shared::shared::Shared;
use ckb_shared::shared::SharedBuilder;
use ckb_store::ChainKVStore;
use ckb_store::ChainStore;
use ckb_traits::chain_provider::ChainProvider;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use test_chain_utils::{build_block, create_always_success_cell};

const MIN_CAP: Capacity = capacity_bytes!(60);

pub(crate) fn create_always_success_tx() -> Transaction {
    let (ref always_success_cell, _) = create_always_success_cell();
    TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .output(always_success_cell.clone())
        .build()
}

// NOTE: this is quite a waste of resource but the alternative is to modify 100+
// invocations, let's stick to this way till this becomes a real problem
pub(crate) fn create_always_success_out_point() -> OutPoint {
    OutPoint::new_cell(create_always_success_tx().hash().to_owned(), 0)
}

pub(crate) fn start_chain(
    consensus: Option<Consensus>,
) -> (
    ChainController,
    Shared<ChainKVStore<MemoryKeyValueDB>>,
    Header,
) {
    let builder = SharedBuilder::<MemoryKeyValueDB>::new();
    let (_, ref always_success_script) = create_always_success_cell();

    let consensus = consensus.unwrap_or_else(|| {
        let genesis_block = BlockBuilder::default()
            .transaction(create_always_success_tx())
            .build();
        Consensus::default()
            .set_cellbase_maturity(0)
            .set_bootstrap_lock(always_success_script.clone())
            .set_genesis_block(genesis_block)
    });
    let shared = builder.consensus(consensus).build().unwrap();

    let notify = NotifyService::default().start::<&str>(None);
    let chain_service = ChainService::new(shared.clone(), notify);
    let chain_controller = chain_service.start::<&str>(None);
    let parent = shared
        .store()
        .get_block_header(&shared.store().get_block_hash(0).unwrap())
        .unwrap();

    (chain_controller, shared, parent)
}

pub(crate) fn create_cellbase(number: BlockNumber) -> Transaction {
    let (_, always_success_script) = create_always_success_cell();
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .output(CellOutput::new(
            capacity_bytes!(1_000),
            Bytes::default(),
            always_success_script.clone(),
            None,
        ))
        .witness(always_success_script.clone().into_witness())
        .build()
}

// more flexible mock function for make non-full-dead-cell test case
pub(crate) fn create_multi_outputs_transaction(
    parent: &Transaction,
    indices: Vec<usize>,
    output_len: usize,
    data: Vec<u8>,
) -> Transaction {
    let (_, always_success_script) = create_always_success_cell();
    let always_success_out_point = create_always_success_out_point();

    let parent_outputs = parent.outputs();
    let total_capacity = indices
        .iter()
        .map(|i| parent_outputs[*i].capacity)
        .try_fold(Capacity::zero(), Capacity::safe_add)
        .unwrap();

    let output_capacity = Capacity::shannons(total_capacity.as_u64() / output_len as u64);
    let reminder = Capacity::shannons(total_capacity.as_u64() % output_len as u64);

    assert!(output_capacity > MIN_CAP);
    let data = Bytes::from(data);

    let outputs = (0..output_len).map(|i| {
        let capacity = if i == output_len - 1 {
            output_capacity.safe_add(reminder).unwrap()
        } else {
            output_capacity
        };
        CellOutput::new(capacity, data.clone(), always_success_script.clone(), None)
    });

    let parent_pts = parent.output_pts();
    let inputs = indices
        .iter()
        .map(|i| CellInput::new(parent_pts[*i].clone(), 0));

    TransactionBuilder::default()
        .outputs(outputs)
        .inputs(inputs)
        .dep(always_success_out_point)
        .build()
}

pub(crate) fn create_transaction(parent: &H256, unique_data: u8) -> Transaction {
    create_transaction_with_out_point(OutPoint::new_cell(parent.to_owned(), 0), unique_data)
}

pub(crate) fn create_transaction_with_out_point(
    out_point: OutPoint,
    unique_data: u8,
) -> Transaction {
    let (_, always_success_script) = create_always_success_cell();
    let always_success_out_point = create_always_success_out_point();

    TransactionBuilder::default()
        .output(CellOutput::new(
            capacity_bytes!(100),
            Bytes::from(vec![unique_data]),
            always_success_script.clone(),
            None,
        ))
        .input(CellInput::new(out_point, 0))
        .dep(always_success_out_point)
        .build()
}

#[derive(Clone)]
pub struct MockChain {
    blocks: Vec<Block>,
    parent: Header,
}

impl MockChain {
    pub fn new(parent: Header) -> Self {
        Self {
            blocks: vec![],
            parent,
        }
    }

    pub fn gen_block_with_proposal_txs(&mut self, txs: Vec<Transaction>) {
        let difficulty = self.difficulty();
        let parent = self.tip_header();
        let new_block = build_block!(
            from_header_builder: {
                parent_hash: parent.hash().to_owned(),
                number: parent.number() + 1,
                difficulty: difficulty + U256::from(100u64),
            },
            transaction: create_cellbase(parent.number() + 1),
            proposals: txs.iter().map(Transaction::proposal_short_id),
        );
        self.blocks.push(new_block);
    }

    pub fn gen_empty_block_with_difficulty(&mut self, difficulty: u64) {
        let parent = self.tip_header();
        let new_block = build_block!(
            from_header_builder: {
                parent_hash: parent.hash().to_owned(),
                number: parent.number() + 1,
                difficulty: U256::from(difficulty),
            },
            transaction: create_cellbase(parent.number() + 1),
        );
        self.blocks.push(new_block);
    }

    pub fn gen_empty_block(&mut self, diff: u64) {
        let difficulty = self.difficulty();
        let parent = self.tip_header();
        let new_block = build_block!(
            from_header_builder: {
                parent_hash: parent.hash().to_owned(),
                number: parent.number() + 1,
                difficulty: difficulty + U256::from(diff),
            },
            transaction: create_cellbase(parent.number() + 1),
        );
        self.blocks.push(new_block);
    }

    pub fn gen_block_with_commit_txs(&mut self, txs: Vec<Transaction>) {
        let difficulty = self.difficulty();
        let parent = self.tip_header();
        let new_block = build_block!(
            from_header_builder: {
                parent_hash: parent.hash().to_owned(),
                number: parent.number() + 1,
                difficulty: difficulty + U256::from(100u64),
            },
            transaction: create_cellbase(parent.number() + 1),
            transactions: txs,
        );
        self.blocks.push(new_block);
    }

    pub fn tip_header(&self) -> &Header {
        self.blocks.last().map_or(&self.parent, |b| b.header())
    }

    pub fn tip(&self) -> &Block {
        self.blocks.last().expect("should have tip")
    }

    pub fn difficulty(&self) -> U256 {
        self.tip_header().difficulty().to_owned()
    }

    pub fn blocks(&self) -> &Vec<Block> {
        &self.blocks
    }

    pub fn total_difficulty(&self) -> U256 {
        self.blocks()
            .iter()
            .fold(U256::from(0u64), |sum, b| sum + b.header().difficulty())
    }
}
