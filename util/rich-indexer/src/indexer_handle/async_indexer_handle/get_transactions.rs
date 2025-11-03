use super::*;
use crate::store::SQLXPool;

use ckb_app_config::DBDriver;
use ckb_indexer_sync::Error;
use ckb_jsonrpc_types::{
    IndexerCellType, IndexerScriptType, IndexerSearchMode, IndexerTx, IndexerTxWithCell,
    IndexerTxWithCells,
};
use ckb_jsonrpc_types::{IndexerOrder, IndexerPagination, IndexerSearchKey, JsonBytes, Uint32};
use sql_builder::{SqlBuilder, name, name::SqlName};
use sqlx::{Row, Transaction, any::Any};

impl AsyncRichIndexerHandle {
    /// Get transactions
    pub async fn get_transactions(
        &self,
        mut search_key: IndexerSearchKey,
        order: IndexerOrder,
        limit: Uint32,
        after: Option<JsonBytes>,
    ) -> Result<IndexerPagination<IndexerTx>, Error> {
        let limit = limit.value();
        if limit == 0 {
            return Err(Error::invalid_params("limit should be greater than 0"));
        }
        if limit as usize > self.request_limit {
            return Err(Error::invalid_params(format!(
                "limit must be less than {}",
                self.request_limit,
            )));
        }
        search_key.filter = convert_max_values_in_search_filter(&search_key.filter);

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

                let txs = get_tx_with_cell(
                    self.store.db_driver,
                    search_key,
                    &order,
                    limit,
                    last_cursor,
                    &mut tx,
                )
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
                let txs = get_tx_with_cells(
                    self.store.db_driver,
                    search_key,
                    &order,
                    limit,
                    after,
                    &mut tx,
                )
                .await?;

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
}

pub async fn get_tx_with_cell(
    db_driver: DBDriver,
    search_key: IndexerSearchKey,
    order: &IndexerOrder,
    limit: u32,
    last_cursor: Option<(i64, i32)>,
    tx: &mut Transaction<'_, Any>,
) -> Result<Vec<(i64, u64, u32, Vec<u8>, u16, u32)>, Error> {
    let sql_union = build_tx_with_cell_union_sub_query(db_driver, &search_key)?;

    let mut query_builder = SqlBuilder::select_from(format!("{} AS res", sql_union));
    query_builder.field("tx_id, block.block_number, ckb_transaction.tx_index, ckb_transaction.tx_hash, io_type, io_index");
    query_builder
        .join("ckb_transaction")
        .on("res.tx_id = ckb_transaction.id")
        .join("block")
        .on("ckb_transaction.block_id = block.id");

    if let Some(filter) = &search_key.filter
        && let Some(block_range) = &filter.block_range
    {
        query_builder.and_where_ge("block.block_number", block_range.start());
        query_builder.and_where_lt("block.block_number", block_range.end());
    }
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
            .bind(search_key.script.hash_type as i16);
        match &search_key.script_search_mode {
            Some(IndexerSearchMode::Prefix) | None => {
                query = query
                    .bind(search_key.script.args.as_bytes())
                    .bind(get_binary_upper_boundary(search_key.script.args.as_bytes()));
            }
            Some(IndexerSearchMode::Exact) => {
                query = query.bind(search_key.script.args.as_bytes());
            }
            Some(IndexerSearchMode::Partial) => match db_driver {
                DBDriver::Postgres => {
                    let new_args = escape_and_wrap_for_postgres_like(&search_key.script.args);
                    query = query.bind(new_args);
                }
                DBDriver::Sqlite => {
                    query = query.bind(search_key.script.args.as_bytes());
                }
            },
        }
        if let Some(filter) = search_key.filter.as_ref() {
            if let Some(script) = filter.script.as_ref() {
                query = query
                    .bind(script.code_hash.as_bytes())
                    .bind(script.hash_type as i16);
                // Default prefix search
                query = query
                    .bind(script.args.as_bytes())
                    .bind(get_binary_upper_boundary(script.args.as_bytes()))
            }
            if let Some(data) = &filter.output_data {
                match &filter.output_data_filter_mode {
                    Some(IndexerSearchMode::Prefix) | None => {
                        query = query
                            .bind(data.as_bytes())
                            .bind(get_binary_upper_boundary(data.as_bytes()));
                    }
                    Some(IndexerSearchMode::Exact) => {
                        query = query.bind(data.as_bytes());
                    }
                    Some(IndexerSearchMode::Partial) => match db_driver {
                        DBDriver::Postgres => {
                            let new_data = escape_and_wrap_for_postgres_like(data);
                            query = query.bind(new_data);
                        }
                        DBDriver::Sqlite => {
                            query = query.bind(data.as_bytes());
                        }
                    },
                }
            }
        }
    }

    // fetch
    let outputs = query
        .fetch_all(tx.as_mut())
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
    db_driver: DBDriver,
    search_key: IndexerSearchKey,
    order: &IndexerOrder,
    limit: u32,
    after: Option<JsonBytes>,
    tx: &mut Transaction<'_, Any>,
) -> Result<Vec<(i64, u64, u32, Vec<u8>, Vec<(u16, u32)>)>, Error> {
    let sql_union = build_tx_with_cell_union_sub_query(db_driver, &search_key)?;

    let mut query_builder = SqlBuilder::select_from(format!("{} AS res_union", sql_union));
    query_builder
        .field("tx_id, block.block_number, ckb_transaction.tx_index, ckb_transaction.tx_hash, io_type, io_index")
        .join("ckb_transaction")
        .on("res_union.tx_id = ckb_transaction.id")
        .join("block")
        .on("ckb_transaction.block_id = block.id");

    if let Some(filter) = &search_key.filter
        && let Some(block_range) = &filter.block_range
    {
        query_builder.and_where_ge("block.block_number", block_range.start());
        query_builder.and_where_lt("block.block_number", block_range.end());
    }
    let sql = query_builder
        .subquery()
        .map_err(|err| Error::DB(err.to_string()))?
        .trim_end_matches(';')
        .to_string();

    let mut query_builder = SqlBuilder::select_from(format!("{} AS res", sql));
    query_builder
        .field("tx_id")
        .field("block_number")
        .field("tx_index")
        .field("tx_hash");
    match db_driver {
        DBDriver::Postgres => {
            query_builder.field(
                "'\"' || array_to_string(ARRAY_AGG(CONCAT(io_type, ',', io_index)), '\",\"') || '\"' AS io_pairs",
            );
        }
        DBDriver::Sqlite => {
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
            .bind(search_key.script.hash_type as i16);
        match &search_key.script_search_mode {
            Some(IndexerSearchMode::Prefix) | None => {
                query = query
                    .bind(search_key.script.args.as_bytes())
                    .bind(get_binary_upper_boundary(search_key.script.args.as_bytes()));
            }
            Some(IndexerSearchMode::Exact) => {
                query = query.bind(search_key.script.args.as_bytes());
            }
            Some(IndexerSearchMode::Partial) => match db_driver {
                DBDriver::Postgres => {
                    let new_args = escape_and_wrap_for_postgres_like(&search_key.script.args);
                    query = query.bind(new_args);
                }
                DBDriver::Sqlite => {
                    query = query.bind(search_key.script.args.as_bytes());
                }
            },
        }
        if let Some(filter) = search_key.filter.as_ref() {
            if let Some(script) = filter.script.as_ref() {
                query = query
                    .bind(script.code_hash.as_bytes())
                    .bind(script.hash_type as i16);
                // Default prefix search
                query = query
                    .bind(script.args.as_bytes())
                    .bind(get_binary_upper_boundary(script.args.as_bytes()))
            }
            if let Some(data) = &filter.output_data {
                match &filter.output_data_filter_mode {
                    Some(IndexerSearchMode::Prefix) | None => {
                        query = query
                            .bind(data.as_bytes())
                            .bind(get_binary_upper_boundary(data.as_bytes()));
                    }
                    Some(IndexerSearchMode::Exact) => {
                        query = query.bind(data.as_bytes());
                    }
                    Some(IndexerSearchMode::Partial) => match db_driver {
                        DBDriver::Postgres => {
                            let new_data = escape_and_wrap_for_postgres_like(data);
                            query = query.bind(new_data);
                        }
                        DBDriver::Sqlite => {
                            query = query.bind(data.as_bytes());
                        }
                    },
                }
            }
        }
    }

    // fetch
    let outputs = query
        .fetch_all(tx.as_mut())
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
    db_driver: DBDriver,
    search_key: &IndexerSearchKey,
) -> Result<String, Error> {
    let mut param_index = 1;

    // query outputs
    let mut query_output_builder = SqlBuilder::select_from("output");
    query_output_builder
        .field("output.tx_id AS tx_id")
        .field("1 AS io_type")
        .field("output.output_index AS io_index");
    query_output_builder.join(format!(
        "{} AS query_script",
        build_query_script_id_sql(db_driver, &search_key.script_search_mode, &mut param_index)?
    ));
    match search_key.script_type {
        IndexerScriptType::Lock => {
            query_output_builder.on("output.lock_script_id = query_script.id");
        }
        IndexerScriptType::Type => {
            query_output_builder.on("output.type_script_id = query_script.id");
        }
    }
    if let Some(ref filter) = search_key.filter
        && (filter.script.is_some() || filter.script_len_range.is_some())
    {
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
    build_filter(
        db_driver,
        &mut query_output_builder,
        search_key,
        &mut param_index,
    );

    // query inputs
    let mut query_input_builder = SqlBuilder::select_from("input");
    query_input_builder
        .field("input.consumed_tx_id AS tx_id")
        .field("0 AS io_type")
        .field("input.input_index AS io_index");
    query_input_builder
        .join("output")
        .on("output.id = input.output_id")
        .join(format!(
            "{} AS query_script",
            build_query_script_id_sql(db_driver, &search_key.script_search_mode, &mut param_index)?
        ));
    match search_key.script_type {
        IndexerScriptType::Lock => {
            query_input_builder.on("output.lock_script_id = query_script.id");
        }
        IndexerScriptType::Type => {
            query_input_builder.on("output.type_script_id = query_script.id");
        }
    }
    if let Some(ref filter) = search_key.filter
        && (filter.script.is_some() || filter.script_len_range.is_some())
    {
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
    build_filter(
        db_driver,
        &mut query_input_builder,
        search_key,
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

fn build_filter(
    db_driver: DBDriver,
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
                    query_builder.and_where_ge("type_script.args", format!("${}", param_index));
                    *param_index += 1;
                    query_builder.and_where_lt("type_script.args", format!("${}", param_index));
                    *param_index += 1;
                }
                IndexerScriptType::Type => {
                    query_builder
                        .and_where_eq("lock_script.code_hash", format!("${}", param_index));
                    *param_index += 1;
                    query_builder
                        .and_where_eq("lock_script.hash_type", format!("${}", param_index));
                    *param_index += 1;
                    query_builder.and_where_ge("lock_script.args", format!("${}", param_index));
                    *param_index += 1;
                    query_builder.and_where_lt("lock_script.args", format!("${}", param_index));
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
        if filter.output_data.is_some() {
            match filter.output_data_filter_mode {
                Some(IndexerSearchMode::Prefix) | None => {
                    query_builder.and_where_ge("output.data", format!("${}", param_index));
                    *param_index += 1;
                    query_builder.and_where_lt("output.data", format!("${}", param_index));
                    *param_index += 1;
                }
                Some(IndexerSearchMode::Exact) => {
                    query_builder.and_where_eq("output.data", format!("${}", param_index));
                    *param_index += 1;
                }
                Some(IndexerSearchMode::Partial) => {
                    match db_driver {
                        DBDriver::Postgres => {
                            query_builder.and_where(format!("output.data LIKE ${}", param_index));
                        }
                        DBDriver::Sqlite => {
                            query_builder
                                .and_where(format!("instr(output.data, ${}) > 0", param_index));
                        }
                    }
                    *param_index += 1;
                }
            }
        }
    }
}
