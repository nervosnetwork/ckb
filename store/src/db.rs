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
    Col, DBPinnableSlice, RocksDB,
};
use ckb_error::Error;
use ckb_types::core::BlockExt;
use std::sync::Arc;

/// TODO(doc): @quake
#[derive(Clone)]
pub struct ChainDB {
    db: RocksDB,
    cache: Arc<StoreCache>,
}

impl<'a> ChainStore<'a> for ChainDB {
    type Vector = DBPinnableSlice<'a>;

    fn cache(&'a self) -> Option<&'a StoreCache> {
        Some(&self.cache)
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

    /// TODO(doc): @quake
    pub fn begin_transaction(&self) -> StoreTransaction {
        StoreTransaction {
            inner: self.db.transaction(),
            cache: Arc::clone(&self.cache),
        }
    }

    /// TODO(doc): @quake
    pub fn get_snapshot(&self) -> StoreSnapshot {
        StoreSnapshot {
            inner: self.db.get_snapshot(),
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

        attach_block_cell(&db_txn, &genesis)?;
        let last_block_hash_in_previous_epoch = epoch.last_block_hash_in_previous_epoch();

        db_txn.insert_block(genesis)?;
        db_txn.insert_block_ext(&genesis_hash, &ext)?;
        db_txn.insert_tip_header(&genesis.header())?;
        db_txn.insert_current_epoch_ext(epoch)?;
        db_txn.insert_block_epoch_index(&genesis_hash, &last_block_hash_in_previous_epoch)?;
        db_txn.insert_epoch_ext(&last_block_hash_in_previous_epoch, &epoch)?;
        db_txn.attach_block(genesis)?;
        db_txn.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::COLUMNS;
    use super::*;
    use ckb_chain_spec::consensus::ConsensusBuilder;
    use ckb_db::RocksDB;
    use ckb_types::{packed, prelude::*};

    fn setup_db(columns: u32) -> RocksDB {
        RocksDB::open_tmp(columns)
    }

    #[test]
    fn save_and_get_block() {
        let db = setup_db(COLUMNS);
        let store = ChainDB::new(db, Default::default());
        let consensus = ConsensusBuilder::default().build();
        let block = consensus.genesis_block();

        let hash = block.hash();
        let txn = store.begin_transaction();
        txn.insert_block(&block).unwrap();
        txn.commit().unwrap();
        assert_eq!(block, &store.get_block(&hash).unwrap());
    }

    #[test]
    fn save_and_get_block_with_transactions() {
        let db = setup_db(COLUMNS);
        let store = ChainDB::new(db, Default::default());
        let block = packed::Block::new_builder()
            .transactions(
                (0..3)
                    .map(|_| packed::Transaction::new_builder().build())
                    .collect::<Vec<_>>()
                    .pack(),
            )
            .build()
            .into_view();

        let hash = block.hash();
        let txn = store.begin_transaction();
        txn.insert_block(&block).unwrap();
        txn.commit().unwrap();
        assert_eq!(block, store.get_block(&hash).unwrap());
    }

    #[test]
    fn save_and_get_block_ext() {
        let db = setup_db(COLUMNS);
        let store = ChainDB::new(db, Default::default());
        let consensus = ConsensusBuilder::default().build();
        let block = consensus.genesis_block();

        let ext = BlockExt {
            received_at: block.timestamp(),
            total_difficulty: block.difficulty(),
            total_uncles_count: block.data().uncles().len() as u64,
            verified: Some(true),
            txs_fees: vec![],
        };

        let hash = block.hash();
        let txn = store.begin_transaction();
        txn.insert_block_ext(&hash, &ext).unwrap();
        txn.commit().unwrap();
        assert_eq!(ext, store.get_block_ext(&hash).unwrap());
    }

    #[test]
    fn index_store() {
        let db = RocksDB::open_tmp(COLUMNS);
        let store = ChainDB::new(db, Default::default());
        let consensus = ConsensusBuilder::default().build();
        let block = consensus.genesis_block();
        let hash = block.hash();
        store.init(&consensus).unwrap();
        assert_eq!(hash, store.get_block_hash(0).unwrap());

        assert_eq!(
            block.difficulty(),
            store.get_block_ext(&hash).unwrap().total_difficulty
        );

        assert_eq!(block.number(), store.get_block_number(&hash).unwrap());

        assert_eq!(block.header(), store.get_tip_header().unwrap());
    }
}
