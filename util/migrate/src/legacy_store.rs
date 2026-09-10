use ckb_db::{
    DBPinnableSlice, Direction, IteratorMode, RocksDB, RocksDBTransaction, RocksDBWriteBatch,
    iter::{DBIter, DBIterator},
};
use ckb_db_schema::{Col, META_LATEST_BUILT_FILTER_DATA_KEY, META_TIP_HEADER_KEY, legacy};
use ckb_error::Error;
use ckb_merkle_mountain_range::{Error as MMRError, MMRStore, Result as MMRResult};
use ckb_types::{
    core::{BlockExt, BlockNumber, BlockView, EpochExt, HeaderView, TransactionView},
    packed,
    prelude::*,
};

#[derive(Clone)]
pub(crate) struct LegacyChainDB {
    db: RocksDB,
}

pub(crate) struct LegacyStoreTransaction {
    inner: RocksDBTransaction,
}

impl LegacyChainDB {
    pub(crate) fn new(db: RocksDB) -> Self {
        Self { db }
    }

    pub(crate) fn get(&self, col: Col, key: &[u8]) -> Option<DBPinnableSlice<'_>> {
        self.db
            .get_pinned(col, key)
            .expect("db operation should be ok")
    }

    pub(crate) fn get_iter(&self, col: Col, mode: IteratorMode) -> DBIter<'_> {
        self.db.iter(col, mode).expect("db operation should be ok")
    }

    pub(crate) fn new_write_batch(&self) -> RocksDBWriteBatch {
        self.db.new_write_batch()
    }

    pub(crate) fn write(&self, write_batch: &RocksDBWriteBatch) -> Result<(), Error> {
        self.db.write(write_batch)
    }

    pub(crate) fn begin_transaction(&self) -> LegacyStoreTransaction {
        LegacyStoreTransaction {
            inner: self.db.transaction(),
        }
    }

    pub(crate) fn db(&self) -> &RocksDB {
        &self.db
    }

    pub(crate) fn into_inner(self) -> RocksDB {
        self.db
    }

    pub(crate) fn get_tip_header(&self) -> Option<HeaderView> {
        self.get(legacy::COLUMN_META, META_TIP_HEADER_KEY)
            .and_then(|raw| {
                let hash = packed::Byte32Reader::from_slice_should_be_ok(raw.as_ref()).to_entity();
                self.get_block_header(&hash)
            })
    }

    pub(crate) fn get_block_hash(&self, number: BlockNumber) -> Option<packed::Byte32> {
        let block_number: packed::Uint64 = number.into();
        self.get(legacy::COLUMN_INDEX, block_number.as_slice())
            .map(|raw| packed::Byte32Reader::from_slice_should_be_ok(raw.as_ref()).to_entity())
    }

    pub(crate) fn get_block_number(&self, hash: &packed::Byte32) -> Option<BlockNumber> {
        self.get(legacy::COLUMN_INDEX, hash.as_slice())
            .map(|raw| packed::Uint64Reader::from_slice_should_be_ok(raw.as_ref()).into())
    }

    pub(crate) fn is_main_chain(&self, hash: &packed::Byte32) -> bool {
        self.get(legacy::COLUMN_INDEX, hash.as_slice()).is_some()
    }

    pub(crate) fn get_block_header(&self, hash: &packed::Byte32) -> Option<HeaderView> {
        self.get(legacy::COLUMN_BLOCK_HEADER, hash.as_slice())
            .map(|slice| {
                let reader = packed::HeaderViewReader::from_slice_should_be_ok(slice.as_ref());
                reader.into()
            })
    }

    pub(crate) fn get_block_body(&self, hash: &packed::Byte32) -> Vec<TransactionView> {
        let prefix = hash.as_slice();
        self.get_iter(
            legacy::COLUMN_BLOCK_BODY,
            IteratorMode::From(prefix, Direction::Forward),
        )
        .take_while(|(key, _)| key.starts_with(prefix))
        .map(|(_, value)| {
            let reader = packed::TransactionViewReader::from_slice_should_be_ok(value.as_ref());
            reader.into()
        })
        .collect()
    }

    pub(crate) fn get_block(&self, hash: &packed::Byte32) -> Option<BlockView> {
        let header = self.get_block_header(hash)?;
        let body = self.get_block_body(hash);
        let uncles = self
            .get(legacy::COLUMN_BLOCK_UNCLE, hash.as_slice())
            .map(|slice| {
                let reader =
                    packed::UncleBlockVecViewReader::from_slice_should_be_ok(slice.as_ref());
                reader.into()
            })
            .expect("block uncles must be stored");
        let proposals = self
            .get(legacy::COLUMN_BLOCK_PROPOSAL_IDS, hash.as_slice())
            .map(|slice| {
                packed::ProposalShortIdVecReader::from_slice_should_be_ok(slice.as_ref())
                    .to_entity()
            })
            .expect("block proposal_ids must be stored");
        let extension_opt = self
            .get(legacy::COLUMN_BLOCK_EXTENSION, hash.as_slice())
            .map(|slice| packed::BytesReader::from_slice_should_be_ok(slice.as_ref()).to_entity());

        Some(if let Some(extension) = extension_opt {
            BlockView::new_unchecked_with_extension(header, uncles, body, proposals, extension)
        } else {
            BlockView::new_unchecked(header, uncles, body, proposals)
        })
    }

    pub(crate) fn get_epoch_ext(&self, hash: &packed::Byte32) -> Option<EpochExt> {
        self.get(legacy::COLUMN_EPOCH, hash.as_slice())
            .map(|slice| packed::EpochExtReader::from_slice_should_be_ok(slice.as_ref()).into())
    }

    pub(crate) fn get_block_filter(&self, hash: &packed::Byte32) -> Option<packed::Bytes> {
        self.get(legacy::COLUMN_BLOCK_FILTER, hash.as_slice())
            .map(|slice| packed::BytesReader::from_slice_should_be_ok(slice.as_ref()).to_entity())
    }

    pub(crate) fn get_latest_built_filter_data_block_hash(&self) -> Option<packed::Byte32> {
        self.get(legacy::COLUMN_META, META_LATEST_BUILT_FILTER_DATA_KEY)
            .map(|raw| packed::Byte32Reader::from_slice_should_be_ok(raw.as_ref()).to_entity())
    }
}

impl LegacyStoreTransaction {
    pub(crate) fn commit(&self) -> Result<(), Error> {
        self.inner.commit()
    }

    pub(crate) fn get_block_header(&self, hash: &packed::Byte32) -> Option<HeaderView> {
        self.inner
            .get_pinned(legacy::COLUMN_BLOCK_HEADER, hash.as_slice())
            .expect("db operation should be ok")
            .map(|slice| {
                let reader = packed::HeaderViewReader::from_slice_should_be_ok(slice.as_ref());
                reader.into()
            })
    }

    pub(crate) fn get_block_ext(&self, hash: &packed::Byte32) -> Option<BlockExt> {
        self.inner
            .get_pinned(legacy::COLUMN_BLOCK_EXT, hash.as_slice())
            .expect("db operation should be ok")
            .map(block_ext_from_slice)
    }

    pub(crate) fn insert_block_ext(
        &self,
        hash: &packed::Byte32,
        ext: &BlockExt,
    ) -> Result<(), Error> {
        let packed_ext: packed::BlockExtV1 = ext.into();
        self.inner.put(
            legacy::COLUMN_BLOCK_EXT,
            hash.as_slice(),
            packed_ext.as_slice(),
        )
    }

    fn get_header_digest(&self, position_u64: u64) -> Option<packed::HeaderDigest> {
        let position: packed::Uint64 = position_u64.into();
        self.inner
            .get_pinned(legacy::COLUMN_CHAIN_ROOT_MMR, position.as_slice())
            .expect("db operation should be ok")
            .map(|slice| {
                let reader = packed::HeaderDigestReader::from_slice_should_be_ok(slice.as_ref());
                reader.to_entity()
            })
    }

    fn insert_header_digest(
        &self,
        position_u64: u64,
        header_digest: &packed::HeaderDigest,
    ) -> Result<(), Error> {
        let position: packed::Uint64 = position_u64.into();
        self.inner.put(
            legacy::COLUMN_CHAIN_ROOT_MMR,
            position.as_slice(),
            header_digest.as_slice(),
        )
    }
}

impl MMRStore<packed::HeaderDigest> for &LegacyStoreTransaction {
    fn get_elem(&self, pos: u64) -> MMRResult<Option<packed::HeaderDigest>> {
        Ok(self.get_header_digest(pos))
    }

    fn append(&mut self, pos: u64, elems: Vec<packed::HeaderDigest>) -> MMRResult<()> {
        for (offset, elem) in elems.iter().enumerate() {
            let pos = pos + offset as u64;
            self.insert_header_digest(pos, elem).map_err(|err| {
                MMRError::StoreError(format!("Failed to append to MMR, DB error {err}"))
            })?;
        }
        Ok(())
    }
}

fn block_ext_from_slice(slice: impl AsRef<[u8]>) -> BlockExt {
    let slice = slice.as_ref();
    let reader = packed::BlockExtReader::from_compatible_slice_should_be_ok(slice);
    match reader.count_extra_fields() {
        0 => reader.into(),
        2 => packed::BlockExtV1Reader::from_slice_should_be_ok(slice).into(),
        _ => {
            panic!(
                "BlockExt storage field count doesn't match, expect 7 or 5, actual {}",
                reader.field_count()
            )
        }
    }
}
