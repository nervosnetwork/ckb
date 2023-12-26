use crate::store::SQLXPool;
use crate::{AsyncRichIndexer, AsyncRichIndexerHandle};

use ckb_app_config::RichIndexerConfig;
use ckb_indexer_sync::CustomFilters;
use ckb_jsonrpc_types::{
    BlockView as JsonBlockView, IndexerOrder, IndexerScriptType, IndexerSearchKey,
    IndexerSearchMode, ScriptHashType,
};
use ckb_types::packed::ScriptBuilder;
use ckb_types::prelude::*;
use ckb_types::{h160, h256};

const MEMORY_DB: &str = ":memory:";
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
    let indexer = AsyncRichIndexer::new(store, 100, 1000, None, CustomFilters::new(None, None));
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

#[tokio::test]
async fn test_query_tip() {
    let pool = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexerHandle::new(pool.clone(), None);
    let res = indexer.query_indexer_tip().await.unwrap();
    assert!(res.is_none());

    insert_blocks(pool.clone()).await;
    let res = indexer.query_indexer_tip().await.unwrap().unwrap();
    assert_eq!(9, res.block_number.value());
    assert_eq!(
        "953761d56c03bfedf5e70dde0583470383184c41331f709df55d4acab5358640".to_string(),
        res.block_hash.to_string()
    );
}

#[tokio::test]
async fn test_append_block_0() {
    let storage = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexer::new(
        storage.clone(),
        100,
        1000,
        None,
        CustomFilters::new(
            Some("block.header.number.to_uint() >= \"0x0\".to_uint()"),
            None,
        ),
    );

    let data_path = String::from(BLOCK_DIR);
    indexer
        .append(&read_block_view(0, data_path.clone()).into())
        .await
        .unwrap();

    assert_eq!(1, storage.fetch_count("block").await.unwrap());
    assert_eq!(2, storage.fetch_count("ckb_transaction").await.unwrap());
    assert_eq!(12, storage.fetch_count("output").await.unwrap());
    assert_eq!(1, storage.fetch_count("input").await.unwrap());
    assert_eq!(9, storage.fetch_count("script").await.unwrap());

    assert_eq!(
        0,
        storage
            .fetch_count("block_association_proposal")
            .await
            .unwrap()
    );
    assert_eq!(
        0,
        storage
            .fetch_count("block_association_uncle")
            .await
            .unwrap()
    );
    assert_eq!(
        0,
        storage
            .fetch_count("tx_association_header_dep")
            .await
            .unwrap()
    );
    assert_eq!(
        2,
        storage
            .fetch_count("tx_association_cell_dep")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn test_rollback_block_0() {
    let storage = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexer::new(
        storage.clone(),
        100,
        1000,
        None,
        CustomFilters::new(
            Some("block.header.number.to_uint() >= \"0x0\".to_uint()"),
            None,
        ),
    );

    let data_path = String::from(BLOCK_DIR);
    indexer
        .append(&read_block_view(0, data_path.clone()).into())
        .await
        .unwrap();

    indexer.rollback().await.unwrap();

    assert_eq!(0, storage.fetch_count("block").await.unwrap());
    assert_eq!(0, storage.fetch_count("ckb_transaction").await.unwrap());
    assert_eq!(0, storage.fetch_count("output").await.unwrap());
    assert_eq!(0, storage.fetch_count("input").await.unwrap());
    assert_eq!(0, storage.fetch_count("script").await.unwrap());

    assert_eq!(
        0,
        storage
            .fetch_count("block_association_proposal")
            .await
            .unwrap()
    );
    assert_eq!(
        0,
        storage
            .fetch_count("block_association_uncle")
            .await
            .unwrap()
    );
    assert_eq!(
        0,
        storage
            .fetch_count("tx_association_header_dep")
            .await
            .unwrap()
    );
    assert_eq!(
        0,
        storage
            .fetch_count("tx_association_cell_dep")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn test_rollback_block_9() {
    let storage = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexer::new(
        storage.clone(),
        100,
        1000,
        None,
        CustomFilters::new(
            Some("block.header.number.to_uint() >= \"0x0\".to_uint()"),
            None,
        ),
    );
    insert_blocks(storage.clone()).await;

    assert_eq!(15, storage.fetch_count("block").await.unwrap()); // 10 blocks, 5 uncles
    assert_eq!(11, storage.fetch_count("ckb_transaction").await.unwrap());
    assert_eq!(12, storage.fetch_count("output").await.unwrap());
    assert_eq!(1, storage.fetch_count("input").await.unwrap());
    assert_eq!(9, storage.fetch_count("script").await.unwrap());
    assert_eq!(
        0,
        storage
            .fetch_count("block_association_proposal")
            .await
            .unwrap()
    );
    assert_eq!(
        5,
        storage
            .fetch_count("block_association_uncle")
            .await
            .unwrap()
    );
    assert_eq!(
        0,
        storage
            .fetch_count("tx_association_header_dep")
            .await
            .unwrap()
    );
    assert_eq!(
        2,
        storage
            .fetch_count("tx_association_cell_dep")
            .await
            .unwrap()
    );

    indexer.rollback().await.unwrap();

    assert_eq!(12, storage.fetch_count("block").await.unwrap()); // 9 blocks, 3 uncles
    assert_eq!(10, storage.fetch_count("ckb_transaction").await.unwrap());
    assert_eq!(12, storage.fetch_count("output").await.unwrap());
    assert_eq!(1, storage.fetch_count("input").await.unwrap());
    assert_eq!(9, storage.fetch_count("script").await.unwrap());
    assert_eq!(
        0,
        storage
            .fetch_count("block_association_proposal")
            .await
            .unwrap()
    );
    assert_eq!(
        3,
        storage
            .fetch_count("block_association_uncle")
            .await
            .unwrap()
    );
    assert_eq!(
        0,
        storage
            .fetch_count("tx_association_header_dep")
            .await
            .unwrap()
    );
    assert_eq!(
        2,
        storage
            .fetch_count("tx_association_cell_dep")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn test_block_filter_and_rollback_block() {
    let storage = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexer::new(
        storage.clone(),
        100,
        1000,
        None,
        CustomFilters::new(
            Some("block.header.number.to_uint() >= \"0x1\".to_uint()"),
            None,
        ),
    );

    let data_path = String::from(BLOCK_DIR);
    indexer
        .append(&read_block_view(0, data_path.clone()).into())
        .await
        .unwrap();

    assert_eq!(1, storage.fetch_count("block").await.unwrap());
    assert_eq!(0, storage.fetch_count("ckb_transaction").await.unwrap());
    assert_eq!(0, storage.fetch_count("output").await.unwrap());
    assert_eq!(0, storage.fetch_count("input").await.unwrap());
    assert_eq!(0, storage.fetch_count("script").await.unwrap());

    indexer.rollback().await.unwrap();

    assert_eq!(0, storage.fetch_count("block").await.unwrap());
    assert_eq!(0, storage.fetch_count("ckb_transaction").await.unwrap());
    assert_eq!(0, storage.fetch_count("output").await.unwrap());
    assert_eq!(0, storage.fetch_count("input").await.unwrap());
    assert_eq!(0, storage.fetch_count("script").await.unwrap());
}

#[tokio::test]
async fn get_cells() {
    let pool = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexerHandle::new(pool.clone(), None);
    let res = indexer.query_indexer_tip().await.unwrap();
    assert!(res.is_none());

    insert_blocks(pool.clone()).await;

    let search_key = IndexerSearchKey {
        script: ScriptBuilder::default()
            .code_hash(
                h256!("0x709f3fda12f561cfacf92273c57a98fede188a3f1a59b1f888d113f9cce08649").pack(),
            )
            .hash_type((ScriptHashType::Data as u8).into())
            .args(
                h160!("0xb73961e46d9eb118d3de1d1e8f30b3af7bbf3160")
                    .as_bytes()
                    .pack(),
            )
            .build()
            .into(),
        script_type: IndexerScriptType::Lock,
        script_search_mode: Some(IndexerSearchMode::Exact),
        filter: None,
        with_data: Some(false),
        group_by_transaction: None,
    };
    let cells = indexer
        .query_cells(search_key, IndexerOrder::Asc, 100u32.into(), None)
        .await
        .unwrap();
    assert_eq!(1, cells.objects.len());
    let cell = &cells.objects[0];
    assert_eq!(cell.block_number, 0u64.into());
    assert_eq!(cell.tx_index, 0u32.into());
    assert_eq!(cell.out_point.index, 5u32.into());
    assert_eq!(cell.output.type_, None);
    assert_eq!(cell.output_data, None);
}
