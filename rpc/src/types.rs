use bigint::H256;
use ckb_core::block::Block;
use ckb_core::cell::CellStatus;
use ckb_core::header::{Header, RawHeader};
use ckb_core::transaction::ProposalShortId;
use ckb_core::transaction::{Capacity, CellOutput, OutPoint, Transaction};
use ckb_core::uncle::UncleBlock;
use ckb_shared::error::SharedError;

#[derive(Serialize)]
pub struct TransactionWithHash {
    pub hash: H256,
    pub transaction: Transaction,
}

impl From<Transaction> for TransactionWithHash {
    fn from(transaction: Transaction) -> Self {
        Self {
            hash: transaction.hash(),
            transaction,
        }
    }
}

#[derive(Serialize)]
pub struct BlockWithHash {
    pub hash: H256,
    pub header: Header,
    pub transactions: Vec<TransactionWithHash>,
}

impl From<Block> for BlockWithHash {
    fn from(block: Block) -> Self {
        Self {
            header: block.header().clone(),
            transactions: block
                .commit_transactions()
                .iter()
                .map(|tx| tx.clone().into())
                .collect(),
            hash: block.header().hash(),
        }
    }
}

// This is used as return value of get_cells_by_type_hash RPC:
// it contains both OutPoint data used for referencing a cell, as well as
// cell's own data such as lock and capacity
#[derive(Serialize)]
pub struct CellOutputWithOutPoint {
    pub outpoint: OutPoint,
    pub capacity: Capacity,
    pub lock: H256,
}

#[derive(Serialize)]
pub struct CellWithStatus {
    pub cell: Option<CellOutput>,
    pub status: String,
}

impl From<CellStatus> for CellWithStatus {
    fn from(status: CellStatus) -> Self {
        let (cell, status) = match status {
            CellStatus::Current(cell) => (Some(cell), "current"),
            CellStatus::Old => (None, "old"),
            CellStatus::Unknown => (None, "unknown"),
        };
        Self {
            cell,
            status: status.to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Config {
    pub listen_addr: String,
}

#[derive(Serialize, Debug)]
pub struct BlockTemplate {
    pub raw_header: RawHeader,
    pub uncles: Vec<UncleBlock>,
    pub commit_transactions: Vec<Transaction>,
    pub proposal_transactions: Vec<ProposalShortId>,
}

pub type BlockTemplateArgs = (H256, usize, usize);
pub type BlockTemplateReturn = Result<BlockTemplate, SharedError>;
