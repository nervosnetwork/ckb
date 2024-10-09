use ckb_db::RocksDBWriteBatch;
use ckb_db_schema::{
    Col, COLUMN_BLOCK_BODY, COLUMN_BLOCK_EXTENSION, COLUMN_BLOCK_HEADER, COLUMN_BLOCK_PROPOSAL_IDS,
    COLUMN_BLOCK_UNCLE, COLUMN_CELL, COLUMN_CELL_DATA, COLUMN_CELL_DATA_HASH, COLUMN_NUMBER_HASH,
};
use ckb_error::Error;
use ckb_types::{core::BlockNumber, packed, prelude::*};

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
            let key = out_point.to_cell_key();
            self.put(COLUMN_CELL, &key, cell.as_slice())?;
            if let Some(data) = cell_data {
                self.put(COLUMN_CELL_DATA, &key, data.as_slice())?;
                self.put(
                    COLUMN_CELL_DATA_HASH,
                    &key,
                    data.output_data_hash().as_slice(),
                )?;
            } else {
                self.put(COLUMN_CELL_DATA, &key, &[])?;
                self.put(COLUMN_CELL_DATA_HASH, &key, &[])?;
            }
        }
        Ok(())
    }

    /// Remove cells from this write batch
    pub fn delete_cells(
        &mut self,
        out_points: impl Iterator<Item = packed::OutPoint>,
    ) -> Result<(), Error> {
        for out_point in out_points {
            let key = out_point.to_cell_key();
            self.delete(COLUMN_CELL, &key)?;
            self.delete(COLUMN_CELL_DATA, &key)?;
            self.delete(COLUMN_CELL_DATA_HASH, &key)?;
        }

        Ok(())
    }

    /// Removes the block body from database with corresponding hash, number and txs number
    pub fn delete_block_body(
        &mut self,
        number: BlockNumber,
        hash: &packed::Byte32,
        txs_len: u32,
    ) -> Result<(), Error> {
        self.inner.delete(COLUMN_BLOCK_UNCLE, hash.as_slice())?;
        self.inner.delete(COLUMN_BLOCK_EXTENSION, hash.as_slice())?;
        self.inner
            .delete(COLUMN_BLOCK_PROPOSAL_IDS, hash.as_slice())?;
        self.inner.delete(
            COLUMN_NUMBER_HASH,
            packed::NumberHash::new_builder()
                .number(number.pack())
                .block_hash(hash.clone())
                .build()
                .as_slice(),
        )?;

        let key_range = (0u32..txs_len).map(|i| {
            packed::TransactionKey::new_builder()
                .block_hash(hash.clone())
                .index(i.pack())
                .build()
        });

        self.inner.delete_range(COLUMN_BLOCK_BODY, key_range)?;
        Ok(())
    }

    /// Removes the entire block from database with corresponding hash, number and txs number
    pub fn delete_block(
        &mut self,
        number: BlockNumber,
        hash: &packed::Byte32,
        txs_len: u32,
    ) -> Result<(), Error> {
        self.inner.delete(COLUMN_BLOCK_HEADER, hash.as_slice())?;
        self.delete_block_body(number, hash, txs_len)
    }
}
