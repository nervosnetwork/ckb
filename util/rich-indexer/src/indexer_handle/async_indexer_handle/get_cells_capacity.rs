use super::*;

use crate::store::SQLXPool;

use ckb_indexer_sync::Error;
use ckb_jsonrpc_types::{IndexerCellsCapacity, IndexerSearchKey};
use ckb_jsonrpc_types::{IndexerScriptType, IndexerSearchMode};
use sql_builder::{name, name::SqlName, SqlBuilder};
use sqlx::Row;

impl AsyncRichIndexerHandle {
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
        match &search_key.script_search_mode {
            Some(IndexerSearchMode::Prefix) | None => {
                query = query
                    .bind(search_key.script.args.as_bytes())
                    .bind(get_binary_upper_boundary(search_key.script.args.as_bytes()));
            }
            Some(IndexerSearchMode::Exact) | Some(IndexerSearchMode::Partial) => {
                query = query.bind(search_key.script.args.as_bytes());
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
                    Some(IndexerSearchMode::Exact) | Some(IndexerSearchMode::Partial) => {
                        query = query.bind(data.as_bytes());
                    }
                }
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
