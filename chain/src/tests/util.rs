use crate::chain::{ChainController, ChainService};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::block::BlockBuilder;
use ckb_core::header::{Header, HeaderBuilder};
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, Transaction, TransactionBuilder};
use ckb_core::uncle::UncleBlock;
use ckb_core::{capacity_bytes, BlockNumber, Bytes, Capacity};
use ckb_db::memorydb::MemoryKeyValueDB;
use ckb_notify::NotifyService;
use ckb_shared::shared::Shared;
use ckb_shared::shared::SharedBuilder;
use ckb_store::ChainKVStore;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use test_chain_utils::create_always_success_cell;

fn create_always_success_tx() -> Transaction {
    let (always_success_cell, _) = create_always_success_cell();
    TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0, Default::default()))
        .output(always_success_cell)
        .build()
}

// NOTE: this is quite a waste of resource but the alternative is to modify 100+
// invocations, let's stick to this way till this becomes a real problem
fn create_always_success_out_point() -> OutPoint {
    OutPoint::new_cell(create_always_success_tx().hash().to_owned(), 0)
}

pub(crate) fn start_chain(
    consensus: Option<Consensus>,
) -> (ChainController, Shared<ChainKVStore<MemoryKeyValueDB>>) {
    let builder = SharedBuilder::<MemoryKeyValueDB>::new();
    let shared = builder
        .consensus(consensus.unwrap_or_else(|| {
            let genesis_block = BlockBuilder::default()
                .transaction(create_always_success_tx())
                .build();
            Consensus::default()
                .set_cellbase_maturity(0)
                .set_genesis_block(genesis_block)
        }))
        .build()
        .unwrap();

    let notify = NotifyService::default().start::<&str>(None);
    let chain_service = ChainService::new(shared.clone(), notify);
    let chain_controller = chain_service.start::<&str>(None);
    (chain_controller, shared)
}

fn create_cellbase(number: BlockNumber) -> Transaction {
    let (_, always_success_script) = create_always_success_cell();
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .output(CellOutput::new(
            capacity_bytes!(2_500),
            Bytes::default(),
            always_success_script,
            None,
        ))
        .build()
}

pub(crate) fn gen_block(
    parent_header: &Header,
    difficulty: U256,
    transactions: Vec<Transaction>,
    proposals: Vec<Transaction>,
    uncles: Vec<UncleBlock>,
) -> Block {
    let number = parent_header.number() + 1;
    let cellbase = create_cellbase(number);
    let header_builder = HeaderBuilder::default()
        .parent_hash(parent_header.hash().to_owned())
        .timestamp(parent_header.timestamp() + 20_000)
        .number(number)
        .difficulty(difficulty);

    BlockBuilder::default()
        .transaction(cellbase)
        .transactions(transactions)
        .uncles(uncles)
        .proposals(
            proposals
                .iter()
                .map(Transaction::proposal_short_id)
                .collect(),
        )
        .header_builder(header_builder)
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
            capacity_bytes!(2_500),
            Bytes::from(vec![unique_data]),
            always_success_script,
            None,
        ))
        .input(CellInput::new(out_point, 0, 0))
        .dep(always_success_out_point)
        .build()
}
