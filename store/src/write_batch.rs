use ckb_db::RocksDBWriteBatch;
use ckb_db_schema::{
    Col, COLUMN_BLOCK_BODY, COLUMN_BLOCK_EXTENSION, COLUMN_BLOCK_HEADER, COLUMN_BLOCK_PROPOSAL_IDS,
    COLUMN_BLOCK_UNCLE, COLUMN_CELL, COLUMN_CELL_DATA, COLUMN_CELL_DATA_HASH, COLUMN_NUMBER_HASH,
};
use ckb_error::Error;
use ckb_types::core::BlockNumber;
use ckb_types::{packed, prelude::*, BlockNumberAndHash};

/// Wrapper of `RocksDBWriteBatch`, provides atomic batch of write operations.
pub struct StoreWriteBatch {
    pub(crate) inner: RocksDBWriteBatch,
}

impl StoreWriteBatch {
    /// Write the bytes into the given column with associated key.
    pub fn put(&mut self, col: Col, key: &[u8], value: &[u8]) -> Result<(), Error> {
        self.inner.put(col, key, value)
    }

    /// Delete the data associated with the given key and given column.
    pub fn delete(&mut self, col: Col, key: &[u8]) -> Result<(), Error> {
        self.inner.delete(col, key)
    }

    /// Return WriteBatch serialized size (in bytes).
    pub fn size_in_bytes(&self) -> usize {
        self.inner.size_in_bytes()
    }

    /// Return the count of write batch.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns true if the write batch contains no operations.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Clear all updates buffered in this batch.
    pub fn clear(&mut self) -> Result<(), Error> {
        self.inner.clear()
    }

    /// Put cells into this write batch
    pub fn insert_cells(
        &mut self,
        cells: impl Iterator<
            Item = (
                packed::OutPoint,
                packed::CellEntry,
                Option<packed::CellDataEntry>,
            ),
        >,
    ) -> Result<(), Error> {
        for (out_point, cell, cell_data) in cells {
            let block_number: BlockNumber = cell.block_number().unpack();
            self.put(
                COLUMN_CELL::NAME,
                COLUMN_CELL::key(block_number, &out_point).as_ref(),
                cell.as_slice(),
            )?;
            if let Some(data) = cell_data {
                self.put(
                    COLUMN_CELL_DATA::NAME,
                    COLUMN_CELL_DATA::key(block_number, &out_point).as_ref(),
                    data.as_slice(),
                )?;
                self.put(
                    COLUMN_CELL_DATA_HASH::NAME,
                    COLUMN_CELL_DATA_HASH::key(block_number, &out_point).as_ref(),
                    data.output_data_hash().as_slice(),
                )?;
            } else {
                self.put(
                    COLUMN_CELL_DATA::NAME,
                    COLUMN_CELL_DATA::key(block_number, &out_point).as_ref(),
                    &[],
                )?;
                self.put(
                    COLUMN_CELL_DATA_HASH::NAME,
                    COLUMN_CELL_DATA_HASH::key(block_number, &out_point).as_ref(),
                    &[],
                )?;
            }
        }
        Ok(())
    }

    /// Remove cells from this write batch
    pub fn delete_cells(
        &mut self,
        block_number: BlockNumber,
        out_points: impl Iterator<Item = packed::OutPoint>,
    ) -> Result<(), Error> {
        for out_point in out_points {
            let key = out_point.to_cell_key(block_number);
            self.delete(COLUMN_CELL::NAME, &key)?;
            self.delete(COLUMN_CELL_DATA::NAME, &key)?;
            self.delete(COLUMN_CELL_DATA_HASH::NAME, &key)?;
        }

        Ok(())
    }

    /// Removes the block body from database with corresponding hash, number and txs number
    pub fn delete_block_body(
        &mut self,
        num_hash: BlockNumberAndHash,
        txs_len: u32,
    ) -> Result<(), Error> {
        self.inner.delete(
            COLUMN_BLOCK_UNCLE::NAME,
            COLUMN_BLOCK_UNCLE::key(num_hash.clone()).as_ref(),
        )?;
        self.inner
            .delete(COLUMN_BLOCK_EXTENSION::NAME, num_hash.hash().as_slice())?;
        self.inner
            .delete(COLUMN_BLOCK_PROPOSAL_IDS::NAME, num_hash.hash().as_slice())?;
        self.inner.delete(
            COLUMN_NUMBER_HASH::NAME,
            packed::NumberHash::new_builder()
                .number(num_hash.number().pack())
                .block_hash(num_hash.hash().clone())
                .build()
                .as_slice(),
        )?;

        let key_range =
            (0u32..txs_len).map(|i| COLUMN_BLOCK_BODY::key(num_hash.clone(), i as usize));

        self.inner
            .delete_range(COLUMN_BLOCK_BODY::NAME, key_range)?;
        Ok(())
    }

    /// Removes the entire block from database with corresponding hash, number and txs number
    pub fn delete_block(
        &mut self,
        num_hash: BlockNumberAndHash,
        txs_len: u32,
    ) -> Result<(), Error> {
        self.inner.delete(
            COLUMN_BLOCK_HEADER::NAME,
            COLUMN_BLOCK_HEADER::key(num_hash.clone()).as_mut_slice(),
        )?;
        self.delete_block_body(num_hash, txs_len)
    }
}
