use super::*;
use crate::indexer_handle::sqlx_param_placeholders;

use ckb_indexer_sync::Error;
use sql_builder::SqlBuilder;
use sqlx::{Any, Row, Transaction};

pub(crate) async fn rollback_block(tx: &mut Transaction<'_, Any>) -> Result<(), Error> {
    let block_id = if let Some(block_id) = query_tip_id(tx).await? {
        block_id
    } else {
        return Ok(());
    };

    let tx_id_list = query_tx_id_list_by_block_id(block_id, tx).await?;
    let output_lock_type_list = query_outputs_by_tx_id_list(&tx_id_list, tx).await?;

    // remove transactions, associations, inputs, output
    remove_batch_by_blobs("ckb_transaction", "id", &tx_id_list, tx).await?;
    remove_batch_by_blobs("tx_association_cell_dep", "tx_id", &tx_id_list, tx).await?;
    remove_batch_by_blobs("tx_association_header_dep", "tx_id", &tx_id_list, tx).await?;
    remove_batch_by_blobs("input", "consumed_tx_id", &tx_id_list, tx).await?;
    remove_batch_by_blobs("output", "tx_id", &tx_id_list, tx).await?;

    // remove script
    let mut script_id_list_to_remove = Vec::new();
    for (_, lock_script_id, type_script_id) in output_lock_type_list {
        if !script_exists_in_output(lock_script_id, tx).await? {
            script_id_list_to_remove.push(lock_script_id);
        }
        if let Some(type_script_id) = type_script_id {
            if !script_exists_in_output(type_script_id, tx).await? {
                script_id_list_to_remove.push(type_script_id);
            }
        }
    }
    remove_batch_by_blobs("script", "id", &script_id_list_to_remove, tx).await?;

    // remove block and block associations
    let uncle_id_list = query_uncle_id_list_by_block_id(block_id, tx).await?;
    remove_batch_by_blobs("block", "id", &[block_id], tx).await?;
    remove_batch_by_blobs("block_association_proposal", "block_id", &[block_id], tx).await?;
    remove_batch_by_blobs("block_association_uncle", "block_id", &[block_id], tx).await?;

    // remove uncles
    remove_batch_by_blobs("block", "id", &uncle_id_list, tx).await?;
    remove_batch_by_blobs("block_association_proposal", "block_id", &uncle_id_list, tx).await?;

    Ok(())
}

async fn remove_batch_by_blobs(
    table_name: &str,
    column_name: &str,
    ids: &[i64],
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    if ids.is_empty() {
        return Ok(());
    }

    // build query str
    let mut query_builder = SqlBuilder::delete_from(table_name);
    let sql = query_builder
        .and_where_in(column_name, &sqlx_param_placeholders(1..ids.len())?)
        .sql()
        .map_err(|err| Error::DB(err.to_string()))?;

    // bind
    let mut query: sqlx::query::Query<'_, Any, sqlx::any::AnyArguments<'_>> = sqlx::query(&sql);
    for hash in ids {
        query = query.bind(hash);
    }

    // execute
    query
        .execute(tx)
        .await
        .map_err(|err| Error::DB(err.to_string()))?;

    Ok(())
}

async fn query_uncle_id_list_by_block_id(
    block_id: i64,
    tx: &mut Transaction<'_, Any>,
) -> Result<Vec<i64>, Error> {
    SQLXPool::new_query(
        r#"
            SELECT DISTINCT uncle_id 
            FROM block_association_uncle
            WHERE block_id = $1
            "#,
    )
    .bind(block_id)
    .fetch_all(tx)
    .await
    .map(|rows| rows.into_iter().map(|row| row.get("uncle_id")).collect())
    .map_err(|err| Error::DB(err.to_string()))
}

async fn query_tip_id(tx: &mut Transaction<'_, Any>) -> Result<Option<i64>, Error> {
    SQLXPool::new_query(
        r#"
            SELECT id FROM block
            ORDER BY id DESC
            LIMIT 1
            "#,
    )
    .fetch_optional(tx)
    .await
    .map(|res| res.map(|row| row.get::<i64, _>("id")))
    .map_err(|err| Error::DB(err.to_string()))
}

async fn query_tx_id_list_by_block_id(
    block_id: i64,
    tx: &mut Transaction<'_, Any>,
) -> Result<Vec<i64>, Error> {
    SQLXPool::new_query(
        r#"
        SELECT id FROM ckb_transaction
        WHERE block_id = $1
        ORDER BY id
        ASC
        "#,
    )
    .bind(block_id)
    .fetch_all(tx)
    .await
    .map(|rows| {
        rows.into_iter()
            .map(|row| row.get::<i64, _>("id"))
            .collect()
    })
    .map_err(|err| Error::DB(err.to_string()))
}

async fn query_outputs_by_tx_id_list(
    tx_id_list: &[i64],
    tx: &mut Transaction<'_, Any>,
) -> Result<Vec<(i64, i64, Option<i64>)>, Error> {
    if tx_id_list.is_empty() {
        return Ok(vec![]);
    }

    // build query str
    let mut query_builder = SqlBuilder::select_from("output");
    let sql = query_builder
        .fields(&["id", "lock_script_id", "type_script_id"])
        .and_where_in("tx_id", &sqlx_param_placeholders(1..tx_id_list.len())?)
        .order_by("output_index", false)
        .sql()
        .map_err(|err| Error::DB(err.to_string()))?;

    // bind
    let mut query = SQLXPool::new_query(&sql);
    for tx_id in tx_id_list {
        query = query.bind(tx_id);
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
                        row.get("id"),
                        row.get("lock_script_id"),
                        row.get("type_script_id"),
                    )
                })
                .collect()
        })
}

async fn script_exists_in_output(
    script_id: i64,
    tx: &mut Transaction<'_, Any>,
) -> Result<bool, Error> {
    let row_lock = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT 1 
            FROM output 
            WHERE lock_script_id = $1
        )
        "#,
    )
    .bind(script_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| Error::DB(err.to_string()))?;

    let row_type = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT 1 
            FROM output 
            WHERE type_script_id = $1
        )
        "#,
    )
    .bind(script_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| Error::DB(err.to_string()))?;

    Ok(row_lock.get::<bool, _>(0) || row_type.get::<bool, _>(0))
}
