use crate::cache::StoreCache;
use crate::store::ChainStore;
use ckb_chain_spec::versionbits::VersionbitsIndexer;
use ckb_db::{
    DBPinnableSlice, RocksDBTransaction, RocksDBTransactionSnapshot,
    iter::{DBIter, DBIterator, IteratorMode},
};
use ckb_db_schema::{
    COLUMN_BLOCK_BODY, COLUMN_BLOCK_EPOCH, COLUMN_BLOCK_EXT, COLUMN_BLOCK_EXTENSION,
    COLUMN_BLOCK_FILTER, COLUMN_BLOCK_FILTER_HASH, COLUMN_BLOCK_HEADER, COLUMN_BLOCK_PROPOSAL_IDS,
    COLUMN_BLOCK_UNCLE, COLUMN_CELL, COLUMN_CELL_DATA, COLUMN_CELL_DATA_HASH,
    COLUMN_CHAIN_ROOT_MMR, COLUMN_EPOCH, COLUMN_INDEX, COLUMN_META, COLUMN_NUMBER_HASH,
    COLUMN_TRANSACTION_INFO, COLUMN_UNCLES, Col, META_CURRENT_EPOCH_KEY,
    META_LATEST_BUILT_FILTER_DATA_KEY, META_TIP_HEADER_KEY,
};
use ckb_error::Error;
use ckb_freezer::Freezer;
use ckb_merkle_mountain_range::{Error as MMRError, MMRStore, Result as MMRResult};
use ckb_types::{
    core::{
        BlockExt, BlockView, EpochExt, HeaderView, TransactionView,
        cell::{CellChecker, CellProvider, CellStatus},
    },
    packed::{self, Byte32, OutPoint},
    prelude::*,
    utilities::calc_filter_hash,
};
use std::sync::Arc;

/// A Transaction DB
pub struct StoreTransaction {
    pub(crate) inner: RocksDBTransaction,
    pub(crate) freezer: Option<Freezer>,
    pub(crate) cache: Arc<StoreCache>,
}

impl ChainStore for StoreTransaction {
    fn cache(&self) -> Option<&StoreCache> {
        Some(&self.cache)
    }

    fn freezer(&self) -> Option<&Freezer> {
        self.freezer.as_ref()
    }

    fn get(&self, col: Col, key: &[u8]) -> Option<DBPinnableSlice<'_>> {
        self.inner
            .get_pinned(col, key)
            .expect("db operation should be ok")
    }

    fn get_iter(&self, col: Col, mode: IteratorMode) -> DBIter<'_> {
        self.inner
            .iter(col, mode)
            .expect("db operation should be ok")
    }
}

impl VersionbitsIndexer for StoreTransaction {
    fn block_epoch_index(&self, block_hash: &Byte32) -> Option<Byte32> {
        ChainStore::get_block_epoch_index(self, block_hash)
    }

    fn epoch_ext(&self, index: &Byte32) -> Option<EpochExt> {
        ChainStore::get_epoch_ext(self, index)
    }

    fn block_header(&self, block_hash: &Byte32) -> Option<HeaderView> {
        ChainStore::get_block_header(self, block_hash)
    }

    fn cellbase(&self, block_hash: &Byte32) -> Option<TransactionView> {
        ChainStore::get_cellbase(self, block_hash)
    }
}

impl CellProvider for StoreTransaction {
    fn cell(&self, out_point: &OutPoint, eager_load: bool) -> CellStatus {
        match self.get_cell(out_point) {
            Some(mut cell_meta) => {
                if eager_load
                    && let Some((data, data_hash)) = self.get_cell_data(out_point) {
                        cell_meta.mem_cell_data = Some(data);
                        cell_meta.mem_cell_data_hash = Some(data_hash);
                    }
                CellStatus::live_cell(cell_meta)
            }
            None => CellStatus::Unknown,
        }
    }
}

impl CellChecker for StoreTransaction {
    fn is_live(&self, out_point: &OutPoint) -> Option<bool> {
        if self.have_cell(out_point) {
            Some(true)
        } else {
            None
        }
    }
}

pub struct StoreTransactionSnapshot<'a> {
    pub(crate) inner: RocksDBTransactionSnapshot<'a>,
    pub(crate) freezer: Option<Freezer>,
    pub(crate) cache: Arc<StoreCache>,
}

impl<'a> ChainStore for StoreTransactionSnapshot<'a> {
    fn cache(&self) -> Option<&StoreCache> {
        Some(&self.cache)
    }

    fn freezer(&self) -> Option<&Freezer> {
        self.freezer.as_ref()
    }

    fn get(&self, col: Col, key: &[u8]) -> Option<DBPinnableSlice<'_>> {
        self.inner
            .get_pinned(col, key)
            .expect("db operation should be ok")
    }

    fn get_iter(&self, col: Col, mode: IteratorMode) -> DBIter<'_> {
        self.inner
            .iter(col, mode)
            .expect("db operation should be ok")
    }
}

impl StoreTransaction {
    /// Inserts a raw key-value pair into the specified column.
    pub fn insert_raw(&self, col: Col, key: &[u8], value: &[u8]) -> Result<(), Error> {
        self.inner.put(col, key, value)
    }

    /// Deletes a key from the specified column.
    pub fn delete(&self, col: Col, key: &[u8]) -> Result<(), Error> {
        self.inner.delete(col, key)
    }

    /// Commits the transaction, writing all changes to the database.
    pub fn commit(&self) -> Result<(), Error> {
        self.inner.commit()
    }

    /// Returns a snapshot of the transaction's current state.
    pub fn get_snapshot(&self) -> StoreTransactionSnapshot<'_> {
        StoreTransactionSnapshot {
            inner: self.inner.get_snapshot(),
            freezer: self.freezer.clone(),
            cache: Arc::clone(&self.cache),
        }
    }

    /// Gets the tip header hash for update, locking the row in the transaction.
    pub fn get_update_for_tip_hash(
        &self,
        snapshot: &StoreTransactionSnapshot<'_>,
    ) -> Option<packed::Byte32> {
        self.inner
            .get_for_update(COLUMN_META, META_TIP_HEADER_KEY, &snapshot.inner)
            .expect("db operation should be ok")
            .map(|slice| packed::Byte32Reader::from_slice_should_be_ok(slice.as_ref()).to_entity())
    }

    /// Inserts or updates the tip header hash in the store.
    pub fn insert_tip_header(&self, h: &HeaderView) -> Result<(), Error> {
        self.insert_raw(COLUMN_META, META_TIP_HEADER_KEY, h.hash().as_slice())
    }

    /// Inserts a block into the store.
    pub fn insert_block(&self, block: &BlockView) -> Result<(), Error> {
        let hash = block.hash();
        let header = Into::<packed::HeaderView>::into(block.header());
        let uncles = Into::<packed::UncleBlockVecView>::into(block.uncles());
        let proposals = block.data().proposals();
        let txs_len: packed::Uint32 = (block.transactions().len() as u32).into();
        self.insert_raw(COLUMN_BLOCK_HEADER, hash.as_slice(), header.as_slice())?;
        self.insert_raw(COLUMN_BLOCK_UNCLE, hash.as_slice(), uncles.as_slice())?;
        if let Some(extension) = block.extension() {
            self.insert_raw(
                COLUMN_BLOCK_EXTENSION,
                hash.as_slice(),
                extension.as_slice(),
            )?;
        }
        self.insert_raw(
            COLUMN_NUMBER_HASH,
            packed::NumberHash::new_builder()
                .number(block.number())
                .block_hash(hash.clone())
                .build()
                .as_slice(),
            txs_len.as_slice(),
        )?;
        self.insert_raw(
            COLUMN_BLOCK_PROPOSAL_IDS,
            hash.as_slice(),
            proposals.as_slice(),
        )?;
        for (index, tx) in block.transactions().into_iter().enumerate() {
            let key = packed::TransactionKey::new_builder()
                .block_hash(hash.clone())
                .index(index)
                .build();
            let tx_data = Into::<packed::TransactionView>::into(tx);
            self.insert_raw(COLUMN_BLOCK_BODY, key.as_slice(), tx_data.as_slice())?;
        }
        Ok(())
    }

    /// Deletes a block from the store.
    pub fn delete_block(&self, block: &BlockView) -> Result<(), Error> {
        let hash = block.hash();
        let txs_len = block.transactions().len();
        self.delete(COLUMN_BLOCK_HEADER, hash.as_slice())?;
        self.delete(COLUMN_BLOCK_UNCLE, hash.as_slice())?;
        self.delete(COLUMN_BLOCK_EXTENSION, hash.as_slice())?;
        self.delete(COLUMN_BLOCK_PROPOSAL_IDS, hash.as_slice())?;
        self.delete(
            COLUMN_NUMBER_HASH,
            packed::NumberHash::new_builder()
                .number(block.number())
                .block_hash(hash.clone())
                .build()
                .as_slice(),
        )?;
        // currently rocksdb transaction do not support `DeleteRange`
        // https://github.com/facebook/rocksdb/issues/4812
        for index in 0..txs_len {
            let key = packed::TransactionKey::new_builder()
                .block_hash(hash.clone())
                .index(index)
                .build();
            self.delete(COLUMN_BLOCK_BODY, key.as_slice())?;
        }
        Ok(())
    }

    /// Inserts block extension data.
    pub fn insert_block_ext(
        &self,
        block_hash: &packed::Byte32,
        ext: &BlockExt,
    ) -> Result<(), Error> {
        let packed_ext: packed::BlockExtV1 = ext.into();
        self.insert_raw(
            COLUMN_BLOCK_EXT,
            block_hash.as_slice(),
            packed_ext.as_slice(),
        )
    }

    /// Attaches a block to the main chain, indexing its transactions and uncles.
    pub fn attach_block(&self, block: &BlockView) -> Result<(), Error> {
        let header = block.data().header();
        let block_hash = block.hash();
        for (index, tx_hash) in block.tx_hashes().iter().enumerate() {
            let key = packed::TransactionKey::new_builder()
                .block_hash(block_hash.clone())
                .index(index)
                .build();
            let info = packed::TransactionInfo::new_builder()
                .key(key)
                .block_number(header.raw().number())
                .block_epoch(header.raw().epoch())
                .build();
            self.insert_raw(COLUMN_TRANSACTION_INFO, tx_hash.as_slice(), info.as_slice())?;
        }
        let block_number: packed::Uint64 = block.number().into();
        self.insert_raw(COLUMN_INDEX, block_number.as_slice(), block_hash.as_slice())?;
        for uncle in block.uncles().into_iter() {
            self.insert_raw(
                COLUMN_UNCLES,
                uncle.hash().as_slice(),
                Into::<packed::HeaderView>::into(uncle.header()).as_slice(),
            )?;
        }
        self.insert_raw(COLUMN_INDEX, block_hash.as_slice(), block_number.as_slice())
    }

    /// Detaches a block from the main chain, removing its transaction and uncle indices.
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

    /// Inserts the block-to-epoch index mapping.
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

    /// Inserts epoch extension data.
    pub fn insert_epoch_ext(&self, hash: &packed::Byte32, epoch: &EpochExt) -> Result<(), Error> {
        self.insert_raw(
            COLUMN_EPOCH,
            hash.as_slice(),
            Into::<packed::EpochExt>::into(epoch).as_slice(),
        )?;
        let epoch_number: packed::Uint64 = epoch.number().into();
        self.insert_raw(COLUMN_EPOCH, epoch_number.as_slice(), hash.as_slice())
    }

    /// Inserts the current epoch extension data.
    pub fn insert_current_epoch_ext(&self, epoch: &EpochExt) -> Result<(), Error> {
        self.insert_raw(
            COLUMN_META,
            META_CURRENT_EPOCH_KEY,
            Into::<packed::EpochExt>::into(epoch).as_slice(),
        )
    }

    /// Inserts multiple cells into the store.
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
                self.insert_raw(
                    COLUMN_CELL_DATA_HASH,
                    &key,
                    data.output_data_hash().as_slice(),
                )?;
            } else {
                self.insert_raw(COLUMN_CELL_DATA, &key, &[])?;
                self.insert_raw(COLUMN_CELL_DATA_HASH, &key, &[])?;
            }
        }
        Ok(())
    }

    /// Deletes multiple cells from the store.
    pub fn delete_cells(
        &self,
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

    /// Inserts a header digest.
    pub fn insert_header_digest(
        &self,
        position_u64: u64,
        header_digest: &packed::HeaderDigest,
    ) -> Result<(), Error> {
        let position: packed::Uint64 = position_u64.into();
        self.insert_raw(
            COLUMN_CHAIN_ROOT_MMR,
            position.as_slice(),
            header_digest.as_slice(),
        )
    }

    /// Deletes a header digest.
    pub fn delete_header_digest(&self, position_u64: u64) -> Result<(), Error> {
        let position: packed::Uint64 = position_u64.into();
        self.delete(COLUMN_CHAIN_ROOT_MMR, position.as_slice())
    }

    /// insert block filter data
    pub fn insert_block_filter(
        &self,
        block_hash: &packed::Byte32,
        filter_data: &packed::Bytes,
        parent_block_filter_hash: &packed::Byte32,
    ) -> Result<(), Error> {
        self.insert_raw(
            COLUMN_BLOCK_FILTER,
            block_hash.as_slice(),
            filter_data.as_slice(),
        )?;
        let current_block_filter_hash = calc_filter_hash(parent_block_filter_hash, filter_data);
        self.insert_raw(
            COLUMN_BLOCK_FILTER_HASH,
            block_hash.as_slice(),
            current_block_filter_hash.as_slice(),
        )?;
        self.insert_raw(
            COLUMN_META,
            META_LATEST_BUILT_FILTER_DATA_KEY,
            block_hash.as_slice(),
        )
    }
}

impl MMRStore<packed::HeaderDigest> for &StoreTransaction {
    fn get_elem(&self, pos: u64) -> MMRResult<Option<packed::HeaderDigest>> {
        Ok(self.get_header_digest(pos))
    }

    fn append(&mut self, pos: u64, elems: Vec<packed::HeaderDigest>) -> MMRResult<()> {
        for (offset, elem) in elems.iter().enumerate() {
            let pos: u64 = pos + (offset as u64);
            self.insert_header_digest(pos, elem).map_err(|err| {
                MMRError::StoreError(format!("Failed to append to MMR, DB error {err}"))
            })?;
        }
        Ok(())
    }
}
