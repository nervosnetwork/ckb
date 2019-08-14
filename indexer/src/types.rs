use ckb_jsonrpc_types::{
    BlockNumber as JsonBlockNumber, CellTransaction as JsonCellTransaction,
    LiveCell as JsonLiveCell, TransactionPoint as JsonTransactionPoint, Unsigned,
};
use ckb_types::{
    core::BlockNumber,
    packed::{self, CellOutput, OutPoint},
    prelude::*,
    H256,
};
use std::convert::TryInto;

#[derive(Debug)]
pub struct LockHashIndex {
    pub lock_hash: H256,
    pub block_number: BlockNumber,
    pub out_point: OutPoint,
}

pub struct LiveCell {
    pub created_by: TransactionPoint,
    pub cell_output: CellOutput,
}

pub struct CellTransaction {
    pub created_by: TransactionPoint,
    pub consumed_by: Option<TransactionPoint>,
}

pub struct TransactionPoint {
    pub block_number: BlockNumber,
    pub tx_hash: H256,
    // index of transaction outputs (create cell) or inputs (consume cell)
    pub index: u32,
}

#[derive(Clone)]
pub struct LockHashCellOutput {
    pub lock_hash: H256,
    pub block_number: BlockNumber,
    // Cache the `CellOutput` when `LiveCell` is deleted, it's required for fork switching.
    pub cell_output: Option<CellOutput>,
}

#[derive(Debug, Clone)]
pub struct LockHashIndexState {
    pub block_number: BlockNumber,
    pub block_hash: H256,
}

impl Pack<packed::TransactionPoint> for TransactionPoint {
    fn pack(&self) -> packed::TransactionPoint {
        packed::TransactionPoint::new_builder()
            .block_number(self.block_number.pack())
            .tx_hash(self.tx_hash.pack())
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
            .lock_hash(self.lock_hash.pack())
            .block_number(self.block_number.pack())
            .cell_output(cell_output_opt)
            .build()
    }
}

impl Pack<packed::LockHashIndexState> for LockHashIndexState {
    fn pack(&self) -> packed::LockHashIndexState {
        packed::LockHashIndexState::new_builder()
            .block_number(self.block_number.pack())
            .block_hash(self.block_hash.pack())
            .build()
    }
}

impl TransactionPoint {
    pub(crate) fn from_packed(input: packed::TransactionPointReader<'_>) -> Self {
        let block_number = input.block_number().unpack();
        let tx_hash = input.tx_hash().unpack();
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
        let lock_hash = input.lock_hash().unpack();
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
        let block_hash = input.block_hash().unpack();
        LockHashIndexState {
            block_number,
            block_hash,
        }
    }
}

impl LockHashIndex {
    pub fn new(lock_hash: H256, block_number: BlockNumber, tx_hash: H256, index: u32) -> Self {
        let out_point = OutPoint::new_builder()
            .tx_hash(tx_hash.pack())
            .index(index.pack())
            .build();
        LockHashIndex {
            lock_hash,
            block_number,
            out_point,
        }
    }

    // TODO: because we need prefix match, can be change to pack
    pub fn from_slice(slice: &[u8]) -> Self {
        debug_assert!(slice.len() == 76);
        let lock_hash = H256::from_slice(&slice[0..32]).unwrap();
        let block_number = BlockNumber::from_be_bytes(slice[32..40].try_into().unwrap());
        let tx_hash = H256::from_slice(&slice[40..72]).unwrap();
        let index = u32::from_be_bytes(slice[72..76].try_into().unwrap());

        let out_point = OutPoint::new_builder()
            .tx_hash(tx_hash.pack())
            .index(index.pack())
            .build();
        Self {
            lock_hash,
            block_number,
            out_point,
        }
    }

    // TODO: because we need prefix match, can be change to pack
    pub fn to_vec(&self) -> Vec<u8> {
        let mut result = Vec::with_capacity(76);
        let tx_hash: H256 = self.out_point.tx_hash().unpack();
        let index: u32 = self.out_point.index().unpack();
        result.extend_from_slice(self.lock_hash.as_bytes());
        result.extend_from_slice(&self.block_number.to_be_bytes());
        result.extend_from_slice(tx_hash.as_bytes());
        result.extend_from_slice(&index.to_be_bytes());
        result
    }
}

impl From<LockHashIndex> for TransactionPoint {
    fn from(lock_hash_index: LockHashIndex) -> Self {
        TransactionPoint {
            block_number: lock_hash_index.block_number,
            tx_hash: lock_hash_index.out_point.tx_hash().unpack(),
            index: lock_hash_index.out_point.index().unpack(),
        }
    }
}

impl From<LiveCell> for JsonLiveCell {
    fn from(live_cell: LiveCell) -> JsonLiveCell {
        let LiveCell {
            created_by,
            cell_output,
        } = live_cell;
        JsonLiveCell {
            created_by: created_by.into(),
            cell_output: cell_output.into(),
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
            block_number: JsonBlockNumber(block_number),
            tx_hash,
            index: Unsigned(u64::from(index)),
        }
    }
}
