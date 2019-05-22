use ckb_core::transaction::{CellOutPoint, CellOutput};
use ckb_core::BlockNumber;
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};
use std::convert::TryInto;

pub struct LockHashIndex {
    pub lock_hash: H256,
    pub block_number: BlockNumber,
    pub cell_out_point: CellOutPoint,
}

pub struct LiveCell {
    pub created_by: TransactionPoint,
    pub cell_output: CellOutput,
}

pub struct CellTransaction {
    pub created_by: TransactionPoint,
    pub consumed_by: Option<TransactionPoint>,
}

#[derive(Serialize, Deserialize)]
pub struct TransactionPoint {
    pub block_number: BlockNumber,
    pub tx_hash: H256,
    // index of transaction outputs (create cell) or inputs (cosume cell)
    pub index: u32,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct LockHashCellOutput {
    pub lock_hash: H256,
    pub block_number: BlockNumber,
    // Cache the `CellOutput` when `LiveCell` is deleted, it's required for fork switching.
    pub cell_output: Option<CellOutput>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct LockHashIndexState {
    pub block_number: BlockNumber,
    pub block_hash: H256,
}

impl LockHashIndex {
    pub fn new(lock_hash: H256, block_number: BlockNumber, tx_hash: H256, index: u32) -> Self {
        LockHashIndex {
            lock_hash,
            block_number,
            cell_out_point: CellOutPoint { tx_hash, index },
        }
    }

    pub fn from_slice(slice: &[u8]) -> Self {
        debug_assert!(slice.len() == 76);
        let lock_hash = H256::from_slice(&slice[0..32]).unwrap();
        let block_number = BlockNumber::from_le_bytes(slice[32..40].try_into().unwrap());
        let tx_hash = H256::from_slice(&slice[40..72]).unwrap();
        let index = u32::from_le_bytes(slice[72..76].try_into().unwrap());

        Self {
            lock_hash,
            block_number,
            cell_out_point: CellOutPoint { tx_hash, index },
        }
    }

    pub fn to_vec(&self) -> Vec<u8> {
        let mut bytes = self.lock_hash.to_vec();
        bytes.extend_from_slice(&self.block_number.to_le_bytes());
        bytes.extend_from_slice(self.cell_out_point.tx_hash.as_bytes());
        bytes.extend_from_slice(&self.cell_out_point.index.to_le_bytes());
        bytes.to_vec()
    }
}

impl From<LockHashIndex> for TransactionPoint {
    fn from(lock_hash_index: LockHashIndex) -> Self {
        TransactionPoint {
            block_number: lock_hash_index.block_number,
            tx_hash: lock_hash_index.cell_out_point.tx_hash,
            index: lock_hash_index.cell_out_point.index,
        }
    }
}
