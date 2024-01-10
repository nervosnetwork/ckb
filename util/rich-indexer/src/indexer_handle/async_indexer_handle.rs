use crate::indexer::to_fixed_array;
use crate::store::{DBType, SQLXPool};

use ckb_indexer_sync::{Error, Pool};
use ckb_jsonrpc_types::{
    IndexerCell, IndexerCellsCapacity, IndexerOrder, IndexerPagination, IndexerSearchKey,
    JsonBytes, Uint32,
};
use ckb_jsonrpc_types::{
    IndexerCellType, IndexerRange, IndexerScriptType, IndexerSearchMode, IndexerTip, IndexerTx,
    IndexerTxWithCell, IndexerTxWithCells,
};
use ckb_types::packed::{CellOutputBuilder, OutPointBuilder, ScriptBuilder};
use ckb_types::prelude::*;
use ckb_types::H256;
use sql_builder::{name, name::SqlName, SqlBuilder};
use sqlx::{
    any::{Any, AnyRow},
    Row, Transaction,
};

use std::sync::{Arc, RwLock};

/// Async handle to the rich-indexer.
#[derive(Clone)]
pub struct AsyncRichIndexerHandle {
    store: SQLXPool,
    _pool: Option<Arc<RwLock<Pool>>>,
}

impl AsyncRichIndexerHandle {
    /// Construct new AsyncRichIndexerHandle instance
    pub fn new(store: SQLXPool, pool: Option<Arc<RwLock<Pool>>>) -> Self {
        Self { store, _pool: pool }
    }
}

impl AsyncRichIndexerHandle {
    /// Get indexer current tip
    pub async fn get_indexer_tip(&self) -> Result<Option<IndexerTip>, Error> {
        let query = SQLXPool::new_query(
            r#"
            SELECT block_hash, block_number FROM block
            ORDER BY id DESC
            LIMIT 1
            "#,
        );
        self.store
            .fetch_optional(query)
            .await
            .map(|res| {
                res.map(|row| IndexerTip {
                    block_number: (row.get::<i64, _>("block_number") as u64).into(),
                    block_hash: bytes_to_h256(row.get("block_hash")),
                })
            })
            .map_err(|err| Error::DB(err.to_string()))
    }

    /// Get cells
    pub async fn get_cells(
        &self,
        search_key: IndexerSearchKey,
        order: IndexerOrder,
        limit: Uint32,
        after: Option<JsonBytes>,
    ) -> Result<IndexerPagination<IndexerCell>, Error> {
        let limit = limit.value() as u32;
        if limit == 0 {
            return Err(Error::invalid_params("limit should be greater than 0"));
        }

        // sub query for script
        let script_sub_query_sql = build_query_script_sql(
            self.store
                .get_db_type()
                .map_err(|err| Error::DB(err.to_string()))?,
            &search_key.script_search_mode,
        )?;

        // query output
        let mut query_builder = SqlBuilder::select_from("output");
        query_builder
            .field("output.id")
            .field("output.output_index")
            .field("output.capacity");
        match search_key.script_type {
            IndexerScriptType::Lock => {
                query_builder
                    .field("query_script.code_hash AS lock_code_hash")
                    .field("query_script.hash_type AS lock_hash_type")
                    .field("query_script.args AS lock_args")
                    .field("type_script.code_hash AS type_code_hash")
                    .field("type_script.hash_type AS type_hash_type")
                    .field("type_script.args AS type_args");
            }
            IndexerScriptType::Type => {
                query_builder
                    .field("lock_script.code_hash AS lock_code_hash")
                    .field("lock_script.hash_type AS lock_hash_type")
                    .field("lock_script.args AS lock_args")
                    .field("query_script.code_hash AS type_code_hash")
                    .field("query_script.hash_type AS type_hash_type")
                    .field("query_script.args AS type_args");
            }
        }
        query_builder
            .field("ckb_transaction.tx_index")
            .field("ckb_transaction.tx_hash")
            .field("block.block_number");
        match search_key.with_data {
            Some(true) | None => {
                query_builder.field("output.data as output_data");
            }
            Some(false) => {
                query_builder.field("NULL as output_data");
            }
        }
        query_builder.join(&format!("{} AS query_script", script_sub_query_sql));
        match search_key.script_type {
            IndexerScriptType::Lock => {
                query_builder.on("output.lock_script_id = query_script.id");
            }
            IndexerScriptType::Type => {
                query_builder.on("output.type_script_id = query_script.id");
            }
        }
        query_builder
            .join("ckb_transaction")
            .on("output.tx_id = ckb_transaction.id")
            .join("block")
            .on("ckb_transaction.block_id = block.id");
        match search_key.script_type {
            IndexerScriptType::Lock => query_builder
                .left()
                .join(name!("script";"type_script"))
                .on("output.type_script_id = type_script.id"),
            IndexerScriptType::Type => query_builder
                .left()
                .join(name!("script";"lock_script"))
                .on("output.lock_script_id = lock_script.id"),
        }
        .join("input")
        .on("output.id = input.output_id")
        .and_where("input.output_id IS NULL"); // live cells

        if let Some(after) = after {
            let after = decode_i64(after.as_bytes())?;
            match order {
                IndexerOrder::Asc => query_builder.and_where_gt("output.id", after),
                IndexerOrder::Desc => query_builder.and_where_lt("output.id", after),
            };
        }

        let mut param_index = 4;
        build_cell_filter(
            self.store
                .get_db_type()
                .map_err(|err| Error::DB(err.to_string()))?,
            &mut query_builder,
            &search_key,
            &mut param_index,
        );

        match order {
            IndexerOrder::Asc => query_builder.order_by("output.id", false),
            IndexerOrder::Desc => query_builder.order_by("output.id", true),
        };
        query_builder.limit(limit);

        // sql string
        let sql = query_builder
            .sql()
            .map_err(|err| Error::DB(err.to_string()))?
            .trim_end_matches(';')
            .to_string();

        // bind
        let mut query = SQLXPool::new_query(&sql);
        query = query
            .bind(search_key.script.code_hash.as_bytes())
            .bind(search_key.script.hash_type as i16);
        let new_args =
            process_bind_data_by_mode(&search_key.script_search_mode, &search_key.script.args);
        query = query.bind(new_args);
        if let Some(filter) = search_key.filter.as_ref() {
            if let Some(script) = filter.script.as_ref() {
                query = query
                    .bind(script.code_hash.as_bytes())
                    .bind(script.hash_type.clone() as i16);
                // Default prefix search
                let mut new_args = script.args.as_bytes().to_vec();
                new_args.push(0x25); // End with %
                query = query.bind(new_args);
            }
            if let Some(data) = &filter.output_data {
                let new_data = process_bind_data_by_mode(&filter.output_data_filter_mode, &data);
                query = query.bind(new_data);
            }
        }

        // fetch
        let mut last_cursor = Vec::new();
        let cells = self
            .store
            .fetch_all(query)
            .await
            .map_err(|err| Error::DB(err.to_string()))?
            .iter()
            .map(|row| {
                last_cursor = row.get::<i64, _>("id").to_le_bytes().to_vec();
                build_indexer_cell(row)
            })
            .collect::<Vec<_>>();

        Ok(IndexerPagination {
            objects: cells,
            last_cursor: JsonBytes::from_vec(last_cursor),
        })
    }

    /// Get transactions
    pub async fn get_transactions(
        &self,
        search_key: IndexerSearchKey,
        order: IndexerOrder,
        limit: Uint32,
        after: Option<JsonBytes>,
    ) -> Result<IndexerPagination<IndexerTx>, Error> {
        let limit = limit.value() as u32;
        if limit == 0 {
            return Err(Error::invalid_params("limit should be greater than 0"));
        }

        let db_type = self
            .store
            .get_db_type()
            .map_err(|err| Error::DB(err.to_string()))?;

        let mut tx = self
            .store
            .transaction()
            .await
            .map_err(|err| Error::DB(err.to_string()))?;

        match search_key.group_by_transaction {
            Some(false) | None => {
                let mut last_cursor = None;
                if let Some(after) = after {
                    if after.len() != 12 {
                        return Err(Error::Params(
                            "Unable to parse the 'after' parameter.".to_string(),
                        ));
                    }
                    let (last, offset) = after.as_bytes().split_at(after.len() - 4);
                    let last = decode_i64(last)?;
                    let offset = decode_i32(offset)?;
                    last_cursor = Some((last, offset));
                };

                let txs =
                    get_tx_with_cell(db_type, search_key, &order, limit, last_cursor, &mut tx)
                        .await?;

                let mut last_id = 0;
                let mut count = 0i32;
                let txs = txs
                    .into_iter()
                    .map(|(id, block_number, tx_index, tx_hash, io_type, io_index)| {
                        if id == last_id {
                            count += 1;
                        } else {
                            last_id = id;
                            count = 1;
                        }
                        IndexerTx::Ungrouped(IndexerTxWithCell {
                            tx_hash: bytes_to_h256(&tx_hash),
                            block_number: block_number.into(),
                            tx_index: tx_index.into(),
                            io_index: io_index.into(),
                            io_type: match io_type {
                                0 => IndexerCellType::Input,
                                1 => IndexerCellType::Output,
                                _ => unreachable!(),
                            },
                        })
                    })
                    .collect::<Vec<_>>();

                let mut last_cursor = last_id.to_le_bytes().to_vec();
                let mut offset = count.to_le_bytes().to_vec();
                last_cursor.append(&mut offset);

                Ok(IndexerPagination {
                    objects: txs,
                    last_cursor: JsonBytes::from_vec(last_cursor),
                })
            }
            Some(true) => {
                let txs =
                    get_tx_with_cells(db_type, search_key, &order, limit, after, &mut tx).await?;

                let mut last_cursor = 0;
                let txs = txs
                    .into_iter()
                    .map(|(id, block_number, tx_index, tx_hash, io_pairs)| {
                        last_cursor = id;
                        IndexerTx::Grouped(IndexerTxWithCells {
                            tx_hash: bytes_to_h256(&tx_hash),
                            block_number: block_number.into(),
                            tx_index: tx_index.into(),
                            cells: io_pairs
                                .into_iter()
                                .map(|(io_type, io_index)| {
                                    (
                                        match io_type {
                                            0 => IndexerCellType::Input,
                                            1 => IndexerCellType::Output,
                                            _ => unreachable!(),
                                        },
                                        io_index.into(),
                                    )
                                })
                                .collect::<Vec<_>>(),
                        })
                    })
                    .collect::<Vec<_>>();
                Ok(IndexerPagination {
                    objects: txs,
                    last_cursor: JsonBytes::from_vec(last_cursor.to_le_bytes().to_vec()),
                })
            }
        }
    }

    /// Get cells_capacity by specified search_key
    pub async fn get_cells_capacity(
        &self,
        search_key: IndexerSearchKey,
    ) -> Result<Option<IndexerCellsCapacity>, Error> {
        // sub query for script
        let mut param_indexer = 1;
        let script_sub_query_sql = build_query_script_id_sql(
            self.store
                .get_db_type()
                .map_err(|err| Error::DB(err.to_string()))?,
            &search_key.script_search_mode,
            &mut param_indexer,
        )?;

        // query output
        let mut query_builder = SqlBuilder::select_from("output");
        query_builder.field("CAST(SUM(output.capacity) AS BIGINT) AS total_capacity");
        query_builder.join(&format!("{} AS query_script", script_sub_query_sql));
        match search_key.script_type {
            IndexerScriptType::Lock => {
                query_builder.on("output.lock_script_id = query_script.id");
            }
            IndexerScriptType::Type => {
                query_builder.on("output.type_script_id = query_script.id");
            }
        }
        if let Some(ref filter) = search_key.filter {
            if filter.block_range.is_some() {
                query_builder
                    .join("ckb_transaction")
                    .on("output.tx_id = ckb_transaction.id")
                    .join("block")
                    .on("ckb_transaction.block_id = block.id");
            }
        }
        query_builder
            .left()
            .join("input")
            .on("output.id = input.output_id");
        if let Some(ref filter) = search_key.filter {
            if filter.script.is_some() || filter.script_len_range.is_some() {
                match search_key.script_type {
                    IndexerScriptType::Lock => {
                        query_builder
                            .join(name!("script";"type_script"))
                            .on("output.type_script_id = type_script.id");
                    }
                    IndexerScriptType::Type => {
                        query_builder
                            .join(name!("script";"lock_script"))
                            .on("output.lock_script_id = lock_script.id");
                    }
                }
            }
        }
        query_builder.and_where("input.output_id IS NULL"); // live cells

        build_cell_filter(
            self.store
                .get_db_type()
                .map_err(|err| Error::DB(err.to_string()))?,
            &mut query_builder,
            &search_key,
            &mut param_indexer,
        );

        // sql string
        let sql = query_builder
            .sql()
            .map_err(|err| Error::DB(err.to_string()))?
            .trim_end_matches(';')
            .to_string();

        // bind
        let mut query = SQLXPool::new_query(&sql);
        query = query
            .bind(search_key.script.code_hash.as_bytes())
            .bind(search_key.script.hash_type as i16);
        let new_args =
            process_bind_data_by_mode(&search_key.script_search_mode, &search_key.script.args);
        query = query.bind(new_args);
        if let Some(filter) = search_key.filter.as_ref() {
            if let Some(script) = filter.script.as_ref() {
                query = query
                    .bind(script.code_hash.as_bytes())
                    .bind(script.hash_type.clone() as i16);
                // Default prefix search
                let mut new_args = script.args.as_bytes().to_vec();
                new_args.push(0x25); // End with %
                query = query.bind(new_args);
            }
            if let Some(data) = &filter.output_data {
                let new_data = process_bind_data_by_mode(&filter.output_data_filter_mode, &data);
                query = query.bind(new_data);
            }
        }

        let mut tx = self
            .store
            .transaction()
            .await
            .map_err(|err| Error::DB(err.to_string()))?;

        // fetch
        let capacity = query
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| Error::DB(err.to_string()))?
            .and_then(|row| row.try_get::<i64, _>("total_capacity").ok());
        let capacity = match capacity {
            Some(capacity) => capacity as u64,
            None => return Ok(None),
        };

        let (block_hash, block_number) = SQLXPool::new_query(
            r#"
                SELECT block_hash, block_number FROM block
                ORDER BY id DESC
                LIMIT 1
                "#,
        )
        .fetch_optional(&mut *tx)
        .await
        .map(|res| {
            res.map(|row| {
                (
                    bytes_to_h256(row.get("block_hash")),
                    row.get::<i64, _>("block_number") as u64,
                )
            })
        })
        .map_err(|err| Error::DB(err.to_string()))?
        .unwrap();

        tx.commit()
            .await
            .map_err(|err| Error::DB(err.to_string()))?;

        Ok(Some(IndexerCellsCapacity {
            capacity: capacity.into(),
            block_hash,
            block_number: block_number.into(),
        }))
    }
}

fn bytes_to_h256(input: &[u8]) -> H256 {
    H256::from_slice(&input[0..32]).expect("bytes to h256")
}

fn build_indexer_cell(row: &AnyRow) -> IndexerCell {
    let out_point = OutPointBuilder::default()
        .tx_hash(to_fixed_array::<32>(&row.get::<Vec<u8>, _>("tx_hash")).pack())
        .index((row.get::<i32, _>("output_index") as u32).pack())
        .build();
    let lock_script = ScriptBuilder::default()
        .code_hash(to_fixed_array::<32>(&row.get::<Vec<u8>, _>("lock_code_hash")).pack())
        .hash_type((row.get::<i16, _>("lock_hash_type") as u8).into())
        .args(row.get::<Vec<u8>, _>("lock_args").pack())
        .build();
    let type_script = row
        .get::<Option<Vec<u8>>, _>("type_code_hash")
        .as_ref()
        .map(|value| {
            ScriptBuilder::default()
                .code_hash(to_fixed_array::<32>(value).pack())
                .hash_type((row.get::<Option<i16>, _>("type_hash_type").unwrap() as u8).into())
                .args(row.get::<Option<Vec<u8>>, _>("type_args").unwrap().pack())
                .build()
        });
    let output = CellOutputBuilder::default()
        .capacity((row.get::<i64, _>("capacity") as u64).pack())
        .lock(lock_script)
        .type_(type_script.pack())
        .build();

    IndexerCell {
        output: output.into(),
        output_data: row
            .get::<Option<Vec<u8>>, _>("output_data")
            .map(JsonBytes::from_vec),
        out_point: out_point.into(),
        block_number: (row.get::<i64, _>("block_number") as u64).into(),
        tx_index: (row.get::<i32, _>("tx_index") as u32).into(),
    }
}

fn decode_i64(data: &[u8]) -> Result<i64, Error> {
    if data.len() != 8 {
        return Err(Error::Params(
            "unable to convert from bytes to i64 due to insufficient data in little-endian format"
                .to_string(),
        ));
    }
    Ok(i64::from_le_bytes(to_fixed_array(&data[0..8])))
}

fn decode_i32(data: &[u8]) -> Result<i32, Error> {
    if data.len() != 4 {
        return Err(Error::Params(
            "unable to convert from bytes to i32 due to insufficient data in little-endian format"
                .to_string(),
        ));
    }
    Ok(i32::from_le_bytes(to_fixed_array(&data[0..4])))
}

fn add_filter_script_len_range_conditions(
    query_builder: &mut SqlBuilder,
    script_name: &str,
    range: &IndexerRange,
) {
    let condition = format!(
        r#"COALESCE(LENGTH({}_script.code_hash), 0)
        + (CASE WHEN {}_script.hash_type IS NULL THEN 0 ELSE 1 END)
        + COALESCE(LENGTH({}_script.args), 0)"#,
        script_name, script_name, script_name
    );
    query_builder.and_where_ge(&condition, range.start());
    query_builder.and_where_lt(&condition, range.end());
}

fn build_query_script_sql(
    db_type: DBType,
    script_search_mode: &Option<IndexerSearchMode>,
) -> Result<String, Error> {
    let mut query_builder = SqlBuilder::select_from("script");
    query_builder
        .field("script.id")
        .field("script.code_hash")
        .field("script.hash_type")
        .field("script.args")
        .and_where_eq("code_hash", "$1")
        .and_where_eq("hash_type", "$2");
    match script_search_mode {
        Some(IndexerSearchMode::Prefix) | None | Some(IndexerSearchMode::Partial) => {
            match db_type {
                DBType::Postgres => {
                    query_builder.and_where("args LIKE $3");
                }
                DBType::Sqlite => {
                    query_builder.and_where("args LIKE $3 ESCAPE '\x5c'");
                }
            }
        }
        Some(IndexerSearchMode::Exact) => {
            query_builder.and_where_eq("args", "$3");
        }
    }
    let sql_sub_query = query_builder
        .subquery()
        .map_err(|err| Error::DB(err.to_string()))?;
    Ok(sql_sub_query)
}

fn build_query_script_id_sql(
    db_type: DBType,
    script_search_mode: &Option<IndexerSearchMode>,
    param_index: &mut usize,
) -> Result<String, Error> {
    let mut query_builder = SqlBuilder::select_from("script");
    query_builder
        .field("script.id")
        .and_where_eq("code_hash", &format!("${}", param_index));
    *param_index += 1;
    query_builder.and_where_eq("hash_type", &format!("${}", param_index));
    *param_index += 1;
    match script_search_mode {
        Some(IndexerSearchMode::Prefix) | None | Some(IndexerSearchMode::Partial) => {
            match db_type {
                DBType::Postgres => {
                    query_builder.and_where(&format!("args LIKE ${}", param_index));
                }
                DBType::Sqlite => {
                    query_builder.and_where(&format!("args LIKE ${} ESCAPE '\x5c'", param_index));
                }
            }
        }
        Some(IndexerSearchMode::Exact) => {
            query_builder.and_where_eq("args", &format!("${}", param_index));
        }
    }
    *param_index += 1;
    let sql_sub_query = query_builder
        .subquery()
        .map_err(|err| Error::DB(err.to_string()))?;
    Ok(sql_sub_query)
}

fn build_cell_filter(
    db_type: DBType,
    query_builder: &mut SqlBuilder,
    search_key: &IndexerSearchKey,
    param_index: &mut usize,
) {
    if let Some(ref filter) = search_key.filter {
        if filter.script.is_some() {
            match search_key.script_type {
                IndexerScriptType::Lock => {
                    query_builder
                        .and_where_eq("type_script.code_hash", format!("${}", param_index));
                    *param_index += 1;
                    query_builder
                        .and_where_eq("type_script.hash_type", format!("${}", param_index));
                    *param_index += 1;
                    match db_type {
                        DBType::Postgres => {
                            query_builder
                                .and_where(format!("type_script.args LIKE ${}", param_index));
                        }
                        DBType::Sqlite => {
                            query_builder.and_where(format!(
                                "type_script.args LIKE ${} ESCAPE '\x5c'",
                                param_index
                            ));
                        }
                    }
                    *param_index += 1;
                }
                IndexerScriptType::Type => {
                    query_builder
                        .and_where_eq("lock_script.code_hash", format!("${}", param_index));
                    *param_index += 1;
                    query_builder
                        .and_where_eq("lock_script.hash_type", format!("${}", param_index));
                    *param_index += 1;
                    match db_type {
                        DBType::Postgres => {
                            query_builder
                                .and_where(format!("lock_script.args LIKE ${}", param_index));
                        }
                        DBType::Sqlite => {
                            query_builder.and_where(format!(
                                "lock_script.args LIKE ${} ESCAPE '\x5c'",
                                param_index
                            ));
                        }
                    }
                    *param_index += 1;
                }
            }
        }
        if let Some(script_len_range) = &filter.script_len_range {
            match search_key.script_type {
                IndexerScriptType::Lock => {
                    add_filter_script_len_range_conditions(query_builder, "type", script_len_range);
                }
                IndexerScriptType::Type => {
                    add_filter_script_len_range_conditions(query_builder, "lock", script_len_range);
                }
            }
        }
        if let Some(data_len_range) = &filter.output_data_len_range {
            query_builder.and_where_ge("LENGTH(output.data)", data_len_range.start());
            query_builder.and_where_lt("LENGTH(output.data)", data_len_range.end());
        }
        if let Some(capacity_range) = &filter.output_capacity_range {
            query_builder.and_where_ge("output.capacity", capacity_range.start());
            query_builder.and_where_lt("output.capacity", capacity_range.end());
        }
        if let Some(block_range) = &filter.block_range {
            query_builder.and_where_ge("block.block_number", block_range.start());
            query_builder.and_where_lt("block.block_number", block_range.end());
        }
        if filter.output_data.is_some() {
            match filter.output_data_filter_mode {
                Some(IndexerSearchMode::Prefix) | None | Some(IndexerSearchMode::Partial) => {
                    match db_type {
                        DBType::Postgres => {
                            query_builder.and_where(format!("output.data LIKE ${}", param_index));
                        }
                        DBType::Sqlite => {
                            query_builder.and_where(format!(
                                "output.data LIKE ${} ESCAPE '\x5c'",
                                param_index
                            ));
                        }
                    }
                }
                Some(IndexerSearchMode::Exact) => {
                    query_builder.and_where_eq("output.data", format!("${}", param_index));
                }
            }
            *param_index += 1;
        }
    }
}

fn process_bind_data_by_mode(mode: &Option<IndexerSearchMode>, data: &JsonBytes) -> Vec<u8> {
    match mode {
        Some(IndexerSearchMode::Exact) => data.as_bytes().to_vec(),
        _ => {
            // 0x5c is the escape character
            // 0x25 is the % character
            let mut new_data: Vec<u8> = data
                .as_bytes()
                .iter()
                .flat_map(|&b| {
                    if b == 0x25 || b == 0x5c {
                        vec![0x5c, b]
                    } else {
                        vec![b]
                    }
                })
                .collect();

            match mode {
                Some(IndexerSearchMode::Partial) => {
                    new_data.insert(0, 0x25); // Start with %
                    new_data.push(0x25); // End with %
                }
                Some(IndexerSearchMode::Prefix) | None => {
                    new_data.push(0x25); // End with %
                }
                _ => {}
            }

            new_data
        }
    }
}

pub async fn get_tx_with_cell(
    db_type: DBType,
    search_key: IndexerSearchKey,
    order: &IndexerOrder,
    limit: u32,
    last_cursor: Option<(i64, i32)>,
    tx: &mut Transaction<'_, Any>,
) -> Result<Vec<(i64, u64, u32, Vec<u8>, u16, u32)>, Error> {
    let sql_union = build_tx_with_cell_union_sub_query(db_type, &search_key)?;

    let mut query_builder = SqlBuilder::select_from(&format!("{} AS res", sql_union));
    query_builder.field("*");

    if let Some((last, _)) = last_cursor {
        match order {
            IndexerOrder::Asc => query_builder.and_where_ge("tx_id", last),
            IndexerOrder::Desc => query_builder.and_where_le("tx_id", last),
        };
    }
    match order {
        IndexerOrder::Asc => query_builder.order_by("tx_id", false),
        IndexerOrder::Desc => query_builder.order_by("tx_id", true),
    };
    query_builder.limit(limit);
    if let Some((_, offset)) = last_cursor {
        query_builder.offset(offset);
    }

    // build sql
    let sql = query_builder
        .sql()
        .map_err(|err| Error::DB(err.to_string()))?
        .trim_end_matches(';')
        .to_string();

    // bind for output and input
    let mut query = SQLXPool::new_query(&sql);
    for _ in 0..2 {
        query = query
            .bind(search_key.script.code_hash.as_bytes())
            .bind(search_key.script.hash_type.clone() as i16);
        let new_args =
            process_bind_data_by_mode(&search_key.script_search_mode, &search_key.script.args);
        query = query.bind(new_args);
        if let Some(filter) = search_key.filter.as_ref() {
            if let Some(script) = filter.script.as_ref() {
                query = query
                    .bind(script.code_hash.as_bytes())
                    .bind(script.hash_type.clone() as i16);
                // Default prefix search
                let mut new_args = script.args.as_bytes().to_vec();
                new_args.push(0x25); // End with %
                query = query.bind(new_args);
            }
            if let Some(data) = &filter.output_data {
                let new_data = process_bind_data_by_mode(&filter.output_data_filter_mode, &data);
                query = query.bind(new_data);
            }
        }
    }

    // fetch
    let outputs = query
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| Error::DB(err.to_string()))?
        .iter()
        .map(|row| {
            (
                row.get::<i64, _>("tx_id"),
                row.get::<i64, _>("block_number") as u64,
                row.get::<i32, _>("tx_index") as u32,
                row.get::<Vec<u8>, _>("tx_hash"),
                row.get::<i32, _>("io_type") as u16,
                row.get::<i32, _>("io_index") as u32,
            )
        })
        .collect::<Vec<_>>();

    Ok(outputs)
}

pub async fn get_tx_with_cells(
    db_type: DBType,
    search_key: IndexerSearchKey,
    order: &IndexerOrder,
    limit: u32,
    after: Option<JsonBytes>,
    tx: &mut Transaction<'_, Any>,
) -> Result<Vec<(i64, u64, u32, Vec<u8>, Vec<(u16, u32)>)>, Error> {
    let sql_union = build_tx_with_cell_union_sub_query(db_type.clone(), &search_key)?;

    let mut query_builder = SqlBuilder::select_from(&format!("{} AS res_union", sql_union));
    let sql = query_builder
        .field("*")
        .subquery()
        .map_err(|err| Error::DB(err.to_string()))?
        .trim_end_matches(';')
        .to_string();

    let mut query_builder = SqlBuilder::select_from(&format!("{} AS res", sql));
    query_builder
        .field("tx_id")
        .field("block_number")
        .field("tx_index")
        .field("tx_hash");
    match db_type {
        DBType::Postgres => {
            query_builder.field(
                "'\"' || array_to_string(ARRAY_AGG(CONCAT(io_type, ',', io_index)), '\",\"') || '\"' AS io_pairs",
            );
        }
        DBType::Sqlite => {
            query_builder.field(
                " '\"' || GROUP_CONCAT(io_type || ',' || io_index, '\",\"') || '\"' AS io_pairs",
            );
        }
    }

    if let Some(after) = after {
        let after = decode_i64(after.as_bytes())?;
        match order {
            IndexerOrder::Asc => query_builder.and_where_gt("tx_id", after),
            IndexerOrder::Desc => query_builder.and_where_lt("tx_id", after),
        };
    }
    query_builder.group_by("tx_id, block_number, tx_index, tx_hash");
    match order {
        IndexerOrder::Asc => query_builder.order_by("tx_id", false),
        IndexerOrder::Desc => query_builder.order_by("tx_id", true),
    };
    query_builder.limit(limit);

    // build sql
    let sql = query_builder
        .sql()
        .map_err(|err| Error::DB(err.to_string()))?
        .trim_end_matches(';')
        .to_string();

    // bind for output and input
    let mut query = SQLXPool::new_query(&sql);
    for _ in 0..2 {
        query = query
            .bind(search_key.script.code_hash.as_bytes())
            .bind(search_key.script.hash_type.clone() as i16);
        let new_args =
            process_bind_data_by_mode(&search_key.script_search_mode, &search_key.script.args);
        query = query.bind(new_args);
        if let Some(filter) = search_key.filter.as_ref() {
            if let Some(script) = filter.script.as_ref() {
                query = query
                    .bind(script.code_hash.as_bytes())
                    .bind(script.hash_type.clone() as i16);
                // Default prefix search
                let mut new_args = script.args.as_bytes().to_vec();
                new_args.push(0x25); // End with %
                query = query.bind(new_args);
            }
            if let Some(data) = &filter.output_data {
                let new_data = process_bind_data_by_mode(&filter.output_data_filter_mode, &data);
                query = query.bind(new_data);
            }
        }
    }

    // fetch
    let outputs = query
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| Error::DB(err.to_string()))?
        .iter()
        .map(|row| {
            (
                row.get::<i64, _>("tx_id"),
                row.get::<i64, _>("block_number") as u64,
                row.get::<i32, _>("tx_index") as u32,
                row.get::<Vec<u8>, _>("tx_hash"),
                {
                    row.get::<String, _>("io_pairs")
                        .trim_matches('{')
                        .trim_matches('}')
                        .split("\",\"")
                        .map(|s| {
                            let s = s.trim_matches('\"');
                            let mut iter = s.split(',');
                            (
                                iter.next().unwrap().parse::<u16>().unwrap(),
                                iter.next().unwrap().parse::<u32>().unwrap(),
                            )
                        })
                        .collect::<Vec<_>>()
                },
            )
        })
        .collect::<Vec<_>>();

    Ok(outputs)
}

fn build_tx_with_cell_union_sub_query(
    db_type: DBType,
    search_key: &IndexerSearchKey,
) -> Result<String, Error> {
    let mut param_index = 1;

    // query outputs
    let mut query_output_builder = SqlBuilder::select_from("ckb_transaction");
    query_output_builder
        .field("ckb_transaction.id AS tx_id")
        .field("block.block_number")
        .field("ckb_transaction.tx_index")
        .field("ckb_transaction.tx_hash")
        .field("1 AS io_type")
        .field("output.output_index AS io_index");
    query_output_builder
        .join("block")
        .on("ckb_transaction.block_id = block.id")
        .join("output")
        .on("output.tx_id = ckb_transaction.id")
        .join(&format!(
            "{} AS query_script",
            build_query_script_id_sql(
                db_type.clone(),
                &search_key.script_search_mode,
                &mut param_index
            )?
        ));
    match search_key.script_type {
        IndexerScriptType::Lock => {
            query_output_builder.on("output.lock_script_id = query_script.id");
        }
        IndexerScriptType::Type => {
            query_output_builder.on("output.type_script_id = query_script.id");
        }
    }
    if let Some(ref filter) = search_key.filter {
        if filter.script.is_some() || filter.script_len_range.is_some() {
            match search_key.script_type {
                IndexerScriptType::Lock => {
                    query_output_builder
                        .left()
                        .join(name!("script";"type_script"))
                        .on("output.type_script_id = type_script.id");
                }
                IndexerScriptType::Type => {
                    query_output_builder
                        .left()
                        .join(name!("script";"lock_script"))
                        .on("output.lock_script_id = lock_script.id");
                }
            }
        }
    }
    build_cell_filter(
        db_type.clone(),
        &mut query_output_builder,
        &search_key,
        &mut param_index,
    );

    // query inputs
    let mut query_input_builder = SqlBuilder::select_from("ckb_transaction");
    query_input_builder
        .field("ckb_transaction.id AS tx_id")
        .field("block.block_number")
        .field("ckb_transaction.tx_index")
        .field("ckb_transaction.tx_hash")
        .field("0 AS io_type")
        .field("input.input_index AS io_index");
    query_input_builder
        .join("block")
        .on("ckb_transaction.block_id = block.id")
        .join("input")
        .on("input.consumed_tx_id = ckb_transaction.id")
        .join("output")
        .on("output.id = input.output_id")
        .join(&format!(
            "{} AS query_script",
            build_query_script_id_sql(
                db_type.clone(),
                &search_key.script_search_mode,
                &mut param_index
            )?
        ));
    match search_key.script_type {
        IndexerScriptType::Lock => {
            query_input_builder.on("output.lock_script_id = query_script.id");
        }
        IndexerScriptType::Type => {
            query_input_builder.on("output.type_script_id = query_script.id");
        }
    }
    if let Some(ref filter) = search_key.filter {
        if filter.script.is_some() || filter.script_len_range.is_some() {
            match search_key.script_type {
                IndexerScriptType::Lock => {
                    query_input_builder
                        .left()
                        .join(name!("script";"type_script"))
                        .on("output.type_script_id = type_script.id");
                }
                IndexerScriptType::Type => {
                    query_input_builder
                        .left()
                        .join(name!("script";"lock_script"))
                        .on("output.lock_script_id = lock_script.id");
                }
            }
        }
    }
    build_cell_filter(
        db_type.clone(),
        &mut query_input_builder,
        &search_key,
        &mut param_index,
    );

    let sql_query_input = query_input_builder
        .sql()
        .map_err(|err| Error::DB(err.to_string()))?
        .trim_end_matches(';')
        .to_string();

    let sql_union = query_output_builder
        .union_all(&sql_query_input)
        .subquery()
        .map_err(|err| Error::DB(err.to_string()))?
        .trim_end_matches(';')
        .to_string();

    Ok(sql_union)
}
