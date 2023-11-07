use crate::store::SQLXPool;

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
    let _pool = connect_sqlite_db(MEMORY_DB).await;
}
