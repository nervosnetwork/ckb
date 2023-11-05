use crate::store::SQLXPool;

use ckb_indexer_sync::{CustomFilters, Error, IndexerSync, Pool};
use ckb_types::{
    core::{BlockNumber, BlockView},
    packed::Byte32,
};
use std::sync::{Arc, RwLock};

/// IndexerR store wrapper
#[derive(Clone)]
pub(crate) struct IndexerR {
    /// storage
    _store: SQLXPool,
    /// number of blocks to keep for rollback and forking, for example:
    /// keep_num: 100, current tip: 321, will prune data where block_number <= 221
    _keep_num: u64,
    /// prune interval
    _prune_interval: u64,
    /// An optional overlay to index the pending txs in the ckb tx pool
    /// currently only supports removals of dead cells from the pending txs
    _pool: Option<Arc<RwLock<Pool>>>,
    /// custom filters
    _custom_filters: CustomFilters,
}

impl IndexerR {
    /// Construct new Indexer-R instance
    pub fn new(
        store: SQLXPool,
        keep_num: u64,
        prune_interval: u64,
        pool: Option<Arc<RwLock<Pool>>>,
        custom_filters: CustomFilters,
    ) -> Self {
        Self {
            _store: store,
            _keep_num: keep_num,
            _prune_interval: prune_interval,
            _pool: pool,
            _custom_filters: custom_filters,
        }
    }
}

impl IndexerSync for IndexerR {
    /// Retrieves the tip of the indexer
    fn tip(&self) -> Result<Option<(BlockNumber, Byte32)>, Error> {
        unimplemented!()
    }

    /// Appends a new block to the indexer
    fn append(&self, _block: &BlockView) -> Result<(), Error> {
        unimplemented!()
    }

    /// Rollback the indexer to a previous state
    fn rollback(&self) -> Result<(), Error> {
        unimplemented!()
    }
}
