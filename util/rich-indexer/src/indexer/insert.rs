#![allow(clippy::needless_borrow)]

use crate::store::SQLXPool;

use ckb_indexer_sync::Error;
use ckb_types::{
    bytes::Bytes,
    core::{BlockView, TransactionView},
    packed::{Byte, CellInput, CellOutput, OutPoint, ScriptBuilder},
    prelude::*,
};
use seq_macro::seq;
use sql_builder::SqlBuilder;
use sqlx::{any::AnyRow, Any, Row, Transaction};

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
) -> Result<i64, Error> {
    let block_views = vec![block_view.clone()];

    // insert "uncle" first so that the row with the maximum ID in the "block" table corresponds to the tip block.
    let uncle_id_list = bulk_insert_uncle_blocks(block_view, tx).await?;
    let block_id_list = bulk_insert_block_table(&block_views, tx).await?;
    bulk_insert_block_association_proposal_table(&block_id_list, &block_views, tx).await?;
    bulk_insert_block_association_uncle_table(block_id_list[0], &uncle_id_list, tx).await?;
    Ok(block_id_list[0])
}

pub(crate) async fn insert_transaction_table(
    block_id: i64,
    tx_index: usize,
    tx_view: &TransactionView,
    tx: &mut Transaction<'_, Any>,
) -> Result<i64, Error> {
    let tx_row = (
        tx_view.hash().raw_data().to_vec(),
        tx_view.version().to_be_bytes().to_vec(),
        tx_view.inputs().len() as i32,
        tx_view.outputs().len() as i32,
        tx_view.witnesses().as_bytes().to_vec(),
        block_id,
        tx_index as i32,
    );

    // build query str
    let mut builder = SqlBuilder::insert_into("ckb_transaction");
    builder.field(
        r#"
        tx_hash, 
        version, 
        input_count, 
        output_count, 
        witnesses,
        block_id,   
        tx_index"#,
    );
    push_values_placeholders(&mut builder, 7, 1);
    let sql = builder
        .sql()
        .map_err(|err| Error::DB(err.to_string()))?
        .trim_end_matches(';')
        .to_string();
    let sql = format!("{} RETURNING id", sql);

    // bind
    let mut query = SQLXPool::new_query(&sql);
    seq!(i in 0..7 {
        query = query.bind(&tx_row.i);
    });

    // execute
    let row = query
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| Error::DB(err.to_string()))?;

    Ok(row.get::<i64, _>("id"))
}

pub(crate) async fn bulk_insert_blocks_simple(
    block_rows: &[(Vec<u8>, i64)],
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    for start in (0..block_rows.len()).step_by(BATCH_SIZE_THRESHOLD) {
        let end = (start + BATCH_SIZE_THRESHOLD).min(block_rows.len());

        // insert block
        // build query str
        let mut builder = SqlBuilder::insert_into("block");
        builder.field(
            r#"
            block_hash,
            block_number"#,
        );
        push_values_placeholders(&mut builder, 2, 1);
        let sql = builder
            .sql()
            .map_err(|err| Error::DB(err.to_string()))?
            .trim_end_matches(';')
            .to_string();

        // bind
        let mut query = SQLXPool::new_query(&sql);
        for row in block_rows[start..end].iter() {
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

pub(crate) async fn bulk_insert_uncle_blocks(
    block_view: &BlockView,
    tx: &mut Transaction<'_, Any>,
) -> Result<Vec<i64>, Error> {
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

    let uncle_id_list = bulk_insert_block_table(&uncle_blocks, tx).await?;
    bulk_insert_block_association_proposal_table(&uncle_id_list, &uncle_blocks, tx).await?;

    Ok(uncle_id_list)
}

async fn bulk_insert_block_table(
    block_views: &[BlockView],
    tx: &mut Transaction<'_, Any>,
) -> Result<Vec<i64>, Error> {
    let block_rows: Vec<_> = block_views
        .iter()
        .map(|block_view| {
            (
                block_view.hash().raw_data().to_vec(),
                block_view.number() as i64,
                block_view.compact_target().to_be_bytes().to_vec(),
                block_view.parent_hash().raw_data().to_vec(),
                block_view.nonce().to_be_bytes().to_vec(),
                block_view.timestamp() as i64,
                block_view.version().to_be_bytes().to_vec(),
                block_view.transactions_root().raw_data().to_vec(),
                block_view.epoch().full_value().to_be_bytes().to_vec(),
                block_view.dao().raw_data().to_vec(),
                block_view.proposals_hash().raw_data().to_vec(),
                block_view.extra_hash().raw_data().to_vec(),
            )
        })
        .collect();

    let mut block_id_list = Vec::new();
    for start in (0..block_rows.len()).step_by(BATCH_SIZE_THRESHOLD) {
        let end = (start + BATCH_SIZE_THRESHOLD).min(block_rows.len());

        // insert block
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
            epoch,
            dao,
            proposals_hash,
            extra_hash"#,
        );
        push_values_placeholders(&mut builder, 12, end - start);
        let sql = builder
            .sql()
            .map_err(|err| Error::DB(err.to_string()))?
            .trim_end_matches(';')
            .to_string();
        let sql = format!("{} RETURNING id", sql);

        // bind
        let mut query = SQLXPool::new_query(&sql);
        for row in block_rows[start..end].iter() {
            seq!(i in 0..12 {
                query = query.bind(&row.i);
            });
        }

        // execute
        let mut rows = query
            .fetch_all(&mut *tx)
            .await
            .map_err(|err| Error::DB(err.to_string()))?;
        block_id_list.append(&mut rows);
    }
    let ret: Vec<_> = block_id_list
        .iter()
        .map(|row| row.get::<i64, _>("id"))
        .collect();
    Ok(ret)
}

async fn bulk_insert_block_association_proposal_table(
    block_id_list: &[i64],
    block_views: &[BlockView],
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let mut block_association_proposal_rows: Vec<_> = Vec::new();
    for (block_id, block_view) in block_id_list.iter().zip(block_views) {
        for proposal_hash in block_view.data().proposals().into_iter() {
            let row = (block_id, proposal_hash.raw_data().to_vec());
            block_association_proposal_rows.push(row);
        }
    }

    // bulk insert
    for start in (0..block_association_proposal_rows.len()).step_by(BATCH_SIZE_THRESHOLD) {
        let end = (start + BATCH_SIZE_THRESHOLD).min(block_association_proposal_rows.len());

        // build query str
        let mut builder = SqlBuilder::insert_into("block_association_proposal");
        builder.field(
            r#"
            block_id,
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
    block_id: i64,
    uncle_id_list: &[i64],
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let block_association_uncle_rows: Vec<_> = uncle_id_list
        .iter()
        .map(|uncle_id| (block_id, uncle_id))
        .collect();

    // bulk insert
    for start in (0..block_association_uncle_rows.len()).step_by(BATCH_SIZE_THRESHOLD) {
        let end = (start + BATCH_SIZE_THRESHOLD).min(block_association_uncle_rows.len());

        // build query str
        let mut builder = SqlBuilder::insert_into("block_association_uncle");
        builder.field(
            r#"
            block_id,
            uncle_id"#,
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

pub(crate) async fn bulk_insert_output_table(
    tx_id: i64,
    output_cell_rows: &[(
        i32,
        i64,
        (Vec<u8>, i16, Vec<u8>),
        Option<(Vec<u8>, i16, Vec<u8>)>,
        Vec<u8>,
    )],
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let mut new_rows = Vec::new();
    for row in output_cell_rows {
        let new_row = (
            tx_id,
            row.0,
            row.1,
            query_script_id(&row.2 .0, row.2 .1, &row.2 .2, tx).await?,
            if let Some(type_script) = &row.3 {
                query_script_id(&type_script.0, type_script.1, &type_script.2, tx).await?
            } else {
                None
            },
            &row.4,
        );
        new_rows.push(new_row);
    }

    // bulk insert
    for start in (0..output_cell_rows.len()).step_by(BATCH_SIZE_THRESHOLD) {
        let end = (start + BATCH_SIZE_THRESHOLD).min(output_cell_rows.len());

        // build query str
        let mut builder = SqlBuilder::insert_into("output");
        builder.field(
            r#"
            tx_id,
            output_index,
            capacity,
            lock_script_id,
            type_script_id,
            data"#,
        );
        push_values_placeholders(&mut builder, 6, end - start);
        let sql = builder
            .sql()
            .map_err(|err| Error::DB(err.to_string()))?
            .trim_end_matches(';')
            .to_string();

        // bind
        let mut query = SQLXPool::new_query(&sql);
        for row in new_rows[start..end].iter() {
            seq!(i in 0..6 {
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

pub(crate) async fn bulk_insert_input_table(
    tx_id: i64,
    input_rows: &[(i64, Vec<u8>, i32)],
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    // replace tx_hash with tx_id
    let input_rows = input_rows
        .iter()
        .map(|row| (row.0, &row.1, tx_id, row.2))
        .collect::<Vec<_>>();

    // bulk insert
    for start in (0..input_rows.len()).step_by(BATCH_SIZE_THRESHOLD) {
        let end = (start + BATCH_SIZE_THRESHOLD).min(input_rows.len());

        // build query str
        let mut builder = SqlBuilder::insert_into("input");
        builder.field(
            r#"
            output_id,
            since,
            consumed_tx_id,
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

pub(crate) async fn bulk_insert_script_table(
    script_set: &HashSet<(Vec<u8>, i16, Vec<u8>)>,
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let script_rows = script_set.iter().cloned().collect::<Vec<_>>();

    // bulk insert
    for start in (0..script_rows.len()).step_by(BATCH_SIZE_THRESHOLD) {
        let end = (start + BATCH_SIZE_THRESHOLD).min(script_rows.len());

        // build query str
        let mut builder = SqlBuilder::insert_into("script");
        builder.field(
            r#"code_hash,
            hash_type,
            args"#,
        );
        push_values_placeholders(&mut builder, 3, end - start);
        let sql = builder
            .sql()
            .map_err(|err| Error::DB(err.to_string()))?
            .trim_end_matches(';')
            .to_string();
        let sql = format!(
            "{} ON CONFLICT (code_hash, hash_type, args) DO NOTHING",
            sql
        );

        // bind
        let mut query = SQLXPool::new_query(&sql);
        for row in script_rows[start..end].iter() {
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

pub(crate) async fn bulk_insert_tx_association_header_dep_table(
    tx_id: i64,
    tx_view: &TransactionView,
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let mut tx_association_header_dep_rows = Vec::new();
    for header_dep in tx_view.header_deps_iter() {
        let row = (tx_id, header_dep.raw_data().to_vec());
        tx_association_header_dep_rows.push(row);
    }

    // bulk insert
    for start in (0..tx_association_header_dep_rows.len()).step_by(BATCH_SIZE_THRESHOLD) {
        let end = (start + BATCH_SIZE_THRESHOLD).min(tx_association_header_dep_rows.len());

        // build query str
        let mut builder = SqlBuilder::insert_into("tx_association_header_dep");
        builder.field(
            r#"
            tx_id,
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

pub(crate) async fn bulk_insert_tx_association_cell_dep_table(
    tx_id: i64,
    tx_view: &TransactionView,
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let mut tx_association_cell_dep_rows: Vec<(i64, Vec<u8>, i32, i16)> = Vec::new();
    for cell_dep in tx_view.cell_deps_iter() {
        let row = (
            tx_id,
            cell_dep.out_point().tx_hash().raw_data().to_vec(),
            {
                let idx: u32 = cell_dep.out_point().index().unpack();
                idx as i32
            },
            u8::try_from(cell_dep.dep_type()).expect("cell_dep to u8 should be OK") as i16,
        );
        tx_association_cell_dep_rows.push(row);
    }

    // bulk insert
    for start in (0..tx_association_cell_dep_rows.len()).step_by(BATCH_SIZE_THRESHOLD) {
        let end = (start + BATCH_SIZE_THRESHOLD).min(tx_association_cell_dep_rows.len());

        // build query str
        let mut builder = SqlBuilder::insert_into("tx_association_cell_dep");
        builder.field(
            r#"
            tx_id,
            output_tx_hash,
            output_index,
            dep_type"#,
        );
        push_values_placeholders(&mut builder, 4, end - start);
        let sql = builder
            .sql()
            .map_err(|err| Error::DB(err.to_string()))?
            .trim_end_matches(';')
            .to_string();

        // bind
        let mut query = SQLXPool::new_query(&sql);
        for row in tx_association_cell_dep_rows[start..end].iter() {
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

pub(crate) async fn query_output_cell(
    out_point: &OutPoint,
    tx: &mut Transaction<'_, Any>,
) -> Result<Option<(i64, CellOutput, Bytes)>, Error> {
    let output_tx_hash = out_point.tx_hash().raw_data().to_vec();
    let output_index: u32 = out_point.index().unpack();

    let row = sqlx::query(
        r#"
        SELECT 
            output.id,
            output.capacity,
            output.data,
            lock_script.code_hash AS lock_code_hash,
            lock_script.hash_type AS lock_hash_type,
            lock_script.args AS lock_args,
            type_script.code_hash AS type_code_hash,
            type_script.hash_type AS type_hash_type,
            type_script.args AS type_args
        FROM 
            output 
        LEFT JOIN 
            script AS lock_script ON output.lock_script_id = lock_script.id
        LEFT JOIN 
            script AS type_script ON output.type_script_id = type_script.id
        WHERE 
            output.tx_id = (SELECT id FROM ckb_transaction WHERE tx_hash = $1) 
            AND output.output_index = $2
        "#,
    )
    .bind(output_tx_hash)
    .bind(output_index as i32)
    .fetch_optional(tx)
    .await
    .map_err(|err| Error::DB(err.to_string()))?;

    build_cell_output(row)
}

pub(crate) async fn query_output_id(
    out_point: &OutPoint,
    tx: &mut Transaction<'_, Any>,
) -> Result<Option<i64>, Error> {
    let output_tx_hash = out_point.tx_hash().raw_data().to_vec();
    let output_index: u32 = out_point.index().unpack();

    sqlx::query(
        r#"
        SELECT id
        FROM 
            output 
        WHERE 
            output.tx_id = (SELECT id FROM ckb_transaction WHERE tx_hash = $1) 
            AND output_index = $2
        "#,
    )
    .bind(output_tx_hash)
    .bind(output_index as i32)
    .fetch_optional(tx)
    .await
    .map_err(|err| Error::DB(err.to_string()))
    .map(|row| row.map(|row| row.get::<i64, _>("id")))
}

pub(crate) async fn query_script_id(
    code_hash: &[u8],
    hash_type: i16,
    args: &[u8],
    tx: &mut Transaction<'_, Any>,
) -> Result<Option<i64>, Error> {
    sqlx::query(
        r#"
        SELECT id
        FROM 
            script 
        WHERE 
            code_hash = $1 AND hash_type = $2 AND args = $3
        "#,
    )
    .bind(code_hash)
    .bind(hash_type)
    .bind(args)
    .fetch_optional(tx)
    .await
    .map_err(|err| Error::DB(err.to_string()))
    .map(|row| row.map(|row| row.get::<i64, _>("id")))
}

pub(crate) fn build_output_cell_rows(
    cell: &CellOutput,
    output_index: usize,
    data: &Bytes,
    output_cell_rows: &mut Vec<(
        i32,
        i64,
        (Vec<u8>, i16, Vec<u8>),
        Option<(Vec<u8>, i16, Vec<u8>)>,
        Vec<u8>,
    )>,
) {
    let cell_capacity: u64 = cell.capacity().unpack();
    let cell_row = (
        output_index as i32,
        cell_capacity as i64,
        (
            cell.lock().code_hash().raw_data().to_vec(),
            u8::try_from(cell.lock().hash_type()).expect("hash_type to u8 should be OK") as i16,
            cell.lock().args().raw_data().to_vec(),
        ),
        (cell.type_().to_opt().map(|type_script| {
            (
                type_script.code_hash().raw_data().to_vec(),
                u8::try_from(type_script.hash_type()).expect("hash_type to u8 should be OK") as i16,
                type_script.args().raw_data().to_vec(),
            )
        })),
        data.to_vec(),
    );
    output_cell_rows.push(cell_row);
}

pub(crate) async fn build_script_set(
    cell: &CellOutput,
    script_row: &mut HashSet<(Vec<u8>, i16, Vec<u8>)>,
) {
    let lock_script = cell.lock();
    let lock_script_row = (
        lock_script.code_hash().raw_data().to_vec(),
        u8::try_from(lock_script.hash_type()).expect("hash_type to u8 should be OK") as i16,
        lock_script.args().raw_data().to_vec(),
    );
    script_row.insert(lock_script_row);

    if let Some(type_script) = cell.type_().to_opt() {
        let type_script_row = (
            type_script.code_hash().raw_data().to_vec(),
            u8::try_from(type_script.hash_type()).expect("hash_type to u8 should be OK") as i16,
            type_script.args().raw_data().to_vec(),
        );
        script_row.insert(type_script_row);
    }
}

pub(crate) fn build_input_rows(
    output_id: i64,
    input: &CellInput,
    input_index: usize,
    input_rows: &mut Vec<(i64, Vec<u8>, i32)>,
) {
    let since: u64 = input.since().unpack();
    let input_row = (output_id, since.to_be_bytes().to_vec(), input_index as i32);
    input_rows.push(input_row);
}

fn build_cell_output(row: Option<AnyRow>) -> Result<Option<(i64, CellOutput, Bytes)>, Error> {
    let row = match row {
        Some(row) => row,
        None => return Ok(None),
    };
    let id: i64 = row.get("id");
    let capacity: i64 = row.get("capacity");
    let data: Vec<u8> = row.get("data");
    let lock_code_hash: Option<Vec<u8>> = row.get("lock_code_hash");
    let lock_hash_type: Option<i16> = row.get("lock_hash_type");
    let lock_args: Option<Vec<u8>> = row.get("lock_args");
    let type_code_hash: Option<Vec<u8>> = row.get("type_code_hash");
    let type_hash_type: Option<i16> = row.get("type_hash_type");
    let type_args: Option<Vec<u8>> = row.get("type_args");

    let mut lock_builder = ScriptBuilder::default();
    if let Some(lock_code_hash) = lock_code_hash {
        lock_builder = lock_builder.code_hash(to_fixed_array::<32>(&lock_code_hash[0..32]).pack());
    }
    if let Some(lock_args) = lock_args {
        lock_builder = lock_builder.args(lock_args.pack());
    }
    if let Some(lock_hash_type) = lock_hash_type {
        lock_builder = lock_builder.hash_type(Byte::new(lock_hash_type as u8));
    }
    let lock_script = lock_builder.build();

    let mut type_builder = ScriptBuilder::default();
    if let Some(type_code_hash) = type_code_hash {
        type_builder = type_builder.code_hash(to_fixed_array::<32>(&type_code_hash[0..32]).pack());
    }
    if let Some(type_args) = type_args {
        type_builder = type_builder.args(type_args.pack());
    }
    if let Some(type_hash_type) = type_hash_type {
        type_builder = lock_builder.hash_type(Byte::new(type_hash_type as u8));
    }
    let type_script = type_builder.build();

    let cell_output = CellOutput::new_builder()
        .capacity((capacity as u64).pack())
        .lock(lock_script)
        .type_(Some(type_script).pack())
        .build();

    Ok(Some((id, cell_output, data.into())))
}

fn to_fixed_array<const LEN: usize>(input: &[u8]) -> [u8; LEN] {
    assert_eq!(input.len(), LEN);
    let mut list = [0; LEN];
    list.copy_from_slice(input);
    list
}
