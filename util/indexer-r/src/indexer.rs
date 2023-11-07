#![allow(clippy::needless_borrow)]

use crate::{store::SQLXPool, IndexerRHandle};

use ckb_async_runtime::Handle;
use ckb_indexer_sync::{CustomFilters, Error, IndexerSync, Pool};
use ckb_types::{
    core::{BlockNumber, BlockView},
    packed::Byte32,
    prelude::*,
};
use seq_macro::seq;
use sql_builder::SqlBuilder;
use sqlx::{Any, Transaction};

use std::sync::{Arc, RwLock};

// Note that every database has a practical limit on the number of bind parameters you can add to a single query.
// This varies by database.
// https://docs.rs/sqlx/0.6.3/sqlx/struct.QueryBuilder.html#note-database-specific-limits
// BATCH_SIZE_THRESHOLD represents the number of rows that can be bound in an insert sql execution.
// The number of columns in each row multiplied by this BATCH_SIZE_THRESHOLD yields the total number of bound parameters,
// which should be within the above limits.
pub(crate) const BATCH_SIZE_THRESHOLD: usize = 1_000;

/// Handle to the indexer-r.
///
/// The handle is internally reference-counted and can be freely cloned.
/// A handle can be obtained using the IndexerRService::handle method.
#[derive(Clone)]
pub struct IndexerR {
    async_indexer_r: AsyncIndexerR,
    async_runtime: Handle,
}

impl IndexerR {
    /// Construct new IndexerR instance
    pub fn new(
        store: SQLXPool,
        keep_num: u64,
        prune_interval: u64,
        pool: Option<Arc<RwLock<Pool>>>,
        custom_filters: CustomFilters,
        async_runtime: Handle,
    ) -> Self {
        Self {
            async_indexer_r: AsyncIndexerR::new(
                store,
                keep_num,
                prune_interval,
                pool,
                custom_filters,
            ),
            async_runtime,
        }
    }
}

impl IndexerSync for IndexerR {
    /// Retrieves the tip of the indexer
    fn tip(&self) -> Result<Option<(BlockNumber, Byte32)>, Error> {
        let indexer_handle = IndexerRHandle::new(
            self.async_indexer_r.store.clone(),
            self.async_indexer_r.pool.clone(),
            self.async_runtime.clone(),
        );
        indexer_handle
            .get_indexer_tip()
            .map(|tip| tip.map(|tip| (tip.block_number.value(), tip.block_hash.0.pack())))
            .map_err(|err| Error::DB(err.to_string()))
    }

    /// Appends a new block to the indexer
    fn append(&self, block: &BlockView) -> Result<(), Error> {
        let future = self.async_indexer_r.append_block(block);
        self.async_runtime.block_on(future)
    }

    /// Rollback the indexer to a previous state
    fn rollback(&self) -> Result<(), Error> {
        let future = self.async_indexer_r.rollback();
        self.async_runtime.block_on(future)
    }
}

/// Async handle to the indexer-r.
#[derive(Clone)]
pub(crate) struct AsyncIndexerR {
    /// storage
    store: SQLXPool,
    /// number of blocks to keep for rollback and forking, for example:
    /// keep_num: 100, current tip: 321, will prune data where block_number <= 221
    _keep_num: u64,
    /// prune interval
    _prune_interval: u64,
    /// An optional overlay to index the pending txs in the ckb tx pool
    /// currently only supports removals of dead cells from the pending txs
    pool: Option<Arc<RwLock<Pool>>>,
    /// custom filters
    _custom_filters: CustomFilters,
}

impl AsyncIndexerR {
    /// Construct new AsyncIndexerR instance
    pub fn new(
        store: SQLXPool,
        keep_num: u64,
        prune_interval: u64,
        pool: Option<Arc<RwLock<Pool>>>,
        custom_filters: CustomFilters,
    ) -> Self {
        Self {
            store,
            _keep_num: keep_num,
            _prune_interval: prune_interval,
            pool,
            _custom_filters: custom_filters,
        }
    }
}

impl AsyncIndexerR {
    pub(crate) async fn append_block(&self, block: &BlockView) -> Result<(), Error> {
        let mut tx = self
            .store
            .transaction()
            .await
            .map_err(|err| Error::DB(err.to_string()))?;
        self.insert_block_table(block, &mut tx).await?;
        self.insert_transaction_table(block, &mut tx).await?;
        tx.commit().await.map_err(|err| Error::DB(err.to_string()))
    }

    pub(crate) async fn rollback(&self) -> Result<(), Error> {
        unimplemented!("rollback")
    }

    async fn insert_block_table(
        &self,
        block_view: &BlockView,
        tx: &mut Transaction<'_, Any>,
    ) -> Result<(), Error> {
        bulk_insert_blocks(&[block_view.to_owned()], tx).await
    }

    async fn insert_transaction_table(
        &self,
        _block: &BlockView,
        _tx: &mut Transaction<'_, Any>,
    ) -> Result<(), Error> {
        Ok(())
    }
}

pub async fn bulk_insert_blocks(
    block_views: &[BlockView],
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let block_rows: Vec<_> = block_views
        .iter()
        .map(|block_view| {
            (
                block_view.hash().raw_data().to_vec(),
                block_view.number() as i32,
                block_view.compact_target() as i32,
                block_view.parent_hash().raw_data().to_vec(),
                block_view.nonce().to_be_bytes().to_vec(),
                block_view.timestamp() as i64,
                block_view.version() as i16,
                block_view.transactions_root().raw_data().to_vec(),
                block_view.epoch().number() as i32,
                block_view.epoch().index() as i32,
                block_view.epoch().length() as i32,
                block_view.dao().raw_data().to_vec(),
                block_view.proposals_hash().raw_data().to_vec(),
                block_view.calc_uncles_hash().raw_data().to_vec(),
                block_view.extra_hash().raw_data().to_vec(),
            )
        })
        .collect();

    for start in (0..block_rows.len()).step_by(BATCH_SIZE_THRESHOLD) {
        let end = (start + BATCH_SIZE_THRESHOLD).min(block_rows.len());

        // insert mercury_block
        // build query str
        let mut builder = SqlBuilder::insert_into("block");
        builder.field(
            r#"
                block_hash,
                block_number,
                compact_target,
                parent_hash,
                nonce,
                timestamp,
                version,
                transactions_root,
                epoch_number,
                epoch_index,
                epoch_length,
                dao,
                proposals_hash,
                uncles_hash,
                extra_hash"#,
        );
        push_values_placeholders(&mut builder, 15, end - start);
        let sql = builder
            .sql()
            .map_err(|err| Error::DB(err.to_string()))?
            .trim_end_matches(';')
            .to_string();

        // bind
        let mut query = SQLXPool::new_query(&sql);
        for row in block_rows[start..end].iter() {
            seq!(i in 0..15 {
                query = query.bind(&row.i);
            });
        }

        // execute
        query
            .execute(&mut *tx)
            .await
            .map_err(|err| Error::DB(err.to_string()))?;
    }
    Ok(())
}

pub fn push_values_placeholders(
    builder: &mut SqlBuilder,
    column_number: usize,
    rows_number: usize,
) {
    let mut placeholder_idx = 1usize;
    for _ in 0..rows_number {
        let values = (placeholder_idx..placeholder_idx + column_number)
            .map(|i| format!("${}", i))
            .collect::<Vec<String>>();
        builder.values(&values);
        placeholder_idx += column_number;
    }
}
