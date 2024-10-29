mod insert;
mod query;
mod rollback;

use crate::store::SQLXPool;
use crate::{AsyncRichIndexer, AsyncRichIndexerHandle};

use ckb_app_config::RichIndexerConfig;
use ckb_indexer_sync::CustomFilters;
use ckb_jsonrpc_types::{
    BlockView as JsonBlockView, IndexerOrder, IndexerScriptType, IndexerSearchKey,
    IndexerSearchMode, JsonBytes,
};
use ckb_types::h256;
use ckb_types::prelude::*;

const MEMORY_DB: &str = "sqlite://?mode=memory";
const BLOCK_DIR: &str = "./src/tests/data/blocks/";

async fn connect_sqlite(store_path: &str) -> SQLXPool {
    let mut pool = SQLXPool::default();
    let config = RichIndexerConfig {
        store: store_path.into(),
        ..Default::default()
    };
    pool.connect(&config).await.unwrap();
    pool
}

async fn insert_blocks(store: SQLXPool) {
    let data_path = String::from(BLOCK_DIR);
    let indexer = AsyncRichIndexer::new(store, None, CustomFilters::new(None, None));
    for i in 0..10 {
        indexer
            .append(&read_block_view(i, data_path.clone()).into())
            .await
            .unwrap();
    }
}

pub fn read_block_view(number: u64, dir_path: String) -> JsonBlockView {
    let file_name = number.to_string() + ".json";
    let path = dir_path + file_name.as_str();
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}
