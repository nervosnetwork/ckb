use super::*;

use ckb_jsonrpc_types::{IndexerRange, IndexerSearchKeyFilter};
use ckb_types::packed::Script;

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
async fn query_cells() {
    let pool = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexerHandle::new(pool.clone(), None);
    let res = indexer.query_indexer_tip().await.unwrap();
    assert!(res.is_none());

    insert_blocks(pool.clone()).await;

    let lock_script = ScriptBuilder::default()
        .code_hash(
            h256!("0x0000000000000000000000000000000000000000000000000000000000000000").pack(),
        )
        .hash_type((ScriptHashType::Data as u8).into())
        .args(
            h160!("0x62e907b15cbf27d5425399ebf6f0fb50ebb88f18")
                .as_bytes()
                .pack(),
        )
        .build();
    let script_len = extract_raw_data(&lock_script).len() as u64;
    let search_key = IndexerSearchKey {
        script: lock_script.into(),
        script_type: IndexerScriptType::Lock,
        script_search_mode: Some(IndexerSearchMode::Prefix),
        filter: Some(IndexerSearchKeyFilter {
            script: None,
            script_len_range: Some(IndexerRange::new(script_len, script_len + 10)),
            output_data_len_range: Some(IndexerRange::new(0u64, 10u64)),
            output_capacity_range: Some(IndexerRange::new(
                840_000_000_000_000_000_u64,
                840_000_000_100_000_000_u64,
            )),
            block_range: Some(IndexerRange::new(0u64, 10u64)),
            data: None,
            data_filter_mode: None,
        }),
        with_data: Some(false),
        group_by_transaction: None,
    };
    let cells = indexer
        .query_cells(
            search_key,
            IndexerOrder::Asc,
            100u32.into(),
            Some(vec![5u8, 0, 0, 0, 0, 0, 0, 0].pack().into()),
        )
        .await
        .unwrap();

    assert_eq!(cells.objects.len(), 1);
    assert_eq!(
        cells.last_cursor,
        JsonBytes::from_vec(vec![7u8, 0, 0, 0, 0, 0, 0, 0])
    );

    let cell = &cells.objects[0];
    assert_eq!(cell.block_number, 0u64.into());
    assert_eq!(cell.tx_index, 0u32.into());
    assert_eq!(cell.out_point.index, 6u32.into());
    assert_eq!(cell.output.type_, None);
    assert_eq!(cell.output_data, None);
}

#[tokio::test]
async fn query_cells_filter_data() {
    let pool = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexerHandle::new(pool.clone(), None);
    let res = indexer.query_indexer_tip().await.unwrap();
    assert!(res.is_none());

    insert_blocks(pool.clone()).await;

    let search_key = IndexerSearchKey {
        script: ScriptBuilder::default()
            .code_hash(
                h256!("0x00000000000000000000000000000000000000000000000000545950455f4944").pack(),
            )
            .hash_type((ScriptHashType::Type as u8).into())
            .args(
                hex::decode("b2a8500929d6a1294bf9bf1bf565f549fa4a5f1316a3306ad3d4783e64bcf626")
                    .expect("Decoding failed")
                    .pack(),
            )
            .build()
            .into(),
        script_type: IndexerScriptType::Type,
        script_search_mode: Some(IndexerSearchMode::Exact),
        filter: Some(IndexerSearchKeyFilter {
            script: None,
            script_len_range: None,
            output_data_len_range: None,
            output_capacity_range: None,
            block_range: None,
            data: Some(JsonBytes::from_vec(vec![127, 69, 76])),
            data_filter_mode: Some(IndexerSearchMode::Prefix),
        }),
        with_data: Some(false),
        group_by_transaction: None,
    };
    let cells = indexer
        .query_cells(
            search_key,
            IndexerOrder::Asc,
            100u32.into(),
            Some(vec![2u8, 0, 0, 0, 0, 0, 0, 0].pack().into()),
        )
        .await
        .unwrap();

    assert_eq!(cells.objects.len(), 1);
    assert_eq!(
        cells.last_cursor,
        JsonBytes::from_vec(vec![3u8, 0, 0, 0, 0, 0, 0, 0])
    );

    let cell = &cells.objects[0];
    assert_eq!(cell.block_number, 0u64.into());
    assert_eq!(cell.tx_index, 0u32.into());
    assert_eq!(cell.out_point.index, 2u32.into());
    assert_eq!(cell.output_data, None);
}

#[tokio::test]
async fn query_cells_by_cursor() {
    let pool = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexerHandle::new(pool.clone(), None);
    let res = indexer.query_indexer_tip().await.unwrap();
    assert!(res.is_none());

    insert_blocks(pool.clone()).await;

    let lock_script = ScriptBuilder::default()
        .code_hash(
            h256!("0x0000000000000000000000000000000000000000000000000000000000000000").pack(),
        )
        .hash_type((ScriptHashType::Data as u8).into())
        .args(hex::decode("").expect("Decoding failed").pack())
        .build();
    let search_key = IndexerSearchKey {
        script: lock_script.clone().into(),
        script_type: IndexerScriptType::Lock,
        script_search_mode: Some(IndexerSearchMode::Exact),
        filter: None,
        with_data: Some(false),
        group_by_transaction: None,
    };
    let first_query_cells = indexer
        .query_cells(
            search_key,
            IndexerOrder::Asc,
            3u32.into(),
            Some(vec![0u8, 0, 0, 0, 0, 0, 0, 0].pack().into()),
        )
        .await
        .unwrap();

    assert_eq!(first_query_cells.objects.len(), 3);
    assert_eq!(
        first_query_cells.last_cursor,
        JsonBytes::from_vec(vec![3u8, 0, 0, 0, 0, 0, 0, 0])
    );

    // query using last_cursor
    let search_key = IndexerSearchKey {
        script: lock_script.into(),
        script_type: IndexerScriptType::Lock,
        script_search_mode: Some(IndexerSearchMode::Exact),
        filter: None,
        with_data: Some(false),
        group_by_transaction: None,
    };
    let second_query_cells = indexer
        .query_cells(
            search_key,
            IndexerOrder::Asc,
            100u32.into(),
            Some(first_query_cells.last_cursor),
        )
        .await
        .unwrap();

    assert_eq!(second_query_cells.objects.len(), 4);
}

/// helper fn extracts script fields raw data
fn extract_raw_data(script: &Script) -> Vec<u8> {
    [
        script.code_hash().as_slice(),
        script.hash_type().as_slice(),
        &script.args().raw_data(),
    ]
    .concat()
}
