use super::*;

use ckb_types::{
    bytes::Bytes,
    core::{
        capacity_bytes, BlockBuilder, Capacity, EpochNumberWithFraction, HeaderBuilder,
        ScriptHashType, TransactionBuilder,
    },
    packed::{CellInput, CellOutputBuilder, OutPoint, Script, ScriptBuilder},
    H256,
};

#[tokio::test]
async fn test_append_block_0() {
    let storage = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexer::new(
        storage.clone(),
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
async fn with_custom_block_filter() {
    let storage = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexer::new(
        storage.clone(),
        None,
        CustomFilters::new(
            Some("block.header.number.to_uint() >= \"0x1\".to_uint()"),
            None,
        ),
    );
    let indexer_handle = AsyncRichIndexerHandle::new(storage, None);

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
    let tip = indexer_handle.get_indexer_tip().await.unwrap().unwrap();
    assert_eq!(0u64, tip.block_number.value());
    assert_eq!(block0.hash(), tip.block_hash.pack());

    let cellbase1 = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(1))
        .witness(Script::default().into_witness())
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(1000).pack())
                .lock(lock_script1.clone())
                .build(),
        )
        .output_data(Default::default())
        .build();

    let tx10 = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new(tx00.hash(), 0), 0))
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(1000).pack())
                .lock(lock_script1.clone())
                .type_(Some(type_script1).pack())
                .build(),
        )
        .output_data(Default::default())
        .build();

    let tx11 = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new(tx01.hash(), 0), 0))
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(2000).pack())
                .lock(lock_script2)
                .type_(Some(type_script2).pack())
                .build(),
        )
        .output_data(Default::default())
        .build();

    let block1 = BlockBuilder::default()
        .transaction(cellbase1)
        .transaction(tx10)
        .transaction(tx11)
        .header(
            HeaderBuilder::default()
                .number(1.pack())
                .parent_hash(block0.hash())
                .epoch(
                    EpochNumberWithFraction::new(block0.number() + 1, block0.number(), 1000).pack(),
                )
                .build(),
        )
        .build();

    indexer.append(&block1).await.unwrap();
    let tip = indexer_handle.get_indexer_tip().await.unwrap().unwrap();
    assert_eq!(1, tip.block_number.value());
    assert_eq!(block1.hash(), tip.block_hash.pack());
    assert_eq!(
        2, // cellbase1, tx10
        indexer_handle
            .get_cells(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    script_type: IndexerScriptType::Lock,
                    ..Default::default()
                },
                IndexerOrder::Asc,
                100u32.into(),
                None
            )
            .await
            .unwrap()
            .objects
            .len()
    );
    assert_eq!(
        2, //cellbase1, tx10(only output)
        indexer_handle
            .get_transactions(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    script_type: IndexerScriptType::Lock,
                    ..Default::default()
                },
                IndexerOrder::Asc,
                100u32.into(),
                None
            )
            .await
            .unwrap()
            .objects
            .len()
    );

    indexer.rollback().await.unwrap();
    let tip = indexer_handle.get_indexer_tip().await.unwrap().unwrap();
    assert_eq!(0, tip.block_number.value());
    assert_eq!(block0.hash(), tip.block_hash.pack());
    assert_eq!(
        0,
        indexer_handle
            .get_cells(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                100u32.into(),
                None
            )
            .await
            .unwrap()
            .objects
            .len()
    );
    assert_eq!(
        0,
        indexer_handle
            .get_transactions(
                IndexerSearchKey {
                    script: lock_script1.into(),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                100u32.into(),
                None
            )
            .await
            .unwrap()
            .objects
            .len()
    );
}

#[tokio::test]
async fn with_custom_cell_filter() {
    let storage = connect_sqlite(MEMORY_DB).await;
    let indexer = AsyncRichIndexer::new(
        storage.clone(),
        None,
        CustomFilters::new(
            None,
            Some(r#"output.type?.args == "0x747970655f73637269707431""#),
        ),
    );
    let indexer_handle = AsyncRichIndexerHandle::new(storage, None);

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
    let tip = indexer_handle.get_indexer_tip().await.unwrap().unwrap();
    assert_eq!(0, tip.block_number.value());
    assert_eq!(block0.hash(), tip.block_hash.pack());
    assert_eq!(
        1, // cellbase1, tx00
        indexer_handle
            .get_cells(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                100u32.into(),
                None
            )
            .await
            .unwrap()
            .objects
            .len()
    );
    assert_eq!(
        1, //tx00
        indexer_handle
            .get_transactions(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    ..Default::default()
                },
                IndexerOrder::Asc,
                100u32.into(),
                None
            )
            .await
            .unwrap()
            .objects
            .len()
    );

    let cellbase1 = TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(1))
        .witness(Script::default().into_witness())
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(1000).pack())
                .lock(lock_script1.clone())
                .build(),
        )
        .output_data(Default::default())
        .build();

    let tx10 = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new(tx00.hash(), 0), 0))
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(1000).pack())
                .lock(lock_script1.clone())
                .type_(Some(type_script1).pack())
                .build(),
        )
        .output_data(Default::default())
        .build();

    let tx11 = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new(tx01.hash(), 0), 0))
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(2000).pack())
                .lock(lock_script2)
                .type_(Some(type_script2).pack())
                .build(),
        )
        .output_data(Default::default())
        .build();

    let block1 = BlockBuilder::default()
        .transaction(cellbase1)
        .transaction(tx10)
        .transaction(tx11)
        .header(
            HeaderBuilder::default()
                .number(1.pack())
                .parent_hash(block0.hash())
                .epoch(
                    EpochNumberWithFraction::new(block0.number() + 1, block0.number(), 1000).pack(),
                )
                .build(),
        )
        .build();

    indexer.append(&block1).await.unwrap();
    let tip = indexer_handle.get_indexer_tip().await.unwrap().unwrap();
    assert_eq!(1, tip.block_number.value());
    assert_eq!(block1.hash(), tip.block_hash.pack());
    assert_eq!(
        1, // tx10
        indexer_handle
            .get_cells(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    script_type: IndexerScriptType::Lock,
                    ..Default::default()
                },
                IndexerOrder::Asc,
                100u32.into(),
                None
            )
            .await
            .unwrap()
            .objects
            .len()
    );
    assert_eq!(
        3, //tx00(input and output), tx10(only output)
        indexer_handle
            .get_transactions(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    script_type: IndexerScriptType::Lock,
                    ..Default::default()
                },
                IndexerOrder::Asc,
                100u32.into(),
                None
            )
            .await
            .unwrap()
            .objects
            .len()
    );

    indexer.rollback().await.unwrap();
    let tip = indexer_handle.get_indexer_tip().await.unwrap().unwrap();
    assert_eq!(0, tip.block_number.value());
    assert_eq!(block0.hash(), tip.block_hash.pack());
    assert_eq!(
        1, // tx00
        indexer_handle
            .get_cells(
                IndexerSearchKey {
                    script: lock_script1.clone().into(),
                    script_type: IndexerScriptType::Lock,
                    ..Default::default()
                },
                IndexerOrder::Asc,
                100u32.into(),
                None
            )
            .await
            .unwrap()
            .objects
            .len()
    );
    assert_eq!(
        1, // tx00
        indexer_handle
            .get_transactions(
                IndexerSearchKey {
                    script: lock_script1.into(),
                    script_type: IndexerScriptType::Lock,
                    ..Default::default()
                },
                IndexerOrder::Asc,
                100u32.into(),
                None
            )
            .await
            .unwrap()
            .objects
            .len()
    );
}
