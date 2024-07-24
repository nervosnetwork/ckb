use crate::cache::StoreCache;
use crate::cell::attach_block_cell;
use crate::store::ChainStore;
use crate::transaction::StoreTransaction;
use crate::write_batch::StoreWriteBatch;
use crate::StoreSnapshot;
use ckb_app_config::StoreConfig;
use ckb_chain_spec::{consensus::Consensus, versionbits::VersionbitsIndexer};
use ckb_db::{
    iter::{DBIter, DBIterator, IteratorMode},
    DBPinnableSlice, RocksDB,
};
use ckb_db_schema::{Col, COLUMN_META};
use ckb_error::{Error, InternalErrorKind};
use ckb_freezer::Freezer;
use ckb_types::{
    core::{BlockExt, EpochExt, HeaderView, TransactionView},
    packed,
    prelude::*,
    utilities::merkle_mountain_range::ChainRootMMR,
    BlockNumberAndHash,
};
use std::sync::Arc;

/// A database of the chain store based on the RocksDB wrapper `RocksDB`
#[derive(Clone)]
pub struct ChainDB {
    db: RocksDB,
    freezer: Option<Freezer>,
    cache: Arc<StoreCache>,
}

impl ChainStore for ChainDB {
    fn cache(&self) -> Option<&StoreCache> {
        Some(&self.cache)
    }

    fn freezer(&self) -> Option<&Freezer> {
        self.freezer.as_ref()
    }

    fn get(&self, col: Col, key: &[u8]) -> Option<DBPinnableSlice> {
        self.db
            .get_pinned(col, key)
            .expect("db operation should be ok")
    }

    fn get_iter(&self, col: Col, mode: IteratorMode) -> DBIter {
        self.db.iter(col, mode).expect("db operation should be ok")
    }
}

impl VersionbitsIndexer for ChainDB {
    fn block_epoch_index(&self, block_hash: &packed::Byte32) -> Option<packed::Byte32> {
        ChainStore::get_block_epoch_index(self, block_hash)
    }

    fn epoch_ext(&self, index: &packed::Byte32) -> Option<EpochExt> {
        ChainStore::get_epoch_ext(self, index)
    }

    fn block_header(&self, block_hash: &packed::Byte32) -> Option<HeaderView> {
        ChainStore::get_block_header(self, block_hash)
    }

    fn cellbase(&self, block_hash: &packed::Byte32) -> Option<TransactionView> {
        ChainStore::get_cellbase(self, block_hash)
    }
}

impl ChainDB {
    /// Allocate a new ChainDB instance with the given config
    pub fn new(db: RocksDB, config: StoreConfig) -> Self {
        let cache = StoreCache::from_config(config);
        ChainDB {
            db,
            freezer: None,
            cache: Arc::new(cache),
        }
    }

    /// Open new ChainDB with freezer instance
    pub fn new_with_freezer(db: RocksDB, freezer: Freezer, config: StoreConfig) -> Self {
        let cache = StoreCache::from_config(config);
        ChainDB {
            db,
            freezer: Some(freezer),
            cache: Arc::new(cache),
        }
    }

    /// Return the inner RocksDB instance
    pub fn db(&self) -> &RocksDB {
        &self.db
    }

    /// Converts self into a `RocksDB`
    pub fn into_inner(self) -> RocksDB {
        self.db
    }

    /// Store the chain spec hash
    pub fn put_chain_spec_hash(&self, hash: &packed::Byte32) -> Result<(), Error> {
        self.db
            .put_default(COLUMN_META::CHAIN_SPEC_HASH_KEY, hash.as_slice())
    }

    /// Return the chain spec hash
    pub fn get_chain_spec_hash(&self) -> Option<packed::Byte32> {
        self.db
            .get_pinned_default(COLUMN_META::CHAIN_SPEC_HASH_KEY)
            .expect("db operation should be ok")
            .map(|raw| packed::Byte32Reader::from_slice_should_be_ok(raw.as_ref()).to_entity())
    }

    /// Return the chain spec hash
    pub fn get_migration_version(&self) -> Option<DBPinnableSlice> {
        self.db
            .get_pinned_default(COLUMN_META::MIGRATION_VERSION_KEY)
            .expect("db operation should be ok")
    }

    /// Set this snapshot at start of transaction
    pub fn begin_transaction(&self) -> StoreTransaction {
        StoreTransaction {
            inner: self.db.transaction(),
            freezer: self.freezer.clone(),
            cache: Arc::clone(&self.cache),
        }
    }

    /// Return `StoreSnapshot`
    pub fn get_snapshot(&self) -> StoreSnapshot {
        StoreSnapshot {
            inner: self.db.get_snapshot(),
            freezer: self.freezer.clone(),
            cache: Arc::clone(&self.cache),
        }
    }

    /// Construct `StoreWriteBatch` with default option.
    pub fn new_write_batch(&self) -> StoreWriteBatch {
        StoreWriteBatch {
            inner: self.db.new_write_batch(),
        }
    }

    /// Write batch into chain db.
    pub fn write(&self, write_batch: &StoreWriteBatch) -> Result<(), Error> {
        self.db.write(&write_batch.inner)
    }

    /// write options set_sync = true
    ///
    /// see [`RocksDB::write_sync`](ckb_db::RocksDB::write_sync).
    pub fn write_sync(&self, write_batch: &StoreWriteBatch) -> Result<(), Error> {
        self.db.write_sync(&write_batch.inner)
    }

    /// Force the data to go through the compaction in order to consolidate it
    ///
    /// see [`RocksDB::compact_range`](ckb_db::RocksDB::compact_range).
    pub fn compact_range(
        &self,
        col: Col,
        start: Option<&[u8]>,
        end: Option<&[u8]>,
    ) -> Result<(), Error> {
        self.db.compact_range(col, start, end)
    }

    /// TODO(doc): @quake
    pub fn init(&self, consensus: &Consensus) -> Result<(), Error> {
        let genesis = consensus.genesis_block();
        let epoch = consensus.genesis_epoch_ext();
        let db_txn = self.begin_transaction();
        let genesis_hash = genesis.hash();
        let genesis_num_hash = BlockNumberAndHash::new(0, genesis_hash.clone());
        let ext = BlockExt {
            received_at: genesis.timestamp(),
            total_difficulty: genesis.difficulty(),
            total_uncles_count: 0,
            verified: Some(true),
            txs_fees: vec![],
            cycles: Some(vec![]),
            txs_sizes: Some(vec![]),
        };

        attach_block_cell(&db_txn, genesis)?;
        let last_block_hash_in_previous_epoch = epoch.last_block_hash_in_previous_epoch();

        db_txn.insert_block(genesis)?;
        db_txn.insert_block_ext(genesis_num_hash, &ext)?;
        db_txn.insert_tip_header(&genesis.header())?;
        db_txn.insert_current_epoch_ext(epoch)?;
        db_txn.insert_block_epoch_index(&genesis_hash, &last_block_hash_in_previous_epoch)?;
        db_txn.insert_epoch_ext(&last_block_hash_in_previous_epoch, epoch)?;
        db_txn.attach_block(genesis)?;

        let mut mmr = ChainRootMMR::new(0, &db_txn);
        mmr.push(genesis.digest())
            .map_err(|e| InternalErrorKind::MMR.other(e))?;
        mmr.commit().map_err(|e| InternalErrorKind::MMR.other(e))?;

        db_txn.commit()?;

        Ok(())
    }
}
