#![allow(clippy::needless_borrow)]

use super::to_fixed_array;
use crate::store::SQLXPool;

use ckb_indexer_sync::Error;
use ckb_types::{
    bytes::Bytes,
    core::{BlockView, TransactionView},
    packed::{Byte, CellInput, CellOutput, OutPoint, ScriptBuilder},
    prelude::*,
};
use sql_builder::SqlBuilder;
use sqlx::{
    any::{Any, AnyArguments, AnyRow},
    query::Query,
    Row, Transaction,
};

use std::collections::HashSet;

// Note that every database has a practical limit on the number of bind parameters you can add to a single query.
// This varies by database.
// https://docs.rs/sqlx/0.6.3/sqlx/struct.QueryBuilder.html#note-database-specific-limits
// BATCH_SIZE_THRESHOLD represents the number of rows that can be bound in an insert sql execution.
// The number of columns in each row multiplied by this BATCH_SIZE_THRESHOLD yields the total number of bound parameters,
// which should be within the above limits.
pub(crate) const BATCH_SIZE_THRESHOLD: usize = 1_000;

enum FieldValue {
    Binary(Vec<u8>),
    BigInt(i64),
    Int(i32),
    NoneBigInt,
    SmallInt(i16),
}

impl FieldValue {
    fn bind<'a>(
        &'a self,
        query: Query<'a, Any, AnyArguments<'a>>,
    ) -> Query<'a, Any, AnyArguments<'a>> {
        match self {
            FieldValue::Binary(value) => query.bind(value),
            FieldValue::BigInt(value) => query.bind(value),
            FieldValue::Int(value) => query.bind(value),
            FieldValue::NoneBigInt => query.bind(Option::<i64>::None),
            FieldValue::SmallInt(value) => query.bind(value),
        }
    }
}

impl From<Vec<u8>> for FieldValue {
    fn from(value: Vec<u8>) -> Self {
        FieldValue::Binary(value)
    }
}

impl From<i64> for FieldValue {
    fn from(value: i64) -> Self {
        FieldValue::BigInt(value)
    }
}

impl From<i32> for FieldValue {
    fn from(value: i32) -> Self {
        FieldValue::Int(value)
    }
}

impl From<i16> for FieldValue {
    fn from(value: i16) -> Self {
        FieldValue::SmallInt(value)
    }
}

pub(crate) async fn append_block(
    block_view: &BlockView,
    tx: &mut Transaction<'_, Any>,
) -> Result<i64, Error> {
    // insert "uncle" first so that the row with the maximum ID in the "block" table corresponds to the tip block.
    let uncle_id_list = insert_uncle_blocks(block_view, tx).await?;
    let block_id = insert_block_table(block_view, tx).await?;
    insert_block_proposals(block_id, block_view, tx).await?;
    bulk_insert_block_association_uncle_table(block_id, &uncle_id_list, tx).await?;
    Ok(block_id)
}

pub(crate) async fn insert_uncle_blocks(
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
    let uncle_block_rows: Vec<Vec<FieldValue>> = uncle_blocks
        .iter()
        .map(block_view_to_field_values)
        .collect();
    let uncle_id_list = bulk_insert_block_table(&uncle_block_rows, tx).await?;
    insert_blocks_proposals(&uncle_id_list, &uncle_blocks, tx).await?;
    Ok(uncle_id_list)
}

async fn insert_block_table(
    block_view: &BlockView,
    tx: &mut Transaction<'_, Any>,
) -> Result<i64, Error> {
    let block_row = block_view_to_field_values(block_view);
    bulk_insert_block_table(&[block_row], tx)
        .await
        .map(|ids| ids[0])
}

async fn insert_blocks_proposals(
    block_id_list: &[i64],
    block_views: &[BlockView],
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let block_association_proposal_rows: Vec<_> = block_id_list
        .iter()
        .zip(block_views)
        .flat_map(|(block_id, block_view)| {
            block_view
                .data()
                .proposals()
                .into_iter()
                .map(move |proposal_hash| {
                    vec![(*block_id).into(), proposal_hash.raw_data().to_vec().into()]
                })
        })
        .collect();

    bulk_insert_block_association_proposal_table(&block_association_proposal_rows, tx).await
}

async fn insert_block_proposals(
    block_id: i64,
    block_view: &BlockView,
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let block_association_proposal_rows: Vec<_> = block_view
        .data()
        .proposals()
        .into_iter()
        .map(move |proposal_hash| vec![block_id.into(), proposal_hash.raw_data().to_vec().into()])
        .collect();
    bulk_insert_block_association_proposal_table(&block_association_proposal_rows, tx).await
}

pub(crate) async fn insert_transaction_table(
    block_id: i64,
    tx_index: usize,
    tx_view: &TransactionView,
    tx: &mut Transaction<'_, Any>,
) -> Result<i64, Error> {
    let tx_row = vec![
        tx_view.hash().raw_data().to_vec().into(),
        tx_view.version().to_be_bytes().to_vec().into(),
        (tx_view.inputs().len() as i32).into(),
        (tx_view.outputs().len() as i32).into(),
        tx_view.witnesses().as_bytes().to_vec().into(),
        block_id.into(),
        (tx_index as i32).into(),
    ];
    bulk_insert_and_return_ids(
        "ckb_transaction",
        &[
            "tx_hash",
            "version",
            "input_count",
            "output_count",
            "witnesses",
            "block_id",
            "tx_index",
        ],
        &[tx_row],
        tx,
    )
    .await
    .map(|ids| ids[0])
}

pub(crate) async fn bulk_insert_blocks_simple(
    block_rows: Vec<(Vec<u8>, i64)>,
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let simple_block_rows: Vec<Vec<FieldValue>> = block_rows
        .into_iter()
        .map(|(block_hash, block_number)| vec![block_hash.into(), block_number.into()])
        .collect();
    bulk_insert(
        "block",
        &["block_hash", "block_number"],
        &simple_block_rows,
        None,
        tx,
    )
    .await
}

async fn bulk_insert_block_table(
    block_rows: &[Vec<FieldValue>],
    tx: &mut Transaction<'_, Any>,
) -> Result<Vec<i64>, Error> {
    bulk_insert_and_return_ids(
        "block",
        &[
            "block_hash",
            "block_number",
            "compact_target",
            "parent_hash",
            "nonce",
            "timestamp",
            "version",
            "transactions_root",
            "epoch",
            "dao",
            "proposals_hash",
            "extra_hash",
            "extension",
        ],
        block_rows,
        tx,
    )
    .await
}

async fn bulk_insert_block_association_proposal_table(
    block_association_proposal_rows: &[Vec<FieldValue>],
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    bulk_insert(
        "block_association_proposal",
        &["block_id", "proposal"],
        &block_association_proposal_rows,
        None,
        tx,
    )
    .await
}

async fn bulk_insert_block_association_uncle_table(
    block_id: i64,
    uncle_id_list: &[i64],
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let block_association_uncle_rows: Vec<_> = uncle_id_list
        .iter()
        .map(|uncle_id| vec![block_id.into(), (*uncle_id).into()])
        .collect();
    bulk_insert(
        "block_association_uncle",
        &["block_id", "uncle_id"],
        &block_association_uncle_rows,
        None,
        tx,
    )
    .await
}

pub(crate) async fn bulk_insert_output_table(
    tx_id: i64,
    output_cell_rows: Vec<(
        i32,
        i64,
        (Vec<u8>, i16, Vec<u8>),
        Option<(Vec<u8>, i16, Vec<u8>)>,
        Vec<u8>,
    )>,
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let mut new_rows: Vec<Vec<FieldValue>> = Vec::new();
    for row in output_cell_rows {
        let type_script_id = if let Some(type_script) = &row.3 {
            query_script_id(&type_script.0, type_script.1, &type_script.2, tx).await?
        } else {
            None
        };
        let new_row = vec![
            tx_id.into(),
            row.0.into(),
            row.1.into(),
            query_script_id(&row.2 .0, row.2 .1, &row.2 .2, tx)
                .await?
                .map_or(FieldValue::NoneBigInt, FieldValue::BigInt),
            type_script_id.map_or(FieldValue::NoneBigInt, FieldValue::BigInt),
            row.4.into(),
        ];
        new_rows.push(new_row);
    }
    bulk_insert(
        "output",
        &[
            "tx_id",
            "output_index",
            "capacity",
            "lock_script_id",
            "type_script_id",
            "data",
        ],
        &new_rows,
        None,
        tx,
    )
    .await
}

pub(crate) async fn bulk_insert_input_table(
    tx_id: i64,
    input_rows: Vec<(i64, Vec<u8>, i32)>,
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let input_rows = input_rows
        .into_iter()
        .map(|row| vec![row.0.into(), row.1.into(), tx_id.into(), row.2.into()])
        .collect::<Vec<Vec<FieldValue>>>();
    bulk_insert(
        "input",
        &["output_id", "since", "consumed_tx_id", "input_index"],
        &input_rows,
        None,
        tx,
    )
    .await
}

pub(crate) async fn bulk_insert_script_table(
    script_set: HashSet<(Vec<u8>, i16, Vec<u8>)>,
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    // let script_rows = script_set.iter().collect::<Vec<_>>();
    let script_rows = script_set
        .into_iter()
        .map(|(code_hash, hash_type, args)| vec![code_hash.into(), hash_type.into(), args.into()])
        .collect::<Vec<_>>();
    bulk_insert(
        "script",
        &["code_hash", "hash_type", "args"],
        &script_rows,
        Some(&["code_hash", "hash_type", "args"]),
        tx,
    )
    .await
}

pub(crate) async fn bulk_insert_tx_association_header_dep_table(
    tx_id: i64,
    tx_view: &TransactionView,
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let mut tx_association_header_dep_rows = Vec::new();
    for header_dep in tx_view.header_deps_iter() {
        query_block_id(&header_dep.raw_data().to_vec(), tx)
            .await?
            .map(|block_id| {
                tx_association_header_dep_rows.push(vec![tx_id.into(), block_id.into()]);
            });
    }
    bulk_insert(
        "tx_association_header_dep",
        &["tx_id", "block_id"],
        &tx_association_header_dep_rows,
        None,
        tx,
    )
    .await
}

pub(crate) async fn bulk_insert_tx_association_cell_dep_table(
    tx_id: i64,
    tx_view: &TransactionView,
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    let mut tx_association_cell_dep_rows = Vec::new();
    for cell_dep in tx_view.cell_deps_iter() {
        query_output_id(&cell_dep.out_point(), tx)
            .await?
            .map(|output_id| {
                tx_association_cell_dep_rows.push(vec![
                    tx_id.into(),
                    output_id.into(),
                    (u8::try_from(cell_dep.dep_type()).expect("cell_dep to u8 should be OK")
                        as i16)
                        .into(),
                ]);
            });
    }
    bulk_insert(
        "tx_association_cell_dep",
        &["tx_id", "output_id", "dep_type"],
        &tx_association_cell_dep_rows,
        None,
        tx,
    )
    .await
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
            output.tx_id = (SELECT ckb_transaction.id FROM ckb_transaction WHERE tx_hash = $1)
            AND output.output_index = $2
        "#,
    )
    .bind(output_tx_hash)
    .bind(output_index as i32)
    .fetch_optional(tx)
    .await
    .map_err(|err| Error::DB(err.to_string()))?;

    Ok(build_cell_output(row))
}

pub(crate) async fn query_output_id(
    out_point: &OutPoint,
    tx: &mut Transaction<'_, Any>,
) -> Result<Option<i64>, Error> {
    let output_tx_hash = out_point.tx_hash().raw_data().to_vec();
    let output_index: u32 = out_point.index().unpack();

    sqlx::query(
        r#"
        SELECT output.id
        FROM
            output
        WHERE
            output.tx_id = (SELECT ckb_transaction.id FROM ckb_transaction WHERE tx_hash = $1)
            AND output.output_index = $2
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

pub(crate) async fn query_block_id(
    block_hash: &[u8],
    tx: &mut Transaction<'_, Any>,
) -> Result<Option<i64>, Error> {
    sqlx::query(
        r#"
        SELECT id
        FROM
            block
        WHERE
            block_hash = $1
        "#,
    )
    .bind(block_hash)
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

fn build_cell_output(row: Option<AnyRow>) -> Option<(i64, CellOutput, Bytes)> {
    let row = match row {
        Some(row) => row,
        None => return None,
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
        type_builder = type_builder.hash_type(Byte::new(type_hash_type as u8));
    }
    let type_script = type_builder.build();

    let cell_output = CellOutput::new_builder()
        .capacity((capacity as u64).pack())
        .lock(lock_script)
        .type_(Some(type_script).pack())
        .build();

    Some((id, cell_output, data.into()))
}

async fn bulk_insert(
    table: &str,
    fields: &[&str],
    rows: &[Vec<FieldValue>],
    conflict_do_nothing_fields: Option<&[&str]>,
    tx: &mut Transaction<'_, Any>,
) -> Result<(), Error> {
    for bulk in rows.chunks(BATCH_SIZE_THRESHOLD) {
        // build query str
        let mut sql = build_bulk_insert_sql(table, fields, bulk)?;
        if let Some(fields) = conflict_do_nothing_fields {
            sql = format!("{} ON CONFLICT ({}) DO NOTHING", sql, fields.join(", "));
        }

        // bind
        let mut query = SQLXPool::new_query(&sql);
        for row in bulk {
            for field in row {
                query = field.bind(query);
            }
        }

        // execute
        query
            .execute(&mut *tx)
            .await
            .map_err(|err| Error::DB(err.to_string()))?;
    }
    Ok(())
}

async fn bulk_insert_and_return_ids(
    table: &str,
    fields: &[&str],
    rows: &[Vec<FieldValue>],
    tx: &mut Transaction<'_, Any>,
) -> Result<Vec<i64>, Error> {
    let mut id_list = Vec::new();
    for bulk in rows.chunks(BATCH_SIZE_THRESHOLD) {
        // build query str
        let sql = build_bulk_insert_sql(table, fields, bulk)?;
        let sql = format!("{} RETURNING id", sql);

        // bind
        let mut query = SQLXPool::new_query(&sql);
        for row in bulk {
            for field in row {
                query = field.bind(query);
            }
        }

        // execute
        let mut rows = query
            .fetch_all(&mut *tx)
            .await
            .map_err(|err| Error::DB(err.to_string()))?;
        id_list.append(&mut rows);
    }
    let ret: Vec<_> = id_list.iter().map(|row| row.get::<i64, _>("id")).collect();
    Ok(ret)
}

fn build_bulk_insert_sql(
    table: &str,
    fields: &[&str],
    bulk: &[Vec<FieldValue>],
) -> Result<String, Error> {
    let mut builder = SqlBuilder::insert_into(table);
    builder.fields(fields);
    bulk.iter().enumerate().for_each(|(row_index, row)| {
        let placeholders = (1..=row.len())
            .map(|i| format!("${}", i + row_index * row.len()))
            .collect::<Vec<String>>();
        builder.values(&placeholders);
    });
    let sql = builder
        .sql()
        .map_err(|err| Error::DB(err.to_string()))?
        .trim_end_matches(';')
        .to_string();
    Ok(sql)
}

fn block_view_to_field_values(block_view: &BlockView) -> Vec<FieldValue> {
    vec![
        block_view.hash().raw_data().to_vec().into(),
        (block_view.number() as i64).into(),
        block_view.compact_target().to_be_bytes().to_vec().into(),
        block_view.parent_hash().raw_data().to_vec().into(),
        block_view.nonce().to_be_bytes().to_vec().into(),
        (block_view.timestamp() as i64).into(),
        block_view.version().to_be_bytes().to_vec().into(),
        block_view.transactions_root().raw_data().to_vec().into(),
        block_view
            .epoch()
            .full_value()
            .to_be_bytes()
            .to_vec()
            .into(),
        block_view.dao().raw_data().to_vec().into(),
        block_view.proposals_hash().raw_data().to_vec().into(),
        block_view.extra_hash().raw_data().to_vec().into(),
        match block_view.data().extension() {
            Some(extension) => extension.raw_data().to_vec().into(),
            None => Vec::new().into(),
        },
    ]
}
