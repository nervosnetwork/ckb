use super::*;
use crate::indexer_handle::{bytes_to_h256, sqlx_param_placeholders};

use ckb_indexer_sync::Error;
use ckb_types::{bytes::Bytes, packed::OutPoint, H256};
use sql_builder::SqlBuilder;
use sqlx::{Any, Row, Transaction};

pub(crate) async fn rollback_block(
    block_hash: H256,
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let tx_hashes = query_transaction_hashes_by_block_hash(block_hash.as_bytes(), tx).await?;
    let outputs = query_outputs_by_tx_hashes(&tx_hashes, tx).await?;

    // remove transactions, associations, inputs
    let tx_hashes_to_remove: Vec<Vec<u8>> = tx_hashes
        .iter()
        .map(|hash| hash.as_bytes().to_vec())
        .collect();
    remove_batch_by_blobs("ckb_transaction", "tx_hash", &tx_hashes_to_remove, tx).await?;
    remove_batch_by_blobs(
        "tx_association_cell_dep",
        "tx_hash",
        &tx_hashes_to_remove,
        tx,
    )
    .await?;
    remove_batch_by_blobs(
        "tx_association_header_dep",
        "tx_hash",
        &tx_hashes_to_remove,
        tx,
    )
    .await?;
    remove_batch_by_blobs("input", "tx_hash", &tx_hashes_to_remove, tx).await?;

    // remove output and association
    let out_points_to_remove: Vec<Vec<u8>> = outputs
        .iter()
        .map(|(out_point, _, _)| out_point.as_bytes().to_vec())
        .collect();
    remove_batch_by_blobs("output", "out_point", &out_points_to_remove, tx).await?;

    // remove script
    let mut script_hashes_to_remove = Vec::new();
    for (_, lock_script_hash, type_script_hash) in outputs {
        if !script_exists_in_output(&lock_script_hash, tx).await? {
            script_hashes_to_remove.push(lock_script_hash);
        }
        if !type_script_hash.is_empty() && !script_exists_in_output(&type_script_hash, tx).await? {
            script_hashes_to_remove.push(type_script_hash);
        }
    }
    remove_batch_by_blobs("script", "script_hash", &script_hashes_to_remove, tx).await?;

    // remove block and block associations
    let uncle_hashes = query_uncle_hashes_by_block_hash(block_hash.as_bytes(), tx).await?;
    let block_hashes_to_remove = vec![block_hash.as_bytes().to_vec()];
    remove_batch_by_blobs("block", "block_hash", &block_hashes_to_remove, tx).await?;
    remove_batch_by_blobs(
        "block_association_proposal",
        "block_hash",
        &block_hashes_to_remove,
        tx,
    )
    .await?;
    remove_batch_by_blobs(
        "block_association_uncle",
        "block_hash",
        &block_hashes_to_remove,
        tx,
    )
    .await?;

    // remove uncles
    let mut uncle_hashes_to_remove = Vec::new();
    for uncle_hash in uncle_hashes {
        if !uncle_exists_in_association_table(uncle_hash.as_bytes(), tx).await? {
            uncle_hashes_to_remove.push(uncle_hash.as_bytes().to_vec());
        }
    }
    remove_batch_by_blobs("block", "block_hash", &uncle_hashes_to_remove, tx).await?;
    remove_batch_by_blobs(
        "block_association_proposal",
        "block_hash",
        &uncle_hashes_to_remove,
        tx,
    )
    .await?;

    Ok(())
}

async fn remove_batch_by_blobs(
    table_name: &str,
    column_name: &str,
    blobs: &[Vec<u8>],
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    if blobs.is_empty() {
        return Ok(());
    }

    // build query str
    let mut query_builder = SqlBuilder::delete_from(table_name);
    let sql = query_builder
        .and_where_in(column_name, &sqlx_param_placeholders(1..blobs.len())?)
        .sql()
        .map_err(|err| Error::DB(err.to_string()))?;

    // bind
    let mut query: sqlx::query::Query<'_, Any, sqlx::any::AnyArguments<'_>> = sqlx::query(&sql);
    for hash in blobs {
        query = query.bind(hash);
    }

    // execute
    query
        .execute(tx)
        .await
        .map_err(|err| Error::DB(err.to_string()))?;

    Ok(())
}

async fn query_uncle_hashes_by_block_hash(
    block_hash: &[u8],
    tx: &mut Transaction<'_, Any>,
) -> Result<Vec<H256>, Error> {
    SQLXPool::new_query(
        r#"
            SELECT DISTINCT uncle_hash 
            FROM block_association_uncle
            WHERE block_hash = $1
            "#,
    )
    .bind(block_hash)
    .fetch_all(tx)
    .await
    .map(|rows| {
        rows.into_iter()
            .map(|row| bytes_to_h256(row.get("uncle_hash")))
            .collect()
    })
    .map_err(|err| Error::DB(err.to_string()))
}

async fn uncle_exists_in_association_table(
    uncle_hash: &[u8],
    tx: &mut Transaction<'_, Any>,
) -> Result<bool, Error> {
    let row = SQLXPool::new_query(
        r#"
        SELECT COUNT(*) as count 
        FROM block_association_uncle WHERE
        uncle_hash = $1
        "#,
    )
    .bind(uncle_hash)
    .fetch_one(tx)
    .await
    .map_err(|err| Error::DB(err.to_string()))?;
    Ok(row.get::<i64, _>("count") != 0)
}

async fn query_transaction_hashes_by_block_hash(
    block_hash: &[u8],
    tx: &mut Transaction<'_, Any>,
) -> Result<Vec<H256>, Error> {
    SQLXPool::new_query(
        r#"
        SELECT tx_hash FROM ckb_transaction
        WHERE block_hash = $1
        ORDER BY tx_index
        ASC
        "#,
    )
    .bind(block_hash)
    .fetch_all(tx)
    .await
    .map(|rows| {
        rows.into_iter()
            .map(|row| bytes_to_h256(row.get("tx_hash")))
            .collect()
    })
    .map_err(|err| Error::DB(err.to_string()))
}

async fn query_outputs_by_tx_hashes(
    tx_hashes: &[H256],
    tx: &mut Transaction<'_, Any>,
) -> Result<Vec<(OutPoint, Vec<u8>, Vec<u8>)>, Error> {
    if tx_hashes.is_empty() {
        return Ok(vec![]);
    }

    // build query str
    let mut query_builder = SqlBuilder::select_from("output");
    let sql = query_builder
        .fields(&["out_point", "lock_script_hash", "type_script_hash"])
        .and_where_in("tx_hash", &sqlx_param_placeholders(1..tx_hashes.len())?)
        .order_by("output_index", false)
        .sql()
        .map_err(|err| Error::DB(err.to_string()))?;

    // bind
    let mut query = SQLXPool::new_query(&sql);
    for hash in tx_hashes {
        query = query.bind(hash.as_bytes());
    }

    // execute
    query
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| Error::DB(err.to_string()))
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    (
                        OutPoint::new_unchecked(Bytes::copy_from_slice(row.get("out_point"))),
                        row.get("lock_script_hash"),
                        row.get("type_script_hash"),
                    )
                })
                .collect()
        })
}

async fn script_exists_in_output(
    script_hash: &[u8],
    tx: &mut Transaction<'_, Any>,
) -> Result<bool, Error> {
    let row = sqlx::query(
        r#"
        SELECT COUNT(*) as count 
        FROM output WHERE
        lock_script_hash = $1 OR type_script_hash = $1
        "#,
    )
    .bind(script_hash)
    .fetch_one(tx)
    .await
    .map_err(|err| Error::DB(err.to_string()))?;

    Ok(row.get::<i64, _>("count") != 0)
}
