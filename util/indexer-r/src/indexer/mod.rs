mod insert;
mod remove;

pub(crate) use insert::*;
pub(crate) use remove::*;

use crate::{service::SUBSCRIBER_NAME, store::SQLXPool, AsyncIndexerRHandle, IndexerRHandle};

use ckb_async_runtime::Handle;
use ckb_indexer_sync::{CustomFilters, Error, IndexerSync, Pool};
use ckb_types::{
    core::{BlockNumber, BlockView, TransactionView},
    packed::Byte32,
    prelude::*,
};
use sqlx::{Any, Transaction};

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

/// the database tables are as follows:
///
/// - block
/// - tx
/// - input
/// - output
/// - script
/// - block_association_proposal
/// - block_association_uncle
/// - tx_association_header_dep
/// - tx_association_cell_dep
/// The detailed table design can be found in the SQL files in the resources folder of this crate

/// Indexer-r, which is based on a relational database
#[derive(Clone)]
pub(crate) struct IndexerR {
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
        let future = self.async_indexer_r.append(block);
        self.async_runtime.block_on(future)
    }

    /// Appends new blocks to the indexer
    fn append_bulk(&self, block: &[BlockView]) -> Result<(), Error> {
        let future = self.async_indexer_r.append_bulk(block);
        self.async_runtime.block_on(future)
    }

    /// Rollback the indexer to a previous state
    fn rollback(&self) -> Result<(), Error> {
        let future = self.async_indexer_r.rollback();
        self.async_runtime.block_on(future)
    }

    /// Return identity
    fn get_identity(&self) -> &str {
        SUBSCRIBER_NAME
    }
}

/// Async indexer-r.
#[derive(Clone)]
pub(crate) struct AsyncIndexerR {
    /// storage
    pub(crate) store: SQLXPool,
    /// number of blocks to keep for rollback and forking, for example:
    /// keep_num: 100, current tip: 321, will prune data where block_number <= 221
    _keep_num: u64,
    /// prune interval
    _prune_interval: u64,
    /// An optional overlay to index the pending txs in the ckb tx pool
    /// currently only supports removals of dead cells from the pending txs
    pub(crate) pool: Option<Arc<RwLock<Pool>>>,
    /// custom filters
    custom_filters: CustomFilters,
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
            custom_filters,
        }
    }
}

impl AsyncIndexerR {
    pub(crate) async fn append(&self, block: &BlockView) -> Result<(), Error> {
        let mut tx = self
            .store
            .transaction()
            .await
            .map_err(|err| Error::DB(err.to_string()))?;
        if self.custom_filters.is_block_filter_match(block) {
            append_blocks(&vec![block.clone()], &mut tx).await?;
            self.insert_transactions(&vec![block.clone()], &mut tx)
                .await?;
        } else {
            append_block_header(&block.hash().raw_data(), block.number() as i64, &mut tx).await?;
        }
        tx.commit().await.map_err(|err| Error::DB(err.to_string()))
    }

    pub(crate) async fn append_bulk(&self, blocks: &[BlockView]) -> Result<(), Error> {
        let mut tx = self
            .store
            .transaction()
            .await
            .map_err(|err| Error::DB(err.to_string()))?;

        let mut block_groups = Vec::new();
        let mut current_group = Vec::new();
        let mut current_match = self.custom_filters.is_block_filter_match(&blocks[0]);
        for block in blocks {
            let is_match = self.custom_filters.is_block_filter_match(block);
            if is_match == current_match {
                current_group.push(block.clone());
            } else {
                block_groups.push((current_match, current_group));
                current_group = vec![block.clone()];
                current_match = is_match;
            }
        }
        if !current_group.is_empty() {
            block_groups.push((current_match, current_group));
        }

        for (is_match, blocks) in block_groups {
            if is_match {
                append_blocks(&blocks, &mut tx).await?;
                self.insert_transactions(&blocks, &mut tx).await?;
            } else {
                let block_headers: Vec<(_, _)> = blocks
                    .iter()
                    .map(|block| (block.hash().raw_data().to_vec(), block.number() as i64))
                    .collect();
                append_block_headers(&block_headers, &mut tx).await?;
            }
        }

        tx.commit().await.map_err(|err| Error::DB(err.to_string()))
    }

    pub(crate) async fn rollback(&self) -> Result<(), Error> {
        let indexer_handle = AsyncIndexerRHandle::new(self.store.clone(), self.pool.clone());
        let tip = indexer_handle
            .query_indexer_tip()
            .await
            .map_err(|err| Error::DB(err.to_string()))?;

        if let Some(tip) = tip {
            let mut tx = self
                .store
                .transaction()
                .await
                .map_err(|err| Error::DB(err.to_string()))?;

            rollback_block(tip.block_hash, &mut tx).await?;

            return tx.commit().await.map_err(|err| Error::DB(err.to_string()));
        }

        Ok(())
    }

    pub(crate) async fn insert_transactions(
        &self,
        block_views: &[BlockView],
        tx: &mut Transaction<'_, Any>,
    ) -> Result<(), Error> {
        let tx_views = block_views
            .into_iter()
            .flat_map(|block_view| {
                let block_hash = block_view.hash().raw_data().to_vec();
                block_view
                    .transactions()
                    .into_iter()
                    .enumerate()
                    .map(move |(tx_index, tx_view)| (block_hash.clone(), tx_index, tx_view))
            })
            .collect::<Vec<(_, _, _)>>();

        let mut matched_tx_hashes = HashSet::new();
        let mut output_cell_rows = Vec::new();
        let mut input_rows = Vec::new();
        let mut output_association_script_rows = Vec::new();
        let mut script_set = HashSet::new();

        for (_, _, tx_view) in tx_views.iter() {
            for ((output_index, (cell, data)), out_point) in tx_view
                .outputs_with_data_iter()
                .enumerate()
                .zip(tx_view.output_pts_iter())
            {
                if self
                    .custom_filters
                    .is_cell_filter_match(&cell, &data.pack())
                {
                    build_output_cell_rows(
                        &cell,
                        tx_view,
                        output_index,
                        &data,
                        &mut output_cell_rows,
                    )?;
                    build_script_set(&cell, &mut script_set, tx).await?;
                    build_output_association_script_rows(
                        &cell,
                        &out_point,
                        &mut output_association_script_rows,
                    )?;
                    matched_tx_hashes.insert(tx_view.hash());
                }
            }
        }
        bulk_insert_output_table(&output_cell_rows, tx).await?;
        bulk_insert_script_table(&script_set, tx).await?;

        // The output needs to be inserted into the db-transaction before the input traversal.
        // This is to cope with the case where the output is spent in a transaction in the same block,
        // because the input needs to query the corresponding output cell when applying a cell filter.
        //
        // Skip the first cellbase transaction.
        for (_, _, tx_view) in tx_views.iter().skip(1) {
            for (input_index, input) in tx_view.inputs().into_iter().enumerate() {
                let mut is_match = true;
                if self.custom_filters.is_cell_filter_enabled() {
                    let out_point = input.previous_output();
                    if let Some((output, output_data)) = query_cell_output(&out_point, tx).await? {
                        is_match = self
                            .custom_filters
                            .is_cell_filter_match(&output, &output_data.pack());
                    } else {
                        is_match = false;
                    }
                }
                if is_match {
                    build_input_rows(tx_view, &input, input_index, &mut input_rows)?;
                    matched_tx_hashes.insert(tx_view.hash());
                }
            }
        }
        bulk_insert_input_table(&input_rows, tx).await?;

        let matched_txs_iter: Vec<(_, _, TransactionView)> = tx_views
            .into_iter()
            .filter(|(_, _, tx)| matched_tx_hashes.contains(&tx.hash()))
            .collect();

        bulk_insert_transaction_table(matched_txs_iter.iter(), tx).await?;
        bulk_insert_tx_association_header_dep_table(matched_txs_iter.iter(), tx).await?;
        bulk_insert_tx_association_cell_dep_table(matched_txs_iter.iter(), tx).await?;

        Ok(())
    }
}
