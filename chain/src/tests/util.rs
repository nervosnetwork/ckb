use crate::chain::{ChainBuilder, ChainController};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::block::BlockBuilder;
use ckb_core::header::{Header, HeaderBuilder};
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, Transaction, TransactionBuilder};
use ckb_core::uncle::UncleBlock;
use ckb_core::BlockNumber;
use ckb_db::memorydb::MemoryKeyValueDB;
use ckb_notify::NotifyService;
use ckb_shared::shared::Shared;
use ckb_shared::shared::SharedBuilder;
use ckb_shared::store::ChainKVStore;
use faketime::unix_time_as_millis;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;

pub(crate) fn start_chain(
    consensus: Option<Consensus>,
    verification: bool,
) -> (ChainController, Shared<ChainKVStore<MemoryKeyValueDB>>) {
    let builder = SharedBuilder::<MemoryKeyValueDB>::new();
    let shared = builder
        .consensus(consensus.unwrap_or_else(|| Consensus::default().set_cellbase_maturity(0)))
        .build();

    let notify = NotifyService::default().start::<&str>(None);
    let chain_service = ChainBuilder::new(shared.clone(), notify)
        .verification(verification)
        .build();
    let chain_controller = chain_service.start::<&str>(None);
    (chain_controller, shared)
}

fn create_cellbase(number: BlockNumber) -> Transaction {
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .output(CellOutput::new(
            5000,
            vec![],
            Script::always_success(),
            None,
        ))
        .build()
}

pub(crate) fn gen_block(
    parent_header: &Header,
    difficulty: U256,
    commit_transactions: Vec<Transaction>,
    proposal_transactions: Vec<Transaction>,
    uncles: Vec<UncleBlock>,
) -> Block {
    let number = parent_header.number() + 1;
    let cellbase = create_cellbase(number);
    let header_builder = HeaderBuilder::default()
        .parent_hash(parent_header.hash().clone())
        .timestamp(unix_time_as_millis())
        .number(number)
        .difficulty(difficulty);

    BlockBuilder::default()
        .commit_transaction(cellbase)
        .commit_transactions(commit_transactions)
        .uncles(uncles)
        .proposal_transactions(
            proposal_transactions
                .iter()
                .map(Transaction::proposal_short_id)
                .collect(),
        )
        .with_header_builder(header_builder)
}

pub(crate) fn create_transaction(parent: H256, unique_data: u8) -> Transaction {
    TransactionBuilder::default()
        .output(CellOutput::new(
            5000,
            vec![unique_data],
            Script::always_success(),
            None,
        ))
        .input(CellInput::new(OutPoint::new(parent, 0), 0, vec![]))
        .build()
}
