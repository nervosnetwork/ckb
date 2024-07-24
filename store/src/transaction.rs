use crate::cache::StoreCache;
use crate::store::ChainStore;
use ckb_chain_spec::versionbits::VersionbitsIndexer;
use ckb_db::{
    iter::{DBIter, DBIterator, IteratorMode},
    DBPinnableSlice, RocksDBTransaction, RocksDBTransactionSnapshot,
};
use ckb_db_schema::{
    Col, COLUMN_BLOCK_BODY, COLUMN_BLOCK_EPOCH, COLUMN_BLOCK_EXT, COLUMN_BLOCK_EXTENSION,
    COLUMN_BLOCK_FILTER, COLUMN_BLOCK_FILTER_HASH, COLUMN_BLOCK_HEADER, COLUMN_BLOCK_HEADER_NUM,
    COLUMN_BLOCK_PROPOSAL_IDS, COLUMN_BLOCK_UNCLE, COLUMN_CELL, COLUMN_CELL_DATA,
    COLUMN_CELL_DATA_HASH, COLUMN_CHAIN_ROOT_MMR, COLUMN_EPOCH, COLUMN_INDEX, COLUMN_META,
    COLUMN_NUMBER_HASH, COLUMN_TRANSACTION_INFO, COLUMN_UNCLES,
};
use ckb_error::Error;
use ckb_freezer::Freezer;
use ckb_merkle_mountain_range::{Error as MMRError, MMRStore, Result as MMRResult};
use ckb_types::core::BlockNumber;
use ckb_types::{
    core::{
        cell::{CellChecker, CellProvider, CellStatus},
        BlockExt, BlockView, EpochExt, HeaderView, TransactionView,
    },
    packed::{self, Byte32, OutPoint},
    prelude::*,
    utilities::calc_filter_hash,
    BlockNumberAndHash,
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
        // println!("get col={:?} key={}", col, hex(key));
        self.inner
            .get_pinned(col, key)
            .expect("db operation should be ok")
    }

    fn get_iter(&self, col: Col, mode: IteratorMode) -> DBIter {
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
                if eager_load {
                    if let Some((data, data_hash)) = self.get_cell_data(out_point) {
                        cell_meta.mem_cell_data = Some(data);
                        cell_meta.mem_cell_data_hash = Some(data_hash);
                    }
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

    fn get(&self, col: Col, key: &[u8]) -> Option<DBPinnableSlice> {
        // println!("get col={:?} key={}", col, hex(key));
        self.inner
            .get_pinned(col, key)
            .expect("db operation should be ok")
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
        // println!(
        //     "insert_raw col={:?} key={} value={}",
        //     col,
        //     hex(key),
        //     hex(value)
        // );
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
            freezer: self.freezer.clone(),
            cache: Arc::clone(&self.cache),
        }
    }

    /// TODO(doc): @quake
    pub fn get_update_for_tip_hash(
        &self,
        snapshot: &StoreTransactionSnapshot<'_>,
    ) -> Option<packed::Byte32> {
        self.inner
            .get_for_update(
                COLUMN_META::NAME,
                COLUMN_META::META_TIP_HEADER_KEY,
                &snapshot.inner,
            )
            .expect("db operation should be ok")
            .map(|slice| packed::Byte32Reader::from_slice_should_be_ok(slice.as_ref()).to_entity())
    }

    /// TODO(doc): @quake
    pub fn insert_tip_header(&self, h: &HeaderView) -> Result<(), Error> {
        self.insert_raw(
            COLUMN_META::NAME,
            COLUMN_META::META_TIP_HEADER_KEY,
            h.hash().as_slice(),
        )
    }

    /// TODO(doc): @quake
    pub fn insert_block(&self, block: &BlockView) -> Result<(), Error> {
        let hash = block.hash();
        let header = block.header().pack();
        let number = block.number();
        let num_hash = BlockNumberAndHash::new(number, hash.clone());
        let uncles = block.uncles().pack();
        let proposals = block.data().proposals();
        let txs_len: packed::Uint32 = (block.transactions().len() as u32).pack();
        let block_number: packed::Uint64 = number.pack();
        self.insert_raw(
            COLUMN_BLOCK_HEADER_NUM::NAME,
            hash.as_slice(),
            block_number.as_slice(),
        )?;
        self.insert_raw(
            COLUMN_BLOCK_HEADER::NAME,
            COLUMN_BLOCK_HEADER::key(num_hash.clone()).as_slice(),
            header.as_slice(),
        )?;

        self.insert_raw(
            COLUMN_BLOCK_UNCLE::NAME,
            COLUMN_BLOCK_UNCLE::key(num_hash.clone()).as_ref(),
            uncles.as_slice(),
        )?;
        if let Some(extension) = block.extension() {
            self.insert_raw(
                COLUMN_BLOCK_EXTENSION::NAME,
                hash.as_slice(),
                extension.as_slice(),
            )?;
        }
        self.insert_raw(
            COLUMN_NUMBER_HASH::NAME,
            COLUMN_NUMBER_HASH::key(num_hash.clone()).as_ref(),
            txs_len.as_slice(),
        )?;
        self.insert_raw(
            COLUMN_BLOCK_PROPOSAL_IDS::NAME,
            COLUMN_BLOCK_PROPOSAL_IDS::key(num_hash.clone()).as_ref(),
            proposals.as_slice(),
        )?;
        for (index, tx) in block.transactions().into_iter().enumerate() {
            let key = COLUMN_BLOCK_BODY::key(num_hash.clone(), index);
            let tx_data = tx.pack();
            self.insert_raw(COLUMN_BLOCK_BODY::NAME, key.as_ref(), tx_data.as_slice())?;
        }
        Ok(())
    }

    /// TODO(doc): @quake
    pub fn delete_block(&self, block: &BlockView) -> Result<(), Error> {
        let hash = block.hash();
        let number = block.number();
        let num_hash = BlockNumberAndHash::new(number, hash.clone());
        let txs_len = block.transactions().len();
        self.delete(
            COLUMN_BLOCK_HEADER::NAME,
            COLUMN_BLOCK_HEADER::key(num_hash.clone()).as_slice(),
        )?;
        self.delete(
            COLUMN_BLOCK_UNCLE::NAME,
            COLUMN_BLOCK_UNCLE::key(num_hash.clone()).as_ref(),
        )?;
        self.delete(COLUMN_BLOCK_EXTENSION::NAME, hash.as_slice())?;
        self.delete(
            COLUMN_BLOCK_PROPOSAL_IDS::NAME,
            COLUMN_BLOCK_PROPOSAL_IDS::key(num_hash.clone()).as_ref(),
        )?;
        self.delete(
            COLUMN_NUMBER_HASH::NAME,
            packed::NumberHash::new_builder()
                .number(block.number().pack())
                .block_hash(hash.clone())
                .build()
                .as_slice(),
        )?;
        // currently rocksdb transaction do not support `DeleteRange`
        // https://github.com/facebook/rocksdb/issues/4812
        for index in 0..txs_len {
            let key = COLUMN_BLOCK_BODY::key(num_hash.clone(), index);
            self.delete(COLUMN_BLOCK_BODY::NAME, key.as_ref())?;
        }
        Ok(())
    }

    /// TODO(doc): @quake
    pub fn insert_block_ext(
        &self,
        num_hash: BlockNumberAndHash,
        ext: &BlockExt,
    ) -> Result<(), Error> {
        let packed_ext: packed::BlockExtV1 = ext.pack();
        self.insert_raw(
            COLUMN_BLOCK_EXT::NAME,
            COLUMN_BLOCK_EXT::key(num_hash).as_ref(),
            packed_ext.as_slice(),
        )
    }

    /// TODO(doc): @quake
    pub fn attach_block(&self, block: &BlockView) -> Result<(), Error> {
        let header = block.data().header();
        let block_hash = block.hash();
        let number = block.number();
        for (index, tx_hash) in block.tx_hashes().iter().enumerate() {
            let key = packed::TransactionKey::new_builder()
                .block_number(number.pack())
                .block_hash(block_hash.clone())
                .index(index.pack())
                .build();
            let info = packed::TransactionInfo::new_builder()
                .key(key)
                .block_number(header.raw().number())
                .block_epoch(header.raw().epoch())
                .build();
            self.insert_raw(
                COLUMN_TRANSACTION_INFO::NAME,
                tx_hash.as_slice(),
                info.as_slice(),
            )?;
        }
        let block_number: packed::Uint64 = block.number().pack();
        self.insert_raw(
            COLUMN_INDEX::NAME,
            block_number.as_slice(),
            block_hash.as_slice(),
        )?;
        for uncle in block.uncles().into_iter() {
            self.insert_raw(
                COLUMN_UNCLES::NAME,
                uncle.hash().as_slice(),
                uncle.header().pack().as_slice(),
            )?;
        }
        self.insert_raw(
            COLUMN_INDEX::NAME,
            block_hash.as_slice(),
            block_number.as_slice(),
        )
    }

    /// TODO(doc): @quake
    pub fn detach_block(&self, block: &BlockView) -> Result<(), Error> {
        for tx_hash in block.tx_hashes().iter() {
            self.delete(COLUMN_TRANSACTION_INFO::NAME, tx_hash.as_slice())?;
        }
        for uncle in block.uncles().into_iter() {
            self.delete(COLUMN_UNCLES::NAME, uncle.hash().as_slice())?;
        }
        let block_number = block.data().header().raw().number();
        self.delete(COLUMN_INDEX::NAME, block_number.as_slice())?;
        self.delete(COLUMN_INDEX::NAME, block.hash().as_slice())
    }

    /// TODO(doc): @quake
    pub fn insert_block_epoch_index(
        &self,
        block_hash: &packed::Byte32,
        epoch_hash: &packed::Byte32,
    ) -> Result<(), Error> {
        self.insert_raw(
            COLUMN_BLOCK_EPOCH::NAME,
            block_hash.as_slice(),
            epoch_hash.as_slice(),
        )
    }

    /// TODO(doc): @quake
    pub fn insert_epoch_ext(&self, hash: &packed::Byte32, epoch: &EpochExt) -> Result<(), Error> {
        self.insert_raw(COLUMN_EPOCH::NAME, hash.as_slice(), epoch.pack().as_slice())?;
        let epoch_number: packed::Uint64 = epoch.number().pack();
        self.insert_raw(COLUMN_EPOCH::NAME, epoch_number.as_slice(), hash.as_slice())
    }

    /// TODO(doc): @quake
    pub fn insert_current_epoch_ext(&self, epoch: &EpochExt) -> Result<(), Error> {
        self.insert_raw(
            COLUMN_META::NAME,
            COLUMN_META::META_CURRENT_EPOCH_KEY,
            epoch.pack().as_slice(),
        )
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
            let block_number: BlockNumber = cell.block_number().unpack();
            self.insert_raw(
                COLUMN_CELL::NAME,
                COLUMN_CELL::key(block_number, &out_point).as_ref(),
                cell.as_slice(),
            )?;
            if let Some(data) = cell_data {
                self.insert_raw(
                    COLUMN_CELL_DATA::NAME,
                    COLUMN_CELL_DATA::key(block_number, &out_point).as_ref(),
                    data.as_slice(),
                )?;
                self.insert_raw(
                    COLUMN_CELL_DATA_HASH::NAME,
                    COLUMN_CELL_DATA_HASH::key(block_number, &out_point).as_ref(),
                    data.output_data_hash().as_slice(),
                )?;
            } else {
                self.insert_raw(
                    COLUMN_CELL_DATA::NAME,
                    COLUMN_CELL_DATA::key(block_number, &out_point).as_ref(),
                    &[],
                )?;
                self.insert_raw(
                    COLUMN_CELL_DATA_HASH::NAME,
                    COLUMN_CELL_DATA_HASH::key(block_number, &out_point).as_ref(),
                    &[],
                )?;
            }
        }
        Ok(())
    }

    /// TODO(doc): @quake
    pub fn delete_cells(
        &self,
        block_number: BlockNumber,
        out_points: impl Iterator<Item = packed::OutPoint>,
    ) -> Result<(), Error> {
        for out_point in out_points {
            self.delete(
                COLUMN_CELL::NAME,
                COLUMN_CELL::key(block_number, &out_point).as_ref(),
            )?;
            self.delete(
                COLUMN_CELL_DATA::NAME,
                COLUMN_CELL_DATA::key(block_number, &out_point).as_ref(),
            )?;
            self.delete(
                COLUMN_CELL_DATA_HASH::NAME,
                COLUMN_CELL_DATA_HASH::key(block_number, &out_point).as_ref(),
            )?;
        }
        Ok(())
    }

    /// Inserts a header digest.
    pub fn insert_header_digest(
        &self,
        position_u64: u64,
        header_digest: &packed::HeaderDigest,
    ) -> Result<(), Error> {
        self.insert_raw(
            COLUMN_CHAIN_ROOT_MMR::NAME,
            COLUMN_CHAIN_ROOT_MMR::key(position_u64).as_slice(),
            header_digest.as_slice(),
        )
    }

    /// Deletes a header digest.
    pub fn delete_header_digest(&self, position_u64: u64) -> Result<(), Error> {
        self.delete(
            COLUMN_CHAIN_ROOT_MMR::NAME,
            COLUMN_CHAIN_ROOT_MMR::key(position_u64).as_slice(),
        )
    }

    /// insert block filter data
    pub fn insert_block_filter(
        &self,
        num_hash: &BlockNumberAndHash,
        filter_data: &packed::Bytes,
        parent_block_filter_hash: &packed::Byte32,
    ) -> Result<(), Error> {
        self.insert_raw(
            COLUMN_BLOCK_FILTER::NAME,
            COLUMN_BLOCK_FILTER::key(num_hash.clone()).as_ref(),
            filter_data.as_slice(),
        )?;
        let current_block_filter_hash = calc_filter_hash(parent_block_filter_hash, filter_data);
        self.insert_raw(
            COLUMN_BLOCK_FILTER_HASH::NAME,
            COLUMN_BLOCK_FILTER_HASH::key(num_hash.clone()).as_ref(),
            current_block_filter_hash.as_slice(),
        )?;
        self.insert_raw(
            COLUMN_META::NAME,
            COLUMN_META::META_LATEST_BUILT_FILTER_DATA_KEY,
            num_hash.hash().as_slice(),
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
