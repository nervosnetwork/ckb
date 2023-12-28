use crate::indexer::to_fixed_array;
use crate::store::SQLXPool;

use ckb_indexer_sync::{Error, Pool};
use ckb_jsonrpc_types::{
    IndexerCell, IndexerOrder, IndexerPagination, IndexerSearchKey, JsonBytes, Uint32,
};
use ckb_jsonrpc_types::{IndexerScriptType, IndexerSearchMode, IndexerTip};
use ckb_types::packed::{CellOutputBuilder, OutPointBuilder, ScriptBuilder};
use ckb_types::prelude::*;
use ckb_types::H256;
use sql_builder::{name, name::SqlName, SqlBuilder};
use sqlx::{any::AnyRow, Row};

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
    pub async fn query_indexer_tip(&self) -> Result<Option<IndexerTip>, Error> {
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
    pub async fn query_cells(
        &self,
        search_key: IndexerSearchKey,
        order: IndexerOrder,
        limit: Uint32,
        after: Option<JsonBytes>,
    ) -> Result<IndexerPagination<IndexerCell>, Error> {
        // sub query for script
        let mut query_builder = SqlBuilder::select_from("script");
        query_builder
            .field("script.id")
            .and_where_eq("code_hash", "?")
            .and_where_eq("hash_type", "?");
        match search_key.script_search_mode {
            Some(IndexerSearchMode::Prefix) | None | Some(IndexerSearchMode::Partial) => {
                query_builder.and_where("args LIKE ?");
            }
            Some(IndexerSearchMode::Exact) => {
                query_builder.and_where_eq("args", "?");
            }
        }
        let sql_sub_query = query_builder
            .subquery()
            .map_err(|err| Error::DB(err.to_string()))?;

        // query output
        let mut query_builder = SqlBuilder::select_from("output");
        query_builder
            .field("output.id")
            .field("output.output_index")
            .field("output.capacity")
            .field("lock_script.code_hash AS lock_code_hash")
            .field("lock_script.hash_type AS lock_hash_type")
            .field("lock_script.args AS lock_args")
            .field("type_script.code_hash AS type_code_hash")
            .field("type_script.hash_type AS type_hash_type")
            .field("type_script.args AS type_args")
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
        query_builder.join(&format!("{} script_res", sql_sub_query));
        match search_key.script_type {
            IndexerScriptType::Lock => {
                query_builder.on("output.lock_script_id = script_res.id");
            }
            IndexerScriptType::Type => {
                query_builder.on("output.type_script_id = script_res.id");
            }
        }
        query_builder
            .join("ckb_transaction")
            .on("output.tx_id = ckb_transaction.id")
            .join("block")
            .on("ckb_transaction.block_id = block.id")
            .left()
            .join(name!("script";"lock_script"))
            .on("output.lock_script_id = lock_script.id")   
            .join(name!("script";"type_script"))
            .on("output.type_script_id = type_script.id")
            .join("input")
            .on("output.id = input.output_id");
        query_builder.and_where("input.output_id IS NULL");
        if let Some(after) = after {
            let after = decode_i64(after.as_bytes())?;
            match order {
                IndexerOrder::Asc => query_builder.and_where_gt("output.id", after),
                IndexerOrder::Desc => query_builder.and_where_lt("output.id", after),
            };
        }
        if let Some(ref filter) = search_key.filter {
            if filter.script.is_some() {
                match search_key.script_type {
                    IndexerScriptType::Lock => {
                        query_builder.and_where_eq("type_script.code_hash", "?");
                        query_builder.and_where_eq("type_script.hash_type", "?");
                        query_builder.and_where("type_script.args LIKE ?");
                    }
                    IndexerScriptType::Type => {
                        query_builder.and_where_eq("lock_script.code_hash", "?");
                        query_builder.and_where_eq("lock_script.hash_type", "?");
                        query_builder.and_where("lock_script.args LIKE ?");
                    }
                }
            }
            if let Some(script_len_range) = &filter.script_len_range {
                match search_key.script_type {
                    IndexerScriptType::Lock => {
                        query_builder.and_where_ge(
                            "length(lock_script.code_hash)+1+LENGTH(lock_script.args)",
                            script_len_range.start(),
                        );
                        query_builder.and_where_lt(
                            "length(lock_script.code_hash)+1+LENGTH(lock_script.args)",
                            script_len_range.end(),
                        );
                    }
                    IndexerScriptType::Type => {
                        query_builder.and_where_ge(
                            "LENGTH(type_script.code_hash)+1+LENGTH(type_script.args)",
                            script_len_range.start(),
                        );
                        query_builder.and_where_lt(
                            "LENGTH(type_script.code_hash)+1+LENGTH(type_script.args)",
                            script_len_range.end(),
                        );
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
            if filter.data.is_some() {
                match filter.data_filter_mode {
                    Some(IndexerSearchMode::Prefix) | None | Some(IndexerSearchMode::Partial) => {
                        query_builder.and_where("output.data LIKE ?");
                    }
                    Some(IndexerSearchMode::Exact) => {
                        query_builder.and_where_eq("output.data", "?");
                    }
                }
            }
        }
        match order {
            IndexerOrder::Asc => query_builder.order_by("output.id", false),
            IndexerOrder::Desc => query_builder.order_by("output.id", true),
        };
        query_builder.limit(limit.value());

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
        match search_key.script_search_mode {
            Some(IndexerSearchMode::Prefix) | None => {
                let mut new_args = search_key.script.args.as_bytes().to_vec();
                new_args.push(0x25); // End with %
                query = query.bind(new_args);
            }
            Some(IndexerSearchMode::Exact) => {
                query = query.bind(search_key.script.args.as_bytes());
            }
            Some(IndexerSearchMode::Partial) => {
                let mut new_args = vec![0x25]; // Start with %
                new_args.extend_from_slice(search_key.script.args.as_bytes());
                new_args.push(0x25); // End with %
                query = query.bind(new_args);
            }
        }
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
            if let Some(data) = &filter.data {
                match filter.data_filter_mode {
                    Some(IndexerSearchMode::Prefix) | None => {
                        let mut new_data = data.as_bytes().to_vec();
                        new_data.push(0x25); // End with %
                        query = query.bind(new_data);
                    }
                    Some(IndexerSearchMode::Exact) => {
                        query = query.bind(data.as_bytes());
                    }
                    Some(IndexerSearchMode::Partial) => {
                        let mut new_data = vec![0x25]; // Start with %
                        new_data.extend_from_slice(data.as_bytes());
                        new_data.push(0x25); // End with %
                        query = query.bind(new_data);
                    }
                }
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
}

pub(crate) fn bytes_to_h256(input: &[u8]) -> H256 {
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

pub(crate) fn decode_i64(data: &[u8]) -> Result<i64, Error> {
    if data.len() != 8 {
        return Err(Error::Params(
            "unable to convert from bytes to i64 due to insufficient data in little-endian format"
                .to_string(),
        ));
    }
    Ok(i64::from_le_bytes(to_fixed_array(&data[0..8])))
}
