use super::*;

use ckb_indexer_sync::{CustomFilters, Pool};
use ckb_jsonrpc_types::{IndexerRange, IndexerSearchKeyFilter, IndexerTx};
use ckb_types::{
    H256,
    bytes::Bytes,
    core::{
        BlockBuilder, Capacity, EpochNumberWithFraction, HeaderBuilder, ScriptHashType,
        TransactionBuilder, capacity_bytes,
    },
    packed::{self, CellInput, CellOutputBuilder, OutPoint, Script, ScriptBuilder},
};

use std::sync::{Arc, RwLock};
use tokio::test;

#[test]
async fn test_query_tip() {
    let pool = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexerHandle::new(pool.clone(), None, usize::MAX);
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

#[test]
async fn get_cells() {
    let pool = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexerHandle::new(pool.clone(), None, usize::MAX);
    let res = indexer.get_indexer_tip().await.unwrap();
    assert!(res.is_none());

    insert_blocks(pool.clone()).await;

    let lock_script = ScriptBuilder::default()
        .code_hash(h256!(
            "0x0000000000000000000000000000000000000000000000000000000000000000"
        ))
        .hash_type(ScriptHashType::Data)
        .args(hex::decode("62e907b15cbf27d5425399ebf6f0fb50ebb88f18").expect("Decoding failed"))
        .build();
    let search_key = IndexerSearchKey {
        script: lock_script.into(),
        script_type: IndexerScriptType::Lock,
        script_search_mode: Some(IndexerSearchMode::Exact),
        filter: None,
        with_data: Some(false),
        group_by_transaction: None,
    };
    let cells = indexer
        .get_cells(search_key, IndexerOrder::Asc, 100u32.into(), None)
        .await
        .unwrap();
    assert_eq!(cells.objects.len(), 1);

    let lock_script = ScriptBuilder::default()
        .code_hash(h256!(
            "0x0000000000000000000000000000000000000000000000000000000000000000"
        ))
        .hash_type(ScriptHashType::Data)
        .args(hex::decode("62e907b15cbf").expect("Decoding failed"))
        .build();
    let search_key = IndexerSearchKey {
        script: lock_script.into(),
        script_type: IndexerScriptType::Lock,
        script_search_mode: Some(IndexerSearchMode::Prefix),
        filter: Some(IndexerSearchKeyFilter {
            script_len_range: Some(IndexerRange::new(0, 1)),
            output_data_len_range: Some(IndexerRange::new(0u64, 10u64)),
            output_capacity_range: Some(IndexerRange::new(
                840_000_000_000_000_000_u64,
                840_000_000_100_000_000_u64,
            )),
            block_range: Some(IndexerRange::new(0u64, 10u64)),
            ..Default::default()
        }),
        with_data: Some(false),
        group_by_transaction: None,
    };
    let cells = indexer
        .get_cells(
            search_key,
            IndexerOrder::Asc,
            100u32.into(),
            Some(Into::<packed::Bytes>::into([5u8, 0, 0, 0, 0, 0, 0, 0]).into()),
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

    let type_script = ScriptBuilder::default()
        .code_hash(h256!(
            "0x00000000000000000000000000000000000000000000000000545950455f4944"
        ))
        .hash_type(ScriptHashType::Type)
        .args(
            h256!("0xb2a8500929d6a1294bf9bf1bf565f549fa4a5f1316a3306ad3d4783e64bcf626").as_bytes(),
        )
        .build();
    let lock_script = ScriptBuilder::default()
        .code_hash(h256!(
            "0x0000000000000000000000000000000000000000000000000000000000000000"
        ))
        .hash_type(ScriptHashType::Data)
        .args(vec![].as_slice())
        .build();
    let lock_script_len = extract_raw_data(&lock_script).len() as u64;
    let search_key = IndexerSearchKey {
        script: type_script.into(),
        script_type: IndexerScriptType::Type,
        script_search_mode: Some(IndexerSearchMode::Exact),
        filter: Some(IndexerSearchKeyFilter {
            script: Some(lock_script.into()),
            script_len_range: Some(IndexerRange::new(lock_script_len, lock_script_len + 1)),
            output_capacity_range: Some(IndexerRange::new(
                1_600_000_000_000_u64,
                1_600_100_000_000_u64,
            )),
            block_range: Some(IndexerRange::new(0u64, 1u64)),
            ..Default::default()
        }),
        with_data: Some(false),
        group_by_transaction: None,
    };
    let cells = indexer
        .get_cells(
            search_key,
            IndexerOrder::Asc,
            10u32.into(),
            Some(Into::<packed::Bytes>::into([1u8, 0, 0, 0, 0, 0, 0, 0]).into()),
        )
        .await
        .unwrap();
    assert_eq!(cells.objects.len(), 1);
}

#[test]
async fn get_cells_filter_data() {
    let pool = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexerHandle::new(pool.clone(), None, usize::MAX);
    let res = indexer.get_indexer_tip().await.unwrap();
    assert!(res.is_none());

    insert_blocks(pool.clone()).await;

    let search_key = IndexerSearchKey {
        script: ScriptBuilder::default()
            .code_hash(h256!(
                "0x00000000000000000000000000000000000000000000000000545950455f4944"
            ))
            .hash_type(ScriptHashType::Type)
            .args(
                hex::decode("b2a8500929d6a1294bf9bf1bf565f549fa4a5f1316a3306ad3d4783e64bcf626")
                    .expect("Decoding failed"),
            )
            .build()
            .into(),
        script_type: IndexerScriptType::Type,
        script_search_mode: Some(IndexerSearchMode::Exact),
        filter: Some(IndexerSearchKeyFilter {
            output_data: Some(JsonBytes::from_vec(vec![127, 69, 76])),
            output_data_filter_mode: Some(IndexerSearchMode::Prefix),
            block_range: Some(IndexerRange::new(0u64, u64::MAX)),
            ..Default::default()
        }),
        with_data: Some(false),
        group_by_transaction: None,
    };
    let cells = indexer
        .get_cells(
            search_key,
            IndexerOrder::Asc,
            100u32.into(),
            Some(Into::<packed::Bytes>::into([2u8, 0, 0, 0, 0, 0, 0, 0]).into()),
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

#[test]
async fn get_cells_by_cursor() {
    let pool = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexerHandle::new(pool.clone(), None, usize::MAX);
    let res = indexer.get_indexer_tip().await.unwrap();
    assert!(res.is_none());

    insert_blocks(pool.clone()).await;

    let lock_script = ScriptBuilder::default()
        .code_hash(h256!(
            "0x0000000000000000000000000000000000000000000000000000000000000000"
        ))
        .hash_type(ScriptHashType::Data)
        .args(hex::decode("").expect("Decoding failed"))
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
        .get_cells(
            search_key,
            IndexerOrder::Asc,
            3u32.into(),
            Some(Into::<packed::Bytes>::into([0u8, 0, 0, 0, 0, 0, 0, 0]).into()),
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
        .get_cells(
            search_key,
            IndexerOrder::Asc,
            100u32.into(),
            Some(first_query_cells.last_cursor),
        )
        .await
        .unwrap();

    assert_eq!(second_query_cells.objects.len(), 4);
}

#[test]
async fn get_transactions_ungrouped() {
    let pool = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexerHandle::new(pool.clone(), None, usize::MAX);

    insert_blocks(pool).await;

    let lock_script = ScriptBuilder::default()
        .code_hash(h256!(
            "0x0000000000000000000000000000000000000000000000000000000000000000"
        ))
        .hash_type(ScriptHashType::Data)
        .args(hex::decode("").expect("Decoding failed"))
        .build();

    let search_key = IndexerSearchKey {
        script: lock_script.clone().into(),
        script_type: IndexerScriptType::Lock,
        script_search_mode: Some(IndexerSearchMode::Exact),
        filter: Some(IndexerSearchKeyFilter {
            block_range: Some(IndexerRange::new(0, 1)),
            ..Default::default()
        }),
        with_data: Some(false),
        group_by_transaction: None,
    };
    let txs = indexer
        .get_transactions(search_key, IndexerOrder::Asc, 4u32.into(), None)
        .await
        .unwrap();
    assert_eq!(4, txs.objects.len());

    let search_key = IndexerSearchKey {
        script: lock_script.clone().into(),
        script_type: IndexerScriptType::Lock,
        script_search_mode: Some(IndexerSearchMode::Exact),
        filter: None,
        with_data: Some(false),
        group_by_transaction: None,
    };
    let txs = indexer
        .get_transactions(
            search_key,
            IndexerOrder::Asc,
            4u32.into(),
            Some(txs.last_cursor),
        )
        .await
        .unwrap();
    assert_eq!(3, txs.objects.len());

    let search_key = IndexerSearchKey {
        script: lock_script.clone().into(),
        script_type: IndexerScriptType::Lock,
        script_search_mode: Some(IndexerSearchMode::Exact),
        filter: None,
        with_data: Some(false),
        group_by_transaction: None,
    };
    let txs = indexer
        .get_transactions(search_key, IndexerOrder::Asc, 100u32.into(), None)
        .await
        .unwrap();
    assert_eq!(7, txs.objects.len());

    let lock_script = ScriptBuilder::default()
        .code_hash(h256!(
            "0x709f3fda12f561cfacf92273c57a98fede188a3f1a59b1f888d113f9cce08649"
        ))
        .hash_type(ScriptHashType::Data)
        .args(hex::decode("b73961e46d9eb118d3de1d1e8f30b3af7bbf3160").expect("Decoding failed"))
        .build();
    let search_key = IndexerSearchKey {
        script: lock_script.clone().into(),
        script_type: IndexerScriptType::Lock,
        script_search_mode: Some(IndexerSearchMode::Exact),
        filter: None,
        with_data: Some(false),
        group_by_transaction: None,
    };
    let txs = indexer
        .get_transactions(search_key, IndexerOrder::Asc, 1u32.into(), None)
        .await
        .unwrap();
    assert_eq!(1, txs.objects.len());

    let search_key = IndexerSearchKey {
        script: lock_script.clone().into(),
        script_type: IndexerScriptType::Lock,
        script_search_mode: Some(IndexerSearchMode::Exact),
        filter: None,
        with_data: Some(false),
        group_by_transaction: None,
    };
    let txs = indexer
        .get_transactions(
            search_key,
            IndexerOrder::Asc,
            1u32.into(),
            Some(txs.last_cursor),
        )
        .await
        .unwrap();
    assert_eq!(1, txs.objects.len());

    let search_key = IndexerSearchKey {
        script: lock_script.clone().into(),
        script_type: IndexerScriptType::Lock,
        script_search_mode: Some(IndexerSearchMode::Exact),
        filter: None,
        with_data: Some(false),
        group_by_transaction: None,
    };
    let txs = indexer
        .get_transactions(search_key, IndexerOrder::Asc, 100u32.into(), None)
        .await
        .unwrap();
    assert_eq!(2, txs.objects.len());
}

#[test]
async fn get_transactions_grouped() {
    let pool = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexerHandle::new(pool.clone(), None, usize::MAX);

    insert_blocks(pool).await;

    let lock_script = ScriptBuilder::default()
        .code_hash(h256!(
            "0x0000000000000000000000000000000000000000000000000000000000000000"
        ))
        .hash_type(ScriptHashType::Data)
        .args(hex::decode("").expect("Decoding failed"))
        .build();

    let search_key = IndexerSearchKey {
        script: lock_script.clone().into(),
        script_type: IndexerScriptType::Lock,
        script_search_mode: Some(IndexerSearchMode::Exact),
        filter: Some(IndexerSearchKeyFilter {
            block_range: Some(IndexerRange::new(0, 1)),
            ..Default::default()
        }),
        with_data: Some(false),
        group_by_transaction: Some(true),
    };
    let txs = indexer
        .get_transactions(search_key, IndexerOrder::Asc, 100u32.into(), None)
        .await
        .unwrap();
    assert_eq!(2, txs.objects.len());

    let search_key = IndexerSearchKey {
        script: lock_script.clone().into(),
        script_type: IndexerScriptType::Lock,
        script_search_mode: Some(IndexerSearchMode::Exact),
        filter: Some(IndexerSearchKeyFilter {
            block_range: Some(IndexerRange::new(0u64, u64::MAX)),
            ..Default::default()
        }),
        with_data: Some(false),
        group_by_transaction: Some(true),
    };
    let txs = indexer
        .get_transactions(search_key, IndexerOrder::Asc, 1u32.into(), None)
        .await
        .unwrap();
    assert_eq!(1, txs.objects.len());
    match &txs.objects[0] {
        IndexerTx::Grouped(tx_with_cells) => {
            assert_eq!(5, tx_with_cells.cells.len());
        }
        _ => panic!("unexpected transaction type"),
    }

    let search_key = IndexerSearchKey {
        script: lock_script.clone().into(),
        script_type: IndexerScriptType::Lock,
        script_search_mode: Some(IndexerSearchMode::Exact),
        filter: None,
        with_data: Some(false),
        group_by_transaction: Some(true),
    };
    let txs = indexer
        .get_transactions(
            search_key,
            IndexerOrder::Asc,
            1u32.into(),
            Some(txs.last_cursor),
        )
        .await
        .unwrap();
    assert_eq!(1, txs.objects.len());
    match &txs.objects[0] {
        IndexerTx::Grouped(tx_with_cells) => {
            assert_eq!(2, tx_with_cells.cells.len());
        }
        _ => panic!("unexpected transaction type"),
    }
}

#[test]
async fn get_cells_capacity() {
    let pool = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexerHandle::new(pool.clone(), None, usize::MAX);

    insert_blocks(pool).await;

    let search_key = IndexerSearchKey {
        script: ScriptBuilder::default()
            .code_hash(h256!(
                "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8"
            ))
            .hash_type(ScriptHashType::Type)
            .args(hex::decode("57ccb07be6875f61d93636b0ee11b675494627d2").expect("Decoding failed"))
            .build()
            .into(),
        script_type: IndexerScriptType::Lock,
        script_search_mode: Some(IndexerSearchMode::Exact),
        filter: Some(IndexerSearchKeyFilter {
            script_len_range: Some(IndexerRange::new(0, 1)),
            block_range: Some(IndexerRange::new(0, 1)),
            ..Default::default()
        }),
        with_data: None,
        group_by_transaction: None,
    };

    let capacity = indexer
        .get_cells_capacity(search_key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(839957834700000000, capacity.capacity.value());

    let search_key = IndexerSearchKey {
        script: ScriptBuilder::default()
            .code_hash(h256!(
                "0x00000000000000000000000000000000000000000000000000545950455f4944"
            ))
            .hash_type(ScriptHashType::Type)
            .args(
                hex::decode("500929d6a1294bf9bf1bf565f549fa4a5f1316a3306ad3d4783e64bc")
                    .expect("Decoding failed"),
            )
            .build()
            .into(),
        script_type: IndexerScriptType::Type,
        script_search_mode: Some(IndexerSearchMode::Partial),
        filter: Some(IndexerSearchKeyFilter {
            output_data: Some(JsonBytes::from_vec(vec![127, 69, 76])),
            output_data_filter_mode: Some(IndexerSearchMode::Prefix),
            ..Default::default()
        }),
        with_data: Some(false),
        group_by_transaction: None,
    };
    let capacity = indexer
        .get_cells_capacity(search_key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(1600000000000, capacity.capacity.value());
}

#[test]
async fn rpc() {
    let store = connect_sqlite(MEMORY_DB).await;
    let pool = Arc::new(RwLock::new(Pool::default()));
    let indexer = AsyncRichIndexer::new(store.clone(), None, CustomFilters::new(None, None));
    let rpc = AsyncRichIndexerHandle::new(store, Some(Arc::clone(&pool)), usize::MAX);

    // setup test data
    let lock_script1 = ScriptBuilder::default()
        .code_hash(H256(rand::random()))
        .hash_type(ScriptHashType::Data)
        .args(Bytes::from(b"lock_script1".to_vec()))
        .build();

    let lock_script2 = ScriptBuilder::default()
        .code_hash(H256(rand::random()))
        .hash_type(ScriptHashType::Type)
        .args(Bytes::from(b"lock_script2".to_vec()))
        .build();

    let type_script1 = ScriptBuilder::default()
        .code_hash(H256(rand::random()))
        .hash_type(ScriptHashType::Data)
        .args(Bytes::from(b"type_script1".to_vec()))
        .build();

    let type_script2 = ScriptBuilder::default()
        .code_hash(H256(rand::random()))
        .hash_type(ScriptHashType::Type)
        .args(Bytes::from(b"type_script2".to_vec()))
        .build();

    let cellbase0 = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .witness(Script::default().into_witness())
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(1000))
                .lock(lock_script1.clone())
                .build(),
        )
        .output_data(Bytes::default())
        .build();

    let tx00 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(1000))
                .lock(lock_script1.clone())
                .type_(Some(type_script1.clone()))
                .build(),
        )
        .output_data(Bytes::default())
        .build();

    let tx01 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(2000))
                .lock(lock_script2.clone())
                .type_(Some(type_script2.clone()))
                .build(),
        )
        .output_data(Bytes::default())
        .build();

    let block0 = BlockBuilder::default()
        .transaction(cellbase0)
        .transaction(tx00.clone())
        .transaction(tx01.clone())
        .header(HeaderBuilder::default().number(0).build())
        .build();

    indexer.append(&block0).await.unwrap();

    let (mut pre_tx0, mut pre_tx1, mut pre_block) = (tx00, tx01, block0);
    let total_blocks = 255;
    for i in 1..total_blocks {
        let cellbase = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(i + 1))
            .witness(Script::default().into_witness())
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000))
                    .lock(lock_script1.clone())
                    .build(),
            )
            .output_data(Bytes::from(i.to_string()))
            .build();

        pre_tx0 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(pre_tx0.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000))
                    .lock(lock_script1.clone())
                    .type_(Some(type_script1.clone()))
                    .build(),
            )
            .output_data(Bytes::default())
            .build();

        pre_tx1 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(pre_tx1.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(2000))
                    .lock(lock_script2.clone())
                    .type_(Some(type_script2.clone()))
                    .build(),
            )
            .output_data(Bytes::default())
            .build();

        pre_block = BlockBuilder::default()
            .transaction(cellbase)
            .transaction(pre_tx0.clone())
            .transaction(pre_tx1.clone())
            .header(
                HeaderBuilder::default()
                    .number(pre_block.number() + 1)
                    .parent_hash(pre_block.hash())
                    .epoch(EpochNumberWithFraction::new(
                        pre_block.number() + 1,
                        pre_block.number(),
                        1000,
                    ))
                    .build(),
            )
            .build();

        indexer.append(&pre_block).await.unwrap();
    }

    // test get_tip rpc
    let tip = rpc.get_indexer_tip().await.unwrap().unwrap();
    assert_eq!(Into::<H256>::into(pre_block.hash()), tip.block_hash);
    assert_eq!(pre_block.number(), tip.block_number.value());

    // test get_cells rpc
    let cells_page_1 = rpc
        .get_cells(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                ..Default::default()
            },
            IndexerOrder::Asc,
            150.into(),
            None,
        )
        .await
        .unwrap();
    let cells_page_2 = rpc
        .get_cells(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                with_data: Some(false),
                ..Default::default()
            },
            IndexerOrder::Asc,
            150.into(),
            Some(cells_page_1.last_cursor),
        )
        .await
        .unwrap();

    assert_eq!(
        total_blocks as usize + 1,
        cells_page_1.objects.len() + cells_page_2.objects.len(),
        "total size should be cellbase cells count + 1 (last block live cell)"
    );

    let output_data: packed::Bytes = cells_page_1.objects[10].output_data.clone().unwrap().into();
    assert_eq!(
        output_data.raw_data().to_vec(),
        b"10",
        "block #10 cellbase output_data should be 10"
    );

    assert!(
        cells_page_2.objects[10].output_data.is_none(),
        "cellbase output_data should be none when the params with_data is false"
    );

    let desc_cells_page_1 = rpc
        .get_cells(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                ..Default::default()
            },
            IndexerOrder::Desc,
            150.into(),
            None,
        )
        .await
        .unwrap();

    let desc_cells_page_2 = rpc
        .get_cells(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                ..Default::default()
            },
            IndexerOrder::Desc,
            150.into(),
            Some(desc_cells_page_1.last_cursor),
        )
        .await
        .unwrap();

    assert_eq!(
        total_blocks as usize + 1,
        desc_cells_page_1.objects.len() + desc_cells_page_2.objects.len(),
        "total size should be cellbase cells count + 1 (last block live cell)"
    );
    assert_eq!(
        desc_cells_page_1.objects.first().unwrap().out_point,
        cells_page_2.objects.last().unwrap().out_point
    );

    let filter_cells_page_1 = rpc
        .get_cells(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                filter: Some(IndexerSearchKeyFilter {
                    block_range: Some(IndexerRange::new(100, 200)),
                    ..Default::default()
                }),
                ..Default::default()
            },
            IndexerOrder::Asc,
            60.into(),
            None,
        )
        .await
        .unwrap();

    let filter_cells_page_2 = rpc
        .get_cells(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                filter: Some(IndexerSearchKeyFilter {
                    block_range: Some(IndexerRange::new(100, 200)),
                    ..Default::default()
                }),
                ..Default::default()
            },
            IndexerOrder::Asc,
            60.into(),
            Some(filter_cells_page_1.last_cursor),
        )
        .await
        .unwrap();

    assert_eq!(
        100,
        filter_cells_page_1.objects.len() + filter_cells_page_2.objects.len(),
        "total size should be filtered cellbase cells (100~199)"
    );

    let filter_empty_type_script_cells_page_1 = rpc
        .get_cells(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                filter: Some(IndexerSearchKeyFilter {
                    script_len_range: Some(IndexerRange::new(0, 1)),
                    ..Default::default()
                }),
                ..Default::default()
            },
            IndexerOrder::Asc,
            150.into(),
            None,
        )
        .await
        .unwrap();

    let filter_empty_type_script_cells_page_2 = rpc
        .get_cells(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                filter: Some(IndexerSearchKeyFilter {
                    script_len_range: Some(IndexerRange::new(0, 1)),
                    ..Default::default()
                }),
                ..Default::default()
            },
            IndexerOrder::Asc,
            150.into(),
            Some(filter_empty_type_script_cells_page_1.last_cursor),
        )
        .await
        .unwrap();

    assert_eq!(
        total_blocks as usize,
        filter_empty_type_script_cells_page_1.objects.len()
            + filter_empty_type_script_cells_page_2.objects.len(),
        "total size should be cellbase cells count (empty type script)"
    );

    // test get_transactions rpc
    let txs_page_1 = rpc
        .get_transactions(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                ..Default::default()
            },
            IndexerOrder::Asc,
            500.into(),
            None,
        )
        .await
        .unwrap();
    let txs_page_2 = rpc
        .get_transactions(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                ..Default::default()
            },
            IndexerOrder::Asc,
            500.into(),
            Some(txs_page_1.last_cursor),
        )
        .await
        .unwrap();

    assert_eq!(
        total_blocks as usize * 3 - 1,
        txs_page_1.objects.len() + txs_page_2.objects.len(),
        "total size should be cellbase tx count + total_block * 2 - 1 (genesis block only has one tx)"
    );

    let desc_txs_page_1 = rpc
        .get_transactions(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                ..Default::default()
            },
            IndexerOrder::Desc,
            500.into(),
            None,
        )
        .await
        .unwrap();
    let desc_txs_page_2 = rpc
        .get_transactions(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                ..Default::default()
            },
            IndexerOrder::Desc,
            500.into(),
            Some(desc_txs_page_1.last_cursor),
        )
        .await
        .unwrap();

    assert_eq!(
        total_blocks as usize * 3 - 1,
        desc_txs_page_1.objects.len() + desc_txs_page_2.objects.len(),
        "total size should be cellbase tx count + total_block * 2 - 1 (genesis block only has one tx)"
    );
    assert_eq!(
        desc_txs_page_1.objects.first().unwrap().tx_hash(),
        txs_page_2.objects.last().unwrap().tx_hash()
    );

    let filter_txs_page_1 = rpc
        .get_transactions(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                filter: Some(IndexerSearchKeyFilter {
                    block_range: Some(IndexerRange::new(100, 200)),
                    ..Default::default()
                }),
                ..Default::default()
            },
            IndexerOrder::Asc,
            200.into(),
            None,
        )
        .await
        .unwrap();

    let filter_txs_page_2 = rpc
        .get_transactions(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                filter: Some(IndexerSearchKeyFilter {
                    block_range: Some(IndexerRange::new(100, 200)),
                    ..Default::default()
                }),
                ..Default::default()
            },
            IndexerOrder::Asc,
            200.into(),
            Some(filter_txs_page_1.last_cursor),
        )
        .await
        .unwrap();

    assert_eq!(
        300,
        filter_txs_page_1.objects.len() + filter_txs_page_2.objects.len(),
        "total size should be filtered blocks count * 3 (100~199 * 3)"
    );

    // test get_transactions rpc group by tx hash
    let txs_page_1 = rpc
        .get_transactions(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                group_by_transaction: Some(true),
                ..Default::default()
            },
            IndexerOrder::Asc,
            500.into(),
            None,
        )
        .await
        .unwrap();
    let txs_page_2 = rpc
        .get_transactions(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                group_by_transaction: Some(true),
                ..Default::default()
            },
            IndexerOrder::Asc,
            500.into(),
            Some(txs_page_1.last_cursor),
        )
        .await
        .unwrap();

    assert_eq!(
        total_blocks as usize * 2,
        txs_page_1.objects.len() + txs_page_2.objects.len(),
        "total size should be cellbase tx count + total_block"
    );

    let desc_txs_page_1 = rpc
        .get_transactions(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                group_by_transaction: Some(true),
                ..Default::default()
            },
            IndexerOrder::Desc,
            500.into(),
            None,
        )
        .await
        .unwrap();
    let desc_txs_page_2 = rpc
        .get_transactions(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                group_by_transaction: Some(true),
                ..Default::default()
            },
            IndexerOrder::Desc,
            500.into(),
            Some(desc_txs_page_1.last_cursor),
        )
        .await
        .unwrap();

    assert_eq!(
        total_blocks as usize * 2,
        desc_txs_page_1.objects.len() + desc_txs_page_2.objects.len(),
        "total size should be cellbase tx count + total_block"
    );
    assert_eq!(
        desc_txs_page_1.objects.first().unwrap().tx_hash(),
        txs_page_2.objects.last().unwrap().tx_hash()
    );

    let filter_txs_page_1 = rpc
        .get_transactions(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                group_by_transaction: Some(true),
                filter: Some(IndexerSearchKeyFilter {
                    block_range: Some(IndexerRange::new(100, 200)),
                    ..Default::default()
                }),
                ..Default::default()
            },
            IndexerOrder::Asc,
            150.into(),
            None,
        )
        .await
        .unwrap();

    let filter_txs_page_2 = rpc
        .get_transactions(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                group_by_transaction: Some(true),
                filter: Some(IndexerSearchKeyFilter {
                    block_range: Some(IndexerRange::new(100, 200)),
                    ..Default::default()
                }),
                ..Default::default()
            },
            IndexerOrder::Asc,
            150.into(),
            Some(filter_txs_page_1.last_cursor),
        )
        .await
        .unwrap();

    assert_eq!(
        200,
        filter_txs_page_1.objects.len() + filter_txs_page_2.objects.len(),
        "total size should be filtered blocks count * 2 (100~199 * 2)"
    );

    // test get_cells_capacity rpc
    let capacity = rpc
        .get_cells_capacity(IndexerSearchKey {
            script: lock_script1.clone().into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        1000 * 100000000 * (total_blocks + 1),
        capacity.capacity.value(),
        "cellbases + last block live cell"
    );

    let capacity = rpc
        .get_cells_capacity(IndexerSearchKey {
            script: lock_script2.into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        2000 * 100000000,
        capacity.capacity.value(),
        "last block live cell"
    );

    // test get_cells rpc with tx-pool overlay
    let pool_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new(pre_tx0.hash(), 0), 0))
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(1000))
                .lock(lock_script1.clone())
                .type_(Some(type_script1))
                .build(),
        )
        .output_data(Bytes::default())
        .build();
    pool.write().unwrap().new_transaction(&pool_tx);

    let cells_page_1 = rpc
        .get_cells(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                ..Default::default()
            },
            IndexerOrder::Asc,
            150.into(),
            None,
        )
        .await
        .unwrap();
    let cells_page_2 = rpc
        .get_cells(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                ..Default::default()
            },
            IndexerOrder::Asc,
            150.into(),
            Some(cells_page_1.last_cursor),
        )
        .await
        .unwrap();

    assert_eq!(
        total_blocks as usize,
        cells_page_1.objects.len() + cells_page_2.objects.len(),
        "total size should be cellbase cells count (last block live cell was consumed by a pending tx in the pool)"
    );

    // test get_cells_capacity rpc with tx-pool overlay
    let capacity = rpc
        .get_cells_capacity(IndexerSearchKey {
            script: lock_script1.into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        1000 * 100000000 * total_blocks,
        capacity.capacity.value(),
        "cellbases (last block live cell was consumed by a pending tx in the pool)"
    );
}

#[test]
async fn script_search_mode_rpc() {
    let pool = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexer::new(pool.clone(), None, CustomFilters::new(None, None));
    let rpc = AsyncRichIndexerHandle::new(pool, None, usize::MAX);

    // setup test data
    let lock_script1 = ScriptBuilder::default()
        .code_hash(H256(rand::random()))
        .hash_type(ScriptHashType::Type)
        .args(Bytes::from(b"lock_script1".to_vec()))
        .build();

    let lock_script11 = ScriptBuilder::default()
        .code_hash(lock_script1.code_hash())
        .hash_type(ScriptHashType::Type)
        .args(Bytes::from(b"lock_script11".to_vec()))
        .build();

    let type_script1 = ScriptBuilder::default()
        .code_hash(H256(rand::random()))
        .hash_type(ScriptHashType::Data)
        .args(Bytes::from(b"type_script1".to_vec()))
        .build();

    let type_script11 = ScriptBuilder::default()
        .code_hash(type_script1.code_hash())
        .hash_type(ScriptHashType::Data)
        .args(Bytes::from(b"type_script11".to_vec()))
        .build();

    let cellbase0 = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .witness(Script::default().into_witness())
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(1000))
                .lock(lock_script1.clone())
                .build(),
        )
        .output_data(Bytes::default())
        .build();

    let tx00 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(1000))
                .lock(lock_script1.clone())
                .type_(Some(type_script1.clone()))
                .build(),
        )
        .output_data(Bytes::default())
        .build();

    let tx01 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(2000))
                .lock(lock_script11.clone())
                .type_(Some(type_script11.clone()))
                .build(),
        )
        .output_data(Bytes::default())
        .build();

    let block0 = BlockBuilder::default()
        .transaction(cellbase0)
        .transaction(tx00.clone())
        .transaction(tx01.clone())
        .header(HeaderBuilder::default().number(0).build())
        .build();

    indexer.append(&block0).await.unwrap();

    let (mut pre_tx0, mut pre_tx1, mut pre_block) = (tx00, tx01, block0);
    let total_blocks = 255;
    for i in 1..total_blocks {
        let cellbase = TransactionBuilder::default()
            .input(CellInput::new_cellbase_input(i + 1))
            .witness(Script::default().into_witness())
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000))
                    .lock(lock_script1.clone())
                    .build(),
            )
            .output_data(Bytes::from(i.to_string()))
            .build();

        pre_tx0 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(pre_tx0.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000))
                    .lock(lock_script1.clone())
                    .type_(Some(type_script1.clone()))
                    .build(),
            )
            .output_data(Bytes::default())
            .build();

        pre_tx1 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(pre_tx1.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(2000))
                    .lock(lock_script11.clone())
                    .type_(Some(type_script11.clone()))
                    .build(),
            )
            .output_data(Bytes::default())
            .build();

        pre_block = BlockBuilder::default()
            .transaction(cellbase)
            .transaction(pre_tx0.clone())
            .transaction(pre_tx1.clone())
            .header(
                HeaderBuilder::default()
                    .number(pre_block.number() + 1)
                    .parent_hash(pre_block.hash())
                    .epoch(EpochNumberWithFraction::new(
                        pre_block.number() + 1,
                        pre_block.number(),
                        1000,
                    ))
                    .build(),
            )
            .build();

        indexer.append(&pre_block).await.unwrap();
    }

    // test get_cells rpc with prefix search mode
    let cells = rpc
        .get_cells(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                ..Default::default()
            },
            IndexerOrder::Asc,
            1000.into(),
            None,
        )
        .await
        .unwrap();

    assert_eq!(
        total_blocks as usize + 2,
        cells.objects.len(),
        "total size should be cellbase cells count + 2 (last block live cell: lock_script1 and lock_script11)"
    );

    // test get_cells rpc with exact search mode
    let cells = rpc
        .get_cells(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                script_search_mode: Some(IndexerSearchMode::Exact),
                ..Default::default()
            },
            IndexerOrder::Asc,
            1000.into(),
            None,
        )
        .await
        .unwrap();

    assert_eq!(
        total_blocks as usize + 1,
        cells.objects.len(),
        "total size should be cellbase cells count + 1 (last block live cell: lock_script1)"
    );

    // test get_transactions rpc with exact search mode
    let txs = rpc
        .get_transactions(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                script_search_mode: Some(IndexerSearchMode::Exact),
                ..Default::default()
            },
            IndexerOrder::Asc,
            1000.into(),
            None,
        )
        .await
        .unwrap();

    assert_eq!(
        total_blocks as usize * 3 - 1,
        txs.objects.len(),
        "total size should be cellbase tx count + total_block * 2 - 1 (genesis block only has one tx)"
    );

    // test get_transactions rpc group by tx hash with exact search mode
    let txs = rpc
        .get_transactions(
            IndexerSearchKey {
                script: lock_script1.clone().into(),
                script_search_mode: Some(IndexerSearchMode::Exact),
                group_by_transaction: Some(true),
                ..Default::default()
            },
            IndexerOrder::Asc,
            1000.into(),
            None,
        )
        .await
        .unwrap();

    assert_eq!(
        total_blocks as usize * 2,
        txs.objects.len(),
        "total size should be cellbase tx count + total_block"
    );

    // test get_cells_capacity rpc with exact search mode
    let capacity = rpc
        .get_cells_capacity(IndexerSearchKey {
            script: lock_script1.clone().into(),
            script_search_mode: Some(IndexerSearchMode::Exact),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        1000 * 100000000 * (total_blocks + 1),
        capacity.capacity.value(),
        "cellbases + last block live cell"
    );

    // test get_cells_capacity rpc with prefix search mode (by default)
    let capacity = rpc
        .get_cells_capacity(IndexerSearchKey {
            script: lock_script1.into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        1000 * 100000000 * (total_blocks + 1) + 2000 * 100000000,
        capacity.capacity.value()
    );
}

#[test]
async fn output_data_filter_mode_rpc() {
    let pool = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexer::new(pool.clone(), None, CustomFilters::new(None, None));
    let rpc = AsyncRichIndexerHandle::new(pool, None, usize::MAX);

    // setup test data
    let lock_script1 = ScriptBuilder::default()
        .code_hash(H256(rand::random()))
        .hash_type(ScriptHashType::Type)
        .args(Bytes::from(b"lock_script1".to_vec()))
        .build();

    let lock_script11 = ScriptBuilder::default()
        .code_hash(lock_script1.code_hash())
        .hash_type(ScriptHashType::Type)
        .args(Bytes::from(b"lock_script11".to_vec()))
        .build();

    let type_script1 = ScriptBuilder::default()
        .code_hash(H256(rand::random()))
        .hash_type(ScriptHashType::Data)
        .args(Bytes::from(b"type_script1".to_vec()))
        .build();

    let type_script11 = ScriptBuilder::default()
        .code_hash(type_script1.code_hash())
        .hash_type(ScriptHashType::Data)
        .args(Bytes::from(b"type_script11".to_vec()))
        .build();

    let cellbase0 = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .witness(Script::default().into_witness())
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(1000))
                .lock(lock_script1.clone())
                .build(),
        )
        .output_data(Bytes::default())
        .build();

    let tx00 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(1000))
                .lock(lock_script1.clone())
                .type_(Some(type_script1.clone()))
                .build(),
        )
        .output_data(Bytes::default())
        .build();

    let tx01 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(2000))
                .lock(lock_script11.clone())
                .type_(Some(type_script11.clone()))
                .build(),
        )
        .output_data(hex::decode("62e907b15cbf00aa00bbcc").unwrap())
        .build();

    let block0 = BlockBuilder::default()
        .transaction(cellbase0)
        .transaction(tx00.clone())
        .transaction(tx01.clone())
        .header(HeaderBuilder::default().number(0).build())
        .build();

    indexer.append(&block0).await.unwrap();

    // test get_cells rpc with output_data Prefix search mode
    let cells = rpc
        .get_cells(
            IndexerSearchKey {
                script: lock_script11.clone().into(),
                filter: Some(IndexerSearchKeyFilter {
                    output_data: Some(JsonBytes::from_vec(hex::decode("62").unwrap())),
                    output_data_filter_mode: Some(IndexerSearchMode::Prefix),
                    ..Default::default()
                }),
                ..Default::default()
            },
            IndexerOrder::Asc,
            1000.into(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(1, cells.objects.len(),);

    // test get_cells rpc with output_data Partial search mode
    let cells = rpc
        .get_cells(
            IndexerSearchKey {
                script: lock_script11.clone().into(),
                filter: Some(IndexerSearchKeyFilter {
                    output_data: Some(JsonBytes::from_vec(hex::decode("e907b1").unwrap())),
                    output_data_filter_mode: Some(IndexerSearchMode::Partial),
                    ..Default::default()
                }),
                ..Default::default()
            },
            IndexerOrder::Asc,
            1000.into(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(1, cells.objects.len(),);

    // test get_cells rpc with output_data Partial search mode
    let cells = rpc
        .get_cells(
            IndexerSearchKey {
                script: lock_script11.clone().into(),
                filter: Some(IndexerSearchKeyFilter {
                    output_data: Some(JsonBytes::from_vec(hex::decode("").unwrap())),
                    output_data_filter_mode: Some(IndexerSearchMode::Partial),
                    ..Default::default()
                }),
                ..Default::default()
            },
            IndexerOrder::Asc,
            1000.into(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(1, cells.objects.len(),);

    // test get_cells rpc with output_data Exact search mode
    let cells = rpc
        .get_cells(
            IndexerSearchKey {
                script: lock_script11.clone().into(),
                filter: Some(IndexerSearchKeyFilter {
                    output_data: Some(JsonBytes::from_vec(
                        hex::decode("62e907b15cbf00aa00bbcc").unwrap(),
                    )),
                    output_data_filter_mode: Some(IndexerSearchMode::Exact),
                    ..Default::default()
                }),
                ..Default::default()
            },
            IndexerOrder::Asc,
            1000.into(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(1, cells.objects.len(),);

    // test get_cells_capacity rpc with output_data Prefix search mode
    let cells = rpc
        .get_cells_capacity(IndexerSearchKey {
            script: lock_script11.clone().into(),
            filter: Some(IndexerSearchKeyFilter {
                output_data: Some(JsonBytes::from_vec(
                    hex::decode("62e907b15cbf00aa00bb").unwrap(),
                )),
                output_data_filter_mode: Some(IndexerSearchMode::Prefix),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .unwrap();
    let capacity: u64 = cells.unwrap().capacity.into();
    assert_eq!(200000000000, capacity);

    // test get_cells_capacity rpc with output_data Partial search mode
    let cells = rpc
        .get_cells_capacity(IndexerSearchKey {
            script: lock_script11.clone().into(),
            filter: Some(IndexerSearchKeyFilter {
                output_data: Some(JsonBytes::from_vec(hex::decode("aa00bb").unwrap())),
                output_data_filter_mode: Some(IndexerSearchMode::Partial),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .unwrap();
    let capacity: u64 = cells.unwrap().capacity.into();
    assert_eq!(200000000000, capacity);

    // test get_cells_capacity rpc with output_data Partial search mode
    let cells = rpc
        .get_cells_capacity(IndexerSearchKey {
            script: lock_script11.clone().into(),
            filter: Some(IndexerSearchKeyFilter {
                output_data: Some(JsonBytes::from_vec(hex::decode("").unwrap())),
                output_data_filter_mode: Some(IndexerSearchMode::Partial),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .unwrap();
    let capacity: u64 = cells.unwrap().capacity.into();
    assert_eq!(200000000000, capacity);
}

/// Regression test: when the tx-pool overlay has dead cells AND the search key
/// includes a filter with bound parameters (`filter.output_data`), the SQL
/// `$n` placeholders must match the bind order.
///
/// Before the fix the dead-cell predicates were emitted into the SQL before
/// the filter predicates, but the bind loop emitted filter parameters before
/// dead-cell parameters.  This caused filter values to be bound to dead-cell
/// placeholders and vice-versa, allowing dead cells to leak into results.
#[test]
async fn get_cells_with_pool_overlay_and_filter() {
    let store = connect_sqlite(MEMORY_DB).await;
    let pool = Arc::new(RwLock::new(Pool::default()));
    let indexer = AsyncRichIndexer::new(store.clone(), None, CustomFilters::new(None, None));
    let rpc = AsyncRichIndexerHandle::new(store, Some(Arc::clone(&pool)), usize::MAX);

    // Scripts
    let lock_script = ScriptBuilder::default()
        .code_hash(H256(rand::random()))
        .hash_type(ScriptHashType::Data)
        .args(Bytes::from(b"lock_a".to_vec()))
        .build();

    let type_script = ScriptBuilder::default()
        .code_hash(H256(rand::random()))
        .hash_type(ScriptHashType::Data)
        .args(Bytes::from(b"type_a".to_vec()))
        .build();

    // Block 0: cellbase (unrelated lock) + tx0 (lock_script, data="hello")
    // We need exactly 1 dead cell so that the single dead_cells placeholder
    // ($4) is followed by the filter placeholders ($5-$6).  With the buggy
    // bind order the filter values occupy $4-$5 and the dead hash ends up
    // at $6, causing the dead cell to leak through the NOT-IN.
    let cellbase = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .witness(Script::default().into_witness())
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(1000))
                .lock(ScriptBuilder::default().build())
                .build(),
        )
        .output_data(Bytes::default())
        .build();

    let tx0 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(500))
                .lock(lock_script.clone())
                .type_(Some(type_script.clone()))
                .build(),
        )
        // Dead cell data = [0x02].  With the buggy bind:
        //   $5 ← filter.data_upper = [0x01] (upper boundary of [0x00])
        //   [0x02] >= [0x01] → passes data filter (bug!)
        // With correct bind:
        //   $5 ← filter.data = [0x00]
        //   $6 ← filter.data_upper = [0x01]
        //   but dead cell is excluded by NOT IN first.
        .output_data(Bytes::from(vec![0x02]))
        .build();

    let block0 = BlockBuilder::default()
        .transaction(cellbase)
        .transaction(tx0.clone())
        .header(HeaderBuilder::default().number(0).build())
        .build();

    indexer.append(&block0).await.unwrap();

    // Verify baseline: 1 live cell matching lock_script (Exact mode).
    let cells = rpc
        .get_cells(
            IndexerSearchKey {
                script: lock_script.clone().into(),
                script_search_mode: Some(IndexerSearchMode::Exact),
                ..Default::default()
            },
            IndexerOrder::Asc,
            100.into(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(cells.objects.len(), 1);

    // Spend tx0 output 0 via the pool overlay → 1 dead cell.
    let pool_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new(tx0.hash(), 0), 0))
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(500))
                .lock(lock_script.clone())
                .build(),
        )
        .output_data(Bytes::default())
        .build();
    pool.write().unwrap().new_transaction(&pool_tx);

    // The bug: SQL placeholders are emitted as script → dead_cells → filter,
    // but the bind loop emitted script → filter → dead_cells.
    //
    // With 1 dead cell and filter.output_data (Prefix mode, 2 bound params):
    //   SQL:  $1-$3 (script) + $4 (dead_cells) + $5-$6 (output_data)
    //   Buggy bind: script(3) + filter(2) + dead(1)
    //     $4 ← filter.data  (should be dead_tx_hash)
    //     $5 ← filter.upper (should be filter.data)
    //     $6 ← dead_tx_hash (should be filter.upper)
    //   Dead cell (data=[0x02]) passes all swapped checks:
    //     NOT IN (filter.data=[0x00], 0): tx_hash ≠ [0x00] → passes
    //     data >= $5=[0x01]: [0x02] >= [0x01] → passes
    //     data <  $6=dead_hash(32B): [0x02] < 32-byte-blob → passes
    //   → dead cell leaks through!
    //
    //   Correct bind: script(3) + dead(1) + filter(2)
    //     $4 ← dead_tx_hash → NOT IN excludes dead cell ✓

    // Sanity: pool overlay without filter should exclude the dead cell.
    let cells_no_filter = rpc
        .get_cells(
            IndexerSearchKey {
                script: lock_script.clone().into(),
                script_search_mode: Some(IndexerSearchMode::Exact),
                ..Default::default()
            },
            IndexerOrder::Asc,
            100.into(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        cells_no_filter.objects.len(),
        0,
        "pool overlay without filter should exclude both dead cells"
    );

    // ---- get_cells: pool overlay + filter.output_data (Prefix mode) ----
    let cells = rpc
        .get_cells(
            IndexerSearchKey {
                script: lock_script.clone().into(),
                script_search_mode: Some(IndexerSearchMode::Exact),
                filter: Some(IndexerSearchKeyFilter {
                    output_data: Some(JsonBytes::from_vec(vec![0x00])),
                    output_data_filter_mode: Some(IndexerSearchMode::Prefix),
                    ..Default::default()
                }),
                ..Default::default()
            },
            IndexerOrder::Asc,
            100.into(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        cells.objects.len(),
        0,
        "dead cell must be excluded when filter.output_data is present"
    );

    // ---- get_cells_capacity: pool overlay + filter.output_data ----
    let capacity = rpc
        .get_cells_capacity(IndexerSearchKey {
            script: lock_script.clone().into(),
            script_search_mode: Some(IndexerSearchMode::Exact),
            filter: Some(IndexerSearchKeyFilter {
                output_data: Some(JsonBytes::from_vec(vec![0x00])),
                output_data_filter_mode: Some(IndexerSearchMode::Prefix),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        capacity.is_none(),
        "no live cell with matching output_data after pool overlay (get_cells_capacity)"
    );
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
