use crate::store::SQLXPool;
use crate::{AsyncIndexerR, AsyncIndexerRHandle};

use ckb_app_config::IndexerRConfig;
use ckb_indexer_sync::CustomFilters;
use ckb_jsonrpc_types::BlockView as JsonBlockView;

const MEMORY_DB: &str = ":memory:";
const BLOCK_DIR: &str = "./src/tests/data/blocks/";

async fn connect_sqlite(store_path: &str) -> SQLXPool {
    let mut pool = SQLXPool::default();
    let config = IndexerRConfig {
        store: store_path.into(),
        ..Default::default()
    };
    pool.connect(&config).await.unwrap();
    pool
}

async fn insert_blocks(store: SQLXPool) {
    let data_path = String::from(BLOCK_DIR);
    let indexer = AsyncIndexerR::new(
        store,
        // handle,
        100,
        1000,
        None,
        CustomFilters::new(None, None),
    );
    for i in 0..10 {
        indexer
            .append_block(&read_block_view(i, data_path.clone()).into())
            .await
            .unwrap();
    }
}

pub fn read_block_view(number: u64, dir_path: String) -> JsonBlockView {
    let file_name = number.to_string() + ".json";
    let path = dir_path + file_name.as_str();
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[tokio::test]
async fn test_query_tip() {
    let pool = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncIndexerRHandle::new(pool.clone(), None);
    let res = indexer.get_indexer_tip().await.unwrap();
    assert!(res.is_none());

    insert_blocks(pool.clone()).await;
    let res = indexer.get_indexer_tip().await.unwrap().unwrap();
    assert_eq!(9, res.block_number.value());
    assert_eq!(
        "953761d56c03bfedf5e70dde0583470383184c41331f709df55d4acab5358640".to_string(),
        res.block_hash.to_string()
    );
}
