use super::*;

use crate::indexer::to_fixed_array;
use crate::store::SQLXPool;

use ckb_indexer_sync::Error;
use ckb_jsonrpc_types::{
    IndexerCell, IndexerOrder, IndexerPagination, IndexerSearchKey, JsonBytes, Uint32,
};
use ckb_jsonrpc_types::{IndexerScriptType, IndexerSearchMode};
use ckb_types::packed::{CellOutputBuilder, OutPointBuilder, ScriptBuilder};
use ckb_types::prelude::*;
use sql_builder::{name, name::SqlName, SqlBuilder};
use sqlx::{any::AnyRow, Row};

impl AsyncRichIndexerHandle {
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

        let mut param_index = 1;

        // sub query for script
        let script_sub_query_sql = build_query_script_sql(
            self.store
                .get_db_type()
                .map_err(|err| Error::DB(err.to_string()))?,
            &search_key.script_search_mode,
            &mut param_index,
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
        match &search_key.script_search_mode {
            Some(IndexerSearchMode::Prefix) | None => {
                query = query
                    .bind(search_key.script.args.as_bytes())
                    .bind(get_binary_upper_boundary(search_key.script.args.as_bytes()));
            }
            Some(IndexerSearchMode::Exact) => {
                query = query.bind(search_key.script.args.as_bytes());
            }
            Some(IndexerSearchMode::Partial) => {
                let new_args = process_bind_data_for_partial_mode(&search_key.script.args);
                query = query.bind(new_args);
            }
        }
        if let Some(filter) = search_key.filter.as_ref() {
            if let Some(script) = filter.script.as_ref() {
                query = query
                    .bind(script.code_hash.as_bytes())
                    .bind(script.hash_type.clone() as i16);
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
                    Some(IndexerSearchMode::Partial) => {
                        let new_data = process_bind_data_for_partial_mode(&data);
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
