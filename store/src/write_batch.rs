use ckb_db::RocksDBWriteBatch;
use ckb_db_schema::{
    COLUMN_BLOCK_ARCHIVE, COLUMN_BLOCK_BODY, COLUMN_BLOCK_EXTENSION, COLUMN_BLOCK_HEADER,
    COLUMN_BLOCK_PROPOSAL_IDS, COLUMN_BLOCK_UNCLE, COLUMN_CELL, COLUMN_CELL_DATA,
    COLUMN_CELL_DATA_HASH, COLUMN_NUMBER_HASH, Col,
};
use ckb_error::Error;
use ckb_types::{core::BlockNumber, packed, prelude::*};

/// Wrapper of `RocksDBWriteBatch`, provides atomic batch of write operations.
pub struct StoreWriteBatch {
    pub(crate) inner: RocksDBWriteBatch,
    pub(crate) store: crate::ChainDB,
    pub(crate) payload: Vec<(Col, Vec<u8>, Option<Vec<u8>>)>,
}

impl StoreWriteBatch {
    /// Write the bytes into the given column with associated key.
    pub fn put(&mut self, col: Col, key: &[u8], value: &[u8]) -> Result<(), Error> {
        self.inner.put(col, key, value)?;
        self.remember_payload(col, key, Some(value));
        Ok(())
    }

    /// Delete the data associated with the given key and given column.
    pub fn delete(&mut self, col: Col, key: &[u8]) -> Result<(), Error> {
        self.inner.delete(col, key)?;
        self.remember_payload(col, key, None);
        Ok(())
    }

    fn remember_payload(&mut self, col: Col, key: &[u8], value: Option<&[u8]>) {
        use crate::ChainStore;
        if self.store.freezer().is_some() && crate::archive::block_hash(col, key).is_some() {
            self.payload
                .push((col, key.to_vec(), value.map(<[u8]>::to_vec)));
        }
    }

    pub(crate) fn commit(&self, db: &ckb_db::RocksDB, sync: bool) -> Result<(), Error> {
        use crate::ChainStore;
        // Only commit is serialized: constructing two batches must not freeze a
        // stale copy of the same cold block into both of them.
        let _commit = self.store.archive_commit.lock();
        let mut restored = std::collections::BTreeSet::new();
        let mut prepared = None;
        for (col, key, _) in &self.payload {
            let hash = crate::archive::block_hash(col, key).expect("payload key recorded above");
            if restored.insert(hash)
                && let Some(block) = self
                    .store
                    .get_archived_block_view(&packed::Byte32::from(*hash))
            {
                let batch = prepared.get_or_insert_with(|| self.inner.clone());
                crate::archive::restore_payload(&block, |col, key, value| {
                    batch.put(col, key, value)
                })?;
                batch.put(COLUMN_BLOCK_ARCHIVE, hash.as_slice(), &0u64.to_le_bytes())?;
            }
        }
        if let Some(batch) = &mut prepared {
            // Restoration is a prefix to the caller's requested logical changes.
            // Replaying these puts/deletes after it preserves their original order.
            for (col, key, value) in &self.payload {
                match value {
                    Some(value) => batch.put(col, key, value)?,
                    None => batch.delete(col, key)?,
                }
            }
        }
        let batch = prepared.as_ref().unwrap_or(&self.inner);
        if sync {
            db.write_sync(batch)
        } else {
            db.write(batch)
        }
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
        self.inner.clear()?;
        self.payload.clear();
        Ok(())
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
        self.delete(COLUMN_BLOCK_UNCLE, hash.as_slice())?;
        self.delete(COLUMN_BLOCK_EXTENSION, hash.as_slice())?;
        self.delete(COLUMN_BLOCK_PROPOSAL_IDS, hash.as_slice())?;
        self.delete(
            COLUMN_NUMBER_HASH,
            packed::NumberHash::new_builder()
                .number(number)
                .block_hash(hash.clone())
                .build()
                .as_slice(),
        )?;

        let key_range = (0u32..txs_len).map(|i| {
            packed::TransactionKey::new_builder()
                .block_hash(hash.clone())
                .index(i)
                .build()
        });

        for key in key_range {
            self.delete(COLUMN_BLOCK_BODY, key.as_slice())?;
        }
        Ok(())
    }

    /// Removes the entire block from database with corresponding hash, number and txs number
    pub fn delete_block(
        &mut self,
        number: BlockNumber,
        hash: &packed::Byte32,
        txs_len: u32,
    ) -> Result<(), Error> {
        self.delete(COLUMN_BLOCK_HEADER, hash.as_slice())?;
        self.delete_block_body(number, hash, txs_len)
    }
}
