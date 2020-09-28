use crate::{BlockNumber, Capacity, CellOutput, Uint64};
use ckb_types::H256;
use serde::{Deserialize, Serialize};

/// An indexed live cell.
#[derive(Debug, Serialize, Deserialize)]
pub struct LiveCell {
    /// Where this cell is created.
    ///
    /// The cell is the `created_by.index`-th output in the transaction `created_by.tx_hash`, which
    /// has been committed to at the height `created_by.block_number` in the chain.
    pub created_by: TransactionPoint,
    /// The cell properties.
    pub cell_output: CellOutput,
    /// The cell data length.
    pub output_data_len: Uint64,
    /// Whether this cell is an output of a cellbase transaction.
    pub cellbase: bool,
}

/// Cell related transaction information.
#[derive(Debug, Serialize, Deserialize)]
pub struct CellTransaction {
    /// Where this cell is created.
    ///
    /// The cell is the `created_by.index`-th output in the transaction `created_by.tx_hash`, which
    /// has been committed in at the height `created_by.block_number` in the chain.
    pub created_by: TransactionPoint,
    /// Where this cell is consumed.
    ///
    /// This is null if the cell is still live.
    ///
    /// The cell is consumed as the `consumed_by.index`-th input in the transaction `consumed_by.tx_hash`, which
    /// has been committed to at the height `consumed_by.block_number` in the chain.
    pub consumed_by: Option<TransactionPoint>,
}

/// Reference to a cell by transaction hash and output index, as well as in which block this
/// transaction is committed.
#[derive(Debug, Serialize, Deserialize)]
pub struct TransactionPoint {
    /// In which block the transaction creating the cell is committed.
    pub block_number: BlockNumber,
    /// In which transaction this cell is an output.
    pub tx_hash: H256,
    /// The index of this cell in the transaction. Based on the context, this is either an input index
    /// or an output index.
    pub index: Uint64,
}

/// Cell script lock hash index state.
#[derive(Serialize, Deserialize, Debug)]
pub struct LockHashIndexState {
    /// The script lock hash.
    ///
    /// This index will index cells that lock script hash matches.
    pub lock_hash: H256,
    /// The max block number this index has already scanned.
    pub block_number: BlockNumber,
    /// The hash of the block with the max block number that this index has already scanned.
    pub block_hash: H256,
}

/// The accumulated capacity of a set of cells.
#[derive(Serialize, Deserialize, Debug)]
pub struct LockHashCapacity {
    /// Total capacity of all the cells in the set.
    pub capacity: Capacity,
    /// Count of cells in the set.
    pub cells_count: Uint64,
    /// This information is calculated when the max block number in the chain is `block_number`.
    pub block_number: BlockNumber,
}
