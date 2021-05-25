use ckb_db::RocksDBWriteBatch;
use ckb_db_schema::{
    Col, COLUMN_BLOCK_BODY, COLUMN_BLOCK_EXTENSION, COLUMN_BLOCK_HEADER, COLUMN_BLOCK_PROPOSAL_IDS,
    COLUMN_BLOCK_UNCLE, COLUMN_CELL, COLUMN_CELL_DATA, COLUMN_NUMBER_HASH,
};
use ckb_error::Error;
use ckb_types::{core::BlockNumber, packed, prelude::*};

/// TODO(doc): @quake
pub struct StoreWriteBatch {
    pub(crate) inner: RocksDBWriteBatch,
}

impl StoreWriteBatch {
    /// TODO(doc): @quake
    pub fn put(&mut self, col: Col, key: &[u8], value: &[u8]) -> Result<(), Error> {
        self.inner.put(col, key, value)
    }

    /// TODO(doc): @quake
    pub fn delete(&mut self, col: Col, key: &[u8]) -> Result<(), Error> {
        self.inner.delete(col, key)
    }

    /// Return WriteBatch serialized size (in bytes).
    pub fn size_in_bytes(&self) -> usize {
        self.inner.size_in_bytes()
    }

    /// TODO(doc): @quake
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// TODO(doc): @quake
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// TODO(doc): @quake
    pub fn clear(&mut self) -> Result<(), Error> {
        self.inner.clear()
    }

    /// TODO(doc): @quake
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
            } else {
                self.put(COLUMN_CELL_DATA, &key, &[])?;
            }
        }
        Ok(())
    }

    /// TODO(doc): @quake
    pub fn delete_cells(
        &mut self,
        out_points: impl Iterator<Item = packed::OutPoint>,
    ) -> Result<(), Error> {
        for out_point in out_points {
            let key = out_point.to_cell_key();
            self.delete(COLUMN_CELL, &key)?;
            self.delete(COLUMN_CELL_DATA, &key)?;
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
        let txs_start_key = packed::TransactionKey::new_builder()
            .block_hash(hash.clone())
            .index(0u32.pack())
            .build();

        let txs_end_key = packed::TransactionKey::new_builder()
            .block_hash(hash.clone())
            .index(txs_len.pack())
            .build();

        self.inner.delete_range(
            COLUMN_BLOCK_BODY,
            txs_start_key.as_slice(),
            txs_end_key.as_slice(),
        )?;
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
