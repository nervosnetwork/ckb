use super::*;

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
