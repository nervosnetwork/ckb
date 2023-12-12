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
            let block_id = append_block(block, &mut tx).await?;
            self.insert_transactions(block_id, block, &mut tx).await?;
        } else {
            let block_headers = vec![(block.hash().raw_data().to_vec(), block.number() as i64)];
            bulk_insert_blocks_simple(&block_headers, &mut tx).await?;
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
        block_id: i64,
        block_view: &BlockView,
        tx: &mut Transaction<'_, Any>,
    ) -> Result<(), Error> {
        for (tx_index, tx_view) in block_view.transactions().into_iter().enumerate() {
            self.insert_transaction(block_id, tx_index, tx_view, tx)
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn insert_transaction(
        &self,
        block_id: i64,
        tx_index: usize,
        tx_view: TransactionView,
        tx: &mut Transaction<'_, Any>,
    ) -> Result<(), Error> {
        let mut is_tx_matched = false;
        let mut output_cell_rows = Vec::new();
        let mut input_rows = Vec::new();
        let mut script_set = HashSet::new();

        for (output_index, (cell, data)) in tx_view.outputs_with_data_iter().enumerate() {
            if self
                .custom_filters
                .is_cell_filter_match(&cell, &data.pack())
            {
                build_output_cell_rows(
                    &cell,
                    &tx_view,
                    output_index,
                    &data,
                    &mut output_cell_rows,
                )?;
                build_script_set(&cell, &mut script_set).await?;
                is_tx_matched = true;
            }
        }

        if tx_index != 0 {
            for (input_index, input) in tx_view.inputs().into_iter().enumerate() {
                let mut is_match = true;
                if self.custom_filters.is_cell_filter_enabled() {
                    let out_point = input.previous_output();
                    let (output, output_data) =
                        query_cell_output(&out_point, tx)
                            .await?
                            .ok_or(Error::DB(format!(
                                "Failed to query output by out_point: {:?}",
                                out_point
                            )))?;
                    is_match = self
                        .custom_filters
                        .is_cell_filter_match(&output, &output_data.pack());
                }
                if is_match {
                    build_input_rows(&tx_view, &input, input_index, &mut input_rows)?;
                    is_tx_matched = true;
                }
            }
        }

        if !is_tx_matched {
            return Ok(());
        }

        let tx_id = insert_transaction_table(block_id, tx_index, &tx_view, tx).await?;
        bulk_insert_tx_association_header_dep_table(tx_id, &tx_view, tx).await?;
        bulk_insert_tx_association_cell_dep_table(tx_id, &tx_view, tx).await?;

        bulk_insert_output_table(&output_cell_rows, tx).await?;
        bulk_insert_script_table(&script_set, tx).await?;
        bulk_insert_input_table(&input_rows, tx).await?;

        Ok(())
    }
}
