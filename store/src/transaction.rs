use crate::cache::StoreCache;
use crate::store::ChainStore;
use crate::{
    COLUMN_BLOCK_BODY, COLUMN_BLOCK_EPOCH, COLUMN_BLOCK_EXT, COLUMN_BLOCK_HEADER,
    COLUMN_BLOCK_PROPOSAL_IDS, COLUMN_BLOCK_UNCLE, COLUMN_CELL, COLUMN_CELL_DATA, COLUMN_EPOCH,
    COLUMN_INDEX, COLUMN_META, COLUMN_TRANSACTION_INFO, COLUMN_UNCLES, META_CURRENT_EPOCH_KEY,
    META_TIP_HEADER_KEY,
};
use ckb_db::{
    iter::{DBIter, DBIterator, IteratorMode},
    Col, DBVector, RocksDBTransaction, RocksDBTransactionSnapshot,
};
use ckb_error::Error;
use ckb_types::{
    core::{BlockExt, BlockView, EpochExt, HeaderView},
    packed,
    prelude::*,
};
use std::sync::Arc;

/// TODO(doc): @quake
pub struct StoreTransaction {
    pub(crate) inner: RocksDBTransaction,
    pub(crate) cache: Arc<StoreCache>,
}

impl<'a> ChainStore<'a> for StoreTransaction {
    type Vector = DBVector;

    fn cache(&'a self) -> Option<&'a StoreCache> {
        Some(&self.cache)
    }

    fn get(&self, col: Col, key: &[u8]) -> Option<Self::Vector> {
        self.inner.get(col, key).expect("db operation should be ok")
    }

    fn get_iter(&self, col: Col, mode: IteratorMode) -> DBIter {
        self.inner
            .iter(col, mode)
            .expect("db operation should be ok")
    }
}

pub struct StoreTransactionSnapshot<'a> {
    pub(crate) inner: RocksDBTransactionSnapshot<'a>,
    pub(crate) cache: Arc<StoreCache>,
}

impl<'a> ChainStore<'a> for StoreTransactionSnapshot<'a> {
    type Vector = DBVector;

    fn cache(&'a self) -> Option<&'a StoreCache> {
        Some(&self.cache)
    }

    fn get(&self, col: Col, key: &[u8]) -> Option<Self::Vector> {
        self.inner.get(col, key).expect("db operation should be ok")
    }

    fn get_iter(&self, col: Col, mode: IteratorMode) -> DBIter {
        self.inner
            .iter(col, mode)
            .expect("db operation should be ok")
    }
}

impl StoreTransaction {
    /// TODO(doc): @quake
    pub fn insert_raw(&self, col: Col, key: &[u8], value: &[u8]) -> Result<(), Error> {
        self.inner.put(col, key, value)
    }

    /// TODO(doc): @quake
    pub fn delete(&self, col: Col, key: &[u8]) -> Result<(), Error> {
        self.inner.delete(col, key)
    }

    /// TODO(doc): @quake
    pub fn commit(&self) -> Result<(), Error> {
        self.inner.commit()
    }

    /// TODO(doc): @quake
    pub fn get_snapshot(&self) -> StoreTransactionSnapshot<'_> {
        StoreTransactionSnapshot {
            inner: self.inner.get_snapshot(),
            cache: Arc::clone(&self.cache),
        }
    }

    /// TODO(doc): @quake
    pub fn get_update_for_tip_hash(
        &self,
        snapshot: &StoreTransactionSnapshot<'_>,
    ) -> Option<packed::Byte32> {
        self.inner
            .get_for_update(COLUMN_META, META_TIP_HEADER_KEY, &snapshot.inner)
            .expect("db operation should be ok")
            .map(|slice| packed::Byte32Reader::from_slice_should_be_ok(&slice.as_ref()).to_entity())
    }

    /// TODO(doc): @quake
    pub fn insert_tip_header(&self, h: &HeaderView) -> Result<(), Error> {
        self.insert_raw(COLUMN_META, META_TIP_HEADER_KEY, h.hash().as_slice())
    }

    /// TODO(doc): @quake
    pub fn insert_block(&self, block: &BlockView) -> Result<(), Error> {
        let hash = block.hash();
        let header = block.header().pack();
        let uncles = block.uncles().pack();
        let proposals = block.data().proposals();
        self.insert_raw(COLUMN_BLOCK_HEADER, hash.as_slice(), header.as_slice())?;
        self.insert_raw(COLUMN_BLOCK_UNCLE, hash.as_slice(), uncles.as_slice())?;
        self.insert_raw(
            COLUMN_BLOCK_PROPOSAL_IDS,
            hash.as_slice(),
            proposals.as_slice(),
        )?;
        for (index, tx) in block.transactions().into_iter().enumerate() {
            let key = packed::TransactionKey::new_builder()
                .block_hash(hash.clone())
                .index(index.pack())
                .build();
            let tx_data = tx.pack();
            self.insert_raw(COLUMN_BLOCK_BODY, key.as_slice(), tx_data.as_slice())?;
        }
        Ok(())
    }

    /// TODO(doc): @quake
    pub fn delete_block(&self, hash: &packed::Byte32, txs_len: usize) -> Result<(), Error> {
        self.delete(COLUMN_BLOCK_HEADER, hash.as_slice())?;
        self.delete(COLUMN_BLOCK_UNCLE, hash.as_slice())?;
        self.delete(COLUMN_BLOCK_PROPOSAL_IDS, hash.as_slice())?;
        // currently rocksdb transaction do not support `DeleteRange`
        // https://github.com/facebook/rocksdb/issues/4812
        for index in 0..txs_len {
            let key = packed::TransactionKey::new_builder()
                .block_hash(hash.clone())
                .index(index.pack())
                .build();
            self.delete(COLUMN_BLOCK_BODY, key.as_slice())?;
        }
        Ok(())
    }

    /// TODO(doc): @quake
    pub fn insert_block_ext(
        &self,
        block_hash: &packed::Byte32,
        ext: &BlockExt,
    ) -> Result<(), Error> {
        self.insert_raw(
            COLUMN_BLOCK_EXT,
            block_hash.as_slice(),
            ext.pack().as_slice(),
        )
    }

    /// TODO(doc): @quake
    pub fn attach_block(&self, block: &BlockView) -> Result<(), Error> {
        let header = block.data().header();
        let block_hash = block.hash();
        for (index, tx_hash) in block.tx_hashes().iter().enumerate() {
            let key = packed::TransactionKey::new_builder()
                .block_hash(block_hash.clone())
                .index(index.pack())
                .build();
            let info = packed::TransactionInfo::new_builder()
                .key(key)
                .block_number(header.raw().number())
                .block_epoch(header.raw().epoch())
                .build();
            self.insert_raw(COLUMN_TRANSACTION_INFO, tx_hash.as_slice(), info.as_slice())?;
        }
        let block_number: packed::Uint64 = block.number().pack();
        self.insert_raw(COLUMN_INDEX, block_number.as_slice(), block_hash.as_slice())?;
        for uncle in block.uncles().into_iter() {
            self.insert_raw(
                COLUMN_UNCLES,
                &uncle.hash().as_slice(),
                &uncle.header().pack().as_slice(),
            )?;
        }
        self.insert_raw(COLUMN_INDEX, block_hash.as_slice(), block_number.as_slice())
    }

    /// TODO(doc): @quake
    pub fn detach_block(&self, block: &BlockView) -> Result<(), Error> {
        for tx_hash in block.tx_hashes().iter() {
            self.delete(COLUMN_TRANSACTION_INFO, tx_hash.as_slice())?;
        }
        for uncle in block.uncles().into_iter() {
            self.delete(COLUMN_UNCLES, uncle.hash().as_slice())?;
        }
        let block_number = block.data().header().raw().number();
        self.delete(COLUMN_INDEX, block_number.as_slice())?;
        self.delete(COLUMN_INDEX, block.hash().as_slice())
    }

    /// TODO(doc): @quake
    pub fn insert_block_epoch_index(
        &self,
        block_hash: &packed::Byte32,
        epoch_hash: &packed::Byte32,
    ) -> Result<(), Error> {
        self.insert_raw(
            COLUMN_BLOCK_EPOCH,
            block_hash.as_slice(),
            epoch_hash.as_slice(),
        )
    }

    /// TODO(doc): @quake
    pub fn insert_epoch_ext(&self, hash: &packed::Byte32, epoch: &EpochExt) -> Result<(), Error> {
        self.insert_raw(COLUMN_EPOCH, hash.as_slice(), epoch.pack().as_slice())?;
        let epoch_number: packed::Uint64 = epoch.number().pack();
        self.insert_raw(COLUMN_EPOCH, epoch_number.as_slice(), hash.as_slice())
    }

    /// TODO(doc): @quake
    pub fn insert_current_epoch_ext(&self, epoch: &EpochExt) -> Result<(), Error> {
        self.insert_raw(COLUMN_META, META_CURRENT_EPOCH_KEY, epoch.pack().as_slice())
    }

    /// TODO(doc): @quake
    pub fn insert_cells(
        &self,
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
            self.insert_raw(COLUMN_CELL, &key, cell.as_slice())?;
            if let Some(data) = cell_data {
                self.insert_raw(COLUMN_CELL_DATA, &key, data.as_slice())?;
            } else {
                self.insert_raw(COLUMN_CELL_DATA, &key, &[])?;
            }
        }
        Ok(())
    }

    /// TODO(doc): @quake
    pub fn delete_cells(
        &self,
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
