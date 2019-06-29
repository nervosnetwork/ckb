use crate::chain::{ChainController, ChainService};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::block::BlockBuilder;
use ckb_core::cell::{resolve_transaction, OverlayCellProvider, TransactionsProvider};
use ckb_core::header::{Header, HeaderBuilder};
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, Transaction, TransactionBuilder};
use ckb_core::{capacity_bytes, Bytes, Capacity};
use ckb_dao::DaoCalculator;
use ckb_dao_utils::genesis_dao_data;
use ckb_db::memorydb::MemoryKeyValueDB;
use ckb_notify::NotifyService;
use ckb_shared::shared::Shared;
use ckb_shared::shared::SharedBuilder;
use ckb_store::{ChainKVStore, ChainStore};
use ckb_test_chain_utils::{build_block, create_always_success_cell};
use ckb_traits::chain_provider::ChainProvider;
use fnv::FnvHashSet;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::sync::Arc;

pub use ckb_test_chain_utils::MockStore;

const MIN_CAP: Capacity = capacity_bytes!(60);

pub(crate) fn create_always_success_tx() -> Transaction {
    let (ref always_success_cell, ref script) = create_always_success_cell();
    TransactionBuilder::default()
        .witness(script.clone().into_witness())
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

    let consensus = consensus.unwrap_or_else(|| {
        let tx = create_always_success_tx();
        let dao = genesis_dao_data(&tx).unwrap();
        let header_builder = HeaderBuilder::default().dao(dao);
        let genesis_block = BlockBuilder::from_header_builder(header_builder)
            .transaction(tx)
            .build();
        Consensus::default()
            .set_cellbase_maturity(0)
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

pub(crate) fn calculate_reward(
    store: &mut MockStore,
    consensus: &Consensus,
    parent: &Header,
) -> Capacity {
    let number = parent.number() + 1;
    let target_number = consensus.finalize_target(number).unwrap();
    let target = store.0.get_ancestor(parent.hash(), target_number).unwrap();
    let calculator = DaoCalculator::new(consensus, Arc::clone(&store.0));
    calculator
        .primary_block_reward(&target)
        .unwrap()
        .safe_add(calculator.secondary_block_reward(&target).unwrap())
        .unwrap()
}

pub(crate) fn create_cellbase(
    store: &mut MockStore,
    consensus: &Consensus,
    parent: &Header,
) -> Transaction {
    let (_, always_success_script) = create_always_success_cell();
    let capacity = calculate_reward(store, consensus, parent);
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(parent.number() + 1))
        .output(CellOutput::new(
            capacity,
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
pub struct MockChain<'a> {
    blocks: Vec<Block>,
    parent: Header,
    consensus: &'a Consensus,
}

impl<'a> MockChain<'a> {
    pub fn new(parent: Header, consensus: &'a Consensus) -> Self {
        Self {
            blocks: vec![],
            parent,
            consensus,
        }
    }

    pub fn gen_block_with_proposal_txs(&mut self, txs: Vec<Transaction>, store: &mut MockStore) {
        let difficulty = self.difficulty();
        let parent = self.tip_header();
        let cellbase = create_cellbase(store, self.consensus, &parent);
        let dao = dao_data(
            &self.consensus,
            &parent,
            &[cellbase.to_owned()],
            store,
            false,
        );
        let new_block = build_block!(
            from_header_builder: {
                parent_hash: parent.hash().to_owned(),
                number: parent.number() + 1,
                difficulty: difficulty + U256::from(100u64),
                dao: dao,
            },
            transaction: cellbase,
            proposals: txs.iter().map(Transaction::proposal_short_id),
        );
        store.insert_block(&new_block, self.consensus.genesis_epoch_ext());
        self.blocks.push(new_block);
    }

    pub fn gen_empty_block_with_difficulty(&mut self, difficulty: u64, store: &mut MockStore) {
        let parent = self.tip_header();
        let cellbase = create_cellbase(store, self.consensus, &parent);
        let dao = dao_data(
            &self.consensus,
            &parent,
            &[cellbase.to_owned()],
            store,
            false,
        );
        let new_block = build_block!(
            from_header_builder: {
                parent_hash: parent.hash().to_owned(),
                number: parent.number() + 1,
                difficulty: U256::from(difficulty),
                dao: dao,
            },
            transaction: cellbase,
        );
        store.insert_block(&new_block, self.consensus.genesis_epoch_ext());
        self.blocks.push(new_block);
    }

    pub fn gen_empty_block(&mut self, diff: u64, store: &mut MockStore) {
        let difficulty = self.difficulty();
        let parent = self.tip_header();
        let cellbase = create_cellbase(store, self.consensus, &parent);
        let dao = dao_data(
            &self.consensus,
            &parent,
            &[cellbase.to_owned()],
            store,
            false,
        );
        let new_block = build_block!(
            from_header_builder: {
                parent_hash: parent.hash().to_owned(),
                number: parent.number() + 1,
                difficulty: difficulty + U256::from(diff),
                dao: dao,
            },
            transaction: cellbase,
        );
        store.insert_block(&new_block, self.consensus.genesis_epoch_ext());
        self.blocks.push(new_block);
    }

    pub fn gen_block_with_commit_txs(
        &mut self,
        txs: Vec<Transaction>,
        store: &mut MockStore,
        ignore_resolve_error: bool,
    ) {
        let difficulty = self.difficulty();
        let parent = self.tip_header();
        let cellbase = create_cellbase(store, self.consensus, &parent);
        let mut txs_to_resolve = vec![cellbase.to_owned()];
        txs_to_resolve.extend_from_slice(&txs);
        let dao = dao_data(
            &self.consensus,
            &parent,
            &txs_to_resolve,
            store,
            ignore_resolve_error,
        );
        let new_block = build_block!(
            from_header_builder: {
                parent_hash: parent.hash().to_owned(),
                number: parent.number() + 1,
                difficulty: difficulty + U256::from(100u64),
                dao: dao,
            },
            transaction: cellbase,
            transactions: txs,
        );
        store.insert_block(&new_block, self.consensus.genesis_epoch_ext());
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

pub fn dao_data(
    consensus: &Consensus,
    parent: &Header,
    txs: &[Transaction],
    store: &mut MockStore,
    ignore_resolve_error: bool,
) -> Bytes {
    let mut seen_inputs = FnvHashSet::default();
    // In case of resolving errors, we just output a dummp DAO field,
    // since those should be the cases where we are testing invalid
    // blocks
    let transactions_provider = TransactionsProvider::new(txs);
    let overlay_cell_provider = OverlayCellProvider::new(&transactions_provider, store);
    let rtxs = txs.iter().try_fold(vec![], |mut rtxs, tx| {
        let rtx = resolve_transaction(tx, &mut seen_inputs, &overlay_cell_provider, store);
        match rtx {
            Ok(rtx) => {
                rtxs.push(rtx);
                Ok(rtxs)
            }
            Err(e) => Err(e),
        }
    });
    let rtxs = if ignore_resolve_error {
        rtxs.unwrap_or_else(|_| vec![])
    } else {
        rtxs.unwrap()
    };
    let calculator = DaoCalculator::new(consensus, Arc::clone(&store.0));
    calculator.dao_field(&rtxs, &parent).unwrap()
}
