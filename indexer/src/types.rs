use ckb_db::DBConfig;
use ckb_jsonrpc_types::{
    CellTransaction as JsonCellTransaction, LiveCell as JsonLiveCell,
    LockHashCapacity as JsonLockHashCapacity, TransactionPoint as JsonTransactionPoint,
};
use ckb_types::{
    core::{BlockNumber, Capacity},
    packed::{self, Byte32, CellOutput, OutPoint},
    prelude::*,
};
use serde::{Deserialize, Serialize};

/// Indexer configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexerConfig {
    /// The minimum time (in milliseconds) between indexing execution, default is 500
    pub batch_interval: u64,
    /// The maximum number of blocks in a single indexing execution batch, default is 200
    pub batch_size: usize,
    pub db: DBConfig,
}

impl Default for IndexerConfig {
    fn default() -> Self {
        IndexerConfig {
            batch_interval: 500,
            batch_size: 200,
            db: Default::default(),
        }
    }
}

pub struct LockHashIndex {
    pub lock_hash: Byte32,
    pub block_number: BlockNumber,
    pub out_point: OutPoint,
}

pub struct LiveCell {
    pub created_by: TransactionPoint,
    pub cell_output: CellOutput,
    pub output_data_len: u64,
    pub cellbase: bool,
}

pub struct CellTransaction {
    pub created_by: TransactionPoint,
    pub consumed_by: Option<TransactionPoint>,
}

pub struct TransactionPoint {
    pub block_number: BlockNumber,
    pub tx_hash: Byte32,
    // index of transaction outputs (create cell) or inputs (consume cell)
    pub index: u32,
}

#[derive(Clone)]
pub struct LockHashCellOutput {
    pub lock_hash: Byte32,
    pub block_number: BlockNumber,
    // Cache the `CellOutput` when `LiveCell` is deleted, it's required for fork switching.
    pub cell_output: Option<CellOutput>,
}

#[derive(Debug, Clone)]
pub struct LockHashIndexState {
    pub block_number: BlockNumber,
    pub block_hash: Byte32,
}

#[derive(Debug, Clone)]
pub struct LockHashCapacity {
    pub capacity: Capacity,
    pub cells_count: u64,
    pub block_number: BlockNumber,
}

impl Pack<packed::LockHashIndex> for LockHashIndex {
    fn pack(&self) -> packed::LockHashIndex {
        let index: u32 = self.out_point.index().unpack();
        packed::LockHashIndex::new_builder()
            .lock_hash(self.lock_hash.clone())
            .block_number(self.block_number.pack())
            .tx_hash(self.out_point.tx_hash())
            .index(index.pack())
            .build()
    }
}

impl Pack<packed::TransactionPoint> for TransactionPoint {
    fn pack(&self) -> packed::TransactionPoint {
        packed::TransactionPoint::new_builder()
            .block_number(self.block_number.pack())
            .tx_hash(self.tx_hash.clone())
            .index(self.index.pack())
            .build()
    }
}

impl Pack<packed::LockHashCellOutput> for LockHashCellOutput {
    fn pack(&self) -> packed::LockHashCellOutput {
        let cell_output_opt = packed::CellOutputOpt::new_builder()
            .set(self.cell_output.clone())
            .build();
        packed::LockHashCellOutput::new_builder()
            .lock_hash(self.lock_hash.clone())
            .block_number(self.block_number.pack())
            .cell_output(cell_output_opt)
            .build()
    }
}

impl Pack<packed::LockHashIndexState> for LockHashIndexState {
    fn pack(&self) -> packed::LockHashIndexState {
        packed::LockHashIndexState::new_builder()
            .block_number(self.block_number.pack())
            .block_hash(self.block_hash.clone())
            .build()
    }
}

impl LockHashIndex {
    pub(crate) fn from_packed(input: packed::LockHashIndexReader<'_>) -> Self {
        let lock_hash = input.lock_hash().to_entity();
        let block_number = input.block_number().unpack();
        let index: u32 = input.index().unpack();
        let out_point = OutPoint::new_builder()
            .tx_hash(input.tx_hash().to_entity())
            .index(index.pack())
            .build();
        LockHashIndex {
            lock_hash,
            block_number,
            out_point,
        }
    }
}

impl TransactionPoint {
    pub(crate) fn from_packed(input: packed::TransactionPointReader<'_>) -> Self {
        let block_number = input.block_number().unpack();
        let tx_hash = input.tx_hash().to_entity();
        let index = input.index().unpack();
        TransactionPoint {
            block_number,
            tx_hash,
            index,
        }
    }
}

impl LockHashCellOutput {
    pub(crate) fn from_packed(input: packed::LockHashCellOutputReader<'_>) -> Self {
        let lock_hash = input.lock_hash().to_entity();
        let block_number = input.block_number().unpack();
        let cell_output = input.cell_output().to_entity().to_opt();
        LockHashCellOutput {
            lock_hash,
            block_number,
            cell_output,
        }
    }
}

impl LockHashIndexState {
    pub(crate) fn from_packed(input: packed::LockHashIndexStateReader<'_>) -> Self {
        let block_number = input.block_number().unpack();
        let block_hash = input.block_hash().to_entity();
        LockHashIndexState {
            block_number,
            block_hash,
        }
    }
}

impl LockHashIndex {
    pub fn new(lock_hash: Byte32, block_number: BlockNumber, tx_hash: Byte32, index: u32) -> Self {
        let out_point = OutPoint::new_builder()
            .tx_hash(tx_hash)
            .index(index.pack())
            .build();
        LockHashIndex {
            lock_hash,
            block_number,
            out_point,
        }
    }
}

impl From<LockHashIndex> for TransactionPoint {
    fn from(lock_hash_index: LockHashIndex) -> Self {
        TransactionPoint {
            block_number: lock_hash_index.block_number,
            tx_hash: lock_hash_index.out_point.tx_hash(),
            index: lock_hash_index.out_point.index().unpack(),
        }
    }
}

impl From<LiveCell> for JsonLiveCell {
    fn from(live_cell: LiveCell) -> JsonLiveCell {
        let LiveCell {
            created_by,
            cell_output,
            output_data_len,
            cellbase,
        } = live_cell;
        JsonLiveCell {
            created_by: created_by.into(),
            cell_output: cell_output.into(),
            output_data_len: output_data_len.into(),
            cellbase,
        }
    }
}

impl From<CellTransaction> for JsonCellTransaction {
    fn from(cell_transaction: CellTransaction) -> JsonCellTransaction {
        let CellTransaction {
            created_by,
            consumed_by,
        } = cell_transaction;
        JsonCellTransaction {
            created_by: created_by.into(),
            consumed_by: consumed_by.map(Into::into),
        }
    }
}

impl From<TransactionPoint> for JsonTransactionPoint {
    fn from(transaction_point: TransactionPoint) -> JsonTransactionPoint {
        let TransactionPoint {
            block_number,
            tx_hash,
            index,
        } = transaction_point;
        JsonTransactionPoint {
            block_number: block_number.into(),
            tx_hash: tx_hash.unpack(),
            index: u64::from(index).into(),
        }
    }
}

impl From<LockHashCapacity> for JsonLockHashCapacity {
    fn from(lock_hash_capacity: LockHashCapacity) -> JsonLockHashCapacity {
        let LockHashCapacity {
            capacity,
            cells_count,
            block_number,
        } = lock_hash_capacity;
        JsonLockHashCapacity {
            capacity: capacity.into(),
            cells_count: cells_count.into(),
            block_number: block_number.into(),
        }
    }
}
