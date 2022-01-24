use crate::cache::StoreCache;
use crate::cell::attach_block_cell;
use crate::store::ChainStore;
use crate::transaction::StoreTransaction;
use crate::write_batch::StoreWriteBatch;
use crate::StoreSnapshot;
use ckb_app_config::StoreConfig;
use ckb_chain_spec::consensus::Consensus;
use ckb_db::{
    iter::{DBIter, DBIterator, IteratorMode},
    DBPinnableSlice, RocksDB,
};
use ckb_db_schema::{Col, CHAIN_SPEC_HASH_KEY, MIGRATION_VERSION_KEY};
use ckb_error::Error;
use ckb_freezer::Freezer;
use ckb_types::{core::BlockExt, packed, prelude::*};
use std::sync::Arc;

/// TODO(doc): @quake
#[derive(Clone)]
pub struct ChainDB {
    db: RocksDB,
    freezer: Option<Freezer>,
    cache: Arc<StoreCache>,
}

impl<'a> ChainStore<'a> for ChainDB {
    type Vector = DBPinnableSlice<'a>;

    fn cache(&'a self) -> Option<&'a StoreCache> {
        Some(&self.cache)
    }

    fn freezer(&'a self) -> Option<&'a Freezer> {
        self.freezer.as_ref()
    }

    fn get(&'a self, col: Col, key: &[u8]) -> Option<Self::Vector> {
        self.db
            .get_pinned(col, key)
            .expect("db operation should be ok")
    }

    fn get_iter(&self, col: Col, mode: IteratorMode) -> DBIter {
        self.db.iter(col, mode).expect("db operation should be ok")
    }
}

impl ChainDB {
    /// TODO(doc): @quake
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

    /// TODO(doc): @quake
    pub fn db(&self) -> &RocksDB {
        &self.db
    }

    /// TODO(doc): @quake
    pub fn into_inner(self) -> RocksDB {
        self.db
    }

    /// Store the chain spec hash
    pub fn put_chain_spec_hash(&self, hash: &packed::Byte32) -> Result<(), Error> {
        self.db.put_default(CHAIN_SPEC_HASH_KEY, hash.as_slice())
    }

    /// Return the chain spec hash
    pub fn get_chain_spec_hash(&self) -> Option<packed::Byte32> {
        self.db
            .get_pinned_default(CHAIN_SPEC_HASH_KEY)
            .expect("db operation should be ok")
            .map(|raw| packed::Byte32Reader::from_slice_should_be_ok(raw.as_ref()).to_entity())
    }

    /// Return the chain spec hash
    pub fn get_migration_version(&self) -> Option<DBPinnableSlice> {
        self.db
            .get_pinned_default(MIGRATION_VERSION_KEY)
            .expect("db operation should be ok")
    }

    /// TODO(doc): @quake
    pub fn begin_transaction(&self) -> StoreTransaction {
        StoreTransaction {
            inner: self.db.transaction(),
            freezer: self.freezer.clone(),
            cache: Arc::clone(&self.cache),
        }
    }

    /// TODO(doc): @quake
    pub fn get_snapshot(&self) -> StoreSnapshot {
        StoreSnapshot {
            inner: self.db.get_snapshot(),
            freezer: self.freezer.clone(),
            cache: Arc::clone(&self.cache),
        }
    }

    /// TODO(doc): @quake
    pub fn new_write_batch(&self) -> StoreWriteBatch {
        StoreWriteBatch {
            inner: self.db.new_write_batch(),
        }
    }

    /// TODO(doc): @quake
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
        let ext = BlockExt {
            received_at: genesis.timestamp(),
            total_difficulty: genesis.difficulty(),
            total_uncles_count: 0,
            verified: Some(true),
            txs_fees: vec![],
        };

        attach_block_cell(&db_txn, genesis)?;
        let last_block_hash_in_previous_epoch = epoch.last_block_hash_in_previous_epoch();

        db_txn.insert_block(genesis)?;
        db_txn.insert_block_ext(&genesis_hash, &ext)?;
        db_txn.insert_tip_header(&genesis.header())?;
        db_txn.insert_current_epoch_ext(epoch)?;
        db_txn.insert_block_epoch_index(&genesis_hash, &last_block_hash_in_previous_epoch)?;
        db_txn.insert_epoch_ext(&last_block_hash_in_previous_epoch, epoch)?;
        db_txn.attach_block(genesis)?;
        db_txn.commit()?;
        Ok(())
    }
}
