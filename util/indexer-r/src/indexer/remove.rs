use super::*;
use crate::indexer_handle::sqlx_param_placeholders;
use crate::AsyncIndexerRHandle;

use ckb_indexer_sync::Error;
use ckb_types::H256;
use sql_builder::SqlBuilder;
use sqlx::{Any, Transaction};

const COLUMN_BLOCK_HASH: &str = "block_hash";
const COLUMN_TX_HASH: &str = "tx_hash";

pub(crate) async fn rollback_block(
    block_hash: H256,
    indexer_handle: AsyncIndexerRHandle,
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let tx_hashes = indexer_handle
        .query_transaction_hashes_by_block_hash(block_hash.as_bytes())
        .await?;

    remove_batch_by_hashes(TABLE_TRANSACTION, COLUMN_TX_HASH, &tx_hashes, tx).await?;
    remove_batch_by_hashes(
        TABLE_TX_ASSOCIATION_CELL_DEP,
        COLUMN_TX_HASH,
        &tx_hashes,
        tx,
    )
    .await?;
    remove_batch_by_hashes(
        TABLE_TX_ASSOCIATION_HEADER_DEP,
        COLUMN_TX_HASH,
        &tx_hashes,
        tx,
    )
    .await?;
    remove_batch_by_hashes(TABLE_OUTPUT, COLUMN_TX_HASH, &tx_hashes, tx).await?;
    remove_batch_by_hashes(TABLE_INPUT, COLUMN_TX_HASH, &tx_hashes, tx).await?;

    let block_hashes = vec![block_hash];
    remove_batch_by_hashes(TABLE_BLOCK, COLUMN_BLOCK_HASH, &block_hashes, tx).await?;
    remove_batch_by_hashes(
        TABLE_BLOCK_ASSOCIATION_PROPOSAL,
        COLUMN_BLOCK_HASH,
        &block_hashes,
        tx,
    )
    .await?;
    remove_batch_by_hashes(
        TABLE_BLOCK_ASSOCIATION_UNCLE,
        COLUMN_BLOCK_HASH,
        &block_hashes,
        tx,
    )
    .await?;

    Ok(())
}

async fn remove_batch_by_hashes(
    table_name: &str,
    column_name: &str,
    hashes: &[H256],
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    if hashes.is_empty() {
        return Ok(());
    }

    // build query str
    let mut query_builder = SqlBuilder::delete_from(table_name);
    let sql = query_builder
        .and_where_in(column_name, &sqlx_param_placeholders(1..hashes.len())?)
        .sql()
        .map_err(|err| Error::DB(err.to_string()))?;

    // bind
    let mut query: sqlx::query::Query<'_, Any, sqlx::any::AnyArguments<'_>> = sqlx::query(&sql);
    for hash in hashes {
        query = query.bind(hash.as_bytes());
    }

    // execute
    query
        .execute(tx)
        .await
        .map_err(|err| Error::DB(err.to_string()))?;

    Ok(())
}
