#![allow(clippy::needless_borrow)]

use super::*;
use crate::store::SQLXPool;

use ckb_indexer_sync::Error;
use ckb_types::{
    core::{BlockView, TransactionView},
    packed::OutPointBuilder,
    prelude::*,
};
use seq_macro::seq;
use sql_builder::SqlBuilder;
use sqlx::{Any, Row, Transaction};

use std::collections::HashSet;

// Note that every database has a practical limit on the number of bind parameters you can add to a single query.
// This varies by database.
// https://docs.rs/sqlx/0.6.3/sqlx/struct.QueryBuilder.html#note-database-specific-limits
// BATCH_SIZE_THRESHOLD represents the number of rows that can be bound in an insert sql execution.
// The number of columns in each row multiplied by this BATCH_SIZE_THRESHOLD yields the total number of bound parameters,
// which should be within the above limits.
pub(crate) const BATCH_SIZE_THRESHOLD: usize = 1_000;

pub(crate) async fn append_block(
    block_view: &BlockView,
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    bulk_insert_block_table(&[block_view.to_owned()], tx).await?;
    bulk_insert_block_association_proposal_table(&[block_view.to_owned()], tx).await?;

    insert_uncle_block(block_view, tx).await?;

    Ok(())
}

pub(crate) async fn insert_uncle_block(
    block_view: &BlockView,
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let uncle_blocks = block_view
        .uncles()
        .into_iter()
        .map(|uncle| {
            let uncle_block_header = uncle.header();
            BlockView::new_advanced_builder()
                .header(uncle_block_header)
                .proposals(uncle.data().proposals())
                .build()
        })
        .collect::<Vec<_>>();

    bulk_insert_block_table(&uncle_blocks, tx).await?;
    bulk_insert_block_association_uncle_table(&[block_view.to_owned()], tx).await?;

    Ok(())
}

pub(crate) async fn insert_transactions(
    block_view: &BlockView,
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let block_hash = block_view.hash().raw_data().to_vec();
    let tx_views = block_view.transactions();

    bulk_insert_transaction_table(&block_hash, &tx_views, tx).await?;

    bulk_insert_output_table(&tx_views, tx).await?;
    bulk_insert_input_table(&tx_views, tx).await?;
    bulk_insert_script_table(&tx_views, tx).await?;

    bulk_insert_tx_association_header_dep_table(&tx_views, tx).await?;
    bulk_insert_tx_association_cell_dep_table(&tx_views, tx).await?;
    bulk_insert_output_association_script(&tx_views, tx).await?;

    Ok(())
}

async fn bulk_insert_block_table(
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
                block_view.extra_hash().raw_data().to_vec(),
            )
        })
        .collect();

    for start in (0..block_rows.len()).step_by(BATCH_SIZE_THRESHOLD) {
        let end = (start + BATCH_SIZE_THRESHOLD).min(block_rows.len());

        // insert mercury_block
        // build query str
        let mut builder = SqlBuilder::insert_into(TABLE_BLOCK);
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
                extra_hash"#,
        );
        push_values_placeholders(&mut builder, 14, end - start);
        let sql = builder
            .sql()
            .map_err(|err| Error::DB(err.to_string()))?
            .trim_end_matches(';')
            .to_string();

        // bind
        let mut query = SQLXPool::new_query(&sql);
        for row in block_rows[start..end].iter() {
            seq!(i in 0..14 {
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

async fn bulk_insert_block_association_proposal_table(
    block_views: &[BlockView],
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let mut block_association_proposal_rows: Vec<_> = Vec::new();

    for block_view in block_views.iter() {
        for proposal_hash in block_view.data().proposals().into_iter() {
            let row = (
                block_view.hash().raw_data().to_vec(),
                proposal_hash.raw_data().to_vec(),
            );
            block_association_proposal_rows.push(row);
        }
    }

    // bulk insert
    for start in (0..block_association_proposal_rows.len()).step_by(BATCH_SIZE_THRESHOLD) {
        let end = (start + BATCH_SIZE_THRESHOLD).min(block_association_proposal_rows.len());

        // build query str
        let mut builder = SqlBuilder::insert_into(TABLE_BLOCK_ASSOCIATION_PROPOSAL);
        builder.field(
            r#"
            block_hash,
            proposal"#,
        );
        push_values_placeholders(&mut builder, 2, end - start);
        let sql = builder
            .sql()
            .map_err(|err| Error::DB(err.to_string()))?
            .trim_end_matches(';')
            .to_string();

        // bind
        let mut query = SQLXPool::new_query(&sql);
        for row in block_association_proposal_rows[start..end].iter() {
            seq!(i in 0..2 {
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

async fn bulk_insert_block_association_uncle_table(
    block_views: &[BlockView],
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let mut block_association_uncle_rows: Vec<_> = Vec::new();

    for block_view in block_views.iter() {
        for uncle in block_view.uncles().into_iter() {
            let row = (
                block_view.hash().raw_data().to_vec(),
                uncle.hash().raw_data().to_vec(),
            );
            block_association_uncle_rows.push(row);
        }
    }

    // bulk insert
    for start in (0..block_association_uncle_rows.len()).step_by(BATCH_SIZE_THRESHOLD) {
        let end = (start + BATCH_SIZE_THRESHOLD).min(block_association_uncle_rows.len());

        // build query str
        let mut builder = SqlBuilder::insert_into(TABLE_BLOCK_ASSOCIATION_UNCLE);
        builder.field(
            r#"
            block_hash,
            uncle_hash"#,
        );
        push_values_placeholders(&mut builder, 2, end - start);
        let sql = builder
            .sql()
            .map_err(|err| Error::DB(err.to_string()))?
            .trim_end_matches(';')
            .to_string();

        // bind
        let mut query = SQLXPool::new_query(&sql);
        for row in block_association_uncle_rows[start..end].iter() {
            seq!(i in 0..2 {
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

async fn bulk_insert_transaction_table(
    block_hash: &[u8],
    tx_views: &[TransactionView],
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let tx_rows: Vec<_> = tx_views
        .iter()
        .enumerate()
        .map(|(tx_index, transaction)| {
            (
                transaction.hash().raw_data().to_vec(),
                transaction.version() as i16,
                transaction.inputs().len() as i32,
                transaction.outputs().len() as i32,
                transaction.witnesses().as_bytes().to_vec(),
                block_hash.to_vec(),
                tx_index as i32,
            )
        })
        .collect();

    for start in (0..tx_rows.len()).step_by(BATCH_SIZE_THRESHOLD) {
        let end = (start + BATCH_SIZE_THRESHOLD).min(tx_rows.len());

        // build query str
        let mut builder = SqlBuilder::insert_into(TABLE_TRANSACTION);
        builder.field(
            r#"tx_hash, 
            version, 
            input_count, 
            output_count, 
            witnesses,
            block_hash,   
            tx_index"#,
        );
        push_values_placeholders(&mut builder, 7, end - start);
        let sql = builder
            .sql()
            .map_err(|err| Error::DB(err.to_string()))?
            .trim_end_matches(';')
            .to_string();

        // bind
        let mut query = SQLXPool::new_query(&sql);
        for row in tx_rows[start..end].iter() {
            seq!(i in 0..7 {
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

async fn bulk_insert_output_table(
    tx_views: &[TransactionView],
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let mut output_cell_rows = Vec::new();

    for tx_view in tx_views.iter() {
        for (output_index, (cell, data)) in tx_view.outputs_with_data_iter().enumerate() {
            let cell_capacity: u64 = cell.capacity().unpack();
            let cell_row = (
                OutPointBuilder::default()
                    .tx_hash(tx_view.hash())
                    .index(output_index.pack())
                    .build()
                    .as_bytes()
                    .to_vec(),
                i64::try_from(cell_capacity).map_err(|err| Error::DB(err.to_string()))?,
                data.to_vec(),
                tx_view.hash().raw_data().to_vec(),
                i32::try_from(output_index).map_err(|err| Error::DB(err.to_string()))?,
            );
            output_cell_rows.push(cell_row);
        }
    }

    // bulk insert
    for start in (0..output_cell_rows.len()).step_by(BATCH_SIZE_THRESHOLD) {
        let end = (start + BATCH_SIZE_THRESHOLD).min(output_cell_rows.len());

        // build query str
        let mut builder = SqlBuilder::insert_into(TABLE_OUTPUT);
        builder.field(
            r#"
            out_point,
            capacity,
            data,
            tx_hash,
            output_index"#,
        );
        push_values_placeholders(&mut builder, 5, end - start);
        let sql = builder
            .sql()
            .map_err(|err| Error::DB(err.to_string()))?
            .trim_end_matches(';')
            .to_string();

        // bind
        let mut query = SQLXPool::new_query(&sql);
        for row in output_cell_rows[start..end].iter() {
            seq!(i in 0..5 {
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

async fn bulk_insert_input_table(
    tx_views: &[TransactionView],
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let mut input_rows = Vec::new();

    for (tx_index, tx_view) in tx_views.iter().enumerate() {
        if tx_index == 0 {
            // cellbase
            continue;
        }
        for (input_index, input) in tx_view.inputs().into_iter().enumerate() {
            let since: u64 = input.since().unpack();
            let input_row = (
                input.previous_output().as_bytes().to_vec(),
                since.to_be_bytes().to_vec(),
                tx_view.hash().raw_data().to_vec(),
                i32::try_from(input_index).map_err(|err| Error::DB(err.to_string()))?,
            );
            input_rows.push(input_row);
        }
    }

    // bulk insert
    for start in (0..input_rows.len()).step_by(BATCH_SIZE_THRESHOLD) {
        let end = (start + BATCH_SIZE_THRESHOLD).min(input_rows.len());

        // build query str
        let mut builder = SqlBuilder::insert_into(TABLE_INPUT);
        builder.field(
            r#"
            out_point,
            since,
            tx_hash,
            input_index"#,
        );
        push_values_placeholders(&mut builder, 4, end - start);
        let sql = builder
            .sql()
            .map_err(|err| Error::DB(err.to_string()))?
            .trim_end_matches(';')
            .to_string();

        // bind
        let mut query = SQLXPool::new_query(&sql);
        for row in input_rows[start..end].iter() {
            seq!(i in 0..4 {
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

async fn bulk_insert_script_table(
    tx_views: &[TransactionView],
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let mut script_set = HashSet::new();
    let mut exist_script_cache = HashSet::new();

    for tx_view in tx_views.iter() {
        for (cell, _) in tx_view.outputs_with_data_iter() {
            if let Some(type_script) = cell.type_().to_opt() {
                let type_hash = type_script.calc_script_hash().raw_data();
                let type_script_args = type_script.args().raw_data();

                let type_script_row = (
                    type_hash.to_vec(),
                    type_script.code_hash().raw_data().to_vec(),
                    type_script_args.to_vec(),
                    i16::try_from(
                        u8::try_from(type_script.hash_type())
                            .map_err(|err| Error::DB(err.to_string()))?,
                    )
                    .map_err(|err| Error::DB(err.to_string()))?,
                );
                if !script_set.contains(&type_script_row)
                    && !script_exists(&type_script_row.0, &mut exist_script_cache, tx).await?
                {
                    exist_script_cache.insert(type_script_row.0.clone());
                    script_set.insert(type_script_row);
                }
            }

            let lock_script = cell.lock();
            let lock_hash = lock_script.calc_script_hash().raw_data();
            let lock_script_args = lock_script.args().raw_data();
            let lock_script_row = (
                lock_hash.to_vec(),
                lock_script.code_hash().raw_data().to_vec(),
                lock_script_args.to_vec(),
                i16::try_from(
                    u8::try_from(lock_script.hash_type())
                        .map_err(|err| Error::DB(err.to_string()))?,
                )
                .map_err(|err| Error::DB(err.to_string()))?,
            );
            if !script_set.contains(&lock_script_row)
                && !script_exists(&lock_script_row.0, &mut exist_script_cache, tx).await?
            {
                exist_script_cache.insert(lock_script_row.0.clone());
                script_set.insert(lock_script_row);
            }
        }
    }

    let script_rows = script_set.iter().cloned().collect::<Vec<_>>();

    // bulk insert
    for start in (0..script_rows.len()).step_by(BATCH_SIZE_THRESHOLD) {
        let end = (start + BATCH_SIZE_THRESHOLD).min(script_rows.len());

        // build query str
        let mut builder = SqlBuilder::insert_into(TABLE_SCRIPT);
        builder.field(
            r#"script_hash,
            script_code_hash,
            script_args,
            script_type"#,
        );
        push_values_placeholders(&mut builder, 4, end - start);
        let sql = builder
            .sql()
            .map_err(|err| Error::DB(err.to_string()))?
            .trim_end_matches(';')
            .to_string();

        // bind
        let mut query = SQLXPool::new_query(&sql);
        for row in script_rows[start..end].iter() {
            seq!(i in 0..4 {
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

async fn bulk_insert_tx_association_header_dep_table(
    tx_views: &[TransactionView],
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let mut tx_association_header_dep_rows = Vec::new();

    for tx_view in tx_views.iter() {
        for header_dep in tx_view.header_deps_iter() {
            let row = (
                tx_view.hash().raw_data().to_vec(),
                header_dep.raw_data().to_vec(),
            );
            tx_association_header_dep_rows.push(row);
        }
    }

    // bulk insert
    for start in (0..tx_association_header_dep_rows.len()).step_by(BATCH_SIZE_THRESHOLD) {
        let end = (start + BATCH_SIZE_THRESHOLD).min(tx_association_header_dep_rows.len());

        // build query str
        let mut builder = SqlBuilder::insert_into(TABLE_TX_ASSOCIATION_HEADER_DEP);
        builder.field(
            r#"
            tx_hash,
            block_hash"#,
        );
        push_values_placeholders(&mut builder, 2, end - start);
        let sql = builder
            .sql()
            .map_err(|err| Error::DB(err.to_string()))?
            .trim_end_matches(';')
            .to_string();

        // bind
        let mut query = SQLXPool::new_query(&sql);
        for row in tx_association_header_dep_rows[start..end].iter() {
            seq!(i in 0..2 {
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

async fn bulk_insert_tx_association_cell_dep_table(
    tx_views: &[TransactionView],
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let mut tx_association_cell_dep_rows = Vec::new();

    for tx_view in tx_views.iter() {
        for cell_dep in tx_view.cell_deps_iter() {
            let row = (
                tx_view.hash().raw_data().to_vec(),
                cell_dep.out_point().as_bytes().to_vec(),
                i16::try_from(
                    u8::try_from(cell_dep.dep_type()).map_err(|err| Error::DB(err.to_string()))?,
                )
                .map_err(|err| Error::DB(err.to_string()))?,
            );
            tx_association_cell_dep_rows.push(row);
        }
    }

    // bulk insert
    for start in (0..tx_association_cell_dep_rows.len()).step_by(BATCH_SIZE_THRESHOLD) {
        let end = (start + BATCH_SIZE_THRESHOLD).min(tx_association_cell_dep_rows.len());

        // build query str
        let mut builder = SqlBuilder::insert_into(TABLE_TX_ASSOCIATION_CELL_DEP);
        builder.field(
            r#"
            tx_hash,
            out_point,
            dep_type"#,
        );
        push_values_placeholders(&mut builder, 3, end - start);
        let sql = builder
            .sql()
            .map_err(|err| Error::DB(err.to_string()))?
            .trim_end_matches(';')
            .to_string();

        // bind
        let mut query = SQLXPool::new_query(&sql);
        for row in tx_association_cell_dep_rows[start..end].iter() {
            seq!(i in 0..3 {
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

async fn bulk_insert_output_association_script(
    tx_views: &[TransactionView],
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let mut output_association_script_rows = Vec::new();

    for tx_view in tx_views.iter() {
        for ((cell, _), out_point) in tx_view
            .outputs_with_data_iter()
            .zip(tx_view.output_pts_iter())
        {
            if let Some(type_script) = cell.type_().to_opt() {
                let type_hash = type_script.calc_script_hash().raw_data();
                let row = (out_point.as_bytes().to_vec(), type_hash.to_vec());
                output_association_script_rows.push(row)
            }

            let lock_script = cell.lock();
            let lock_hash = lock_script.calc_script_hash().raw_data();
            let row = (out_point.as_bytes().to_vec(), lock_hash.to_vec());
            output_association_script_rows.push(row);
        }
    }

    // bulk insert
    for start in (0..output_association_script_rows.len()).step_by(BATCH_SIZE_THRESHOLD) {
        let end = (start + BATCH_SIZE_THRESHOLD).min(output_association_script_rows.len());

        // build query str
        let mut builder = SqlBuilder::insert_into(TABLE_OUTPUT_ASSOCIATION_SCRIPT);
        builder.field(
            r#"
                out_point,
                script_hash"#,
        );
        push_values_placeholders(&mut builder, 2, end - start);
        let sql = builder
            .sql()
            .map_err(|err| Error::DB(err.to_string()))?
            .trim_end_matches(';')
            .to_string();

        // bind
        let mut query = SQLXPool::new_query(&sql);
        for row in output_association_script_rows[start..end].iter() {
            seq!(i in 0..2 {
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

async fn script_exists(
    script_hash: &[u8],
    exist_script_cache: &mut HashSet<Vec<u8>>,
    tx: &mut Transaction<'_, Any>,
) -> Result<bool, Error> {
    if exist_script_cache.contains(script_hash) {
        return Ok(true);
    }

    let row = sqlx::query(
        "SELECT COUNT(*) as count 
        FROM script WHERE
        script_hash = $1",
    )
    .bind(script_hash)
    .fetch_one(tx)
    .await
    .map_err(|err| Error::DB(err.to_string()))?;

    Ok(row.get::<i64, _>("count") != 0)
}
