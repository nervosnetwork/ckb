use super::*;

use ckb_indexer_sync::{CustomFilters, Pool};
use ckb_jsonrpc_types::{IndexerRange, IndexerSearchKeyFilter, IndexerTx};
use ckb_types::{
    bytes::Bytes,
    core::{
        capacity_bytes, BlockBuilder, Capacity, EpochNumberWithFraction, HeaderBuilder,
        ScriptHashType, TransactionBuilder,
    },
    packed::{self, CellInput, CellOutputBuilder, OutPoint, Script, ScriptBuilder},
    H256,
};

use std::sync::{Arc, RwLock};
use tokio::test;

#[test]
async fn test_query_tip() {
    let pool = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexerHandle::new(pool.clone(), None);
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
    let indexer = AsyncRichIndexerHandle::new(pool.clone(), None);
    let res = indexer.get_indexer_tip().await.unwrap();
    assert!(res.is_none());

    insert_blocks(pool.clone()).await;

    let lock_script = ScriptBuilder::default()
        .code_hash(
            h256!("0x0000000000000000000000000000000000000000000000000000000000000000").pack(),
        )
        .hash_type((ScriptHashType::Data as u8).into())
        .args(
            hex::decode("62e907b15cbf27d5425399ebf6f0fb50ebb88f18")
                .expect("Decoding failed")
                .pack(),
        )
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
        .code_hash(
            h256!("0x0000000000000000000000000000000000000000000000000000000000000000").pack(),
        )
        .hash_type((ScriptHashType::Data as u8).into())
        .args(hex::decode("62e907b15cbf").expect("Decoding failed").pack())
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

    let type_script = ScriptBuilder::default()
        .code_hash(
            h256!("0x00000000000000000000000000000000000000000000000000545950455f4944").pack(),
        )
        .hash_type((ScriptHashType::Type as u8).into())
        .args(
            h256!("0xb2a8500929d6a1294bf9bf1bf565f549fa4a5f1316a3306ad3d4783e64bcf626")
                .as_bytes()
                .pack(),
        )
        .build();
    let lock_script = ScriptBuilder::default()
        .code_hash(
            h256!("0x0000000000000000000000000000000000000000000000000000000000000000").pack(),
        )
        .hash_type((ScriptHashType::Data as u8).into())
        .args(vec![].as_slice().pack())
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
            Some(vec![1u8, 0, 0, 0, 0, 0, 0, 0].pack().into()),
        )
        .await
        .unwrap();
    assert_eq!(cells.objects.len(), 1);
}

#[test]
async fn get_cells_filter_data() {
    let pool = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexerHandle::new(pool.clone(), None);
    let res = indexer.get_indexer_tip().await.unwrap();
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

#[test]
async fn get_cells_by_cursor() {
    let pool = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexerHandle::new(pool.clone(), None);
    let res = indexer.get_indexer_tip().await.unwrap();
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
        .get_cells(
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
    let indexer = AsyncRichIndexerHandle::new(pool.clone(), None);

    insert_blocks(pool).await;

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
        .code_hash(
            h256!("0x709f3fda12f561cfacf92273c57a98fede188a3f1a59b1f888d113f9cce08649").pack(),
        )
        .hash_type((ScriptHashType::Data as u8).into())
        .args(
            hex::decode("b73961e46d9eb118d3de1d1e8f30b3af7bbf3160")
                .expect("Decoding failed")
                .pack(),
        )
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
    let indexer = AsyncRichIndexerHandle::new(pool.clone(), None);

    insert_blocks(pool).await;

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
    let indexer = AsyncRichIndexerHandle::new(pool.clone(), None);

    insert_blocks(pool).await;

    let search_key = IndexerSearchKey {
        script: ScriptBuilder::default()
            .code_hash(
                h256!("0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8").pack(),
            )
            .hash_type((ScriptHashType::Type as u8).into())
            .args(
                hex::decode("57ccb07be6875f61d93636b0ee11b675494627d2")
                    .expect("Decoding failed")
                    .pack(),
            )
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
            .code_hash(
                h256!("0x00000000000000000000000000000000000000000000000000545950455f4944").pack(),
            )
            .hash_type((ScriptHashType::Type as u8).into())
            .args(
                hex::decode("500929d6a1294bf9bf1bf565f549fa4a5f1316a3306ad3d4783e64bc")
                    .expect("Decoding failed")
                    .pack(),
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
    let rpc = AsyncRichIndexerHandle::new(store, Some(Arc::clone(&pool)));

    // setup test data
    let lock_script1 = ScriptBuilder::default()
        .code_hash(H256(rand::random()).pack())
        .hash_type(ScriptHashType::Data.into())
        .args(Bytes::from(b"lock_script1".to_vec()).pack())
        .build();

    let lock_script2 = ScriptBuilder::default()
        .code_hash(H256(rand::random()).pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(b"lock_script2".to_vec()).pack())
        .build();

    let type_script1 = ScriptBuilder::default()
        .code_hash(H256(rand::random()).pack())
        .hash_type(ScriptHashType::Data.into())
        .args(Bytes::from(b"type_script1".to_vec()).pack())
        .build();

    let type_script2 = ScriptBuilder::default()
        .code_hash(H256(rand::random()).pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(b"type_script2".to_vec()).pack())
        .build();

    let cellbase0 = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .witness(Script::default().into_witness())
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(1000).pack())
                .lock(lock_script1.clone())
                .build(),
        )
        .output_data(Default::default())
        .build();

    let tx00 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(1000).pack())
                .lock(lock_script1.clone())
                .type_(Some(type_script1.clone()).pack())
                .build(),
        )
        .output_data(Default::default())
        .build();

    let tx01 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(2000).pack())
                .lock(lock_script2.clone())
                .type_(Some(type_script2.clone()).pack())
                .build(),
        )
        .output_data(Default::default())
        .build();

    let block0 = BlockBuilder::default()
        .transaction(cellbase0)
        .transaction(tx00.clone())
        .transaction(tx01.clone())
        .header(HeaderBuilder::default().number(0.pack()).build())
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
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .build(),
            )
            .output_data(Bytes::from(i.to_string()).pack())
            .build();

        pre_tx0 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(pre_tx0.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .type_(Some(type_script1.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        pre_tx1 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(pre_tx1.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(2000).pack())
                    .lock(lock_script2.clone())
                    .type_(Some(type_script2.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        pre_block = BlockBuilder::default()
            .transaction(cellbase)
            .transaction(pre_tx0.clone())
            .transaction(pre_tx1.clone())
            .header(
                HeaderBuilder::default()
                    .number((pre_block.number() + 1).pack())
                    .parent_hash(pre_block.hash())
                    .epoch(
                        EpochNumberWithFraction::new(
                            pre_block.number() + 1,
                            pre_block.number(),
                            1000,
                        )
                        .pack(),
                    )
                    .build(),
            )
            .build();

        indexer.append(&pre_block).await.unwrap();
    }

    // test get_tip rpc
    let tip = rpc.get_indexer_tip().await.unwrap().unwrap();
    assert_eq!(Unpack::<H256>::unpack(&pre_block.hash()), tip.block_hash);
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

    assert_eq!(total_blocks as usize * 3 - 1, txs_page_1.objects.len() + txs_page_2.objects.len(), "total size should be cellbase tx count + total_block * 2 - 1 (genesis block only has one tx)");

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

    assert_eq!(total_blocks as usize * 3 - 1, desc_txs_page_1.objects.len() + desc_txs_page_2.objects.len(), "total size should be cellbase tx count + total_block * 2 - 1 (genesis block only has one tx)");
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
                .capacity(capacity_bytes!(1000).pack())
                .lock(lock_script1.clone())
                .type_(Some(type_script1).pack())
                .build(),
        )
        .output_data(Default::default())
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
    let rpc = AsyncRichIndexerHandle::new(pool, None);

    // setup test data
    let lock_script1 = ScriptBuilder::default()
        .code_hash(H256(rand::random()).pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(b"lock_script1".to_vec()).pack())
        .build();

    let lock_script11 = ScriptBuilder::default()
        .code_hash(lock_script1.code_hash())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(b"lock_script11".to_vec()).pack())
        .build();

    let type_script1 = ScriptBuilder::default()
        .code_hash(H256(rand::random()).pack())
        .hash_type(ScriptHashType::Data.into())
        .args(Bytes::from(b"type_script1".to_vec()).pack())
        .build();

    let type_script11 = ScriptBuilder::default()
        .code_hash(type_script1.code_hash())
        .hash_type(ScriptHashType::Data.into())
        .args(Bytes::from(b"type_script11".to_vec()).pack())
        .build();

    let cellbase0 = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .witness(Script::default().into_witness())
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(1000).pack())
                .lock(lock_script1.clone())
                .build(),
        )
        .output_data(Default::default())
        .build();

    let tx00 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(1000).pack())
                .lock(lock_script1.clone())
                .type_(Some(type_script1.clone()).pack())
                .build(),
        )
        .output_data(Default::default())
        .build();

    let tx01 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(2000).pack())
                .lock(lock_script11.clone())
                .type_(Some(type_script11.clone()).pack())
                .build(),
        )
        .output_data(Default::default())
        .build();

    let block0 = BlockBuilder::default()
        .transaction(cellbase0)
        .transaction(tx00.clone())
        .transaction(tx01.clone())
        .header(HeaderBuilder::default().number(0.pack()).build())
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
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .build(),
            )
            .output_data(Bytes::from(i.to_string()).pack())
            .build();

        pre_tx0 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(pre_tx0.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(lock_script1.clone())
                    .type_(Some(type_script1.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        pre_tx1 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(pre_tx1.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(2000).pack())
                    .lock(lock_script11.clone())
                    .type_(Some(type_script11.clone()).pack())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        pre_block = BlockBuilder::default()
            .transaction(cellbase)
            .transaction(pre_tx0.clone())
            .transaction(pre_tx1.clone())
            .header(
                HeaderBuilder::default()
                    .number((pre_block.number() + 1).pack())
                    .parent_hash(pre_block.hash())
                    .epoch(
                        EpochNumberWithFraction::new(
                            pre_block.number() + 1,
                            pre_block.number(),
                            1000,
                        )
                        .pack(),
                    )
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

    assert_eq!(total_blocks as usize * 3 - 1, txs.objects.len(), "total size should be cellbase tx count + total_block * 2 - 1 (genesis block only has one tx)");

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
    let rpc = AsyncRichIndexerHandle::new(pool, None);

    // setup test data
    let lock_script1 = ScriptBuilder::default()
        .code_hash(H256(rand::random()).pack())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(b"lock_script1".to_vec()).pack())
        .build();

    let lock_script11 = ScriptBuilder::default()
        .code_hash(lock_script1.code_hash())
        .hash_type(ScriptHashType::Type.into())
        .args(Bytes::from(b"lock_script11".to_vec()).pack())
        .build();

    let type_script1 = ScriptBuilder::default()
        .code_hash(H256(rand::random()).pack())
        .hash_type(ScriptHashType::Data.into())
        .args(Bytes::from(b"type_script1".to_vec()).pack())
        .build();

    let type_script11 = ScriptBuilder::default()
        .code_hash(type_script1.code_hash())
        .hash_type(ScriptHashType::Data.into())
        .args(Bytes::from(b"type_script11".to_vec()).pack())
        .build();

    let cellbase0 = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(0))
        .witness(Script::default().into_witness())
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(1000).pack())
                .lock(lock_script1.clone())
                .build(),
        )
        .output_data(Default::default())
        .build();

    let tx00 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(1000).pack())
                .lock(lock_script1.clone())
                .type_(Some(type_script1.clone()).pack())
                .build(),
        )
        .output_data(Default::default())
        .build();

    let tx01 = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(2000).pack())
                .lock(lock_script11.clone())
                .type_(Some(type_script11.clone()).pack())
                .build(),
        )
        .output_data(hex::decode("62e907b15cbf00aa00bbcc").unwrap().pack())
        .build();

    let block0 = BlockBuilder::default()
        .transaction(cellbase0)
        .transaction(tx00.clone())
        .transaction(tx01.clone())
        .header(HeaderBuilder::default().number(0.pack()).build())
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

    // test get_cells_capacity rpc with output_data Partial search mode
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

/// helper fn extracts script fields raw data
fn extract_raw_data(script: &Script) -> Vec<u8> {
    [
        script.code_hash().as_slice(),
        script.hash_type().as_slice(),
        &script.args().raw_data(),
    ]
    .concat()
}
