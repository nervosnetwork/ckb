mod get_cells;
mod get_cells_capacity;
mod get_transactions;

use crate::indexer::to_fixed_array;
use crate::store::SQLXPool;

use ckb_app_config::DBDriver;
use ckb_indexer_sync::{Error, Pool};
use ckb_jsonrpc_types::{
    IndexerRange, IndexerScriptType, IndexerSearchKey, IndexerSearchMode, IndexerTip,
};
use ckb_types::H256;
use num_bigint::BigUint;
use sql_builder::SqlBuilder;
use sqlx::Row;

use std::sync::{Arc, RwLock};

/// Async handle to the rich-indexer.
#[derive(Clone)]
pub struct AsyncRichIndexerHandle {
    store: SQLXPool,
    pool: Option<Arc<RwLock<Pool>>>,
}

impl AsyncRichIndexerHandle {
    /// Construct new AsyncRichIndexerHandle instance
    pub fn new(store: SQLXPool, pool: Option<Arc<RwLock<Pool>>>) -> Self {
        Self { store, pool }
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
    db_driver: DBDriver,
    script_search_mode: &Option<IndexerSearchMode>,
    param_index: &mut usize,
) -> Result<String, Error> {
    let mut query_builder = SqlBuilder::select_from("script");
    query_builder
        .field("script.id")
        .field("script.code_hash")
        .field("script.hash_type")
        .field("script.args")
        .and_where_eq("code_hash", &format!("${}", param_index));
    *param_index += 1;
    query_builder.and_where_eq("hash_type", &format!("${}", param_index));
    *param_index += 1;
    match script_search_mode {
        Some(IndexerSearchMode::Prefix) | None => {
            query_builder.and_where_ge("args", &format!("${}", param_index));
            *param_index += 1;
            query_builder.and_where_lt("args", &format!("${}", param_index));
            *param_index += 1;
        }
        Some(IndexerSearchMode::Exact) => {
            query_builder.and_where_eq("args", &format!("${}", param_index));
            *param_index += 1;
        }
        Some(IndexerSearchMode::Partial) => {
            match db_driver {
                DBDriver::Postgres => {
                    query_builder.and_where(format!("position(${} in args) > 0", param_index));
                }
                DBDriver::Sqlite => {
                    query_builder.and_where(format!("instr(args, ${}) > 0", param_index));
                }
            }
            *param_index += 1;
        }
    }
    let sql_sub_query = query_builder
        .subquery()
        .map_err(|err| Error::DB(err.to_string()))?;
    Ok(sql_sub_query)
}

fn build_query_script_id_sql(
    db_driver: DBDriver,
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
        Some(IndexerSearchMode::Prefix) | None => {
            query_builder.and_where_ge("args", &format!("${}", param_index));
            *param_index += 1;
            query_builder.and_where_lt("args", &format!("${}", param_index));
            *param_index += 1;
        }
        Some(IndexerSearchMode::Exact) => {
            query_builder.and_where_eq("args", &format!("${}", param_index));
            *param_index += 1;
        }
        Some(IndexerSearchMode::Partial) => {
            match db_driver {
                DBDriver::Postgres => {
                    query_builder.and_where(format!("position(${} in args) > 0", param_index));
                }
                DBDriver::Sqlite => {
                    query_builder.and_where(format!("instr(args, ${}) > 0", param_index));
                }
            }
            *param_index += 1;
        }
    }
    let sql_sub_query = query_builder
        .subquery()
        .map_err(|err| Error::DB(err.to_string()))?;
    Ok(sql_sub_query)
}

fn build_cell_filter(
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
                    query_builder.and_where_ge("type_script.args", &format!("${}", param_index));
                    *param_index += 1;
                    query_builder.and_where_lt("type_script.args", &format!("${}", param_index));
                    *param_index += 1;
                }
                IndexerScriptType::Type => {
                    query_builder
                        .and_where_eq("lock_script.code_hash", format!("${}", param_index));
                    *param_index += 1;
                    query_builder
                        .and_where_eq("lock_script.hash_type", format!("${}", param_index));
                    *param_index += 1;
                    query_builder.and_where_ge("lock_script.args", &format!("${}", param_index));
                    *param_index += 1;
                    query_builder.and_where_lt("lock_script.args", &format!("${}", param_index));
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
                Some(IndexerSearchMode::Prefix) | None => {
                    query_builder.and_where_ge("output.data", &format!("${}", param_index));
                    *param_index += 1;
                    query_builder.and_where_lt("output.data", &format!("${}", param_index));
                    *param_index += 1;
                }
                Some(IndexerSearchMode::Exact) => {
                    query_builder.and_where_eq("output.data", format!("${}", param_index));
                    *param_index += 1;
                }
                Some(IndexerSearchMode::Partial) => {
                    match db_driver {
                        DBDriver::Postgres => {
                            query_builder.and_where(format!(
                                "position(${} in output.data) > 0",
                                param_index
                            ));
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

fn get_binary_upper_boundary(value: &[u8]) -> Vec<u8> {
    if value.is_empty() {
        return vec![255; 32];
    }
    let value_big = BigUint::from_bytes_be(value);
    let value_upper = value_big + 1usize;
    value_upper.to_bytes_be()
}

fn bytes_to_h256(input: &[u8]) -> H256 {
    H256::from_slice(&input[0..32]).expect("bytes to h256")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_binary_upper_boundary_empty() {
        let result = get_binary_upper_boundary(&[]);
        assert_eq!(result, vec![255; 32]);
    }

    #[test]
    fn test_get_binary_upper_boundary_hex_input() {
        let input = hex::decode("b2a8500929d6a1294bf9bf1bf565f549fa4a5f1316a3306ad3d4783e64bcf626")
            .expect("Decoding failed");
        let expected =
            hex::decode("b2a8500929d6a1294bf9bf1bf565f549fa4a5f1316a3306ad3d4783e64bcf627")
                .expect("Decoding failed");
        let result = get_binary_upper_boundary(&input);
        assert_eq!(result, expected);
    }
}
