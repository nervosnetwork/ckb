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
use sqlx::Row;

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
        _order: IndexerOrder,
        _limit: Uint32,
        _after: Option<JsonBytes>,
    ) -> Result<IndexerPagination<IndexerCell>, Error> {
        // sub query for script
        let mut query_builder = SqlBuilder::select_from("script");
        query_builder
            .field("script.id")
            .and_where_eq("code_hash", "$1")
            .and_where_eq("hash_type", "$2");
        match search_key.script_search_mode {
            Some(IndexerSearchMode::Prefix) | None => {
                // default is prefix
                query_builder.and_where_like_left("args", "$3");
            }
            Some(IndexerSearchMode::Exact) => {
                query_builder.and_where_eq("args", "$3");
            }
            Some(IndexerSearchMode::Partial) => {
                query_builder.and_where_like_any("args", "$3");
            }
        }
        let sql_sub_query = query_builder
            .subquery()
            .map_err(|err| Error::DB(err.to_string()))?;

        // query output
        let mut query_builder = SqlBuilder::select_from("output");
        query_builder
            .field("ckb_transaction.tx_index")
            .field("ckb_transaction.tx_hash")
            .field("output.output_index")
            .field("output.capacity")
            .field("lock_script.code_hash AS lock_code_hash")
            .field("lock_script.hash_type AS lock_hash_type")
            .field("lock_script.args AS lock_args")
            .field("type_script.code_hash AS type_code_hash")
            .field("type_script.hash_type AS type_hash_type")
            .field("type_script.args AS type_args")
            .field("block.block_number");
        match search_key.with_data {
            Some(true) | None => {
                query_builder.field("data as output_data");
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
            .join(name!("script";"lock_script"))
            .on("output.lock_script_id = lock_script.id")
            .left_outer()
            .join(name!("script";"type_script"))
            .on("output.type_script_id = type_script.id")
            .join("ckb_transaction")
            .on("output.tx_id = ckb_transaction.id")
            .join("block")
            .on("ckb_transaction.block_id = block.id");

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
            .bind(search_key.script.hash_type as i16)
            .bind(search_key.script.args.as_bytes());

        // fetch
        let cells = self
            .store
            .fetch_all(query)
            .await
            .map_err(|err| Error::DB(err.to_string()))
            .map(|rows| {
                rows.iter()
                    .map(|row| {
                        (
                            row.get::<i32, _>("tx_index"),
                            row.get::<Vec<u8>, _>("tx_hash"),
                            row.get::<i32, _>("output_index"),
                            row.get::<i64, _>("capacity"),
                            row.get::<Vec<u8>, _>("lock_code_hash"),
                            row.get::<i16, _>("lock_hash_type"),
                            row.get::<Vec<u8>, _>("lock_args"),
                            row.get::<Option<Vec<u8>>, _>("type_code_hash"),
                            row.get::<Option<i16>, _>("type_hash_type"),
                            row.get::<Option<Vec<u8>>, _>("type_args"),
                            row.get::<Option<Vec<u8>>, _>("output_data"),
                            row.get::<i64, _>("block_number"),
                        )
                    })
                    .collect::<Vec<_>>()
            })?;

        let cells = cells
            .into_iter()
            .map(|row| {
                let out_point = OutPointBuilder::default()
                    .tx_hash(to_fixed_array::<32>(&row.1).pack())
                    .index((row.2 as u32).pack())
                    .build();

                let lock_script = ScriptBuilder::default()
                    .code_hash(to_fixed_array::<32>(&row.4).pack())
                    .hash_type((row.5 as u8).into())
                    .args(row.6.pack())
                    .build();
                let type_script = row.7.as_ref().map(|value| {
                    ScriptBuilder::default()
                        .code_hash(to_fixed_array::<32>(value).pack())
                        .hash_type((row.8.unwrap() as u8).into())
                        .args(row.9.unwrap().pack())
                        .build()
                });
                let output = CellOutputBuilder::default()
                    .capacity((row.3 as u64).pack())
                    .lock(lock_script)
                    .type_(type_script.pack())
                    .build();

                IndexerCell {
                    output: output.into(),
                    output_data: row.10.map(|value| JsonBytes::from_vec(value)),
                    out_point: out_point.into(),
                    block_number: (row.11 as u64).into(),
                    tx_index: (row.0 as u32).into(),
                }
            })
            .collect::<Vec<_>>();

        Ok(IndexerPagination {
            objects: cells,
            last_cursor: JsonBytes::from_vec(vec![]),
        })
    }
}

pub(crate) fn bytes_to_h256(input: &[u8]) -> H256 {
    H256::from_slice(&input[0..32]).expect("bytes to h256")
}

pub(crate) fn sqlx_param_placeholders(range: std::ops::Range<usize>) -> Result<Vec<String>, Error> {
    if range.start == 0 {
        return Err(Error::Params("no valid parameter".to_owned()));
    }
    Ok((1..=range.end)
        .map(|i| format!("${}", i))
        .collect::<Vec<String>>())
}

fn to_fixed_array<const LEN: usize>(input: &[u8]) -> [u8; LEN] {
    assert_eq!(input.len(), LEN);
    let mut list = [0; LEN];
    list.copy_from_slice(input);
    list
}
