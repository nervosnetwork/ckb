use super::*;

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
        JsonBytes::from_vec(vec![6u8, 0, 0, 0, 0, 0, 0, 0])
    );
    
    let cell = &cells.objects[0];
    assert_eq!(cell.block_number, 0u64.into());
    assert_eq!(cell.tx_index, 0u32.into());
    assert_eq!(cell.out_point.index, 5u32.into());
    assert_eq!(cell.output.type_, None);
    assert_eq!(cell.output_data, None);
}
