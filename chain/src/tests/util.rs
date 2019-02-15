use crate::chain::{ChainBuilder, ChainController};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::block::BlockBuilder;
use ckb_core::header::{Header, HeaderBuilder};
use ckb_core::transaction::{
    CellInput, CellOutput, OutPoint, ProposalShortId, Transaction, TransactionBuilder,
};
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
) -> (ChainController, Shared<ChainKVStore<MemoryKeyValueDB>>) {
    let builder = SharedBuilder::<MemoryKeyValueDB>::new();
    let shared = builder
        .consensus(consensus.unwrap_or_else(Default::default))
        .build();

    let notify = NotifyService::default().start::<&str>(None);
    let chain_service = ChainBuilder::new(shared.clone(), notify)
        .verification(false)
        .build();
    let chain_controller = chain_service.start::<&str>(None);
    (chain_controller, shared)
}

fn create_cellbase(number: BlockNumber) -> Transaction {
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .output(CellOutput::new(0, vec![], H256::zero(), None))
        .build()
}

pub(crate) fn gen_block(
    parent_header: &Header,
    nonce: u64,
    difficulty: U256,
    commit_transactions: Vec<Transaction>,
    uncles: Vec<UncleBlock>,
) -> Block {
    let number = parent_header.number() + 1;
    let cellbase = create_cellbase(number);
    let header = HeaderBuilder::default()
        .parent_hash(parent_header.hash().clone())
        .timestamp(unix_time_as_millis())
        .number(number)
        .difficulty(difficulty)
        .nonce(nonce)
        .build();

    BlockBuilder::default()
        .header(header)
        .commit_transaction(cellbase)
        .commit_transactions(commit_transactions)
        .uncles(uncles)
        .proposal_transaction(ProposalShortId::from_slice(&[1; 10]).unwrap())
        .build()
}

pub(crate) fn create_transaction(parent: H256) -> Transaction {
    let mut output = CellOutput::default();
    output.capacity = 100_000_000 / 100 as u64;
    let outputs: Vec<CellOutput> = vec![output.clone(); 100];

    TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new(parent, 0), Default::default()))
        .outputs(outputs)
        .build()
}
