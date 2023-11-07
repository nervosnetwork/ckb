use crate::store::SQLXPool;
use crate::AsyncIndexerRHandle;

use ckb_app_config::IndexerRConfig;

const MEMORY_DB: &str = ":memory:";

async fn connect_sqlite_db(store_path: &str) -> SQLXPool {
    let mut pool = SQLXPool::default();
    let config = IndexerRConfig {
        store: store_path.into(),
        ..Default::default()
    };
    pool.connect(&config).await.unwrap();
    pool
}

#[tokio::test]
async fn test_query_tip() {
    let pool = connect_sqlite_db(MEMORY_DB).await;
    let indexer = AsyncIndexerRHandle::new(pool, None);
    let res = indexer.get_indexer_tip().await.unwrap();
    assert!(res.is_none());
}
