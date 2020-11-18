use crate::{COLUMN_CELL, COLUMN_CELL_DATA};
use ckb_db::Col;
use ckb_db::RocksDBWriteBatch;
use ckb_error::Error;
use ckb_types::{packed, prelude::*};

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
}
